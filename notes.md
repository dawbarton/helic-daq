# Hardware verification status

Last updated 2026-07-14. Read this before a hardware session and update the
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
- phase accumulator, Fourier generator and streaming at 8 kHz using a
  commanded 100 Hz sine;
- closed-loop PID on ADC0 with live gain tuning. A 2 V to 3 V step settled to
  2% in approximately 39 ms in the test loopback;
- a disconnected laser UART no longer starves core 0 when GP1 has the fitted
  external pull-up.

The default firmware was returned to `PassThrough` after PID testing. The
current analogue cape is all-unipolar, and the CBC and encoder `DAC_POLARITY`
arrays intentionally match it.

## Not yet verified on hardware

- An optoNCDT 1420 producing real binary measurements. Only disconnected-line
  behaviour has been checked.
- Arbitrary table playback, atomic re-commit and long phase-locked operation.
- `fw-sig-gen`, `fw-pwm-rig`, `fw-encoder-rig` and `fw-sig-gen-w`. They build
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
- On hardware, the protocol edge cases that reject `StreamSetup` while a
  stream is active with `Busy`, and non-finite parameter writes with
  `BadValue`. Both are covered by software tests.

For the RMB20, confirm the ordered part's bit count, Gray/binary encoding,
maximum clock and monoflop time before replacing the provisional 13-bit,
500 kHz constants. For the Pico 2W, verify PIO1 radio bring-up, DHCP,
discovery, a light capture and real-time tick stability while Wi-Fi is active.

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
2. arbitrary table playback and atomic recommit on a scope;
3. RMB20 format confirmation and SSI clock/data capture;
4. Pico 2W association, discovery and decimated streaming;
5. all-source W5500 streaming while watching `records_dropped`, UDP sequence
   gaps, `loop_time_max`, `overruns` and `tick_timeouts`.
6. W6100 link, static addressing, DHCP, discovery, control and all-source
   streaming, including core-0 load with broadcast traffic.
