//! Bounded single-drive-point phase-locked frequency tracking.
//!
//! The PLL coherently demodulates one measured force and one measured response
//! against a shared harmonic frame. Acquisition failure is non-faulting;
//! only loss of a previously established lock enters the latched fault state.

use core::marker::PhantomData;

use crate::HarmonicFrame;

const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PllState {
    Fixed,
    Acquiring,
    Locked,
    LockLost,
}

/// Complete construction-time PLL configuration.
#[derive(Clone, Copy, Debug)]
pub struct PllConfig {
    pub setpoint_increment: u32,
    pub min_increment: u32,
    pub max_increment: u32,
    /// Increment correction per degree-second.
    pub gain: f32,
    pub target_phase_deg: f32,
    pub demod_time_constant_s: f32,
    pub min_amplitude: f32,
    pub lock_tolerance_deg: f32,
    pub unlock_tolerance_deg: f32,
    pub lock_dwell_s: f32,
    pub unlock_dwell_s: f32,
    pub acquire_timeout_s: f32,
    pub saturation_dwell_s: f32,
}

impl Default for PllConfig {
    fn default() -> Self {
        Self {
            setpoint_increment: 0,
            min_increment: 0,
            max_increment: u32::MAX,
            gain: 0.0,
            target_phase_deg: 0.0,
            demod_time_constant_s: 0.1,
            min_amplitude: 0.0,
            lock_tolerance_deg: 5.0,
            unlock_tolerance_deg: 10.0,
            lock_dwell_s: 0.1,
            unlock_dwell_s: 0.1,
            acquire_timeout_s: 5.0,
            saturation_dwell_s: 0.1,
        }
    }
}

impl PllConfig {
    fn validate<const H: usize>(&self) {
        assert!(H > 0, "PLL requires a fundamental harmonic");
        assert!(self.min_increment <= self.setpoint_increment);
        assert!(self.setpoint_increment <= self.max_increment);
        assert!(self.gain.is_finite());
        assert!(valid_phase_degrees(self.target_phase_deg));
        assert!(non_negative_finite(self.demod_time_constant_s));
        assert!(non_negative_finite(self.min_amplitude));
        assert!(non_negative_finite(self.lock_tolerance_deg));
        assert!(non_negative_finite(self.unlock_tolerance_deg));
        assert!(self.lock_tolerance_deg <= self.unlock_tolerance_deg);
        assert!(non_negative_finite(self.lock_dwell_s));
        assert!(non_negative_finite(self.unlock_dwell_s));
        assert!(non_negative_finite(self.acquire_timeout_s));
        assert!(non_negative_finite(self.saturation_dwell_s));
    }
}

const fn non_negative_finite(value: f32) -> bool {
    value >= 0.0 && value.is_finite()
}

#[derive(Clone, Copy, Debug, Default)]
struct FundamentalDemodulator {
    force_cos: f32,
    force_sin: f32,
    response_cos: f32,
    response_sin: f32,
}

impl FundamentalDemodulator {
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn update<const H: usize>(
        &mut self,
        frame: &HarmonicFrame<H>,
        force: f32,
        response: f32,
        dt_s: f32,
        time_constant_s: f32,
        min_amplitude: f32,
    ) -> Option<f32> {
        if !force.is_finite() || !response.is_finite() || !valid_dt(dt_s) {
            return None;
        }
        let alpha = if time_constant_s == 0.0 {
            1.0
        } else {
            dt_s / (time_constant_s + dt_s)
        };
        let cos = frame.cos(0);
        let sin = frame.sin(0);
        self.force_cos += alpha * (2.0 * force * cos - self.force_cos);
        self.force_sin += alpha * (2.0 * force * sin - self.force_sin);
        self.response_cos += alpha * (2.0 * response * cos - self.response_cos);
        self.response_sin += alpha * (2.0 * response * sin - self.response_sin);

        let force_amplitude_sq = self.force_cos * self.force_cos + self.force_sin * self.force_sin;
        let response_amplitude_sq =
            self.response_cos * self.response_cos + self.response_sin * self.response_sin;
        let min_amplitude_sq = min_amplitude * min_amplitude;
        if force_amplitude_sq < min_amplitude_sq || response_amplitude_sq < min_amplitude_sq {
            return None;
        }

        let force_phase = atan2_degrees(self.force_sin, self.force_cos);
        let response_phase = atan2_degrees(self.response_sin, self.response_cos);
        Some(wrap_degrees(response_phase - force_phase))
    }
}

/// Fundamental measured-force/measured-response PLL with bounded frequency.
pub struct Pll<const H: usize> {
    state: PllState,
    setpoint_increment: u32,
    min_increment: u32,
    max_increment: u32,
    commanded_increment: u32,
    correction_remainder: f32,
    gain: f32,
    target_phase_deg: f32,
    demod_time_constant_s: f32,
    min_amplitude: f32,
    lock_tolerance_deg: f32,
    unlock_tolerance_deg: f32,
    lock_dwell_s: f32,
    unlock_dwell_s: f32,
    acquire_timeout_s: f32,
    saturation_dwell_s: f32,
    phase_error_deg: f32,
    state_elapsed_s: f32,
    lock_elapsed_s: f32,
    unlock_elapsed_s: f32,
    saturation_elapsed_s: f32,
    demodulator: FundamentalDemodulator,
    harmonics: PhantomData<[f32; H]>,
}

impl<const H: usize> Pll<H> {
    pub fn new(config: PllConfig) -> Self {
        config.validate::<H>();
        Self {
            state: PllState::Fixed,
            setpoint_increment: config.setpoint_increment,
            min_increment: config.min_increment,
            max_increment: config.max_increment,
            commanded_increment: config.setpoint_increment,
            correction_remainder: 0.0,
            gain: config.gain,
            target_phase_deg: wrap_degrees(config.target_phase_deg),
            demod_time_constant_s: config.demod_time_constant_s,
            min_amplitude: config.min_amplitude,
            lock_tolerance_deg: config.lock_tolerance_deg,
            unlock_tolerance_deg: config.unlock_tolerance_deg,
            lock_dwell_s: config.lock_dwell_s,
            unlock_dwell_s: config.unlock_dwell_s,
            acquire_timeout_s: config.acquire_timeout_s,
            saturation_dwell_s: config.saturation_dwell_s,
            phase_error_deg: 0.0,
            state_elapsed_s: 0.0,
            lock_elapsed_s: 0.0,
            unlock_elapsed_s: 0.0,
            saturation_elapsed_s: 0.0,
            demodulator: FundamentalDemodulator::default(),
            harmonics: PhantomData,
        }
    }

    /// Update from raw measured force and response samples.
    ///
    /// The measured phase is `response - force`; `gain` may be signed to match
    /// the plant's phase/frequency slope. The returned increment is always
    /// inside the configured inclusive bounds, including after non-finite or
    /// divergent input.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn update(
        &mut self,
        frame: &HarmonicFrame<H>,
        force: f32,
        response: f32,
        dt_s: f32,
    ) -> u32 {
        if matches!(self.state, PllState::Fixed | PllState::LockLost) {
            return self.commanded_increment();
        }
        let measured_phase = self.demodulator.update(
            frame,
            force,
            response,
            dt_s,
            self.demod_time_constant_s,
            self.min_amplitude,
        );
        self.update_observation(measured_phase, dt_s)
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn update_observation(&mut self, measured_phase_deg: Option<f32>, dt_s: f32) -> u32 {
        if matches!(self.state, PllState::Fixed | PllState::LockLost) || !valid_dt(dt_s) {
            return self.commanded_increment();
        }

        self.state_elapsed_s += dt_s;
        let valid_phase = measured_phase_deg.filter(|phase| phase.is_finite());
        let saturated = if let Some(measured_phase) = valid_phase {
            self.phase_error_deg = wrap_degrees(self.target_phase_deg - measured_phase);
            self.apply_correction(dt_s)
        } else {
            false
        };

        match self.state {
            PllState::Acquiring => {
                if valid_phase.is_some() {
                    if self.phase_error_deg.abs() < self.lock_tolerance_deg {
                        self.lock_elapsed_s += dt_s;
                        if self.lock_elapsed_s >= self.lock_dwell_s {
                            self.enter_locked();
                        }
                    } else {
                        self.lock_elapsed_s = 0.0;
                    }
                }
                if self.state == PllState::Acquiring
                    && self.state_elapsed_s >= self.acquire_timeout_s
                {
                    self.acquisition_failed();
                }
            }
            PllState::Locked => {
                let outside_lock =
                    valid_phase.is_none() || self.phase_error_deg.abs() > self.unlock_tolerance_deg;
                if outside_lock {
                    self.unlock_elapsed_s += dt_s;
                } else {
                    self.unlock_elapsed_s = 0.0;
                }
                if saturated {
                    self.saturation_elapsed_s += dt_s;
                } else {
                    self.saturation_elapsed_s = 0.0;
                }
                if (outside_lock && self.unlock_elapsed_s >= self.unlock_dwell_s)
                    || (saturated && self.saturation_elapsed_s >= self.saturation_dwell_s)
                {
                    self.state = PllState::LockLost;
                }
            }
            PllState::Fixed | PllState::LockLost => {}
        }
        self.commanded_increment()
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply_correction(&mut self, dt_s: f32) -> bool {
        let correction = self.correction_remainder + self.gain * self.phase_error_deg * dt_s;
        if correction.is_nan() {
            self.correction_remainder = 0.0;
            return true;
        }
        if correction >= u32::MAX as f32 {
            self.commanded_increment = self.max_increment;
            self.correction_remainder = 0.0;
            return true;
        }
        if correction <= -(u32::MAX as f32) {
            self.commanded_increment = self.min_increment;
            self.correction_remainder = 0.0;
            return true;
        }

        let positive = correction >= 0.0;
        let magnitude = if positive { correction } else { -correction };
        let whole_magnitude = (magnitude + 0.5) as u32;
        let whole = if positive {
            whole_magnitude as f32
        } else {
            -(whole_magnitude as f32)
        };
        let saturated = if positive {
            let increase = whole_magnitude;
            if increase > self.max_increment - self.commanded_increment {
                self.commanded_increment = self.max_increment;
                true
            } else {
                self.commanded_increment += increase;
                false
            }
        } else {
            let decrease = whole_magnitude;
            if decrease > self.commanded_increment - self.min_increment {
                self.commanded_increment = self.min_increment;
                true
            } else {
                self.commanded_increment -= decrease;
                false
            }
        };
        self.correction_remainder = if saturated { 0.0 } else { correction - whole };
        saturated
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn enter_locked(&mut self) {
        self.state = PllState::Locked;
        self.state_elapsed_s = 0.0;
        self.unlock_elapsed_s = 0.0;
        self.saturation_elapsed_s = 0.0;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn acquisition_failed(&mut self) {
        self.reset_loop();
        self.state = PllState::Fixed;
    }

    /// Enter acquisition only from `Fixed`; repeated enables are idempotent.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.reset();
        } else if self.state == PllState::Fixed {
            self.reset_loop();
            self.state = PllState::Acquiring;
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub const fn state(&self) -> PllState {
        self.state
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub const fn phase_error(&self) -> f32 {
        self.phase_error_deg
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn commanded_increment(&self) -> u32 {
        self.commanded_increment
    }

    /// Return to the fixed setpoint and clear demodulation and loop state.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn reset(&mut self) {
        self.reset_loop();
        self.state = PllState::Fixed;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn reset_loop(&mut self) {
        self.commanded_increment = self.setpoint_increment;
        self.correction_remainder = 0.0;
        self.phase_error_deg = 0.0;
        self.state_elapsed_s = 0.0;
        self.lock_elapsed_s = 0.0;
        self.unlock_elapsed_s = 0.0;
        self.saturation_elapsed_s = 0.0;
        self.demodulator = FundamentalDemodulator::default();
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_setpoint_increment(&mut self, increment: u32) {
        assert!((self.min_increment..=self.max_increment).contains(&increment));
        self.setpoint_increment = increment;
        if self.state == PllState::Fixed {
            self.commanded_increment = increment;
            self.correction_remainder = 0.0;
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_gain(&mut self, gain: f32) {
        assert!(gain.is_finite());
        self.gain = gain;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_target_phase(&mut self, degrees: f32) {
        assert!(valid_phase_degrees(degrees));
        self.target_phase_deg = degrees;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_min_increment(&mut self, increment: u32) {
        assert!(increment <= self.max_increment);
        self.min_increment = increment;
        self.setpoint_increment = self.setpoint_increment.max(increment);
        if self.commanded_increment < increment {
            self.commanded_increment = increment;
            self.correction_remainder = 0.0;
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_max_increment(&mut self, increment: u32) {
        assert!(increment >= self.min_increment);
        self.max_increment = increment;
        self.setpoint_increment = self.setpoint_increment.min(increment);
        if self.commanded_increment > increment {
            self.commanded_increment = increment;
            self.correction_remainder = 0.0;
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_demod_time_constant(&mut self, seconds: f32) {
        assert!(non_negative_finite(seconds));
        self.demod_time_constant_s = seconds;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_lock_tolerance(&mut self, degrees: f32) {
        assert!(non_negative_finite(degrees));
        assert!(degrees <= self.unlock_tolerance_deg);
        self.lock_tolerance_deg = degrees;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_unlock_tolerance(&mut self, degrees: f32) {
        assert!(non_negative_finite(degrees));
        assert!(degrees >= self.lock_tolerance_deg);
        self.unlock_tolerance_deg = degrees;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_lock_dwell(&mut self, seconds: f32) {
        assert!(non_negative_finite(seconds));
        self.lock_dwell_s = seconds;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_unlock_dwell(&mut self, seconds: f32) {
        assert!(non_negative_finite(seconds));
        self.unlock_dwell_s = seconds;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_acquire_timeout(&mut self, seconds: f32) {
        assert!(non_negative_finite(seconds));
        self.acquire_timeout_s = seconds;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_min_amplitude(&mut self, amplitude: f32) {
        assert!(non_negative_finite(amplitude));
        self.min_amplitude = amplitude;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_saturation_dwell(&mut self, seconds: f32) {
        assert!(non_negative_finite(seconds));
        self.saturation_dwell_s = seconds;
    }
}

#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
fn valid_dt(dt_s: f32) -> bool {
    dt_s.is_finite() && dt_s > 0.0
}

const fn valid_phase_degrees(degrees: f32) -> bool {
    degrees >= -180.0 && degrees < 180.0 && degrees.is_finite()
}

#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
fn wrap_degrees(mut degrees: f32) -> f32 {
    if degrees >= 180.0 {
        degrees -= 360.0;
    } else if degrees < -180.0 {
        degrees += 360.0;
    }
    degrees
}

/// Bounded atan2 approximation, better than 0.1 degrees over the full plane.
#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
fn atan2_degrees(y: f32, x: f32) -> f32 {
    if x == 0.0 {
        return if y > 0.0 {
            90.0
        } else if y < 0.0 {
            -90.0
        } else {
            0.0
        };
    }

    let angle = if x.abs() >= y.abs() {
        let base = atan_unit(y / x);
        if x < 0.0 {
            if y >= 0.0 {
                base + core::f32::consts::PI
            } else {
                base - core::f32::consts::PI
            }
        } else {
            base
        }
    } else if y > 0.0 {
        core::f32::consts::FRAC_PI_2 - atan_unit(x / y)
    } else {
        -core::f32::consts::FRAC_PI_2 - atan_unit(x / y)
    };
    angle * RAD_TO_DEG
}

#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
fn atan_unit(value: f32) -> f32 {
    let magnitude = value.abs();
    core::f32::consts::FRAC_PI_4 * value - value * (magnitude - 1.0) * (0.2447 + 0.0663 * magnitude)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HarmonicGenerator, PhaseAccumulator, SinLut};

    fn config() -> PllConfig {
        PllConfig {
            setpoint_increment: 100,
            min_increment: 90,
            max_increment: 110,
            gain: 1.0,
            target_phase_deg: 0.0,
            demod_time_constant_s: 0.02,
            min_amplitude: 0.1,
            lock_tolerance_deg: 2.0,
            unlock_tolerance_deg: 5.0,
            lock_dwell_s: 0.03,
            unlock_dwell_s: 0.03,
            acquire_timeout_s: 0.05,
            saturation_dwell_s: 0.03,
        }
    }

    #[test]
    fn bounded_atan2_approximation_is_accurate_over_the_plane() {
        let mut max_error = 0.0_f32;
        for x in -100..=100 {
            for y in -100..=100 {
                if x == 0 && y == 0 {
                    continue;
                }
                let approximate = atan2_degrees(y as f32, x as f32);
                let reference = libm::atan2f(y as f32, x as f32) * RAD_TO_DEG;
                max_error = max_error.max(wrap_degrees(approximate - reference).abs());
            }
        }
        assert!(max_error < 0.1, "maximum error {max_error}");
    }

    #[test]
    fn acquisition_timeout_is_non_faulting_and_requires_fresh_enable() {
        let mut pll = Pll::<1>::new(config());
        pll.set_enabled(true);
        for _ in 0..6 {
            pll.update_observation(None, 0.01);
        }
        assert_eq!(pll.state(), PllState::Fixed);
        assert_eq!(pll.commanded_increment(), 100);
        for _ in 0..10 {
            pll.update_observation(Some(0.0), 0.01);
        }
        assert_eq!(pll.state(), PllState::Fixed);

        pll.set_enabled(true);
        assert_eq!(pll.state(), PllState::Acquiring);
    }

    #[test]
    fn dwell_hysteresis_and_replayed_enable_preserve_lock() {
        let mut pll = Pll::<1>::new(config());
        pll.set_enabled(true);
        pll.update_observation(Some(0.0), 0.01);
        pll.update_observation(Some(0.0), 0.01);
        assert_eq!(pll.state(), PllState::Acquiring);
        pll.update_observation(Some(0.0), 0.01);
        assert_eq!(
            pll.state(),
            PllState::Locked,
            "phase error {}",
            pll.phase_error()
        );

        pll.update_observation(Some(-4.0), 0.01);
        let increment = pll.commanded_increment();
        pll.set_enabled(true);
        assert_eq!(pll.state(), PllState::Locked);
        assert_eq!(pll.commanded_increment(), increment);

        pll.update_observation(Some(-5.0), 0.02);
        assert_eq!(pll.state(), PllState::Locked);
        pll.update_observation(Some(-5.1), 0.02);
        assert_eq!(pll.state(), PllState::Locked);
        pll.update_observation(Some(0.0), 0.01);
        assert_eq!(pll.state(), PllState::Locked);
    }

    #[test]
    fn invalid_samples_stall_acquisition_but_lose_an_established_lock() {
        let mut pll = Pll::<1>::new(config());
        pll.set_enabled(true);
        pll.update_observation(Some(0.0), 0.02);
        pll.update_observation(None, 0.01);
        pll.update_observation(Some(0.0), 0.01);
        assert_eq!(pll.state(), PllState::Locked);

        for _ in 0..3 {
            pll.update_observation(None, 0.01);
        }
        assert_eq!(pll.state(), PllState::LockLost);
        pll.set_enabled(true);
        assert_eq!(pll.state(), PllState::LockLost);
        pll.set_enabled(false);
        assert_eq!(pll.state(), PllState::Fixed);
    }

    #[test]
    fn saturation_is_bounded_and_faults_only_after_lock() {
        let mut cfg = config();
        cfg.gain = f32::MAX;
        cfg.acquire_timeout_s = 1.0;
        cfg.lock_dwell_s = 0.0;
        let mut pll = Pll::<1>::new(cfg);
        pll.set_enabled(true);

        for _ in 0..10 {
            assert_eq!(pll.update_observation(Some(-90.0), 0.01), 110);
            assert_eq!(pll.state(), PllState::Acquiring);
        }
        pll.update_observation(Some(0.0), 0.01);
        assert_eq!(pll.state(), PllState::Locked);
        for _ in 0..3 {
            let increment = pll.update_observation(Some(-90.0), 0.01);
            assert!((90..=110).contains(&increment));
        }
        assert_eq!(pll.state(), PllState::LockLost);
        assert_eq!(pll.update_observation(Some(f32::NAN), f32::INFINITY), 110);
    }

    #[test]
    fn sub_minimum_amplitude_stalls_without_driving_acquisition() {
        let mut cfg = config();
        cfg.gain = f32::MAX;
        cfg.acquire_timeout_s = 1.0;
        let mut pll = Pll::<1>::new(cfg);
        let mut generator = HarmonicGenerator::<1>::new();
        generator.set_increment(PhaseAccumulator::increment_for(50.0, 1000.0));
        let lut = SinLut::new();
        pll.set_enabled(true);

        for _ in 0..100 {
            assert_eq!(pll.update(generator.step(&lut), 0.0, 0.0, 0.001), 100);
        }
        assert_eq!(pll.state(), PllState::Acquiring);
        assert_eq!(pll.phase_error(), 0.0);
    }

    #[test]
    fn coherent_two_channel_demodulation_locks_measured_phase() {
        const FS: f32 = 1000.0;
        const FREQUENCY: f64 = 50.0;
        let increment = PhaseAccumulator::increment_for(FREQUENCY, FS as f64);
        let mut cfg = config();
        cfg.setpoint_increment = increment;
        cfg.min_increment = increment - 100;
        cfg.max_increment = increment + 100;
        cfg.gain = 0.0;
        cfg.target_phase_deg = -30.0;
        cfg.demod_time_constant_s = 0.1;
        cfg.acquire_timeout_s = 4.0;
        cfg.lock_dwell_s = 0.02;
        let mut pll = Pll::<1>::new(cfg);
        let mut generator = HarmonicGenerator::<1>::new();
        generator.set_increment(increment);
        let lut = SinLut::new();
        let shift = 30.0_f32.to_radians();
        pll.set_enabled(true);

        for _ in 0..3000 {
            let frame = generator.step(&lut);
            let force = frame.cos(0);
            let response = frame.cos(0) * libm::cosf(shift) - frame.sin(0) * libm::sinf(shift);
            pll.update(frame, force, response, 1.0 / FS);
        }

        assert_eq!(
            pll.state(),
            PllState::Locked,
            "phase error {}",
            pll.phase_error()
        );
        assert!(pll.phase_error().abs() < 1.0, "error {}", pll.phase_error());
        assert_eq!(pll.commanded_increment(), increment);
    }
}
