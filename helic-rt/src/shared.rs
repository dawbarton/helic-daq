//! Atomic state shared between the support and real-time cores.

use core::sync::atomic::{AtomicU32, Ordering};

const REBOOT_IDLE: u32 = 0;
const REBOOT_REQUESTED: u32 = 1;
const REBOOT_QUIESCED: u32 = 2;

/// Current and lifetime values which survive a diagnostics reset.
pub struct Live {
    pub ticks: AtomicU32,
    pub loop_time_last_us: AtomicU32,
}

impl Live {
    const fn new() -> Self {
        Self {
            ticks: AtomicU32::new(0),
            loop_time_last_us: AtomicU32::new(0),
        }
    }
}

/// Exactly the event counters and maxima cleared by `diag_reset`.
pub struct Diagnostics {
    pub loop_time_max_us: AtomicU32,
    pub clock_jitter_us: AtomicU32,
    pub overruns: AtomicU32,
    pub tick_timeouts: AtomicU32,
    pub records_dropped: AtomicU32,
    pub command_backlog_max: AtomicU32,
    pub wake_phase_min_us: AtomicU32,
    pub wake_phase_max_us: AtomicU32,
    pub t_measure_max_us: AtomicU32,
    pub t_actuate_max_us: AtomicU32,
    pub t_rest_max_us: AtomicU32,
    pub safety_clamp_ticks: AtomicU32,
    pub safety_quiet_ticks: AtomicU32,
}

impl Diagnostics {
    const fn new() -> Self {
        Self {
            loop_time_max_us: AtomicU32::new(0),
            clock_jitter_us: AtomicU32::new(0),
            overruns: AtomicU32::new(0),
            tick_timeouts: AtomicU32::new(0),
            records_dropped: AtomicU32::new(0),
            command_backlog_max: AtomicU32::new(0),
            wake_phase_min_us: AtomicU32::new(u32::MAX),
            wake_phase_max_us: AtomicU32::new(0),
            t_measure_max_us: AtomicU32::new(0),
            t_actuate_max_us: AtomicU32::new(0),
            t_rest_max_us: AtomicU32::new(0),
            safety_clamp_ticks: AtomicU32::new(0),
            safety_quiet_ticks: AtomicU32::new(0),
        }
    }

    /// Clear resettable diagnostics without touching lifetime or safety state.
    pub fn reset(&self) {
        self.loop_time_max_us.store(0, Ordering::Relaxed);
        self.clock_jitter_us.store(0, Ordering::Relaxed);
        self.overruns.store(0, Ordering::Relaxed);
        self.tick_timeouts.store(0, Ordering::Relaxed);
        self.records_dropped.store(0, Ordering::Relaxed);
        self.command_backlog_max.store(0, Ordering::Relaxed);
        self.wake_phase_min_us.store(u32::MAX, Ordering::Relaxed);
        self.wake_phase_max_us.store(0, Ordering::Relaxed);
        self.t_measure_max_us.store(0, Ordering::Relaxed);
        self.t_actuate_max_us.store(0, Ordering::Relaxed);
        self.t_rest_max_us.store(0, Ordering::Relaxed);
        self.safety_clamp_ticks.store(0, Ordering::Relaxed);
        self.safety_quiet_ticks.store(0, Ordering::Relaxed);
    }
}

/// Snapshot read by core 1 at the start of the safety decision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SafetyInputs {
    pub armed: bool,
    pub tripped: bool,
}

/// Latched output-safety state with role-named cross-core operations.
///
/// Core 1 may only load the state and monotonically latch a trip. Core 0 owns
/// arming and disarming, which prevents a stale core-1 snapshot from writing
/// `armed = true` after a control-connection loss.
pub struct Safety {
    armed: AtomicU32,
    tripped: AtomicU32,
}

impl Safety {
    const fn new() -> Self {
        Self {
            armed: AtomicU32::new(0),
            tripped: AtomicU32::new(0),
        }
    }

    /// Core 1: load the state used by one tick's safety decision.
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    #[inline]
    pub fn load_inputs(&self) -> SafetyInputs {
        SafetyInputs {
            armed: self.armed.load(Ordering::Relaxed) != 0,
            tripped: self.tripped.load(Ordering::Relaxed) != 0,
        }
    }

    /// Core 1: monotonically latch a fault trip.
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    #[inline]
    pub fn latch_trip(&self) {
        self.tripped.store(1, Ordering::Relaxed);
    }

    /// Core 0: clear any old trip, then arm the output.
    pub fn arm(&self) {
        self.tripped.store(0, Ordering::Relaxed);
        self.armed.store(1, Ordering::Relaxed);
    }

    /// Core 0: quiet the output without clearing its latched trip.
    pub fn disarm(&self) {
        self.armed.store(0, Ordering::Relaxed);
    }

    /// Core 0: encode the safety state for the discovered `safety` parameter.
    pub fn flags(&self, diagnostics: &Diagnostics) -> u32 {
        let inputs = self.load_inputs();
        let clamped = diagnostics.safety_clamp_ticks.load(Ordering::Relaxed) != 0;
        let quieted = diagnostics.safety_quiet_ticks.load(Ordering::Relaxed) != 0;
        inputs.armed as u32
            | ((inputs.tripped as u32) << 1)
            | ((clamped as u32) << 2)
            | ((quieted as u32) << 3)
    }
}

/// Cross-core handshake for a fail-safe, host-requested MCU reboot.
pub struct RebootShared {
    state: AtomicU32,
}

impl RebootShared {
    const fn new() -> Self {
        Self {
            state: AtomicU32::new(REBOOT_IDLE),
        }
    }

    /// Core 0: request quiescence; repeated requests are idempotent.
    pub fn request(&self) {
        let _ = self.state.compare_exchange(
            REBOOT_IDLE,
            REBOOT_REQUESTED,
            Ordering::Release,
            Ordering::Relaxed,
        );
    }

    /// Core 1: report whether output quiescence has been requested.
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    #[inline]
    pub fn is_requested(&self) -> bool {
        self.state.load(Ordering::Acquire) >= REBOOT_REQUESTED
    }

    /// Core 1: publish that the experiment hardware is safe to reset.
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    #[inline]
    pub fn mark_quiesced(&self) {
        self.state.store(REBOOT_QUIESCED, Ordering::Release);
    }

    /// Core 0: report whether core 1 completed its quiescence sequence.
    pub fn is_quiesced(&self) -> bool {
        self.state.load(Ordering::Acquire) == REBOOT_QUIESCED
    }
}

/// All atomic state shared by the two firmware cores for one rig instance.
pub struct RtShared {
    pub live: Live,
    pub diagnostics: Diagnostics,
    pub safety: Safety,
    pub reboot: RebootShared,
}

impl RtShared {
    /// Construct zeroed lifetime state, reset diagnostics, disarmed safety,
    /// and an idle reboot handshake.
    pub const fn new() -> Self {
        Self {
            live: Live::new(),
            diagnostics: Diagnostics::new(),
            safety: Safety::new(),
            reboot: RebootShared::new(),
        }
    }
}

impl Default for RtShared {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_reset_preserves_live_and_safety_state() {
        let shared = RtShared::new();
        shared.live.ticks.store(17, Ordering::Relaxed);
        shared.live.loop_time_last_us.store(23, Ordering::Relaxed);
        shared.diagnostics.overruns.store(3, Ordering::Relaxed);
        shared
            .diagnostics
            .wake_phase_min_us
            .store(36, Ordering::Relaxed);
        shared
            .diagnostics
            .safety_quiet_ticks
            .store(9, Ordering::Relaxed);
        shared.safety.arm();
        shared.safety.latch_trip();

        shared.diagnostics.reset();

        assert_eq!(shared.live.ticks.load(Ordering::Relaxed), 17);
        assert_eq!(shared.live.loop_time_last_us.load(Ordering::Relaxed), 23);
        assert_eq!(shared.diagnostics.overruns.load(Ordering::Relaxed), 0);
        assert_eq!(
            shared.diagnostics.wake_phase_min_us.load(Ordering::Relaxed),
            u32::MAX
        );
        assert_eq!(
            shared
                .diagnostics
                .safety_quiet_ticks
                .load(Ordering::Relaxed),
            0
        );
        assert_eq!(
            shared.safety.load_inputs(),
            SafetyInputs {
                armed: true,
                tripped: true,
            }
        );
    }

    #[test]
    fn disarm_between_snapshot_and_trip_latch_stays_disarmed() {
        let shared = RtShared::new();
        shared.safety.arm();
        let snapshot = shared.safety.load_inputs();
        assert!(snapshot.armed);

        shared.safety.disarm();
        shared.safety.latch_trip();

        assert_eq!(
            shared.safety.load_inputs(),
            SafetyInputs {
                armed: false,
                tripped: true,
            }
        );
    }

    #[test]
    fn safety_flags_include_state_and_resettable_events() {
        let shared = RtShared::new();
        shared.safety.arm();
        shared.safety.latch_trip();
        shared
            .diagnostics
            .safety_clamp_ticks
            .store(1, Ordering::Relaxed);
        shared
            .diagnostics
            .safety_quiet_ticks
            .store(2, Ordering::Relaxed);
        assert_eq!(shared.safety.flags(&shared.diagnostics), 0b1111);
    }

    #[test]
    fn reboot_handshake_is_ordered_and_request_is_idempotent() {
        let shared = RtShared::new();
        assert!(!shared.reboot.is_requested());
        assert!(!shared.reboot.is_quiesced());

        shared.reboot.request();
        shared.reboot.request();
        assert!(shared.reboot.is_requested());
        assert!(!shared.reboot.is_quiesced());

        shared.reboot.mark_quiesced();
        assert!(shared.reboot.is_requested());
        assert!(shared.reboot.is_quiesced());
        shared.reboot.request();
        assert!(shared.reboot.is_quiesced());
    }
}
