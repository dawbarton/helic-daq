//! HELIC-DAQ firmware: boots both RP2350 cores under Embassy.
//!
//! Core 1 runs the real-time loop (`rt_loop`): PWM-timed CONVST, BUSY-edge
//! pipeline, generators + controller, DAC output. Core 0 owns host
//! communications: W5500 Ethernet with a TCP control server (parameter
//! registry, stream control) and a UDP sample streamer, plus the laser UART
//! and a 1 Hz defmt status line.

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
use helic_fw_common::comms::{self, EthernetParts, StaticNetConfig};
use helic_fw_common::params::{self, ParamStore};
use helic_fw_common::rt_loop as shared_rt;
use panic_probe as _;
use static_cell::StaticCell;

mod board;
mod config;
mod rt_loop;

use board::LaserParts;
use rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

type Store = ParamStore<config::ActiveController>;

/// RP2350 boot image definition, required in flash for the boot ROM.
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(pub struct Irqs {
    UART0_IRQ => uart::InterruptHandler<UART0>;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<DMA_CH1>,
        embassy_rp::dma::InterruptHandler<DMA_CH2>,
        embassy_rp::dma::InterruptHandler<DMA_CH3>;
});

static CORE1_STACK: StaticCell<CoreStack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!("helic-daq firmware boot: {}", params::FIRMWARE_VERSION);

    let b = board::Board::new(p);

    let (cmd_tx, cmd_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (rec_tx, rec_rx) = RECORD_QUEUE.init(Queue::new()).split();

    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| spawner.spawn(unwrap!(rt_loop::rt_loop(b.analog, cmd_rx, rec_tx))));
    });

    // laser_task requires a pull-up on the optoNCDT RX pin (GP1). Without it
    // the floating line free-runs into a UART framing/break interrupt storm
    // that livelocks core 0; an external 10k pull-up to 3V3 holds the line in
    // the idle (mark) state so a disconnected/quiet sensor just parks in
    // `rx.read().await`. See docs/developer_guide.md known gaps.
    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(
            spawner,
            b.eth,
            Store::new(cmd_tx, config::SAMPLE_RATE),
            rec_rx
        )));
        spawner.spawn(unwrap!(blink(b.led)));
        spawner.spawn(unwrap!(laser_task(b.laser)));
        spawner.spawn(unwrap!(status_task()));
    });
}

/// Brings the network up (async, so it cannot run inside `main`), then
/// spawns the servers.
#[embassy_executor::task]
async fn core0_main(
    spawner: Spawner,
    eth: EthernetParts,
    store: Store,
    records: shared_rt::RecordConsumer,
) {
    info!("core0_main: task started");
    let stack = comms::init(
        spawner,
        eth,
        StaticNetConfig {
            mac: config::MAC_ADDR,
            addr: config::IP_ADDR,
            prefix: config::IP_PREFIX,
        },
    )
    .await;
    spawner.spawn(unwrap!(control_task(stack, store)));
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records)));
}

#[embassy_executor::task]
async fn control_task(stack: embassy_net::Stack<'static>, store: Store) -> ! {
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
    helic_fw_common::laser::laser_run(rx, config::LASER_RANGE_MM, &shared_rt::LASER_VALUE).await
}

/// Core 0: 1 Hz diagnostics over defmt.
#[embassy_executor::task]
async fn status_task() -> ! {
    shared_rt::status_run().await
}
