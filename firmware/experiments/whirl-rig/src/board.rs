//! Auditable pin map and peripheral ownership for the wired whirl experiment.
//!
//! WIZnet reserves GP16–21 and GP25. Whirl assigns GP22 to the shared SSI
//! clock, GP26/27 to pitch/yaw SSI data, GP28 to the optical pulse, and GP14
//! to tick timing. Sensor behaviour and PIO assembly live in `rig.rs`.

use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{CORE1, PIN_22, PIN_26, PIN_27, PIN_28, PIO0, PWM_SLICE4, SPI0};
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::{Peri, Peripherals};
use helic_fw_common::net::wiznet::EthernetParts;

pub struct Board {
    pub led: Output<'static>,
    pub rt: WhirlParts,
    pub eth: EthernetParts,
    pub core1: Peri<'static, CORE1>,
}

/// Unassembled resources moved as one value to core 1.
pub struct WhirlParts {
    pub(crate) tick_pin: Output<'static>,
    pub(crate) tick_slice: Peri<'static, PWM_SLICE4>,
    pub(crate) pio: Peri<'static, PIO0>,
    pub(crate) ssi_clock: Peri<'static, PIN_22>,
    pub(crate) pitch_data: Peri<'static, PIN_26>,
    pub(crate) yaw_data: Peri<'static, PIN_27>,
    pub(crate) revolution_pulse: Peri<'static, PIN_28>,
}

impl Board {
    pub fn new(p: Peripherals) -> Self {
        let mut eth_config = spi::Config::default();
        eth_config.frequency = 40_000_000;
        let eth_spi: Spi<'static, SPI0, Async> = Spi::new(
            p.SPI0,
            p.PIN_18,
            p.PIN_19,
            p.PIN_16,
            p.DMA_CH2,
            p.DMA_CH3,
            crate::Irqs,
            eth_config,
        );

        Self {
            led: Output::new(p.PIN_25, Level::Low),
            rt: WhirlParts {
                tick_pin: Output::new(p.PIN_14, Level::Low),
                tick_slice: p.PWM_SLICE4,
                pio: p.PIO0,
                ssi_clock: p.PIN_22,
                pitch_data: p.PIN_26,
                yaw_data: p.PIN_27,
                revolution_pulse: p.PIN_28,
            },
            eth: EthernetParts {
                spi: eth_spi,
                cs: Output::new(p.PIN_17, Level::High),
                int: Input::new(p.PIN_21, Pull::Up),
                rst: Output::new(p.PIN_20, Level::High),
            },
            core1: p.CORE1,
        }
    }
}
