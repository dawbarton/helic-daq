//! CBC-DAQ firmware: boots both RP2350 cores under Embassy.
//!
//! Core 0 will own host communications (Ethernet/USB); core 1 owns the
//! real-time loop. At this milestone the drivers are wired (AD7608 + AD5064
//! on shared SPI1, assembled on core 1) and core 1 exercises the full
//! per-tick SPI cost — ADC frame read plus one DAC write — inside the
//! debug-pin window, so the timing budget is scope-measurable before the
//! real loop lands.

#![no_std]
#![no_main]

use cbc_drivers::ad7608::{InputRange, Oversampling};
use cbc_drivers::AnalogIn;
use defmt::{info, unwrap};
use defmt_rtt as _;
use embassy_executor::Executor;
use embassy_rp::block::ImageDef;
use embassy_rp::gpio::Output;
use embassy_rp::multicore::{spawn_core1, Stack};
use embassy_time::{Delay, Duration, Ticker, Timer};
use panic_probe as _;
use static_cell::StaticCell;

mod board;

use board::AnalogParts;

/// RP2350 boot image definition, required in flash for the boot ROM.
#[unsafe(link_section = ".start_block")]
#[used]
pub static IMAGE_DEF: ImageDef = ImageDef::secure_exe();

/// Sample rate placeholder until the preset mechanism lands with the RT loop.
const SAMPLE_RATE_HZ: f32 = 8000.0;

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
        executor1.run(|spawner| spawner.spawn(unwrap!(rt_tick(b.analog))));
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

/// Core 1 placeholder for the real-time loop. Each tick: raise the debug
/// pin, read one ADC frame and write one DAC channel (the dominant fixed
/// costs of the real loop), drop the pin. The high time on GP14 is the
/// current tick cost on a scope. The real loop (milestone 4) replaces the
/// Ticker with PWM-driven CONVST and a BUSY interrupt.
#[embassy_executor::task]
async fn rt_tick(analog: AnalogParts) -> ! {
    let mut rt = analog.build();

    rt.adc.init(
        InputRange::Bipolar5V,
        Oversampling::for_sample_rate(SAMPLE_RATE_HZ),
        &mut Delay,
    );
    if rt.dac.zero_all().is_err() {
        defmt::warn!("DAC zeroing failed");
    }
    info!("core 1: ADC and DAC configured");

    let mut ticker = Ticker::every(Duration::from_hz(8000));
    loop {
        rt.tick_pin.set_high();
        // Software CONVST pulse (t_high ≥ 25 ns: two register writes
        // suffice). The real loop replaces this with a PWM output and waits
        // for BUSY to fall instead of reading immediately.
        rt.adc_convst.set_high();
        rt.adc_convst.set_low();
        let _converting = rt.adc_busy.is_high();
        let frame = rt.adc.read_frame().unwrap_or_default();
        let volts = frame[0] as f32 * rt.adc.scale();
        let _ = rt.dac.write_volts(0, volts);
        rt.tick_pin.set_low();
        ticker.next().await;
    }
}
