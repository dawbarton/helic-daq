//! HELIC-DAQ signal generator: PWM-paced DAC output plus laser logging.
//!
//! This is the smallest wired DAC example: core 1 generates and actuates;
//! core 0 owns Ethernet, laser UART and diagnostics. Compare `cbc-rig` to add
//! an ADC, and see "Adding an experiment" in `docs/developer_guide.md`.

// Embedded binaries have no operating-system standard library or conventional
// `main`; the Cortex-M runtime calls the entry function below.
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
mod rt_loop;

use board::{LaserParts, RtAnalog};
use rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

type Store = ParamStore<config::ActiveController, RtAnalog>;
// Fail at compile time rather than advertising more sources than a packet can
// describe. The controller's telemetry is included in this calculation.
const _: () =
    assert!(helic_fw_common::rig::source_count::<RtAnalog>() <= helic_fw_common::rig::MAX_SOURCES);

pub(crate) static LASER_VALUE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static LASER_RANGE_MM: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

// f32 has no portable atomic type on this target, so its exact bit pattern is
// carried by AtomicU32. This is an independent latest-value hand-off.
fn get_laser(out: &mut [u8]) {
    out.copy_from_slice(
        &LASER_VALUE
            .load(core::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
}

// `ExtraParam` adds experiment-specific, read-only state to the discoverable
// registry. It does not add a new protocol message or fixed host index.
const EXTRA_PARAMS: &[ExtraParam] = &[ExtraParam {
    def: ParamDef {
        name: "laser",
        ty: ParamType::F32,
        count: 1,
        writable: false,
    },
    get: get_laser,
}];

#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

// This macro connects hardware interrupt vectors to Embassy driver handlers.
bind_interrupts!(pub struct Irqs {
    UART0_IRQ => uart::InterruptHandler<UART0>;
    PWM_IRQ_WRAP_0 => helic_fw_common::rig::PwmWrapInterruptHandler;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH1>,
        embassy_rp::dma::InterruptHandler<DMA_CH2>,
        embassy_rp::dma::InterruptHandler<DMA_CH3>;
});

// StaticCell provides one-time, heap-free storage with the `'static` lifetime
// required by Embassy tasks. Queue capacities are deliberately fixed.
static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    // `Peripherals` contains one uniquely owned value for every hardware unit.
    // Board::new divides them into bundles which are then moved to their owner.
    let p = embassy_rp::init(Default::default());
    LASER_RANGE_MM.store(
        config::LASER_RANGE_MM.to_bits(),
        core::sync::atomic::Ordering::Relaxed,
    );
    info!(
        "helic-daq {} boot: {}",
        config::EXPERIMENT,
        params::FIRMWARE_BANNER
    );

    let board = board::Board::new(p);
    // Commands travel core 0 -> 1 and records core 1 -> 0 through SPSC queues.
    // Splitting produces one typed endpoint for each core.
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

    // The `move` closure transfers analogue hardware and RT state to core 1;
    // Rust then prevents core 0 from touching them.
    spawn_core1(board.core1, CORE1_STACK.init(CoreStack::new()), move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| {
            spawner.spawn(unwrap!(rt_loop::rt_loop(
                board.analog,
                controller,
                cmd_rx,
                rec_tx
            )))
        });
    });

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(spawner, board.eth, store, rec_rx,)));
        spawner.spawn(unwrap!(blink(board.led)));
        // A disconnected optoNCDT RX line needs the same external 10k pull-up
        // to 3V3 as cbc-rig, preventing UART break interrupts from starving
        // the network executor.
        spawner.spawn(unwrap!(laser_task(board.laser)));
        spawner.spawn(unwrap!(status_task()));
    });
}

#[embassy_executor::task]
async fn core0_main(
    spawner: Spawner,
    eth: EthernetParts,
    store: Store,
    records: shared_rt::RecordConsumer,
) {
    // Network bring-up is asynchronous. While it awaits hardware, unrelated
    // core-0 tasks and the separate core-1 executor continue running.
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
    // `!` is the never type: a server task has no normal return path.
    comms::tcp::control_run(stack, store).await
}

#[embassy_executor::task]
async fn blink(mut led: Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

#[embassy_executor::task]
async fn laser_task(parts: LaserParts) -> ! {
    // The concrete wrapper is local because Embassy tasks cannot be generic;
    // parsing and scaling remain reusable in firmware/common.
    let mut config = uart::Config::default();
    config.baudrate = 921_600;
    let rx = uart::UartRx::new(parts.uart, parts.rx, Irqs, parts.rx_dma, config);
    helic_fw_common::laser::laser_run(rx, &LASER_RANGE_MM, &LASER_VALUE).await
}

#[embassy_executor::task]
async fn status_task() -> ! {
    shared_rt::status_run().await
}
