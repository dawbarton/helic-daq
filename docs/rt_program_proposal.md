# Real-time programme ownership proposal

Status: proposed, not implemented.

This proposal moves experiment-level signal generation, controller ownership
and output mixing out of the common real-time loop and into a statically
selected, host-testable real-time programme. The first and only programme to
implement is `StandardProgram`; it must reproduce the current behaviour and
host interface exactly. The abstraction should leave a bounded route to later
MIMO programmes without adding runtime dispatch or returning experiment policy
to `firmware/common`.

The proposal is deliberately an ownership and extension-boundary refactor. It
does not propose a runtime graph, a new wire protocol, independent generator
banks for every actuator, or a MIMO controller now.

## Motivation

The current `RtLoopState` in `firmware/common/src/rt_loop.rs` owns both
platform machinery and the particular signal graph used by every experiment:

- the hardware `Rig`, `TickSource`, queues and timing state;
- the active controller;
- the master `PhaseAccumulator`;
- `target_coeffs` and `forcing_coeffs`;
- the `TablePlayer` and active `WaveTable` reference; and
- the fixed sum `controller + forcing + table`.

That is simple, but it makes one experiment's control topology a property of
the common scheduler. It also fixes the common controller and actuation path to
one scalar. SISO and MISO experiments fit well, and a rig can map that scalar
onto several coupled physical channels, but a genuinely independent MIMO
experiment would require another common-loop rewrite.

The desired boundary is:

- the common loop owns hardware timing, bounded cross-core command handling,
  safety enforcement, record transport and diagnostics;
- `Rig` owns physical acquisition, physical actuation and hardware-specific
  safety facts;
- an `RtProgram` owns the experiment's portable real-time signal and control
  state; and
- core 0 retains host-facing shadow state and sends copied updates through the
  existing SPSC queue.

This retains the repository rule that reusable logic is host-testable in
`helic-core` and that experiment firmware crates remain thin.

## Goals

1. Preserve the current firmware behaviour, parameter names, source names,
   source order and sample-boundary update semantics through
   `StandardProgram`.
2. Make the master phase, Fourier coefficient owners, controllers, table
   player and signal mixing an explicit portable object.
3. Evaluate the sine and cosine harmonic basis once per sample and make that
   phase-coherent basis available to every programme component.
4. Preserve all five waveform-table modes, especially the distinction between
   free-running playback and exact locking to the master Fourier phase.
5. Keep the common tick path statically dispatched, allocation-free, bounded,
   `f32`-only and SRAM-resident.
6. Make the common programme-to-rig boundary a fixed-capacity logical output
   vector so a later MIMO programme does not require another scheduler
   redesign.
7. Keep global safety outside the programme so every logical output is clamped
   or quieted before physical actuation.
8. Allow future physical SISO and MISO experiments to require changes only in
   their experiment crate when they use `StandardProgram` and an existing
   controller.

## Non-goals

- Do not implement a second programme, a MIMO controller, a runtime graph or
  dynamic dispatch in the first change.
- Do not add independent master clocks or per-actuator Fourier/table parameter
  banks yet.
- Do not change the protocol-v3 frame codec or add compatibility aliases.
- Do not change existing base parameter names or meanings.
- Do not change waveform-table upload or double-buffer ownership.
- Do not move portable controller or generator logic into an experiment
  firmware crate.
- Do not make `Rig` responsible for controller calculation or signal mixing.

## Current ownership

There are intentionally two copies of writable real-time configuration:

1. On core 0, `ParamStore` owns host-facing shadow copies of frequency,
   Fourier coefficients and table settings. Reads are served from these
   shadows. A validated write is copied into an `RtCommand` and acknowledged
   only if it can be queued.
2. On core 1, `RtLoopState` owns the live values applied by the tick. At most
   `COMMANDS_PER_TICK` commands are applied at a sample boundary.

Moving live state into a programme does not remove the core-0 shadow. Core 0
must never read or mutate the programme directly. The programme becomes the
authoritative owner of the applied core-1 state; `ParamStore` remains the
authoritative owner of the last accepted host-facing shadow.

Waveform samples remain in the two static buffers in
`firmware/common/src/table.rs`. The programme owns only the active
`&'static WaveTable` reference and its `TablePlayer`. On a `UseTable` command,
common firmware activates the committed buffer and passes the resulting
immutable reference to the programme at the same sample boundary.

## Proposed ownership

```text
core 1: RtLoopState<R, T, P>
|
+-- tick: T
+-- rig: R
|   +-- ADC, DAC, encoder, GPIO and PIO owners
|   +-- measure(inputs)
|   +-- actuate(applied_outputs)
|   +-- physical clamps, safe values and fault detection
|
+-- program: P
|   +-- master harmonic generator
|   +-- ControlledAxis<C>
|   |   +-- controller C
|   |   +-- reference FourierSignal<H>
|   +-- forcing FourierSignal<H>
|   +-- TablePlayer
|   +-- active WaveTable reference
|   +-- logical output calculation and programme telemetry
|
+-- command consumer and record producer
+-- sample index, command epoch and timing diagnostics
```

The first concrete `P` is
`StandardProgram<C, HARMONICS>`. The three production experiments select that
same programme. No general graph builder is needed.

## Portable core types

The portable types belong in `helic-core`, with host tests. Exact names may be
adjusted during implementation, but responsibilities and boundaries should
remain as described below.

### Harmonic frame and master generator

One full turn remains represented by a wrapping `u32`. A master generator
owns one `PhaseAccumulator` and advances it exactly once per hardware tick. It
uses the existing `SinLut` to construct a frame containing the raw master
phase, the accumulator's actual overflow flag and all harmonic sine/cosine
terms:

```rust
pub struct HarmonicFrame<const H: usize> {
    pub phase: u32,
    pub period_start: bool,
    cos: [f32; H],
    sin: [f32; H],
}

pub struct HarmonicGenerator<const H: usize> {
    phase: PhaseAccumulator,
}

impl<const H: usize> HarmonicGenerator<H> {
    pub fn set_increment(&mut self, increment: u32);
    pub fn step(&mut self, lut: &SinLut) -> HarmonicFrame<H>;
}
```

For harmonic index `k` in `0..H`, the lookup phase is
`(k as u32 + 1).wrapping_mul(master_phase)`, exactly as in the existing
`FourierCoeffs::evaluate`. `period_start` must be the boolean returned by
`PhaseAccumulator::step`; it must not be reconstructed from `phase == 0`,
because a general increment need not land on zero when it wraps.

Add a projection operation which retains the existing accumulation order:

```rust
impl<const H: usize> HarmonicFrame<H> {
    pub fn project(&self, coeffs: &FourierCoeffs<H>) -> f32;
}
```

It computes:

```text
mean + sum(a[k] * cos[k] + b[k] * sin[k])
```

Generating the basis once avoids repeated LUT interpolation when the target,
forcing and future controlled axes share a phase. Keep
`FourierCoeffs::evaluate` as a public convenience and regression oracle; do
not force unrelated users to construct a frame.

The implementation must determine whether returning the fixed arrays by value
introduces compiler-generated copies. It may instead retain a frame inside the
generator and return a borrow if that produces a clearer SRAM call graph. The
choice must be based on release ELF inspection and timing, not source-level
aesthetics. Any emitted ARM EABI copy/clear helper must resolve to `rt_mem` in
SRAM.

### Fourier signal

Separate ownership of one Fourier coefficient bank from ownership of the
master phase and harmonic basis. `HarmonicGenerator` produces the shared
phase-coherent basis; `FourierSignal` owns coefficients and projects that
basis into one scalar:

```rust
pub struct FourierSignal<const H: usize> {
    coefficients: FourierCoeffs<H>,
}

impl<const H: usize> FourierSignal<H> {
    pub const fn zero() -> Self;
    pub const fn new(coefficients: FourierCoeffs<H>) -> Self;

    pub fn coefficients(&self) -> &FourierCoeffs<H>;
    pub fn set_coefficients(&mut self, coefficients: FourierCoeffs<H>);

    pub fn sample(&self, harmonics: &HarmonicFrame<H>) -> f32 {
        harmonics.project(&self.coefficients)
    }
}
```

`FourierSignal` is the common abstraction for both the controller reference
and direct Fourier forcing. It may also be reused later for another logical
actuator, an injected perturbation or another phase-locked scalar consumer.
It does not own or advance phase: every signal sampled from one
`HarmonicFrame` is exactly phase coherent. Replacing its complete coefficient
value at a sample boundary retains the current atomic update semantics.

Do not introduce a general `SignalSource` trait in the initial change.
`FourierSignal` and `TablePlayer` have materially different state and phase
semantics, and there is not yet a second reference-source implementation that
justifies a common trait. A later `ControlledAxis<C, R>` can generalise the
reference type without changing the role of `FourierSignal`.

The existing `PeriodicGenerator` combines its own phase accumulator and
coefficient bank and remains useful when a signal needs an independent clock.
Retain its public API. It may be reimplemented from `HarmonicGenerator` and
`FourierSignal` if that is behaviourally exact, but `StandardProgram` must use
the split types so its reference and forcing share one calculated basis. Do
not keep a second live coefficient copy in `PeriodicGenerator` for either
standard-programme signal.

### Controlled axis

A generic controller algorithm should not itself be coupled to Fourier
references. Compose the algorithm with its reference instead:

```rust
pub struct ControlledAxis<C, const H: usize> {
    controller: C,
    reference: FourierSignal<H>,
}

impl<C: Controller, const H: usize> ControlledAxis<C, H> {
    pub fn new(controller: C) -> Self;
    pub fn reference(&self) -> &FourierSignal<H>;
    pub fn set_reference_coefficients(&mut self, coeffs: FourierCoeffs<H>);

    pub fn step(
        &mut self,
        inputs: &[f32],
        harmonics: &HarmonicFrame<H>,
        dt: f32,
    ) -> AxisSample;

    pub fn reset(&mut self);
}

pub struct AxisSample {
    pub reference: f32,
    pub control: f32,
}
```

`step` samples the owned reference `FourierSignal` against the shared frame
and passes the resulting scalar to `Controller::tick`. Controller parameter
and telemetry methods continue to belong to `Controller`; `ControlledAxis`
and `StandardProgram` delegate to them.

This composition permits a future axis to use another reference source
without putting phase and coefficient state inside `PidController` or another
reusable control algorithm.

### Standard programme

`StandardProgram` owns the complete current signal graph:

```rust
pub struct StandardProgram<C, const H: usize> {
    harmonics: HarmonicGenerator<H>,
    axis: ControlledAxis<C, H>,
    forcing: FourierSignal<H>,
    table_player: TablePlayer,
    active_table: &'static WaveTable,
    last_target: f32,
    last_forcing: f32,
    last_table: f32,
}
```

Its per-tick calculation is, in this exact order:

1. advance `harmonics` once, producing `(master_phase, period_start)` and the
   shared harmonic basis;
2. sample the axis reference and forcing `FourierSignal` values against that
   basis;
3. run the controller with the current input slice, reference and `dt`;
4. call `TablePlayer::step(active_table, master_phase, period_start)`;
5. calculate logical output zero as `control + forcing + table`;
6. retain `target`, `forcing` and `table` for post-actuation signal emission.

The common loop subsequently applies safety and actuates the rig. Only after
actuation does it ask the programme to emit controller telemetry followed by
the retained `target`, `forcing` and `table`; it then appends the applied
output and `cmd_epoch`. This preserves both the existing stream order and the
existing sample-to-actuator critical path:

```text
rig inputs
controller telemetry
target
forcing
table
out
cmd_epoch
```

`StandardProgram` is initially the only implementation. The programme API
must not contain a branch or tag selecting a programme at runtime.

## Programme interface

The programme-to-loop interface should be bounded and output-vector capable
even though `StandardProgram` produces exactly one logical output. One viable
shape is:

```rust
pub const MAX_ACTUATORS: usize = 4;

pub trait RtProgram<const H: usize> {
    const OUTPUT_COUNT: usize;

    fn signal_count() -> usize;
    fn signal(index: usize) -> Option<(&'static str, &'static str)>;

    fn step(
        &mut self,
        inputs: &[f32],
        dt: f32,
        lut: &SinLut,
        outputs: &mut [f32],
    );

    fn write_signals(&self, signals: &mut [f32]);

    fn reset(&mut self);
    fn use_table(&mut self, table: &'static WaveTable);

    fn param_names() -> &'static [&'static str]
    where
        Self: Sized;
    fn param_value(&self, id: u16) -> Option<f32>;
    fn normalise_param(
        id: u16,
        value: f32,
        input_count: usize,
        output_count: usize,
    ) -> Option<f32>
    where
        Self: Sized;
    fn set_param(&mut self, id: u16, value: f32);
}
```

`step` must receive a slice of exactly `OUTPUT_COUNT` elements;
`write_signals` must receive a slice of exactly `signal_count()` elements.
Validate those lengths during setup and use only the prevalidated prefixes in
the hot loop. `StandardProgram::write_signals` calls controller telemetry and
then writes `last_target`, `last_forcing` and `last_table`. Keeping this call
after `Rig::actuate` avoids moving telemetry calculation onto the
measurement-to-output latency path.

Prefer explicit programme setters so the existing non-generic `RtCommand` and
static queue storage remain unchanged. For `RtProgram<const H: usize>`, the
required command-facing methods are:

```rust
fn set_master_increment(&mut self, increment: u32);
fn set_target_coeffs(&mut self, coeffs: FourierCoeffs<H>);
fn set_forcing_coeffs(&mut self, coeffs: FourierCoeffs<H>);
fn set_table_increment(&mut self, increment: u32);
fn set_table_gain(&mut self, gain: f32);
fn set_table_interpolation(&mut self, value: TableInterpolation);
fn set_table_mode(&mut self, value: TableMode);
fn set_table_multiplier(&mut self, multiplier: u32);
fn set_table_phase(&mut self, phase_offset: u32);
fn trigger_table(&mut self);
fn use_table(&mut self, table: &'static WaveTable);
fn reset(&mut self);
fn set_param(&mut self, id: u16, value: f32);
```

These methods support the existing operations without exposing firmware queue
types in `helic-core`:

- set master increment;
- atomically replace target/reference coefficients;
- atomically replace forcing coefficients;
- set table increment, gain, interpolation, mode, multiplier and phase;
- trigger the table;
- replace the active immutable table reference;
- reset the controller/programme; and
- set a scalar programme parameter.

For `StandardProgram`, the two coefficient setters delegate to
`FourierSignal::set_coefficients` on the controlled reference and forcing
signal respectively.

`RtCommand` remains the cross-core envelope. Common firmware applies at most
two commands per tick and changes the existing match arms to invoke the
corresponding programme setter. `SetRigParam` continues to target `Rig`.
Rename `SetCtrlParam` to `SetProgramParam` as part of the `ParamStore`
migration; it remains the same bounded `(u16, f32)` payload. For
`UseTable(buffer)`, common firmware must call `table::activate(buffer)` before
passing the resulting reference to `P::use_table`.

`StandardProgram::param_*` delegates to its axis controller so the existing
`ctrl_*` names, validation, initial values and update behaviour do not change.
`ctrl_reset` resets the controlled axis. A later programme may expose a
different set of programme parameters through the same discovered registry,
but parameter indices must never be hard-coded by hosts.

If Rust trait constraints make the sketched interface unnecessarily awkward,
prefer small capability traits, such as `ProgramSignals`, `ProgramParams` and
`RtProgram`, over moving programme policy back into `rt_loop.rs`. Document any
departure from this proposal in the implementation commit.

## Rig and output boundary

Make physical actuation accept a bounded slice and declare its logical
actuators:

```rust
pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];
    const ACTUATORS: &'static [(&'static str, &'static str)];

    fn measure(&mut self, values: &mut [f32]);
    fn actuate(&mut self, outputs: &[f32]);

    fn clamp_output(&self, actuator: usize, value: f32) -> f32 {
        value
    }

    fn safe_output(&self, _actuator: usize) -> f32 {
        0.0
    }
}
```

At setup, assert:

```text
P::OUTPUT_COUNT == R::ACTUATORS.len()
P::OUTPUT_COUNT <= MAX_ACTUATORS
total discovered source count <= MAX_SOURCES
```

For migration, all three production rigs declare one logical actuator named
`out`, including `whirl-rig`, whose `actuate` remains a no-op. This preserves
the current source registry and host behaviour. CBC and Pico 2W use
`outputs[0]` where they currently use the scalar.

A rig remains free to map one logical output onto several coupled physical
channels, including offsets, polarity, complementary output and an LDAC
strobe. Independent logical commands require a programme with
`OUTPUT_COUNT > 1`; the rig must not hide an additional controller inside
`actuate`.

### Safety for bounded outputs

Retain one conservative global arm/trip state initially:

- call `output_fault(inputs)` once per tick;
- any fault latches the global trip and quiets every actuator;
- disarming quiets every actuator;
- otherwise clamp every programme output using its actuator index;
- increment `SAFETY_QUIET_TICKS` once per quieted tick, not once per output;
- increment `SAFETY_CLAMP_TICKS` once if any output was clamped that tick; and
- stream every applied output after safety.

Per-actuator arming or trip state is a separate future design. Do not infer it
from the introduction of an output vector.

## Source discovery and records

Replace the current fixed generated-source assembly with:

```text
Rig::INPUTS
P::signals()             # StandardProgram: controller telemetry, target,
                         # forcing and table
Rig::ACTUATORS            # applied outputs, after safety
cmd_epoch
```

For `StandardProgram`, this produces the same names and order as today. The
fixed `Record { values: [f32; MAX_SOURCES] }` and wire stream format do not
change.

Programme and actuator source definitions must use the existing name/unit
limits and must be unique across the assembled registry. The existing encoded
registry headroom and `MAX_SOURCES == 24` checks remain. A future MIMO
programme must budget its programme telemetry and applied outputs within that
limit rather than automatically streaming every internal vector.

## Parameter registry and command shadows

Parameter discovery remains on core 0. Change `ParamStore<C, R>` to be
parameterised by the active programme, for example `ParamStore<P, R>`. It
uses `P::param_names`, `P::param_value` and `P::normalise_param` where it now
uses `Controller` hooks. `StandardProgram` delegates these calls to its
controller, preserving the registry.

The base platform parameters remain unchanged:

- `freq` controls the programme's master harmonic increment;
- `target_coeffs` atomically replaces the coefficients owned by
  `ControlledAxis::reference`;
- `forcing_coeffs` atomically replaces the coefficients owned by
  `StandardProgram::forcing`;
- `ctrl_reset` calls `P::reset`;
- all table parameters configure the programme-owned `TablePlayer`; and
- table upload still uses the common double buffer and boundary activation.

The shadow and live values must start consistently. Initially retain the
current defaults: zero frequency, zero target and forcing coefficients, table
off, table gain one, linear interpolation, multiplier one and phase offset
zero. If constructors later permit other defaults, `ParamStore::new` must seed
its shadows by querying the programme rather than duplicating constants.

No host library should require a behavioural change for `StandardProgram`.
The Python simulator should continue to model the same base parameters and
source calculation; its existing tests become compatibility tests for the
refactor.

## Mandatory table-phase semantics

Table phase behaviour is an acceptance requirement, not an implementation
detail. Keep `TablePlayer`'s public stepping contract initially:

```rust
table_player.step(active_table, master_phase, master_period_start)
```

`master_phase` and `master_period_start` must come from the same single
`HarmonicGenerator::step` used for target and forcing on that tick.

The modes must retain these exact meanings:

### `Off`

Return zero. Do not advance the free-running table phase.

### `Loop`

Advance and use `TablePlayer`'s private accumulator. Ignore master phase and
master period start. Changing `freq` must not alter free-running table phase or
frequency; only `table_freq` changes its increment.

### `OneShot`

On trigger, reset the private table accumulator and start immediately. Stop
and return zero when that accumulator wraps. Master frequency, phase and wrap
must not affect it.

### `LockedLoop`

Do not advance the private table accumulator. Derive table phase exactly as:

```text
master_phase.wrapping_mul(table_mult).wrapping_add(table_phase_offset)
```

This retains exact integer-multiple phase lock and zero relative drift.

### `LockedOneShot`

On trigger, arm but do not start. Start only on the next true
`master_period_start` from the master accumulator overflow. While running,
derive the visible table phase from the same wrapping multiply and offset as
`LockedLoop`. Use the existing `previous_master` and 64-bit
`locked_progress` method to stop after one multiplied table cycle. Do not use
`master_phase == 0` as a boundary test.

Preserve these additional behaviours:

- applying commands happens before the master phase step, as it does now;
- a `freq` update changes the next increment without resetting master phase;
- `table_freq` affects only free-running modes;
- changing interpolation does not reset either phase;
- changing mode resets one-shot state as `TablePlayer::set_mode` does now;
- changing multiplier or phase offset does not reset playback state;
- activating a committed table before programme stepping makes the new table
  visible in the record whose `cmd_epoch` reports that command; and
- sub-harmonic locking remains unsupported.

Do not duplicate table-mode logic inside `StandardProgram`. It should own and
call the existing, host-tested `TablePlayer`.

## Per-tick order

The common core-1 loop after this refactor is:

1. wait on the continuously armed BUSY edge latch or PWM-wrap latch;
2. start timing diagnostics and call `rig.tick_start()`;
3. apply at most `COMMANDS_PER_TICK` queued commands, dispatching each to the
   rig, programme or common table activation as appropriate;
4. call `rig.measure` into the declared input prefix;
5. call `program.step`, which advances the master phase once and fills the
   bounded logical output slice;
6. evaluate the global rig fault and apply safe values/clamps to every logical
   output;
7. call `rig.actuate` once with the complete applied output slice;
8. call `program.write_signals`, then assemble inputs, programme signals,
   applied outputs and `cmd_epoch` into one coherent record;
9. enqueue or count the dropped record, call `rig.tick_end()` and update timing
   diagnostics.

This order deliberately retains measurement before controller calculation,
command application before phase advancement, safety after all programme
mixing and controller telemetry after physical actuation.

## Placement and static dispatch

Suggested locations:

- `helic-core/src/generator.rs` or a new `harmonics.rs`: `HarmonicFrame` and
  `HarmonicGenerator`;
- `helic-core/src/generator.rs`: `FourierSignal`, beside `FourierCoeffs` and
  the existing periodic-generator types;
- `helic-core/src/controller.rs` or a new `controlled_axis.rs`:
  `ControlledAxis`;
- `helic-core/src/program.rs`: `RtProgram`, `StandardProgram` and portable
  programme state types;
- `firmware/common/src/rt_loop.rs`: generic scheduling over `R`, `T` and `P`;
- `firmware/common/src/params.rs`: programme-backed shadow/command registry;
  and
- `firmware/common/src/rig.rs`: logical actuator declaration and assembled
  source lookup.

Every concrete programme is selected statically in experiment `config.rs`,
analogous to the current `ActiveController`:

```rust
pub type ActiveController = PassThrough;
pub type ActiveProgram = StandardProgram<ActiveController, HARMONICS>;

pub fn make_program(active_table: &'static WaveTable) -> ActiveProgram {
    StandardProgram::new(make_controller(), active_table)
}
```

Firmware setup calls `config::make_program(table::active())`, uses a shared
reference to that completed programme to seed `ParamStore`, then moves the
programme to core 1. This keeps the storage operation in common firmware and
does not make experiment configuration depend on mutable cross-core table
storage.

`RtLoopState<R, T, P>` owns `P` by value. There is no `dyn RtProgram`, trait
object, heap allocation or runtime programme selection.

All methods reachable from `P::step`, harmonic projection, controller
calculation, table playback, safety and `Rig::actuate` must meet the existing
SRAM hot-path rules. Use `#[cfg_attr(feature = "rt-sram",
unsafe(link_section = ".data.ram_func"))]` in portable crates and the direct
firmware annotation where appropriate.

## Migration plan

Implement in small, buildable stages:

1. Add `HarmonicFrame`/`HarmonicGenerator`, `FourierSignal` and projection
   tests in `helic-core`. Do not integrate firmware yet.
2. Add `ControlledAxis` and tests proving that reference sampling and
   controller calculation match the current path.
3. Add `RtProgram` and `StandardProgram` with host tests for the complete
   scalar calculation and every table mode.
4. Make `RtLoopState` own `StandardProgram` rather than separate controller,
   phase, coefficient and table-player fields. Preserve the scalar rig
   boundary temporarily if that is needed to keep an intermediate commit
   buildable.
5. Introduce the bounded programme output slice, `Rig::ACTUATORS`, indexed
   safety hooks and slice-based `Rig::actuate`; migrate all three production
   rigs in the same logical change so no production target is left broken.
6. Assemble source discovery from rig inputs, programme signals, actuators and
   `cmd_epoch`. Assert that current names and order are unchanged.
7. Parameterise `ParamStore` by the programme and delegate current controller
   parameters through `StandardProgram`.
8. Update developer documentation only after the implementation exists;
   retain this proposal as rationale or replace its status with an implemented
   design note.

If Rust's type constraints make steps 4--7 easier in another order, preserve
the behavioural checkpoints rather than the numbering. Keep each commit to
one logical unit and keep all production firmware crates compiling at commit
boundaries where practical.

## Required tests

### Host unit tests

Add tests for:

- every harmonic-frame projection matching `FourierCoeffs::evaluate` over
  representative phases, coefficients and all 16 harmonics;
- `FourierSignal::sample` matching direct harmonic-frame projection;
- two `FourierSignal` values sampled from one frame remaining exactly phase
  coherent while owning independent coefficient banks;
- complete `FourierSignal` coefficient replacement taking effect without
  retaining values from the previous bank;
- exact `u32` wrapping multiplication for every harmonic;
- one and only one master phase advancement per programme tick;
- phase-continuous master frequency changes;
- `ControlledAxis<PassThrough>` producing its reference;
- `ControlledAxis<PidController>` using measured input and reference exactly
  as the current controller path does;
- `StandardProgram` producing `controller + forcing + table`;
- controller telemetry preceding `target`, `forcing` and `table`;
- a free-running table remaining independent when master frequency changes;
- locked loop remaining an exact master-phase multiple with phase offset over
  long wrapping runs;
- locked one-shot waiting for the next actual master overflow and stopping
  after one multiplied cycle;
- free one-shot starting immediately and using only its private accumulator;
- mode, multiplier, phase-offset and interpolation changes preserving current
  reset/non-reset semantics;
- programme parameter delegation and normalisation;
- global safety quieting all outputs and clamping each indexed output;
- safety event counters incrementing per tick rather than per actuator;
- current production source names and ordering; and
- source/output capacity validation failures.

Where possible, run the old scalar calculation and `StandardProgram` from the
same input/command sequence and compare every sample. Exact equality is
preferred; if shared-basis evaluation changes floating-point evaluation order,
document and bound the difference before accepting it.

### Software integration checks

Run the complete repository checks required by `AGENTS.md`, including the
release firmware workspace build and both W6100 variants. Immediately before
the layout checker, build the complete release firmware workspace.

The release ELF inspection must confirm that:

- `run_hot_loop` and any emitted `run_rt_tick` remain in SRAM;
- emitted programme, harmonic-frame, projection, table and output-loop helpers
  reachable per tick are in SRAM or demonstrably inlined;
- ARM EABI copy/clear helpers resolve to the SRAM `rt_mem` implementations;
  and
- analogue transfer symbols remain in SRAM.

Update `firmware/tools/check_rt_layout.py` if the refactor creates stable,
non-inlined hot-path symbols worth requiring. The checker remains a named
symbol guard, not a complete call-graph proof.

The Python simulator and Python, Julia and MATLAB host suites must demonstrate
that `StandardProgram` has not changed the discovered base registry or user
semantics.

### Hardware regression

This refactor touches nearly the entire tick calculation and therefore
requires the complete sequential hardware regression in the developer guide.
At minimum:

1. Run CBC idle, TCP-poll and capture phases at 8 kHz and require zero
   overruns, tick timeouts and record drops, the accepted wake-phase spread
   and `loop_time_max <= 60 us`.
2. Run the all-source 8000-sample capture followed by the no-flash
   60000-sample capture.
3. Exercise zero, target-only, forcing-only and combined target/forcing paths.
4. Exercise table off, free loop, free one-shot, locked loop and locked
   one-shot. Verify phase relationship on a scope or through an appropriate
   loopback capture; software-only comparison is not physical phase evidence.
5. Repeatedly update complete target and forcing coefficient sets while
   capturing `cmd_epoch`, and prove boundary-coherent records.
6. Recommit tables and prove the activated table changes at the command epoch
   without a torn record.
7. Record exact firmware identity and results in `notes.md`.

Do not relax an acceptance limit to accommodate the abstraction.

## Acceptance criteria

The initial implementation is complete only when:

- `StandardProgram` is the only concrete programme and reproduces current
  SISO/MISO behaviour;
- the current base parameter registry is unchanged;
- current production source names and order are unchanged;
- target and forcing share one master harmonic frame advanced once per tick;
- the reference and forcing coefficient banks are each owned by a
  `FourierSignal`, with no duplicate live coefficient owner;
- free table modes remain independent and locked table modes retain exact
  master-phase semantics;
- the common loop no longer owns target/forcing coefficients, controller or
  table-player state separately;
- the programme owns one immutable active table reference while common table
  storage retains safe double-buffer ownership;
- all applied logical outputs pass through common safety before one rig
  actuation call;
- there is no allocation, dynamic dispatch, blocking lock, `f64`, Embassy,
  logging or critical section on the tick path;
- all software checks and the SRAM layout check pass; and
- required hardware evidence is recorded without degraded limits.

## Future extension boundary

After this change, a new SISO or MISO experiment using the standard signal
graph selects `StandardProgram`, chooses or constructs a controller and
implements only its experiment-local board/configuration/telemetry/rig files.
It does not change the common scheduler.

A future MIMO experiment supplies another statically selected `RtProgram`
which fills more than one logical output. It may own several
`ControlledAxis` values, a portable matrix controller or experiment-specific
`FourierSignal` banks and coefficient routing. The common loop, safety stage,
source assembly and rig actuation boundary should already accept it. Reusable
MIMO algorithms still belong in `helic-core`, not in `rig.rs`.

Independent per-actuator reference, forcing or table banks require a separate
proposal. In particular, atomic multi-axis coefficient replacement should use
a complete copied bank or a double-buffered bank swap; sequential per-channel
commands can otherwise take effect on different sample boundaries. That later
work must also budget command-queue SRAM, `MAX_SOURCES`, parameter payloads and
per-tick Fourier evaluation cost.
