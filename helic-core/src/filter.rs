//! Cascaded biquad (second-order section) IIR filters, Direct Form II
//! transposed, f32 states. Coefficient design helpers run in f64 and are for
//! the non-RT path (init / parameter updates).

/// One second-order section. Transfer function
/// `H(z) = (b0 + b1·z⁻¹ + b2·z⁻²) / (1 + a1·z⁻¹ + a2·z⁻²)`.
#[derive(Clone, Copy, Debug, Default)]
pub struct BiquadCoeffs {
    pub b0: f32,
    pub b1: f32,
    pub b2: f32,
    pub a1: f32,
    pub a2: f32,
}

impl BiquadCoeffs {
    /// Unity pass-through.
    pub const fn identity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
        }
    }

    /// Second-order Butterworth low-pass, bilinear transform.
    /// `fc` = cut-off in Hz, `fs` = sample rate in Hz; requires 0 < fc < fs/2.
    pub fn butterworth_lowpass(fc: f64, fs: f64) -> Self {
        debug_assert!(fc > 0.0 && fc < fs / 2.0);
        let k = libm::tan(core::f64::consts::PI * fc / fs);
        let sqrt2 = core::f64::consts::SQRT_2;
        let norm = 1.0 / (1.0 + sqrt2 * k + k * k);
        Self {
            b0: (k * k * norm) as f32,
            b1: (2.0 * k * k * norm) as f32,
            b2: (k * k * norm) as f32,
            a1: (2.0 * (k * k - 1.0) * norm) as f32,
            a2: ((1.0 - sqrt2 * k + k * k) * norm) as f32,
        }
    }
}

/// Direct Form II transposed state for one section.
#[derive(Clone, Copy, Debug, Default)]
struct BiquadState {
    s1: f32,
    s2: f32,
}

/// A cascade of `N` biquad sections sharing one input/output path.
#[derive(Clone, Copy, Debug)]
pub struct SosFilter<const N: usize> {
    coeffs: [BiquadCoeffs; N],
    state: [BiquadState; N],
}

impl<const N: usize> SosFilter<N> {
    pub const fn identity() -> Self {
        Self {
            coeffs: [BiquadCoeffs::identity(); N],
            state: [BiquadState { s1: 0.0, s2: 0.0 }; N],
        }
    }

    pub fn new(coeffs: [BiquadCoeffs; N]) -> Self {
        Self {
            coeffs,
            state: [BiquadState::default(); N],
        }
    }

    /// Replace coefficients, keeping filter state (for small live retunes;
    /// call `reset` too if the change is large).
    pub fn set_coeffs(&mut self, coeffs: [BiquadCoeffs; N]) {
        self.coeffs = coeffs;
    }

    pub fn reset(&mut self) {
        self.state = [BiquadState::default(); N];
    }

    /// Process one sample through all sections.
    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let mut y = x;
        for (c, s) in self.coeffs.iter().zip(self.state.iter_mut()) {
            let out = c.b0 * y + s.s1;
            s.s1 = c.b1 * y - c.a1 * out + s.s2;
            s.s2 = c.b2 * y - c.a2 * out;
            y = out;
        }
        y
    }
}

impl<const N: usize> Default for SosFilter<N> {
    fn default() -> Self {
        Self::identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state amplitude of the filter response to a sine at `f`.
    fn measure_gain(filter: &mut SosFilter<1>, f: f64, fs: f64) -> f64 {
        let n_settle = (10.0 * fs / f) as usize;
        let n_measure = (4.0 * fs / f) as usize;
        let mut peak = 0.0f64;
        for i in 0..(n_settle + n_measure) {
            let x = libm::sin(core::f64::consts::TAU * f * i as f64 / fs) as f32;
            let y = filter.process(x) as f64;
            if i >= n_settle {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn identity_passes_through() {
        let mut f = SosFilter::<2>::identity();
        for x in [0.0f32, 1.0, -3.5, 100.0] {
            assert_eq!(f.process(x), x);
        }
    }

    #[test]
    fn lowpass_dc_gain_is_unity() {
        let c = BiquadCoeffs::butterworth_lowpass(100.0, 8000.0);
        let mut f = SosFilter::new([c]);
        let mut y = 0.0;
        for _ in 0..10_000 {
            y = f.process(1.0);
        }
        assert!((y - 1.0).abs() < 1e-4, "DC gain {y}");
    }

    #[test]
    fn lowpass_gain_is_3db_down_at_cutoff() {
        let c = BiquadCoeffs::butterworth_lowpass(100.0, 8000.0);
        let mut f = SosFilter::new([c]);
        let gain = measure_gain(&mut f, 100.0, 8000.0);
        assert!(
            (gain - core::f64::consts::FRAC_1_SQRT_2).abs() < 0.01,
            "gain {gain}"
        );
    }

    #[test]
    fn lowpass_attenuates_above_cutoff() {
        let c = BiquadCoeffs::butterworth_lowpass(100.0, 8000.0);
        let mut f = SosFilter::new([c]);
        // Butterworth order 2: -40 dB/decade → gain ≈ 0.01 a decade up.
        let gain = measure_gain(&mut f, 1000.0, 8000.0);
        assert!(gain < 0.02, "gain {gain}");
    }

    #[test]
    fn fourth_order_cascade_attenuates_twice_as_fast() {
        let c = BiquadCoeffs::butterworth_lowpass(100.0, 8000.0);
        let mut f1 = SosFilter::new([c]);
        let g1 = measure_gain(&mut f1, 400.0, 8000.0);
        let mut f2 = SosFilter::new([c, c]);
        let mut peak = 0.0f64;
        let (n_settle, n_measure) = (200_000, 80_000);
        for i in 0..(n_settle + n_measure) {
            let x = libm::sin(core::f64::consts::TAU * 400.0 * i as f64 / 8000.0) as f32;
            let y = f2.process(x) as f64;
            if i >= n_settle {
                peak = peak.max(y.abs());
            }
        }
        assert!((peak - g1 * g1).abs() < 0.01, "{peak} vs {}", g1 * g1);
    }
}
