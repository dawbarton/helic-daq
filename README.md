# HELIC-DAQ

Hackable Experimental Laboratory Instrumentation and Control: a real-time
control and data acquisition platform for laboratory control, signal
generation and instrumentation. It targets RP2350 boards, uses Rust with
Embassy, and succeeds the BeagleBone Black-based
[rtc](https://github.com/dawbarton/rtc).

HELIC-DAQ can run a hardware-timed 1–8 kHz control loop, generate
phase-coherent Fourier and arbitrary waveforms, expose experiment parameters
at run time, and stream discovered signals over Ethernet or Wi-Fi. The
controller, rig and network transport are selected at compile time; the host
interface discovers their parameters and stream sources.

An optional Rust broker can hold the MCU connection for long-running
monitoring, share one stream among local clients, retain recent history, and
record segmented HDF5 files without changing the firmware.

## Documentation

- [User guide](docs/user_guide.md): experiments, flashing, networking and host
  interfaces.
- [Developer guide](docs/developer_guide.md): architecture, design principles,
  extension points and testing.
- [Wire protocol](docs/protocol.md): authoritative protocol v3 specification.
- [Shared broker](docs/broker.md): multi-client stream semantics, optional
  recording, host APIs, and recovery behaviour.
- [Periodic signal generator](docs/periodic_signal_generator.md): numerical
  design and error bounds.
- [Hardware status](notes.md): verified paths, outstanding checks and bring-up
  constraints.
- [Interim cape wiring](bbb-daq.md): BBB-DAQ analogue cape pin, power and
  bring-up reference.
- [Repository guidance](AGENTS.md): high-value constraints for contributors and
  coding agents.

## Experiments

| Firmware package | Board and purpose | Verification |
|---|---|---|
| `fw-cbc-rig` | W5500/W6100-EVB-Pico2, AD7609, AD5064 and optional optoNCDT | Core path verified on W5500; W6100 software verified |
| `fw-whirl-rig` | W5500/W6100-EVB-Pico2, dual RMB20 SSI and optical revolution pulse | Software verified; hardware verification pending |
| `fw-pico2w-rig` | Pico 2W and AD5064 over Wi-Fi | Software verified |

Here, software verified means that portable logic passes host tests and the
complete firmware target builds; it is not a claim about the physical path.
See [notes.md](notes.md) for the precise hardware-verification boundary.

## Layout

| Directory | Contents |
|---|---|
| `helic-core/` | Hardware-independent DSP, controllers and generators; `no_std`, host-tested |
| `helic-drivers/` | Portable peripheral drivers over `embedded-hal` traits; host-tested |
| `helic-proto/` | Protocol framing, payloads and stream codec shared with firmware |
| `helic-broker/` | Loopback-only shared stream broker with optional HDF5 recording |
| `firmware/common/` | Experiment-independent RP2350 firmware support |
| `firmware/experiments/` | One binary, pin map and compile-time configuration per experiment |
| `host-python/` | Python package `helic_daq`, simulator and `helic-daq` CLI |
| `host-julia/` | Julia package `HelicDAQ` with a Tables.jl capture interface |
| `host-matlab/` | MATLAB package `helicdaq` with native table captures |

## Build and test

```sh
cargo test
cargo build --release -p helic-broker
cd firmware && cargo build --release --workspace
cd ../host-python && PYTHONPATH=.:tests python3 -m unittest discover -s tests
cd ../host-julia && julia --project=. -e 'using Pkg; Pkg.instantiate(); Pkg.test()'
cd ../host-matlab && matlab -batch "runTests()"
```

CI also gates both Rust workspaces with formatting and clippy warnings as
errors, and tests the Python, Julia, and MATLAB packages. See the developer
guide for the complete check set.

## Flash and connect

With a debug probe and
[probe-rs](https://probe.rs), flash the CBC experiment and stream its defmt
log:

```sh
cd firmware
cargo run --release -p fw-cbc-rig
```

Install the host package from the repository root, discover devices, and
inspect one:

```sh
pip install -e host-python
helic-daq find
helic-daq --host 192.168.1.235 status
julia --project=host-julia -e 'using Pkg; Pkg.instantiate()'
matlab -batch 'addpath("host-matlab"); helicdaq.findDevices()'
```

The [user guide](docs/user_guide.md) covers BOOTSEL/UF2 flashing, all firmware
packages, static addresses, Wi-Fi configuration and the simulator.
