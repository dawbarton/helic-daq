//! CBC-DAQ firmware: boots both RP2350 cores under Embassy.
//!
//! Core 1 runs the real-time loop (`rt_loop`): PWM-timed CONVST, BUSY-edge
//! pipeline, generators + controller, DAC output. Core 0 owns host
//! communications: W5500 Ethernet with a TCP control server (parameter
//! registry, stream control) and a UDP sample streamer, plus the laser UART
//! and a 1 Hz defmt status line.

#![no_std]
#![no_main]

use cbc_drivers::optoncdt::{DistanceScale, Parser, Reading};
use core::sync::atomic::Ordering;
use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::{Executor, Spawner};
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack as CoreStack};
use embassy_rp::peripherals::{DMA_CH1, DMA_CH2, DMA_CH3, UART0};
use embassy_rp::uart;
use embassy_time::{Duration, Ticker, Timer};
use heapless::spsc::Queue;
use panic_probe as _;
use static_cell::StaticCell;

mod board;
mod comms;
mod config;
mod params;
mod rt_loop;

use board::LaserParts;
use params::ParamStore;
use rt_loop::{Record, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN};

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
    info!("cbc-daq firmware boot: {}", params::FIRMWARE_VERSION);

    let b = board::Board::new(p);

    let (cmd_tx, cmd_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (rec_tx, rec_rx) = RECORD_QUEUE.init(Queue::new()).split();

    spawn_core1(b.core1, CORE1_STACK.init(CoreStack::new()), move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| spawner.spawn(unwrap!(rt_loop::rt_loop(b.analog, cmd_rx, rec_tx))));
    });

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(core0_main(
            spawner,
            b.eth,
            ParamStore::new(cmd_tx),
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
    eth: board::EthParts,
    store: ParamStore,
    records: rt_loop::RecordConsumer,
) {
    let stack = comms::init(spawner, eth).await;
    spawner.spawn(unwrap!(comms::tcp::control_task(stack, store)));
    spawner.spawn(unwrap!(comms::udp::stream_task(stack, records)));
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
    let mut rx = uart::UartRx::new(parts.uart, parts.rx, Irqs, parts.rx_dma, uart_config);

    let mut parser = Parser::new();
    let scale = DistanceScale::new(config::LASER_RANGE_MM);
    let mut buf = [0u8; 3];
    loop {
        if rx.read(&mut buf).await.is_err() {
            // Break/overrun: parser resynchronises on the flag bits.
            continue;
        }
        for byte in buf {
            let Some(v) = parser.push(byte) else { continue };
            // With factory data selection the first output value is distance.
            if v.first {
                if let Reading::InRange(mm) = scale.convert(v.value) {
                    rt_loop::LASER_VALUE.store(mm.to_bits(), Ordering::Relaxed);
                }
            }
        }
    }
}

/// Core 0: 1 Hz diagnostics over defmt.
#[embassy_executor::task]
async fn status_task() -> ! {
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        ticker.next().await;
        info!(
            "ticks {} | loop {}/{} us | jitter {} us | overruns {} | busy timeouts {} | dropped {} | laser {} mm",
            rt_loop::TICKS.load(Ordering::Relaxed),
            rt_loop::LOOP_TIME_LAST_US.load(Ordering::Relaxed),
            rt_loop::LOOP_TIME_MAX_US.load(Ordering::Relaxed),
            rt_loop::CLOCK_JITTER_US.load(Ordering::Relaxed),
            rt_loop::OVERRUNS.load(Ordering::Relaxed),
            rt_loop::BUSY_TIMEOUTS.load(Ordering::Relaxed),
            rt_loop::RECORDS_DROPPED.load(Ordering::Relaxed),
            f32::from_bits(rt_loop::LASER_VALUE.load(Ordering::Relaxed)),
        );
    }
}
