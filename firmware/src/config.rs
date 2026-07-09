//! Compile-time configuration: sample-rate preset, harmonic count, and the
//! active controller. This is the one file a user edits to reconfigure the
//! instrument for their experiment.

use cbc_core::controller::PassThrough;

/// Number of harmonics in the periodic target/forcing generators (AGENTS.md:
/// 5–20 depending on budget; 16 costs ~0.8k cycles per series per tick).
pub const HARMONICS: usize = 16;

/// DAC channel driven by the control output (0 and 2 are bipolar).
pub const OUTPUT_CHANNEL: usize = 0;

/// Measuring range of the attached optoNCDT sensor in mm (model-dependent:
/// 10/25/50/100/200/500).
pub const LASER_RANGE_MM: f32 = 50.0;

/// The controller that runs inside every sample tick. Swap the alias (and
/// `make_controller`) to change the control law at compile time, e.g.:
///
/// ```ignore
/// pub type ActiveController = cbc_core::controller::PidController;
/// pub fn make_controller() -> ActiveController {
///     PidController::new(Pid::new(PidConfig { kp: 1.0, ..Default::default() }), Some(0))
/// }
/// ```
pub type ActiveController = PassThrough;

pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Selected sample-rate preset.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;

/// The restricted preset list (AGENTS.md). Each maps to an exact PWM divider
/// and wrap value for the hardware-timed CONVST from the 150 MHz system
/// clock, so the sample clock is crystal-exact.
#[allow(dead_code)] // presets are selected by editing SAMPLE_RATE
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleRate {
    Hz1000,
    Hz2000,
    Hz4000,
    Hz8000,
}

impl SampleRate {
    pub const fn hz(self) -> f32 {
        match self {
            Self::Hz1000 => 1000.0,
            Self::Hz2000 => 2000.0,
            Self::Hz4000 => 4000.0,
            Self::Hz8000 => 8000.0,
        }
    }

    pub const fn dt(self) -> f32 {
        1.0 / self.hz()
    }

    pub const fn period_us(self) -> u64 {
        match self {
            Self::Hz1000 => 1000,
            Self::Hz2000 => 500,
            Self::Hz4000 => 250,
            Self::Hz8000 => 125,
        }
    }

    /// (clock divider, wrap value): 150 MHz / divider / (top + 1) = fs.
    pub const fn pwm_params(self) -> (u8, u16) {
        match self {
            Self::Hz1000 => (4, 37_500 - 1),
            Self::Hz2000 => (2, 37_500 - 1),
            Self::Hz4000 => (2, 18_750 - 1),
            Self::Hz8000 => (2, 9_375 - 1),
        }
    }
}
