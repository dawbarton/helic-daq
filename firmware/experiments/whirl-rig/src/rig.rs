//! Whirl sensor assembly and bounded dual-encoder/optical measurement logic.

use core::sync::atomic::Ordering;

use embassy_rp::gpio::Output;
use embassy_rp::pac;
use embassy_rp::peripherals::PIO0;
use embassy_rp::pio::Pio;
use helic_core::rpm::RpmEstimator;
use helic_drivers::ssi::{deinterleave_pair, SsiFormat, SsiScale};
use helic_fw_common::pulse_pio::PulsePeriodReader;
use helic_fw_common::rig::{PwmWrapSpinTick, Rig};
use helic_fw_common::ssi_pio::DualSsiReader;
use helic_fw_common::SampleRate;

use crate::board::WhirlParts;
use crate::config::{
    ActiveController, ENCODER_BITS, ENCODER_BIT_RATE_HZ, ENCODER_COUNTS_PER_REV, PULSE_COUNTER_HZ,
    PULSE_COUNTER_OFFSET_TICKS, RPM_MIN_PERIOD_S, RPM_STALE_AFTER_S, RPM_TAU_S,
};
use crate::telemetry::{
    PITCH_VALUE, PULSE_COUNT, PULSE_ERRORS, PULSE_GLITCHES, REV_PERIOD_VALUE, RPM_VALUE,
    SSI_ERRORS, YAW_VALUE,
};

pub type Tick = PwmWrapSpinTick;

/// Whirl-specific state reached by the generic real-time pipeline.
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

impl WhirlParts {
    pub fn build(self, sample_rate: SampleRate) -> (WhirlRig, Tick) {
        let mut pio = Pio::new(self.pio, crate::Irqs);
        let encoders = DualSsiReader::new(
            &mut pio.common,
            pio.sm0,
            self.ssi_clock,
            self.pitch_data,
            self.yaw_data,
            ENCODER_BITS,
            ENCODER_BIT_RATE_HZ,
        );
        let pulse = PulsePeriodReader::new(
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

        // Start the clock after both PIO machines are ready, so the first wrap
        // observed by core 1 cannot pre-date sensor initialisation.
        let tick = PwmWrapSpinTick::new(self.tick_slice, sample_rate);
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

    type Ctrl = ActiveController;

    fn init(&mut self) {
        if !self.encoders.start() {
            SSI_ERRORS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[unsafe(link_section = ".data.ram_func")]
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

        // These atomics are latest-value diagnostics for core 0. The coherent
        // stream record remains the values slice above.
        PITCH_VALUE.store(self.positions[0].to_bits(), Ordering::Relaxed);
        YAW_VALUE.store(self.positions[1].to_bits(), Ordering::Relaxed);
        REV_PERIOD_VALUE.store(estimate.period_s.to_bits(), Ordering::Relaxed);
        RPM_VALUE.store(estimate.rpm.to_bits(), Ordering::Relaxed);
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn actuate(&mut self, _out: f32) {}

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_start(&mut self) {
        self.tick_pin.set_high();
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_phase_us(&self) -> Option<u32> {
        let ctr = pac::PWM.ch(4).ctr().read().ctr() as u32;
        Some(ctr * self.pwm_divider / 150)
    }

    #[unsafe(link_section = ".data.ram_func")]
    fn tick_end(&mut self) {
        self.tick_pin.set_low();
    }
}
