//! Interpolated sine lookup table per `docs/periodic_signal_generator.md`.
//!
//! 1024 entries over one period (4 KiB) plus a duplicated first entry so
//! interpolation never needs an index wrap. Max interpolation error is
//! ≈(2π/1024)²/8 ≈ 4.7×10⁻⁶, well below a 16-bit DAC LSB.

const BITS: u32 = 10;
const SIZE: usize = 1 << BITS;
const FRAC_BITS: u32 = 32 - BITS;
const FRAC_SCALE: f32 = 1.0 / (1u32 << FRAC_BITS) as f32;

/// Sine table indexed by a `u32` phase (full range = one period).
pub struct SinLut {
    table: [f32; SIZE + 1],
}

impl SinLut {
    /// Build the table. Runs once at startup; not for the RT path.
    pub fn new() -> Self {
        let mut table = [0.0f32; SIZE + 1];
        for (i, entry) in table.iter_mut().enumerate() {
            *entry = libm::sinf(i as f32 * (core::f32::consts::TAU / SIZE as f32));
        }
        table[SIZE] = table[0];
        Self { table }
    }

    /// sin(phase), with phase in u32 turns (2³² = 2π), linearly interpolated.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn sin(&self, phase: u32) -> f32 {
        let idx = (phase >> FRAC_BITS) as usize;
        let frac = (phase & ((1 << FRAC_BITS) - 1)) as f32 * FRAC_SCALE;
        let a = self.table[idx];
        let b = self.table[idx + 1];
        a + (b - a) * frac
    }

    /// cos(phase): same table, quarter-period phase offset (exact, wrapping).
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn cos(&self, phase: u32) -> f32 {
        self.sin(phase.wrapping_add(1 << 30))
    }
}

impl Default for SinLut {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sin_error_is_below_interpolation_bound() {
        let lut = SinLut::new();
        let mut max_err = 0.0f32;
        // Sweep phases that do not coincide with table nodes.
        for i in 0..100_000u32 {
            let phase = i.wrapping_mul(42_949_673); // ~1% of full scale steps
            let exact = libm::sin(phase as f64 / 4294967296.0 * core::f64::consts::TAU) as f32;
            let err = (lut.sin(phase) - exact).abs();
            max_err = max_err.max(err);
        }
        assert!(max_err < 6e-6, "max error {max_err}");
    }

    #[test]
    fn cos_matches_shifted_sin() {
        let lut = SinLut::new();
        for phase in [0u32, 1 << 30, 1 << 31, 0xDEAD_BEEF, u32::MAX] {
            let exact = libm::cos(phase as f64 / 4294967296.0 * core::f64::consts::TAU) as f32;
            assert!((lut.cos(phase) - exact).abs() < 6e-6);
        }
    }

    #[test]
    fn endpoints_are_exact() {
        let lut = SinLut::new();
        assert_eq!(lut.sin(0), 0.0);
        assert_eq!(lut.sin(1 << 31), libm::sinf(core::f32::consts::PI));
    }
}
