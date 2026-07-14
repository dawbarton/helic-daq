# HELIC-DAQ user guide

HELIC-DAQ is a real-time control and data acquisition platform for laboratory
control, signal generation and instrumentation. `cbc-rig` targets
control-based continuation using an AD7609 ADC and AD5064 DAC. Wired
experiments support the W5500-EVB-Pico2 and W6100-EVB-Pico2. `sig-gen` uses
the same board and DAC as an arbitrary/function generator with optional
optoNCDT laser logging, but requires no ADC board. `pwm-rig` replaces
the DAC with a filtered 10-bit PWM output on GP10. `whirl-rig` samples two
RMB20 SSI encoders and an optical revolution pulse. `sig-gen-w` runs the
signal generator on a Raspberry Pi Pico 2W over Wi-Fi.

## What it does

- In `cbc-rig`, samples all 8 analogue inputs simultaneously at **1, 2, 4 or
  8 kHz** (compile-time preset), with hardware-timed conversion starts.
- In `whirl-rig`, samples pitch and yaw simultaneously at **2 kHz** using one
  PIO state machine and estimates rotor speed from a hardware-timed optical
  pulse period.
- Runs a **real-time control loop** every sample: measurements → controller →
  actuation where output hardware is fitted. The default builds select
  open-loop pass-through; a PID controller is provided and others can be
  added in firmware. `whirl-rig` retains these calculations for future output
  hardware but currently has no actuation.
- Generates a **periodic reference/forcing signal** as a Fourier series
  (16 harmonics by default) with µHz-resolution frequency control and
  glitch-free, phase-continuous updates, which are central to CBC.
- Lets a host computer **change parameters on the fly** (frequency, Fourier
  coefficients, controller gains) over the network, safely: updates take
  effect atomically at a sample boundary.
- **Streams discovered live data** to the host over UDP, with optional
  decimation and finite captures. Available sources depend on the experiment
  and controller and carry names and units.

## Putting the firmware on the device

You need the Rust toolchain (`rustup`, stable channel; the repository pins the
rest) and one of:

**With a debug probe** (a Raspberry Pi Debug Probe on the SWD
header, plus `cargo install probe-rs-tools`):

```sh
cd firmware
cargo run --release -p fw-cbc-rig # builds, flashes, and streams the device log
cargo run --release -p fw-sig-gen # ADC-free signal generator
cargo run --release -p fw-pwm-rig # PWM output on GP10
cargo run --release -p fw-whirl-rig # Dual SSI encoders and revolution pulse
cargo run --release -p fw-sig-gen-w # Pico 2W Wi-Fi signal generator
```

Wired packages target the W5500-EVB-Pico2 by default. Select the pin-compatible
W6100-EVB-Pico2 explicitly:

```sh
cargo run --release -p fw-cbc-rig --no-default-features --features board-w6100
```

The log shows a boot banner, network bring-up, and a once-a-second status
line with loop timing and overruns.

**Without a probe**, via the USB bootloader:

```sh
cd firmware
cargo build --release -p fw-cbc-rig
picotool uf2 convert target/thumbv8m.main-none-eabihf/release/fw-cbc-rig -t elf helic-daq.uf2
# hold BOOTSEL while plugging in the USB cable, then:
picotool load helic-daq.uf2 && picotool reboot
```

Add `--no-default-features --features board-w6100` to the build command for a
W6100 image. The resulting executable has the same filename, so convert or
copy it before building the other board variant.

Substitute another `fw-*` experiment package in the build and output filename
to flash it.

## Connecting to it

Find devices without knowing their addresses:

```sh
helic-daq find
```

The wired experiments use static addresses by default: `192.168.1.235/24` for
`cbc-rig`, `192.168.1.236/24` for `sig-gen`, and `192.168.1.237/24` for
`pwm-rig`. `whirl-rig` uses `192.168.1.238/24`.
Connect it to your machine directly or via a switch and give your machine an
address on the same subnet, for example `192.168.1.10/24`. After installing
the host package below, check the TCP control service:

```sh
helic-daq --host 192.168.1.235 status
```

To use a different address, edit `NET_CONFIG` in the selected experiment's
`config.rs` and reflash. Select `NetConfig::Dhcp` to request an address from
the network instead. The sample rate, laser measuring range and controller
are selected there too. Discovery uses local UDP broadcasts; on Wi-Fi, disable
access-point client isolation if `find` sees nothing but direct connections
still work.

For `sig-gen-w`, set `WIFI_SSID` and `WIFI_PASSWORD` in its `config.rs`
before flashing. Credentials are compiled into the image and the device joins
as a station, retrying until the access point is available. DHCP is the
default and `helic-daq find` reports the assigned address. The Pico 2W LED is
driven through the CYW43439, not GP25. Use wired Ethernet for sustained
full-rate multi-source streaming; Wi-Fi is intended for control, signal
generation and lighter captures.

The whirl build reports wrapped pitch and yaw in revolutions. Both
RMB20SC12BC96 encoders use 12-bit natural-binary SSI at 1 MHz and share one
clock, so PIO samples both data inputs on the same instruction. The optical
input exposes `rev_period`, EWMA `rpm`, `rev_pulse` and `rpm_valid`. The
estimate uses a 250 ms time constant and becomes invalid after 100 ms without
an accepted pulse. `ssi_errors`, `pulse_count`, `pulse_glitches` and
`pulse_errors` provide transport diagnostics.

Install the Python package from the repository root:

```sh
pip install -e host        # pip install -e "host[plot]" for plotting
```

To exercise the host tools without hardware, start the protocol-v2 simulator
in one terminal and connect to it from another:

```sh
python3 -m helic_daq.sim
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
| SSI clock (`whirl-rig`) | GP22, fanned out to both TTL→RS422 clock transmitters |
| Pitch/yaw (`whirl-rig`) | RS422→TTL data on GP26/GP27 respectively |
| Revolution pulse (`whirl-rig`) | Active-high 3.3 V input on GP28 |

Output-channel polarity must match your analogue board's output stages. The
target design is two bipolar + two unipolar; the current build is **all four
unipolar** (matching the interim bring-up board). The controller writes to
output channel 0 by default. The laser sensor must be preconfigured (via
Micro-Epsilon's tool) for binary output at 921.6 kBaud; the firmware only
listens.

## Things you set at compile time

Edit the selected experiment's `src/config.rs` and reflash. The table shows
the `cbc-rig` defaults:

| Setting | Constant | Default |
|---|---|---|
| Sample rate | `SAMPLE_RATE` | 8 kHz |
| Controller | `ActiveController` + `make_controller()` | pass-through |
| Output channel | `OUTPUT_CHANNEL` | 0 |
| Network | `NET_CONFIG` | static 192.168.1.235/24 |
| Laser range | `LASER_RANGE_MM` | 50 mm |

`HARMONICS` is a platform-wide constant in `firmware/common/src/lib.rs`, not
an experiment setting. Changing it also changes the Fourier coefficient array
size exposed by the protocol and must be checked against payload and real-time
budgets.

## Health monitoring

`helic-daq list` shows the loop diagnostics at any time:

- `loop_time_last` / `loop_time_max`: tick processing time in µs; must
  stay well under the sample period (125 µs at 8 kHz).
- `overruns`: ticks that ran over the period. Should be 0.
- `tick_timeouts`: non-zero means the selected tick source isn't responding;
  for `cbc-rig`, the ADC may not be wired or powered.
- `records_dropped`: stream data lost because the host wasn't keeping up.

If something looks wrong, the same numbers appear once a second in the
debug-probe log, along with connection events.

**If `capture` times out with no data** while `status`/`get`/`set` work, check
whether a host firewall is blocking inbound UDP on stream port 2351. Control
uses outbound TCP and can remain functional. On the managed macOS machine used
for bring-up, the Application Firewall silently dropped UDP to an unsigned
Homebrew Python executable; [../notes.md](../notes.md) records the workaround.
