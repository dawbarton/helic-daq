//! HELIC-DAQ firmware: boots both RP2350 cores under Embassy.
//!
//! Core 1 runs the real-time loop (`rt_loop`): PWM-timed CONVST, BUSY-edge
//! pipeline, generators + controller, DAC output. Core 0 owns host
//! communications: WIZnet Ethernet with a TCP control server (parameter
//! registry, stream control) and a UDP sample streamer, plus the laser UART
//! and a 1 Hz defmt status line.
//!
//! This variant follows `cbc-rig` and adds a PIO-driven SSI encoder owned by
//! core 1. `board.rs` is therefore the main teaching point for the difference.
//! See the encoder notes in `docs/user_guide.md` and `notes.md` before hardware
//! use.

// Embedded firmware has no desktop standard library or conventional `main`.
#![no_std]
#![no_main]

use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_rp::peripherals::{DMA_CH1, DMA_CH2, DMA_CH3, PIO0, UART0};
use embassy_rp::pio;
use embassy_rp::uart;
use embassy_time::Timer;
use heapless::spsc::Queue;
use helic_fw_common::comms;
use helic_fw_common::net;
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::params::{self, ExtraParam, ParamDef, ParamStore};
use helic_fw_common::rt_loop as shared_rt;
use helic_proto::ParamType;
use panic_probe as _;
use static_cell::StaticCell;

mod board;
mod config;
mod rt_loop;

use board::{LaserParts, RtAnalog};
use rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

type Store = ParamStore<config::ActiveController, RtAnalog>;
// Check the complete discovered source count, including controller telemetry.
const _: () =
    assert!(helic_fw_common::rig::source_count::<RtAnalog>() <= helic_fw_common::rig::MAX_SOURCES);

pub(crate) static LASER_VALUE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static LASER_RANGE_MM: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static ENCODER_VALUE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static ENCODER_ERRORS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static ADC_ERRORS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

// These getters serialise independent latest-value atomics into little-endian
// protocol bytes. Floating-point values use their exact u32 bit pattern.
fn get_laser(out: &mut [u8]) {
    out.copy_from_slice(
        &LASER_VALUE
            .load(core::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
}

fn get_encoder(out: &mut [u8]) {
    out.copy_from_slice(
        &ENCODER_VALUE
            .load(core::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
}

fn get_encoder_errors(out: &mut [u8]) {
    out.copy_from_slice(
        &ENCODER_ERRORS
            .load(core::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
}

fn get_adc_errors(out: &mut [u8]) {
    out.copy_from_slice(
        &ADC_ERRORS
            .load(core::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
}

// Extra parameters are discovered by name and need no new protocol messages.
// Keep each ParamDef's type consistent with the four bytes written by getter.
const EXTRA_PARAMS: &[ExtraParam] = &[
    ExtraParam {
        def: ParamDef {
            name: "laser",
            ty: ParamType::F32,
            count: 1,
            writable: false,
        },
        get: get_laser,
    },
    ExtraParam {
        def: ParamDef {
            name: "encoder",
            ty: ParamType::F32,
            count: 1,
            writable: false,
        },
        get: get_encoder,
    },
    ExtraParam {
        def: ParamDef {
            name: "encoder_errors",
            ty: ParamType::U32,
            count: 1,
            writable: false,
        },
        get: get_encoder_errors,
    },
    ExtraParam {
        def: ParamDef {
            name: "adc_errors",
            ty: ParamType::U32,
            count: 1,
            writable: false,
        },
        get: get_adc_errors,
    },
];

/// RP2350 boot image definition, required in flash for the boot ROM.
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(pub struct Irqs {
    // PIO0 belongs to SSI here. The remaining bindings service the laser,
    // sample timing and DMA-backed Ethernet paths.
    UART0_IRQ => uart::InterruptHandler<UART0>;
    PIO0_IRQ_0 => pio::InterruptHandler<PIO0>;
    PWM_IRQ_WRAP_0 => helic_fw_common::rig::PwmWrapInterruptHandler;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH1>,
        embassy_rp::dma::InterruptHandler<DMA_CH2>,
        embassy_rp::dma::InterruptHandler<DMA_CH3>;
});

// StaticCell provides one-time, heap-free storage for indefinitely lived tasks
// and queues. SPSC capacities are fixed to keep memory and timing bounded.
static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    // The peripheral singleton is consumed and divided into ownership bundles.
    let p = embassy_rp::init(Default::default());
    LASER_RANGE_MM.store(
        config::LASER_RANGE_MM.to_bits(),
        core::sync::atomic::Ordering::Relaxed,
    );
    info!("helic-daq firmware boot: {}", params::FIRMWARE_BANNER);

    let b = board::Board::new(p);

    // Commands flow core 0 -> 1; records flow core 1 -> 0. The split endpoints
    // make the single-producer/single-consumer rule explicit in Rust's types.
    let (cmd_tx, cmd_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (rec_tx, rec_rx) = RECORD_QUEUE.init(Queue::new()).split();
    let controller = config::make_controller();
    let store = Store::new(
        cmd_tx,
        config::SAMPLE_RATE,
        config::EXPERIMENT,
        EXTRA_PARAMS,
        &controller,
    );

    // Moving the analogue/encoder bundle and RT endpoints into the closure
    // prevents accidental use from the communications core.
    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| {
            spawner.spawn(unwrap!(rt_loop::rt_loop(
                b.analog, controller, cmd_rx, rec_tx
            )))
        });
    });

    // laser_task requires a pull-up on the optoNCDT RX pin (GP1). Without it
    // the floating line free-runs into a UART framing/break interrupt storm
    // that livelocks core 0; an external 10k pull-up to 3V3 holds the line in
    // the idle (mark) state so a disconnected/quiet sensor just parks in
    // `rx.read().await`. See docs/developer_guide.md known gaps.
    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(spawner, b.eth, store, rec_rx)));
        spawner.spawn(unwrap!(blink(b.led)));
        spawner.spawn(unwrap!(laser_task(b.laser)));
        spawner.spawn(unwrap!(status_task()));
    });
}

/// Brings the network up (async, so it cannot run inside `main`), then
/// spawns the transport-independent servers.
#[embassy_executor::task]
async fn core0_main(
    spawner: Spawner,
    eth: EthernetParts,
    store: Store,
    records: shared_rt::RecordConsumer,
) {
    info!("core0_main: task started");
    // Awaiting network hardware yields core 0 and never affects core-1 timing.
    let stack = net::wiznet::init(spawner, eth, config::MAC_ADDR, config::NET_CONFIG).await;
    spawner.spawn(unwrap!(control_task(stack, store)));
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records)));
    spawner.spawn(unwrap!(comms::beacon::beacon_task(
        stack,
        config::MAC_ADDR,
        config::EXPERIMENT,
    )));
}

#[embassy_executor::task]
async fn control_task(stack: embassy_net::Stack<'static>, store: Store) -> ! {
    // The never return type `!` documents this as an indefinitely lived task.
    comms::tcp::control_run(stack, store).await
}

/// Core 0: heartbeat LED.
#[embassy_executor::task]
async fn blink(mut led: Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

/// Core 0: read the optoNCDT measurement stream and publish the latest
/// distance for the RT loop (single atomic write; at most one sample stale).
#[embassy_executor::task]
async fn laser_task(parts: LaserParts) -> ! {
    let mut uart_config = uart::Config::default();
    uart_config.baudrate = 921_600;
    let rx = uart::UartRx::new(parts.uart, parts.rx, Irqs, parts.rx_dma, uart_config);
    helic_fw_common::laser::laser_run(rx, &LASER_RANGE_MM, &LASER_VALUE).await
}

/// Core 0: 1 Hz diagnostics over defmt.
#[embassy_executor::task]
async fn status_task() -> ! {
    shared_rt::status_run().await
}
