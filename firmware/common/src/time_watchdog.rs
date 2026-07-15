//! Self-healing kick for the embassy-time alarm (embassy-rs/embassy#3758
//! class of failure).
//!
//! The embassy-rp time driver arms TIMER0 alarm 0 for the earliest deadline
//! in its timer queue. The RP2350 alarm is a 32-bit equality comparator; if
//! an arming write races the counter passing the target microsecond, the
//! match can be lost while the driver still believes an alarm is armed. The
//! integrated timer queue then never re-arms — every timer-waiting task on
//! the core sleeps forever — until some non-timer event (for this firmware:
//! network traffic) happens to schedule a fresh deadline. This was observed
//! on hardware as the record-ring drain, status log and TCP timeouts all
//! stopping for minutes at a time. The pico-sdk fixed the same hazard in its
//! own alarm pool by verifying the ARMED bit after every arm (pico-sdk PRs
//! #2127/#2190); embassy-rp 0.10 (and current git) has no such check.
//!
//! Rather than fork the driver, this module arms the otherwise unused
//! TIMER0 alarm 1 as a 50 ms heartbeat whose handler re-pends TIMER0_IRQ_0.
//! The driver's `check_alarm` then re-evaluates its queue: if the armed
//! alarm was lost, the overdue deadline is noticed and the queue recovers;
//! if all is well, the pend is a few hundred nanoseconds of no-op. Worst-case
//! embassy-time stall is therefore bounded at ~50 ms.
//!
//! Call [`start`] once on core 0 (the core that runs embassy-time's alarm
//! IRQ) after the executor is up, and bind [`TimeWatchdogHandler`] to
//! `TIMER0_IRQ_1` in the experiment's `bind_interrupts!`.

use embassy_rp::interrupt::typelevel::{Handler, Interrupt, TIMER0_IRQ_1};
use embassy_rp::interrupt::InterruptExt;
use embassy_rp::{interrupt, pac};

/// Kick interval. Long enough to be invisible in profiles, short enough
/// that a lost alarm cannot meaningfully disturb host-visible behaviour.
const KICK_INTERVAL_US: u32 = 50_000;

const ALARM_N: usize = 1;
const ALARM_BIT: u8 = 1 << ALARM_N;

/// Arm alarm 1 `KICK_INTERVAL_US` from now, guarding against the very race
/// this module exists to mitigate: if the deadline passed during arming
/// while the alarm still shows armed, the match was lost — disarm and retry.
/// (If it fired during arming, INTR is latched and the IRQ re-arms anyway.)
fn arm_kick() {
    loop {
        let target = pac::TIMER0.timerawl().read().wrapping_add(KICK_INTERVAL_US);
        pac::TIMER0.alarm(ALARM_N).write_value(target);
        let now = pac::TIMER0.timerawl().read();
        let still_ahead = (target.wrapping_sub(now) as i32) > 0;
        let fired = pac::TIMER0.armed().read().armed() & ALARM_BIT == 0;
        if still_ahead || fired {
            return;
        }
        pac::TIMER0.armed().write(|w| w.set_armed(ALARM_BIT));
    }
}

pub struct TimeWatchdogHandler;

impl Handler<TIMER0_IRQ_1> for TimeWatchdogHandler {
    unsafe fn on_interrupt() {
        pac::TIMER0.intr().write(|w| w.set_alarm(ALARM_N, true));
        arm_kick();
        // Re-pending the time driver's own IRQ makes `check_alarm` compare
        // its recorded deadline against now and re-process the queue if the
        // hardware alarm was lost.
        interrupt::TIMER0_IRQ_0.pend();
    }
}

/// Start the heartbeat. Call once, on core 0.
pub fn start() {
    pac::TIMER0.inte().modify(|w| w.set_alarm(ALARM_N, true));
    arm_kick();
    TIMER0_IRQ_1::unpend();
    unsafe { TIMER0_IRQ_1::enable() };
}
