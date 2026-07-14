//! Fourier signal generation driven by a [`PhaseAccumulator`] so frequency
//! changes are glitch-free and phase-continuous.

use crate::lut::SinLut;
use crate::phase::PhaseAccumulator;

/// Fourier series coefficients: `mean + Σₖ a[k-1]·cos(kθ) + b[k-1]·sin(kθ)`
/// for k = 1..=K.
#[derive(Clone, Copy, Debug)]
pub struct FourierCoeffs<const K: usize> {
    pub mean: f32,
    pub a: [f32; K],
    pub b: [f32; K],
}

impl<const K: usize> FourierCoeffs<K> {
    pub const fn zero() -> Self {
        Self {
            mean: 0.0,
            a: [0.0; K],
            b: [0.0; K],
        }
    }

    /// Evaluate the series at the given fundamental phase (u32 turns).
    /// Harmonic phases are exact wrapping multiples, so they remain
    /// phase-locked to the fundamental indefinitely.
    #[inline]
    pub fn evaluate(&self, lut: &SinLut, phase: u32) -> f32 {
        let mut sum = self.mean;
        for k in 0..K {
            let phase_k = (k as u32 + 1).wrapping_mul(phase);
            sum += self.a[k] * lut.cos(phase_k) + self.b[k] * lut.sin(phase_k);
        }
        sum
    }

    /// Worst-case amplitude: `|mean| + Σ√(aₖ²+bₖ²)`. Useful for checking the
    /// output cannot clip before committing new coefficients.
    pub fn amplitude_bound(&self) -> f32 {
        let mut sum = if self.mean < 0.0 {
            -self.mean
        } else {
            self.mean
        };
        for k in 0..K {
            sum += libm::sqrtf(self.a[k] * self.a[k] + self.b[k] * self.b[k]);
        }
        sum
    }
}

impl<const K: usize> Default for FourierCoeffs<K> {
    fn default() -> Self {
        Self::zero()
    }
}

/// Output of one generator step.
#[derive(Clone, Copy, Debug)]
pub struct GenSample {
    pub value: f32,
    /// True on the sample where the fundamental phase wrapped: the
    /// `period_start` hook for per-period processing (Fourier statistics,
    /// period-locked triggers).
    pub period_start: bool,
}

/// Periodic signal generator: Fourier series driven by a phase accumulator.
#[derive(Clone, Copy, Debug)]
pub struct PeriodicGenerator<const K: usize> {
    pub phase: PhaseAccumulator,
    pub coeffs: FourierCoeffs<K>,
}

impl<const K: usize> PeriodicGenerator<K> {
    pub const fn new() -> Self {
        Self {
            phase: PhaseAccumulator::new(),
            coeffs: FourierCoeffs::zero(),
        }
    }

    /// Advance one sample and evaluate.
    #[inline]
    pub fn step(&mut self, lut: &SinLut) -> GenSample {
        let (phase, period_start) = self.phase.step();
        GenSample {
            value: self.coeffs.evaluate(lut, phase),
            period_start,
        }
    }
}

impl<const K: usize> Default for PeriodicGenerator<K> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: f64 = 8000.0;

    #[test]
    fn pure_tone_matches_reference_sine() {
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<3>::new();
        gen.phase.set_frequency(50.0, FS);
        gen.coeffs.b[0] = 1.0;
        for i in 1..=1000 {
            let s = gen.step(&lut);
            let exact = libm::sin(core::f64::consts::TAU * 50.0 * i as f64 / FS) as f32;
            assert!((s.value - exact).abs() < 1e-5, "sample {i}");
        }
    }

    #[test]
    fn harmonics_sum_matches_direct_evaluation() {
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<5>::new();
        gen.phase.set_frequency(123.4, FS);
        gen.coeffs = FourierCoeffs {
            mean: 0.25,
            a: [0.5, -0.3, 0.2, 0.0, 0.1],
            b: [1.0, 0.4, -0.2, 0.05, 0.0],
        };
        for i in 1..=5000 {
            let s = gen.step(&lut);
            let theta = core::f64::consts::TAU * 123.4 * i as f64 / FS;
            let mut exact = 0.25;
            for k in 1..=5 {
                exact += gen.coeffs.a[k - 1] as f64 * libm::cos(k as f64 * theta)
                    + gen.coeffs.b[k - 1] as f64 * libm::sin(k as f64 * theta);
            }
            assert!((s.value - exact as f32).abs() < 5e-5, "sample {i}");
        }
    }

    #[test]
    fn period_start_fires_once_per_period() {
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<1>::new();
        gen.phase.set_frequency(1000.0, FS); // 8 samples per period
        let starts: usize = (0..800).filter(|_| gen.step(&lut).period_start).count();
        assert_eq!(starts, 100);
    }

    #[test]
    fn amplitude_bound_is_a_bound() {
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<2>::new();
        gen.phase.set_frequency(77.7, FS);
        gen.coeffs = FourierCoeffs {
            mean: -0.1,
            a: [0.3, 0.2],
            b: [0.4, -0.6],
        };
        let bound = gen.coeffs.amplitude_bound();
        for _ in 0..100_000 {
            let v = gen.step(&lut).value.abs();
            assert!(v <= bound + 1e-6);
        }
    }
}
