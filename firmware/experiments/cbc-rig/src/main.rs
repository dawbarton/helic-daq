//! HELIC-DAQ firmware: boots both RP2350 cores under Embassy.
//!
//! Core 1 runs the real-time loop (`rt_loop`): PWM-timed CONVST, BUSY-edge
//! pipeline, generators + controller, DAC output. Core 0 owns host
//! communications: WIZnet Ethernet with a TCP control server (parameter
//! registry, stream control) and a UDP sample streamer, plus the laser UART
//! and a 1 Hz defmt status line.
//!
//! This file is intentionally orchestration rather than experiment logic. A
//! new experiment normally changes the concrete parts and task wrappers here,
//! implements `Rig` in `board.rs`, and reuses the runners in `firmware/common`.
//! See "Firmware architecture" and "Adding an experiment" in
//! `docs/developer_guide.md`.

// `no_std` removes the desktop standard library, which is unavailable on the
// microcontroller. `no_main` lets the Cortex-M runtime provide the reset entry.
#![no_std]
#![no_main]

use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_rp::peripherals::{DMA_CH1, DMA_CH2, DMA_CH3, UART0};
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
mod rig;
mod rt_loop;

use board::LaserParts;
use rig::CbcRig;
use rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

type Store = ParamStore<config::ActiveController, CbcRig>;
// This unnamed compile-time assertion fails the build if the chosen rig and
// controller would overflow the fixed protocol source table.
const _: () =
    assert!(helic_fw_common::rig::source_count::<CbcRig>() <= helic_fw_common::rig::MAX_SOURCES);

// Atomics are the only scalar state shared across cores. An AtomicU32 can also
// carry an f32 losslessly by storing its IEEE-754 bit pattern with to_bits().
// Relaxed ordering is sufficient because each value is an independent latest
// reading; the SPSC queues provide ordering for coherent commands and records.
pub(crate) static LASER_VALUE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static LASER_RANGE_MM: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static ADC_ERRORS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn get_laser(out: &mut [u8]) {
    // Protocol values are little-endian bytes. `out` has the four-byte size
    // guaranteed by the matching `ParamDef` below.
    out.copy_from_slice(
        &LASER_VALUE
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

const EXTRA_PARAMS: &[ExtraParam] = &[
    // Extra parameters supplement the common registry without changing the
    // wire protocol. Names, types and access are discovered by the host.
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
    // Embassy turns this declarative list into type-safe interrupt tokens.
    // Bind only peripherals owned by this experiment.
    UART0_IRQ => uart::InterruptHandler<UART0>;
    TIMER0_IRQ_1 => helic_fw_common::time_watchdog::TimeWatchdogHandler;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH1>,
        embassy_rp::dma::InterruptHandler<DMA_CH2>,
        embassy_rp::dma::InterruptHandler<DMA_CH3>;
});

// Embedded async tasks live for the whole firmware run. StaticCell performs a
// one-time initialisation and returns the required `'static` reference without
// a heap allocator. Queue capacities are fixed for the same reason.
static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    // Taking `Peripherals` gives this function unique ownership of every RP2350
    // peripheral. `Board::new` then divides those resources by task and core.
    let p = embassy_rp::init(Default::default());
    LASER_RANGE_MM.store(
        config::LASER_RANGE_MM.to_bits(),
        core::sync::atomic::Ordering::Relaxed,
    );
    info!("helic-daq firmware boot: {}", params::FIRMWARE_BANNER);

    let b = board::Board::new(p);

    // `split` creates a producer and consumer with Rust types that prevent
    // either SPSC endpoint being used from both cores. Commands flow 0 -> 1;
    // sample records flow 1 -> 0.
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

    // `move` transfers ownership of the analogue peripherals, controller and
    // queue endpoints into core 1. Core 0 cannot use them afterwards, which
    // enforces the architecture at compile time.
    // Core 1 runs the loop directly with no executor, so nothing on the core
    // can suspend the tick or pull Embassy scheduling into its hot path.
    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        rt_loop::run(b.rt, controller, cmd_rx, rec_tx)
    });

    // laser_task requires a pull-up on the optoNCDT RX pin (GP1). Without it
    // the floating line free-runs into a UART framing/break interrupt storm
    // that livelocks core 0; an external 10k pull-up to 3V3 holds the line in
    // the idle (mark) state so a disconnected/quiet sensor just parks in
    // `rx.read().await`. See docs/developer_guide.md known gaps.
    //
    // Bounded self-healing for lost embassy-time alarms; see `time_watchdog`.
    helic_fw_common::time_watchdog::start();

    // `Executor::run` never returns. Embassy polls these cooperative async
    // tasks whenever interrupts or timers make progress possible.
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
///
/// Embassy task functions cannot be generic, hence this concrete wrapper
/// around the reusable WIZnet and communications functions.
#[embassy_executor::task]
async fn core0_main(
    spawner: Spawner,
    eth: EthernetParts,
    store: Store,
    records: shared_rt::RecordConsumer,
) {
    info!("core0_main: task started");
    // `.await` yields core 0 while the network initialises; it does not block
    // the independent real-time executor running on core 1.
    let stack = net::wiznet::init(spawner, eth, config::MAC_ADDR, config::NET_CONFIG).await;
    spawner.spawn(unwrap!(control_task(stack, store)));
    #[cfg(not(feature = "diag-no-udp"))]
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records)));
    #[cfg(feature = "diag-no-udp")]
    let _ = records;
    spawner.spawn(unwrap!(comms::beacon::beacon_task(
        stack,
        config::MAC_ADDR,
        config::EXPERIMENT,
    )));
}

#[embassy_executor::task]
async fn control_task(stack: embassy_net::Stack<'static>, store: Store) -> ! {
    // `-> !` is Rust's never type: a server task is expected to run forever.
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
    #[cfg(feature = "diag-no-status-log")]
    {
        core::future::pending::<()>().await;
        unreachable!()
    }
    #[cfg(not(feature = "diag-no-status-log"))]
    shared_rt::status_run().await
}
