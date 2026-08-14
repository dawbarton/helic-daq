# Hardware verification status

Last updated 2026-08-12. Read this before a hardware session and update the
verification boundary, failures and fitted-hardware assumptions afterwards.

Entries before 2026-08-12 record the wired analogue rig under its former
package name `fw-cbc-rig`. That rig is now the magneto-elastic rig, maintained
at [helic-magneto-elastic-rig](https://github.com/dawbarton/helic-magneto-elastic-rig)
and advertising the experiment name `magnetoelastic`; the record of the work
done while it lived here stays below, unedited, and its evidence continues in
its own repository.

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
- `fw-pico2w-rig`. It builds with the firmware workspace and its portable logic
  has host tests, but it has not been exercised as a complete physical
  experiment. It uses the mandatory synchronous SRAM core-1 architecture with
  the raw PWM-wrap latch and SPI1 DAC path. This is static ELF and cross-build
  evidence only, not real Wi-Fi, DAC or timing evidence.
- W6100 Ethernet on every wired experiment. The W6100 variants cross-build,
  but no W6100-EVB-Pico2 has been exercised. Verify link, static addressing,
  DHCP, discovery, TCP control and sustained UDP streaming before treating it
  as equivalent to the W5500 path. The pinned Embassy driver configures 4 KiB
  MACRAW TX and RX buffers and disables MAC filtering for W6100, so also check
  packet loss and core-0 load under unrelated broadcast traffic.
- Full 24-source W5500 throughput and CYW43439 throughput, latency and RF
  behaviour.

For the Pico 2W, verify PIO1 radio bring-up, DHCP, discovery, a light capture
and real-time tick stability while Wi-Fi is active.

### Whirl rig

Moved on 2026-08-11. The whirl rig is maintained at
[helic-whirl-rig](https://github.com/dawbarton/helic-whirl-rig) and records its
hardware constraints, bring-up sequence and evidence in its own `notes.md`. Its
dual-SSI and optical-period paths were still unverified at the split; that
status moved with it. Entries dated before the split remain below as the
platform's own history.

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

## Network MCU reboot verification (2026-08-10)

At 2026-08-10T16:08:02+00:00, release CBC firmware reported identity
`0.1.0 830d290` on the W5500 rig at `192.168.1.235`. The ADC was attached,
but the laser and actuator were powered down. The first post-flash regression
attempt timed out in TCP connection after an incomplete ARP exchange, matching
the known post-flash link symptom. An ordered target reset and direct W5500
reflash restored the link; RTT then reported the 8 kHz core-1 loop, 14 sources,
and a 34 us loop maximum with zero overruns, timeouts or jitter.

The network reboot interface was exercised directly while a continuous
two-source UDP stream was active. Writes of `0`, `1`, and `0x52454253` to
`mcu_reboot` were rejected with `BadValue`; the exact confirmation token was
accepted. TCP and UDP stopped, the device reappeared after approximately
2.51 s with 2.182 s uptime, and the same identity, `arm = 0`, and
`mcu_reboot = 0`. A second consecutive reboot gave the same timings and clean
diagnostics. Thus reboot acceptance, connection loss, reset, network recovery,
and the disarmed post-boot state are hardware-verified. The per-channel DAC
quiescence sequence is covered by code and unit tests, but was not observed
electrically in this session because the actuator was unpowered.

The current release broker was then tested with two clients, live streaming,
and recording. After the reboot acknowledgement it disconnected both clients,
failed its first immediate reconnect while the MCU was down, and connected as
generation 2 approximately 3.10 s later with the same firmware identity. The
closed HDF5 session contained 197003 records and recorded
`close_reason = 8`, `clean_close = 1`, and `session_complete = 0`; no partial
file remained. A replacement client discovered the fresh registry and saw the
device disarmed.

The required CBC regressions passed after the reflash. The 8000-record
all-14-source capture was contiguous with zero UDP loss, device drops,
overruns, timeouts or clock jitter; the loop maximum was 34 us and wake phase
was fixed at 36/36 us. The 60000-record `adc0,out` capture likewise had zero
loss, drops, gaps or timing faults at 8000.371 ticks/s, with a 34 us maximum
and 36/36 us wake phase. After a 300 s client-free interval, firmware uptime
was 384.766 s and the tick count had advanced by 2512257 with zero record
drops, overruns, timeouts or jitter. A final 512-record quiet capture had
`out == 0`, zero packet loss and device drops, `arm = 0`, `mcu_reboot = 0`,
and clean timing counters; `safety = 0b1010` reflects the expected absent-laser
trip and quiet state.

## Next hardware session

Prioritise tests that move a complete path from software-only to physical
evidence:

1. Pico 2W association, discovery, DAC output and decimated streaming while
   checking the 8 kHz synchronous tick diagnostics;
3. all-source W5500 streaming while watching `records_dropped`, UDP sequence
   gaps, `loop_time_max`, `overruns` and `tick_timeouts`;
4. W6100 link, static addressing, DHCP, discovery, control and all-source
   streaming, including core-0 load with broadcast traffic.

## 2026-08-11T12:08+00:00 Rig-decoupling implementation stage 0

- Stage 0 of `docs/rig_decoupling_proposal.md` is complete: the new portable
  `helic-rt` crate owns injected `RtShared` diagnostic, safety, and reboot
  state, and each production firmware crate owns one const-initialised
  instance shared by both cores.
- Reboot ordering now follows the reviewed safety contract: core 0 disarms,
  requests core-1 quiescence, waits with the existing bound, and only then
  schedules the ROM reset. Host tests cover the lifecycle and ownership
  invariants, including disarm between a core-1 snapshot and trip latch.
- All three release firmware ELFs cross-built and passed
  `check_rt_layout.py`; the new core-1 methods are explicitly placed in SRAM
  through the `rt-sram` feature. This is static software evidence only. The
  attached CBC hardware has not yet been flashed, and timing remains at the
  previously verified baseline until the stage-2 measurement gate.

## 2026-08-11T12:22+00:00 Rig-decoupling implementation stage 1

- Existing portable runtime contracts, command/record queue types, source
  assembly, and parameter storage now live in the host-testable `helic-rt`
  crate. Golden host tests pin the 33-entry platform registry and current
  source-segment order.
- The reviewed migration list named `ParamGroup` and `Program` in stage 1 even
  though neither exists in the pre-migration implementation; introducing them
  remains part of stages 5 and 6. The legacy table module moved mechanically
  with `ParamStore` only to avoid a reverse dependency and is still scheduled
  for replacement by `TableBuffer` in stage 3.
- Firmware identity was not moved into the shared runtime: firmware support
  generates it and injects it into `ParamStore`, so a separately versioned rig
  will report its own build. All production release ELFs cross-built and passed
  the SRAM layout gate. This stage adds no hardware evidence; timing measurement
  remains the stage-2 gate.

## 2026-08-11T12:35+00:00 Rig-decoupling implementation stage 2

- The RP2350 firmware layer is now split by execution domain: `helic-fw-rt`
  owns mandatory core-1 mechanisms, `helic-fw-support` owns universal core-0
  services, and the non-universal optoNCDT UART task has its own integration
  crate. CI checks the portable crate dependency sets, rejects direct
  executor/time/network dependencies and source use in `helic-fw-rt`, and
  prevents `helic-fw-support` from reaching `helic-fw-rt` transitively.
- All production release ELFs passed the real-time layout gate. Explicit CBC
  symbol inspection placed `run_hot_loop`, `analog_spi::transfer_in_place`,
  the reboot quiescence hand-off, `__aeabi_memcpy4`, and `__aeabi_memclr4` in
  SRAM. Both W6100 wired variants also cross-built.
- W5500 CBC firmware identity `0.1.0 d0fadf9-dirty` reported protocol 3,
  42 parameters, 14 sources, and an 8 kHz sample rate. The 8000-record
  all-source regression had 34 us loop maxima in idle, TCP-poll, and capture
  phases, fixed 36/36 us wake phase, and zero overruns, timeouts, clock jitter,
  record or packet drops, and index gaps. The 60000-record `adc0,out` capture
  ran at 8000.158 ticks/s with a 34 us capture maximum; TCP polling reached
  35 us, with the same zero-fault counters and fixed wake phase. These results
  match the pre-split 32–34 us reference within timer granularity and show no
  observable tick-path cost from the crate split or injected `RtShared`.
- The regression runner's default 10 s flash timeout expired before probe-rs
  completed this host's roughly 7 s erase/program cycle plus start-up, so its
  first connection attempt saw an incomplete ARP entry. Allowing the flash to
  finish, detaching the probe, and then using `--no-flash` produced both clean
  runs. This was a runner sequencing limit, not a firmware or link failure.
  The ADC and Ethernet paths were live; the laser and actuator supplies were
  down, so this stage adds no new electrical evidence for either powered path.

## 2026-08-11T12:56+00:00 Rig-decoupling implementation stage 3

- `helic-core::TableBuffer` now owns the two waveform banks and exposes one
  non-`Sync` endpoint per core. Its commit token is linear and owner-checked;
  host tests cover pending/cancel behaviour, rejected commits, cross-buffer
  misuse, publication ordering, 100000 activation cycles, and four compile-fail
  ownership invariants. Each firmware crate constructs the 32 KiB buffer in a
  `ConstStaticCell`, rather than through a stack temporary. `table_len` is
  published from core 1 only after activation.
- Two release-only hazards were caught before acceptance. First, initial
  staging mutations were accidentally placed inside `debug_assert!`, so the
  release build activated an empty bank; unconditional mutation and a release
  `helic-rt` test now cover this. Second, the non-`Copy` command made LLVM emit
  generic `__aeabi_memcpy`; its SRAM veneer still jumped to the flash
  compiler-builtins implementation. `rt_mem` and `check_rt_layout.py` now cover
  the generic, 4-byte, and 8-byte variants of both copy and clear. CBC hot-loop
  disassembly confirms every realised memory-helper call lands directly in
  SRAM.
- Static layout also invalidated the reviewed `pending = 2` idle encoding: one
  non-zero byte inside `TableBuffer` placed both otherwise zeroed banks in
  `.data`, adding roughly 32 KiB of flash initialisation. Encoding idle as zero
  and pending as `bank + 1` preserves the state machine and puts the complete
  const-initialised buffer in `.bss` as intended.
- W5500 CBC firmware identity `0.1.0 940c1ea-dirty` retained protocol 3,
  42 parameters, 14 sources, and 8 kHz sampling. A disarmed four-sample table
  activation reported `table_len = 4`, generated -0.250 to +0.250 V in the
  `table` source, kept applied `out` exactly zero, and atomically replaced it
  with a three-sample table reporting length 3. Final-image activation reached
  46 us and the 1600-record playback capture reached 36 us, with no timing
  faults, loss, drops, or gaps.
- The final-image 8000-record all-source regression had 34, 35, and 35 us
  maxima for idle, TCP polling, and capture; the 60000-record `adc0,out`
  regression had 34, 35, and 35 us. Both held wake phase at 36/36 us and had
  zero overruns, timeouts, jitter, record/packet drops, and index gaps. The
  laser and actuator supplies
  remained down, so the table signal and safety-quiet behaviour are verified,
  but no powered actuator response was measured.

## 2026-08-11T13:33+00:00 Rig-decoupling implementation stage 4

- The copied-payload timing assumption failed decisively on the W5500 CBC rig.
  Diagnostic firmware `19e659c` materialised every value in two queued
  132-value commands at one sample boundary: five backlog-two runs measured
  95–96 us maximum loop time and 1 us clock jitter, although wake phase stayed
  at 36/36 us and overruns, timeouts, and drops remained zero. The narrower
  production-shaped copied coefficient pair on `59d76d7` still measured 73 us,
  beyond the unchanged 60 us CBC gate. The proposal's 2–4 us copy estimate was
  therefore wrong; do not retry copied force vectors or relax the gate.
- Target and forcing coefficient sets now use the same linear,
  owner-checked `DoubleBuffer<T>` activation protocol as waveform tables.
  Queue-full host tests return coefficient tokens to their owning staging
  endpoints. The exact two-activation diagnostic on firmware `a3bf233` measured
  55–56 us over ten runs, with `cmd_backlog_max = 2`, fixed 36/36 us wake phase,
  and zero jitter, overruns, timeouts, or drops. The production command queue is
  0x118c bytes (4.5 KiB); the target and forcing buffers are each 0x110 bytes,
  and all are zero-initialised in `.bss`.
- The final default W5500 image was also `0.1.0 a3bf233`, protocol 3, with 42
  parameters, 14 sources, and 8 kHz sampling. Its 8000-record all-source run
  measured 34, 35, and 36 us maxima for idle, TCP polling, and capture. Its
  60000-record `adc0,out` run measured 34, 35, and 35 us. Both held wake phase
  at 36/36 us and had zero jitter, overruns, timeouts, record/packet drops,
  capture drops, or index gaps.
- A focused disarmed capture applied buffered target mean 0.25 and forcing mean
  0.125 at sample boundaries: `target` remained 0.25, `forcing` transitioned
  from 0 to 0.125 during the capture, `cmd_epoch` advanced from 14 to 15, and
  applied `out` remained exactly zero. The capture had no loss, drops, or gaps,
  and a 35 us loop maximum. Target, forcing, table mode, and arm were returned
  to zero. The laser and actuator supplies remained down, so this is timing,
  ownership, and safety-quiet evidence, not powered actuator evidence.

## 2026-08-11T14:20+00:00 Rig-decoupling implementation stage 5

- `ParamStore` now composes six statically allocated `ParamGroup`s for the
  platform, generator, table, controller, rig, and experiment telemetry. Each
  group owns its definitions, shadows, validation, and transactional staging;
  the store resolves global indices, builds every real-time address from the
  captured target and local ID, accepts only after enqueue, returns rejected
  buffer tokens, broadcasts diagnostic reset, and validates the complete
  composition before serving requests. The 42-name CBC registry set and the
  14-source order are unchanged.
- Host tests cover scalar, coefficient, and table queue-full rejection,
  active-only `table_len`, diagnostic broadcast, direct table/controller/rig
  command IDs, core-0 payload rejection, and malformed group compositions.
  Both normal and release `helic-rt` tests pass. The hardware table test exposed
  a migration regression of the release-only hazard already found at stage 3:
  `write_block` and `set_len` had again been put inside `debug_assert!`, so an
  acknowledged release upload activated an untouched zero-length bank. A
  core-1 probe established that the activation command and owner-checked token
  were valid; making both mutations unconditional restored a three-sample
  activation and streamed range 0.3000001 to 0.4994999 V.
- An initial W5500 image at `1bf761b` reported 1 us `clock_jitter`, while an
  immediate A/B flash of accepted `a3bf233` on the same board reported zero.
  The timestamp was after wake-phase diagnostic atomics, so its integer TIMER0
  phase depended on unrelated linked layout. Commit `f43ca61` moved the spacing
  timestamp directly after the hardware wake and before diagnostic bookkeeping;
  wake-to-wake jitter returned to zero without changing the acceptance limit,
  and loop time now conservatively includes that bookkeeping.
- Final W5500 CBC firmware `0.1.0 e82f10a` reported protocol 3, 42 parameters,
  14 sources, and 8 kHz sampling. The 8000-record all-source regression measured
  34, 34, and 35 us maxima for idle, TCP polling, and capture; the 60000-record
  `adc0,out` regression measured 34 us in all three phases. Both held wake phase
  at 36/36 us and had zero jitter, overruns, timeouts, record or packet drops,
  capture drops, and index gaps.
- The final disarmed transaction test produced constant 0.25 V target and
  forcing sources from the two coefficient buffers, rejected invalid
  `rig_out_channel = 1` with `BadValue` while preserving its 0 V shadow, and
  published `table_len = 3`. Applied `out` remained exactly 0 V throughout.
  Cleanup left `arm = 0`, `table_mode = 0`, `freq = 0`, and zero target and
  forcing coefficients. The attached board is W5500; no W6100 image was
  flashed. The laser and actuator supplies remained down, so this is logic,
  timing, network, and safety-quiet evidence, not powered-path evidence.

## 2026-08-11T14:39+00:00 Rig-decoupling implementation stage 6

- `helic-rt::Program` now defines the statically dispatched computation between
  rig measurement and actuation. `StandardProgram` owns the controller, master
  phase accumulator, active target/forcing/table endpoints, table player,
  active-table-length publication, and programme signal cache. The common
  firmware loop retains timing, bounded command application, record assembly,
  and the rig/program boundary. Its output remains scalar until Stage 7.
- CBC now discovers 15 sources in this exact order: `adc0`–`adc7`, `laser`,
  `target`, `forcing`, `table`, `phase`, `out`, and `cmd_epoch`. On exact clean
  W5500 firmware `0.1.0 77fa0e4`, a disarmed 137 Hz sine capture matched
  `sin(2*pi*phase)` to 4.71e-6 maximum absolute error; the observed phase
  increment matched `137/8000` to 4.84e-8 turn. `cmd_epoch` advanced from 36
  to 38 for the frequency and coefficient commands, and applied `out` remained
  exactly zero. A three-sample table published `table_len = 3` and streamed
  from 0.2500003 to 0.7487499 V over the finite capture, again with zero
  applied output.
- The required 8000-record all-15-source W5500 regression passed with idle,
  TCP-poll, and capture loop maxima of 34, 35, and 35 us. The 60000-record
  `adc0,out` regression measured 34, 35, and 34 us. Both held wake phase at
  36/36 us and had zero jitter, overruns, tick timeouts, source or capture
  drops, UDP loss, and index gaps. Firmware reported protocol 3, 42 parameters,
  15 sources, and 8000 Hz.
- All three production release ELFs rebuilt and passed the SRAM layout gate.
  The CBC and whirl W6100 feature variants cross-built but were not flashed.
  Root Rust tests, 65 Python tests, and all 89 Julia checks passed; MATLAB was
  unavailable on this host. Final W5500 state was `arm = 0`, `table_mode = 0`,
  `freq = 0`, with zero target/forcing coefficients and clean timing counters.
  The laser and actuator supplies remained down, so this establishes logic,
  timing, network, and safety-quiet behaviour, not powered laser or actuator
  behaviour.

## 2026-08-11T14:53+00:00 Rig-decoupling implementation stage 7

- `Rig::ACTUATORS` now declares up to four named applied outputs, `actuate`
  accepts the corresponding slice, and the common loop uses bounded commanded
  and applied vectors. Setup rejects programme/rig count mismatches and excess
  outputs. Source assembly inserts each actuator after programme signals and
  before `cmd_epoch`; all production rigs retain their existing single `out`
  source, so CBC remains at 15 sources. The obsolete controller type on `Rig`
  was removed because the statically selected `Program` owns it.
- The pure `helic_rt::safety_decide` applies limits per actuator, quiets the
  complete vector on disarm, an existing trip, rig fault, `Program::fault`, or
  any non-finite command, and requests the monotonic trip latch on every tick
  while a fault persists. Host tests cover two-channel clamp and safe values,
  every fault source, re-latching after a stale snapshot, non-gated verbatim
  output, source order, count mismatch, and the four-actuator capacity. There
  are now 36 `helic-rt` tests.
- Exact clean W5500 CBC firmware `0.1.0 9714464` reported protocol 3, 42
  parameters, 15 sources, and 8000 Hz. With the laser supply down, a 3 V
  internal forcing mean produced applied `out = 0` while disarmed
  (`safety = 0b1010`). A logical arm cleared the old trip, the still-present
  laser fault re-latched it by the next observation (`safety = 0b1011`), and
  applied output remained exactly zero. Both 512-record captures had zero UDP
  loss, and timing counters remained clean.
- The 8000-record all-15-source W5500 regression measured idle, TCP-poll, and
  capture maxima of 35, 35, and 36 us. The 60000-record `adc0,out` regression
  measured 34, 35, and 35 us. Both held wake phase at 36/36 us and had zero
  jitter, overruns, tick timeouts, source/capture drops, UDP loss, and index
  gaps. This remains within CBC's unchanged 60 us acceptance limit.
- Root formatting, clippy, and tests passed; the complete release firmware
  workspace passed clippy, build, and the SRAM layout gate. Realised CBC
  hot-loop calls remain restricted to SRAM analogue transfers and EABI
  copy/clear helpers, apart from invariant-failure panic thunks. Both W6100
  wired variants cross-built but were not flashed; Python passed 65 tests and
  Julia passed 89 checks. MATLAB was unavailable. Final W5500 state was
  `arm = 0`, `table_mode = 0`, `freq = 0`, zero target/forcing coefficients,
  `safety = 0b1010`, and clean timing counters. The laser and actuator supplies
  remained down, so no powered-path evidence was obtained.

## 2026-08-11T15:00+00:00 Rig-decoupling implementation stage 8, in progress

- `WaveTable<const N: usize = 4096>` now owns capacity at the type level;
  `TableBuffer`, `ActiveTable`, and `TablePlayer` propagate it. Construction
  rejects capacities outside the table's two-to-`u16::MAX` representable
  range. Eight-entry host tests cover maximum upload length, block boundaries,
  active length, interpolation, and playback.
- `StandardProgram<C, N>` and `TableGroup<N>` carry the same capacity through
  core-1 playback and wire discovery. A host test proves an eight-entry group
  advertises `count = maximum = 8`. CBC, whirl, and Pico 2W now each select
  `TABLE_CAPACITY = 4096` in `config.rs`, preserving the current registry and
  memory layout while allowing a separately composed rig to select less.
- Root tests and clippy passed, including 58 `helic-core` and 37 `helic-rt`
  tests. The complete release firmware workspace passed clippy and build, and
  all three production ELFs passed the SRAM layout gate. This partial stage was
  not flashed; the attached CBC W5500 remains on the accepted Stage 7 image
  `0.1.0 9714464`, disarmed and quiet. Harmonic-count genericity remains the
  unfinished half of Stage 8.

## 2026-08-11T15:14+00:00 Rig-decoupling implementation stage 8, completed

- `StandardProgram<C, H, N>`, `GeneratorGroup<H>`, and the coefficient staging
  and active endpoints now propagate an experiment-selected harmonic count.
  The target and forcing double-buffer statics moved from shared firmware into
  each experiment beside its table banks, so a rig pays for its own capacity.
  The reviewed maximum remains 16, and all three production experiments select
  `HARMONICS = 16`; their wire registries and SRAM use are therefore unchanged.
  Host tests exercise a four-harmonic programme, three-harmonic discovery, and
  the earlier eight-entry table path.
- Exact clean CBC W5500 firmware `0.1.0 57d8de7` reported protocol 3, 42
  parameters, 15 sources, 8000 Hz, and 33-float target and forcing arrays. A
  focused 512-record, 137 Hz multiharmonic target matched the series evaluated
  from streamed `phase` to 1.39e-6 maximum absolute error. Constant forcing was
  exactly 0.125 V, applied `out` remained exactly zero while disarmed, capture
  loss and drops were zero, and safety remained `0b1010`.
- The 8000-record all-15-source regression measured idle, TCP-poll, and capture
  maxima of 35, 35, and 36 us. The 60000-record `adc0,out` regression measured
  35, 36, and 35 us. Both held wake phase at 36/36 us and had zero jitter,
  overruns, tick timeouts, source or capture drops, UDP loss, and index gaps.
  The first automatic post-flash connection timed out; reflashing the identical
  W5500 image under the debugger showed normal IPv4 configuration and link-up,
  after which both acceptance runs passed.
- Root formatting, clippy, and all 166 Rust tests plus four doctests passed.
  The committed production firmware workspace passed release clippy, build,
  and the SRAM layout gate; CBC and whirl W6100 variants cross-built but were
  not flashed. Python passed 65 tests, Julia passed 89 checks, and MATLAB was
  unavailable. Final W5500 state is `arm = 0`, `table_mode = 0`, `freq = 0`,
  zero target and forcing coefficients, `safety = 0b1010`, and clean timing
  counters. The laser and actuator supplies remained down, so no powered-path
  evidence was obtained.

## 2026-08-11T15:19+00:00 Rig-decoupling implementation stage 9

- The single-consumer `RpmEstimator` moved from `helic-core` into a new
  dependency-free, `no_std` `whirl-rig-program` crate. Its six host tests moved
  unchanged, preserving raw-period reporting, time-normalised EWMA, glitch
  rejection, stale invalidation, and the 2000–6000 rpm operating range. The
  firmware dependency enables `rt-sram`, and the CI dependency gate now pins
  the programme crate's empty normal-dependency set.
- All 166 root Rust tests plus four doctests pass; the six RPM tests moved from
  the 52-test `helic-core` suite into their own crate rather than disappearing.
  The committed source passed release firmware clippy and build, all three
  production ELFs passed the SRAM layout gate, and both W6100 wired variants
  cross-built. Release `nm` inspection found no standalone RPM symbol, so
  `observe` and `tick` remain inlined into the SRAM-resident whirl hot loop.
- No whirl hardware was attached, so Stage 9 has host, cross-build, and ELF
  evidence only. The CBC W5500 was not reflashed and remains on accepted Stage
  8 firmware `0.1.0 57d8de7`, disarmed and quiet. W6100 was not flashed.

## 2026-08-11T15:31+00:00 Rig-decoupling implementation stage 10

- `helic-core` now provides a borrowed `HarmonicFrame<H>` and
  `HarmonicGenerator<H>`, plus a single-drive-point `Pll<H>` which coherently
  demodulates measured force and response at the fundamental. Projection
  retains the existing Fourier accumulation order, and the frame's
  `period_start` comes directly from accumulator overflow rather than a
  reconstructed zero-phase test.
- The PLL implements `Fixed`, non-faulting `Acquiring`, `Locked`, and latched
  `LockLost` states. Tests cover lock and unlock tolerance/time hysteresis,
  acquisition timeout and explicit retry, invalid-sample stalling/loss,
  idempotent configuration replay, locked-only saturation loss, low-amplitude
  rejection, and bounded output under divergent and non-finite input. The
  missing setpoint ownership in the design sketch was made explicit in
  `PllConfig` and `set_setpoint_increment`; retaining the command as `u32`
  avoids the three-LSB error observed when a 50 Hz phase increment was briefly
  stored as `f32`.
- The future tick path uses squared amplitudes, integer-preserving fractional
  correction, and a bounded algebraic atan2 rather than `libm` square root,
  atan2, or rounding calls. A 40400-direction host sweep bounds atan2 error
  below 0.1 degree, and an end-to-end two-channel test locks a measured
  -30-degree response/force phase while preserving the exact setpoint.
- All 175 root Rust tests plus four doctests pass. The release firmware
  workspace passed dependency checking, formatting, clippy, build, and the
  three-ELF SRAM layout gate; both W6100 wired variants cross-built and were
  not flashed. No production programme instantiates the PLL, so there is no
  relevant hardware path yet. The attached CBC W5500 was not reflashed and
  remains on accepted Stage 8 firmware `0.1.0 57d8de7`, disarmed and quiet.

## 2026-08-11T15:40+00:00 Rig-decoupling implementation stage 11

- Named, non-inlined SRAM adapters now expose programme application, stepping,
  signal publication and fault evaluation, plus rig parameter writes,
  measurement and vector actuation, to static inspection. `Active::get`,
  `Active::activate`, and `safety_decide` are also realised named symbols. The
  strengthened layout gate requires every applicable boundary and rejects any
  emitted instance outside SRAM in all three production ELFs.
- Exact CBC W5500 firmware `0.1.0 a21d762` passed the 8000-record all-source
  regression with idle, TCP-poll and capture loop maxima of 36, 37 and 37 us,
  and the 60000-record `adc0,out` regression with the same maxima. Both held
  wake phase at 36/36 us and had zero jitter, overruns, tick timeouts, source or
  capture drops, UDP loss and index gaps. The unchanged 60 us CBC acceptance
  limit therefore retains 23 us margin.
- The automatic flash again timed out waiting for the first connection. A
  debugger flash of the identical W5500 image reported normal IPv4 and link
  state, after which both ordered no-flash runs passed. Final state was
  `arm = 0`, `freq = 0`, zero target and forcing coefficients,
  `table_mode = 0`, `safety = 0b1010`, and zero jitter, overrun, timeout, drop
  and backlog counters; the live loop repopulated `loop_time_max = 35 us`
  immediately after diagnostic reset.
- Exact committed firmware passed release formatting, clippy, all-production
  build and the strengthened SRAM layout gate. CBC and whirl W6100 variants
  cross-built but were not flashed. Only the attached W5500 was tested; the
  laser and actuator supplies remained down, so no powered-path evidence was
  obtained.

## 2026-08-11T15:49+00:00 Rig-decoupling implementation stage 12

- The hard-coded layout and regression registries moved into schema-versioned
  `rig-profile.toml` files owned by CBC, whirl and Pico 2W. Each profile carries
  identity, ELF and hot-symbol requirements, sample rate, transport and host
  settings, capture defaults, timing limit, and an ordered quieting sequence.
  The shared Python loader validates the contract; external rigs can supply
  `--profile`, `--elf-dir`, and `--firmware-dir` without editing HELIC-DAQ.
- The layout checker still rejects optional hot helpers outside SRAM whenever
  they are emitted, as well as requiring every mandatory pattern and exact EABI
  symbol. Seven new tests cover production loading, duplicate and malformed
  profiles, mandatory and optional SRAM failures, and profile-driven quieting;
  all 65 host-Python tests also pass. CI now runs the profile tests.
- A fresh all-production release build passed the default three-profile layout
  check and an explicit CBC-profile check. Exact device firmware remained
  `0.1.0 a21d762`: loading the CBC profile by explicit path passed an
  8000-record all-15-source W5500 regression at idle, TCP-poll and capture
  maxima of 36, 37 and 37 us, with fixed 36/36 us wake phase and zero jitter,
  overruns, timeouts, drops, loss or index gaps. Name-based `--rig cbc`
  discovery also passed a focused 256-record run at a 37 us maximum.
- Both hardware runs were no-flash and left the W5500 quiet through the
  manifest-defined sequence. W6100 was not flashed, and no whirl or Pico 2W
  hardware evidence was obtained. The laser and actuator supplies remained
  down.

## 2026-08-11T16:01+00:00 Rig-decoupling implementation stage 13

- `tests/external-rig` is an independent Cargo workspace, outside both
  HELIC-DAQ workspaces, with its own lockfile, RP2350 target and linker setup,
  programme, firmware, verification profile, and dependency policy. Its
  manifests consume exact `=0.1.0` HELIC packages; `[patch.crates-io]` points to
  this checkout only because the packages are not yet published.
- The fixture programme passed its host test. Its production-shaped firmware
  instantiates the shared real-time loop with local `Program`, `Rig`, and
  `TickSource` implementations, passed release clippy and build, and passed the
  shared layout checker against its local ELF/profile. The generic dependency
  checker accepted its exact direct dependencies, forbidden source imports, and
  exclusion of `helic-fw-support` from its graph.
- A fixture-owned deterministic device and clock drove the unmodified hardware
  regression runner from the local profile. Identity, ordered quieting, idle,
  polling, capture continuity, rate, phase spread, and loop limit all passed
  without network or hardware access. CI now repeats all four repository-boundary
  checks; this is composition evidence, not electrical evidence.
- Final exact-commit verification passed all 175 root Rust tests plus four
  compile-fail doctests, production and fixture release clippy/build/layout
  gates, both wired W6100 cross-builds, 65 host-Python tests, seven profile-tool
  tests, the external mocked regression, and 89 Julia checks. MATLAB was
  unavailable. No firmware was flashed during Stage 13; the attached CBC
  W5500 remains on accepted firmware `0.1.0 a21d762`, disarmed and quiet. The
  W6100 was cross-built only, and the laser and actuator supplies remained down.

## 2026-08-11T16:19+00:00 Rig-local code stays with its firmware package

- The Stage-9 interpretation of single-consumer ownership was too literal:
  creating a repository-root `whirl-rig-program` package kept RPM estimation
  out of shared `helic-core`, but scattered one rig's implementation across
  unrelated top-level locations.
- Rig-specific portable computation will instead live in a dependency-light,
  host-tested library target within that rig's firmware package. The firmware
  binary remains hardware glue, while the package boundary keeps the complete
  rig together.
- Promotion to `helic-core` remains cheap and is deferred until an algorithm
  has a second real consumer, or is deliberately accepted as a platform
  primitive. Generic-looking code alone is not evidence of platform reuse.

## 2026-08-11T18:20+00:00 Whirl rig split into its own repository

- The whirl rig left this repository for
  [helic-whirl-rig](https://github.com/dawbarton/helic-whirl-rig), with its
  history preserved through `git subtree split`. It consumes the platform
  crates as git dependencies pinned to `v0.1.1`; nothing is vendored.
- Platform changes made first, so the split rested on a working boundary rather
  than on hope: build identity is now owned by the application crate
  (`helic-fw-build` plus `firmware_identity!`) instead of being derived from
  this repository by `helic-fw-support`; `memory.x` and the linker build script
  are shared; and the verification gates ship in the host package as
  `helic-rt-layout`, `helic-rt-regression` and `helic-deps-check`.
- The identity defect was real and would have been silent: built out-of-tree,
  the old build script reported this repository's revision and the platform
  crate's version, while `rt_regression` still passed because it compares only
  the experiment name. The whirl firmware now reports
  `fw-whirl-rig 0.1.0 <its own revision>`.
- `tests/external-rig` gained `fw-fixture-service-rig`, which composes the
  core-0 services the existing fixture forbids. That fixture could not have
  caught the identity defect, because its dependency policy excludes the crate
  containing it. Keep both members.
- Nothing here is hardware evidence. No board was flashed, and the whirl rig's
  dual-SSI and optical-period paths remain unverified; that status moved with
  the rig.

## 2026-08-11T16:26+00:00 Whirl package-local RPM integration verified

- `fw-whirl-rig` now owns `RpmEstimator` in a dependency-free `no_std` library
  target beside its firmware binary. Hardware dependencies are ARM-target-
  gated, both board features enable `rt-sram`, and the dependency policy rejects
  any target-independent dependency added to the package.
- All six unchanged RPM tests pass on x86. The shared root suite passes 169
  tests plus four compile-fail doctests; the complete production firmware
  workspace passes formatting, release clippy, build and the three-profile SRAM
  gate. Both CBC and whirl W6100 variants cross-build, and the whirl ELF retains
  no standalone RPM symbol, consistent with inlining into the checked hot loop.
- The empty legacy `firmware/common` tree, empty CBC `src/drivers` directory and
  retired top-level `whirl-rig-program` directory were removed. Ignored local
  settings, Python caches/package metadata and the 6.2 MB `tmp/hawk-rig`
  reference-material tree were identified but deliberately left untouched.
- No hardware was flashed. W6100 evidence remains cross-build-only, and the
  attached CBC W5500 remains on the accepted, disarmed Stage-11 image.

## 2026-08-12T16:54+00:00 Wired analogue rig split into its own repository

- The wired analogue rig left this repository for
  [helic-magneto-elastic-rig](https://github.com/dawbarton/helic-magneto-elastic-rig),
  consuming the platform crates as git dependencies pinned to `v0.1.2`;
  nothing is vendored. History starts fresh there rather than being carried
  across, so the record of the work done while the rig lived here is the
  entries above and the platform's git history.
- The firmware identity changed with the move and nothing else did. The
  package is `fw-magnetoelastic-rig`, the advertised experiment name is
  `magnetoelastic` (14 bytes, inside the beacon's 16-byte field), and the
  profile is selected as `--rig magnetoelastic`. Pin assignments, safety
  limits, the 42-parameter registry and the 15-source table are unchanged.
- Hardware evidence for the rename, on the attached W5500 board: exact clean
  firmware `0.1.0 86a5262` reported protocol 3, 42 parameters, 15 sources at
  8 kHz, disarmed at boot. The profile regression passed all phases at
  7999.5–8000.3 ticks/s with zero overruns, tick timeouts, dropped records,
  lost packets, capture drops or index gaps, `loop_time_max` 38 µs against the
  unchanged 60 µs limit; the 8000-record all-source capture passed with the
  same counters. Recorded in full in the rig repository's `notes.md`.
- Removed here: the experiment crate, the W6100 wired cross-build CI step, and
  the wired rig's entries in the developer and user guides, which now point at
  the rig repository. The simulator, the shared beacon known-answer vector and
  the host fixtures in all four languages now use `magnetoelastic`; the
  regression runner's `--rig` default is `pico2w`, the only profile this
  repository still ships. The rig-profile tests that needed a wired contract
  now use `tests/external-rig/service-rig-profile.toml`, which this repository
  owns, rather than a production rig's file.
- Verified after the removal: root fmt, clippy and 169 tests; the firmware
  workspace fmt, release clippy, build and layout gate for `pico2w`;
  dependency rules in both workspaces; the external fixtures' build, tests and
  two-profile layout gate; 77 host Python tests plus the external fixture
  test; and the Julia suite. MATLAB is not installed here, so its two edited
  fixtures rest on CI.

## 2026-08-12T17:20+00:00 Control-link liveness, and a correction

- **Correction.** The 2026-08-12 entry above attributed a rig that had
  vanished from the network to an interrupted capture leaving it streaming.
  That is wrong. Killing a streaming client sends a FIN, the control server
  sees the disconnect immediately and stops the stream: measured at 12.0 s
  into a continuous session, `control: client disconnected` appeared in the
  same defmt millisecond as the kill.
- **What the control socket actually did.** It set a 30 s timeout and no
  keep-alive. smoltcp's timeout measures time since the last *received*
  packet, so it fired on silence rather than on death. Measured on hardware:
  an idle but perfectly healthy connection was reset after 30.0 s, exactly as
  predicted. That is the opposite of what the arm-and-hold workflow needs,
  since a reset disarms the output.
- **Fix.** The control socket now sets a 2 s keep-alive interval and a 10 s
  timeout. Probes oblige a live peer to answer, so an idle session survives;
  silence now means the peer is gone, and comms-loss quieting plus the stream
  stop follow within about 10 s instead of 30. The streamer additionally sends
  only while a control connection is open, so a session cannot outlive its
  connection even if the control task never resumes.
- **Evidence.** On the attached W5500 magneto-elastic rig, built against this
  tree: the same idle connection that was reset at 30.0 s before the change
  was still open after 90 s. The profile regression passed all phases at
  7999.4–8000.3 ticks/s with zero overruns, tick timeouts, dropped records,
  lost packets, capture drops or index gaps, `loop_time_max` 37–38 µs against
  the rig's unchanged 60 µs limit.
- **Two unreachable episodes, and they are not the same fault.** The board
  went absent from the network twice this afternoon, both times shortly after
  heavy streaming, with core 1 ticking normally throughout. The defmt captures
  distinguish them, and the distinction was only possible because David
  confirmed afterwards that he switched the optoNCDT off partway through the
  session.
  - **Second episode, explained.** The sensor was off. `helic-fw-optoncdt`
    then probed baud rates about four times a second without bound, and
    `records_dropped` climbed at roughly 6000/s with streaming *off*: core 0
    was not draining the record ring every 5 ms, so the network stack was not
    being serviced either. A switched-off sensor is a legitimate laboratory
    state, so a rig should degrade rather than leave the network; the probe
    loop wants a bounded backoff and a diagnostic counter instead of an
    unbounded retry. Note the probe loop alone is not sufficient: measured
    later with the sensor still off, five probes per second ran with the drop
    counter flat at zero and the full regression passing. The remaining
    candidate is the floating-RX interrupt storm the GP1 pull-up exists to
    prevent, which depends on the line state a powered-down sensor leaves
    behind rather than on probing.
  - **First episode, still unexplained.** Its capture has no laser lines at
    all, so the sensor was streaming normally, and `records_dropped` was frozen
    at 1752338, so core 0 had resumed draining. A healthy laser, a serviced
    ring, and the board still invisible from the host. Whatever this is, it is
    not the laser and not starvation at the time of observation. Next time it
    happens, attach the probe and check whether the WIZnet driver is still
    servicing its RX path before resetting; a reset destroys the state that
    would identify it.
  - The v0.1.3 tag message says the probe loop starves core 0. That is
    stronger than the evidence supports, and applies to at most one of the two
    episodes. A tag cannot be moved, so this entry is the correction.
  - The laser was off for the rest of the session, so `laser_frames_received`
    stayed at 0 and `safety` read 10: the gate had latched a trip and was
    quieting the actuator, which is the designed response to a blind feedback
    path.

## 2026-08-14T21:30+00:00 The copied command payload is deleted, and the regression now writes

Measured on the magneto-elastic rig at 8 kHz, laser, exciter, and stepper
powered down. Every parameter write cost the tick 19 to 20 us, and the cause
was neither the rig nor the handler: `set_rig_param` and `apply_program` are a
few tens of instructions each and entirely SRAM resident.

Isolation. Reads over TCP cost nothing, 45 us against a 44 us quiet tick, so
it is not core-0 network activity, which is all a GET and a SET share.
`t_measure_max` and `t_actuate_max` never moved, so all of it lands in the
rest of the tick. Rig-domain and program-domain writes cost the same. And a
33-float `target_coeffs` write cost exactly what a one-float write cost, which
is the tell: the cost does not depend on what the command carries.

It is the fixed-size envelope. Confirmed by scaling rather than by reading
code, using `diag-wide-command-payload` and two prototype widths:

| `RtCommand` | Write | Extra over quiet |
|---|---|---|
| 16 B, `Values` removed | 45 us | 2 us |
| 44 B, `MAX_RT_VALUES = 8` | 50 us | 6 us |
| 140 B, as shipped | 64 us | 20 us |
| 536 B, `diag-wide-command-payload` | 118 us | 74 us |

Linear through the origin at 0.139 us per byte, about 21 cycles per byte at
150 MHz. Alignment was tested and is not the mechanism: `#[repr(align(8))]` on
both `Payload` and `RtCommand` changed nothing. It is simply bytes moved.

`Payload::Values` had had no production consumer since `a3bf233`, twenty-one
minutes after `af90fea` narrowed it to 33 for the coefficients that `a3bf233`
then moved to buffers. So the buffered path never escaped the copy cost; it
only stopped making it worse, because an enum charges its largest variant to
every command whether or not that variant is in use. Deleted rather than
feature-gated: gating would have left the burst probe measuring a command
shape no shipped image has.

Also on the pre-stator magneto firmware `dd215ec`, quiet 43 us and any write
63 us, so this long predates that rig's stepper axis.

The regression could not have caught it. Idle and poll only read, and every
phase resets the diagnostics after its setup writes, so no phase observed a
tick applying a command, and `max_loop_us` was a quiescent limit. The
magneto-elastic rig declares 60 us and every write cost 64. A write phase now
runs between poll and capture, reusing the rig's declared quiet writes.

Worth keeping in view: at 8 kHz there was 81 us of headroom, so none of this
was urgent. At 16 kHz the period is 62.5 us against a 44 us quiet tick, and a
single 20 us command would have overrun outright.

## 2026-08-14T22:40+00:00 The control link dies about fifteen seconds after it opens

Found by the new regression write phase, which is the first thing in the tool
that kept a connection open long enough. **This is not the write phase, not the
command envelope, and not the magneto-elastic rig: it reproduces on v0.1.3.**
It is very likely the third and best-evidenced instance of the "board becomes
unreachable" episodes recorded above.

The board stops answering the control channel, and afterwards new connections
time out rather than being refused, so the listener is wedged too. Everything
else keeps running perfectly. With the probe attached rather than reset:

- Core 1 ticks at exactly 8000/s, `loop 43/45 us`, jitter 0, overruns 0, tick
  timeouts 0, indefinitely.
- Core 0's executor is alive: the 1 Hz status task logs on the millisecond and
  the laser probe loop keeps cycling its baud rates.
- `records_dropped` is frozen, so core 0 is still draining the record ring.

So it is the network path alone, not starvation and not a core-0 stall.

It is a clock, not a load. Time to failure over six runs on v0.2.0:

| Traffic | Spacing | Operations | Time to failure |
|---|---|---|---|
| writes | 5 ms | 3684 | 15.06 s |
| writes | 20 ms | 1491 | 15.40 s |
| writes | 20 ms | 1545 | 15.35 s |
| writes | 50 ms | 672 | 15.32 s |
| reads | 20 ms | 570 | 15.06 s |
| reads | 200 ms | 62 | 15.52 s |
| reads | 1 s | 13 | 16.02 s |

Operation count varies by sixty times and direction does not matter; the time
is 15.06 to 16.02 s. A connection left almost entirely silent, one read at
0 s and one at 10 s, also failed by 20 s. The clock therefore starts when the
connection opens and runs regardless of what crosses it.

That explains why nothing had caught it. Every `helic-daq` invocation is
connect, one request, disconnect. The measurement scripts used today ran about
eleven seconds and always finished just inside the window. The regression
before the write phase ran roughly thirteen seconds and passed by a margin of
two; adding five seconds of writes pushed it over, which is the whole reason
the phase found this rather than a loop-time regression. It also explains the
stale connection at the start of this session that presented as "connection
refused" from a second client.

First suspect is `firmware/support/src/comms/tcp.rs`, where `8110974` set
`CONTROL_KEEP_ALIVE` to 2 s and `CONTROL_TIMEOUT` to 10 s to make the link
prove liveness rather than assume it. Fifteen seconds is not either constant,
so the mechanism is not yet established and this entry deliberately does not
guess further. The evidence log is kept as the third episode's probe capture.

Next step is to bisect `8110974` against its parent with a fixed
sixteen-second connection, which is now a two-minute test rather than an
unexplained episode.

## 2026-08-14T23:30+00:00 Correction: it was blocking RTT, not the control link

The entry above is wrong and is retracted. There is no fifteen-second control
link defect, the keep-alive constants introduced in `8110974` are not
implicated, and neither is the embassy-time alarm watchdog. **`defmt-rtt`
blocks when its buffer fills, and the buffer fills whenever no debugger is
draining it.** The status task logs once a second, so it fills in roughly
fifteen seconds; the next `info!` blocks inside a critical section and core 0
stops. Core 1 is Embassy-free, SRAM-resident and never logs, so it keeps
ticking at exactly 8 kHz throughout.

What misled me, recorded because the same trap will catch the next person.
My test harness flashed with `cargo run` and then killed the tmux session to
free the probe before each measurement, so every failing run had no RTT reader
and every passing run had one. The fifteen seconds looked like a network clock
because it is genuinely constant, but it is the buffer fill time at a fixed
logging rate, which is exactly why it did not depend on traffic direction,
rate, or volume.

Worse, **attaching the probe drains the buffer and releases the block**, so
the board recovers the moment you go to look at it. That is why the earlier
episodes were unexplainable, why the note above them says a reset destroys the
evidence, and why my own probe capture showed a perfectly healthy core 0 with
the status task logging on the millisecond: the attach had already fixed it.
Observing the fault removes it.

Evidence, all on one firmware with no reset between:

| Condition | 20 ms reads |
|---|---|
| Probe attached | survives, test ended at 90 s |
| Probe killed | dies after 455 reads, 12.83 s |
| Probe re-attached, no reset, no reflash | answers immediately, `ticks` 12462784 |

The last row is the one that settles it. A reset would have destroyed it.

Fixed by enabling `defmt-rtt`'s `disable-blocking-mode`, so a full buffer drops
frames rather than halting the core. With it, ninety seconds of 20 ms reads
with no probe attached survive, and **the full default regression passes end to
end for the first time**: 630 writes in the write phase at 45 us, 8000 records,
no lost packets, no index gaps, no acceptance errors.

This also explains the regression's history rather than just its present. The
tool detaches from defmt deliberately before opening its host connection, so it
has always been racing this; it passed at about thirteen seconds by a margin of
two, and the five seconds of writes I added pushed it past. The write phase
found a real defect after all, just not the one I attributed it to.

The earlier "board became unreachable" episodes are very probably all of this,
including the stale connection that opened this session.
