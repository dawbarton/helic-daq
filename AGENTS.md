# HELIC-DAQ

Real-time control and data acquisition on RP2350 boards using Rust and
Embassy. HELIC-DAQ is the platform; CBC is one experiment under
`firmware/experiments/cbc-rig`.

## Read before changing code

- `docs/developer_guide.md`: architecture, design constraints and extension
  points.
- `docs/protocol.md`: authoritative wire protocol, including shared
  known-answer vectors.
- `docs/user_guide.md`: supported experiments and host workflows.
- `notes.md`: hardware verification status and bring-up constraints. Read and
  update it when doing hardware work.

There is no deployed protocol v1. Do not add compatibility shims. Crates are
`helic-core`, `helic-drivers` and `helic-proto`; the Python package is
`helic_daq`. The repository directory may still be named `cbc-daq`, but code
and current documentation use HELIC-DAQ except where CBC is the experiment.

## Architectural constraints

- Keep logic host-testable. DSP belongs in `helic-core`, portable peripheral
  logic in `helic-drivers`, and codecs in `helic-proto`. These crates are
  `no_std` and tested at the repository root. RP2350-specific shared plumbing
  belongs in `firmware/common`; experiment crates contain pins, constants,
  interrupt bindings, task wrappers and a thin `Rig` implementation.
- Keep the real-time path bounded: no allocation, blocking cross-core locks or
  `f64`. At 8 kHz, core 1 has 125 µs per tick and the Cortex-M33 only
  accelerates single-precision floating point.
- Preserve hardware-timed sampling. ADC experiments use PWM-driven CONVST and
  the BUSY falling edge; ADC-free experiments use a PWM-wrap interrupt. Do not
  replace either with software timing.
- Core 0 and core 1 communicate only through fixed-capacity SPSC queues and
  atomics. Parameter changes and waveform-buffer swaps take effect at sample
  boundaries. Streaming drops and counts records rather than blocking core 1.
- Controllers are selected statically through each experiment's
  `ActiveController` alias. Reusable controllers implement
  `helic_core::controller::Controller`; do not add runtime dispatch to the
  tick path.
- Parameters and stream sources are discovered by name on connection. Never
  hard-code registry or source indices in host code. New controller and rig
  parameters and controller telemetry use their trait hooks rather than wire
  protocol changes.
- Network transport is selected per experiment behind `embassy_net::Stack`.
  The W5500 is the full-rate path; CYW43439 Wi-Fi is station-mode and should
  use decimation for heavier streams.

## Hardware constraints worth preserving

- The current CBC build configures all AD5064 channels as unipolar for the
  interim analogue board. `DAC_POLARITY` in `cbc-rig/board.rs` must match the
  fitted output stages before hardware use.
- The optoNCDT UART input needs an idle-high line. The current rig uses an
  external 10 kΩ pull-up on GP1; without it, a disconnected sensor can cause
  a UART interrupt storm.
- The whirl rig uses two RMB20SC12BC96 encoders: 12-bit natural binary SSI at
  1 MHz with a shared clock. Its dual-SSI, optical-period, ADC-free signal
  generator, PWM, Pico 2W and arbitrary-waveform paths are not yet
  hardware-verified; consult `notes.md` before relying on them.

## Working conventions

- Use British English in prose with Oxford commas.
- Give every new source or configuration file a concise file-level comment
  describing its purpose, using the repository's module-documentation style
  where the language supports it.
- Add comments for non-obvious timing, safety, or hardware constraints, not
  to restate code.
- Keep commits to one logical unit. Use the established `<Area>: <what and
  why>` style and explain rationale in the body. Commit as you go.
- Preserve unrelated working-tree changes.

Before declaring a change complete, run the checks relevant to it. The full
set is:

```sh
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cd firmware
cargo fmt --all -- --check
cargo clippy --release --workspace -- -D warnings
cargo build --release --workspace
cd ../host
PYTHONPATH=.:tests python3 -m unittest discover -s tests
```

Software checks do not establish real-time, electrical, throughput or RF
behaviour. Record hardware evidence in `notes.md`.
