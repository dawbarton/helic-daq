//! Hardware-independent safety helpers for the real-time output stage.
//!
//! These are the pure, host-tested pieces of the firmware safety gate: the
//! amplitude clamp that keeps a biased DAC channel inside a safe voltage
//! window, and a stall detector that flags a frozen sensor frame counter (a
//! blind-feedback guard). The arming/latching state and the atomics that
//! surface these to the host live in the firmware real-time loop; only the
//! decisions worth testing in isolation live here.

/// Clamp a signed command that is applied as `mid_rail + out` on a DAC
/// channel, so the resulting channel voltage stays within `[floor_v, ceil_v]`.
/// Returns the clamped command expressed about the bias point, i.e. in the
/// same units as `out`.
///
/// This is the hard amplitude limit: it is applied after the controller,
/// forcing and table contributions have been summed, so no single stage can
/// drive the channel past the window.
#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
pub fn clamp_channel_command(out: f32, mid_rail: f32, floor_v: f32, ceil_v: f32) -> f32 {
    out.clamp(floor_v - mid_rail, ceil_v - mid_rail)
}

/// Detects a stalled monotonic frame counter: a sensor task that has stopped
/// publishing new frames (link lost, sensor unpowered) leaves feedback blind.
/// [`observe`](Self::observe) is called once per real-time tick with the
/// latest counter value and returns whether the source is now considered
/// stale.
#[derive(Clone, Copy, Debug)]
pub struct StaleCounter {
    last: u32,
    ticks_since_change: u32,
    limit: u32,
}

impl StaleCounter {
    /// `limit` is the number of consecutive unchanged ticks tolerated before
    /// the source is flagged stale.
    pub const fn new(limit: u32) -> Self {
        Self {
            last: 0,
            ticks_since_change: 0,
            limit,
        }
    }

    /// Observe the current counter value; returns `true` if the source is
    /// now stale. A source that never advances from its initial value is
    /// flagged once `limit` unchanged ticks have elapsed.
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn observe(&mut self, current: u32) -> bool {
        if current != self.last {
            self.last = current;
            self.ticks_since_change = 0;
        } else {
            self.ticks_since_change = self.ticks_since_change.saturating_add(1);
        }
        self.ticks_since_change > self.limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_command_within_channel_window() {
        // Unipolar interim board: mid-rail 2.048 V, safe window [0.096, 4.0] V
        // → command window [-1.952, +1.952] V about the bias point.
        let mid = 2.048;
        let (floor, ceil) = (0.096, 4.0);
        assert_eq!(clamp_channel_command(0.0, mid, floor, ceil), 0.0);
        assert_eq!(clamp_channel_command(1.0, mid, floor, ceil), 1.0);
        assert!((clamp_channel_command(5.0, mid, floor, ceil) - (ceil - mid)).abs() < 1e-6);
        assert!((clamp_channel_command(-5.0, mid, floor, ceil) - (floor - mid)).abs() < 1e-6);
    }

    #[test]
    fn stale_counter_flags_frozen_source_then_recovers() {
        let mut s = StaleCounter::new(3);
        for f in 1..=10 {
            assert!(!s.observe(f), "advancing frames must not be stale");
        }
        assert!(!s.observe(10)); // 1 unchanged
        assert!(!s.observe(10)); // 2
        assert!(!s.observe(10)); // 3
        assert!(s.observe(10)); // 4 > limit → stale
        assert!(s.observe(10));
        assert!(!s.observe(11), "a fresh frame clears the stall");
    }

    #[test]
    fn stale_counter_flags_source_that_never_starts() {
        let mut s = StaleCounter::new(2);
        assert!(!s.observe(0));
        assert!(!s.observe(0));
        assert!(s.observe(0));
    }
}
