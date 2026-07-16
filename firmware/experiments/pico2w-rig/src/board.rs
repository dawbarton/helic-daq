//! Auditable Pico 2W pin map and ownership for the wireless DAC experiment.
//!
//! The radio owns PIO1, DMA0, and GP23/24/25/29. SPI1 drives the AD5064 on
//! GP10/11 with GP9 as SYNC and GP15 as LDAC; GP14 is the tick timing output.
//! DAC behaviour and the sample pipeline adaptation live in `rig.rs`.

use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{CORE1, PIN_1, PWM_SLICE4, SPI1, UART0};
use embassy_rp::spi::{Blocking, Spi};
use embassy_rp::{Peri, Peripherals};
use helic_fw_common::net::cyw43::WifiParts;

use crate::rig::DAC_SPI_CONFIG;

pub struct Board {
    pub rt: PicoDacParts,
    pub laser: LaserParts,
    pub wifi: WifiParts,
    pub core1: Peri<'static, CORE1>,
}

pub struct LaserParts {
    pub uart: Peri<'static, UART0>,
    pub rx: Peri<'static, PIN_1>,
}

/// Complete core-1 ownership bundle, assembled only after the core hand-off.
pub struct PicoDacParts {
    pub(crate) tick_pin: Output<'static>,
    pub(crate) tick_slice: Peri<'static, PWM_SLICE4>,
    pub(crate) dac_ldac: Output<'static>,
    pub(crate) spi: Spi<'static, SPI1, Blocking>,
    pub(crate) dac_cs: Output<'static>,
}

impl Board {
    pub fn new(p: Peripherals) -> Self {
        let dac_spi =
            Spi::new_blocking_txonly(p.SPI1, p.PIN_10, p.PIN_11, DAC_SPI_CONFIG.embassy());

        Self {
            rt: PicoDacParts {
                tick_pin: Output::new(p.PIN_14, Level::Low),
                tick_slice: p.PWM_SLICE4,
                dac_ldac: Output::new(p.PIN_15, Level::Low),
                spi: dac_spi,
                dac_cs: Output::new(p.PIN_9, Level::High),
            },
            laser: LaserParts {
                uart: p.UART0,
                rx: p.PIN_1,
            },
            wifi: WifiParts {
                pio: p.PIO1,
                pwr: p.PIN_23,
                dio: p.PIN_24,
                cs: p.PIN_25,
                clk: p.PIN_29,
                dma: p.DMA_CH0,
            },
            core1: p.CORE1,
        }
    }
}
