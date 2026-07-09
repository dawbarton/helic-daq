//! CBC-DAQ firmware: boots both RP2350 cores under Embassy.
//!
//! Core 1 runs the real-time loop (`rt_loop`): PWM-timed CONVST, BUSY-edge
//! pipeline, generators + controller, DAC output, stream records out through
//! a lock-free ring. Core 0 owns everything else: the laser UART, and (next
//! milestone) Ethernet host communications. Until then a status task logs
//! the loop diagnostics once a second and a boot-time demo configures a
//! 10 Hz, 1 V sine on the output — an out-of-the-box hardware smoke test.

#![no_std]
#![no_main]

use cbc_core::generator::FourierCoeffs;
use cbc_core::phase::PhaseAccumulator;
use cbc_drivers::optoncdt::{DistanceScale, Parser, Reading};
use core::sync::atomic::Ordering;
use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::Executor;
use embassy_rp::bind_interrupts;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack};
use embassy_rp::peripherals::UART0;
use embassy_rp::uart;
use embassy_time::{Duration, Ticker, Timer};
use heapless::spsc::Queue;
use panic_probe as _;
use static_cell::StaticCell;

mod board;
mod config;
mod rt_loop;

use board::LaserParts;
use config::{HARMONICS, SAMPLE_RATE};
use rt_loop::{
    CommandProducer, Record, RecordConsumer, RtCommand, COMMAND_QUEUE_LEN, RECORD_QUEUE_LEN,
};

/// RP2350 boot image definition, required in flash for the boot ROM.
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

bind_interrupts!(struct Irqs {
    UART0_IRQ => uart::InterruptHandler<UART0>;
    DMA_IRQ_0 => embassy_rp::dma::InterruptHandler<embassy_rp::peripherals::DMA_CH1>;
});

static CORE1_STACK: StaticCell<Stack<16384>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();
static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!("cbc-daq firmware boot");

    let b = board::Board::new(p);

    let (cmd_tx, cmd_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (rec_tx, rec_rx) = RECORD_QUEUE.init(Queue::new()).split();

    spawn_core1(b.core1, CORE1_STACK.init(Stack::new()), move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| spawner.spawn(unwrap!(rt_loop::rt_loop(b.analog, cmd_rx, rec_tx))));
    });

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        spawner.spawn(unwrap!(blink(b.led)));
        spawner.spawn(unwrap!(laser_task(b.laser)));
        spawner.spawn(unwrap!(status_task(cmd_tx, rec_rx)));
    });
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

/// Core 0: placeholder for host comms. Configures the boot-time demo signal,
/// then drains stream records and logs the loop diagnostics once a second.
#[embassy_executor::task]
async fn status_task(mut commands: CommandProducer, mut records: RecordConsumer) -> ! {
    // Demo: 10 Hz, 1 V (sin) forcing on the output channel from boot.
    let mut forcing = FourierCoeffs::<HARMONICS>::zero();
    forcing.b[0] = 1.0;
    let increment = PhaseAccumulator::increment_for(10.0, SAMPLE_RATE.hz() as f64);
    let _ = commands.enqueue(RtCommand::SetIncrement(increment));
    let _ = commands.enqueue(RtCommand::SetForcingCoeffs(forcing));

    let mut ticker = Ticker::every(Duration::from_secs(1));
    let mut last_record = Record::default();
    loop {
        ticker.next().await;

        let mut drained: u32 = 0;
        while let Some(r) = records.dequeue() {
            last_record = r;
            drained += 1;
        }

        info!(
            "ticks {} | rec/s {} | loop {}/{} us | jitter {} us | overruns {} | busy timeouts {} | dropped {} | rec[{}]: adc0 {} V target {} forcing {} out {} V laser {} mm",
            rt_loop::TICKS.load(Ordering::Relaxed),
            drained,
            rt_loop::LOOP_TIME_LAST_US.load(Ordering::Relaxed),
            rt_loop::LOOP_TIME_MAX_US.load(Ordering::Relaxed),
            rt_loop::CLOCK_JITTER_US.load(Ordering::Relaxed),
            rt_loop::OVERRUNS.load(Ordering::Relaxed),
            rt_loop::BUSY_TIMEOUTS.load(Ordering::Relaxed),
            rt_loop::RECORDS_DROPPED.load(Ordering::Relaxed),
            last_record.index,
            last_record.adc[0],
            last_record.target,
            last_record.forcing,
            last_record.out,
            last_record.laser,
        );
    }
}
