#![no_std]

pub mod comms;
pub mod laser;
pub mod net;
pub mod params;
pub mod rig;
pub mod rt_loop;
pub mod ssi_pio;
pub mod table;

/// Number of harmonics in the periodic target and forcing generators.
pub const HARMONICS: usize = 16;

/// Supported crystal-exact sample-rate presets.
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

    /// `(divider, top)` for a 150 MHz PWM clock.
    pub const fn pwm_params(self) -> (u8, u16) {
        match self {
            Self::Hz1000 => (4, 37_500 - 1),
            Self::Hz2000 => (2, 37_500 - 1),
            Self::Hz4000 => (2, 18_750 - 1),
            Self::Hz8000 => (2, 9_375 - 1),
        }
    }
}
