# HELIC-DAQ user guide

HELIC-DAQ is a real-time control and data acquisition platform for laboratory
control, signal generation and instrumentation. The current `cbc-rig`
experiment targets control-based continuation (CBC), using a W5500-EVB-Pico2
(RP2350), an AD7609 8-channel 18-bit ADC, an AD5064 4-channel 16-bit DAC, and
an optional Micro-Epsilon optoNCDT 1420 laser displacement sensor.

## What it does

- Samples all 8 analogue inputs simultaneously at **1, 2, 4 or 8 kHz**
  (compile-time preset), hardware-timed so the sampling instant has
  essentially zero jitter.
- Runs a **real-time control loop** every sample: measurements → controller
  → analogue output, with total in-to-out latency under one sample period.
  The default build ships an open-loop pass-through and a PID controller;
  others can be added in firmware.
- Generates a **periodic reference/forcing signal** as a Fourier series
  (16 harmonics by default) with µHz-resolution frequency control and
  glitch-free, phase-continuous updates — the core ingredient of CBC.
- Lets a host computer **change parameters on the fly** (frequency, Fourier
  coefficients, controller gains) over Ethernet, safely: updates take
  effect atomically at a sample boundary.
- **Streams live data** (any of the ADC channels, laser distance, reference,
  forcing, output) to the host over UDP, with optional decimation and
  finite captures.

## Putting the firmware on the device

You need the Rust toolchain (`rustup`, stable channel — the repo pins the
rest) and one of:

**With a debug probe** (recommended — Raspberry Pi Debug Probe on the SWD
header, plus `cargo install probe-rs-tools`):

```sh
cd firmware
cargo run --release -p fw-cbc-rig # builds, flashes, and streams the device log
```

The log shows a boot banner, the network address, and a once-a-second
status line (loop timing, overruns, laser reading).

**Without a probe**, via the USB bootloader:

```sh
cd firmware
cargo build --release -p fw-cbc-rig
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/fw-cbc-rig -t elf helic-daq.uf2
# hold BOOTSEL while plugging in the USB cable, then:
picotool load helic-daq.uf2 && picotool reboot
```

## Connecting to it

The device uses a **static IP address**, `192.168.1.235/24` by default.
Connect it to your machine directly or via a switch, give your machine an
address on the same subnet (e.g. `192.168.1.10/24`), and check:

```sh
ping 192.168.1.235
```

To use a different address, edit `IP_ADDR` in `firmware/experiments/cbc-rig/src/config.rs` and
reflash. (Same for the sample rate, laser measuring range, and controller —
see the table below.)

Install the Python package from the repository root:

```sh
pip install -e host        # pip install -e "host[plot]" for plotting
```

## Using it

Command line (`--host <ip>` or `export CBC_DAQ_HOST=<ip>` if not the
default):

```sh
helic-daq status                     # firmware version, sample rate, uptime
helic-daq list                       # all parameters and current values
helic-daq sine 10 1.0                # output a 10 Hz, 1 V sine (smoke test)
helic-daq get laser loop_time_max
helic-daq set freq 17.5
helic-daq set ctrl_kp 0.8            # PID gain (when the PID build is flashed)
helic-daq stream --sources adc0,out --seconds 2 -o capture.npz
helic-daq stream --sources adc0,target,out --seconds 1 --plot
helic-daq stop                       # zero the forcing and target
```

Python:

```python
from helic_daq import Device

dev = Device("192.168.1.235")
print(dev.status())
print([p.name for p in dev.params])   # discovered parameter registry

dev.par.freq = 10.0                   # attribute-style parameter access

# Fourier coefficients: [mean, a1..a16, b1..b16] for
# mean + sum(a_k cos + b_k sin). This is a 1 V sine at the fundamental:
coeffs = [0.0] * 33
coeffs[17] = 1.0                      # b_1
dev.par.forcing_coeffs = coeffs

# Capture 2 s of data as numpy arrays:
data = dev.capture(["adc0", "out"], seconds=2.0)
print(data["adc0"].mean(), data["dropped"])
```

The `target_coeffs` series is the reference the controller tracks; the
`forcing_coeffs` series is added directly to the output. With the default
pass-through controller the output is simply `target + forcing`.

## Signal connections

| Signal | Where |
|---|---|
| Analogue in 0–7 | AD7609 inputs, ±10 V (or ±20 V, compile-time) |
| Analogue out 0–3 | Per-channel polarity, set in `board.rs` (`DAC_POLARITY`): unipolar 0–4.096 V or bipolar ±4.096 V |
| Laser | optoNCDT 1420 via RS422→TTL at 921.6 kBaud, 8 kHz output rate |

Output-channel polarity must match your analog board's output stages. The
target design is two bipolar + two unipolar; the current build is **all four
unipolar** (matching the interim bring-up board). The controller writes to
output channel 0 by default. The laser sensor must be preconfigured (via
Micro-Epsilon's tool) for binary output at 921.6 kBaud; the firmware only
listens.

## Things you set at compile time

Edit `firmware/experiments/cbc-rig/src/config.rs` and reflash:

| Setting | Constant | Default |
|---|---|---|
| Sample rate | `SAMPLE_RATE` | 8 kHz |
| Controller | `ActiveController` + `make_controller()` | pass-through |
| Harmonics | `HARMONICS` | 16 |
| Output channel | `OUTPUT_CHANNEL` | 0 |
| IP address | `IP_ADDR` / `IP_PREFIX` | 192.168.1.235/24 |
| Laser range | `LASER_RANGE_MM` | 50 mm |

## Health monitoring

`helic-daq list` shows the loop diagnostics at any time:

- `loop_time_last` / `loop_time_max` — tick processing time in µs; must
  stay well under the sample period (125 µs at 8 kHz).
- `overruns` — ticks that ran over the period. Should be 0.
- `busy_timeouts` — non-zero means the ADC isn't responding (not wired,
  not powered).
- `records_dropped` — stream data lost because the host wasn't keeping up.

If something looks wrong, the same numbers appear once a second in the
debug-probe log, along with connection events.

**If `stream` times out with no data** while `status`/`get`/`set` work, a
host firewall is almost certainly blocking inbound UDP on the stream port
(2351) — control is outbound TCP and unaffected. Allow your client through the
firewall, or receive on a host/binary the firewall permits. (On managed macOS
the built-in Application Firewall silently drops UDP to unsigned Python builds;
see `notes.md` for the Apple-signed-receiver workaround used during bring-up.)
