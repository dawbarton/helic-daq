# HELIC-DAQ

A real-time control and data acquisition platform for laboratory control,
signal generation and instrumentation, built on an RP2350
(W5500-EVB-Pico2) with Rust + Embassy. Its first experiment supports
control-based continuation (CBC), succeeding the BeagleBone Black-based
[dawbarton/rtc](https://github.com/dawbarton/rtc); see
`docs/implementation_plan.md` §10 for what was carried over and what
changed.

**Aim**: sample up to 8 analogue channels at a jitter-free 1–8 kHz,
run a real-time controller between measurement and output within one
sample period, generate phase-coherent periodic/arbitrary reference
signals, and let a host computer get/set parameters and stream data
live over Ethernet — all while keeping the controller and DSP
swappable at compile time for different physical experiments.

## Current priorities (2026-07)

Two coordinated transitions are underway. Read
`docs/multi_experiment_plan.md` **before** changing firmware, protocol,
or host code — it is the authoritative spec for this work, and its
decisions table (D1–D11) is settled: don't re-open those choices.

- **Multi-experiment restructure**, following the plan's ten phases
  (phase 1, splitting the firmware workspace into `firmware/common` +
  `firmware/experiments/*`, is in progress). Order of work: workspace
  restructure → Rig/TickSource abstractions → protocol v2 → host
  simulator → arbitrary waveform generator → the sig-gen / pwm-rig /
  encoder-rig example experiments → CI + docs.
- **Rename `cbc-daq` → `helic-daq`** (in progress). HELIC-DAQ is the
  platform; CBC is just its first experiment, and
  `firmware/experiments/cbc-rig` deliberately keeps the CBC name.
  Crates are `helic-core`/`helic-drivers`/`helic-proto`, the Python
  package is `helic_daq`. Fix stray `cbc`/`cbc_daq` references on
  sight — except cbc-rig itself and historical design docs.
- **No deployed v1 exists** — no fielded firmware or hosts speak the
  v1 protocol. Wire protocol, parameter names, registry layout, and
  crate names are all free to change; never add v1 compatibility
  shims.

## Where things are documented

Don't re-derive this from the code — read the docs first:

- `docs/multi_experiment_plan.md` — **the current work**: the
  multi-experiment restructure, protocol v2, arbitrary waveform
  generator, and new drivers, as a phased spec with settled decisions,
  acceptance criteria, and a performance budget.
- `docs/user_guide.md` — what the device does, flashing, connecting,
  CLI/Python usage.
- `docs/developer_guide.md` — architecture, cross-core design, how to
  add a controller/parameter/stream source, timing budget.
- `docs/protocol.md` — the wire protocol, authoritative, with
  known-answer test vectors shared by the Rust and Python codecs.
  Documents v1 until the plan's phase 3 lands; protocol v2 is
  specified in `docs/multi_experiment_plan.md` §6 and replaces it
  outright (no v1 compatibility).
- `docs/implementation_plan.md` — design rationale and milestone
  history.
- `docs/periodic_signal_generator.md` — the phase-accumulator design
  the generators implement.
- `notes.md` — status of the most recent hardware bring-up session:
  what's been tested on real hardware, what's confirmed working, and
  the current open bug. **Read this before starting a new hardware
  session**; update it when you end one.

## Hardware target

- RP2350 boards generally; network transport is a per-experiment
  choice behind `embassy_net::Stack` (plan D11/§6.7):
  - W5500-EVB-Pico2 (on-board Wiznet W5500, wired Ethernet) — the
    primary target.
  - Raspberry Pi Pico 2W (CYW43439 Wi-Fi, station mode, DHCP) —
    planned via the `sig-gen-w` experiment; note its LED is on the
    CYW43, not GP25, and full-rate streaming needs wired Ethernet.
- AD7609: 8-channel, 18-bit true-bipolar differential ADC, ±10 V/±20 V,
  SPI readout, MCU-timed CONVST, BUSY handshake. Range/oversampling
  set via logic-level GPIO (a future AD7606B would set these over SPI
  instead — kept behind the `AnalogIn` trait for that reason).
- AD5064: 4-channel, 16-bit DAC, SPI. Two channels are bipolar via
  external op-amp stages (±4.096 V), two unipolar (0–4.096 V). A
  future AD5764 swap is kept behind the `AnalogOut` trait.
- Micro-Epsilon optoNCDT 1420 laser displacement sensor via RS422→TTL,
  UART, binary framing. Optional peripheral — firmware must degrade
  gracefully if it's absent (see `notes.md` for a bug in this area
  found during bring-up).
- Planned peripherals (see `docs/multi_experiment_plan.md` §7): an
  RLS RMB20 absolute encoder (SSI via RS422→TTL, read by PIO) and a
  PWM-based analog output stage. Accommodating peripherals like these
  without restructuring is why drivers sit behind `embedded-hal`
  traits rather than being wired directly into the RT loop.

## Conventions

- **Everything with logic in it must be host-testable.** `helic-core`
  (DSP), `helic-drivers` (peripheral drivers), and `helic-proto` (wire
  protocol) are `no_std` but compile and `cargo test` on the host —
  drivers are generic over `embedded-hal` traits with mock
  implementations, not tied to `embassy-rp` types. `firmware/` is a
  separate Cargo workspace (own `.cargo/config.toml`, always targets
  `thumbv8m.main-none-eabihf`): RP2350-specific but
  experiment-independent plumbing lives in `firmware/common`, and each
  physical experiment is a thin bin crate under
  `firmware/experiments/` (~300–600 lines: pin wiring in `board.rs`,
  constants in `config.rs`, `Rig` impl and task wrappers). If you're
  adding logic and can't write a `#[test]` for it on the host, it's
  probably in the wrong crate; if an experiment crate is growing
  logic, it belongs in `common` or a shared crate.
- **No allocation, no `f64` in the real-time path.** The RT loop
  (`firmware/common/src/rt_loop.rs`, core 1) runs at up to 8 kHz with a
  125 µs budget; the Cortex-M33 FPU is single-precision only, so `f64`
  silently gets software-emulated and will blow the budget. Use
  `heapless` containers, not `alloc`.
- **Compile-time swappable controller.** The active `Controller` is a
  type alias in each experiment's `config.rs` (e.g.
  `firmware/experiments/cbc-rig/src/config.rs`), not a runtime
  dispatch — a deliberate design goal (different physical experiments
  need different control laws with zero runtime overhead). New
  controllers implement `helic_core::controller::Controller`;
  `param_names`/`set_param` make their gains host-visible
  automatically, no protocol changes needed — and per the plan (D8),
  rigs and controller telemetry follow the same pattern.
- **Cross-core communication is lock-free, always.** Core 0 (host
  comms) and core 1 (RT loop) talk only via `heapless::spsc` queues
  and `AtomicU32`/similar statics — never a blocking mutex across
  cores. Parameter writes apply at a sample boundary (array-valued
  parameters travel by value through the queue, so a tick never sees
  a torn write); stream records drop-and-count on overflow rather
  than blocking the RT loop.
- **The wire protocol is discoverable, not hard-coded.** Parameters
  are a name-indexed registry (`firmware/common/src/params.rs`) the host
  reads at connect (`docs/protocol.md`) — adding a parameter is a
  firmware-only change. Protocol v2 extends the same principle to
  stream sources (a per-experiment name+unit table, plan §6). Don't
  hard-code parameter or source indices on the host side.
- **Doc comments explain why, not what.** Default to no comments;
  when you add one, it's because of a non-obvious constraint (a
  datasheet timing requirement, a hardware quirk, a reason a simpler
  approach doesn't work) — not a restatement of the code.
- **Commits are one logical unit each**, referencing the milestone/
  area in the subject line (see `git log` for the established style:
  `<Area>: <what and why>` or `<Area> (milestone N)`), with detail on
  *why* in the body, not a diff summary.
- **Verify before calling something done**: `cargo test` at the repo
  root (host crates), `cd firmware && cargo build --release --workspace`
  (and `cargo clippy --release --workspace -- -D warnings`), `cd host &&
  PYTHONPATH=.:tests python -m unittest discover -s tests`. CI runs all three as separate
  jobs (`.github/workflows/ci.yml`) with `cargo fmt --check` +
  `clippy -D warnings` gating the Rust ones. None of this substitutes
  for actual hardware verification — see `notes.md` for how much of
  this system has and hasn't been run on real hardware yet.
