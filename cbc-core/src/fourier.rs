//! Real-time Fourier coefficient estimation by coherent demodulation.
//!
//! Each harmonic of the input is demodulated against sin/cos of the *shared*
//! generator phase, then low-pass filtered with a one-pole IIR. Because the
//! demodulation phases come from the same accumulator that drives the
//! forcing, the estimates are phase-locked to the forcing by construction —
//! exactly what CBC needs.
//!
//! Per-period statistics (mean/variance across periods, as in the rtc duffing
//! rig) are a planned extension; the `period_start` flag from the generator
//! is the hook for them.

use crate::generator::FourierCoeffs;
use crate::lut::SinLut;

/// Estimates `FourierCoeffs<K>` of a signal relative to a fundamental phase.
#[derive(Clone, Copy, Debug)]
pub struct FourierEstimator<const K: usize> {
    estimate: FourierCoeffs<K>,
    /// One-pole smoothing factor per sample, `dt/(tau + dt)`.
    alpha: f32,
}

impl<const K: usize> FourierEstimator<K> {
    /// `tau` is the smoothing time constant in seconds; several fundamental
    /// periods is a sensible starting point (longer = less ripple from
    /// cross-harmonic terms, slower convergence).
    pub fn new(tau: f32, fs: f32) -> Self {
        Self {
            estimate: FourierCoeffs::zero(),
            alpha: 1.0 / (tau * fs + 1.0),
        }
    }

    pub fn set_time_constant(&mut self, tau: f32, fs: f32) {
        self.alpha = 1.0 / (tau * fs + 1.0);
    }

    pub fn reset(&mut self) {
        self.estimate = FourierCoeffs::zero();
    }

    pub fn estimate(&self) -> &FourierCoeffs<K> {
        &self.estimate
    }

    /// Feed one sample of `signal` taken at fundamental `phase` (u32 turns,
    /// from the shared [`crate::PhaseAccumulator`]).
    #[inline]
    pub fn update(&mut self, lut: &SinLut, signal: f32, phase: u32) {
        self.estimate.mean += self.alpha * (signal - self.estimate.mean);
        for k in 0..K {
            let phase_k = (k as u32 + 1).wrapping_mul(phase);
            // E[2·x·cos(kθ)] = aₖ, E[2·x·sin(kθ)] = bₖ for x = Σ aⱼcos + bⱼsin.
            let demod_a = 2.0 * signal * lut.cos(phase_k);
            let demod_b = 2.0 * signal * lut.sin(phase_k);
            self.estimate.a[k] += self.alpha * (demod_a - self.estimate.a[k]);
            self.estimate.b[k] += self.alpha * (demod_b - self.estimate.b[k]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generator::PeriodicGenerator;

    const FS: f64 = 8000.0;

    #[test]
    fn converges_to_known_coefficients() {
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<5>::new();
        gen.phase.set_frequency(20.0, FS);
        gen.coeffs = FourierCoeffs {
            mean: 0.3,
            a: [0.8, 0.0, -0.25, 0.1, 0.0],
            b: [1.2, -0.5, 0.0, 0.0, 0.05],
        };
        // tau = 2 s: residual demodulation ripple scales with the filter
        // corner over the fundamental, ~(1/2πτ)/f0 ≈ 0.4% of each amplitude.
        let mut est = FourierEstimator::<5>::new(2.0, FS as f32);
        // 20 s of data = 10 time constants.
        for _ in 0..160_000 {
            let s = gen.step(&lut);
            est.update(&lut, s.value, gen.phase.phase());
        }
        let e = est.estimate();
        assert!((e.mean - 0.3).abs() < 0.01, "mean {}", e.mean);
        for k in 0..5 {
            assert!((e.a[k] - gen.coeffs.a[k]).abs() < 0.02, "a[{k}] {}", e.a[k]);
            assert!((e.b[k] - gen.coeffs.b[k]).abs() < 0.02, "b[{k}] {}", e.b[k]);
        }
    }

    #[test]
    fn estimate_of_reconstructed_signal_round_trips() {
        // Estimate coefficients, regenerate the signal from them, compare.
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<3>::new();
        gen.phase.set_frequency(35.0, FS);
        gen.coeffs.a[1] = 0.6;
        gen.coeffs.b[2] = -0.4;
        let mut est = FourierEstimator::<3>::new(0.5, FS as f32);
        for _ in 0..40_000 {
            let s = gen.step(&lut);
            est.update(&lut, s.value, gen.phase.phase());
        }
        let mut rms = 0.0f64;
        for _ in 0..1000 {
            let s = gen.step(&lut);
            let rebuilt = est.estimate().evaluate(&lut, gen.phase.phase());
            rms += ((s.value - rebuilt) as f64).powi(2);
        }
        rms = (rms / 1000.0).sqrt();
        assert!(rms < 0.02, "reconstruction rms {rms}");
    }

    #[test]
    fn uncorrelated_harmonic_estimates_stay_near_zero() {
        // Signal at 3× the fundamental must not leak into k=1, 2, 4, 5.
        let lut = SinLut::new();
        let mut gen = PeriodicGenerator::<5>::new();
        gen.phase.set_frequency(20.0, FS);
        gen.coeffs.b[2] = 1.0; // third harmonic only
        let mut est = FourierEstimator::<5>::new(0.5, FS as f32);
        for _ in 0..32_000 {
            let s = gen.step(&lut);
            est.update(&lut, s.value, gen.phase.phase());
        }
        let e = est.estimate();
        for k in [0usize, 1, 3, 4] {
            assert!(
                e.a[k].abs() < 0.02 && e.b[k].abs() < 0.02,
                "leak at k={}",
                k + 1
            );
        }
        assert!((e.b[2] - 1.0).abs() < 0.02);
    }
}
