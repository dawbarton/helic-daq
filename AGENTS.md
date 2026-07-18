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

The supported production firmware set is exactly `cbc-rig`, `whirl-rig` and
`pico2w-rig`. Do not restore retired experiment crates. Adding a genuinely new
experiment also requires updating the firmware workspace, CI, the real-time
layout checker, the regression-tool profiles, the user/developer guides and
`notes.md`.

## Architectural constraints

- Keep logic host-testable. DSP belongs in `helic-core`, portable peripheral
  logic in `helic-drivers`, and codecs in `helic-proto`. These crates are
  `no_std` and tested at the repository root. RP2350-specific shared plumbing
  belongs in `firmware/common`; experiment crates keep an auditable `board.rs`
  pin/ownership map, compile-time configuration, telemetry declarations and a
  thin experiment-local `Rig` implementation.
- Keep every experiment crate predictable: `board.rs` owns only pins and
  unassembled peripheral parts; `config.rs` owns compile-time choices;
  `telemetry.rs` owns atomic-backed declarations; `rig.rs` assembles core-1
  hardware and implements `Rig`; and `main.rs` binds interrupts, assigns cores
  and composes common runners. Move reusable mechanisms out rather than adding
  experiment-local framework wrappers.
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
  Fixed-array operations may lower to ARM EABI memory helpers, so keep
  `rt_mem` and the layout check in place; SRAM annotations on the Rust caller
  alone do not prove that compiler-generated calls avoid flash.
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
  driver can lose its alarm (`docs/overrun_handoff.md`); without the
  watchdog every core-0 timer can freeze until unrelated network traffic
  arrives.
- Core 0 and core 1 communicate only through fixed-capacity SPSC queues and
  atomics. Parameter changes and waveform-buffer swaps take effect at sample
  boundaries. Apply at most `COMMANDS_PER_TICK` commands (currently two) at a
  boundary; do not drain an arbitrary host burst inside one tick. Preserve
  `cmd_backlog_max` so queue pressure remains observable. Streaming drops and
  counts records rather than blocking core 1.
- Keep raw-register access behind ownership-preserving common types. Derive PIO
  blocks and GPIO numbers from typed Embassy owners instead of accepting free
  numeric identifiers. `RawSpiDevice::new` is unsafe because Embassy erases
  chip-select types; construct it once beside the audited experiment pin map,
  document the exclusivity invariant and expose only safe bound operations to
  the tick path.
- Controllers are selected statically through each experiment's
  `ActiveController` alias. Reusable controllers implement
  `helic_core::controller::Controller`; do not add runtime dispatch to the
  tick path.
- Parameters and stream sources are discovered by name on connection. Never
  hard-code registry or source indices in host code. New controller and rig
  parameters and controller telemetry use their trait hooks rather than wire
  protocol changes. Keep the fixed platform schema in `params/schema.rs`, and
  use the typed `ExtraParam::f32`/`u32` constructors for atomic-backed
  experiment telemetry; do not reintroduce free-form type/getter pairs that
  can disagree with storage.
- Network transport is selected per experiment behind `embassy_net::Stack`.
  The W5500 is the full-rate path; CYW43439 Wi-Fi is station-mode and should
  use decimation for heavier streams. Pico 2W credentials come only from the
  `HELIC_WIFI_SSID` and `HELIC_WIFI_PASSWORD` build environment; never commit
  real credentials or placeholder fallbacks.

## Safety rails and regression helpers

- `firmware/tools/check_rt_layout.py` is the static hot-path gate. Build the
  complete release workspace immediately before running it; it checks all
  three production ELFs and must continue to require `run_hot_loop`, the ARM
  EABI copy/clear helpers and each applicable analogue transfer symbol in
  SRAM. Treat it as a minimum named-symbol guard, not a complete call-graph
  proof. Inspect new compiler-generated calls after material tick-path changes.
- `firmware/tools/rt_regression.py` is the sequential hardware runner. It
  flashes one profile, checks identity, measures idle/TCP-poll/capture phases,
  verifies counters, rate, wake-phase spread and capture continuity, then
  quiets outputs. CBC additionally gates `loop_time_max <= 60 µs`; the current
  W5500 reference is 32–34 µs (38 µs during complete coefficient replacement).
  Do not relax an acceptance limit to accommodate a new regression.
- For record/network changes, run the CBC profile once with
  `--capture-sources all --capture-samples 8000`, then once with
  `--no-flash --capture-samples 60000`. For core-0 timer/network changes, also
  disconnect for at least five minutes, reconnect and prove the drain/watchdog
  counters stayed healthy. Record exact firmware identity and results in
  `notes.md`.
- Software checks, ELF addresses and successful streaming do not establish
  electrical, RF or real-time behaviour. Do not promote whirl, Pico 2W or
  W6100 paths from software-only status without ordered physical evidence.

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
python3 tools/check_rt_layout.py
cargo build --release -p fw-cbc-rig --no-default-features --features board-w6100
cargo build --release -p fw-whirl-rig --no-default-features --features board-w6100
cd ../host-python
PYTHONPATH=.:tests python3 -m unittest discover -s tests
cd ../host-julia
julia --project=. -e 'using Pkg; Pkg.instantiate(); Pkg.test()'
cd ../host-matlab
matlab -batch "runTests()"
```

Software checks do not establish real-time, electrical, throughput or RF
behaviour. Record hardware evidence in `notes.md`.
