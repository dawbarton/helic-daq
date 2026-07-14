# Multi-experiment support plan

Status: **in progress** (phase 2 software complete; hardware verification
pending). This document is the
implementation spec for making HELIC-DAQ support multiple physical
experiments from one repository, plus the protocol-v2 and generator
changes agreed alongside it. It is written to be actionable by an
implementer without access to the discussion that produced it.

**Compatibility baseline: there are no deployed v1 devices or hosts.**
Nothing constrains the wire protocol, parameter names, crate names, or
registry layout. Do not add compatibility shims for v1.

## 1. Motivation — what blocks a second experiment today

An architecture review (2026-07-14) found the trait seams are already in
the right places (`Controller`, `AnalogIn`/`AnalogOut`, `embedded-hal`
drivers, single-file pin map), but three couplings make any experiment
that differs from the current rig a source-surgery job rather than a
configuration:

1. **The tick is the ADC.** The RT loop's heartbeat is
   `adc_busy.wait_for_falling_edge()` (`firmware/experiments/cbc-rig/src/rt_loop.rs`). With
   no AD7609 fitted the loop degrades to *half* the sample rate on a
   software timeout, silently losing crystal timing while
   `busy_timeouts` climbs. A DAC-only or PWM-only experiment has no
   correct tick source.
2. **The measurement/record/stream-source shape is fixed.**
   `Measurements { adc: [f32; 8], laser: f32 }` (helic-core),
   `Record { adc: [f32; 8], laser, target, forcing, out }` (firmware),
   and `helic_proto::source::{LASER=8, TARGET=9, ...}` plus the mirrored
   Python `SOURCES` dict hard-code exactly one sensor suite. Adding or
   removing a sensor touches three crates plus the host. This is the
   one place the project's own "discoverable, not hard-coded"
   convention (see AGENTS.md, applied to parameters) was not applied.
3. **The peripheral set is unconditional.** `main.rs` always spawns
   `laser_task`, always builds the full `AnalogParts`, always reserves
   the corresponding pins. An experiment without the laser still
   carries its UART claim and external pull-up requirement.

Conversely, these need essentially nothing: the control law
(`ActiveController` alias), the output stage (`AnalogOut` trait exists;
only the call site is concrete), and pin mapping (`board.rs` is already
the single wiring point).

## 2. Decisions taken

Decided by the project owner on 2026-07-14; do not re-open without
checking first.

| # | Decision | Choice |
|---|---|---|
| D1 | How experiments coexist | **One thin firmware crate per experiment**, sharing a `common` firmware library crate and the existing host-testable crates. Not cargo features, not branches. |
| D2 | Stream sources | **Discoverable source table** (protocol v2): firmware publishes named sources (with units) at connect, mirroring the parameter registry. Fixed `source::*` IDs are removed. |
| D3 | Tick without an ADC | **PWM-wrap interrupt** on a free-running PWM slice using the existing `SampleRate` divider/top presets — crystal-exact, same presets, no new hardware. Not a software `Ticker`. |
| D4 | Concrete deliverables | All three worked examples: a **DAC-only/no-ADC** experiment, a **PWM output** driver + experiment, and an **SSI encoder** driver + experiment. |
| D5 | Encoder target | **RLS RMB20** absolute magnetic encoder, SSI interface, through an RS422→TTL converter, read by a **PIO** state machine. |
| D6 | Arbitrary waveform generation | **Required.** Host uploads a table of up to `MAX_TABLE_LEN` samples; the firmware plays it out with linear interpolation over a host-specified time scale (loop or one-shot), summed into the output, **optionally phase-locked** to the master generator phase at an exact integer multiple (§6.4). Transport is the staged `SetBlock`/`Commit` mechanism v1 had reserved message types for. |
| D7 | Protocol simplifications | Merge name/info discovery into single `GetParams`/`GetSources` messages; source discovery carries **unit strings**; `busy_timeouts` renamed `tick_timeouts`. |
| D8 | Rig parameters + controller telemetry | Rigs expose host-settable parameters exactly like controllers do; controllers declare telemetry stream sources (e.g. the control error). |
| D9 | Identity & network conveniences | `experiment` name parameter, git hash in the firmware string, UDP discovery beacon (`helic-daq find`), optional DHCP selected in `config.rs`. |
| D10 | Host simulator | The Python test fake is promoted to a runnable `helic_daq.sim` speaking full protocol v2 over localhost. |

### Secondary design calls made in this plan

Treat these as decided unless they prove unworkable (§9 lists risks):

- `MAX_SOURCES = 24` fixed in the common crate; `Record` carries a
  fixed-size `[f32; MAX_SOURCES]` with a runtime length. This keeps the
  UDP stream task and record queue **non-generic** (shared, concrete)
  at a memory cost of ~26 KB for the 256-record ring — trivial against
  the RP2350's 520 KB. Sized for the worst case: 10 hardware inputs
  (adc0–7, laser, encoder) + controller telemetry + the 4 appended
  generator slots. `MAX_STREAM_SOURCES` (16 in `comms/mod.rs`) merges
  into this constant.
- `HARMONICS = 16` stays a **common** constant, not per-experiment.
  Per-experiment harmonic counts would make `RtCommand`,
  `FourierCoeffs`, and the coefficient wire size generic; not worth it.
  Experiments needing fewer harmonics leave coefficients zero.
- `MAX_TABLE_LEN = 4096` samples for the arbitrary waveform (16 KB per
  buffer, double-buffered = 32 KB). Uploads shorter tables freely;
  1024 is the expected typical size.
- The generic-code/embassy-task tension is resolved by the
  **"generic async fn, concrete task wrapper"** pattern: embassy tasks
  cannot be generic, but a concrete `#[embassy_executor::task]` in each
  experiment crate can call a generic `async fn` from the common crate.
- The experiment's source table lists **inputs only**; the common loop
  appends the controller's telemetry sources, then `target`,
  `forcing`, `table`, `out` as the last four slots. Experiments never
  do slot-index bookkeeping.
- Message type numbers are renumbered cleanly for v2 (§6.1); no gaps
  held for v1.

## 3. Target repository layout

```
helic-core/                 # unchanged role: no_std DSP, host-tested
helic-proto/                # protocol v2 (§6)
helic-drivers/              # + pwm_out.rs, ssi.rs (§7.2, §7.3)
host/                     # protocol v2 client + helic_daq.sim (§6.3, §6.6)
firmware/                 # cargo workspace (unchanged .cargo/config.toml)
  Cargo.toml              # members = ["common", "experiments/*"]
  common/                 # NEW: lib crate `helic-fw-common`
    src/lib.rs
    src/rig.rs            # Rig + TickSource traits, MAX_SOURCES, tick impls
    src/rt_loop.rs        # generic run_rt_loop<R: Rig>, queues, diagnostics
    src/table.rs          # waveform double-buffer + swap protocol (§6.4)
    src/params.rs         # ParamRegistry trait, base diagnostics params
    src/comms/mod.rs      # STREAM state, W5500/net bring-up, DHCP/static
    src/comms/tcp.rs      # control server over &mut dyn ParamRegistry
    src/comms/udp.rs      # stream task (concrete — Record is non-generic)
    src/comms/beacon.rs   # UDP discovery responder (§6.5)
    src/laser.rs          # optoNCDT reader as a generic async fn
    src/ssi_pio.rs        # PIO SSI master (§7.3)
  experiments/
    cbc-rig/              # the current instrument, moved (§5, phase 1)
      src/main.rs         # task wrappers, interrupt bindings, spawns
      src/board.rs        # pin map (today's board.rs)
      src/config.rs       # SampleRate, controller alias, net config, sources
    sig-gen/              # D4: DAC out, laser in, NO ADC (PWM-wrap tick)
    pwm-rig/              # D4: PWM output stage instead of the AD5064
    encoder-rig/          # D4/D5: cbc-rig + RMB20 SSI encoder input
```

Crate names: `helic-fw-common`, `fw-cbc-rig`, `fw-sig-gen`, `fw-pwm-rig`,
`fw-encoder-rig`. Each experiment crate should stay in the
~300–600-line range; anything bigger belongs in `common` or a shared
crate.

**What lives where** (the existing AGENTS.md rule extended): logic →
`helic-core`/`helic-proto`/`helic-drivers` (host-tested); RP2350-specific but
experiment-independent plumbing → `common`; pins, constants, task
wrappers, interrupt bindings, and the `Rig` impl → the experiment
crate.

## 4. Core abstractions (common crate)

### 4.1 `TickSource`

```rust
/// Paces the RT loop: resolves once per sample instant.
pub trait TickSource {
    async fn wait(&mut self);
}
```

Two implementations:

- **`BusyEdgeTick`** — current behaviour extracted verbatim: owns the
  BUSY `Input`, waits for the falling edge with the existing 2×period
  timeout, increments the `TICK_TIMEOUTS` atomic on timeout. Used by
  experiments with a conversion-complete signal.
- **`PwmWrapTick`** — owns a free-running PWM slice configured from
  `SampleRate::pwm_params()` and resolves on the slice's wrap
  (overflow) event. Crystal-exact instants, identical presets.
  - Use `embassy_rp::pwm::Pwm::new_free` (no output pin needed) when
    the tick is not also CONVST.
  - embassy-rp 0.10 exposes only a blocking `Pwm::wait_for_wrap()`, so the
    implementation uses the fallback (§9 R1):
    enable `PWM_IRQ_WRAP_0` for the slice, a handler that clears the
    slice's `INTR` bit and wakes an `AtomicWaker`; the interrupt
    binding itself lives in the experiment's `main.rs` alongside the
    existing `bind_interrupts!`.
  - In `cbc-rig`, CONVST generation and `BusyEdgeTick` coexist exactly
    as today (PWM drives CONVST; BUSY paces the loop). `PwmWrapTick`
    is for rigs with no data-ready signal.

### 4.2 `Rig` — the per-experiment contract

```rust
pub const MAX_SOURCES: usize = 24;

/// Everything hardware- or experiment-specific inside one tick.
/// Constructed on core 1 (same rule as today's `AnalogParts::build`).
pub trait Rig {
    /// (name, unit) of the experiment's *input* sources, slot order =
    /// stream id. The loop appends the controller's TELEMETRY, then
    /// "target", "forcing", "table", "out" — total must be
    /// <= MAX_SOURCES (compile-time assert in the table-assembly macro).
    const INPUTS: &'static [(&'static str, &'static str)];

    type Tick: TickSource;
    type Ctrl: helic_core::controller::Controller;

    /// One-time hardware init (ADC ranges, DAC zeroing, PIO start...).
    fn init(&mut self);
    /// Read all inputs for this tick into values[..INPUTS.len()].
    fn measure(&mut self, values: &mut [f32]);
    /// Drive the physical output stage with `out` volts.
    fn actuate(&mut self, out: f32);

    /// Host-settable rig parameters, mirroring Controller exactly
    /// (D8). Names by convention prefixed "rig_", e.g.
    /// "rig_laser_range", "rig_encoder_zero", "rig_out_channel".
    fn param_names() -> &'static [&'static str] where Self: Sized { &[] }
    fn set_param(&mut self, _id: u16, _value: f32) {}
}
```

Rig parameter writes travel as `RtCommand::SetRigParam(u16, f32)` and
are applied at a sample boundary like everything else; the registry
shadows them exactly as it shadows controller params (§4.5).

The generic loop in `common::rt_loop`:

```rust
pub async fn run_rt_loop<R: Rig>(
    mut rig: R, mut tick: R::Tick, mut ctrl: R::Ctrl,
    sample_rate: SampleRate,
    mut commands: CommandConsumer, mut records: RecordProducer,
) -> ! { ... }
```

Body = today's loop with the hardware bits delegated: `tick.wait()` →
timing diagnostics → drain `RtCommand`s → `rig.measure(&mut values)` →
master phase step, evaluate target/forcing → step the table generator
(§6.4) → `ctrl.tick(&values[..n_inputs], target, dt)` →
`out = ctrl_out + forcing + table` → `rig.actuate(out)` →
`ctrl.telemetry(...)` into its slots → fill the four appended slots →
enqueue `Record`. All existing diagnostics atomics stay in
`common::rt_loop` (with `BUSY_TIMEOUTS` renamed `TICK_TIMEOUTS`, D7).

Each experiment's `main.rs` supplies the concrete wrapper:

```rust
#[embassy_executor::task]
async fn rt_task(parts: board::AnalogParts, commands: CommandConsumer,
                 records: RecordProducer) -> ! {
    let (rig, tick) = parts.build();          // on core 1, as today
    common::rt_loop::run_rt_loop(rig, tick, config::make_controller(),
                                 config::SAMPLE_RATE, commands, records).await
}
```

### 4.3 `Record` becomes shape-agnostic

```rust
pub struct Record {
    pub index: u32,
    pub n: u8,                      // constant per build (full table len)
    pub values: [f32; MAX_SOURCES],
}
```

`RtCommand`, the queues, and the UDP stream task then need **no**
generics and stay concrete in `common`. The stream task's
`record_value()` lookup is replaced by direct slot indexing
(`r.values[src as usize]`), validated at `StreamSetup` time against the
source-table length (§6).

### 4.4 `Controller` trait changes (helic-core, breaking)

`Measurements` is deleted. The measurement bus **is** the input-slot
slice, and controllers gain telemetry (D8):

```rust
pub trait Controller {
    fn tick(&mut self, inputs: &[f32], reference: f32, dt: f32) -> f32;
    fn reset(&mut self) {}
    fn param_names() -> &'static [&'static str] where Self: Sized { &[] }
    fn set_param(&mut self, id: u16, value: f32) {}

    /// (name, unit) of per-tick internal signals to expose as stream
    /// sources, e.g. [("error", "V")]. Filled after tick().
    const TELEMETRY: &'static [(&'static str, &'static str)] = &[];
    fn telemetry(&self, _out: &mut [f32]) {}
}
```

`PidController.feedback` becomes a plain `usize` slot index (the
`Option<usize>`/laser special case disappears — the laser is just a
named slot), exposed as a settable `ctrl_feedback` parameter (document
the f32→usize truncation). Its `TELEMETRY` is `[("error", "V")]` — in
CBC the error signal is what you watch for noninvasiveness and
convergence, so this must stream without hacks. `ADC_CHANNELS` moves
out of helic-core (it is an AD7609 property; it belongs to the
experiment's `INPUTS`). Update the tests in `controller.rs`; the
plant-simulation test carries over with `inputs: &[f32]`.

### 4.5 Parameter registry decomposition

Today `ParamStore` hard-codes `BASE_PARAMS` (incl. `laser`) +
controller params. New structure in `common::params`:

- An object-safe trait so the TCP task stays concrete:

  ```rust
  pub trait ParamRegistry {
      fn count(&self) -> usize;
      fn def(&self, index: usize) -> Option<ParamDef>;
      fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode>;
      fn set(&mut self, index: usize, data: &[u8]) -> Result<(), ErrorCode>;
      /// Staged writes for long arrays (§6.4).
      fn set_block(&mut self, index: usize, offset: u32, data: &[u8])
          -> Result<(), ErrorCode>;
      fn commit(&mut self, index: usize, len: u32) -> Result<(), ErrorCode>;
  }
  ```

  `common::comms::tcp` exposes
  `pub async fn control_serve(stack, store: &'static mut dyn ParamRegistry, sources: &'static [(&'static str, &'static str)]) -> !`
  (or an equivalent that keeps tcp.rs free of experiment types —
  implementer's call on the exact handoff, see §9 R6).

- A generic store the experiments instantiate:

  ```rust
  pub struct Store<C: Controller, R: Rig> {
      /* today's ParamStore fields + rig shadow + table staging handle */
      extras: &'static [ExtraParam],
  }
  pub struct ExtraParam {          // read-only, atomic-backed values
      pub def: ParamDef,           // e.g. "laser" f32×1 ro
      pub get: fn(&mut [u8]),
  }
  ```

Registry order: base diagnostics → platform writables → extras (ro) →
rig params (rw) → controller params (rw). The `laser` parameter moves
from `BASE_PARAMS` into cbc-rig's `extras`; `encoder-rig` adds
`encoder` and `encoder_errors` extras. Hosts discover by name, never
index, so ordering is free to change later.

Base registry (v2 names): `firmware` (now includes the git hash,
§6.5), `experiment` (c×16, from the experiment's `config.rs`, D9),
`sample_freq`, `ticks`, `loop_time_last`, `loop_time_max`,
`clock_jitter`, `overruns`, `tick_timeouts` (renamed, D7),
`records_dropped`. Platform writables: `freq`, `target_coeffs`,
`forcing_coeffs`, `ctrl_reset`, plus the table controls (§6.4):
`table` (f32×MAX_TABLE_LEN, write via SetBlock/Commit only),
`table_len` (ro), `table_freq`, `table_gain`, `table_mode`,
`table_trigger`.

Note `GetPar` on `table` fails naturally with `BadLength` (its wire
size exceeds `MAX_PAYLOAD`); the host keeps its own copy. No `GetBlock`
readback message.

### 4.6 Optional sensor tasks

`common::laser` keeps the optoNCDT logic as
`pub async fn laser_run(rx: UartRx<'static, Async>, range_mm: f32, dest: &'static AtomicU32) -> !`
(today's `laser_task` body). Experiments that have the sensor define
the task wrapper, the `bind_interrupts!` entry, own the `AtomicU32`,
list `("laser", "mm")` in `INPUTS`, read the atomic in `Rig::measure`,
and add the `ExtraParam`. Experiments without it do none of that — no
UART claim, no pull-up requirement, no dead slot. The GP1 pull-up
warning comment moves to cbc-rig's `main.rs`. (§9 R4 covers the
`UartRx` type-genericity check.) With `rig_laser_range` as a rig
parameter (D8), the sensor range is host-settable rather than a
compile-time constant; the atomic-published value is raw and scaling
happens in `measure` — implementer picks the split, but the range must
be changeable without reflashing.

## 5. Phased implementation

Each phase leaves the repo green (`cargo test` at root, firmware
workspace `cargo build --release` + `clippy -D warnings` + `fmt
--check`, `host` unittests) and is one commit series per the AGENTS.md
convention.

### Phase 1 — workspace restructure (no behaviour change)

The firmware workspace and `fw-cbc-rig` package now exist. Shared
communications, parameters, laser parsing, queues, records, diagnostics and
`SampleRate` presets live in `helic-fw-common`; main, board and configuration
remain in `cbc-rig`. The concrete loop body remains beside the rig until the
`Rig` extraction in phase 2, avoiding a temporary common→experiment
dependency. The workspace mechanics include the workspace `Cargo.toml`,
`[workspace.dependencies]` for the embassy pins, per-crate
`memory.x`/build.rs handling, `.cargo/config.toml` runner still works,
`cargo run --release -p fw-cbc-rig` flashes.
**Accept:** binary behaves identically on hardware; CI green.

### Phase 2 — the abstractions

1. helic-core: `Controller` trait change incl. telemetry (§4.4), delete
   `Measurements`, `PidController` slot index + `error` telemetry,
   tests updated.
2. common: `TickSource` + `BusyEdgeTick` + `PwmWrapTick` (§4.1),
   `Rig` trait incl. rig params (§4.2), `Record` new shape (§4.3),
   generic `run_rt_loop` (§4.2), `RtCommand::SetRigParam`, params
   decomposition (§4.5), laser as fn (§4.6), `tick_timeouts` rename.
3. cbc-rig: implement `Rig` for the AD7609/AD5064/laser hardware
   (mostly moving `AnalogParts::build` + the measure/actuate lines out
   of the old loop), task wrappers, extras, `rig_laser_range` +
   `rig_out_channel` rig params.

**Accept:** cbc-rig on hardware shows the same 1 Hz status line,
identical loop times (compare `loop_time_max` before/after — the
indirection must be monomorphised away; a few µs regression budget, no
more), all host tests pass modulo renamed params.

### Phase 3 — protocol v2 (§6.1–6.3)

helic-proto, firmware common (tcp/udp), docs/protocol.md, host Python,
KAT vectors, in one coordinated change: renumbered message types,
merged `GetParams`/`GetSources` with units, `Status` v2, `experiment`
param. (`SetBlock`/`Commit` codecs land here too, but the firmware
behind them lands in phase 5.)
**Accept:** round-trip KATs in both Rust and Python for `GetParams`
and `GetSources`; `helic-daq capture --sources adc0,out` works against
cbc-rig (names resolve via discovery); requesting an unknown name
gives a local error listing discovered names with units.

### Phase 4 — host simulator (D10, §6.6)

Promote the test fake to `helic_daq.sim`. Do this **before** the
waveform work so its host-side upload logic develops against the sim.
**Accept:** `python -m helic_daq.sim` serves v2 on localhost; the
existing device/stream unittests run against it; `helic-daq capture`
works against it end to end.

### Phase 5 — arbitrary waveform (D6, §6.4)

helic-core `WaveTable` + tests; common double-buffer + swap protocol +
`RtCommand::UseTable`; registry table params; host
`upload_table(values, duration=..)` helper + CLI subcommand.
**Accept:** interpolation unit tests (see §6.4); upload a 1024-point
table from Python and see it on a scope spread over the commanded
duration; a re-upload mid-playback switches cleanly at a sample
boundary (no glitch, old table plays until the commit applies);
non-finite samples rejected at commit; one-shot mode plays once and
returns to 0 V contribution; in locked-loop mode the table stays
phase-coherent with `target` over a minutes-long capture (fixed
relative phase, e.g. scope XY or a long stream capture); a locked
one-shot begins exactly at a master period boundary.

### Phase 6 — `sig-gen` experiment (D4a)

No ADC, no AD7609 pins. `PwmWrapTick` paces the loop; inputs =
`[("laser", "mm")]`; output = AD5064 as today. With the waveform
generator (phase 5) this is a complete arbitrary-function generator +
displacement logger. `tick_timeouts` must stay 0 with no ADC fitted.
**Accept:** on hardware with no analog board attached: 8000 ticks/s
steady, jitter comparable to cbc-rig, DAC outputs a commanded Fourier
series *and* an uploaded table verified on a scope.

### Phase 7 — PWM output (D4b)

`helic-drivers/src/pwm_out.rs` (§7.2), then `pwm-rig` = sig-gen with the
output stage swapped to a PWM slice + pin.
**Accept:** driver unit tests on host (duty↔volts mapping, clamping);
on hardware, commanded sine appears after an external RC filter.

### Phase 8 — SSI encoder (D4c/D5)

`helic-drivers/src/ssi.rs` decode (§7.3.1), `common::ssi_pio` reader
(§7.3.2), then `encoder-rig` = cbc-rig + `("encoder", "rev")` input
slot + `rig_encoder_zero` rig param.
**Accept:** decode unit tests (Gray KAT vectors, error frames); on
hardware, slow manual shaft rotation streams a monotone position and
the value survives at 8 kHz with zero tick overruns.

### Phase 9 — identity & network conveniences (D9, §6.5)

Git hash in the firmware string (build.rs), UDP discovery beacon +
`helic-daq find`, DHCP as a `config.rs` choice (default static).
**Accept:** `helic-daq find` lists a board with experiment name,
firmware+hash, and address; a DHCP build acquires a lease and is still
found by the beacon.

### Phase 10 — CI + docs

- CI firmware job: `cargo build --release --workspace` + clippy/fmt at
  the firmware workspace root now covers every experiment; no matrix
  needed. Confirm cache keys still make sense.
- developer_guide.md: new section **"Adding an experiment"** — copy an
  experiment crate, edit `board.rs`/`config.rs`, implement `Rig`, done;
  plus updates for TickSource/Rig/telemetry/params/table.
- user_guide.md: per-experiment flashing (`-p fw-<name>`), `helic-daq
  find`, source discovery, table upload, the simulator.
- protocol.md: rewritten for v2 (done in phase 3; cross-check here).
- AGENTS.md: layout table and conventions updated; add this file to
  the docs list.

## 6. Protocol v2 specification

`VERSION` 1 → 2 (both the TCP `Status` field and the UDP stream-header
byte). Frame layout, CRC, ports, and error codes are unchanged from v1
(§ "Frame layout" of docs/protocol.md); everything below is the v2
message set. docs/protocol.md is rewritten accordingly, including new
known-answer vectors for `GetParams`, `GetSources`, `SetBlock`,
`Commit`, the 12-byte `Status`, and a beacon exchange.

### 6.1 Message types (renumbered — D7)

| type | name | request payload | response payload |
|---|---|---|---|
| 1 | GetParams | — | per parameter: name NUL-terminated, `type u8, count u16, writable u8` |
| 2 | GetSources | — | per source: name NUL-terminated, unit NUL-terminated |
| 3 | GetPar | `index u16` × n | raw values concatenated |
| 4 | SetPar | `index u16`, raw value | — |
| 5 | SetBlock | `index u16, offset u32, data…` | — |
| 6 | Commit | `index u16, len u32` | — |
| 7 | StreamSetup | `decimation u16, count u32, n u8, source u8 × n` | — |
| 8 | StreamStart | `port u16` | — |
| 9 | StreamStop | — | — |
| 10 | Status | — | `version u8, n_params u16, n_sources u8, sample_rate f32, uptime_ms u32` |
| 255 | Error | — | `error_code u8, request_type u8` |

Notes:

- `GetParams` merges v1's GetParNames/GetParInfo (one connect
  round-trip, no names/info skew). ~20 params × ~16 B fits MAX_PAYLOAD
  (1024) comfortably; keep names ≤ 15 bytes. Firmware limits active
  discovery tables to 75% of the payload, preserving growth headroom.
- `GetSources`: source id = position in the list. Ids `0..n_inputs`
  are the experiment's inputs, then controller telemetry, then always
  `target`, `forcing`, `table`, `out` (all unit "V"). Names ≤ 15
  bytes, units ≤ 7 bytes, ASCII, names unique. Worst case 24 × 24 B
  fits MAX_PAYLOAD.
- `StreamSetup` validation: `source < n_sources` (table length).
- The `helic_proto::source` module is **deleted**; helic-proto keeps
  layout/codec logic only. cbc-rig's table starts `adc0..adc7, laser`
  so the familiar ordering survives, but nothing depends on it.

### 6.2 Firmware side

`common::comms` receives the assembled `(name, unit)` table as
`&'static [(&'static str, &'static str)]`, built by a helper/macro in
`common::rig` from `Rig::INPUTS` ++ `Ctrl::TELEMETRY` ++ the four
generator slots, with the compile-time `MAX_SOURCES` assert. tcp.rs
serves types 1/2 from the registry and this table.

### 6.3 Host side

`protocol.py`: delete `SOURCES`/`SOURCE_NAMES`; codecs for the v2
message set; accept only version 2 (reject others with a clear
"protocol version mismatch" error). `device.py`: fetch and cache both
tables at connect; `stream_setup` resolves names against the source
table; add `upload_table` (§6.4) and `find` (§6.5). `cli.py`: same UX;
error messages list discovered names; new `sources`, `upload`, `find`
subcommands. Tests mirror the Rust KATs; the fake device becomes the
simulator (§6.6).

### 6.4 Arbitrary waveform (D6)

**Generator** (`helic-core/src/table.rs`, host-tested):

```rust
pub const MAX_TABLE_LEN: usize = 4096;

pub struct WaveTable {
    values: [f32; MAX_TABLE_LEN],
    len: u16,                       // 2..=MAX_TABLE_LEN
}
impl WaveTable {
    /// theta = 32-bit phase (one full period = 2^32, as the existing
    /// PhaseAccumulator). Fixed-point index math, no f64:
    /// pos = theta * len / 2^32; linear interpolation between
    /// values[pos] and values[(pos+1) % len].
    pub fn evaluate(&self, theta: u32) -> f32;
}
```

Unit tests: exact recovery of a ramp between knots; wrap continuity
(last→first segment); len = 2 edge; non-power-of-two lengths; the
fixed-point index never exceeds `len - 1`.

**Playback** (common, in the RT loop). The table contribution is
`table_out = table_gain * table.evaluate(theta_table)`, and the final
output is `out = controller + forcing + table_out`; `table_out` is
streamed as the `table` source slot. Where `theta_table` comes from
depends on the mode:

- **Free-running** modes (1 loop, 2 one-shot): the table has its
  **own** `PhaseAccumulator` — a table spread over duration `T` is
  `table_freq = 1/T`, fully independent of the fundamental. In
  one-shot, playback starts on trigger, plays exactly one pass
  (detect wrap via the accumulator's period-start flag), then
  contributes 0. Free-running is crystal-timed but **not**
  phase-locked to the Fourier generators (increment rounding drifts
  the relative phase).
- **Phase-locked** modes (3 locked loop, 4 locked one-shot):
  `theta_table = theta_master.wrapping_mul(table_mult).wrapping_add(phase_off)`
  where `theta_master` is the master accumulator the target/forcing
  generators already share, `table_mult` is an integer ≥ 1, and
  `phase_off = table_phase * 2^32`. Because u32 wrapping
  multiplication by an integer is an *exact* frequency multiple, the
  table plays `table_mult` passes per fundamental period with **zero
  relative drift by construction** — no PLL, no accumulated error.
  `table_freq` is ignored while locked. Locked one-shot:
  `table_trigger` arms playback, which begins at the **next master
  period start** (the `period_start` flag `PhaseAccumulator::step`
  already returns), plays one pass at the locked rate, then
  contributes 0 — giving stroboscopically repeatable transients for
  averaging. Sub-harmonic lock (table slower than the fundamental) is
  **not** offered: phase *division* does not wrap exactly; use a
  free-running mode instead (document this in the user guide).

Registry parameters (§4.5): `table_freq` (f32 Hz, free-running rate,
same validation range as `freq`), `table_gain` (f32, default 1.0),
`table_mode` (u32: 0 off, 1 loop, 2 one-shot, 3 locked loop, 4 locked
one-shot), `table_mult` (u32 ≥ 1, locked modes), `table_phase` (f32 in
[0, 1), phase offset in locked modes), `table_trigger` (u32, write
non-zero to arm/start a one-shot), `table_len` (u16 ro).

**Transport — SetBlock/Commit** (this is what v1 reserved types 5/6
for; a 1024-point table is 4 KB against a 1024 B payload, so staged
chunks + atomic activation, keeping frames small and the swap
tear-free):

- `SetBlock { index, offset, data }`: index must name the `table`
  parameter (BadIndex otherwise, until another block param exists);
  offset is the **element** offset; data length must be a multiple of
  4; offset + n elements ≤ MAX_TABLE_LEN else BadLength. Writes into
  the *staging* buffer. Rejected with Busy while a commit is awaiting
  pickup (below).
- `Commit { index, len }`: 2 ≤ len ≤ MAX_TABLE_LEN else BadValue; the
  staged `values[..len]` are scanned and non-finite entries rejected
  with BadValue (same policy as coefficients); then the swap command
  is enqueued.

**Cross-core double buffer** (`common::table`): two static buffers.
Core 0 (registry) writes only the staging buffer; core 1 reads only
the active one. `Commit` enqueues `RtCommand::UseTable { buf: u8,
len: u16 }`; core 1 switches its table pointer at the sample boundary
and publishes the now-active buffer id to an `ACTIVE_TABLE` atomic
(release ordering; core 0 reads with acquire). Until `ACTIVE_TABLE`
matches the committed id, further `SetBlock`/`Commit` return Busy —
so core 0 provably never writes a buffer core 1 might read. Single
TCP client + strict request/response keeps this simple. This safety
argument must appear as a comment on the module; it is the one new
piece of unsafe-adjacent cross-core code in the plan (§9 R8).

**Host convenience**: `Device.upload_table(values, duration=None,
freq=None, gain=1.0, mode="loop")` — chunks SetBlock frames (254
floats each; a 1024-point upload is 5 frames), commits, sets
`table_freq` from `duration` if given, polls Busy with a short retry.
CLI: `helic-daq upload wave.npy --duration 2.0`.

### 6.5 Identity, discovery, DHCP (D9)

- **Identity**: `experiment` c×16 ro param from `config.rs`. The
  `firmware` string becomes `"helic-daq <version> <githash>"` via a
  `build.rs` `cargo:rustc-env` from `git describe --always --dirty`,
  falling back to `"unknown"` when git is unavailable (§9 R10).
- **Beacon** (`common::comms::beacon`, UDP port **2352**): request =
  magic `0x4C48` (little-endian ASCII `HL`), followed by `0x01`;
  response = `magic, 0x02, version u8,
  control_port u16, mac 6B, experiment c16, firmware c16`. Responder
  answers broadcasts; host `helic-daq find` broadcasts on all
  interfaces, collects for ~1 s, prints a table. Document in
  protocol.md alongside the other two ports.
- **DHCP**: `config.rs` selects
  `NetConfig::Static { addr, prefix, .. }` or `NetConfig::Dhcp`
  (embassy-net `dhcpv4` feature); default static, per-experiment
  distinct defaults so two boards can share a bench network. Flash-
  stored config remains out of scope (§10) — DHCP + beacon covers the
  multi-board pain without it.

### 6.6 Host simulator (D10)

`host/helic_daq/sim.py`, runnable as `python -m helic_daq.sim [--port ..]`:
serves the full v2 control protocol and UDP streaming on localhost
with a configurable param/source table (default mimics cbc-rig) and
synthetic data (sum of commanded generators + noise, so captures look
plausible). It honours SetBlock/Commit and plays uploaded tables. The
existing unittest fake is **replaced by** the simulator (tests import
it) so the two cannot drift. Beacon responder included so `find`
works locally.

## 7. New drivers

### 7.2 PWM analog output (`helic-drivers/src/pwm_out.rs`)

Generic over `embedded_hal::pwm::SetDutyCycle` (host-testable with a
mock; embassy-rp's `PwmOutput` — from `Pwm::split()` — implements it,
§9 R2):

```rust
pub struct PwmOut<P: SetDutyCycle, const N: usize> {
    ch: [P; N],
    v_min: f32, v_max: f32,       // duty 0 ↔ v_min, max_duty ↔ v_max
}
impl<...> AnalogOut<N> for PwmOut<...> { /* code u16 → scaled duty */ }
impl PwmOut { pub fn write_volts(&mut self, ch: usize, v: f32) { /* clamp, map */ } }
```

Mirror the AD5064 driver's shape (`write_volts`, clamping semantics)
so `Rig::actuate` implementations look identical either way. Carrier
frequency is set by the experiment's PWM config in `board.rs`; document
the resolution trade-off (carrier × steps = 150 MHz: e.g. top=1023 →
~146 kHz carrier, 10-bit; top=2047 → ~73 kHz, 11-bit) and that
smoothing (RC/active filter) is external hardware's job. Unit tests:
volts→duty mapping at rails/midpoint, clamping, per-channel
independence.

### 7.3 SSI encoder (RLS RMB20 via RS422→TTL, PIO)

Two GPIOs on the experiment side of the RS422↔TTL converters: SSI
clock out (TTL → RS422 driver) and data in (RS422 receiver → TTL).

#### 7.3.1 Decode logic (`helic-drivers/src/ssi.rs`, host-tested)

```rust
pub struct SsiFormat { pub bits: u8, pub gray: bool }
impl SsiFormat {
    /// Raw shifted-in word (MSB-first, right-aligned) → binary counts.
    /// Err on the disconnected-line signatures (all-0s, all-1s).
    pub fn decode(&self, raw: u32) -> Result<u32, SsiError>;
}
pub struct SsiScale { pub counts_per_rev: u32 }  // → position in revs (f32)
```

Gray→binary is the usual xor-fold. Unit tests: known Gray vectors,
both `bits` = 12 and 13, all-0/all-1 rejection, wrap behaviour.
Position is reported in revolutions (source unit "rev");
`rig_encoder_zero` (a rig parameter, D8) is subtracted so the shaft
can be re-zeroed from the host.

RMB20 parameters to **verify against the datasheet of the ordered
part** before finalising constants (§9 R3): resolution (12/13-bit SSI
variants exist), Gray vs binary output option, max SSI clock
(spec ~1 MHz; drive at ≤ 500 kHz initially), monoflop/timeout time
t_m (~20 µs class) that must elapse between frames.

#### 7.3.2 PIO reader (`common::ssi_pio`, RP2350-specific)

A PIO state machine acting as SSI master: on trigger, emit `bits + 1`
clock cycles (side-set on the clock pin), sample DIN one PIO cycle
before each rising clock edge per SSI convention (data changes on
falling edges; verify edge phase on a scope), push the assembled word
to the RX FIFO, idle with clock high. PIO clock divider set for the
chosen SSI bit rate.

RT integration (jitter-free by construction): `Rig::measure` in
encoder-rig *kicks* a read (TX FIFO push) at tick *n* and consumes the
word kicked at tick *n−1* from the RX FIFO (non-blocking pull — a word
is always ready since the frame takes ~30 µs against a ≥125 µs
period). Encoder data is therefore exactly one sample old, constant —
the same staleness contract the laser already has; comment it where
the kick happens. Monoflop recovery is automatic (inter-tick gap ≫
t_m). On decode error, hold the last good value and increment an
`encoder_errors` diagnostic atomic (exposed as an `ExtraParam`).

Host-testability split: everything after "raw u32" is in helic-drivers
under test; the PIO program is thin enough to verify on hardware with
a scope (add the checklist to notes.md when bringing it up).

## 8. Experiment crate reference

| | tick | inputs (source table prefix) | output | rig params | notes |
|---|---|---|---|---|---|
| `cbc-rig` | `BusyEdgeTick` (CONVST PWM as today) | adc0–7, laser | AD5064 | `rig_laser_range`, `rig_out_channel` | behaviourally = current firmware + table generator |
| `sig-gen` | `PwmWrapTick` | laser | AD5064 | `rig_laser_range`, `rig_out_channel` | no AD7609 pins claimed; GP2–8/13 free; AWG duty |
| `pwm-rig` | `PwmWrapTick` | laser | `PwmOut` (slice ≠ tick slice) | `rig_laser_range` | RC filter external |
| `encoder-rig` | `BusyEdgeTick` | adc0–7, laser, encoder | AD5064 | + `rig_encoder_zero` | PIO0 SM0 for SSI; 2 pins from the free set |

Every table additionally carries the controller's telemetry (e.g.
`error` for the PID) and the appended `target`, `forcing`, `table`,
`out`. Each crate's `config.rs` keeps the compile-time knobs
(SAMPLE_RATE, OUTPUT_CHANNEL default, experiment name, `NetConfig`
with distinct static defaults per experiment).

## 9. Performance envelope, risks, and verification items

### 9.1 Performance envelope

Everything in this plan sits well inside the RP2350's budget; the
numbers below are the argument, and should be re-checked against the
`loop_time_max` parameter as phases land.

**Core 1, per tick at the 8 kHz preset (125 µs = 18,750 cycles @
150 MHz), worst-case configuration (encoder-rig, table locked, PID):**

| item | cost | notes |
|---|---|---|
| ADC frame read | ~12 µs | bus-bound: 18 B on blocking SPI @ 12 MHz |
| 2× Fourier eval, 16 harmonics | ~11 µs | ~0.8k cycles/series (AGENTS.md figure) |
| Table step + lerp | < 0.5 µs | one u32 mul + one interpolation |
| PID + telemetry fill | < 1 µs | |
| DAC write | ~2 µs | 32 b on blocking SPI @ 16 MHz |
| Encoder FIFO kick/pull | < 0.5 µs | conversion runs in PIO in the background |
| Record copy + enqueue, command drain, bookkeeping | ~1–2 µs | ~104 B copy |
| **Total** | **~28–30 µs** | **~25 % of the 125 µs budget** |

PWM output rigs are *cheaper* (a compare-register write replaces the
2 µs DAC SPI transaction); no-ADC rigs save the 12 µs read. Headroom
statement: the design only begins to strain around ~4× the current
maximum preset (≥ 32 kHz), where the blocking ADC SPI read and the
Fourier evaluation would together dominate the period — irrelevant to
the preset list this plan keeps.

**Core 0 — streaming is the tightest path in the whole system.**
Absolute worst case (all 24 sources, no decimation, 8 kHz) is
24 × 4 B × 8000 = **768 KB/s** payload ≈ 560 packets/s, which must
traverse the W5500 on SPI0; the W5500 tops out well above this
(≥ 2.5 MB/s raw even at a conservative 20 MHz SPI clock), but this is
the one figure that deserves an actual hardware measurement (R11).
Typical sessions (4–8 sources) are 128–256 KB/s — comfortable.
Everything else on core 0 (TCP request/response, beacon, laser UART
at 921.6 kbaud) is noise by comparison.

**Memory:** new structures total ~60 KB (26 KB record ring, 32 KB
table double-buffer, staging/shadows) on top of existing stacks and
network buffers — under 25 % of the 520 KB SRAM. Flash: each
experiment binary is a few hundred KB against ≥ 2 MB on the board;
per-binary size is what matters since experiments compile
independently (R5).

**Phase-lock cost:** zero beyond the table itself — locked mode is
one `wrapping_mul` + `wrapping_add` on a value already computed,
and its exactness is arithmetic (u32 wraparound), not control-loop
convergence.

### 9.2 Risks / items to verify at implementation time

| # | Risk | Mitigation |
|---|---|---|
| R1 | embassy-rp 0.10 may not expose an async PWM wrap wait (`Pwm::wait_for_wrap`). | **Resolved:** 0.10 has `Pwm::new_free` but only a blocking wrap wait; `PwmWrapTick` uses `PWM_IRQ_WRAP_0` plus an `AtomicWaker`. |
| R2 | `SetDutyCycle` may be implemented on `PwmOutput` (via `Pwm::split()`) rather than `Pwm` itself, or not at all, in the pinned version. | If absent, a ~20-line adapter in `common` implementing `SetDutyCycle` over compare-register writes; the helic-drivers driver is unaffected either way. |
| R3 | RMB20 datasheet specifics (bit count, Gray/binary, clock max, monoflop t_m) assumed from the SSI class, not verified. | `SsiFormat` is runtime data — get the ordered part's datasheet before hardware bring-up; only constants change. |
| R4 | `UartRx` genericity: if embassy-rp 0.10 still types `UartRx` by UART instance, `common::laser::laser_run` needs a `T: uart::Instance` generic (fine — it's a fn, not a task). | Check the pinned API; both forms work with the wrapper pattern. |
| R5 | Monomorphisation/flash growth from four binaries and a generic loop. | Each binary compiles independently; per-binary flash is what matters (≥ 2 MB on the board — verify the fitted flash part; binaries are a few hundred KB either way). Compare `cbc-rig` size before/after phase 2. |
| R6 | The `&'static mut dyn ParamRegistry` handoff to the TCP task needs a `StaticCell` per experiment. | Same pattern as the existing queues; if lifetimes fight, fall back to a concrete task wrapper per experiment calling a generic `control_serve`. |
| R7 | Phase-2 performance regression in the 125 µs budget from the trait indirection. | `Rig`/`TickSource` are static generics — should inline to today's code. Gate on `loop_time_max` comparison on hardware (phase 2 acceptance). |
| R8 | The waveform double buffer is the one new cross-core shared-memory structure; a mistake is a torn read in the RT path. | The Busy-until-acknowledged protocol in §6.4 (release/acquire on `ACTIVE_TABLE`) makes writer/reader exclusion explicit; keep the safety argument as a module comment and get it code-reviewed specifically. The table adds one interpolation (~tens of cycles) to the tick — negligible, but include it in the phase-5 `loop_time_max` check. |
| R9 | RAM: record ring ~26 KB + 2×16 KB table buffers + existing buffers. | ~60 KB total against 520 KB; check the map file in phase 5 anyway. |
| R10 | Git hash via build.rs breaks in environments without git (CI tarballs, vendored builds). | Fall back to `"unknown"`; never fail the build over it. |
| R11 | Full-rate streaming (24 sources × 8 kHz ≈ 768 KB/s) is the system's tightest data path (W5500 SPI throughput + core-0 CPU for smoltcp/UDP). | Should fit with ≥ 3× margin (§9.1), but measure on hardware in phase 3: stream all sources at 8 kHz, watch `records_dropped` and packet seq gaps. Decimation is the built-in escape valve. |

## 10. Explicitly out of scope

- Flash-stored configuration (IP, MAC, sample rate) — DHCP + the
  discovery beacon (§6.5) cover the multi-board pain; a flash config
  block remains a future milestone.
- Per-experiment harmonic counts (fixed at 16 in common).
- Quadrature (incremental) encoder support — SSI absolute only (D5).
- Multi-output control: the loop has one control output; a second DAC
  channel at a static level is expressible as a rig parameter if
  needed. Per-output targets/forcing would ripple through the
  generator/phase design for a use case CBC doesn't currently have.
- Sub-harmonic phase lock for the waveform table (exact phase
  *division* does not wrap; free-running mode covers the need) —
  integer-multiple lock is in scope, §6.4.
- AD7606B / AD5764 part swaps (kept possible by the existing traits;
  unchanged by this plan).
