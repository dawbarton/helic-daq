//! Bounded single-drive-point phase-locked frequency tracking.
//!
//! The PLL coherently demodulates measured excitation and response against a
//! shared harmonic frame. Acquisition failure is non-faulting; only loss of a
//! previously established lock enters the latched fault state.

use core::marker::PhantomData;

use crate::HarmonicFrame;

const RAD_TO_DEG: f32 = 180.0 / core::f32::consts::PI;
/// Conservative settling time after resetting or explicitly reacquiring.
pub const PLL_WARMUP_TIME_CONSTANTS: f32 = 5.0;

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
    pub centre_increment: u32,
    pub min_increment: u32,
    pub max_increment: u32,
    /// Proportional correction in phase-increment quanta per degree.
    pub proportional_gain: f32,
    /// Integral correction in phase-increment quanta per degree-second.
    pub integral_gain: f32,
    pub target_phase_deg: f32,
    /// Response-path delay minus excitation-path delay.
    pub delay_s: f32,
    pub dc_time_constant_s: f32,
    pub demod_time_constant_s: f32,
    pub min_excitation_amplitude: f32,
    pub min_response_amplitude: f32,
    pub lock_phase_tolerance_deg: f32,
    pub unlock_phase_tolerance_deg: f32,
    /// Maximum peak-to-peak unquantised correction during acquisition.
    pub lock_frequency_tolerance: f32,
    pub lock_dwell_s: f32,
    pub unlock_dwell_s: f32,
    pub acquire_timeout_s: f32,
    pub saturation_dwell_s: f32,
}

impl Default for PllConfig {
    fn default() -> Self {
        Self {
            centre_increment: 0,
            min_increment: 0,
            max_increment: u32::MAX,
            proportional_gain: 0.0,
            integral_gain: 0.0,
            target_phase_deg: 0.0,
            delay_s: 0.0,
            dc_time_constant_s: 0.1,
            demod_time_constant_s: 0.1,
            min_excitation_amplitude: 0.0,
            min_response_amplitude: 0.0,
            lock_phase_tolerance_deg: 5.0,
            unlock_phase_tolerance_deg: 10.0,
            lock_frequency_tolerance: f32::MAX,
            lock_dwell_s: 0.1,
            unlock_dwell_s: 0.1,
            acquire_timeout_s: 5.0,
            saturation_dwell_s: 0.1,
        }
    }
}

impl PllConfig {
    pub fn is_valid<const H: usize>(&self) -> bool {
        H > 0
            && self.min_increment <= self.centre_increment
            && self.centre_increment <= self.max_increment
            && self.proportional_gain.is_finite()
            && self.integral_gain.is_finite()
            && valid_phase_degrees(self.target_phase_deg)
            && self.delay_s.is_finite()
            && positive_finite(self.dc_time_constant_s)
            && non_negative_finite(self.demod_time_constant_s)
            && non_negative_finite(self.min_excitation_amplitude)
            && non_negative_finite(self.min_response_amplitude)
            && non_negative_finite(self.lock_phase_tolerance_deg)
            && non_negative_finite(self.unlock_phase_tolerance_deg)
            && self.lock_phase_tolerance_deg <= self.unlock_phase_tolerance_deg
            && non_negative_finite(self.lock_frequency_tolerance)
            && non_negative_finite(self.lock_dwell_s)
            && non_negative_finite(self.unlock_dwell_s)
            && non_negative_finite(self.acquire_timeout_s)
            && non_negative_finite(self.saturation_dwell_s)
    }
}

const fn non_negative_finite(value: f32) -> bool {
    value >= 0.0 && value.is_finite()
}

const fn positive_finite(value: f32) -> bool {
    value > 0.0 && value.is_finite()
}

#[derive(Clone, Copy, Debug, Default)]
struct DcEstimator {
    mean: f32,
    initialised: bool,
}

impl DcEstimator {
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn remove(&mut self, sample: f32, dt_s: f32, time_constant_s: f32) -> f32 {
        if !self.initialised {
            self.mean = sample;
            self.initialised = true;
        } else {
            let beta = dt_s / (time_constant_s + dt_s);
            self.mean += beta * (sample - self.mean);
        }
        sample - self.mean
    }
}

#[derive(Clone, Copy, Debug)]
struct Demodulated {
    phase_deg: f32,
    excitation_amplitude_sq: f32,
    response_amplitude_sq: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct FundamentalDemodulator {
    excitation_dc: DcEstimator,
    response_dc: DcEstimator,
    excitation_cos: f32,
    excitation_sin: f32,
    response_cos: f32,
    response_sin: f32,
}

impl FundamentalDemodulator {
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn update<const H: usize>(
        &mut self,
        frame: &HarmonicFrame<H>,
        excitation: f32,
        response: f32,
        dt_s: f32,
        dc_time_constant_s: f32,
        demod_time_constant_s: f32,
    ) -> Option<Demodulated> {
        if !excitation.is_finite() || !response.is_finite() || !valid_dt(dt_s) {
            return None;
        }
        let excitation = self
            .excitation_dc
            .remove(excitation, dt_s, dc_time_constant_s);
        let response = self.response_dc.remove(response, dt_s, dc_time_constant_s);
        let alpha = if demod_time_constant_s == 0.0 {
            1.0
        } else {
            dt_s / (demod_time_constant_s + dt_s)
        };
        let cos = frame.cos(0);
        let sin = frame.sin(0);
        self.excitation_cos += alpha * (2.0 * excitation * cos - self.excitation_cos);
        self.excitation_sin += alpha * (2.0 * excitation * sin - self.excitation_sin);
        self.response_cos += alpha * (2.0 * response * cos - self.response_cos);
        self.response_sin += alpha * (2.0 * response * sin - self.response_sin);

        let excitation_amplitude_sq =
            self.excitation_cos * self.excitation_cos + self.excitation_sin * self.excitation_sin;
        let response_amplitude_sq =
            self.response_cos * self.response_cos + self.response_sin * self.response_sin;
        let excitation_phase = atan2_degrees(-self.excitation_sin, self.excitation_cos);
        let response_phase = atan2_degrees(-self.response_sin, self.response_cos);
        Some(Demodulated {
            phase_deg: wrap_degrees(response_phase - excitation_phase),
            excitation_amplitude_sq,
            response_amplitude_sq,
        })
    }
}

/// Fundamental measured-excitation/measured-response PLL with bounded frequency.
pub struct Pll<const H: usize> {
    state: PllState,
    centre_increment: u32,
    min_increment: u32,
    max_increment: u32,
    commanded_increment: u32,
    proportional_gain: f32,
    integral_gain: f32,
    integral_correction: f32,
    unquantised_correction: f32,
    target_phase_deg: f32,
    delay_s: f32,
    dc_time_constant_s: f32,
    demod_time_constant_s: f32,
    min_excitation_amplitude_sq: f32,
    min_response_amplitude_sq: f32,
    lock_phase_tolerance_deg: f32,
    unlock_phase_tolerance_deg: f32,
    lock_frequency_tolerance: f32,
    lock_dwell_s: f32,
    unlock_dwell_s: f32,
    acquire_timeout_s: f32,
    saturation_dwell_s: f32,
    measured_phase_deg: f32,
    phase_error_deg: f32,
    excitation_amplitude: f32,
    response_amplitude: f32,
    observation_valid: bool,
    saturated: bool,
    warmup_elapsed_s: f32,
    acquire_elapsed_s: f32,
    lock_elapsed_s: f32,
    unlock_elapsed_s: f32,
    saturation_elapsed_s: f32,
    lock_correction_min: f32,
    lock_correction_max: f32,
    demodulator: FundamentalDemodulator,
    harmonics: PhantomData<[f32; H]>,
}

impl<const H: usize> Pll<H> {
    pub fn new(config: PllConfig) -> Self {
        assert!(config.is_valid::<H>(), "invalid PLL configuration");
        Self {
            state: PllState::Fixed,
            centre_increment: config.centre_increment,
            min_increment: config.min_increment,
            max_increment: config.max_increment,
            commanded_increment: config.centre_increment,
            proportional_gain: config.proportional_gain,
            integral_gain: config.integral_gain,
            integral_correction: 0.0,
            unquantised_correction: 0.0,
            target_phase_deg: config.target_phase_deg,
            delay_s: config.delay_s,
            dc_time_constant_s: config.dc_time_constant_s,
            demod_time_constant_s: config.demod_time_constant_s,
            min_excitation_amplitude_sq: square(config.min_excitation_amplitude),
            min_response_amplitude_sq: square(config.min_response_amplitude),
            lock_phase_tolerance_deg: config.lock_phase_tolerance_deg,
            unlock_phase_tolerance_deg: config.unlock_phase_tolerance_deg,
            lock_frequency_tolerance: config.lock_frequency_tolerance,
            lock_dwell_s: config.lock_dwell_s,
            unlock_dwell_s: config.unlock_dwell_s,
            acquire_timeout_s: config.acquire_timeout_s,
            saturation_dwell_s: config.saturation_dwell_s,
            measured_phase_deg: f32::NAN,
            phase_error_deg: f32::NAN,
            excitation_amplitude: f32::NAN,
            response_amplitude: f32::NAN,
            observation_valid: false,
            saturated: false,
            warmup_elapsed_s: 0.0,
            acquire_elapsed_s: 0.0,
            lock_elapsed_s: 0.0,
            unlock_elapsed_s: 0.0,
            saturation_elapsed_s: 0.0,
            lock_correction_min: 0.0,
            lock_correction_max: 0.0,
            demodulator: FundamentalDemodulator::default(),
            harmonics: PhantomData,
        }
    }

    /// Update from raw measured excitation and response samples.
    ///
    /// Fourier phase is `atan2(-b, a)`, and measured phase is response minus
    /// excitation. `current_increment` must be the increment which generated
    /// `frame`, so delay compensation and telemetry refer to the same sample.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn update(
        &mut self,
        frame: &HarmonicFrame<H>,
        excitation: f32,
        response: f32,
        current_increment: u32,
        dt_s: f32,
    ) -> u32 {
        if matches!(self.state, PllState::Fixed | PllState::LockLost) || !valid_dt(dt_s) {
            return self.commanded_increment;
        }
        let demodulated = self.demodulator.update(
            frame,
            excitation,
            response,
            dt_s,
            self.dc_time_constant_s,
            self.demod_time_constant_s,
        );
        self.update_observation(demodulated, current_increment, dt_s)
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn update_observation(
        &mut self,
        observation: Option<Demodulated>,
        current_increment: u32,
        dt_s: f32,
    ) -> u32 {
        self.observation_valid = false;
        self.warmup_elapsed_s += dt_s;
        let warmup_s =
            PLL_WARMUP_TIME_CONSTANTS * self.dc_time_constant_s.max(self.demod_time_constant_s);
        if self.warmup_elapsed_s < warmup_s {
            return self.commanded_increment;
        }
        self.acquire_elapsed_s += dt_s;

        let qualified = observation.filter(|value| {
            value.phase_deg.is_finite()
                && value.excitation_amplitude_sq >= self.min_excitation_amplitude_sq
                && value.response_amplitude_sq >= self.min_response_amplitude_sq
        });
        if let Some(value) = qualified {
            let frequency_hz = current_increment as f32 * (1.0 / 4_294_967_296.0) / dt_s;
            self.measured_phase_deg =
                wrap_degrees_full(value.phase_deg + 360.0 * frequency_hz * self.delay_s);
            self.phase_error_deg = wrap_degrees(self.measured_phase_deg - self.target_phase_deg);
            self.excitation_amplitude = libm::sqrtf(value.excitation_amplitude_sq);
            self.response_amplitude = libm::sqrtf(value.response_amplitude_sq);
            self.observation_valid = true;
            self.saturated = self.apply_correction(dt_s);
        } else {
            self.measured_phase_deg = f32::NAN;
            self.phase_error_deg = f32::NAN;
            self.excitation_amplitude = f32::NAN;
            self.response_amplitude = f32::NAN;
            self.saturated = false;
        }

        match self.state {
            PllState::Acquiring => self.update_acquisition(dt_s),
            PllState::Locked => self.update_lock(dt_s),
            PllState::Fixed | PllState::LockLost => {}
        }
        self.commanded_increment
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn update_acquisition(&mut self, dt_s: f32) {
        if self.observation_valid && self.phase_error_deg.abs() <= self.lock_phase_tolerance_deg {
            if self.lock_elapsed_s == 0.0 {
                self.lock_correction_min = self.unquantised_correction;
                self.lock_correction_max = self.unquantised_correction;
            } else {
                self.lock_correction_min =
                    self.lock_correction_min.min(self.unquantised_correction);
                self.lock_correction_max =
                    self.lock_correction_max.max(self.unquantised_correction);
            }
            if self.lock_correction_max - self.lock_correction_min <= self.lock_frequency_tolerance
            {
                self.lock_elapsed_s += dt_s;
                if self.lock_elapsed_s >= self.lock_dwell_s {
                    self.enter_locked();
                    return;
                }
            } else {
                self.restart_lock_dwell();
            }
        } else {
            self.restart_lock_dwell();
        }
        if self.acquire_elapsed_s >= self.acquire_timeout_s {
            self.acquisition_failed();
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn update_lock(&mut self, dt_s: f32) {
        let outside_lock =
            !self.observation_valid || self.phase_error_deg.abs() > self.unlock_phase_tolerance_deg;
        self.unlock_elapsed_s = if outside_lock {
            self.unlock_elapsed_s + dt_s
        } else {
            0.0
        };
        self.saturation_elapsed_s = if self.saturated {
            self.saturation_elapsed_s + dt_s
        } else {
            0.0
        };
        if (outside_lock && self.unlock_elapsed_s >= self.unlock_dwell_s)
            || (self.saturated && self.saturation_elapsed_s >= self.saturation_dwell_s)
        {
            self.state = PllState::LockLost;
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply_correction(&mut self, dt_s: f32) -> bool {
        let integral_increment = self.integral_gain * self.phase_error_deg * dt_s;
        let candidate_integral = self.integral_correction + integral_increment;
        let candidate = self.proportional_gain * self.phase_error_deg + candidate_integral;
        let min_correction = -((self.centre_increment - self.min_increment) as f32);
        let max_correction = (self.max_increment - self.centre_increment) as f32;
        let saturated_high = candidate > max_correction;
        let saturated_low = candidate < min_correction;
        if !((saturated_high && integral_increment > 0.0)
            || (saturated_low && integral_increment < 0.0))
        {
            self.integral_correction = candidate_integral;
        }
        let correction = self.proportional_gain * self.phase_error_deg + self.integral_correction;
        self.unquantised_correction = correction.clamp(min_correction, max_correction);
        self.commanded_increment = add_signed_correction(
            self.centre_increment,
            self.unquantised_correction,
            self.min_increment,
            self.max_increment,
        );
        saturated_high || saturated_low
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn restart_lock_dwell(&mut self) {
        self.lock_elapsed_s = 0.0;
        self.lock_correction_min = self.unquantised_correction;
        self.lock_correction_max = self.unquantised_correction;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn enter_locked(&mut self) {
        self.state = PllState::Locked;
        self.unlock_elapsed_s = 0.0;
        self.saturation_elapsed_s = 0.0;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn acquisition_failed(&mut self) {
        self.reset_loop(true);
        self.state = PllState::Fixed;
    }

    /// Enter acquisition only from `Fixed`; replaying enable preserves lock.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_enabled(&mut self, enabled: bool) {
        if !enabled {
            self.reset();
        } else if self.state == PllState::Fixed {
            self.reset_loop(true);
            self.state = PllState::Acquiring;
        }
    }

    /// Explicitly reacquire from fixed or locked operation at the current frequency.
    /// `LockLost` is deliberately latched and cannot be cleared here.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn reacquire(&mut self) {
        if matches!(self.state, PllState::Fixed | PllState::Locked) {
            self.integral_correction =
                signed_difference(self.commanded_increment, self.centre_increment);
            self.unquantised_correction = self.integral_correction;
            self.clear_qualification();
            self.state = PllState::Acquiring;
        }
    }

    pub const fn state(&self) -> PllState {
        self.state
    }
    pub const fn measured_phase(&self) -> f32 {
        self.measured_phase_deg
    }
    pub const fn phase_error(&self) -> f32 {
        self.phase_error_deg
    }
    pub const fn excitation_amplitude(&self) -> f32 {
        self.excitation_amplitude
    }
    pub const fn response_amplitude(&self) -> f32 {
        self.response_amplitude
    }
    pub const fn observation_valid(&self) -> bool {
        self.observation_valid
    }
    pub const fn commanded_increment(&self) -> u32 {
        self.commanded_increment
    }
    pub const fn saturated(&self) -> bool {
        self.saturated
    }

    /// Return to the fixed centre and clear demodulation and loop state.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn reset(&mut self) {
        self.reset_loop(true);
        self.state = PllState::Fixed;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn reset_loop(&mut self, reset_demodulator: bool) {
        self.commanded_increment = self.centre_increment;
        self.integral_correction = 0.0;
        self.unquantised_correction = 0.0;
        self.saturated = false;
        if reset_demodulator {
            self.demodulator = FundamentalDemodulator::default();
        }
        self.clear_qualification();
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn clear_qualification(&mut self) {
        self.measured_phase_deg = f32::NAN;
        self.phase_error_deg = f32::NAN;
        self.excitation_amplitude = f32::NAN;
        self.response_amplitude = f32::NAN;
        self.observation_valid = false;
        self.warmup_elapsed_s = 0.0;
        self.acquire_elapsed_s = 0.0;
        self.lock_elapsed_s = 0.0;
        self.unlock_elapsed_s = 0.0;
        self.saturation_elapsed_s = 0.0;
        self.lock_correction_min = self.unquantised_correction;
        self.lock_correction_max = self.unquantised_correction;
    }

    pub fn set_centre_increment(&mut self, increment: u32) -> bool {
        if !(self.min_increment..=self.max_increment).contains(&increment) {
            return false;
        }
        self.centre_increment = increment;
        if self.state == PllState::Fixed {
            self.commanded_increment = increment;
            self.integral_correction = 0.0;
            self.unquantised_correction = 0.0;
        }
        true
    }

    pub fn set_min_increment(&mut self, increment: u32) -> bool {
        if increment > self.centre_increment {
            return false;
        }
        self.min_increment = increment;
        self.commanded_increment = self.commanded_increment.max(increment);
        true
    }

    pub fn set_max_increment(&mut self, increment: u32) -> bool {
        if increment < self.centre_increment {
            return false;
        }
        self.max_increment = increment;
        self.commanded_increment = self.commanded_increment.min(increment);
        true
    }

    pub fn set_proportional_gain(&mut self, gain: f32) -> bool {
        if !gain.is_finite() {
            return false;
        }
        self.proportional_gain = gain;
        true
    }

    pub fn set_integral_gain(&mut self, gain: f32) -> bool {
        if !gain.is_finite() {
            return false;
        }
        self.integral_gain = gain;
        true
    }

    pub fn set_target_phase(&mut self, degrees: f32) -> bool {
        if !valid_phase_degrees(degrees) {
            return false;
        }
        let changed = self.target_phase_deg != degrees;
        self.target_phase_deg = degrees;
        if changed {
            self.reacquire();
        }
        true
    }

    pub fn set_delay(&mut self, seconds: f32) -> bool {
        if !seconds.is_finite() {
            return false;
        }
        self.delay_s = seconds;
        true
    }

    pub fn set_dc_time_constant(&mut self, seconds: f32) -> bool {
        if !positive_finite(seconds) {
            return false;
        }
        self.dc_time_constant_s = seconds;
        true
    }

    pub fn set_demod_time_constant(&mut self, seconds: f32) -> bool {
        if !non_negative_finite(seconds) {
            return false;
        }
        self.demod_time_constant_s = seconds;
        true
    }

    pub fn set_min_excitation_amplitude(&mut self, amplitude: f32) -> bool {
        if !non_negative_finite(amplitude) {
            return false;
        }
        self.min_excitation_amplitude_sq = square(amplitude);
        true
    }

    pub fn set_min_response_amplitude(&mut self, amplitude: f32) -> bool {
        if !non_negative_finite(amplitude) {
            return false;
        }
        self.min_response_amplitude_sq = square(amplitude);
        true
    }

    pub fn set_lock_phase_tolerance(&mut self, degrees: f32) -> bool {
        if !non_negative_finite(degrees) || degrees > self.unlock_phase_tolerance_deg {
            return false;
        }
        self.lock_phase_tolerance_deg = degrees;
        true
    }

    pub fn set_unlock_phase_tolerance(&mut self, degrees: f32) -> bool {
        if !non_negative_finite(degrees) || degrees < self.lock_phase_tolerance_deg {
            return false;
        }
        self.unlock_phase_tolerance_deg = degrees;
        true
    }

    pub fn set_lock_frequency_tolerance(&mut self, tolerance: f32) -> bool {
        if !non_negative_finite(tolerance) {
            return false;
        }
        self.lock_frequency_tolerance = tolerance;
        true
    }

    pub fn set_lock_dwell(&mut self, seconds: f32) -> bool {
        set_non_negative(&mut self.lock_dwell_s, seconds)
    }
    pub fn set_unlock_dwell(&mut self, seconds: f32) -> bool {
        set_non_negative(&mut self.unlock_dwell_s, seconds)
    }
    pub fn set_acquire_timeout(&mut self, seconds: f32) -> bool {
        set_non_negative(&mut self.acquire_timeout_s, seconds)
    }
    pub fn set_saturation_dwell(&mut self, seconds: f32) -> bool {
        set_non_negative(&mut self.saturation_dwell_s, seconds)
    }
}

fn set_non_negative(destination: &mut f32, value: f32) -> bool {
    if !non_negative_finite(value) {
        return false;
    }
    *destination = value;
    true
}

const fn square(value: f32) -> f32 {
    value * value
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
fn signed_difference(value: u32, centre: u32) -> f32 {
    if value >= centre {
        (value - centre) as f32
    } else {
        -((centre - value) as f32)
    }
}

#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
fn add_signed_correction(centre: u32, correction: f32, min: u32, max: u32) -> u32 {
    if correction >= 0.0 {
        centre.saturating_add((correction + 0.5) as u32).min(max)
    } else {
        centre.saturating_sub((-correction + 0.5) as u32).max(min)
    }
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

/// Reduce a finite phase offset, including multiple turns.
#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
fn wrap_degrees_full(mut degrees: f32) -> f32 {
    const DIRECT_TURNS: f32 = 2_000_000_000.0;
    if degrees.abs() > 360.0 * DIRECT_TURNS {
        let mut modulus = 360.0 * 1.0e30;
        while modulus >= 360.0 {
            if degrees.abs() >= modulus {
                let quotient = (degrees / modulus) as i32;
                degrees -= quotient as f32 * modulus;
            }
            modulus *= 0.5;
        }
    }
    let turns = (degrees / 360.0) as i32;
    wrap_degrees(degrees - turns as f32 * 360.0)
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

    const DT: f32 = 0.001;

    fn config() -> PllConfig {
        PllConfig {
            centre_increment: 100,
            min_increment: 90,
            max_increment: 110,
            proportional_gain: 1.0,
            integral_gain: 1.0,
            target_phase_deg: 0.0,
            delay_s: 0.0,
            dc_time_constant_s: 0.001,
            demod_time_constant_s: 0.0,
            min_excitation_amplitude: 0.1,
            min_response_amplitude: 0.1,
            lock_phase_tolerance_deg: 2.0,
            unlock_phase_tolerance_deg: 5.0,
            lock_frequency_tolerance: 1.0,
            lock_dwell_s: 0.003,
            unlock_dwell_s: 0.003,
            acquire_timeout_s: 0.05,
            saturation_dwell_s: 0.003,
        }
    }

    fn observation(phase_deg: f32) -> Demodulated {
        Demodulated {
            phase_deg,
            excitation_amplitude_sq: 1.0,
            response_amplitude_sq: 1.0,
        }
    }

    fn warm(pll: &mut Pll<1>) {
        for _ in 0..5 {
            pll.update_observation(Some(observation(0.0)), 100, DT);
        }
    }

    #[test]
    fn conventional_phase_reports_a_lag_as_negative() {
        const FS: f32 = 1000.0;
        let increment = PhaseAccumulator::increment_for(50.0, FS as f64);
        let mut cfg = config();
        cfg.centre_increment = increment;
        cfg.min_increment = increment - 100;
        cfg.max_increment = increment + 100;
        cfg.proportional_gain = 0.0;
        cfg.integral_gain = 0.0;
        cfg.target_phase_deg = -30.0;
        cfg.dc_time_constant_s = 0.1;
        cfg.demod_time_constant_s = 0.1;
        cfg.lock_frequency_tolerance = 100.0;
        cfg.lock_dwell_s = 0.02;
        cfg.acquire_timeout_s = 4.0;
        let mut pll = Pll::<1>::new(cfg);
        let mut generator = HarmonicGenerator::<1>::new();
        generator.set_increment(increment);
        let lut = SinLut::new();
        let shift = 30.0_f32.to_radians();
        pll.set_enabled(true);
        for _ in 0..3000 {
            let frame = generator.step(&lut);
            let excitation = 2.0 + frame.cos(0);
            let response =
                25.0 + frame.cos(0) * libm::cosf(shift) + frame.sin(0) * libm::sinf(shift);
            pll.update(frame, excitation, response, increment, 1.0 / FS);
        }
        assert_eq!(pll.state(), PllState::Locked);
        assert!((pll.measured_phase() + 30.0).abs() < 1.0);
        assert!(pll.phase_error().abs() < 1.0);
    }

    #[test]
    fn warmup_precedes_acquisition_timeout() {
        let mut pll = Pll::<1>::new(config());
        pll.set_enabled(true);
        for _ in 0..4 {
            pll.update_observation(None, 100, DT);
        }
        assert_eq!(pll.state(), PllState::Acquiring);
        assert!(!pll.observation_valid());
        for _ in 0..60 {
            pll.update_observation(None, 100, DT);
        }
        assert_eq!(pll.state(), PllState::Fixed);
    }

    #[test]
    fn delay_compensation_wraps_multiple_turns_both_ways() {
        assert!((wrap_degrees_full(432.0) - 72.0).abs() < 1e-4);
        assert!((wrap_degrees_full(-432.0) + 72.0).abs() < 1e-4);
        assert!((wrap_degrees_full(3779.0) - 179.0).abs() < 1e-4);
        assert!((wrap_degrees_full(-3779.0) + 179.0).abs() < 1e-4);
    }

    #[test]
    fn sub_quantum_integral_accumulates_above_f32_absolute_precision_limit() {
        let centre = PhaseAccumulator::increment_for(30.0, 8000.0);
        let mut cfg = config();
        cfg.centre_increment = centre;
        cfg.min_increment = centre - 100;
        cfg.max_increment = centre + 100;
        cfg.proportional_gain = 0.0;
        cfg.integral_gain = 0.25;
        cfg.lock_phase_tolerance_deg = 0.0;
        cfg.acquire_timeout_s = 10.0;
        let mut pll = Pll::<1>::new(cfg);
        pll.set_enabled(true);
        warm(&mut pll);
        for _ in 0..5 {
            pll.update_observation(Some(observation(1.0)), centre, 1.0);
        }
        assert_eq!(pll.commanded_increment(), centre + 1);
    }

    #[test]
    fn lock_loss_is_latched_against_enable_and_reacquire() {
        let mut pll = Pll::<1>::new(config());
        pll.set_enabled(true);
        warm(&mut pll);
        for _ in 0..3 {
            pll.update_observation(Some(observation(0.0)), 100, DT);
        }
        assert_eq!(pll.state(), PllState::Locked);
        for _ in 0..3 {
            pll.update_observation(None, 100, DT);
        }
        assert_eq!(pll.state(), PllState::LockLost);
        pll.reacquire();
        pll.set_enabled(true);
        assert_eq!(pll.state(), PllState::LockLost);
        pll.reset();
        assert_eq!(pll.state(), PllState::Fixed);
    }

    #[test]
    fn reacquire_preserves_frequency_and_replayed_enable_preserves_lock() {
        let mut pll = Pll::<1>::new(config());
        pll.set_enabled(true);
        warm(&mut pll);
        for _ in 0..3 {
            pll.update_observation(Some(observation(0.0)), 100, DT);
        }
        assert_eq!(pll.state(), PllState::Locked);
        let frequency = pll.commanded_increment();
        pll.set_enabled(true);
        assert_eq!(pll.state(), PllState::Locked);
        pll.reacquire();
        assert_eq!(pll.state(), PllState::Acquiring);
        assert_eq!(pll.commanded_increment(), frequency);
    }

    #[test]
    fn public_setters_reject_invalid_values_without_panicking() {
        let mut pll = Pll::<1>::new(config());
        assert!(!pll.set_centre_increment(89));
        assert!(!pll.set_min_increment(101));
        assert!(!pll.set_max_increment(99));
        assert!(!pll.set_proportional_gain(f32::NAN));
        assert!(!pll.set_integral_gain(f32::INFINITY));
        assert!(!pll.set_target_phase(180.0));
        assert!(!pll.set_delay(f32::NAN));
        assert!(!pll.set_dc_time_constant(0.0));
        assert!(!pll.set_demod_time_constant(-1.0));
        assert!(!pll.set_min_excitation_amplitude(-1.0));
        assert!(!pll.set_min_response_amplitude(f32::NAN));
        assert!(!pll.set_lock_phase_tolerance(20.0));
        assert!(!pll.set_unlock_phase_tolerance(1.0));
        assert!(!pll.set_lock_frequency_tolerance(-1.0));
        assert!(!pll.set_lock_dwell(-1.0));
        assert!(!pll.set_unlock_dwell(f32::NAN));
        assert!(!pll.set_acquire_timeout(-1.0));
        assert!(!pll.set_saturation_dwell(f32::INFINITY));
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
}
