//! Auditable pin map and peripheral ownership for the wired CBC experiment.
//!
//! WIZnet reserves GP16–21 and GP25. CBC assigns GP0/1 to the optoNCDT UART;
//! GP2–8 and GP13 to the AD7609; GP9–12 and GP15 to the shared ADC/DAC SPI
//! path; and GP14 to the tick timing output. Behaviour lives in `rig.rs`.

use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{CORE1, PIN_0, PIN_1, PIN_7, PIN_8, PWM_SLICE4, SPI1, UART0};
use embassy_rp::spi::{self, Async, Blocking, Spi};
use embassy_rp::{Peri, Peripherals};
use helic_drivers::ad7609::ConfigPins;
use helic_fw_common::net::wiznet::EthernetParts;

/// Resources grouped by their eventual core or task owner.
pub struct Board {
    pub led: Output<'static>,
    pub rt: CbcParts,
    pub laser: LaserParts,
    pub eth: EthernetParts,
    pub core1: Peri<'static, CORE1>,
}

/// UART resources assembled on core 0, where the interrupt token is available.
pub struct LaserParts {
    pub uart: Peri<'static, UART0>,
    pub tx: Peri<'static, PIN_0>,
    pub rx: Peri<'static, PIN_1>,
}

/// Unassembled core-1 resources. Fields are visible only to `rig.rs`, which
/// consumes the complete value after it has moved to core 1.
pub struct CbcParts {
    pub(crate) tick_pin: Output<'static>,
    pub(crate) adc_busy: Peri<'static, PIN_7>,
    pub(crate) dac_ldac: Output<'static>,
    pub(crate) spi: Spi<'static, SPI1, Blocking>,
    pub(crate) adc_cs: Output<'static>,
    pub(crate) adc_pins: ConfigPins<Output<'static>>,
    pub(crate) dac_cs: Output<'static>,
    pub(crate) convst_slice: Peri<'static, PWM_SLICE4>,
    pub(crate) convst_pin: Peri<'static, PIN_8>,
}

impl Board {
    /// Consume every peripheral once and make the core boundary explicit.
    pub fn new(p: Peripherals) -> Self {
        let analog_spi =
            Spi::new_blocking(p.SPI1, p.PIN_10, p.PIN_11, p.PIN_12, spi::Config::default());

        let mut eth_config = spi::Config::default();
        #[cfg(feature = "diag-wiznet-10mhz")]
        {
            eth_config.frequency = 10_000_000;
        }
        #[cfg(not(feature = "diag-wiznet-10mhz"))]
        {
            eth_config.frequency = 40_000_000;
        }
        let eth_spi: Spi<'static, _, Async> = Spi::new(
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
            rt: CbcParts {
                tick_pin: Output::new(p.PIN_14, Level::Low),
                adc_busy: p.PIN_7,
                dac_ldac: Output::new(p.PIN_15, Level::Low),
                spi: analog_spi,
                adc_cs: Output::new(p.PIN_13, Level::High),
                adc_pins: ConfigPins {
                    os0: Output::new(p.PIN_2, Level::Low),
                    os1: Output::new(p.PIN_3, Level::Low),
                    os2: Output::new(p.PIN_4, Level::Low),
                    range: Output::new(p.PIN_5, Level::Low),
                    reset: Output::new(p.PIN_6, Level::Low),
                },
                dac_cs: Output::new(p.PIN_9, Level::High),
                convst_slice: p.PWM_SLICE4,
                convst_pin: p.PIN_8,
            },
            laser: LaserParts {
                uart: p.UART0,
                tx: p.PIN_0,
                rx: p.PIN_1,
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
