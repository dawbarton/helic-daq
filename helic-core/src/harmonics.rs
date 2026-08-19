//! Shared phase-coherent harmonic bases for multi-signal synthesis and demodulation.

use crate::{FourierCoeffs, PhaseAccumulator, SinLut};

/// One master phase and its phase-coherent harmonic sine/cosine basis.
pub struct HarmonicFrame<const H: usize> {
    pub phase: u32,
    pub period_start: bool,
    cos: [f32; H],
    sin: [f32; H],
}

impl<const H: usize> HarmonicFrame<H> {
    pub const fn zero() -> Self {
        Self {
            phase: 0,
            period_start: false,
            cos: [0.0; H],
            sin: [0.0; H],
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn cos(&self, harmonic: usize) -> f32 {
        self.cos[harmonic]
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn sin(&self, harmonic: usize) -> f32 {
        self.sin[harmonic]
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn phase_turns(&self) -> f32 {
        self.phase as f32 * (1.0 / 4_294_967_296.0)
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn project(&self, coefficients: &FourierCoeffs<H>) -> f32 {
        let mut value = coefficients.mean;
        for harmonic in 0..H {
            value += coefficients.a[harmonic] * self.cos[harmonic]
                + coefficients.b[harmonic] * self.sin[harmonic];
        }
        value
    }
}

/// Advances one master phase and retains a borrowed harmonic basis per tick.
pub struct HarmonicGenerator<const H: usize> {
    phase: PhaseAccumulator,
    frame: HarmonicFrame<H>,
}

impl<const H: usize> HarmonicGenerator<H> {
    pub const fn new() -> Self {
        Self {
            phase: PhaseAccumulator::new(),
            frame: HarmonicFrame::zero(),
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn set_increment(&mut self, increment: u32) {
        self.phase.set_increment(increment);
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn increment(&self) -> u32 {
        self.phase.increment()
    }

    pub fn frequency_hz(&self, sample_rate_hz: f32) -> f32 {
        self.phase.increment() as f32 * (sample_rate_hz / 4_294_967_296.0)
    }

    pub fn reset(&mut self) {
        self.phase.reset();
        self.frame = HarmonicFrame::zero();
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn step(&mut self, lut: &SinLut) -> &HarmonicFrame<H> {
        let (phase, period_start) = self.phase.step();
        self.frame.phase = phase;
        self.frame.period_start = period_start;
        for harmonic in 0..H {
            let harmonic_phase = (harmonic as u32 + 1).wrapping_mul(phase);
            self.frame.cos[harmonic] = lut.cos(harmonic_phase);
            self.frame.sin[harmonic] = lut.sin(harmonic_phase);
        }
        &self.frame
    }
}

impl<const H: usize> Default for HarmonicGenerator<H> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_matches_direct_fourier_evaluation() {
        let lut = SinLut::new();
        let mut generator = HarmonicGenerator::<4>::new();
        generator.set_increment(123_456_789);
        let coefficients = FourierCoeffs {
            mean: 0.25,
            a: [1.0, -0.5, 0.125, 0.0],
            b: [0.0, 0.75, -0.25, 0.0625],
        };

        for _ in 0..100 {
            let frame = generator.step(&lut);
            assert_eq!(
                frame.project(&coefficients),
                coefficients.evaluate(&lut, frame.phase)
            );
        }
    }

    #[test]
    fn period_start_comes_from_accumulator_overflow() {
        let lut = SinLut::new();
        let mut generator = HarmonicGenerator::<1>::new();
        generator.set_increment(1 << 30);

        for step in 1..=8 {
            assert_eq!(generator.step(&lut).period_start, step % 4 == 0);
        }
    }
}
