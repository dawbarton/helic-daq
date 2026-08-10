# Rig decoupling: component-owned parameters, signals, and buffers

Status: proposed, not implemented. Supersedes parts of
`docs/rt_program_proposal.md` (see "Relationship to the RT programme
proposal").

## Goal

One goal, stated as a falsifiable test:

> Adding a rig, or converting a SISO rig to MIMO, must change no file under
> `helic-core/` or `firmware/common/`.

Everything else here follows from that. The intent is that `helic-core` and
`helic-fw-common` become stable shared dependencies, consumable by firmware
crates that may live in separate repositories, while each rig's specifics
(sensors, actuators, signal graph, controllers, parameter names, stream
sources) live entirely in that rig's own crate.

This is not a request for run-time flexibility. Nothing here needs to be
discovered, registered, or dispatched at run time on the real-time core. The
composition is fixed at compile time for each firmware binary; what changes is
*where the composition is written*.

## What currently forces a shared-code edit

Ten concrete coupling points, each of which a new rig can plausibly hit.

| # | Location | Coupling |
|---|---|---|
| 1 | `firmware/common/src/rig.rs:17` | `GENERATED_SOURCES` is a fixed five-entry list (`target`, `forcing`, `table`, `out`, `cmd_epoch`) baked into the shared crate |
| 2 | `firmware/common/src/rig.rs:29` | `source::<R>()` hand-chains three segments with offset arithmetic; a fourth category needs an edit |
| 3 | `firmware/common/src/rig.rs:202` | `fn actuate(&mut self, out: f32)` is scalar, so MIMO is unrepresentable |
| 4 | `firmware/common/src/rig.rs:198` | `type Ctrl: Controller` binds a rig to exactly one controller |
| 5 | `firmware/common/src/rt_loop.rs:21` | `RtCommand` enumerates all fourteen operations; a new owner needs a new variant |
| 6 | `firmware/common/src/rt_loop.rs:269` | The tick body hardcodes the signal graph and the record layout (`values[generated + 0..4]`) |
| 7 | `firmware/common/src/params/schema.rs:9` | `BASE_PARAMS` is a fixed 33-entry table with 23 companion `IDX_*` constants |
| 8 | `firmware/common/src/params.rs:324,439` | Two large matches keyed on those indices, plus four-segment offset arithmetic repeated in `def`, `get`, and `set` |
| 9 | `firmware/common/src/table.rs` | The entire module is a bespoke unsafe double buffer specialised to `WaveTable` |
| 10 | `firmware/common/src/lib.rs:23`, `helic-core/src/table.rs:5` | `HARMONICS` and `MAX_TABLE_LEN` are shared-crate constants, so a rig cannot trade harmonic count or table length against SRAM without editing them |

Points 1, 2, 5, 6, 7, and 8 are all the same underlying problem, which is worth
naming before proposing a fix.

## The three registries are one problem

The firmware maintains three separate lists of named things:

- **parameters**, assembled as `BASE_PARAMS + extras + rig + controller`;
- **stream sources**, assembled as `INPUTS + TELEMETRY + GENERATED_SOURCES`;
- **commands**, enumerated exhaustively as `RtCommand`.

Each is hand-chained, each has its own offset arithmetic, and each grows a new
segment whenever a new kind of owner appears. They are three instances of one
pattern: *a flat, host-visible index space assembled from independently owned
components*.

This is the reusable idea from the earlier `rtc` project. Its
`rtc_data_add_par(name, ptr, type, size, trigger_func, trigger_data)` let each
subsystem contribute entries to a single lookup table from its own
initialisation code, with the framework owning only the table and never the
storage. What does not carry across is the mechanism: `rtc` was single-core, so
a host write could go straight into a `volatile` pointer or a synchronous
trigger callback running in the same context as the sample loop. HELIC-DAQ's
`ParamStore`/`RtCommand`/SPSC apparatus exists precisely because core 0 and
core 1 must never share a raw pointer. The value-copy boundary stays; the
hand-chaining goes.

## Design principle: core 0 may be dynamic, core 1 must not be

The two cores have opposite constraints, and the current design applies core
1's constraints to both.

Core 0 runs the TCP control server. Parameter reads and writes happen at human
or host-script rates, and a vtable indirection there is free. Core 1 runs a
125 µs tick with a verified 34 µs maximum, an SRAM residency requirement
enforced by `firmware/tools/check_rt_layout.py`, and a whole-graph inlining
assumption underpinning that timing.

So:

- **Core 0** holds a list of trait objects, one per component, and walks it.
  All index arithmetic in the system reduces to one function.
- **Core 1** holds one statically dispatched `Program` value chosen in the
  experiment's `config.rs`, exactly as `ActiveController` is chosen today. No
  `dyn`, no allocation, no run-time selection.

The two halves of a component are then written adjacent in one file, sharing
one set of local id constants, so they cannot drift apart.

## Design

### 1. `RtCommand` collapses to two variants

There are exactly two command targets on core 1: the programme and the rig.
Everything else is a parameter id and a bounded payload.

```rust
/// Bounded command payload. These four variants cover every operation in the
/// current firmware and every one foreseen for MIMO.
#[derive(Clone, Copy, Debug)]
pub enum Payload {
    Unit,                        // triggers: ctrl_reset, table_trigger
    F32(f32),                    // gains, table gain
    U32(u32),                    // increments, modes, multipliers, buffer ids
    Coeffs(FourierCoeffs<HARMONICS>),
}

#[derive(Clone, Copy, Debug)]
pub enum RtCommand {
    Program { id: u16, payload: Payload },
    Rig { id: u16, payload: Payload },
}
```

Fourteen variants become two. Adding a MIMO axis adds a parameter name and a
match arm inside the programme's own file, and touches neither `RtCommand` nor
the dispatch in `rt_loop.rs`.

Queue element size is unchanged, being dominated by `Coeffs` at
`(1 + 2·16)·4 = 132` bytes, so the 32-slot queue stays at about 4.2 KB.

Enum-valued parameters (`TableMode`, `TableInterpolation`) are still validated
into a concrete Rust type on core 0 before being queued; they simply travel as
their `u32` discriminant and are re-tagged by the receiving component, which
already knows the type. This preserves the property that core 1 never parses an
unvalidated host value.

### 2. One index walk, written once

```rust
// firmware/common/src/params.rs
pub trait ParamGroup {
    /// This component's parameters, addressed by local id 0..n.
    fn params(&self) -> &'static [ParamDef];

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode>;

    /// Validate `data`, update this component's own shadow, and say what (if
    /// anything) core 1 must be told.
    fn set(&mut self, id: u16, data: &[u8]) -> Result<Update, ErrorCode>;

    /// Block upload into a staging buffer, for `ParamKind::Blob` parameters.
    fn set_block(&mut self, _id: u16, _offset: u32, _data: &[u8]) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn commit(&mut self, _id: u16, _len: u32) -> Result<Update, ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
}

pub enum Update {
    /// Applied on core 0; nothing to enqueue (`diag_reset`, `arm`).
    Local,
    /// Forward to core 1.
    Rt(RtCommand),
    /// Accepted, and the control server must act (`mcu_reboot`).
    Action(ParamAction),
}

pub struct ParamStore {
    groups: heapless::Vec<&'static mut dyn ParamGroup, MAX_GROUPS>,
    commands: CommandProducer,
}

impl ParamStore {
    /// The only index arithmetic in the firmware.
    fn locate(&self, index: usize) -> Option<(usize, u16)> {
        let mut base = 0;
        for (g, group) in self.groups.iter().enumerate() {
            let n = group.params().len();
            if index < base + n {
                return Some((g, (index - base) as u16));
            }
            base += n;
        }
        None
    }
}
```

`ParamStore` loses its `<C: Controller, R: Rig>` parameters entirely, along
with `PhantomData`, the four-segment arithmetic repeated three times, the 23
`IDX_*` constants, and both hundred-line matches. Groups are `StaticCell`
-initialised in the experiment's `main.rs`, the pattern already used for
`SIN_LUT`, `CORE1_STACK`, and the UART buffers, so no allocator is involved.

`ParamDef` gains a kind so that blob handling stops being an
`index == IDX_TABLE` special case:

```rust
pub enum ParamKind {
    Scalar,
    Array(u16),   // fixed count, written whole (coefficient banks)
    Blob(u32),    // block-uploaded, commit-activated (waveform tables)
}
```

### 3. The programme owns the graph; the rig owns the hardware

```rust
// firmware/common/src/rt_loop.rs
pub trait Program {
    const OUTPUTS: usize;
    const SIGNALS: &'static [(&'static str, &'static str)];

    fn apply(&mut self, id: u16, payload: Payload);
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx, outputs: &mut [f32]);
    fn write_signals(&self, out: &mut [f32]);
}
```

`StepCtx` carries the shared `&SinLut` and anything else the platform provides
per tick. `Rig::actuate` takes `&[f32]` of length `R::ACTUATORS.len()`, and
`Rig` loses its `type Ctrl` association: a rig describes hardware, and how many
controllers run against it is the programme's business.

Source assembly becomes a generic walk over `R::INPUTS`, `P::SIGNALS`,
`R::ACTUATORS`, and `cmd_epoch`, so `GENERATED_SOURCES` disappears. For the
existing rigs this reproduces today's names and order exactly, because
`StandardProgram::SIGNALS` is `[controller telemetry..., target, forcing,
table]` and `ACTUATORS` is `[("out", "V")]`.

### 4. Shared-crate constants become associated constants

`HARMONICS` moves from `firmware/common/src/lib.rs` to a const generic on the
programme, which it already needs for `FourierCoeffs<H>`. `MAX_TABLE_LEN`
becomes a const generic on `WaveTable<const N: usize = 4096>`, defaulted so
existing code is unaffected. Both matter for RAM budgeting once several
components own buffers (see below).

## Double buffering

This is the part that does not generalise for free, and it deserves its own
analysis because it is the one mechanism that cannot simply be made generic and
forgotten.

### The two patterns currently sharing one name

**Small POD.** `FourierCoeffs<H>` travels *inside* the command queue element.
The queue is itself the double buffer: core 0 copies a complete validated value
in, core 1 copies it out at a sample boundary, and no tick ever observes a torn
array. This costs `sizeof(largest payload) × QUEUE_LEN` of SRAM.

**Large blob.** `WaveTable` is 16 KB, far too large to place in every one of 32
queue slots. `firmware/common/src/table.rs` instead keeps two persistent
statics and swaps a `&'static` pointer using a three-state protocol across two
atomics (`ACTIVE`, `PENDING`, with a `NO_PENDING` sentinel). Core 0 mutates
only the inactive buffer; core 1 publishes with release ordering at a sample
boundary.

The ceiling between them is real and worth writing down. A 4×4 MIMO gain matrix
of Fourier banks would be `16 × 132 = 2.1 KB` per queue slot and `67 KB` for the
queue, which is not acceptable against 520 KB total SRAM. So the boundary is
roughly: anything up to a few hundred bytes goes by value through the queue;
anything larger needs a buffer swap.

### Options considered

**A. Extend value-in-command.** Keep everything in the queue and accept the
slot cost. Rejected for anything blob-sized on the arithmetic above, but
retained as the default for coefficient banks, gains, and matrix rows, which is
where MIMO parameters actually sit.

**B. Generic `DoubleBuffer<T>`.** Lift `table.rs`'s protocol verbatim into
`helic-core`, parameterised by `T`. Recommended.

**C. Per-object mailbox polled by core 1.** Drop the `UseTable(id)` command
entirely and let the RT side poll a "new value ready" flag. Slightly less
machinery, and immune to a full command queue returning `Busy`. Rejected
because it decouples activation from `cmd_epoch`: the current guarantee that a
table swap is visible in exactly the record whose `cmd_epoch` reports that
command is hardware-verified (`notes.md`, 2026-07-16, the 0.45 V to 1.65 V live
re-commit and the `forcing,out,cmd_epoch` transition at sample 885064). Option C
would invalidate that evidence for no structural gain.

**D. `Pool<T, N>` instead of two buffers.** The natural generalisation if
several blobs must swap independently, for example per-axis tables on a MIMO
rig. `DoubleBuffer<T>` is exactly `Pool<T, 2>`. Not proposed now, but the
recommended shape is chosen so this remains a drop-in later.

### Recommendation

Move the existing protocol into `helic-core`, unchanged in behaviour:

```rust
// helic-core/src/double_buffer.rs
pub struct DoubleBuffer<T: 'static> {
    buffers: [UnsafeCell<T>; 2],
    active: AtomicU8,
    pending: AtomicU8,
}

impl<T> DoubleBuffer<T> {
    pub const fn new(a: T, b: T) -> Self;

    /// Core 0: exclusive access to the inactive buffer, or `Busy`.
    pub fn staging(&'static self) -> Result<&'static mut T, ErrorCode>;
    /// Core 0: mark the staged buffer ready; the id travels in the command.
    pub fn begin_commit(&'static self) -> Result<u8, ErrorCode>;
    pub fn cancel_commit(&'static self);

    /// Core 1: publish buffer `id` at a sample boundary.
    pub fn activate(&'static self, id: u8) -> &'static T;
    pub fn active(&'static self) -> &'static T;
}
```

The safety argument is the one already written in `table.rs`, generalised:
core 0 mutates only the inactive buffer, a commit marks it pending before the
command enters the queue, further writes return `Busy` until core 1 switches,
and core 1 publishes `ACTIVE` with release ordering before clearing `PENDING`.
Being in `helic-core` makes the state machine host-testable for the first time.

`firmware/common/src/table.rs` then becomes a thin instantiation owned by
whichever experiment wants a table, and a rig with no waveform table (a pure
MIMO feedback rig, say) simply omits the component. Its nine `table_*`
parameters then vanish from discovery rather than being advertised and inert,
which is a behaviour improvement, not just a structural one.

**RAM budget, stated plainly.** One `DoubleBuffer<WaveTable>` is 32 KB. Four
per-axis tables would be 128 KB against a 520 KB budget, on top of the ~130 KB
already used. This is the reason `MAX_TABLE_LEN` should become a const generic:
a MIMO rig can then choose `WaveTable<512>` and pay 4 KB per buffer. Without
that, per-axis tables are not affordable, and the constraint would only be
discovered at link time.

## Examples

### Example 1: CBC after the change (behaviour identical)

```rust
// fw-cbc-rig/src/config.rs
pub type ActiveProgram = StandardProgram<PassThrough, 16>;

pub fn make_program(table: &'static WaveTable) -> ActiveProgram {
    StandardProgram::new(PassThrough, table)
}
```

```rust
// fw-cbc-rig/src/main.rs (assembly, once, at boot)
static PROGRAM_SHADOW: StaticCell<StandardShadow<16>> = StaticCell::new();
static RIG_SHADOW: StaticCell<CbcShadow> = StaticCell::new();

let mut store = ParamStore::new(channels.command_tx, config::SAMPLE_RATE);
store.push(PLATFORM_SHADOW.init(PlatformShadow::new(config::EXPERIMENT)));
store.push(PROGRAM_SHADOW.init(StandardShadow::new(&program)));
store.push(RIG_SHADOW.init(CbcShadow::new()));
store.push(TELEMETRY_SHADOW.init(CbcTelemetry::new()));
```

The discovered registry, source names, source order, and wire format are
unchanged. What changed is that the order is now visible in one place in the
experiment crate rather than implied by arithmetic in the shared crate.

### Example 2: a two-axis MIMO rig, entirely in its own crate

This is the test case the whole proposal exists to serve. Note that both halves
live in one file and share `mod ids`, so the core-0 and core-1 views cannot
disagree.

```rust
// fw-mimo-rig/src/program.rs
use helic_core::{ControlledAxis, FourierCoeffs, HarmonicGenerator, PidController};
use helic_fw_common::params::{ParamDef, ParamGroup, ParamType, Update};
use helic_fw_common::rt_loop::{Payload, Program, RtCommand, StepCtx};

const H: usize = 8;
const AXES: usize = 2;
const COEFFS: u16 = (1 + 2 * H) as u16;

mod ids {
    pub const FREQ: u16 = 0;
    pub const REF0: u16 = 1;
    pub const REF1: u16 = 2;
    pub const KP0: u16 = 3;
    pub const KP1: u16 = 4;
    pub const RESET: u16 = 5;
}

// ---- core 1 -------------------------------------------------------------

pub struct TwoAxis {
    harmonics: HarmonicGenerator<H>,
    axes: [ControlledAxis<PidController, H>; AXES],
    errors: [f32; AXES],
}

impl Program for TwoAxis {
    const OUTPUTS: usize = AXES;
    const SIGNALS: &'static [(&'static str, &'static str)] = &[
        ("ref0", "V"), ("ref1", "V"), ("err0", "V"), ("err1", "V"),
    ];

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply(&mut self, id: u16, payload: Payload) {
        match (id, payload) {
            (ids::FREQ, Payload::U32(inc)) => self.harmonics.set_increment(inc),
            (ids::REF0, Payload::Coeffs(c)) => self.axes[0].set_reference(c),
            (ids::REF1, Payload::Coeffs(c)) => self.axes[1].set_reference(c),
            (ids::KP0, Payload::F32(v)) => self.axes[0].controller.pid.config.kp = v,
            (ids::KP1, Payload::F32(v)) => self.axes[1].controller.pid.config.kp = v,
            (ids::RESET, _) => self.axes.iter_mut().for_each(ControlledAxis::reset),
            _ => {}
        }
    }

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx, outputs: &mut [f32]) {
        let frame = self.harmonics.step(ctx.lut);
        for (i, axis) in self.axes.iter_mut().enumerate() {
            let sample = axis.step(inputs, frame, dt);
            self.errors[i] = sample.reference - inputs[i];
            outputs[i] = sample.control;
        }
    }

    fn write_signals(&self, out: &mut [f32]) {
        out[0] = self.axes[0].reference_value();
        out[1] = self.axes[1].reference_value();
        out[2] = self.errors[0];
        out[3] = self.errors[1];
    }
}

// ---- core 0 -------------------------------------------------------------

pub struct TwoAxisShadow {
    freq_hz: f32,
    refs: [FourierCoeffs<H>; AXES],
    kp: [f32; AXES],
    sample_rate: SampleRate,
}

impl ParamGroup for TwoAxisShadow {
    fn params(&self) -> &'static [ParamDef] {
        &[
            ParamDef::writable("freq", ParamType::F32, 1),
            ParamDef::writable("ref0_coeffs", ParamType::F32, COEFFS),
            ParamDef::writable("ref1_coeffs", ParamType::F32, COEFFS),
            ParamDef::writable("ctrl_kp0", ParamType::F32, 1),
            ParamDef::writable("ctrl_kp1", ParamType::F32, 1),
            ParamDef::writable("ctrl_reset", ParamType::U32, 1),
        ]
    }

    fn set(&mut self, id: u16, data: &[u8]) -> Result<Update, ErrorCode> {
        let cmd = |payload| Update::Rt(RtCommand::Program { id, payload });
        match id {
            ids::FREQ => {
                let hz = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&hz) {
                    return Err(ErrorCode::BadValue);
                }
                self.freq_hz = hz;
                Ok(cmd(Payload::U32(PhaseAccumulator::increment_for(
                    hz as f64, self.sample_rate.hz() as f64,
                ))))
            }
            ids::REF0 | ids::REF1 => {
                let coeffs = deserialize_coeffs::<H>(data)?;
                self.refs[(id - ids::REF0) as usize] = coeffs;
                Ok(cmd(Payload::Coeffs(coeffs)))
            }
            ids::KP0 | ids::KP1 => {
                let v = f32::from_le_bytes(data.try_into().unwrap());
                if !v.is_finite() { return Err(ErrorCode::BadValue); }
                self.kp[(id - ids::KP0) as usize] = v;
                Ok(cmd(Payload::F32(v)))
            }
            ids::RESET => Ok(cmd(Payload::Unit)),
            _ => Err(ErrorCode::BadIndex),
        }
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> { /* ... */ }
}
```

Files changed under `helic-core/` or `firmware/common/` to support this rig:
none.

### Example 3: a table as an ordinary component

```rust
// in whichever experiment crate wants a waveform table
static TABLE: DoubleBuffer<WaveTable<4096>> =
    DoubleBuffer::new(WaveTable::empty(), WaveTable::empty());

impl ParamGroup for TableShadow {
    fn params(&self) -> &'static [ParamDef] {
        &[
            ParamDef::blob("table", MAX_LEN),
            ParamDef::read_only("table_len", ParamType::U16, 1),
            ParamDef::writable("table_freq", ParamType::F32, 1),
            // ... gain, interp, mode, mult, phase, trigger
        ]
    }

    fn set_block(&mut self, id: u16, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        write_f32_block(TABLE.staging()?, offset, data)
    }

    fn commit(&mut self, id: u16, len: u32) -> Result<Update, ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        let buffer = validate_and_begin_commit(&TABLE, len)?;
        Ok(Update::Rt(RtCommand::Program {
            id: ids::TABLE,
            payload: Payload::U32(buffer as u32),
        }))
    }
}
```

Core 1's `apply` calls `TABLE.activate(id)` and hands the resulting
`&'static WaveTable` to its `TablePlayer`, at the same sample boundary as
today, with the same `cmd_epoch` coherence.

### Example 4: what you touch to add a rig

| Task | Today | Proposed |
|---|---|---|
| New SISO rig, existing controller | experiment crate only | experiment crate only |
| New input source | experiment crate only | experiment crate only |
| New controller parameter | experiment crate only | experiment crate only |
| Second actuator | `rig.rs`, `rt_loop.rs`, `params.rs`, `schema.rs` | experiment crate only |
| Second controlled axis | `rt_loop.rs` (new `RtCommand` variants), `params.rs`, `schema.rs` | experiment crate only |
| Rig with no waveform table | not expressible; params advertised and inert | omit the component |
| Second buffered blob | copy `table.rs` | one `DoubleBuffer<T>` static |
| Different harmonic count | `firmware/common/src/lib.rs` | const generic on the programme |
| Shorter tables to save RAM | `helic-core/src/table.rs` | const generic on `WaveTable` |

## Repository separation

With the coupling points removed, a firmware crate in a separate repository
needs:

1. `helic-core`, `helic-proto`, `helic-drivers`, and `helic-fw-common`
   published, or referenced as git dependencies. They are already a clean
   dependency layer; `helic-fw-common` is the only RP2350-specific one.
2. Its own `.cargo/config.toml` with the `thumbv8m.main-none-eabihf` target and
   runner, currently at `firmware/.cargo/config.toml` and shared by the
   firmware workspace.
3. Its own `Cargo.lock`. The pinned Embassy set currently in
   `firmware/Cargo.toml`'s `[workspace.dependencies]` would need publishing as
   documented guidance or a version-range policy, since a separate repository
   cannot inherit it.
4. A copy of, or a shared action for, the `check_rt_layout.py` gate. The gate is
   a named-symbol guard, so its symbol list is partly experiment-specific
   already.

Point 3 is the main practical friction and is worth deciding deliberately.
Pinning exact Embassy versions across repositories that must interoperate with
one `helic-fw-common` is a real maintenance cost, and the alternative, a
version range, weakens the "one tested dependency set" property the comment in
`fw-cbc-rig/Cargo.toml` currently claims. A reasonable middle path is to keep
the three production rigs in this repository and treat the crate boundary as
the contract that *permits* an external rig, verifying it with one
out-of-workspace test rig rather than by moving everything at once.

## Relationship to the RT programme proposal

`docs/rt_program_proposal.md` is retained where it concerns the portable signal
types, and superseded where it concerns the extension mechanism.

**Retained:** `HarmonicFrame`/`HarmonicGenerator` and one shared basis per
tick; `FourierSignal`; `ControlledAxis<C, H>`; removal of `PeriodicGenerator`
and `GenSample`; the bounded logical output vector; `Rig::ACTUATORS`; the
safety gate's `SAFETY_GATED` opt-out and per-tick counter semantics; all
mandatory table-phase semantics; the per-tick ordering.

**Superseded:** the slot-indexed `SetCoeffs(u16, _)`/`SetIncrement(u16, _)`
scheme, which is a half-step towards a general parameter id and leaves twelve
other variants in place; `ParamStore<P, R>` remaining generic over the
programme and rig; the `RtProgram` trait's parameter methods
(`param_names`, `param_value`, `normalise_param`, `set_param`), which are
replaced by the component's own `ParamGroup` half; and the retention of
`GENERATED_SOURCES` and the table's special-cased command path.

The practical difference is that the earlier proposal makes adding a
coefficient bank free but leaves adding a *kind* of thing costly, whereas this
one makes both free at the cost of a larger initial refactor.

## Migration plan

Each stage is buildable and independently testable.

1. **`DoubleBuffer<T>` into `helic-core`**, with host tests for the state
   machine. Reimplement `firmware/common/src/table.rs` on top of it with no
   behaviour change. Independently verifiable on hardware by repeating the
   existing table re-commit regression.
2. **`Payload` and the two-variant `RtCommand`**, with the existing fixed
   dispatch rewritten to match on `(target, id, payload)` against the current
   `IDX_*` constants. No registry change yet. This isolates the command-path
   change from the registry change.
3. **`ParamGroup` and the `ParamStore` walk.** Convert the existing four
   segments into four groups (platform, extras, rig, controller) that reproduce
   the current registry exactly. `locate` replaces all offset arithmetic. This
   is the largest single commit and should change no discovered name or index.
4. **`Program` trait and `StandardProgram`**, absorbing phase, coefficients,
   controller, and table player out of `RtLoopState`. Split the table out as
   its own component in the same change, since its parameters move groups.
5. **Bounded output vector**, `Rig::ACTUATORS`, slice-based `actuate`, and
   generic source assembly. Migrate all three rigs together.
6. **Const generics** for `HARMONICS` and `MAX_TABLE_LEN`, defaulted to current
   values.
7. **An out-of-workspace test rig** proving the crate boundary holds, before
   any decision about separate repositories.

Stages 1 to 3 are worth doing regardless of whether MIMO ever arrives: they
remove the offset arithmetic and the bespoke unsafe module without changing any
externally visible behaviour.

## Tests

Beyond the host unit tests already listed in `rt_program_proposal.md`:

- `DoubleBuffer<T>` state machine: staging returns `Busy` while pending;
  `activate` publishes and clears; `cancel_commit` restores writability; a
  commit rejected for a non-finite value leaves the active buffer untouched.
- `ParamStore::locate` over registries with zero-length groups, a single group,
  and groups crossing a discovery page boundary.
- Golden-registry test: the assembled name list and index order for each of the
  three production rigs, asserted against the current registry verbatim. This is
  the primary regression guard for stages 3 to 5.
- Golden-source test: likewise for source names and order.
- A `Payload` round trip for every parameter kind, proving core 1 receives the
  same value core 0 validated.
- `apply` ignoring an unknown id without panicking, matching today's
  `set_param` behaviour.

Hardware regression is the full sequential suite in the developer guide, since
stages 4 and 5 touch the entire tick calculation. The `cmd_epoch` coherence
tests for coefficient replacement and table re-commit are the specific evidence
that stages 1 and 4 must not degrade, and the 34 µs loop maximum with a fixed
36 µs wake phase is the timing baseline.

## Risks and honest weaknesses

- **This is a larger change than the existing proposal**, and it touches a tick
  path with hardware-verified timing. The mitigation is stage ordering: stages
  1 to 3 are behaviour-preserving and separately verifiable, and the golden
  registry and source tests make an accidental interface change a build
  failure rather than a field surprise.
- **`Program` risks becoming a god-trait.** It currently absorbs the graph, the
  signals, and the command dispatch. If it grows further, split it into
  `ProgramStep`, `ProgramSignals`, and `ProgramParams` rather than letting
  policy drift back into `rt_loop.rs`.
- **Trait objects on core 0 cost flash** for vtables, and `locate` is O(groups)
  per access rather than O(1). Both are negligible on the TCP path, but the
  claim should be checked against the release ELF rather than assumed.
- **The inlining assumption on core 1 is unchanged but newly load-bearing.**
  `Program::step` is statically dispatched, so the compiler should still inline
  the whole graph, but a two-axis programme with a loop over axes may not
  unroll as the current straight-line code does. This must be confirmed by ELF
  inspection and timing, not by reasoning about the source.
- **`Payload` fixes a payload vocabulary.** A future parameter that is neither
  scalar, `u32`, nor a coefficient bank needs a new variant, which is a
  shared-crate edit. Four variants covering everything current and foreseen is a
  judgement, not a proof, and the MIMO gain-matrix case in particular deserves
  checking before stage 2 rather than after.
- **Nothing here improves the host libraries.** Python, Julia, and MATLAB
  already discover the registry by name, so they should need no change, which is
  itself the strongest evidence the current host interface is the right one to
  preserve.

## Open questions

1. Should the platform's own parameters (firmware identity, diagnostics,
   safety) be a `ParamGroup` like any other, or stay privileged and always
   first? Making them ordinary is more uniform; keeping them first guarantees
   a stable prefix for host code that (incorrectly) caches indices.
2. Does any planned MIMO controller need a payload larger than
   `FourierCoeffs<H>`? If so, decide between a larger `Payload` variant and a
   `DoubleBuffer` for controller state before stage 2.
3. Is the three production rigs' shared Embassy pin worth preserving across
   repositories, or is one repository with a proven crate boundary the better
   endpoint?
