# HELIC-DAQ

Real-time control and data acquisition on RP2350 boards using Rust and
Embassy. HELIC-DAQ is the platform; CBC is one experiment under
`firmware/experiments/cbc-rig`.

## Read before changing code

- `docs/developer_guide.md`: architecture, design constraints and extension
  points.
- `docs/rig_decoupling_proposal.md`: the completed component-ownership design,
  platform capacities, crate boundaries and external-rig contract. It
  supersedes conflicting parts of `docs/rt_program_proposal.md`.
- `docs/rt_program_proposal.md`: programme ownership, tick ordering and table
  phase semantics; use the rig-decoupling document where the two differ.
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
experiment also requires updating the firmware workspace and CI, adding its
rig-owned verification profile, and updating the user/developer guides and
`notes.md`. The shared layout and regression tools discover profiles as data;
do not add an experiment-specific registry to either tool.

## Component placement and ownership

Place code according to its execution and reuse boundary, not according to the
experiment in which it was first needed:

| Location | What belongs there |
|---|---|
| `helic-core` | Portable, allocation-free DSP, generators, controllers, estimators, filters, safety primitives and owner-checked buffers. Promote a rig-local algorithm here when it has two actual consumers, or when it is deliberately accepted as a platform primitive. |
| `helic-rt` | Portable, Embassy-free `Rig`, `TickSource` and `Program` contracts; commands, records and queues; `RtShared`; component parameter groups and `ParamStore`; source assembly and the pure safety decision. |
| `helic-drivers` | Portable chip, sensor and peripheral logic expressed over `embedded-hal`, without RP2350 board policy. |
| `helic-proto` | Wire and broker codecs, type codes, framing and protocol constants. |
| `firmware/rt` | Mandatory synchronous RP2350 core-1 mechanisms used by every rig: the loop driver, tick sources, raw PIO/SPI adapters and EABI SRAM shims. No executor, `embassy-time`, network or `firmware/support` dependency. |
| `firmware/support` | Core-0 services used by every production rig: communications, identity, status, networking and the time watchdog. Optional or rig-specific services do not belong here. |
| `firmware/integrations/<device>` | Optional hardware-facing Embassy services used by some rigs, such as the optoNCDT UART integration. Keep portable device logic in `helic-drivers`. |
| `firmware/experiments/<rig>` | The complete rig-specific implementation. Keep dependency-light, host-tested `no_std` computation in the package's library target; keep `board.rs`, `config.rs`, `telemetry.rs`, `rig.rs` and `main.rs` as physical composition and auditable glue; keep `rig-profile.toml` beside them. |
| Host packages | Processing reconstructible from streamed samples, including multi-channel estimation, continuation/update laws and offline analysis. Device-only inputs such as PIO FIFOs remain on core 1. |

- Keep every experiment crate predictable: `board.rs` owns only pins and
  unassembled peripheral parts; `config.rs` owns compile-time choices and the
  concrete `ActiveController`/`ActiveProgram`; `telemetry.rs` owns atomic-backed
  declarations; `rig.rs` assembles core-1 hardware and implements `Rig`; and
  `main.rs` binds interrupts, assigns cores and composes common runners. Move
  code to shared crates only when reuse is established rather than scattering
  one rig across repository-level packages. A rig-local library target must
  remain host-testable without pulling in Embassy or RP2350 dependencies.
- `Program` owns logical sample-rate computation: the master phase, controller,
  signal generators, table player, programme command domains, signals and
  programme-originated faults. `Rig` owns physical measurement, vector
  actuation, hardware parameters, clamps, safe values, physical faults and the
  reboot-safe sequence. The common loop alone owns tick ordering, bounded
  command dispatch, safety application and coherent record assembly. Do not
  duplicate any of these responsibilities in an experiment.
- Core 0 may walk fixed-capacity trait-object parameter groups during control
  requests. Core 1 remains statically dispatched through the concrete
  `ActiveProgram`, `Rig` and `TickSource`; do not add `dyn`, allocation or
  run-time programme/controller selection to the tick path.
- Each firmware application owns one const-initialised `RtShared` and injects
  the same reference into `ParamStore` and the core-1 loop. Do not restore
  library statics or share mutable loop state directly across cores.
- Preserve the common per-tick order: wait on the continuously armed hardware
  latch; call `tick_start`; apply at most two commands; measure; step the
  programme exactly once; evaluate global faults and safety; actuate the whole
  applied vector once; publish programme signals and assemble the coherent
  record; enqueue or count a drop; then call `tick_end` and finish diagnostics.
  Commands precede phase advancement, and measurement precedes control.

## Architectural constraints

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
  `TIMER0` reads; the ADC/DAC transfers use `helic_fw_rt::analog_spi`.
  Fixed-array and non-`Copy` command moves may lower to ARM EABI memory
  helpers, so keep `rt_mem` and the layout check in place; SRAM annotations on
  the Rust caller alone do not prove that compiler-generated calls avoid flash.
  After touching the tick path, run the regression checklist in the
  developer guide before calling the change done.
- Keep the BUSY edge-detect latch continuously armed in `BusyEdgeSpinTick`.
  Re-arming per wait (as the async `InputFuture` does) silently loses edges
  that arrive while a tick body runs; the latch is what makes a late tick
  catch up instead of skipping a sample.
- Preserve hardware-timed sampling. ADC experiments use PWM-driven CONVST and
  the latched BUSY falling edge; ADC-free experiments poll the raw PWM-wrap
  latch. Do not replace either with software timing or an interrupt future.
- Keep `helic_fw_support::time_watchdog` bound to `TIMER0_IRQ_1` and started
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
- Safety ownership is asymmetric: only core 0 arms or disarms; core 1 may only
  load that state and monotonically latch a trip. Rig faults, programme faults
  and non-finite commands all trip and quiet the complete actuator vector;
  clamps are per actuator, and streamed outputs are the applied values. Do not
  write armed state back from a core-1 snapshot or weaken the global-trip
  default for a coupled rig.
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
  protocol changes. A `ParamGroup` owns its definitions, shadows, validation,
  staging, acceptance and rejection; `ParamStore` only maps the discovered
  global index to a group-local id, constructs the command address and commits
  transactionally after queueing. Keep the fixed platform group beside the
  other reusable groups in `helic-rt/src/params/groups.rs`, and use typed
  `ExtraParam::f32`/`u32` constructors for atomic-backed experiment telemetry;
  do not restore a central per-rig schema or free-form type/getter pairs.
- Assemble stream sources generically in this fixed order: `Rig::INPUTS`,
  `Program` signals, applied `Rig::ACTUATORS`, then `cmd_epoch`. Rigs and
  programmes declare names and write only their own slices; they never assign
  global slots. A programme with a master accumulator must expose coherent
  `phase` in turns. Stream post-safety applied outputs, not raw commands.
- Writable autonomous quantities are setpoints. Expose the actual value the
  programme used separately as a stream source or read-only telemetry, and
  document its tick semantics. Use `ExtraParam` only for independent scalar
  latest-value views; correlated multi-value quantities belong in one coherent
  `Record`.
- Keep table phase semantics inside the host-tested `TablePlayer`. `Loop` and
  `OneShot` use its private accumulator and ignore the master phase;
  `LockedLoop` derives phase by wrapping integer multiply and offset;
  `LockedOneShot` starts on the next true master-accumulator overflow and must
  not test `phase == 0`. Table activation occurs before programme stepping so
  the record whose `cmd_epoch` reports the command sees the new bank.
- `helic_core::Pll` is a measured-force/response, fundamental-only device-side
  tracker, not a commanded-oscillator tracker. Preserve its explicit
  `Fixed`/`Acquiring`/`Locked`/latched-`LockLost` state machine: acquisition
  never faults or removes its own excitation, only post-lock loss faults, and
  the commanded increment remains bounded by host-set limits.
- Network transport is selected per experiment behind `embassy_net::Stack`.
  The W5500 is the full-rate path; CYW43439 Wi-Fi is station-mode and should
  use decimation for heavier streams. Pico 2W credentials come only from the
  `HELIC_WIFI_SSID` and `HELIC_WIFI_PASSWORD` build environment; never commit
  real credentials or placeholder fallbacks.

### Bounded platform contract

- Production bounds are `MAX_SOURCES = 24`, `MAX_ACTUATORS = 4`,
  `MAX_GROUPS = 8`, `MAX_RT_VALUES = 33`, `MAX_FORCE_VALUES = 132`,
  `COMMAND_QUEUE_LEN = 32`, `COMMANDS_PER_TICK = 2` and
  `MAX_HARMONICS = 16`. Harmonic count and waveform-table capacity are
  experiment-selected const generics; the current production table capacity
  is 4096.
- Changing an existing shared capacity in either direction is a breaking
  platform change because it changes queue or record layout, SRAM use or WCET.
  Require memory, discovery, layout and hardware-timing evidence; do not raise
  a bound merely to make one rig compile. Prefer a const generic with a
  documented maximum when each rig should pay only for what it uses.
- Treat signature changes, non-defaulted trait additions, wire-visible name or
  semantic changes and capacity changes as major-version changes. Additive
  types and defaulted trait methods may be minor changes. External rigs pin a
  major version and upgrade deliberately.
- Target and forcing coefficients, waveform tables and future large force
  vectors use `helic_core::DoubleBuffer<T>` with its one-time split and linear,
  owner-checked token. Do not widen copied command payloads or introduce a raw
  mutable cross-core buffer. Activation occurs at a sample boundary; rejected
  commands return their token and leave the active bank unchanged.

## Safety rails and regression helpers

- Every production experiment owns `firmware/experiments/<rig>/rig-profile.toml`
  as its static and hardware verification contract: identity, package and ELF,
  required/optional hot symbols, exact EABI helpers, sample rate, transport,
  capture sources, acceptance limit and ordered quieting writes. Update that
  profile when the rig or hot-path boundary changes; do not add a rig by
  hard-coding it into the shared tools.
- `helic-rt-layout` (`helic_daq.verify.layout`) is the static hot-path gate. Build the
  complete release workspace immediately before running it; it checks all
  three production ELFs and must continue to require `run_hot_loop`, the ARM
  EABI generic/aligned copy and clear helpers and each applicable analogue
  transfer symbol in SRAM. Treat it as a minimum named-symbol guard, not a
  complete call-graph proof. Inspect new compiler-generated calls after
  material tick-path changes.
- `helic-rt-regression` (`helic_daq.verify.regression`) is the sequential hardware runner. It
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
- A separately maintained rig owns its portable programme, firmware crate,
  target configuration, lockfile, exact shared-crate pins, dependency policy
  and `rig-profile.toml`. It installs the host package and drives
  `helic-deps-check --policy ...`, `helic-rt-layout --profile ... --elf-dir ...`
  and `helic-rt-regression --profile ... --firmware-dir ...` without editing or
  copying those tools. Every tool resolves its defaults from the current
  directory, so none of them assumes a checkout of this repository.
- Keep `tests/external-rig` passing as the repository-separation acceptance
  fixture. It holds two firmware members on purpose: `fw-fixture-rig` links no
  core-0 support, proving the real-time platform stands alone, and
  `fw-fixture-service-rig` composes `helic-fw-support`, covering the generic
  control, streaming, discovery, status, watchdog and identity paths every
  production rig uses. Do not let either lapse; the second exists because the
  first cannot see defects in the half it excludes. Its local
  `[patch.crates-io]` is only a pre-publication checkout substitution, not
  evidence that the shared crates have been released.

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
- Confirm the Ethernet controller physically attached before flashing a wired
  build. A successful W6100 cross-build is software evidence only and never
  authorises flashing that image to W5500 hardware.

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
cargo clippy --manifest-path firmware/experiments/whirl-rig/Cargo.toml \
  --lib --target x86_64-unknown-linux-gnu --no-default-features -- -D warnings
cargo test --manifest-path firmware/experiments/whirl-rig/Cargo.toml \
  --lib --target x86_64-unknown-linux-gnu --no-default-features
pip install -e host-python   # provides helic-rt-layout and helic-deps-check
cd firmware
helic-deps-check
helic-deps-check --workspace ../tests/external-rig \
  --policy ../tests/external-rig/dependency-policy.toml
cargo fmt --all -- --check
cargo clippy --release --workspace -- -D warnings
cargo build --release --workspace
cargo fmt --manifest-path build/Cargo.toml -- --check
cargo clippy --manifest-path build/Cargo.toml --all-targets \
  --target x86_64-unknown-linux-gnu -- -D warnings
cargo test --manifest-path build/Cargo.toml --target x86_64-unknown-linux-gnu
helic-rt-layout
cargo build --release -p fw-cbc-rig --no-default-features --features board-w6100
cargo build --release -p fw-whirl-rig --no-default-features --features board-w6100
cd ../tests/external-rig
cargo fmt --all -- --check
cargo clippy --release --workspace -- -D warnings
cargo test -p fixture-rig-program --target x86_64-unknown-linux-gnu
cargo build --release --workspace
helic-rt-layout --profile rig-profile.toml --profile service-rig-profile.toml \
  --elf-dir target/thumbv8m.main-none-eabihf/release
helic-deps-check --policy dependency-policy.toml
cd ../../host-python
PYTHONPATH=.:tests python -m unittest discover -s tests
python -m unittest discover -s ../tests/external-rig -p 'test_*.py'
cd ../host-julia
julia --project=. -e 'using Pkg; Pkg.instantiate(); Pkg.test()'
cd ../host-matlab
matlab -batch "runTests()"
```

Software checks do not establish real-time, electrical, throughput or RF
behaviour. Record hardware evidence in `notes.md`.
