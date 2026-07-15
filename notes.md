# Hardware verification status

Last updated 2026-07-15. Read this before a hardware session and update the
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
- the `rt-sync` synchronous SRAM real-time loop (now the `fw-cbc-rig`
  default): zero overruns, zero clock jitter and a constant 36 µs wake
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

Independent re-verification on 2026-07-15 used the release `rt-sync` image
from `4828b79` after formatting-only cleanup. Five-second idle and TCP-poll
phases sustained approximately 8000 ticks/s with zero overruns, timeouts,
record drops, ADC errors or clock jitter; wake phase stayed at exactly 36 us,
and loop maxima were 45 us and 47 us respectively. An 8000-record all-source
capture and a 60000-record `adc0,out` capture had contiguous indices, zero UDP
loss, zero device drops and loop maxima of 41 us and 44 us. After a further
300 s with no host connected, reconnection succeeded without a reset: ticks
advanced by 2400146, while overruns, timeouts, record drops and ADC errors all
remained zero.

The default firmware was returned to `PassThrough` after PID testing. The
current analogue cape is all-unipolar, and the CBC `DAC_POLARITY` array
intentionally matches it.

## Not yet verified on hardware

- An optoNCDT 1420 producing real binary measurements. Only disconnected-line
  behaviour has been checked.
- Long phase-locked arbitrary table operation.
- `fw-sig-gen`, `fw-pwm-rig`, `fw-whirl-rig` and `fw-sig-gen-w`. They build
  with the firmware workspace and their portable logic has host tests, but
  none has been exercised as a complete physical experiment.
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

The sensor must be configured externally for binary output at 921.6 kBaud and
8 kHz. Firmware currently receives only; GP0 is reserved for possible future
sensor commands.

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

After that host fix, the Codex environment could again receive UDP 2351. A
2026-07-15 retry completed a 1000-record `adc0,out` smoke capture, a
4000-record arbitrary-table loopback capture, a live table re-commit during a
6000-record stream, and an 8000-record capture of all 13 discovered `cbc-rig`
sources, all with zero UDP packet loss. During the run, `tick_timeouts`,
`adc_errors` and `records_dropped` did not increase, but `overruns` increased
by 1295 and `loop_time_last` remained around 307–353 µs at the reported 8 kHz
sample rate. Treat the functional capture evidence as valid, but investigate
the timing counters with the debug log and GP14 timing pin before claiming
real-time timing margin from this run.

Follow-up overrun investigation on 2026-07-15 found that the steady idle
core-1 loop is not intrinsically over budget: with only the debug probe
attached after a fresh flash, the status log reported normal tick bodies of
approximately 45–50 µs and a fixed startup-only overrun count. Sequential host
control polling and UDP capture reproduced the overrun growth, and temporary
stage instrumentation showed the slow ticks spread across ADC read, compute,
actuation and record work rather than a command-queue backlog. The supported
root cause is contention from core-0 W5500/TCP/UDP work against core-1
real-time execution through shared RP2350 resources, most likely XIP flash,
AHB/peripheral-bus and DMA/interconnect pressure while the hot RT path runs
from flash and performs blocking SPI1 ADC/DAC transfers. Treat `overruns` and
`clock_jitter` growth during host traffic as a real timing-isolation problem,
not a UDP-loss problem.

A remote A/B matrix then flashed diagnostic variants one at a time, always
using a single host client. Disabling the 1 Hz status log did not help.
Disabling the UDP streamer reduced idle noise but TCP polling still produced
roughly 100 overruns/s. Skipping ADC reads, DAC writes, or record enqueue
reduced the absolute loop cost but did not remove host-traffic-induced
overruns. Running at 4 kHz reduced, but did not eliminate, the symptom.
Reducing the W5500 SPI clock from 40 MHz to 10 MHz reduced capture-induced
overruns but did not materially change TCP polling. These results make an
individual ADC, DAC, record-ring, defmt or UDP-drain bug unlikely. The next
useful firmware intervention was to reduce shared flash/interconnect pressure
by moving the core-1 hot path into SRAM.

That SRAM diagnostic was run remotely on 2026-07-15. It moved the synchronous
RT tick body plus the hot board, ADC, DAC, generator, table and controller
helpers into `.data.ram_func`, then repeated the same idle, TCP-poll and UDP
capture measurements. It did not improve the host-traffic failure mode: idle
remained low at approximately 4.7 overruns/s, TCP polling rose to
approximately 137 overruns/s, and a 1000-record `adc0,out` capture produced
approximately 515 overruns/s with zero UDP packet loss, no new
`records_dropped`, no `adc_errors` and no `tick_timeouts`. This makes XIP
instruction fetch in the core-1 tick body unlikely to be the dominant cause.
The remaining likely mechanisms are W5500/SPI0 DMA or interrupt burst pressure
against core-1 timing, shared bus/interconnect effects during network traffic,
or delayed servicing of the BUSY-edge wait rather than pure RT-loop compute
cost.

With normal firmware restored, a decimation sweep of 1000-record `adc0,out`
captures showed a bandwidth dependence but not a full mitigation: decimation
1 produced approximately 458 overruns/s, decimation 2 approximately 346
overruns/s, decimation 4 approximately 275 overruns/s, decimation 8
approximately 243 overruns/s and decimation 16 approximately 228 overruns/s.
All captures delivered 1000 records with zero UDP packet loss and no new
`records_dropped`, `adc_errors` or `tick_timeouts`. Decimation is therefore a
useful operational reduction for heavier streams, but it does not solve the
underlying isolation issue.

## Resource audit

Release ELF allocated-section totals after protocol v2 hardening were
approximately 130–144 KB flash and 130 KB RAM for wired experiments.
`fw-sig-gen-w`, including CYW43439 blobs, used approximately 404 KB flash and
124 KB RAM. These fit the 2 MB flash and 520 KB SRAM design envelope, but do
not establish timing, wired throughput or RF performance.

## Next hardware session

Prioritise tests that move a complete path from software-only to physical
evidence:

1. optoNCDT binary receive with the fitted pull-up;
2. reduce core-0/core-1 shared-resource contention in the 8 kHz path, focusing
   next on W5500/SPI0 burst size, DMA and interrupt effects; the SRAM hot-path
   diagnostic did not materially improve TCP or streaming overruns;
3. whirl-rig shared-clock SSI, simultaneous pitch/yaw capture and optical
   period calibration;
4. Pico 2W association, discovery and decimated streaming;
5. all-source W5500 streaming while watching `records_dropped`, UDP sequence
   gaps, `loop_time_max`, `overruns` and `tick_timeouts`.
6. W6100 link, static addressing, DHCP, discovery, control and all-source
   streaming, including core-0 load with broadcast traffic.
