//! Board definition for the W5500-EVB-Pico2: the single place where
//! peripherals meet pins. See `docs/implementation_plan.md` §4 for the full
//! pin map; peripherals are claimed here as the milestones need them.
//!
//! Reserved by the board itself (W5500 on SPI0): GP16 MISO, GP17 CSn,
//! GP18 SCK, GP19 MOSI, GP20 RSTn, GP21 INTn, GP25 LED.

use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::CORE1;
use embassy_rp::{Peri, Peripherals};

pub struct Board {
    /// Board LED (GP25), core-0 heartbeat.
    pub led: Output<'static>,
    /// Timing-debug pin (GP14): toggled by the RT tick for scope verification.
    pub tick_pin: Output<'static>,
    /// Second core, handed to `spawn_core1`.
    pub core1: Peri<'static, CORE1>,
}

impl Board {
    pub fn new(p: Peripherals) -> Self {
        Self {
            led: Output::new(p.PIN_25, Level::Low),
            tick_pin: Output::new(p.PIN_14, Level::Low),
            core1: p.CORE1,
        }
    }
}
