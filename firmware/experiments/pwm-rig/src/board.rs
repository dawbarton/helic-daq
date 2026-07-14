//! W5500/W6100-EVB-Pico2 wiring for a filtered PWM analogue output.
//!
//! PWM slice 4 is the internal 8 kHz sample clock. Slice 5 channel A drives
//! GP10 at approximately 146 kHz with 10-bit duty resolution; an external
//! RC or active reconstruction filter converts duty cycle to voltage.
//!
//! GP1 receives the laser UART, GP14 exposes tick timing, and the on-board
//! WIZnet device retains its fixed SPI0 pins. This is the minimal example of
//! implementing `Rig` for an output which is not an SPI DAC.

use core::sync::atomic::Ordering;

use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{CORE1, DMA_CH1, PIN_1, PIN_10, PWM_SLICE4, PWM_SLICE5, UART0};
use embassy_rp::pwm::{self, Pwm, PwmOutput};
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::{Peri, Peripherals};
use helic_drivers::pwm_out::PwmOut;
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::rig::{PwmWrapTick, Rig};
use helic_fw_common::SampleRate;

use crate::config::{ActiveController, LASER_RANGE_MM as DEFAULT_LASER_RANGE_MM};
use crate::{LASER_RANGE_MM, LASER_VALUE};

/// Unique peripheral ownership split into task/core bundles.
pub struct Board {
    /// Core-0 heartbeat LED.
    pub led: Output<'static>,
    /// Core-1 sample clock, timing pin and PWM output.
    pub analog: AnalogParts,
    /// Core-0 optoNCDT receive path.
    pub laser: LaserParts,
    /// Core-0 WIZnet transport.
    pub eth: EthernetParts,
    /// Handle consumed to start core 1.
    pub core1: Peri<'static, CORE1>,
}

/// UART parts are constructed in `main.rs` beside their interrupt binding.
pub struct LaserParts {
    pub uart: Peri<'static, UART0>,
    pub rx: Peri<'static, PIN_1>,
    pub rx_dma: Peri<'static, DMA_CH1>,
}

/// Unassembled core-1 hardware. Moving this value transfers ownership.
pub struct AnalogParts {
    tick_pin: Output<'static>,
    tick_slice: Peri<'static, PWM_SLICE4>,
    output_slice: Peri<'static, PWM_SLICE5>,
    output_pin: Peri<'static, PIN_10>,
}

/// Concrete real-time rig. The const generic `1` means one PWM channel.
pub struct RtAnalog {
    output: PwmOut<PwmOutput<'static>, 1>,
    tick_pin: Output<'static>,
}

impl AnalogParts {
    /// Construct the output and hardware-paced sample tick on core 1.
    pub fn build(self, sample_rate: SampleRate) -> (RtAnalog, PwmWrapTick) {
        let tick = PwmWrapTick::new(self.tick_slice, sample_rate);
        // `top = 1023` gives 1024 duty steps. At the 150 MHz system clock this
        // leaves a roughly 146 kHz carrier, which hardware must filter.
        let mut config = pwm::Config::default();
        config.top = 1023;
        config.compare_a = 0;
        let pwm = Pwm::new_output_a(self.output_slice, self.output_pin, config);
        let output = pwm.split().0.expect("PWM channel A must exist");
        (
            RtAnalog {
                output: PwmOut::new([output], crate::config::PWM_V_MIN, crate::config::PWM_V_MAX),
                tick_pin: self.tick_pin,
            },
            tick,
        )
    }
}

impl Rig for RtAnalog {
    // This experiment has no ADC; the asynchronous laser is its sole input.
    const INPUTS: &'static [(&'static str, &'static str)] = &[("laser", "mm")];

    // Static associated types avoid virtual dispatch in the tick path.
    type Tick = PwmWrapTick;
    type Ctrl = ActiveController;

    fn init(&mut self) {
        // Begin from zero duty as a fail-safe output.
        let _ = self.output.zero_all();
    }

    fn measure(&mut self, values: &mut [f32]) {
        values[0] = f32::from_bits(LASER_VALUE.load(Ordering::Relaxed));
    }

    fn actuate(&mut self, out: f32) {
        // PwmOut maps voltage into the configured duty interval and clamps
        // values outside it. It does not perform reconstruction filtering.
        let _ = self.output.write_volts(0, out);
    }

    fn tick_start(&mut self) {
        // GP14 brackets the tick so processing time is directly measurable.
        self.tick_pin.set_high();
    }

    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }

    fn param_names() -> &'static [&'static str] {
        // The registry discovers this hook and queues writes automatically.
        &["rig_laser_range"]
    }

    fn param_defaults() -> &'static [f32] {
        &[DEFAULT_LASER_RANGE_MM]
    }

    fn normalise_param(id: u16, value: f32) -> Option<f32> {
        // None rejects unknown IDs, NaN, infinities and non-positive ranges.
        (id == 0 && value.is_finite() && value > 0.0).then_some(value)
    }

    fn set_param(&mut self, id: u16, value: f32) {
        if id == 0 {
            LASER_RANGE_MM.store(value.to_bits(), Ordering::Relaxed);
        }
    }
}

impl Board {
    /// Consume the unique RP2350 peripheral set and assign every pin once.
    pub fn new(p: Peripherals) -> Self {
        // Only network SPI is needed; output generation is entirely in PWM.
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
                output_slice: p.PWM_SLICE5,
                output_pin: p.PIN_10,
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
