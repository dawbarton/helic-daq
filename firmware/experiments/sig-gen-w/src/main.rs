//! HELIC-DAQ Pico 2W signal generator: Wi-Fi, DAC output and laser logging.
//!
//! Core 1 owns the SRAM-resident DAC loop; core 0 owns the CYW43439 transport,
//! laser receiver and radio-controlled LED. Network-facing tasks consume the
//! same `embassy_net::Stack` as wired experiments.

// The target has no operating-system standard library or conventional entry.
#![no_std]
#![no_main]

use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_rp::peripherals::{DMA_CH0, DMA_CH1, PIO1, UART0};
use embassy_rp::pio;
use embassy_rp::uart;
use embassy_time::Timer;
use heapless::spsc::Queue;
use helic_fw_common::comms;
use helic_fw_common::net;
use helic_fw_common::net::cyw43::WifiParts;
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
// Reject an over-large discovered source table during compilation.
const _: () =
    assert!(helic_fw_common::rig::source_count::<RtAnalog>() <= helic_fw_common::rig::MAX_SOURCES);

pub(crate) static LASER_VALUE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub(crate) static LASER_RANGE_MM: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

// AtomicU32 carries an f32's exact bits between cores without locking.
fn get_laser(out: &mut [u8]) {
    out.copy_from_slice(
        &LASER_VALUE
            .load(core::sync::atomic::Ordering::Relaxed)
            .to_le_bytes(),
    );
}

// Extra values extend the discovered registry, not the wire protocol.
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

// PIO1 and DMA0 belong to the CYW43439 backend; the remaining handlers serve
// the laser UART and hardware sample clock.
bind_interrupts!(pub struct Irqs {
    UART0_IRQ => uart::InterruptHandler<UART0>;
    PIO1_IRQ_0 => pio::InterruptHandler<PIO1>;
    TIMER0_IRQ_1 => helic_fw_common::time_watchdog::TimeWatchdogHandler;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH0>,
        embassy_rp::dma::InterruptHandler<DMA_CH1>;
});

// StaticCell supplies permanent task and queue storage without a heap.
static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    // Board::new consumes every peripheral once and groups it by core/task.
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
    // Commands flow to core 1; non-blocking sample records flow back to core 0.
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

    // `move` gives the RT core exclusive ownership of its hardware and state.
    spawn_core1(board.core1, CORE1_STACK.init(CoreStack::new()), move || {
        rt_loop::run(board.analog, controller, cmd_rx, rec_tx)
    });

    helic_fw_common::time_watchdog::start();

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(spawner, board.wifi, store, rec_rx,)));
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
    wifi: WifiParts,
    store: Store,
    records: shared_rt::RecordConsumer,
) {
    // Radio initialisation joins the access point and returns the same network
    // stack abstraction used by wired experiments, plus LED control and MAC.
    let (stack, control, mac) = net::cyw43::init(
        spawner,
        wifi,
        Irqs,
        Irqs,
        config::WIFI_SSID,
        config::WIFI_PASSWORD,
        config::NET_CONFIG,
    )
    .await;
    spawner.spawn(unwrap!(blink(control)));
    spawner.spawn(unwrap!(control_task(stack, store)));
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records)));
    spawner.spawn(unwrap!(comms::beacon::beacon_task(
        stack,
        mac,
        config::EXPERIMENT,
    )));
}

#[embassy_executor::task]
async fn control_task(stack: embassy_net::Stack<'static>, store: Store) -> ! {
    // `!` is the never type used for a task intended to run indefinitely.
    comms::tcp::control_run(stack, store).await
}

#[embassy_executor::task]
async fn blink(mut control: cyw43::Control<'static>) -> ! {
    // Pico 2W's LED is attached to the radio GPIO, not an RP2350 GPIO pin.
    let mut on = false;
    loop {
        on = !on;
        control.gpio_set(0, on).await;
        Timer::after_millis(500).await;
    }
}

#[embassy_executor::task]
async fn laser_task(parts: LaserParts) -> ! {
    let mut config = uart::Config::default();
    config.baudrate = 921_600;
    let rx = uart::UartRx::new(parts.uart, parts.rx, Irqs, parts.rx_dma, config);
    helic_fw_common::laser::laser_run(rx, &LASER_RANGE_MM, &LASER_VALUE).await
}

#[embassy_executor::task]
async fn status_task() -> ! {
    shared_rt::status_run().await
}
