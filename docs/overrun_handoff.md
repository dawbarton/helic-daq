# CBC W5500 Overrun Investigation Handoff

Date: 2026-07-15

This document summarises the current timing-overrun investigation for the
`fw-cbc-rig` W5500 setup with the older all-unipolar rtc analogue cape. The
DAQ is physically configured with AD5064 outputs looped back into AD7609
inputs. Treat all DAQ communication as single-client and sequential: do not
run parallel host clients, overlapping scripts, or parallel processes against
the hardware.

## Current State

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

The captures are functionally valid. The unresolved issue is real-time timing
margin under host/network traffic.

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
