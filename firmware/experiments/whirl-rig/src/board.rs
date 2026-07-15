//! Pin ownership and real-time hardware assembly for the wired whirl rig.
//!
//! PIO0 samples the two SSI encoders on a shared clock and measures the
//! optical revolution period; SPI0 remains owned by W5500/W6100 networking.

use core::sync::atomic::Ordering;

use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{CORE1, PIN_22, PIN_26, PIN_27, PIN_28, PIO0, PWM_SLICE4, SPI0};
use embassy_rp::pio::Pio;
use embassy_rp::spi::{self, Async, Spi};
use embassy_rp::{pac, Peri, Peripherals};
use helic_core::rpm::RpmEstimator;
use helic_drivers::ssi::{deinterleave_pair, SsiFormat, SsiScale};
use helic_fw_common::net::wiznet::EthernetParts;
use helic_fw_common::pulse_pio::PulsePeriodReader;
#[cfg(feature = "rt-sync")]
use helic_fw_common::rig::PwmWrapSpinTick;
use helic_fw_common::rig::{PwmWrapTick, Rig};
use helic_fw_common::ssi_pio::DualSsiReader;
use helic_fw_common::SampleRate;

use crate::config::{
    ActiveController, ENCODER_BITS, ENCODER_BIT_RATE_HZ, ENCODER_COUNTS_PER_REV, PULSE_COUNTER_HZ,
    PULSE_COUNTER_OFFSET_TICKS, RPM_MIN_PERIOD_S, RPM_STALE_AFTER_S, RPM_TAU_S,
};
use crate::{
    PITCH_VALUE, PULSE_COUNT, PULSE_ERRORS, PULSE_GLITCHES, REV_PERIOD_VALUE, RPM_VALUE,
    SSI_ERRORS, YAW_VALUE,
};

pub struct Board {
    pub led: Output<'static>,
    pub sensors: SensorParts,
    pub eth: EthernetParts,
    pub core1: Peri<'static, CORE1>,
}

pub struct SensorParts {
    pub tick_pin: Output<'static>,
    tick_slice: Peri<'static, PWM_SLICE4>,
    pio: Peri<'static, PIO0>,
    ssi_clock: Peri<'static, PIN_22>,
    pitch_data: Peri<'static, PIN_26>,
    yaw_data: Peri<'static, PIN_27>,
    revolution_pulse: Peri<'static, PIN_28>,
}

pub struct WhirlRig {
    tick_pin: Output<'static>,
    encoders: DualSsiReader<'static, PIO0, 0>,
    pulse: PulsePeriodReader<'static, PIO0, 1>,
    encoder_format: SsiFormat,
    encoder_scale: SsiScale,
    positions: [f32; 2],
    rpm: RpmEstimator,
    pwm_divider: u32,
}

#[cfg(feature = "rt-sync")]
pub type Tick = PwmWrapSpinTick;
#[cfg(not(feature = "rt-sync"))]
pub type Tick = PwmWrapTick;

impl SensorParts {
    pub fn build(self, sample_rate: SampleRate) -> (WhirlRig, Tick) {
        let mut pio = Pio::new(self.pio, crate::Irqs);
        let encoders = DualSsiReader::new(
            pac::PIO0,
            &mut pio.common,
            pio.sm0,
            self.ssi_clock,
            self.pitch_data,
            self.yaw_data,
            ENCODER_BITS,
            ENCODER_BIT_RATE_HZ,
        );
        let pulse = PulsePeriodReader::new(
            pac::PIO0,
            &mut pio.common,
            pio.sm1,
            self.revolution_pulse,
            PULSE_COUNTER_HZ,
        );
        let rig = WhirlRig {
            tick_pin: self.tick_pin,
            encoders,
            pulse,
            encoder_format: SsiFormat {
                bits: ENCODER_BITS,
                gray: false,
            },
            encoder_scale: SsiScale {
                counts_per_rev: ENCODER_COUNTS_PER_REV,
            },
            positions: [0.0; 2],
            rpm: RpmEstimator::new(RPM_TAU_S, RPM_STALE_AFTER_S, RPM_MIN_PERIOD_S),
            pwm_divider: sample_rate.pwm_params().0 as u32,
        };
        // Start the sample clock only after both PIO state machines are fully
        // configured, so the first latched wrap cannot pre-date sensor setup.
        #[cfg(feature = "rt-sync")]
        let tick = PwmWrapSpinTick::new(self.tick_slice, sample_rate);
        #[cfg(not(feature = "rt-sync"))]
        let tick = PwmWrapTick::new(self.tick_slice, sample_rate);
        (rig, tick)
    }
}

impl Rig for WhirlRig {
    const INPUTS: &'static [(&'static str, &'static str)] = &[
        ("pitch", "rev"),
        ("yaw", "rev"),
        ("rev_period", "s"),
        ("rpm", "rpm"),
        ("rev_pulse", "bool"),
        ("rpm_valid", "bool"),
    ];

    type Tick = PwmWrapTick;
    type Ctrl = ActiveController;

    fn init(&mut self) {
        if !self.encoders.start() {
            SSI_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn measure(&mut self, values: &mut [f32]) {
        if let Some(raw) = self.encoders.read() {
            match deinterleave_pair(raw, ENCODER_BITS).and_then(|words| {
                Ok([
                    self.encoder_format.decode(words[0])?,
                    self.encoder_format.decode(words[1])?,
                ])
            }) {
                Ok(counts) => {
                    self.positions = [
                        self.encoder_scale.position(counts[0]),
                        self.encoder_scale.position(counts[1]),
                    ];
                }
                Err(_) => {
                    SSI_ERRORS.fetch_add(1, Ordering::Relaxed);
                }
            }
        } else {
            SSI_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
        if !self.encoders.start() {
            SSI_ERRORS.fetch_add(1, Ordering::Relaxed);
        }

        self.rpm.tick(crate::config::SAMPLE_RATE.dt());
        let mut new_period = false;
        while let Some(ticks) = self.pulse.read() {
            let corrected = ticks.saturating_add(PULSE_COUNTER_OFFSET_TICKS);
            let period_s = corrected as f32 / PULSE_COUNTER_HZ as f32;
            if self.rpm.observe(period_s) {
                PULSE_COUNT.fetch_add(1, Ordering::Relaxed);
                new_period = true;
            } else {
                PULSE_GLITCHES.fetch_add(1, Ordering::Relaxed);
            }
        }
        if self.pulse.stalled() {
            PULSE_ERRORS.fetch_add(1, Ordering::Relaxed);
        }

        let estimate = self.rpm.estimate();
        values[0] = self.positions[0];
        values[1] = self.positions[1];
        values[2] = estimate.period_s;
        values[3] = estimate.rpm;
        values[4] = if new_period { 1.0 } else { 0.0 };
        values[5] = if estimate.valid { 1.0 } else { 0.0 };
        PITCH_VALUE.store(self.positions[0].to_bits(), Ordering::Relaxed);
        YAW_VALUE.store(self.positions[1].to_bits(), Ordering::Relaxed);
        REV_PERIOD_VALUE.store(estimate.period_s.to_bits(), Ordering::Relaxed);
        RPM_VALUE.store(estimate.rpm.to_bits(), Ordering::Relaxed);
    }

    #[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn actuate(&mut self, _out: f32) {}

    #[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn tick_start(&mut self) {
        self.tick_pin.set_high();
    }

    #[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn tick_phase_us(&self) -> Option<u32> {
        let ctr = pac::PWM.ch(4).ctr().read().ctr() as u32;
        Some(ctr * self.pwm_divider / 150)
    }

    #[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }
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
            sensors: SensorParts {
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
