//! Raspberry Pi Pico 2W wiring for signal generation without an ADC board.
//!
//! PIO1 drives the on-board CYW43439. SPI1 drives only the AD5064 on
//! GP10/GP11 with GP9 as SYNC and GP15 held low as LDAC. PWM slice 4 is an
//! internal sample clock; no CONVST pin or AD7609 GPIO is claimed.
//!
//! The Pico 2W radio owns its fixed PIO1 and GP23/24/25/29 wiring. Keeping the
//! `Rig` identical to the wired variant demonstrates that transport selection
//! does not enter the real-time API.

use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{CORE1, DMA_CH1, PIN_1, PWM_SLICE4, SPI1, UART0};
use embassy_rp::spi::{self, Blocking, Spi};
use embassy_rp::{Peri, Peripherals};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use helic_drivers::ad5064::{Ad5064, ChannelPolarity};
use helic_fw_common::net::cyw43::WifiParts;
use helic_fw_common::rig::{PwmWrapTick, Rig};
use helic_fw_common::SampleRate;

use crate::config::{ActiveController, LASER_RANGE_MM as DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL};
use crate::{LASER_RANGE_MM, LASER_VALUE};

/// Voltage reference fitted to the analogue board.
const DAC_VREF: f32 = 4.096;
/// Must match the fitted output stages before hardware use.
const DAC_POLARITY: [ChannelPolarity; 4] = [
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
];

// No ADC shares SPI1, so an exclusive chip-select wrapper is sufficient.
type DacSpi = ExclusiveDevice<Spi<'static, SPI1, Blocking>, Output<'static>, Delay>;
type Dac = Ad5064<DacSpi>;

/// Unique board resources grouped by core and task ownership.
pub struct Board {
    /// Core-1 real-time hardware.
    pub analog: AnalogParts,
    /// Core-0 laser receiver.
    pub laser: LaserParts,
    /// Fixed Pico 2W radio resources, all owned by core 0.
    pub wifi: WifiParts,
    /// Handle consumed to start core 1.
    pub core1: Peri<'static, CORE1>,
}

/// UART is assembled in `main.rs`, where its interrupt token is available.
pub struct LaserParts {
    pub uart: Peri<'static, UART0>,
    pub rx: Peri<'static, PIN_1>,
    pub rx_dma: Peri<'static, DMA_CH1>,
}

/// Unassembled hardware which can be moved safely into the core-1 closure.
pub struct AnalogParts {
    tick_pin: Output<'static>,
    tick_slice: Peri<'static, PWM_SLICE4>,
    dac_ldac: Output<'static>,
    spi: Spi<'static, SPI1, Blocking>,
    dac_cs: Output<'static>,
}

/// Concrete DAC rig and mutable RT state.
pub struct RtAnalog {
    dac: Dac,
    tick_pin: Output<'static>,
    output_channel: usize,
}

impl AnalogParts {
    /// Consume the peripheral bundle and construct the rig on core 1.
    pub fn build(self, sample_rate: SampleRate) -> (RtAnalog, PwmWrapTick) {
        let tick = PwmWrapTick::new(self.tick_slice, sample_rate);
        let spi = ExclusiveDevice::new(self.spi, self.dac_cs, Delay)
            .expect("AD5064 SPI device construction failed");
        // Keep LDAC driven low permanently for write-and-update operation.
        core::mem::forget(self.dac_ldac);
        (
            RtAnalog {
                dac: Ad5064::new(spi, DAC_POLARITY, DAC_VREF),
                tick_pin: self.tick_pin,
                output_channel: OUTPUT_CHANNEL,
            },
            tick,
        )
    }
}

impl Rig for RtAnalog {
    // The host discovers this list. `measure` fills the same order.
    const INPUTS: &'static [(&'static str, &'static str)] = &[("laser", "mm")];

    // Static choices keep the tick monomorphic and bounded.
    type Tick = PwmWrapTick;
    type Ctrl = ActiveController;

    fn init(&mut self) {
        // Zero every DAC channel before the first generated sample.
        if self
            .dac
            .zero_all_with_delay(&mut embassy_time::Delay)
            .is_err()
        {
            warn!("DAC zeroing failed");
        }
    }

    fn measure(&mut self, values: &mut [f32]) {
        // Read the most recent core-0 laser value without waiting.
        values[0] = f32::from_bits(LASER_VALUE.load(Ordering::Relaxed));
    }

    fn actuate(&mut self, out: f32) {
        let _ = self.dac.write_volts(self.output_channel, out);
    }

    fn tick_start(&mut self) {
        // GP14 gives an independent scope view of RT processing duration.
        self.tick_pin.set_high();
    }

    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }

    fn param_names() -> &'static [&'static str] {
        // Names, defaults and IDs must remain in matching order.
        &["rig_laser_range", "rig_out_channel"]
    }

    fn param_defaults() -> &'static [f32] {
        &[DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL as f32]
    }

    fn normalise_param(id: u16, value: f32) -> Option<f32> {
        // Validate before acknowledgement; the channel is an integer index.
        match id {
            0 if value.is_finite() && value > 0.0 => Some(value),
            1 if value.is_finite()
                && value >= 0.0
                && value < DAC_POLARITY.len() as f32
                && value == value as usize as f32 =>
            {
                Some(value)
            }
            _ => None,
        }
    }

    fn set_param(&mut self, id: u16, value: f32) {
        match id {
            0 => LASER_RANGE_MM.store(value.to_bits(), Ordering::Relaxed),
            1 => self.output_channel = value as usize,
            _ => {}
        }
    }
}

impl Board {
    /// Consume the peripheral singleton and make all pin ownership explicit.
    pub fn new(p: Peripherals) -> Self {
        // AD5064 uses SPI mode 1 at 16 MHz on the real-time core.
        let mut dac_config = spi::Config::default();
        dac_config.frequency = 16_000_000;
        dac_config.polarity = spi::Polarity::IdleLow;
        dac_config.phase = spi::Phase::CaptureOnSecondTransition;
        let dac_spi = Spi::new_blocking_txonly(p.SPI1, p.PIN_10, p.PIN_11, dac_config);

        Self {
            analog: AnalogParts {
                tick_pin: Output::new(p.PIN_14, Level::Low),
                tick_slice: p.PWM_SLICE4,
                dac_ldac: Output::new(p.PIN_15, Level::Low),
                spi: dac_spi,
                dac_cs: Output::new(p.PIN_9, Level::High),
            },
            laser: LaserParts {
                uart: p.UART0,
                rx: p.PIN_1,
                rx_dma: p.DMA_CH1,
            },
            // These pins and PIO/DMA resources are fixed by the Pico 2W board.
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
