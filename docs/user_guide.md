# HELIC-DAQ user guide

HELIC-DAQ is a real-time control and data acquisition platform for laboratory
control, signal generation and instrumentation. `cbc-rig` targets
control-based continuation using an AD7609 ADC and AD5064 DAC. `sig-gen`
uses the same W5500-EVB-Pico2 and DAC as an arbitrary/function generator with
optional optoNCDT laser logging, but requires no ADC board. `pwm-rig` replaces
the DAC with a filtered 10-bit PWM output on GP10. `encoder-rig` extends the
CBC instrument with an SSI absolute encoder input.

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
cargo run --release -p fw-sig-gen # ADC-free signal generator
cargo run --release -p fw-pwm-rig # PWM output on GP10
cargo run --release -p fw-encoder-rig # CBC rig plus SSI encoder
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

Substitute another `fw-*` experiment package in the build and output filename
to flash it.

## Connecting to it

Find devices without knowing their addresses:

```sh
helic-daq find
```

The wired experiments use static addresses by default: `192.168.1.235/24` for
`cbc-rig`, `192.168.1.236/24` for `sig-gen`, and `192.168.1.237/24` for
`pwm-rig`. `encoder-rig` uses `192.168.1.238/24`.
Connect it to your machine directly or via a switch, give your machine an
address on the same subnet (e.g. `192.168.1.10/24`), and check:

```sh
ping 192.168.1.235
```

To use a different address, edit `NET_CONFIG` in the selected experiment's
`config.rs` and reflash. Select `NetConfig::Dhcp` to request an address from
the network instead. The sample rate, laser measuring range and controller
are selected there too. Discovery uses local UDP broadcasts; on Wi-Fi, disable
access-point client isolation if `find` sees nothing but direct connections
still work.

The encoder build reports position in revolutions as the discovered `encoder`
source. Set `rig_encoder_zero` to subtract a host-selected datum. Its 13-bit
Gray format and 500 kHz clock are provisional constants in
`encoder-rig/src/config.rs`; verify them against the ordered RMB20 variant
before connecting hardware. `encoder_errors` counts rejected all-low/all-high
frames and transport overruns.

Install the Python package from the repository root:

```sh
pip install -e host        # pip install -e "host[plot]" for plotting
```

To exercise the host tools without hardware, start the protocol-v2 simulator
in one terminal and connect to it from another:

```sh
python -m helic_daq.sim
helic-daq --host 127.0.0.1 capture --sources adc0,out --samples 1000
```

The simulator exposes the same discoverable parameter/source tables, supports
staged waveform uploads, and generates synthetic TCP-controlled UDP streams.

## Using it

Command line (`--host <ip>` or `export HELIC_DAQ_HOST=<ip>` if not the
default):

```sh
helic-daq status                     # firmware version, sample rate, uptime
helic-daq list                       # all parameters and current values
helic-daq sine 10 1.0                # output a 10 Hz, 1 V sine (smoke test)
helic-daq get laser loop_time_max
helic-daq set freq 17.5
helic-daq set ctrl_kp 0.8            # PID gain (when the PID build is flashed)
helic-daq sources
helic-daq capture --sources adc0,out --seconds 2 -o capture.npz
helic-daq capture --sources adc0,target,out --seconds 1 --plot
helic-daq upload wave.npy --duration 2.0
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

### Arbitrary waveform tables

Upload 2–4096 finite samples from Python or a NumPy `.npy` file:

```python
wave = [0.0, 1.0, 0.0, -1.0]
dev.upload_table(wave, duration=0.2, gain=1.5, mode="loop")
```

Free-running `loop` and `one-shot` modes use `table_freq`, set directly with
`freq=` or as `1 / duration`. `locked` and `locked-one-shot` derive their
phase from the master Fourier accumulator using an exact positive integer
`mult`; `phase` is an offset in turns. This gives zero relative drift. A
locked one-shot begins at the next master-period boundary. Sub-harmonic lock
is not offered because integer phase division would not wrap exactly; use a
free-running mode for a table slower than the master.

The uploaded table is staged in chunks and switched atomically at a sample
boundary. Its contribution is available as the discovered `table` stream
source and is added to controller output plus Fourier forcing.

In `pwm-rig`, GP10 carries a 0–3.3 V duty-cycle representation with a
roughly 146 kHz carrier and 10-bit resolution. An external RC or active
low-pass filter is required; HELIC-DAQ does not smooth the carrier in
software. Negative commands clamp to 0 V unless an external level-shifting
output stage is added. Increasing PWM resolution lowers carrier frequency in
direct proportion because `carrier × duty_steps = 150 MHz`.

The `target_coeffs` series is the reference the controller tracks; the
`forcing_coeffs` series is added directly to the output. With the default
pass-through controller the output is simply `target + forcing`.

## Signal connections

| Signal | Where |
|---|---|
| Analogue in 0–7 | AD7609 inputs, ±10 V (or ±20 V, compile-time) |
| Analogue out 0–3 | Per-channel polarity, set in `board.rs` (`DAC_POLARITY`): unipolar 0–4.096 V or bipolar ±4.096 V |
| Laser | optoNCDT 1420 via RS422→TTL at 921.6 kBaud, 8 kHz output rate |
| Encoder (`encoder-rig`) | RMB20 SSI clock GP22, data GP26, each via the appropriate TTL↔RS422 converter |

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
| Network | `NET_CONFIG` | static 192.168.1.235/24 |
| Laser range | `LASER_RANGE_MM` | 50 mm |

## Health monitoring

`helic-daq list` shows the loop diagnostics at any time:

- `loop_time_last` / `loop_time_max` — tick processing time in µs; must
  stay well under the sample period (125 µs at 8 kHz).
- `overruns` — ticks that ran over the period. Should be 0.
- `tick_timeouts` — non-zero means the selected tick source isn't responding;
  for `cbc-rig`, the ADC may not be wired or powered.
- `records_dropped` — stream data lost because the host wasn't keeping up.

If something looks wrong, the same numbers appear once a second in the
debug-probe log, along with connection events.

**If `capture` times out with no data** while `status`/`get`/`set` work, a
host firewall is almost certainly blocking inbound UDP on the stream port
(2351) — control is outbound TCP and unaffected. Allow your client through the
firewall, or receive on a host/binary the firewall permits. (On managed macOS
the built-in Application Firewall silently drops UDP to unsigned Python builds;
see `notes.md` for the Apple-signed-receiver workaround used during bring-up.)
