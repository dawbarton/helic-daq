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
`helic_daq`, the Julia package is `HelicDAQ`, and the MATLAB package is
`helicdaq`. The repository directory may still be named `cbc-daq`, but code
and current documentation use HELIC-DAQ except where CBC is the experiment.

## Architectural constraints

- Keep logic host-testable. DSP belongs in `helic-core`, portable peripheral
  logic in `helic-drivers`, and codecs in `helic-proto`. These crates are
  `no_std` and tested at the repository root. RP2350-specific shared plumbing
  belongs in `firmware/common`; experiment crates keep an auditable `board.rs`
  pin/ownership map, compile-time configuration, telemetry declarations and a
  thin experiment-local `Rig` implementation.
- Keep the real-time path bounded: no allocation, blocking cross-core locks or
  `f64`. At 8 kHz, core 1 has 125 µs per tick and the Cortex-M33 only
  accelerates single-precision floating point.
- Keep the mandatory core-1 tick path SRAM-resident and Embassy-free. There is
  deliberately no async fallback. Everything
  reachable per tick on core 1 must carry
  `#[unsafe(link_section = ".data.ram_func")]` (or inline into a function
  that does) and must not call into the embassy executor, `embassy-time`,
  async GPIO/SPI, `defmt`, or anything taking a critical section: the shared
  XIP cache and the global cross-core spinlock let core-0 network traffic
  stretch flash-resident tick code past the whole sample period (see
  "Real-time isolation" in `docs/developer_guide.md`). Timing uses raw
  `TIMER0` reads; the ADC/DAC transfers use `helic_fw_common::analog_spi`.
  After touching the tick path, run the regression checklist in the
  developer guide before calling the change done.
- Keep the BUSY edge-detect latch continuously armed in `BusyEdgeSpinTick`.
  Re-arming per wait (as the async `InputFuture` does) silently loses edges
  that arrive while a tick body runs; the latch is what makes a late tick
  catch up instead of skipping a sample.
- Preserve hardware-timed sampling. ADC experiments use PWM-driven CONVST and
  the latched BUSY falling edge; ADC-free experiments poll the raw PWM-wrap
  latch. Do not replace either with software timing or an interrupt future.
- Keep `helic_fw_common::time_watchdog` bound to `TIMER0_IRQ_1` and started
  on core 0 in every experiment that uses embassy-time. The embassy-rp time
  driver can lose its alarm (`docs/embassy_time_alarm_loss.md`); without the
  watchdog every core-0 timer can freeze until unrelated network traffic
  arrives.
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
  interim analogue board. `DAC_POLARITY` in `cbc-rig/rig.rs` must match the
  fitted output stages before hardware use.
- The optoNCDT UART input needs an idle-high line. The current rig uses an
  external 10 kΩ pull-up on GP1; without it, a disconnected sensor can cause
  a UART interrupt storm.
- The whirl rig uses two RMB20SC12BC96 encoders: 12-bit natural binary SSI at
  1 MHz with a shared clock. Its dual-SSI and optical-period paths, and the
  Pico 2W Wi-Fi/DAC path, are not yet hardware-verified; consult `notes.md`
  before relying on them.

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
- Communicate with real DAQ hardware sequentially. Do not run parallel
  processes, parallel tool calls or overlapping clients against the DAQ; the
  control server is single-client and hardware evidence must come from ordered
  interactions.
- Format Julia code with Runic.jl via the `runic` command.

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
cd ../host-python
PYTHONPATH=.:tests python3 -m unittest discover -s tests
cd ../host-julia
julia --project=. -e 'using Pkg; Pkg.instantiate(); Pkg.test()'
cd ../host-matlab
matlab -batch "runTests()"
```

Software checks do not establish real-time, electrical, throughput or RF
behaviour. Record hardware evidence in `notes.md`.
