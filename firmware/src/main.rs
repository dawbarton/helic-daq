//! CBC-DAQ firmware skeleton: boots both RP2350 cores under Embassy.
//!
//! Core 0 will own host communications (Ethernet/USB); core 1 will own the
//! real-time control loop. In this milestone core 0 blinks the board LED and
//! core 1 runs a placeholder tick task that toggles the timing-debug pin at
//! the sample rate, so loop timing can be verified on a scope from day one.

#![no_std]
#![no_main]

use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::Executor;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack};
use embassy_time::{Duration, Ticker, Timer};
use panic_probe as _;
use static_cell::StaticCell;

mod board;

/// RP2350 boot image definition, required in flash for the boot ROM.
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

static CORE1_STACK: StaticCell<Stack<8192>> = StaticCell::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());
    info!("cbc-daq firmware boot");

    let b = board::Board::new(p);

    spawn_core1(b.core1, CORE1_STACK.init(Stack::new()), move || {
        let executor1 = EXECUTOR1.init(Executor::new());
        executor1.run(|spawner| spawner.spawn(unwrap!(rt_tick(b.tick_pin))));
    });

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| spawner.spawn(unwrap!(blink(b.led))));
}

/// Core 0 placeholder: heartbeat LED.
#[embassy_executor::task]
async fn blink(mut led: Output<'static>) -> ! {
    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

/// Core 1 placeholder for the real-time loop: toggles the debug pin at the
/// sample rate so tick timing is observable on a scope. The real loop
/// (milestone 4) replaces the Ticker with PWM-driven CONVST + BUSY interrupt.
#[embassy_executor::task]
async fn rt_tick(mut tick_pin: Output<'static>) -> ! {
    let mut ticker = Ticker::every(Duration::from_hz(8000));
    loop {
        tick_pin.toggle();
        ticker.next().await;
    }
}
