//! W5500/W6100-EVB-Pico2 wiring for signal generation without an ADC board.
//!
//! SPI0 remains the on-board WIZnet chip. SPI1 drives only the AD5064 on
//! GP10/GP11 with GP9 as SYNC and GP15 held low as LDAC. PWM slice 4 is an
//! internal sample clock; no CONVST pin or AD7609 GPIO is claimed.
//!
//! Other assignments are GP1 for laser UART RX, GP14 for tick timing, GP25
//! for heartbeat, and the fixed WIZnet SPI0 pins listed in `cbc-rig/board.rs`.
//! This file demonstrates the `Rig` contract without ADC acquisition. See
//! "Adding an experiment" in `docs/developer_guide.md`.

use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{CORE1, DMA_CH1, PIN_1, PWM_SLICE4, SPI1, UART0};
use embassy_rp::spi::{self, Async, Blocking, Spi};
use embassy_rp::{Peri, Peripherals};
use embassy_time::Delay;
use embedded_hal_bus::spi::ExclusiveDevice;
use helic_drivers::ad5064::{Ad5064, ChannelPolarity};
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::rig::{PwmWrapTick, Rig};
use helic_fw_common::SampleRate;

use crate::config::{ActiveController, LASER_RANGE_MM as DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL};
use crate::{LASER_RANGE_MM, LASER_VALUE};

/// Voltage reference fitted to the analogue board.
const DAC_VREF: f32 = 4.096;
/// Must match the fitted output stages before any hardware use.
const DAC_POLARITY: [ChannelPolarity; 4] = [
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
    ChannelPolarity::Unipolar,
];

// Type aliases hide the full generic driver stack. `ExclusiveDevice` is enough
// here because no ADC shares SPI1 in this experiment.
type DacSpi = ExclusiveDevice<Spi<'static, SPI1, Blocking>, Output<'static>, Delay>;
type Dac = Ad5064<DacSpi>;

/// Unique board resources grouped by their eventual task or core owner.
pub struct Board {
    /// WIZnet board heartbeat LED, used on core 0.
    pub led: Output<'static>,
    /// Unassembled real-time hardware, moved to core 1.
    pub analog: AnalogParts,
    /// Unassembled optoNCDT receive path, used on core 0.
    pub laser: LaserParts,
    /// On-board WIZnet interface, used on core 0.
    pub eth: EthernetParts,
    /// RP2350 core handle consumed by `spawn_core1`.
    pub core1: Peri<'static, CORE1>,
}

/// UART pieces are assembled in `main.rs`, where the interrupt token exists.
pub struct LaserParts {
    pub uart: Peri<'static, UART0>,
    pub rx: Peri<'static, PIN_1>,
    pub rx_dma: Peri<'static, DMA_CH1>,
}

/// Core-1 pieces remain unassembled while they cross the core boundary.
pub struct AnalogParts {
    tick_pin: Output<'static>,
    tick_slice: Peri<'static, PWM_SLICE4>,
    dac_ldac: Output<'static>,
    spi: Spi<'static, SPI1, Blocking>,
    dac_cs: Output<'static>,
}

/// Constructed rig and its mutable state, exclusively owned by core 1.
pub struct RtAnalog {
    dac: Dac,
    tick_pin: Output<'static>,
    output_channel: usize,
}

impl AnalogParts {
    /// Turn unique peripheral parts into the concrete rig and sample tick.
    ///
    /// `self` is consumed, so these peripherals cannot accidentally be built
    /// twice. `PwmWrapTick` provides hardware pacing for an ADC-free rig.
    pub fn build(self, sample_rate: SampleRate) -> (RtAnalog, PwmWrapTick) {
        let tick = PwmWrapTick::new(self.tick_slice, sample_rate);
        let spi = ExclusiveDevice::new(self.spi, self.dac_cs, Delay)
            .expect("AD5064 SPI device construction failed");
        // LDAC must remain driven low for write-and-update operation. Forgetting
        // the owned pin keeps it configured for the rest of the firmware run.
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
    // Source names and units are discovered by the host. `measure` must fill
    // the values slice in this exact order.
    const INPUTS: &'static [(&'static str, &'static str)] = &[("laser", "mm")];

    // Associated types make both choices static, with no dynamic dispatch in
    // the real-time path.
    type Tick = PwmWrapTick;
    type Ctrl = ActiveController;

    fn init(&mut self) {
        // Initialise actuators to a fail-safe zero before the first sample.
        if self
            .dac
            .zero_all_with_delay(&mut embassy_time::Delay)
            .is_err()
        {
            warn!("DAC zeroing failed");
        }
    }

    fn measure(&mut self, values: &mut [f32]) {
        // Laser reception is asynchronous on core 0; the RT loop reads the
        // most recent complete f32 bit pattern without waiting.
        values[0] = f32::from_bits(LASER_VALUE.load(Ordering::Relaxed));
    }

    fn actuate(&mut self, out: f32) {
        let _ = self.dac.write_volts(self.output_channel, out);
    }

    fn tick_start(&mut self) {
        // GP14 brackets the entire tick body for oscilloscope timing checks.
        self.tick_pin.set_high();
    }

    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }

    fn param_names() -> &'static [&'static str] {
        // IDs are positions in these parallel lists and match the match arms
        // below. The common registry handles discovery and queueing.
        &["rig_laser_range", "rig_out_channel"]
    }

    fn param_defaults() -> &'static [f32] {
        &[DEFAULT_LASER_RANGE_MM, OUTPUT_CHANNEL as f32]
    }

    fn normalise_param(id: u16, value: f32) -> Option<f32> {
        // Validate before a host write is acknowledged. Output channels must
        // be exact integers within the four-channel polarity table.
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
        // The shared loop calls this at a sample boundary.
        match id {
            0 => LASER_RANGE_MM.store(value.to_bits(), Ordering::Relaxed),
            1 => self.output_channel = value as usize,
            _ => {}
        }
    }
}

impl Board {
    /// Consume the RP2350 peripheral singleton and assign pins exactly once.
    pub fn new(p: Peripherals) -> Self {
        // AD5064 uses SPI mode 1 at 16 MHz. This is a blocking core-1 transfer
        // with a short, bounded duration.
        let mut dac_config = spi::Config::default();
        dac_config.frequency = 16_000_000;
        dac_config.polarity = spi::Polarity::IdleLow;
        dac_config.phase = spi::Phase::CaptureOnSecondTransition;
        let dac_spi = Spi::new_blocking_txonly(p.SPI1, p.PIN_10, p.PIN_11, dac_config);

        // WIZnet traffic is asynchronous and DMA-backed on core 0.
        let mut eth_config = spi::Config::default();
        eth_config.frequency = 40_000_000;
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
