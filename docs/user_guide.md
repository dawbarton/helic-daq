# HELIC-DAQ user guide

HELIC-DAQ is a real-time control and data acquisition platform for laboratory
control, signal generation and instrumentation. `cbc-rig` targets
control-based continuation using an AD7609 ADC and AD5064 DAC. Wired
experiments support the W5500-EVB-Pico2 and W6100-EVB-Pico2. `whirl-rig`
samples two RMB20 SSI encoders and an optical revolution pulse. `pico2w-rig`
runs an AD5064 signal generator with optional optoNCDT laser logging on a
Raspberry Pi Pico 2W over Wi-Fi.

## What it does

- In `cbc-rig`, samples all 8 analogue inputs simultaneously at **1, 2, 4 or
  8 kHz** (compile-time preset), with hardware-timed conversion starts.
- In `whirl-rig`, samples pitch and yaw simultaneously at **2 kHz** using one
  PIO state machine and estimates rotor speed from a hardware-timed optical
  pulse period.
- In `pico2w-rig`, updates an AD5064 output at a hardware-timed **8 kHz** while
  Wi-Fi control, streaming and optional laser logging run on the other core.
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

Install [`rustup`](https://rustup.rs/), then open a new terminal and initialise
the repository's required stable toolchain, components and RP2350 target:

```sh
cd helic-daq
rustup show
```

The settings in `rust-toolchain.toml` make `rustup` install and select
everything required. You can then put the firmware on the device using one of:

**With a debug probe** (a Raspberry Pi Debug Probe on the SWD
header, plus `cargo install probe-rs-tools`):

```sh
cd firmware
cargo run --release -p fw-cbc-rig # builds, flashes, and streams the device log
cargo run --release -p fw-whirl-rig # Dual SSI encoders and revolution pulse
HELIC_WIFI_SSID=lab HELIC_WIFI_PASSWORD=secret \
  cargo run --release -p fw-pico2w-rig # Pico 2W Wi-Fi signal generator
```

Wired packages target the W5500-EVB-Pico2 by default. Select the pin-compatible
W6100-EVB-Pico2 explicitly:

```sh
cargo run --release -p fw-cbc-rig --no-default-features --features board-w6100
```

The synchronous SRAM real-time path is mandatory and is retained when default
network features are disabled:

```sh
cargo run --release -p fw-whirl-rig --no-default-features --features board-w6100
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

Add `--no-default-features --features board-w6100` to the CBC or whirl
build command for a W6100 image. The resulting executable has the same
filename, so convert or copy it before building the other board variant.

Substitute another `fw-*` experiment package in the build and output filename
to flash it.

## Connecting to it

Find devices without knowing their addresses:

```sh
helic-daq find
```

The wired experiments use static addresses by default: `192.168.1.235/24` for
`cbc-rig` and `192.168.1.238/24` for `whirl-rig`.
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

For `pico2w-rig`, supply `HELIC_WIFI_SSID` and `HELIC_WIFI_PASSWORD` in the
build environment as shown above. They are compiled into the image without a
tracked source edit; a firmware built without them stops during network setup
with a clear panic instead of attempting placeholder credentials. The device
joins as a station and retries until the access point is available. DHCP is the
default and `helic-daq find` reports the assigned address. The Pico 2W LED is
driven through the CYW43439, not GP25. Use wired Ethernet for sustained
full-rate multi-source streaming; Wi-Fi is intended for control, signal
generation and lighter captures.

The whirl build reports wrapped pitch and yaw in revolutions. Both
RMB20SC12BC96 encoders use 12-bit natural-binary SSI at 1 MHz and share one
clock, so PIO samples both data inputs on the same instruction. The optical
input exposes `rev_period`, EWMA `rpm`, `rev_pulse` and `rpm_valid`. The
estimate uses a 250 ms time constant and becomes invalid after 100 ms without
an accepted pulse. Its mandatory real-time path polls the hardware PWM-wrap
latch and accesses both PIO FIFOs from SRAM without an executor on core 1.
`ssi_errors`, `pulse_count`, `pulse_glitches` and `pulse_errors` provide
transport diagnostics.

Install the Python package from the repository root:

```sh
pip install -e host-python # add `[plot]` for plotting
```

For Julia, develop the package into the current environment, or use its project
directly:

```julia
using Pkg
Pkg.develop(path="host-julia")
```

For MATLAB, add the package directory to the path:

```matlab
addpath("host-matlab")
```

To exercise the host tools without hardware, start the protocol-v3 simulator
in one terminal and connect to it from another:

```sh
python3 -m helic_daq.sim
helic-daq --host 127.0.0.1 capture --sources adc0,out --samples 1000
```

The simulator exposes the same discoverable parameter/source tables, supports
staged waveform uploads, and generates synthetic TCP-controlled UDP streams.

### Shared broker and optional recorder

For long-running monitoring, stream sharing, and optional recording, run the
loopback-only Rust broker between the host libraries and the MCU:

```sh
# Monitoring, sharing, and recent history without disk recording:
cargo run --release -p helic-broker -- \
  --mcu-host 192.168.1.235

# Add HDF5 recording:
cargo run --release -p helic-broker -- \
  --mcu-host 192.168.1.235 --output-dir captures
```

The broker prints `Captures are not being saved.` in the first mode and
`Captures are being saved to captures.` in the second. Omitting
`--output-dir` creates no capture directory or files and does not disable
stream sharing, quiet clients, history, or recent replay.

Clients then connect to `127.0.0.1`. The first client configures and starts a
stream; later clients attach to that same stream. Any client may stop it. When
recording is enabled, it is independent of attachment and per-client
quietness. The default recent-history window is 10 seconds and recorded files
roll at a soft 1 GiB:

```sh
cargo run --release -p helic-broker -- \
  --mcu-host 192.168.1.235 --output-dir captures \
  --history 30s --segment-size 1GiB
```

The control, stream, and discovery listeners bind only to `127.0.0.1`. See
[`broker.md`](broker.md) for shared-state semantics, host examples, file
layout, recovery behaviour, and the test matrix.

Use an ephemeral UDP port (`port=0`) for each concurrent client so local
receivers cannot collide. For example, after a monitoring client has started
a continuous stream, another Python client can retrieve the preceding second
without interrupting it:

```python
from helic_daq import Device

snapshot = Device("127.0.0.1")
print(snapshot.broker_info())
data = snapshot.capture_recent(seconds=1.0, port=0)
# This connection remains attached and quiet; the global stream keeps running.
```

The ordinary `capture` helper configures and later stops a stream, so it is
intended for direct-MCU use or for deliberately owning the broker's global
capture. Use `capture_recent` (Python and Julia) or `captureRecent` (MATLAB)
when sampling an already-running broker stream. Quiet-start and
quietness-change methods are also available for clients that keep their own
`StreamReceiver` open.

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
helic-daq diag-reset                 # clear timing and event diagnostics
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

Julia:

```julia
using HelicDAQ, Tables

open(Device, "192.168.1.235") do dev
    @show status(dev)
    dev[:freq] = 10f0

    coeffs = zeros(Float32, 33)
    coeffs[18] = 1f0                    # b₁ with one-based indexing
    dev[:forcing_coeffs] = coeffs

    data = capture(dev, [:adc0, :out]; seconds=2)
    columns = Tables.columntable(data)
    @show columns.adc0[1:5] data.dropped data.lost_packets
end
```

`Capture` implements Tables.jl with `index` followed by the requested sources.
The cumulative device-side drop count and UDP packet loss remain metadata on
the capture rather than being repeated in each row.

MATLAB:

```matlab
device = helicdaq.Device("192.168.1.235");
cleanup = onCleanup(@() delete(device));

device.setParameter("freq", 10);
coefficients = zeros(1, 33, "single");
coefficients(18) = 1;                  % b1 with one-based indexing
device.setParameter("forcing_coeffs", coefficients);

data = device.capture(["adc0", "out"], 'Seconds', 2);
mean(data.adc0)
data.Properties.UserData
```

MATLAB captures are tables with `index` followed by the requested source
variables. Units are in `Properties.VariableUnits`; cumulative device-side
drops and UDP packet loss are in `Properties.UserData`.

Every experiment also exposes the `cmd_epoch` stream source. It starts at zero
and increments once for each queued parameter command applied by the real-time
core. The first full-rate record with a changed epoch is therefore the first
record affected by the command; a jump of two means two commands took effect
at that boundary. The counter wraps modulo 2²⁴, and hosts should compare it
modulo 2²⁴. With stream decimation or record loss, a change still proves that
commands were applied, but the omitted effective sample cannot be recovered.
Direct diagnostic resets and staged table blocks do not advance it; committing
a staged table does.

### Arbitrary waveform tables

Upload 2–4096 finite samples from Python or a NumPy `.npy` file:

```python
wave = [0.0, 1.0, 0.0, -1.0]
dev.upload_table(
    wave,
    duration=0.2,
    gain=1.5,
    mode="loop",
    interpolation="hold",
)
```

In Julia, the corresponding call is:

```julia
upload_table!(
    dev,
    Float32[0, 1, 0, -1];
    duration=0.2,
    gain=1.5,
    mode=:loop,
    interpolation=:hold,
)
```

In MATLAB:

```matlab
device.uploadTable(single([0, 1, 0, -1]), ...
    'Duration', 0.2, 'Gain', 1.5, 'Mode', "loop", ...
    'Interpolation', "hold");
```

Interpolation is selected independently of the playback mode. `linear`, the
default, joins adjacent table values and interpolates the final value back to
the first. `hold` uses zero-order hold: each table value remains constant for
its complete phase interval, including the final value up to the period wrap.
The underlying `table_interp` parameter is the mathematical interpolation
order, with `0` for zero-order hold and `1` for linear interpolation.

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

The `target_coeffs` series is the reference the controller tracks; the
`forcing_coeffs` series is added directly to the output. With the default
pass-through controller the output is simply `target + forcing`.

## Signal connections

| Signal | Where |
|---|---|
| Analogue in 0–7 | AD7609 inputs, ±10 V (or ±20 V, compile-time) |
| Analogue out 0–3 | Per-channel polarity, set in `board.rs` (`DAC_POLARITY`): unipolar 0–4.096 V or bipolar ±4.096 V |
| Laser | optoNCDT 1420 via bidirectional RS422↔TTL at 921.6 kBaud; CBC configures its rate to match the sample clock |
| SSI clock (`whirl-rig`) | GP22, fanned out to both TTL→RS422 clock transmitters |
| Pitch/yaw (`whirl-rig`) | RS422→TTL data on GP26/GP27 respectively |
| Revolution pulse (`whirl-rig`) | Active-high 3.3 V input on GP28 |

Output-channel polarity must match your analogue board's output stages. The
target design is two bipolar + two unipolar; the current build is **all four
unipolar** (matching the interim bring-up board). The controller writes to
output channel 0 by default. The laser sensor must be preconfigured (via
Micro-Epsilon's tool) only if its baud rate has changed from the factory
921.6 kBaud. CBC configures its measuring rate, disables output reduction and
additional values, then selects binary RS422 output. This requires both UART
directions through suitable TTL↔RS422 hardware; GP1 still needs the external
idle-high pull-up.

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
- `laser_frames_received`: complete optoNCDT measurement frames parsed since
  boot. Its rate should match the configured laser measuring rate.
- `laser_uart_errors` / `laser_parse_errors`: UART receive faults and malformed
  L/M/H byte sequences. Both should remain zero.
- `laser_invalid_frames`: complete frames reporting a reserve-band or sensor
  error value rather than an in-range distance.
- `laser_unexpected_values`: additional values after the distance value. This
  should remain zero because startup selects distance-only output.
- `laser_sync_errors`: UART/parser events while acquiring the initial eight
  consecutive distance frames. This may be non-zero when firmware attaches to
  a sensor that was already streaming; it is separate from steady-state loss.

`helic-daq diag-reset` (equivalent to `helic-daq set diag_reset 1`) clears the
laser fault counters together with the timing/event diagnostics, including the
safety clamp/quiet tick counts. It does not reset `laser_frames_received`, so
take before/after snapshots when checking the received frame rate.

If something looks wrong, the same numbers appear once a second in the
debug-probe log, along with connection events (and, for a safety-gated
experiment, the armed/tripped/clamp/quiet fields).

### Output safety (gated experiments)

Experiments that drive a hazardous actuator (e.g. `cbc-rig`) enable a firmware
output safety gate. Two parameters expose and control it:

- `arm` (writable): write `1` to arm the output, `0` to disarm. The output is
  **disarmed after every flash/reset** and is disarmed automatically when the
  MCU control connection drops. A direct connection therefore needs a
  persistent host session that arms once and holds the connection. With the
  broker, its upstream connection remains open across individual client
  departures, but the broker explicitly disarms when its final downstream
  client disconnects. The one-shot CLI rejects non-zero `arm` writes because
  it cannot keep a direct connection alive. Reading `arm` returns the armed
  bit.
- `safety` (read-only): a bitfield — bit0 armed, bit1 latched trip, bit2 clamped
  since last `diag_reset`, bit3 quieted since last `diag_reset`. A latched trip
  (the gate detected a fault such as an out-of-range or stalled sensor) holds the
  actuator quiet until the host re-arms with the fault condition cleared.

The exact per-experiment limits (amplitude window, fault conditions) live in the
experiment's `config.rs`; the streamed `out` source is the applied value after
the gate.

Arm from the same Python session that performs the experiment:

```python
with Device("192.168.1.235") as dev:
    dev.set("arm", 1)
    # Configure and run the experiment while this connection remains open.
```

**If `capture` times out with no data** while `status`/`get`/`set` work, check
whether a host firewall is blocking inbound UDP on the stream port. The host
libraries send a small UDP primer from the receive socket before starting the
stream, which lets ordinary stateful firewalls classify stream packets as
return traffic. Managed firewall policies can still drop those packets.
Control uses outbound TCP and can remain functional. On the managed macOS
machine used for bring-up, the Application Firewall silently dropped UDP to an
unsigned Homebrew Python executable; [../notes.md](../notes.md) records host
specific workarounds.
