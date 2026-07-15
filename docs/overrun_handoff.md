# CBC W5500 Overrun Investigation Handoff

Date: 2026-07-15

## RESOLVED (later the same day)

The investigation below is retained for history. The root cause was found
and fixed; see "Resolution" at the end of this document. In short:

- The tick body and its wake-up path executed embassy code from XIP flash;
  core-0 network traffic evicted the shared XIP cache and stalled core 1 by
  hundreds of microseconds. Phase-resolved diagnostics (new `wake_phase_*`
  and `t_*_max` parameters) showed every tick phase stretching ~10× under
  TCP load, and the tick rate silently dropping to ~6980 ticks/s during
  captures (missed BUSY edges).
- The fix was introduced as the `rt-sync` feature and is now the only supported
  core-1 architecture: core 1 runs a synchronous loop with no executor,
  busy-polls the BUSY edge-detect
  latch, uses register-level SRAM SPI transfers, and keeps the entire
  per-tick instruction stream in `.data.ram_func`. Result: 0 overruns under
  idle, TCP polling and capture; wake phase constant at 36 µs with no
  measurable spread; loop max 43 µs; 8000.0 ticks/s under full load.
- A second, unrelated failure was exposed and mitigated: lost embassy-time
  alarms (embassy-rs/embassy#3758 class) could freeze all core-0 timers for
  minutes. `helic_fw_common::time_watchdog` bounds this to 50 ms.

The remainder of this document is a historical snapshot of the timing-overrun
investigation for the
`fw-cbc-rig` W5500 setup with the older all-unipolar rtc analogue cape. The
DAQ is physically configured with AD5064 outputs looped back into AD7609
inputs. Treat all DAQ communication as single-client and sequential: do not
run parallel host clients, overlapping scripts, or parallel processes against
the hardware.

## Historical state before resolution

- Standard firmware has been restored after diagnostics.
- Last verified device firmware: `0.1.0 85511be`.
- Sample rate: 8 kHz.
- Outputs were quieted after the final check:
  - `forcing_coeffs = 0`
  - `target_coeffs = 0`
  - `table_mode = 0`
- Final sequential status check after restore:
  - `n_params = 26`
  - `n_sources = 13`
  - `ticks = 67133`
  - `loop_time_last = 285 us`
  - `loop_time_max = 434 us`
  - `clock_jitter = 451 us`
  - `overruns = 42`
  - `tick_timeouts = 0`
  - `records_dropped = 493`
  - `adc_errors = 0`

`records_dropped` is cumulative from earlier diagnostic sessions and did not
increase in the final capture tests noted below.

## Confirmed Good Behaviour

- W5500 link, static IPv4, TCP control, and UDP streaming work.
- AD7609 conversion, BUSY-edge handshake, and 12 MHz SPI read work.
- AD5064 output on known-good channels works through the DAC-to-ADC loopback.
- Parameter discovery, parameter read/write, Fourier coefficient round trips,
  table upload, table playback, and live table re-commit have been exercised.
- UDP capture can deliver full requested captures with zero packet loss.
- During the overrun investigation, `tick_timeouts`, `adc_errors`, and new
  `records_dropped` generally stayed at zero even when `overruns` increased.

At this point in the investigation, the captures were functionally valid but
real-time timing margin under host/network traffic remained unresolved. The
resolution above supersedes this state.

## Symptom

At idle after a fresh flash, the core-1 loop is not continuously over budget.
Probe-only status logs showed typical tick bodies around 45-50 us and a fixed
startup-only overrun count.

Sequential TCP polling and UDP capture reproduce overrun growth. Example
earlier functional capture run:

- 1000-record `adc0,out` smoke capture: zero UDP loss.
- 4000-record table loopback capture: zero UDP loss.
- 6000-record live table re-commit capture: zero UDP loss.
- 8000-record all-13-source capture: zero UDP loss.
- Across that run, `overruns` increased by 1295 while `tick_timeouts`,
  `adc_errors`, and `records_dropped` did not increase.

This is a timing-isolation problem, not a UDP-loss problem.

## Tests Already Run

All hardware interactions were run sequentially with one host client at a
time.

### Baseline

- Probe-only after fresh flash: normal loop body, approximately 45-50 us.
- Idle with host connection: approximately 3-5 overruns/s.
- TCP polling: roughly 120 overruns/s.
- 1000-record `adc0,out` capture: roughly 500 overruns/s.
- UDP packet loss: zero.
- New `records_dropped`, `adc_errors`, `tick_timeouts`: zero.

### Diagnostic Matrix

The matrix tool flashes one variant at a time and measures idle, TCP polling,
and, where supported, UDP capture:

```sh
cd firmware
PYTHONPATH=../host-python uv run --with numpy --env-file /dev/null \
  python tools/overrun_matrix.py --variant <name>
```

Variants and conclusions:

| Variant | Result |
|---|---|
| `baseline` | Reproduced host-traffic-induced overruns. |
| `diag-no-status-log` | No material improvement; status logging is not the cause. |
| `diag-no-udp` | Reduced idle noise, but TCP polling still produced roughly 100 overruns/s. |
| `diag-skip-adc` | Reduced absolute loop cost, but did not remove host-traffic-induced overruns. |
| `diag-skip-dac` | Did not remove host-traffic-induced overruns. |
| `diag-skip-record-enqueue` | Did not remove TCP-poll-induced overruns. |
| `diag-sample-4k` | Reduced, but did not eliminate, the symptom. |
| `diag-wiznet-10mhz` | Reduced capture-induced overruns, but did not materially change TCP polling. |
| `diag-rt-sram` | Did not improve TCP or capture overruns. |

These results make a single ADC, DAC, record-ring, defmt/status-log, UDP
drain, or core-1 XIP instruction-fetch bug unlikely.

### SRAM Hot-Path Diagnostic

Commit `25b579b` added `diag-rt-sram`, placing the synchronous core-1 tick
body and hot board, ADC, DAC, generator, table, and controller helpers in
`.data.ram_func`.

Measured result:

- Idle: approximately 4.7 overruns/s.
- TCP polling: approximately 137 overruns/s.
- 1000-record `adc0,out` capture: approximately 515 overruns/s.
- UDP packet loss: zero.
- New `records_dropped`: zero.
- `adc_errors`: zero.
- `tick_timeouts`: zero.

Conclusion: moving the core-1 instruction hot path into SRAM is not sufficient
and makes XIP instruction fetch in that path unlikely to be the dominant
cause.

### Capture Decimation Sweep

After restoring standard firmware, 1000-record `adc0,out` captures were run
with host capture decimation:

| Decimation | Overruns/s | Notes |
|---:|---:|---|
| 1 | ~458 | zero UDP loss |
| 2 | ~346 | zero UDP loss |
| 4 | ~275 | zero UDP loss |
| 8 | ~243 | zero UDP loss |
| 16 | ~228 | zero UDP loss |

Decimation reduces the symptom but does not solve it. It is an operational
mitigation for heavier streams, not a root-cause fix.

## Current Root-Cause Assessment

The best-supported cause is core-0 network activity disturbing core-1
real-time execution through shared RP2350 resources. The leading mechanisms
are now:

- W5500/SPI0 DMA burst pressure affecting interconnect latency.
- W5500 or network-stack interrupt/task burst behaviour delaying the core-1
  BUSY-edge wait or subsequent execution.
- Shared AHB/peripheral-bus contention while core 1 performs blocking SPI1
  ADC/DAC transfers.

Less likely after testing:

- ADC read alone.
- DAC write alone.
- Record enqueue alone.
- UDP stream task alone.
- Status logging alone.
- Command queue backlog.
- Core-1 RT instruction fetch from XIP flash as the dominant factor.

## Recommended Next Tests

Run these in order, keeping DAQ access sequential.

1. Scope GP14 under traffic.

   GP14 is high while the RT tick body runs. Compare probe-only idle, TCP
   polling, UDP capture, and decimated UDP capture. This will distinguish long
   tick-body execution from late wake-up or scheduling jitter.

2. Add a tick-spacing diagnostic independent of body time.

   Current counters report body elapsed and max observed tick spacing beyond
   nominal. Add or expose enough detail to separate:
   - BUSY falling edge arrives late;
   - task wakes late after BUSY;
   - task wakes on time but body runs long.

3. Test W5500 DMA pressure.

   Add variants that change SPI0/DMA usage or burst size if the Embassy WIZnet
   path permits it. The earlier 10 MHz W5500 test helped capture but not TCP
   polling, so test burst shape, not only clock rate.

4. Test network-task scheduling pressure.

   Add variants to reduce TCP response size or cadence, and to rate-limit UDP
   packet sends. TCP polling alone is enough to trigger overruns, so do not
   focus only on streaming.

5. Re-run the minimum matrix after each intervention.

   Use at least:
   - idle;
   - TCP polling;
   - 1000-record `adc0,out` capture;
   - decimated capture if the intervention targets streaming.

## Useful Commands

Post-resolution note: the async runner and the `rt-sync`/`diag-rt-sram`
features have since been removed. Commands below which select historical
diagnostic variants are retained as investigation evidence and require the
corresponding historical commit; they do not describe the current tree.

Restore standard W5500 firmware:

```sh
cd firmware
timeout 10s cargo run --release -p fw-cbc-rig --no-default-features \
  --features board-w5500
```

Run the SRAM diagnostic only:

```sh
cd firmware
PYTHONPATH=../host-python uv run --with numpy --env-file /dev/null \
  python tools/overrun_matrix.py --variant rt_sram
```

Run a normal host status check:

```sh
PYTHONPATH=host-python uv run --with numpy --env-file /dev/null python - <<'PY'
from helic_daq import Device
with Device("192.168.1.235") as dev:
    print(dev.get("firmware"))
    print(dev.status())
    counters = (
        "ticks", "loop_time_last", "loop_time_max", "clock_jitter",
        "overruns", "tick_timeouts", "records_dropped", "adc_errors",
    )
    print(dict(zip(counters, dev.get(*counters))))
PY
```

## Relevant Commits

- `826164e Firmware: add overrun isolation variants`
- `382c613 Hardware: document overrun isolation matrix`
- `25b579b Firmware: add SRAM hot-path diagnostic`
- `85511be Hardware: record SRAM and decimation overrun tests`

## Resolution

### Phase-resolved diagnostics

New always-on parameters split a tick into phases so late wake-up and long
body are distinguishable without a scope (`diag_reset` := 1 clears them):

- `wake_phase_min` / `wake_phase_max`: µs from the CONVST rising edge
  (PWM slice 4 counter) to the start of the tick body;
- `t_measure_max` / `t_actuate_max` / `t_rest_max`: maxima of the ADC read,
  DAC write and remaining body time.

Baseline (async loop) measurements with these diagnostics:

| Condition | Overruns/s | Ticks/s | Wake phase | t_measure max | t_rest max |
|---|---:|---:|---|---:|---:|
| Idle | 2.8 | 7994 | 16–123 µs | 132 µs | 186 µs |
| TCP polling | 124 | 7686 | 0–124 µs | 157 µs | 230 µs |
| 1000-record capture | 501 | 6980 | 1–123 µs | 141 µs | 213 µs |

Every phase — SPI transfers, pure arithmetic, and wake-up — stretched
roughly tenfold under core-0 network load, and the loop silently skipped up
to 13 % of BUSY edges (the async `InputFuture` re-arms the edge interrupt
per wait, so edges during a long body were lost without incrementing any
counter). This uniform stretching identified shared-XIP-cache instruction
fetch, plus the flash-resident embassy wake path (executor, timer queue,
GPIO IRQ dispatch, cross-core critical sections in `AtomicWaker` and
`with_timeout`), as the cause. The earlier `diag-rt-sram` variant did not
help because the dominant flash footprint was embassy code it never moved.

### Fix: synchronous SRAM isolation

Core 1 enters `run_hot_loop`: a plain synchronous SRAM loop with no executor.
The BUSY falling edge is taken from the IO bank's raw edge-detect
latch by an SRAM spin loop (`BusyEdgeSpinTick`) — the latch stays armed
through the body, so a late tick catches up instead of skipping samples.
ADC/DAC transfers use register-level SRAM SPI routines
(`helic_fw_common::analog_spi`); embassy drivers still perform init. The
whole per-tick instruction stream lives in `.data.ram_func` (~12.5 KiB).

Measured with the same matrix and heavier runs (fw `0.1.0 d965b76`+):

- Idle / TCP polling / capture: **0 overruns/s**, 8000.0 ticks/s,
  `clock_jitter` 0 µs, loop max 43–50 µs.
- `wake_phase_min == wake_phase_max == 36 µs` in every condition (constant
  conversion time + fixed handling; no measurable spread at µs resolution).
- 8000-record all-13-source capture: contiguous indices (all gaps == 1),
  zero UDP loss, zero drops.
- 16000-record 100 Hz loopback: gain 0.997, 6 mV offset, 55 mV RMS residual
  — exactly the expected one-sample actuation lag at 8 kHz.
- Sustained 60000-record capture: contiguous, zero loss, zero drops.

### Second failure found: lost embassy-time alarms

With core 1 no longer re-arming the shared timer queue 8000×/s, a latent
race (embassy-rs/embassy#3758 class; the #3763 fix is present but a
sub-microsecond arm-vs-match hazard remains, cf. pico-sdk PRs #2127/#2190)
occasionally loses the TIMER0 alarm. All core-0 timer-waiting tasks then
sleep until unrelated network traffic schedules a fresh deadline — observed
on hardware as the 5 ms record drain, 1 Hz status log and TCP timeouts all
freezing for ~4 minutes (`records_dropped` grew by 1.75 M) until a host
reconnect revived them. `helic_fw_common::time_watchdog` now re-pends the
time driver's IRQ from TIMER0 alarm 1 every 50 ms, bounding any such stall.
Consider reporting upstream.

### Residual notes

- The startup `records_dropped` burst (~500) is benign: core 1 ticks before
  the network drain task spawns. It stops as soon as `stream_task` starts.
- `records_dropped` from earlier sessions was reset by reflashing; new runs
  stay at the startup burst value indefinitely.
- The same mandatory synchronous SRAM architecture now builds for CBC, whirl,
  and Pico 2W. Whirl and Pico 2W still require complete hardware verification.
