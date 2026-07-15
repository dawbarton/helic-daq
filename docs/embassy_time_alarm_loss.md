# embassy-rp time driver: alarm loss on RP2350 freezes all timers (issue draft)

This file is a self-contained, copy-paste-ready GitHub issue for
embassy-rs/embassy describing the alarm-loss failure we observed on
hardware on 2026-07-15 and mitigate locally with
`helic_fw_common::time_watchdog`. Everything below the rule is the issue
body.

---

**Title:** RP2350: time driver alarm occasionally lost; all `embassy-time`
timers freeze until an unrelated `schedule_wake` re-arms the queue (#3758
symptom, observed with #3763 already applied)

## Environment

- Chip: RP2350A (W5500-EVB-Pico2), 150 MHz, both cores in use
- Target: `thumbv8m.main-none-eabihf`
- `embassy-rp` 0.10.0 (`rp235xa`, `time-driver`, `critical-section-impl`)
- `embassy-time` 0.5.1, `embassy-time-queue-utils` 0.3.2 (integrated
  queue), `embassy-executor` 0.10.0 (thread-mode executor per core)
- Core 0: thread-mode executor running `embassy-net` (W5500 MACRAW), a TCP
  server, a UDP task with a 5 ms `Ticker`, a 1 Hz `Ticker` status log and a
  500 ms LED blink. Core 1: no executor and no `embassy-time` use (plain
  synchronous loop; it never calls `schedule_wake`).

## Observed behaviour

With the device idle (no network client), all timer-waiting tasks on
core 0 spontaneously stopped being woken, roughly 80 s after the last
network activity, and stayed frozen for ~4 minutes until a host opened a
TCP connection, at which point everything resumed at once. During the
freeze:

- A 5 ms `Ticker` loop stopped running. We can bound the freeze precisely
  from a side effect: the loop drains a SPSC ring fed at 8 kHz by core 1,
  and the ring's overflow counter grew by 1,751,967 ≈ 219 s × 8 kHz.
- TCP became unresponsive (the socket layer's timeouts are also
  embassy-time based).
- TIMER0 itself never stopped: the device's µs uptime (read from
  `TIMERAWL/H`) stayed exactly consistent with a hardware-timed 8 kHz tick
  counter maintained independently on core 1, so this is not the
  `DBGPAUSE`/debug-halt case.
- Core 1 (which does not use embassy-time) was unaffected throughout.

This matches the symptom of #3758 ("tasks awaiting a Timer stop waking;
creating a new task that awaits a Timer unfreezes it"). However, the fix
from #3763 (clearing INTR inside the critical section at the top of
`check_alarm`) **is present** in the version we run, so a residual loss
path remains.

## Why recovery needs external traffic (integrated queue death state)

With the integrated timer queue, `schedule_wake` returns `true` (causing a
re-arm) only when the task's queue item is not currently enqueued, or when
its deadline moves earlier. Once the hardware alarm is lost while every
timer-waiting task already has a queued item, no task is ever woken, so no
task ever re-inserts, so nothing ever calls `set_alarm` again: the death is
self-sustaining. The first waker that fires for a *non-timer* reason (in
our case: W5500 RX causing the net stack to schedule a fresh poll deadline)
inserts a new item, `schedule_wake` returns `true`, the driver re-arms, and
`next_expiration` releases every overdue task at once — exactly the
all-at-once recovery we observed.

## Suspected residual race

We could not capture the TIMER0 register state at the instant of loss (the
failure is rare), so the arming race below is our best hypothesis rather
than a proven mechanism; the observed facts above are solid.

`set_alarm` in `embassy-rp/src/time_driver.rs`:

```rust
TIMER.alarm(n).write_value(timestamp as u32);

let now = self.now();
if timestamp <= now {
    // If alarm timestamp has passed the alarm will not fire.
    // Disarm the alarm and return `false` to indicate that.
    TIMER.armed().write(|w| w.set_armed(1 << n));
    alarm.timestamp.set(u64::MAX);
    false
} else {
    true
}
```

The RP2350 alarm is an equality comparator on the low 32 counter bits. If
the arming write lands in the same microsecond in which the counter passes
the target, the match can be lost while the subsequent `now` read still
returns the previous microsecond, so the `timestamp <= now` guard does not
catch it: the driver believes an alarm is armed that will not fire until
the counter wraps (~71.6 minutes), and with the integrated queue the system
is dead until then (or until external traffic, above).

The pico-sdk fixed the same hazard class in its alarm pool by verifying the
hardware `ARMED` bit after arming instead of trusting a time comparison
(raspberrypi/pico-sdk PRs #2127 and #2190, prompted by "repeating timers
stop" reports on RP2350). The RP2350 alarm auto-clears its `ARMED` bit when
it fires, so `ARMED` distinguishes "fired while we were arming" (INTR is
latched, the IRQ will handle it) from "armed but the match was missed"
(lost). Something like:

```rust
TIMER.alarm(n).write_value(timestamp as u32);
let now = self.now();
if timestamp <= now {
    // Target passed while arming. If ARMED is still set the match was
    // missed and will never fire; disarm and report failure so the
    // caller re-evaluates the queue. If ARMED cleared, the alarm fired
    // and INTR is latched, so the IRQ path takes over either way.
    if TIMER.armed().read().armed() & (1 << n) != 0 {
        TIMER.armed().write(|w| w.set_armed(1 << n));
    }
    alarm.timestamp.set(u64::MAX);
    false
} else {
    true
}
```

(plus, conservatively, the same `ARMED`-after-arm verification for the
"not elapsed, arm it again" path in `check_alarm`).

## Frequency / conditions

Observed once in an afternoon of hardware testing, and only after we moved
our 8 kHz real-time loop off `embassy-time` entirely. Before that change,
core 1 called `with_timeout` per 125 µs tick, i.e. ~8000 `schedule_wake`
inserts per second, each of which re-arms the alarm — any lost alarm was
masked within 125 µs. We suspect many RP2 applications hide this bug the
same way, and it only becomes visible when the timer queue is quiet.

## Workaround

We arm the otherwise unused TIMER0 alarm 1 with its own IRQ
(`TIMER0_IRQ_1`) as a 50 ms heartbeat whose handler only re-arms itself
(with an `ARMED`-verified arm) and pends `TIMER0_IRQ_0`. `check_alarm` then
compares the driver's recorded deadline against `now` and recovers the
queue, bounding any alarm loss to 50 ms. Happy to share the ~80-line
module.
