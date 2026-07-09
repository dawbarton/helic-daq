//! Signal generators: periodic (Fourier series) and arbitrary (interpolated
//! look-up table), both driven by [`PhaseAccumulator`]s so frequency changes
//! are glitch-free and phase-continuous.

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

/// Playback mode for the arbitrary generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArbMode {
    /// Play the table once, then hold the final sample.
    SingleShot,
    /// Loop the table (the last sample interpolates back towards the first).
    Periodic,
}

/// Run state of the arbitrary generator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArbState {
    Idle,
    Running,
    /// Single-shot playback finished; output holds the final table value.
    Finished,
}

/// Arbitrary signal generator: a caller-owned table (typically 1000–2000
/// samples) with linear interpolation, paced by a phase accumulator so the
/// playback timescale is adjusted exactly like the periodic generator's
/// frequency. One full phase revolution plays the whole table.
pub struct ArbitraryGenerator<'a> {
    table: &'a [f32],
    phase: PhaseAccumulator,
    mode: ArbMode,
    state: ArbState,
}

impl<'a> ArbitraryGenerator<'a> {
    /// `table` must contain at least 2 samples.
    pub fn new(table: &'a [f32], mode: ArbMode) -> Self {
        debug_assert!(table.len() >= 2);
        Self {
            table,
            phase: PhaseAccumulator::new(),
            mode,
            state: ArbState::Idle,
        }
    }

    /// Set the playback rate: `f0` full-table playbacks per second.
    pub fn set_rate(&mut self, f0: f64, fs: f64) {
        self.phase
            .set_increment(PhaseAccumulator::increment_for(f0, fs));
    }

    pub fn state(&self) -> ArbState {
        self.state
    }

    pub fn mode(&self) -> ArbMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: ArbMode) {
        self.mode = mode;
    }

    /// Arm playback from the start of the table.
    pub fn trigger(&mut self) {
        self.phase.reset();
        self.state = ArbState::Running;
    }

    pub fn stop(&mut self) {
        self.state = ArbState::Idle;
    }

    /// Interpolated table value at a u32 phase. One full phase revolution
    /// spans `n-1` intervals in single-shot mode (table[0] → table[n-1]
    /// exactly) and `n` intervals in periodic mode (the extra interval wraps
    /// back towards table[0]).
    #[inline]
    fn sample_at(&self, phase: u32) -> f32 {
        let n = self.table.len();
        let intervals = match self.mode {
            ArbMode::SingleShot => n - 1,
            ArbMode::Periodic => n,
        };
        // 64-bit multiply keeps the index exact for any table length.
        let pos = (phase as u64) * (intervals as u64);
        let idx = (pos >> 32) as usize;
        let frac = (pos as u32) as f32 * (1.0 / 4294967296.0);
        let a = self.table[idx];
        let b = if idx + 1 < n {
            self.table[idx + 1]
        } else {
            self.table[0]
        };
        a + (b - a) * frac
    }

    /// Advance one sample. Idle output is the first table value before any
    /// trigger; after a single-shot completes it holds the last table value.
    #[inline]
    pub fn step(&mut self) -> GenSample {
        match self.state {
            ArbState::Idle => GenSample {
                value: self.table[0],
                period_start: false,
            },
            ArbState::Finished => GenSample {
                value: self.table[self.table.len() - 1],
                period_start: false,
            },
            ArbState::Running => {
                let (phase, wrapped) = self.phase.step();
                if wrapped && self.mode == ArbMode::SingleShot {
                    self.state = ArbState::Finished;
                    return GenSample {
                        value: self.table[self.table.len() - 1],
                        period_start: false,
                    };
                }
                GenSample {
                    value: self.sample_at(phase),
                    period_start: wrapped,
                }
            }
        }
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

    #[test]
    fn arbitrary_ramp_reproduces_linear_interpolation() {
        // A ramp table: interpolation of a linear function is exact.
        let table: [f32; 101] = core::array::from_fn(|i| i as f32);
        let mut arb = ArbitraryGenerator::new(&table, ArbMode::SingleShot);
        arb.set_rate(1.0, 1000.0); // whole table over 1000 samples
        arb.trigger();
        for i in 1..1000 {
            let s = arb.step();
            let expected = i as f32 / 1000.0 * 100.0;
            assert!(
                (s.value - expected).abs() < 1e-3,
                "sample {i}: {} vs {expected}",
                s.value
            );
        }
    }

    #[test]
    fn single_shot_holds_final_value() {
        let table = [0.0f32, 1.0, 2.0, 3.0];
        let mut arb = ArbitraryGenerator::new(&table, ArbMode::SingleShot);
        arb.set_rate(10.0, 1000.0); // 100 samples per playback
        arb.trigger();
        for _ in 0..100 {
            arb.step();
        }
        assert_eq!(arb.state(), ArbState::Finished);
        for _ in 0..50 {
            assert_eq!(arb.step().value, 3.0);
        }
    }

    #[test]
    fn periodic_mode_loops_and_flags_period_start() {
        let table = [0.0f32, 1.0, 0.0, -1.0];
        let mut arb = ArbitraryGenerator::new(&table, ArbMode::Periodic);
        arb.set_rate(10.0, 1000.0); // 100 samples per loop
        arb.trigger();
        let starts: usize = (0..1000).filter(|_| arb.step().period_start).count();
        assert_eq!(starts, 10);
        assert_eq!(arb.state(), ArbState::Running);
    }

    #[test]
    fn idle_before_trigger_outputs_first_sample() {
        let table = [0.5f32, 1.0];
        let mut arb = ArbitraryGenerator::new(&table, ArbMode::SingleShot);
        arb.set_rate(1.0, 1000.0);
        assert_eq!(arb.step().value, 0.5);
    }
}
