//! Fourier coefficient storage and evaluation.

use crate::lut::SinLut;

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
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amplitude_bound_is_a_bound() {
        let lut = SinLut::new();
        let coefficients = FourierCoeffs {
            mean: -0.1,
            a: [0.3, 0.2],
            b: [0.4, -0.6],
        };
        let bound = coefficients.amplitude_bound();
        for phase in (0..=u16::MAX).map(|phase| u32::from(phase) << 16) {
            let v = coefficients.evaluate(&lut, phase).abs();
            assert!(v <= bound + 1e-6);
        }
    }
}
