//! Hardware-independent safety helpers for the real-time output stage.
//!
//! These are the pure, host-tested pieces of the firmware safety gate: the
//! stall detector that flags a frozen sensor frame counter (a blind-feedback
//! guard). The arming/latching state and the atomics that surface this to the
//! host live in the firmware real-time loop; only the decision worth testing
//! in isolation lives here.

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
