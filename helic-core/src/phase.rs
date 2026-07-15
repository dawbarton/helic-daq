//! Integer phase accumulator per `docs/periodic_signal_generator.md`.
//!
//! Phase is a `u32` where the full range represents [0, 2π). The accumulator
//! wraps exactly modulo 2³², so all harmonics derived by wrapping
//! multiplication stay phase-coherent with the fundamental forever. Frequency
//! resolution is fs/2³² (≈1.9 µHz at 8 kHz).

/// A wrapping 32-bit phase accumulator.
#[derive(Clone, Copy, Debug, Default)]
pub struct PhaseAccumulator {
    phase: u32,
    increment: u32,
}

impl PhaseAccumulator {
    pub const fn new() -> Self {
        Self {
            phase: 0,
            increment: 0,
        }
    }

    /// Compute the per-sample increment for a fundamental frequency `f0` at
    /// sample rate `fs`. Done in f64 so the quantisation is the u32 rounding,
    /// not float rounding; this runs on parameter updates, never per-sample.
    pub fn increment_for(f0: f64, fs: f64) -> u32 {
        debug_assert!(f0 >= 0.0 && f0 < fs);
        (f0 / fs * 4294967296.0 + 0.5) as u32
    }

    /// Set the frequency. Takes effect from the next `step`; the phase itself
    /// is never reset, so frequency changes are phase-continuous.
    pub fn set_frequency(&mut self, f0: f64, fs: f64) {
        self.increment = Self::increment_for(f0, fs);
    }

    pub fn set_increment(&mut self, increment: u32) {
        self.increment = increment;
    }

    pub fn increment(&self) -> u32 {
        self.increment
    }

    pub fn phase(&self) -> u32 {
        self.phase
    }

    /// Advance one sample and return the new phase. Returns wrap = `true`
    /// when the phase passed the start of a new period (the `period_start`
    /// hook used by per-period processing such as Fourier statistics).
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn step(&mut self) -> (u32, bool) {
        let (phase, wrapped) = self.phase.overflowing_add(self.increment);
        self.phase = phase;
        (phase, wrapped)
    }

    /// Phase of the k-th harmonic: exact mod-2³² wrapping multiply.
    pub fn harmonic(&self, k: u32) -> u32 {
        k.wrapping_mul(self.phase)
    }

    /// Reset the phase to zero (e.g. arming a single-shot signal).
    pub fn reset(&mut self) {
        self.phase = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8000.0;

    #[test]
    fn increment_is_exact_for_power_of_two_ratio() {
        // 1 kHz at 8 kHz is exactly 1/8 of the phase range.
        assert_eq!(PhaseAccumulator::increment_for(1000.0, FS), 1 << 29);
    }

    #[test]
    fn frequency_resolution_error_is_below_one_lsb() {
        let inc = PhaseAccumulator::increment_for(0.1, FS);
        let f_actual = inc as f64 * FS / 4294967296.0;
        assert!((f_actual - 0.1).abs() < FS / 4294967296.0);
    }

    #[test]
    fn phase_wraps_exactly_at_period_boundary() {
        let mut acc = PhaseAccumulator::new();
        acc.set_frequency(1000.0, FS);
        // Exactly 8 samples per period; wrap flag must fire on the 8th step
        // and the phase must return to 0 with no residue.
        for i in 1..=24 {
            let (phase, wrapped) = acc.step();
            assert_eq!(wrapped, i % 8 == 0, "step {i}");
            if wrapped {
                assert_eq!(phase, 0);
            }
        }
    }

    #[test]
    fn harmonics_stay_coherent_with_fundamental() {
        let mut acc = PhaseAccumulator::new();
        acc.set_frequency(37.3, FS);
        for _ in 0..100_000 {
            acc.step();
        }
        // k-th harmonic phase computed incrementally must equal the wrapping
        // multiply for every k: zero drift by construction.
        for k in 1..=20u32 {
            let mut acc_k = PhaseAccumulator::new();
            acc_k.set_increment(k.wrapping_mul(acc.increment()));
            for _ in 0..100_000 {
                acc_k.step();
            }
            assert_eq!(acc_k.phase(), acc.harmonic(k), "harmonic {k}");
        }
    }

    #[test]
    fn frequency_change_is_phase_continuous() {
        let mut acc = PhaseAccumulator::new();
        acc.set_frequency(1000.0, FS);
        acc.step();
        let before = acc.phase();
        acc.set_frequency(2000.0, FS);
        assert_eq!(acc.phase(), before);
        let (after, _) = acc.step();
        assert_eq!(after, before.wrapping_add(1 << 30));
    }
}
