# Hardware verification status

Last updated 2026-07-22. Read this before a hardware session and update the
verification boundary, failures and fitted-hardware assumptions afterwards.

## Verified on hardware

The `fw-cbc-rig` path has been exercised on a W5500-EVB-Pico2 with the older
rtc analogue cape:

- W5500 link, ARP, TCP control and UDP streaming;
- AD7609 conversion, BUSY handshake and SPI read at 12 MHz;
- AD5064 output on channels A, C and D, including DAC-to-ADC DC and AC
  loopback;
- hardware sample-rate presets at 1, 2, 4 and 8 kHz;
- scalar and complete 33-element Fourier parameter round trips, read-only
  rejection and sample-boundary application;
- arbitrary table playback and atomic re-commit through the DAC-to-ADC
  loopback path. A 128-sample positive waveform streamed with zero UDP packet
  loss, `table == out`, and ADC0 fit residual of 3.3 mV RMS after gain/offset
  fit. A live re-commit from 0.45 V to 1.65 V during a 6000-record stream
  produced only the two expected levels and zero UDP packet loss;
- phase accumulator, Fourier generator and streaming at 8 kHz using a
  commanded 100 Hz sine;
- finite streaming of all 13 currently discovered `cbc-rig` sources at 8 kHz
  for 8000 records with zero UDP packet loss and no increase in
  `records_dropped`;
- closed-loop PID on ADC0 with live gain tuning. A 2 V to 3 V step settled to
  2% in approximately 39 ms in the test loopback;
- hardware protocol rejection of `StreamSetup` while a stream is active with
  `Busy`, and a non-finite `freq` write with `BadValue` while preserving the
  previous finite value;
- a disconnected laser UART no longer starves core 0 when GP1 has the fitted
  external pull-up;
- the complete optoNCDT ILD1420-50 command-and-stream path through an
  ISL3177E. Release firmware `f77e670` detected the sensor at 921.6 kBaud,
  received the documented `->` prompt, and received an accepted reply to
  `OUTPUT NONE`, `MEASRATE 8`, `OUTREDUCEDEVICE NONE`,
  `OUTADD_RS422 NONE`, and `OUTPUT RS422`. The first decoded measurement was
  24.813969 mm. A subsequent 8000-record `laser` capture at 8 kHz ranged from
  24.813969 mm to 24.816301 mm, with zero UDP packet loss. After
  `diag_reset`, the run had zero clock jitter, overruns, tick timeouts, record
  drops, and command backlog, with a 35 µs maximum loop time;
- counter-based core-0 laser/network contention on release firmware
  `7169e0d`. The continuously armed UART ring and discovered
  `laser_frames_received`, UART, parser, invalid-frame, unexpected-value, and
  synchronisation counters were exercised after `diag_reset`. A 30 s idle
  interval received 240015 laser frames during 240019 RT ticks
  (−16.7 ppm), with every laser fault counter at zero. A subsequent 120.25 s
  capture streamed 960000 records of all 13 CBC sources while the TCP
  connection issued 868 unthrottled tick requests. It received 962033 laser
  frames during 962053 RT ticks (−20.8 ppm), with zero UART errors, parser
  resynchronisations, invalid frames, unexpected values, synchronisation
  errors, source drops, UDP sequence gaps, index gaps, clock jitter, overruns,
  tick timeouts, or record drops. Maximum loop time was 35 µs. The independent
  sensor and RP2350 clocks therefore remained rate-matched within 21 ppm, and
  no individual steady-state laser frame loss was observed under maximum
  tested network load;
- the mandatory synchronous SRAM real-time loop: zero overruns, zero clock
  jitter and a constant 36 µs wake
  phase at 8 kHz under idle, TCP polling, 1000-record capture, 8000-record
  all-13-source capture and a sustained 60000-record capture, all with
  index-contiguous records and zero UDP loss. The previous async loop
  stretched tick phases ~10× under core-0 network load through the shared
  XIP cache and silently skipped up to 13 % of BUSY edges (see
  `docs/overrun_handoff.md`);
- the phase-resolved timing diagnostics (`wake_phase_*`, `t_*_max`,
  `diag_reset`) and the TIMER0 alarm-1 time watchdog. A lost embassy-time
  alarm was observed freezing all core-0 timers (drain, status log, TCP
  timeouts) for ~4 minutes; the watchdog bounds that class of stall to
  50 ms.

Independent re-verification on 2026-07-15 used the release synchronous image
from `4828b79` after formatting-only cleanup. Five-second idle and TCP-poll
phases sustained approximately 8000 ticks/s with zero overruns, timeouts,
record drops, ADC errors or clock jitter; wake phase stayed at exactly 36 us,
and loop maxima were 45 us and 47 us respectively. An 8000-record all-source
capture and a 60000-record `adc0,out` capture had contiguous indices, zero UDP
loss, zero device drops and loop maxima of 41 us and 44 us. After a further
300 s with no host connected, reconnection succeeded without a reset: ticks
advanced by 2400146, while overruns, timeouts, record drops and ADC errors all
remained zero.

Post-refactor verification on 2026-07-15 used release image `b35d4b8`, after
the layout gate found and the firmware replaced flash-resident compiler EABI
copy/clear helpers. Five-second idle and TCP-poll phases and an 8000-record
`adc0,out` capture sustained 8000 ticks/s with zero overruns, timeouts, clock
jitter, record drops, packet loss or index gaps; wake phase remained exactly
36 µs and loop maxima were 33–34 µs. A separate 8000-record all-13-source
capture and a sustained 60000-record `adc0,out` capture had the same zero-loss
result and 34 µs loop maxima. Finally, 100 complete coefficient replacements
interleaved with 100 scalar frequency changes produced a 38 µs maximum, zero
errors and `cmd_backlog_max = 1`. Outputs were returned to zero afterwards.

Waveform interpolation was verified on 2026-07-16 with release image
`ce44daf`, using DAC channel A looped back to ADC0 on the all-unipolar analogue
cape. The standard 8 kHz CBC regression sustained 8000.0–8000.3 ticks/s
through five-second idle and TCP-poll phases and an 8000-record `adc0,out`
capture. Every phase had zero overruns, tick timeouts, clock jitter, record
drops, UDP loss, and index gaps; wake phase remained exactly 36 µs, and
maximum loop time was 34 µs. A two-point `[0.5, 2.5] V` table at 20 Hz was
then captured for 4000 records in each interpolation mode with `table`, `out`,
and `adc0`. Linear order 1 produced 601 distinct rounded table values with
10 mV per-sample ramps; `table == out`, and the ADC0 fit had gain 0.99998,
0.115 mV offset, and 10.0 mV RMS residual. Zero-order hold order 0 produced
only the commanded two levels and 20 transitions over ten periods. Away from
the transitions, ADC0 medians were 0.49995 V and 2.50023 V, with 85.5 µV and
85.3 µV standard deviations respectively. Both waveform captures had zero
UDP loss, index gaps, overruns, timeouts, clock jitter, and record drops;
maximum loop time was 36 µs, with a fixed 36 µs wake phase. The table,
forcing, and target outputs were disabled afterwards; a final 512-record
capture reported `table == out == 0`, with ADC0 at 2.68 mV mean and 2.90 mV
maximum absolute value.

The generic `cmd_epoch` source and 14-source CBC stream layout were exercised
on 2026-07-16 with release image `100825a`. An 8000-record full-rate capture of
all 14 sources and a sustained 60000-record `adc0,out` capture both had
contiguous indices, zero UDP loss, zero device drops, zero overruns, and zero
tick timeouts. Wake phase remained exactly 36 µs, and maximum loop time was
35 µs. In a focused full-rate `forcing,out,cmd_epoch` stream, a constant
forcing-coefficient write advanced the epoch from 15 to 16 at sample 885064;
that same record was the first with `forcing == out == 0.25 V`. The transition
had no UDP loss or record drops. Outputs were returned to zero afterwards.
Two initial automated flash-and-connect attempts saw incomplete ARP, although
the probe log reported W5500 link-up after 2.104 s; the subsequent no-flash
control and streaming sessions worked normally.

The default firmware was returned to `PassThrough` after PID testing. The
current analogue cape is all-unipolar, and the CBC `DAC_POLARITY` array
intentionally matches it.

## Not yet verified on hardware

- Release image `100825a` consistently reported `clock_jitter = 1 µs` after a
  clean `diag_reset` in idle, TCP-poll, 14-source capture, sustained capture
  and a final two-second idle check. The fixed 36 µs wake phase, 35 µs maximum
  loop time and exact tick rate remained healthy, but the CBC acceptance limit
  is zero clock jitter. Reproduce and explain this result; do not relax the
  limit to accommodate it.
- Long phase-locked arbitrary table operation.
- `fw-whirl-rig` and `fw-pico2w-rig`. They build with the firmware workspace
  and their portable logic has host tests, but neither has been exercised as
  a complete physical experiment. Both use the mandatory synchronous SRAM
  core-1 architecture: whirl adapts it to the raw PWM-wrap latch and PIO FIFOs,
  while Pico 2W uses the raw latch and SPI1 DAC path. This is static ELF and
  cross-build evidence only, not real SSI, optical-input, Wi-Fi, DAC or timing
  evidence.
- W6100 Ethernet on every wired experiment. The W6100 variants cross-build,
  but no W6100-EVB-Pico2 has been exercised. Verify link, static addressing,
  DHCP, discovery, TCP control and sustained UDP streaming before treating it
  as equivalent to the W5500 path. The pinned Embassy driver configures 4 KiB
  MACRAW TX and RX buffers and disables MAC filtering for W6100, so also check
  packet loss and core-0 load under unrelated broadcast traffic.
- Full 24-source W5500 throughput and CYW43439 throughput, latency and RF
  behaviour.

The `fw-whirl-rig` constants match RMB20SC12BC96: 12-bit natural binary,
4096 positions per revolution, 1 MHz SSI below the 4 MHz limit, and more than
20.5 µs idle-high time between frames. Confirm the complete dual-converter
wiring, bit ordering and PIO period calibration on hardware. For the Pico 2W,
verify PIO1 radio bring-up, DHCP, discovery, a light capture and real-time tick
stability while Wi-Fi is active.

### Whirl rig

- The two 5 V RMB20 encoders share the GP22 clock through separate
  TTL-to-RS422 transmitters. Their 3.3 V-safe receiver outputs connect pitch
  to GP26 and yaw to GP27.
- PIO samples both SSI inputs simultaneously at 1 MHz. Counts 0 and 4095 are
  valid positions and cannot be used as disconnected signatures.
- GP28 receives an active-high 3.3 V optical pulse approximately 100 µs wide.
  PIO measures rising-edge periods with a nominal 1 µs count and a fixed
  program-overhead correction that still requires logic-analyser validation.
- The intended range is approximately 2000–6000 RPM. Periods below 5 ms are
  rejected as glitches; RPM becomes stale after 100 ms without a valid period.

## Bring-up constraints and known hardware faults

### Analogue cape

- Bond all cape grounds to Pico ground. A partial bond previously left
  driven-low signals near 0.8 V, BUSY stuck high and ADC data at all ones.
- AD7609 `V_DRIVE` must be 3.3 V, not 5 V. A 5 V wiring error destroyed one
  ADC and exposed non-5-V-tolerant RP2350 pins. Remove power immediately if
  the ADC heats.
- AD7609 uses SPI mode 2 at 12 MHz. Raise the clock only after checking signal
  integrity. AD5064 uses mode 1 at 16 MHz and needs roughly 3 µs between
  consecutive words.
- DAC channel B on this particular cape is faulty and remains near 5 V
  regardless of command. Use A, C or D; channel A is the current default.
- `DAC_POLARITY` is a property of the fitted output stages, not the AD5064
  itself. Check it before connecting a different analogue board.

### Laser UART

The optoNCDT receive line idles high. A disconnected floating GP1 produced a
UART error-interrupt storm severe enough to starve all core-0 tasks. The rig
now has an external 10 kΩ pull-up from GP1 to 3V3 and retains a 10 ms retry
backoff. Keep the pull-up fitted; a firmware pull configured through
`embassy-rp::gpio::Flex` is lost when the pin is converted to UART ownership.

CBC expects the factory 921.6 kBaud setting. At startup it uses GP0 through a
TTL-to-RS422 transmitter to stop any old stream, set `MEASRATE` to the
firmware sample rate, disable output reduction and additional values, then
select `OUTPUT RS422`. Command replies and the `->` prompt are discarded
before binary parsing starts. The full command exchange and a real 8 kHz
binary stream were verified with an ILD1420-50 and ISL3177E on 2026-07-16.
The receive side uses a continuously interrupt-drained 4096-byte ring,
approximately 170 ms at 8 kHz, so short core-0 stalls do not leave the UART
unarmed or overflow its hardware FIFO.

Debugger detachment can briefly halt the MCU while the independently powered
sensor continues transmitting. In the final stress session this left one UART
event and two parser resynchronisations before the test baseline. Writing
`diag_reset` after detaching cleared those event counters; none advanced
during either the idle or stressed measurement interval. The lifetime
`laser_frames_received` counter is deliberately not reset.

The initial bring-up produced no receive bytes at any supported baud despite
valid GP0 activity, while the real-time loop remained healthy. Correcting the
physical RS-422 wiring resolved the fault without a firmware change. If this
symptom recurs, verify ISL3177E VCC and common ground, package orientation,
pair polarity, and continuity through both differential paths before changing
the UART protocol.

### Ethernet and debug

- The first direct link through a USB Ethernet adaptor transmitted from the
  device but did not receive host frames. A known-good switch port and cable
  resolved it without firmware changes. Suspect the physical link before
  modifying the W5500 receive path.
- `ping` is not a valid liveness test because the current `embassy-net` setup
  does not answer ICMP echo. Use `helic-daq status`, discovery or ARP.
- The SWD connection is mechanically fragile. If probe-rs reports that the
  target does not respond, reseat SWCLK, ground and SWDIO before diagnosing
  the MCU. BOOTSEL mass-storage enumeration is an independent board check.

### Managed macOS host

On the managed bring-up Mac, the MDM-controlled Application Firewall silently
blocked inbound UDP to unsigned Homebrew Python while TCP control continued to
work. `/usr/bin/python3`, which is Apple-signed, could receive port 2351. The
workaround was to issue control commands from the normal environment, receive
length-prefixed UDP datagrams with `/usr/bin/python3`, then decode them offline
with `decode_stream_header` and NumPy. Treat a capture timeout with working
control as a host-firewall symptom before changing firmware.

### daffyduck Linux/Podman host

On `daffyduck`, the original rootless Podman container used for AI-assisted
bring-up did not expose the USB Ethernet interface directly. The host had
`enx001cc245a3b4` configured as `192.168.1.10/24` for the HELIC subnet, but
inside the container only a `pasta` interface on the managed network was
visible. With the default `fw-cbc-rig` static address restored
(`192.168.1.235/24`), firmware build and `probe-rs` flashing worked, TCP
control worked, and unicast discovery to `192.168.1.235` worked from the
container.

After recreating the container with host networking, the container did see
`enx001cc245a3b4` and the `192.168.1.0/24` route. TCP control used local
address `192.168.1.10`, ARP resolved `192.168.1.235` to
`02:48:4c:00:00:01`, and the firmware log showed finite streams arming and
completing. However, Python capture on UDP port 2351 still timed out, and
the Linux UDP receive counters did not increase during the stream attempt.
Host-side `tcpdump` on `enx001cc245a3b4` confirmed UDP packets arriving from
`192.168.1.235:2351` to `192.168.1.10:2351`, so the remaining block was the
host firewall ruleset. The host uses `iptables-nft` rather than `ufw`; its
`INPUT` chain accepted only selected new UDP traffic before a final
unconditional drop. Adding an allow rule for inbound UDP 2351 on
`enx001cc245a3b4` fixed capture:

```sh
sudo iptables -I INPUT 1 -i enx001cc245a3b4 -p udp --dport 2351 -j ACCEPT
```

After that rule, a 1000-sample `adc0,out` baseline capture and a 4000-sample
10 Hz, 1 V sine capture both completed with zero UDP packet loss. With the
unipolar analogue board, `out` reported ±1 V while `adc0` showed the clipped
positive half-cycle from approximately 0 to 1 V. If TCP control works but
capture times out on this machine, first verify UDP 2351 with `tcpdump`.
Current host libraries send a small UDP primer before `StreamStart`, so
stateful firewall rules that accept established return traffic may no longer
need a persistent explicit UDP 2351 allow rule.

The detailed sequence of failed async-loop mitigations, diagnostic variants,
and the final SRAM/latch resolution is historical evidence rather than current
bring-up guidance; it is retained in `docs/overrun_handoff.md`.


## Resource audit

Release ELF allocated-section totals after protocol v2 hardening were
approximately 130–144 KB flash and 130 KB RAM for wired experiments.
`fw-pico2w-rig`, including CYW43439 blobs, used approximately 404 KB flash and
124 KB RAM. These fit the 2 MB flash and 520 KB SRAM design envelope, but do
not establish timing, wired throughput or RF performance.

## Protocol v3 paged parameter discovery (2026-07-18)

Protocol v3 replaced the single-frame `GetParams` registry with indexed pages
that echo their inclusive start and exclusive next indices. The control-frame
payload remains bounded at 1024 bytes, while experiment, rig and controller
parameter capacities are now 16 each. Rust, Python and Julia tests force
registries beyond one page and verify global indices plus a read/write on the
second page. MATLAB received the equivalent codec, fake transport and tests,
but no MATLAB executable was available in this environment to run them.

The complete release workspace and both W6100 variants built, clippy passed,
and the real-time layout checker passed all three production ELFs. CBC W5500
hardware was tested first from the uncommitted pagination worktree reported as
firmware `0.1.0 62914d2`: an 8000-record all-14-source capture and a 60000-record
`adc0,out` capture were contiguous with zero UDP loss, device drops, overruns,
tick timeouts or clock jitter. Loop maximum was 35 µs and wake phase stayed
exactly 36 µs. After more than five minutes disconnected, the same image
reconnected at 369.2 s uptime with 2953498 ticks and zero record drops,
overruns, timeouts or jitter. Direct requests verified page `0..41`, page
`40..41` containing only `rig_out_channel`, the empty terminal page `41..41`,
`BadIndex` for start 42 and `BadLength` for an empty request.

The committed image `0.1.0 800e741` was then rebuilt and flashed. Its first
post-flash run reproduced the outstanding 1 µs `clock_jitter` observation
during TCP polling and capture, so that run correctly failed acceptance even
though its 8000 all-source records were contiguous and loss-free. After a
target reset and fresh diagnostic baseline, the repeat all-source run passed
with zero jitter, overruns, timeouts, record drops, packet loss and index gaps;
loop maximum was 35 µs and wake phase was 36/36 µs. A subsequent 60000-record
`adc0,out` run also passed every acceptance check at 8000.1 ticks/s with the
same 35 µs loop maximum and fixed wake phase. The final flashed image reported
`0.1.0 800e741`; non-zero page `40..41` and terminal page `41..41` were
rechecked on that exact build. Outputs were kept quiet throughout. The laser
was not connected and its retry task reported no sensor reply.

## Output safety gate (2026-07-18)

Added a generic per-tick output safety stage in `firmware/common`, opt-in via a
new `Rig::SAFETY_GATED` const (default false; `whirl-rig`/`pico2w-rig` compile it
out and are behaviourally unchanged). `rt_loop::safety_gate` runs after the
controller/forcing/table sum and before `actuate`: it latches a fault trip from
the rig's `output_fault`, holds `safe_output` while disarmed or tripped, and
otherwise passes the command through the rig's `clamp_output`. Streamed `out` is
now the applied (post-gate) value.

Host interface (no wire change): writable `arm` base param applied directly on
core 0 (like `diag_reset`; arms + clears a stale trip, or disarms), and a
read-only `safety` bitfield (armed/tripped/clamped/quieted). `SAFETY_ARMED`
starts 0 (disarmed after flash); TCP control disconnect disarms (comms-loss
quieting). Arm policy is a plain flag with no lease/heartbeat (operator present
with emergency power-off). `MAX_RIG_PARAMS` trimmed 8→6 to keep the base
registry within the single-frame discovery budget (it was at 1023/1024 bytes; no
experiment declares >2 rig params). Pure, host-tested helpers
(`clamp_channel_command`, `StaleCounter`) added to `helic-core::safety`.

`cbc-rig` is the first gated experiment: clamp to a 0.096–4.0 V DAC-output window
(≈ ±1.952 V differential), quiet on the laser leaving a 10–40 mm window or its
frame counter stalling, `safe_output = 0`. Flashed as commit `c8c3abe`; on-rig
checks with exciter+laser off confirmed disarmed-after-flash, blind-laser trip +
quiet (`safety = 0b1010`), arm/disarm, disconnect-disarm, and `loop_time`
unchanged at 33–34 µs (gate adds no measurable tick cost). The amplitude clamp
path was unit-tested at this boundary and was subsequently exercised live in
the 2026-07-22 commissioning below.

## CBC differential safety commissioning (2026-07-22)

Clean protocol-v3 firmware `0.1.0 cd779ce` was rebuilt and flashed to the W5500
CBC rig. DAC A was connected to ADC0 positive and DAC C to ADC0 negative with
the exciter isolated; the laser was live at approximately 24.82 mm. A fresh
diagnostic baseline had zero jitter, overruns, tick timeouts, command backlog,
record drops, and laser fault counters; wake phase was fixed at 36 us and the
loop maximum was 35 us.

The differential loopback directly established non-inverting near-unity
mapping: +/-50 mV constant captures fitted `adc0 = 1.000134 out - 0.269 mV`
with 0.084 mV RMS residual, and a 7 Hz, 0.1 Vpp sine tracked correctly. With a
+50 mV forcing command retained, explicit `arm = 0` and TCP disconnect each
forced streamed `out` to exactly zero and returned ADC0 to its approximately
-0.23 mV baseline; re-arming restored the command.

The amplitude-clamp path is now hardware-verified rather than unit-test-only.
Retained +/-2.0 V requests produced symmetric applied-output means of
+/-1.9519998 V; ADC0 measured +1.952160 V and -1.952464 V, and safety bit 2 was
set without a trip. Both 4000-record clamp captures had zero packet loss,
device drops, timing faults, or laser faults and a 38 us maximum loop time.

The final 8000-record quiet capture had `out == 0`, ADC0 mean -0.219 mV, arm 0,
`safety = 0b1000`, zero output coefficients, table mode off, and clean
diagnostics. The displacement/stale-laser trip was not deliberately re-induced
in this session; the 2026-07-18 blind-laser test and unit tests remain the
evidence for that path. ADC0 remains temporarily wired as the A-minus-C
loopback and must be restored before use as the experiment signal.

## Next hardware session

Prioritise tests that move a complete path from software-only to physical
evidence:

1. Pico 2W association, discovery, DAC output and decimated streaming while
   checking the 8 kHz synchronous tick diagnostics;
2. whirl-rig shared-clock SSI, simultaneous pitch/yaw capture and optical
   period calibration;
3. all-source W5500 streaming while watching `records_dropped`, UDP sequence
   gaps, `loop_time_max`, `overruns` and `tick_timeouts`;
4. W6100 link, static addressing, DHCP, discovery, control and all-source
   streaming, including core-0 load with broadcast traffic.
