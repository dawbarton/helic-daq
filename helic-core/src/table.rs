//! Fixed-capacity waveform table and host-testable playback state.

use crate::PhaseAccumulator;

pub const MAX_TABLE_LEN: usize = 4096;

pub struct WaveTable {
    values: [f32; MAX_TABLE_LEN],
    len: u16,
}

impl WaveTable {
    pub const fn empty() -> Self {
        Self {
            values: [0.0; MAX_TABLE_LEN],
            len: 0,
        }
    }

    pub fn from_slice(values: &[f32]) -> Option<Self> {
        if !(2..=MAX_TABLE_LEN).contains(&values.len()) {
            return None;
        }
        let mut table = Self::empty();
        table.values[..values.len()].copy_from_slice(values);
        table.len = values.len() as u16;
        Some(table)
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn values(&self) -> &[f32] {
        &self.values[..self.len()]
    }

    pub fn prefix(&self, len: usize) -> Option<&[f32]> {
        self.values.get(..len)
    }

    pub fn write_block(&mut self, offset: usize, values: &[f32]) -> bool {
        let Some(end) = offset.checked_add(values.len()) else {
            return false;
        };
        if end > MAX_TABLE_LEN {
            return false;
        }
        self.values[offset..end].copy_from_slice(values);
        true
    }

    pub fn set_len(&mut self, len: usize) -> bool {
        if !(2..=MAX_TABLE_LEN).contains(&len) {
            return false;
        }
        self.len = len as u16;
        true
    }

    /// Linear periodic interpolation using exact fixed-point index
    /// arithmetic. The last knot interpolates back to the first.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn evaluate(&self, theta: u32) -> f32 {
        self.evaluate_with(theta, TableInterpolation::Linear)
    }

    /// Evaluate using the selected interpolation rule.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn evaluate_with(&self, theta: u32, interpolation: TableInterpolation) -> f32 {
        debug_assert!(self.len >= 2);
        let len = self.len();
        let position = theta as u64 * len as u64;
        let index = (position >> 32) as usize;
        debug_assert!(index < len);
        match interpolation {
            TableInterpolation::Linear => {
                let fraction = position as u32 as f32 * (1.0 / 4294967296.0);
                let next = if index + 1 == len { 0 } else { index + 1 };
                let a = self.values[index];
                a + (self.values[next] - a) * fraction
            }
            TableInterpolation::ZeroOrderHold => self.values[index],
        }
    }
}

impl Default for WaveTable {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TableInterpolation {
    ZeroOrderHold = 0,
    #[default]
    Linear = 1,
}

impl TableInterpolation {
    pub const fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::ZeroOrderHold,
            1 => Self::Linear,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum TableMode {
    #[default]
    Off = 0,
    Loop = 1,
    OneShot = 2,
    LockedLoop = 3,
    LockedOneShot = 4,
}

impl TableMode {
    pub const fn from_u32(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Off,
            1 => Self::Loop,
            2 => Self::OneShot,
            3 => Self::LockedLoop,
            4 => Self::LockedOneShot,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum OneShotState {
    #[default]
    Idle,
    Armed,
    Running,
}

pub struct TablePlayer {
    phase: PhaseAccumulator,
    mode: TableMode,
    interpolation: TableInterpolation,
    gain: f32,
    multiplier: u32,
    phase_offset: u32,
    one_shot: OneShotState,
    previous_master: u32,
    locked_progress: u64,
}

impl TablePlayer {
    pub const fn new() -> Self {
        Self {
            phase: PhaseAccumulator::new(),
            mode: TableMode::Off,
            interpolation: TableInterpolation::Linear,
            gain: 1.0,
            multiplier: 1,
            phase_offset: 0,
            one_shot: OneShotState::Idle,
            previous_master: 0,
            locked_progress: 0,
        }
    }

    pub fn set_increment(&mut self, increment: u32) {
        self.phase.set_increment(increment);
    }

    pub fn set_gain(&mut self, gain: f32) {
        self.gain = gain;
    }

    pub fn set_mode(&mut self, mode: TableMode) {
        self.mode = mode;
        self.one_shot = OneShotState::Idle;
    }

    pub fn set_interpolation(&mut self, interpolation: TableInterpolation) {
        self.interpolation = interpolation;
    }

    pub fn set_multiplier(&mut self, multiplier: u32) {
        self.multiplier = multiplier.max(1);
    }

    pub fn set_phase_offset(&mut self, phase_offset: u32) {
        self.phase_offset = phase_offset;
    }

    pub fn trigger(&mut self) {
        match self.mode {
            TableMode::OneShot => {
                self.phase.reset();
                self.one_shot = OneShotState::Running;
            }
            TableMode::LockedOneShot => self.one_shot = OneShotState::Armed,
            _ => {}
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn step(&mut self, table: &WaveTable, master_phase: u32, master_start: bool) -> f32 {
        if table.len() < 2 {
            return 0.0;
        }
        let theta = match self.mode {
            TableMode::Off => return 0.0,
            TableMode::Loop => self.phase.step().0,
            TableMode::OneShot => {
                if self.one_shot != OneShotState::Running {
                    return 0.0;
                }
                let (theta, wrapped) = self.phase.step();
                if wrapped {
                    self.one_shot = OneShotState::Idle;
                    return 0.0;
                }
                theta
            }
            TableMode::LockedLoop => master_phase
                .wrapping_mul(self.multiplier)
                .wrapping_add(self.phase_offset),
            TableMode::LockedOneShot => {
                if self.one_shot == OneShotState::Armed {
                    if !master_start {
                        return 0.0;
                    }
                    self.one_shot = OneShotState::Running;
                    self.previous_master = master_phase;
                    self.locked_progress = 0;
                } else if self.one_shot == OneShotState::Running {
                    let delta = master_phase.wrapping_sub(self.previous_master);
                    self.previous_master = master_phase;
                    self.locked_progress = self
                        .locked_progress
                        .saturating_add(delta as u64 * self.multiplier as u64);
                    if self.locked_progress >= 1 << 32 {
                        self.one_shot = OneShotState::Idle;
                        return 0.0;
                    }
                } else {
                    return 0.0;
                }
                master_phase
                    .wrapping_mul(self.multiplier)
                    .wrapping_add(self.phase_offset)
            }
        };
        self.gain * table.evaluate_with(theta, self.interpolation)
    }
}

impl Default for TablePlayer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_is_exact_between_knots() {
        let table = WaveTable::from_slice(&[0.0, 1.0, 2.0, 3.0]).unwrap();
        assert_eq!(table.evaluate(0), 0.0);
        assert_eq!(table.evaluate(1 << 30), 1.0);
        assert_eq!(table.evaluate(3 << 29), 1.5);
    }

    #[test]
    fn final_segment_wraps_to_first() {
        let table = WaveTable::from_slice(&[0.0, 1.0, 2.0, 3.0]).unwrap();
        assert_eq!(table.evaluate(7 << 29), 1.5);
        assert!(table.evaluate(u32::MAX).abs() < 1e-5);
    }

    #[test]
    fn two_point_and_non_power_of_two_tables_stay_in_bounds() {
        let two = WaveTable::from_slice(&[2.0, 4.0]).unwrap();
        assert_eq!(two.evaluate(1 << 30), 3.0);
        let three = WaveTable::from_slice(&[0.0, 3.0, 6.0]).unwrap();
        for theta in (0..=u16::MAX).map(|value| (value as u32) << 16) {
            assert!(three.evaluate(theta).is_finite());
        }
    }

    #[test]
    fn zero_order_hold_keeps_each_value_until_the_next_knot() {
        let table = WaveTable::from_slice(&[0.0, 1.0, 2.0, 3.0]).unwrap();
        assert_eq!(
            table.evaluate_with(0, TableInterpolation::ZeroOrderHold),
            0.0
        );
        assert_eq!(
            table.evaluate_with((1 << 30) - 1, TableInterpolation::ZeroOrderHold),
            0.0
        );
        assert_eq!(
            table.evaluate_with(1 << 30, TableInterpolation::ZeroOrderHold),
            1.0
        );
        assert_eq!(
            table.evaluate_with(u32::MAX, TableInterpolation::ZeroOrderHold),
            3.0
        );
    }

    #[test]
    fn interpolation_values_are_mathematical_orders() {
        assert_eq!(
            TableInterpolation::from_u32(0),
            Some(TableInterpolation::ZeroOrderHold)
        );
        assert_eq!(
            TableInterpolation::from_u32(1),
            Some(TableInterpolation::Linear)
        );
        assert_eq!(TableInterpolation::from_u32(2), None);
    }

    #[test]
    fn interpolation_change_does_not_reset_playback_phase() {
        let table = WaveTable::from_slice(&[0.0, 1.0]).unwrap();
        let mut player = TablePlayer::new();
        player.set_mode(TableMode::Loop);
        player.set_increment(1 << 30);
        assert_eq!(player.step(&table, 0, false), 0.5);
        player.set_interpolation(TableInterpolation::ZeroOrderHold);
        assert_eq!(player.step(&table, 0, false), 1.0);
    }

    #[test]
    fn free_one_shot_returns_to_zero_after_one_pass() {
        let table = WaveTable::from_slice(&[1.0, 2.0]).unwrap();
        let mut player = TablePlayer::new();
        player.set_mode(TableMode::OneShot);
        player.set_increment(1 << 30);
        player.trigger();
        assert_ne!(player.step(&table, 0, false), 0.0);
        for _ in 0..3 {
            player.step(&table, 0, false);
        }
        assert_eq!(player.step(&table, 0, false), 0.0);
    }

    #[test]
    fn locked_loop_is_an_exact_phase_multiple() {
        let table = WaveTable::from_slice(&[0.0, 1.0, 0.0, -1.0]).unwrap();
        let mut player = TablePlayer::new();
        player.set_mode(TableMode::LockedLoop);
        player.set_multiplier(3);
        for phase in [0, 123_456_789, u32::MAX] {
            assert_eq!(
                player.step(&table, phase, false),
                table.evaluate(phase.wrapping_mul(3))
            );
        }
    }

    #[test]
    fn locked_one_shot_waits_for_master_start() {
        let table = WaveTable::from_slice(&[1.0, 2.0]).unwrap();
        let mut player = TablePlayer::new();
        player.set_mode(TableMode::LockedOneShot);
        player.trigger();
        assert_eq!(player.step(&table, 100, false), 0.0);
        assert_ne!(player.step(&table, 200, true), 0.0);
        assert_ne!(player.step(&table, 200 + (1 << 30), false), 0.0);
        assert_ne!(player.step(&table, 200 + (2 << 30), false), 0.0);
        assert_ne!(player.step(&table, 200 + (3 << 30), false), 0.0);
        assert_eq!(player.step(&table, 200, false), 0.0);
    }
}
