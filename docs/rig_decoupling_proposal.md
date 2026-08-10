# Rig decoupling: component-owned parameters, signals, and buffers

Status: proposed, not implemented. Revision 2, after review. Supersedes parts
of `docs/rt_program_proposal.md` (see "Relationship to the RT programme
proposal"). Changes made in response to review are listed under "Review
responses".

## Goal

One goal, stated as a falsifiable test:

> Composing a rig from existing platform primitives, within documented source,
> output, group, payload, and memory bounds, must require changes only in that
> rig's own repository.

The qualification matters and is not a hedge. Fixed capacities are a real part
of a bounded real-time platform: `MAX_SOURCES` is 24 today
(`firmware/common/src/rig.rs:12`), and comparable limits will exist for
actuators, parameter groups, payload width, and SRAM. Raising one of those is a
deliberate platform change with its own timing and memory evidence, not a
failure of decoupling. Equally, a genuinely new reusable DSP algorithm belongs
in `helic-core` and a new peripheral driver belongs in `helic-drivers`; putting
either in a rig crate would be the failure mode, not the success case.

What the test does exclude is the situation today, where a second actuator or a
second controlled axis forces edits to the shared command enum, the shared
parameter schema, and the shared source assembly.

### Documented platform capacities

These become the explicit contract. Exceeding one is a shared-crate change.

| Bound | Value | Set by |
|---|---|---|
| `MAX_SOURCES` | 24 | protocol discovery headroom |
| `MAX_ACTUATORS` | 4 | record and safety-gate buffers |
| `MAX_GROUPS` | 8 | `ParamStore` registry vector |
| `MAX_RT_VALUES` | 33 | widest command payload, `1 + 2·16` harmonics |
| `COMMAND_QUEUE_LEN` | 32 | existing |
| `COMMANDS_PER_TICK` | 2 | existing WCET bound |

## What currently forces a shared-code edit

| # | Location | Coupling |
|---|---|---|
| 1 | `firmware/common/src/rig.rs:17` | `GENERATED_SOURCES` is a fixed five-entry list baked into the shared crate |
| 2 | `firmware/common/src/rig.rs:29` | `source::<R>()` hand-chains three segments with offset arithmetic |
| 3 | `firmware/common/src/rig.rs:202` | `fn actuate(&mut self, out: f32)` is scalar, so MIMO is unrepresentable |
| 4 | `firmware/common/src/rig.rs:198` | `type Ctrl: Controller` binds a rig to exactly one controller |
| 5 | `firmware/common/src/rt_loop.rs:21` | `RtCommand` enumerates all fourteen operations |
| 6 | `firmware/common/src/rt_loop.rs:269` | The tick body hardcodes the signal graph and record layout |
| 7 | `firmware/common/src/params/schema.rs:9` | `BASE_PARAMS` is a fixed 33-entry table with 23 companion `IDX_*` constants |
| 8 | `firmware/common/src/params.rs:324,439` | Two large index-keyed matches, plus four-segment arithmetic repeated three times |
| 9 | `firmware/common/src/table.rs` | The whole module is a bespoke unsafe double buffer specialised to `WaveTable` |
| 10 | `firmware/common/src/lib.rs:23`, `helic-core/src/table.rs:5` | `HARMONICS` and `MAX_TABLE_LEN` are shared constants |

Points 1, 2, 5, 6, 7, and 8 are one problem: parameters, stream sources, and
commands are three separately hand-chained index spaces, each growing a segment
whenever a new kind of owner appears.

The reusable idea from the earlier `rtc` project is its
`rtc_data_add_par(name, ptr, type, size, trigger_func, trigger_data)`, which let
each subsystem contribute entries to one lookup table from its own
initialisation code, with the framework owning the table and never the storage.
What does not carry across is the mechanism: `rtc` was single-core, so a host
write could go straight into a `volatile` pointer. The cross-core value-copy
boundary stays; the hand-chaining goes.

## Design principle: core 0 may be dynamic, core 1 must not be

Core 0 runs the TCP control server, where a vtable indirection is free. Core 1
runs a 125 µs tick with a verified 34 µs maximum, SRAM residency enforced by
`firmware/tools/check_rt_layout.py`, and a whole-graph inlining assumption
underpinning that timing.

- **Core 0** holds a list of trait objects, one per component, and walks it. All
  index arithmetic reduces to one function.
- **Core 1** holds one statically dispatched `Program` value chosen in the
  experiment's `config.rs`. No `dyn`, no allocation, no run-time selection.

A component's two halves are written adjacent in one file, sharing one set of
local id constants, so they cannot drift apart.

## Crate layout

Making a rig's programme host-testable requires the runtime *contract* types to
be portable. They currently sit in the RP2350-specific `helic-fw-common`, which
is why the previous revision of this document ended up putting control logic in
a firmware binary crate, in violation of developer-guide principle 4 ("Logic is
portable; wiring is local", `docs/developer_guide.md:24`).

A new portable crate resolves this:

| Crate | Contents | Testable |
|---|---|---|
| `helic-proto` | wire protocol, `ErrorCode` | host |
| `helic-core` | DSP: generators, controllers, filters, tables, `DoubleBuffer` | host |
| **`helic-rt`** (new) | `Rig`, `TickSource`, `Program`, `ParamGroup`, `ParamDef`, `Payload`, `RtCommand`, `ParamStore`, safety gate, source assembly | host |
| `helic-fw-common` | RP2350/Embassy plumbing: concrete tick sources, net, comms, laser, PIO, reboot, `rt_mem`, the core-1 loop driver | cross-build |
| `helic-drivers` | chip drivers | host |
| `<rig>-program` | that rig's programme, controllers, and parameter shadows | host |
| `fw-<rig>` | that rig's `board.rs`, `config.rs`, `rig.rs`, `main.rs` | cross-build |

`helic-rt` depends on `helic-core`, `helic-proto`, and `heapless`; nothing
Embassy or RP2350. A rig repository then contains two crates, `<rig>-program`
and `fw-<rig>`, and its control logic is `cargo test`-able on the host exactly
as `helic-core`'s is. This is still one codebase per rig.

The safety gate moves to `helic-rt` as a pure function, so vector clamping
becomes host-testable for the first time. The tick driver stays in
`helic-fw-common` because it reads `pac::TIMER0` and uses `defmt`.

## Design

### 1. One command address, one payload vocabulary

There is exactly one command shape. `domain` selects a component; `id` is that
component's local parameter id.

```rust
// helic-rt
pub const MAX_RT_VALUES: usize = 33;   // 1 + 2·16 harmonics
pub const DOMAIN_RIG: u8 = 0;          // reserved; programmes use 1..

#[derive(Clone, Copy, Debug)]
pub enum Payload {
    Unit,                                          // triggers
    F32(f32),
    U32(u32),                                      // increments, modes, multipliers
    Values { len: u8, data: [f32; MAX_RT_VALUES] },// coefficient banks, matrix rows
    Buffer(CommitToken),                           // blob activation
}

#[derive(Clone, Copy, Debug)]
pub struct RtCommand {
    pub domain: u8,
    pub id: u16,
    pub payload: Payload,
}

// The queue is 32 of these; assert rather than assume.
const _: () = assert!(core::mem::size_of::<RtCommand>() <= 160);
```

Fourteen variants become one struct with a five-variant payload. Core 1 routes
in the loop:

```rust
if cmd.domain == DOMAIN_RIG {
    rig.apply(cmd.id, cmd.payload);
} else {
    program.apply(cmd.domain, cmd.id, cmd.payload);
}
```

The programme owns routing among its own sub-components, so a `TablePlayer`
group and a `StandardProgram` group can both use local id 0 without colliding.

`Values` replaces a `FourierCoeffs<H>` variant, which could not work: the
payload type must be non-generic to keep `RtCommand` and the queue non-generic,
but `HARMONICS` becomes a per-programme const generic (§4). A flat, length-
tagged array is independent of `H`, and the receiving component reconstructs its
own typed value because the `(domain, id)` pair already determines the meaning.
`MAX_RT_VALUES = 33` preserves the current 16-harmonic capacity; a programme
wanting more harmonics is a documented platform-capacity change.

Enum-valued parameters are still validated into a concrete Rust type on core 0
and travel as their `u32` discriminant, so core 1 never parses an unvalidated
host value.

### 2. Transactional writes, ordered centrally

The current code enqueues before updating the shadow, and rolls back a blob
commit if the enqueue fails (`firmware/common/src/params.rs:596,623`). That
ordering is a correctness property: a host that receives `Busy` must not then
read back the value it was denied. Distributing `set` to components must not
distribute that rule, or it becomes N places to get wrong.

So the group stages, and the store decides:

```rust
// helic-rt
pub enum Staged {
    Local(ParamAction),   // applied on core 0: diag_reset, arm, mcu_reboot
    Rt(RtCommand),
}

pub trait ParamGroup {
    fn params(&self) -> &'static [ParamDef];
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode>;

    /// Validate `data` and stage the change. Must not alter any
    /// host-observable state: `reject` has to undo it completely.
    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode>;
    /// The command was enqueued. Publish the staged value to the shadow.
    fn accept(&mut self, id: u16);
    /// The command could not be enqueued. Discard the staged value and
    /// release any staging buffer.
    fn reject(&mut self, id: u16);

    fn set_block(&mut self, _id: u16, _offset: u32, _data: &[u8]) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn stage_commit(&mut self, _id: u16, _len: u32) -> Result<Staged, ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
}
```

The ordering is then written exactly once, in the store:

```rust
impl ParamStore {
    pub fn set(&mut self, index: usize, data: &[u8]) -> Result<ParamAction, ErrorCode> {
        let (g, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        let group = &mut self.groups[g];
        match group.stage(id, data)? {
            Staged::Local(action) => { group.accept(id); Ok(action) }
            Staged::Rt(cmd) => match self.commands.enqueue(cmd) {
                Ok(()) => { group.accept(id); Ok(ParamAction::None) }
                Err(_) => { group.reject(id); Err(ErrorCode::Busy) }
            },
        }
    }

    /// The only index arithmetic in the firmware.
    fn locate(&self, index: usize) -> Option<(usize, u16)> {
        let mut base = 0;
        for (g, group) in self.groups.iter().enumerate() {
            let n = group.params().len();
            if index < base + n { return Some((g, (index - base) as u16)); }
            base += n;
        }
        None
    }
}
```

Blob commits use the same path through `stage_commit`, so a failed enqueue
calls `reject`, which returns the `CommitToken` to the staging endpoint and
clears the pending state. The table can no longer be left permanently pending.

`ParamStore` loses its `<C: Controller, R: Rig>` parameters, `PhantomData`, the
23 `IDX_*` constants, the four-segment arithmetic repeated three times, and both
hundred-line matches. Groups are `StaticCell`-initialised in the experiment's
`main.rs`, the pattern already used for `SIN_LUT` and `CORE1_STACK`.

`ParamDef` gains a kind so blob handling stops being an `index == IDX_TABLE`
special case:

```rust
pub enum ParamKind {
    Scalar,
    Array(u16),   // fixed count, written whole
    Blob(u32),    // block-uploaded, commit-activated
}
```

### 3. Sound generic double buffering

The existing `firmware/common/src/table.rs` is correct because its call sites
are tightly controlled and its raw accessors are private. That discipline cannot
be exposed as a general safe API: `staging()` returning `&'static mut T` can be
called twice; `active()` returning `&'static T` can outlive the next activation;
an arbitrary `activate(id)` can publish a buffer being written; an arbitrary
`cancel_commit()` can clear a genuinely queued commit.

The fix is the same endpoint split the codebase already uses for its cross-core
queues (`rt_loop::init_channels` returns uniquely owned producer and consumer
endpoints):

```rust
// helic-core/src/double_buffer.rs
pub enum BufferError { Busy }   // local; firmware maps it to ErrorCode

pub struct DoubleBuffer<T> { buffers: [UnsafeCell<T>; 2], state: AtomicU8 }

// Sound only if T can cross cores. `T: 'static` alone is not sufficient.
unsafe impl<T: Send> Sync for DoubleBuffer<T> {}

/// Proof that a commit is outstanding. Constructible only by `Staging::commit`,
/// and consumed by exactly one of `Active::activate` or `Staging::cancel`.
#[derive(Clone, Copy, Debug)]
pub struct CommitToken(u8);

impl<T: Send> DoubleBuffer<T> {
    pub const fn new(a: T, b: T) -> Self;
    /// Split once into two uniquely owned endpoints, one per core.
    pub fn split(&'static self) -> (Staging<T>, Active<T>);
}

impl<T: Send> Staging<T> {
    /// Exclusive access to the inactive buffer. The borrow is tied to
    /// `&mut self`, so a second call cannot alias the first.
    pub fn buffer(&mut self) -> Result<&mut T, BufferError>;
    pub fn commit(&mut self) -> Result<CommitToken, BufferError>;
    pub fn cancel(&mut self, token: CommitToken);
}

impl<T: Send> Active<T> {
    /// Borrow the live buffer. Tied to `&self`, and `activate` takes
    /// `&mut self`, so no borrow can survive an activation.
    #[inline]
    pub fn get(&self) -> &T;
    /// Publish the pending buffer. Ignores a token that does not match the
    /// recorded pending id, so a stale or duplicated command is inert.
    pub fn activate(&mut self, token: CommitToken);
}
```

The four soundness properties, each enforced rather than documented:

- two simultaneous `&mut T` are impossible, because the borrow is tied to
  `&mut Staging<T>`;
- a live borrow cannot survive activation, because `get` borrows `&self` and
  `activate` requires `&mut self`;
- an arbitrary buffer cannot be activated, because `activate` requires a
  `CommitToken` and additionally verifies it against the pending state;
- a queued commit cannot be cancelled behind the queue's back, because the
  token is moved into the command and `cancel` consumes it.

`CommitToken` must cross cores inside `Payload`, so it is `Copy` plain data and
therefore forgeable within the crate. The pending-state check in `activate` is
the second line of defence and is what makes a duplicated or reordered command
inert rather than unsound.

`BufferError` is local to `helic-core`, which depends only on `libm`; returning
`helic_proto::ErrorCode` would have introduced a `helic-core → helic-proto`
edge that does not currently exist. Firmware maps `BufferError::Busy` to
`ErrorCode::Busy` at the `ParamGroup` boundary.

Core 1 holds `Active<WaveTable>` and calls `.get()` per tick instead of caching
a `&'static WaveTable` field. The call is `#[inline]` and resolves to one load,
but this sits inside the hot path and must be confirmed by ELF inspection.

Being in `helic-core` makes the state machine host-testable for the first time.

### 4. Rig, programme, and the MIMO safety contract

```rust
// helic-rt
pub const MAX_ACTUATORS: usize = 4;

pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];
    const ACTUATORS: &'static [(&'static str, &'static str)];
    const SAFETY_GATED: bool = false;

    fn measure(&mut self, values: &mut [f32]);
    fn actuate(&mut self, outputs: &[f32]);
    fn apply(&mut self, id: u16, payload: Payload);

    fn clamp_output(&self, _actuator: usize, value: f32) -> f32 { value }
    fn safe_output(&self, _actuator: usize) -> f32 { 0.0 }
    fn output_fault(&mut self, _inputs: &[f32]) -> bool { false }
}

pub trait Program {
    const OUTPUTS: usize;
    const SIGNALS: &'static [(&'static str, &'static str)];

    fn apply(&mut self, domain: u8, id: u16, payload: Payload);
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx, outputs: &mut [f32]);
    fn write_signals(&self, out: &mut [f32]);
}
```

`Rig` loses `type Ctrl`: a rig describes hardware, and how many controllers run
against it is the programme's business.

The vector safety contract, stated rather than cross-referenced, because
"semantics retained" is not a specification when the scalar interface it refers
to no longer exists:

- **Output buffer** is `[f32; MAX_ACTUATORS]`, with only the
  `R::ACTUATORS.len()` prefix used.
- **Setup asserts** `P::OUTPUTS == R::ACTUATORS.len()` and
  `P::OUTPUTS <= MAX_ACTUATORS`, as compile-time assertions where the constants
  permit and a boot assertion otherwise.
- **Faults are global and per-tick.** `output_fault(inputs)` is called once per
  tick and any fault latches one global trip that quiets *every* actuator.
  Per-axis trip state is a separate future design and must not be inferred from
  the output vector.
- **Clamping is per actuator index**, via `clamp_output(actuator, value)`.
- **Counters are per tick, not per actuator.** `SAFETY_QUIET_TICKS` increments
  once on a quieted tick; `SAFETY_CLAMP_TICKS` increments once if any output was
  clamped that tick. This preserves the meaning of the existing counters and the
  `safety` bitfield.
- **Streamed actuator values are the post-safety applied values**, matching
  today's behaviour, where streamed `out` is the post-gate value (`notes.md`,
  2026-07-18).
- **A non-gated rig applies every output verbatim** and the gate compiles away
  entirely, so `whirl-rig` and `pico2w-rig` stay unarmed and behaviourally
  unchanged.

The gate becomes a pure function in `helic-rt`, host-testable over the vector
cases:

```rust
pub fn safety_gate<R: Rig>(
    rig: &mut R, inputs: &[f32], commanded: &[f32], applied: &mut [f32],
) -> SafetyEvents;
```

### 5. Source assembly and shared constants

Source assembly becomes a generic walk over `R::INPUTS`, `P::SIGNALS`,
`R::ACTUATORS`, and `cmd_epoch`, so `GENERATED_SOURCES` disappears. For the
existing rigs this reproduces today's names and order exactly. Note that
`comms/tcp.rs:16` also consumes `source`/`source_count`, so it changes with
this stage.

`HARMONICS` moves from `firmware/common/src/lib.rs` to a const generic on the
programme, which already needs it for `FourierCoeffs<H>`. `MAX_TABLE_LEN`
becomes a const generic on `WaveTable<const N: usize = 4096>`, defaulted so
existing code is unaffected. Both matter for RAM budgeting once several
components own buffers.

## Double buffering: options considered

**The two patterns.** Small POD travels *inside* the queue element, which is
itself the double buffer. `WaveTable` at 16 KB is far too large for 32 slots and
needs a buffer swap. The ceiling between them is real: a 4×4 MIMO gain matrix of
Fourier banks would be 2.1 KB per slot and 67 KB for the queue, against 520 KB
total SRAM.

**A. Extend value-in-command.** Retained as the default for coefficient banks,
gains, and matrix rows, which is where MIMO parameters actually sit, and now
expressed as the non-generic `Payload::Values`.

**B. Generic `DoubleBuffer<T>` with split endpoints.** Recommended, as above.

**C. Per-object mailbox polled by core 1.** Drops the activation command
entirely and is immune to a full queue. Rejected because it decouples activation
from `cmd_epoch`: the guarantee that a table swap is visible in exactly the
record whose `cmd_epoch` reports that command is hardware-verified (`notes.md`,
2026-07-16, the live 0.45 V to 1.65 V re-commit, and the `forcing,out,cmd_epoch`
transition at sample 885064).

**D. `Pool<T, N>`.** The generalisation if several blobs must swap
independently. `DoubleBuffer<T>` is `Pool<T, 2>`, and the endpoint split above
extends to it unchanged. Not proposed now.

**RAM budget.** One `DoubleBuffer<WaveTable<4096>>` is 32 KB. Four per-axis
tables would be 128 KB on top of the ~130 KB already used. This is why
`MAX_TABLE_LEN` should become a const generic: a MIMO rig can then choose
`WaveTable<512>` and pay 4 KB per buffer. Without that, per-axis tables are not
affordable, and the constraint would surface only at link time.

## Examples

### Example 1: CBC after the change (behaviour identical)

```rust
// fw-cbc-rig/src/main.rs — assembly, once, at boot
let (staging, active) = TABLE.split();

let mut store = ParamStore::new(channels.command_tx, config::SAMPLE_RATE);
store.push(PLATFORM.init(PlatformGroup::new(config::EXPERIMENT)));
store.push(PROGRAM.init(StandardShadow::new(&program)));
store.push(TABLE_GROUP.init(TableShadow::new(staging)));
store.push(RIG.init(CbcShadow::new()));
store.push(TELEMETRY.init(CbcTelemetry::new()));
```

The discovered registry, source names, source order, and wire format are
unchanged. What changed is that the order is visible in one place in the
experiment crate rather than implied by arithmetic in the shared crate.

### Example 2: a two-axis MIMO rig

The programme lives in the rig's **portable** crate and is host-tested. Both
halves are in one file, sharing `mod ids` and a domain constant, so the core-0
and core-1 views cannot disagree.

```rust
// mimo-rig-program/src/two_axis.rs   (no_std, host-tested, no Embassy)
use helic_core::{ControlledAxis, FourierCoeffs, HarmonicGenerator, PidController};
use helic_rt::{ParamDef, ParamGroup, ParamType, Payload, Program, RtCommand, Staged, StepCtx};

const H: usize = 8;
const AXES: usize = 2;
const COEFFS: u16 = (1 + 2 * H) as u16;

pub const DOMAIN: u8 = 1;

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
    fn apply(&mut self, domain: u8, id: u16, payload: Payload) {
        if domain != DOMAIN { return; }
        match (id, payload) {
            (ids::FREQ, Payload::U32(inc)) => self.harmonics.set_increment(inc),
            (ids::REF0, Payload::Values { len, data }) =>
                self.axes[0].set_reference(FourierCoeffs::from_flat(&data[..len as usize])),
            (ids::REF1, Payload::Values { len, data }) =>
                self.axes[1].set_reference(FourierCoeffs::from_flat(&data[..len as usize])),
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
    pending: Option<Pending>,
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

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        let cmd = |payload| Staged::Rt(RtCommand { domain: DOMAIN, id, payload });
        match id {
            ids::FREQ => {
                let hz = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&hz) {
                    return Err(ErrorCode::BadValue);
                }
                self.pending = Some(Pending::Freq(hz));   // not yet observable
                Ok(cmd(Payload::U32(PhaseAccumulator::increment_for(
                    hz as f64, self.sample_rate.hz() as f64,
                ))))
            }
            ids::REF0 | ids::REF1 => {
                let coeffs = deserialize_coeffs::<H>(data)?;
                self.pending = Some(Pending::Ref((id - ids::REF0) as usize, coeffs));
                Ok(cmd(Payload::Values { len: COEFFS as u8, data: coeffs.to_flat() }))
            }
            ids::KP0 | ids::KP1 => {
                let v = f32::from_le_bytes(data.try_into().unwrap());
                if !v.is_finite() { return Err(ErrorCode::BadValue); }
                self.pending = Some(Pending::Kp((id - ids::KP0) as usize, v));
                Ok(cmd(Payload::F32(v)))
            }
            ids::RESET => Ok(cmd(Payload::Unit)),
            _ => Err(ErrorCode::BadIndex),
        }
    }

    /// Only here does a host-visible read change.
    fn accept(&mut self, _id: u16) {
        match self.pending.take() {
            Some(Pending::Freq(hz)) => self.freq_hz = hz,
            Some(Pending::Ref(i, c)) => self.refs[i] = c,
            Some(Pending::Kp(i, v)) => self.kp[i] = v,
            None => {}
        }
    }

    fn reject(&mut self, _id: u16) { self.pending = None; }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> { /* ... */ }
}
```

Files changed under `helic-core/`, `helic-rt/`, or `helic-fw-common/` to
support this rig: none. `fw-mimo-rig` contains only `board.rs`, `config.rs`,
`rig.rs`, and `main.rs`.

### Example 3: a table as an ordinary component

```rust
static TABLE: DoubleBuffer<WaveTable<4096>> =
    DoubleBuffer::new(WaveTable::empty(), WaveTable::empty());

pub const DOMAIN: u8 = 2;

impl ParamGroup for TableShadow {
    fn set_block(&mut self, id: u16, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        write_f32_block(self.staging.buffer().map_err(map_busy)?, offset, data)
    }

    fn stage_commit(&mut self, id: u16, len: u32) -> Result<Staged, ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        self.validate_prefix(len)?;                       // finite values, 2..=N
        let token = self.staging.commit().map_err(map_busy)?;
        self.pending_token = Some(token);
        Ok(Staged::Rt(RtCommand { domain: DOMAIN, id, payload: Payload::Buffer(token) }))
    }

    /// Enqueue failed: hand the buffer back. This is the rollback the previous
    /// revision had no way to express.
    fn reject(&mut self, _id: u16) {
        if let Some(token) = self.pending_token.take() { self.staging.cancel(token); }
    }

    fn accept(&mut self, _id: u16) { self.pending_token = None; }
}
```

Core 1's `apply` calls `active.activate(token)` at the sample boundary, with the
same `cmd_epoch` coherence as today.

### Example 4: what you touch to add a rig

| Task | Today | Proposed |
|---|---|---|
| New SISO rig, existing controller | rig crates only | rig crates only |
| New input source | rig crates only | rig crates only |
| New controller parameter | rig crates only | rig crates only |
| Second actuator | `rig.rs`, `rt_loop.rs`, `params.rs`, `schema.rs` | rig crates only |
| Second controlled axis | `rt_loop.rs`, `params.rs`, `schema.rs` | rig crates only |
| Rig with no waveform table | not expressible; params advertised and inert | omit the component |
| Second buffered blob | copy `table.rs` | one `DoubleBuffer<T>` static |
| Different harmonic count (≤16) | `firmware/common/src/lib.rs` | const generic on the programme |
| Shorter tables to save RAM | `helic-core/src/table.rs` | const generic on `WaveTable` |
| More than 24 sources, 4 actuators, 16 harmonics | shared crates | shared crates, deliberately |

## Repository separation

A rig repository contains `<rig>-program` and `fw-<rig>`, and needs:

1. `helic-core`, `helic-proto`, `helic-rt`, `helic-drivers`, and
   `helic-fw-common` published or referenced as git dependencies.
2. Its own `.cargo/config.toml` with the `thumbv8m.main-none-eabihf` target and
   runner, currently shared at `firmware/.cargo/config.toml`.
3. Its own `Cargo.lock`. The pinned Embassy set in `firmware/Cargo.toml`'s
   `[workspace.dependencies]` cannot be inherited across repositories.
4. A copy of, or a shared action for, the `check_rt_layout.py` gate.

Point 3 is the main friction. Pinning exact Embassy versions across
repositories that must interoperate with one `helic-fw-common` is a real
maintenance cost, while a version range weakens the "one tested dependency set"
property. A reasonable path is to keep the three production rigs here and treat
the crate boundary as the contract that *permits* an external rig, verified by
one out-of-workspace test rig rather than by moving everything at once.

## Relationship to the RT programme proposal

**Retained from `rt_program_proposal.md`:** `HarmonicFrame`/`HarmonicGenerator`
and one shared basis per tick; `FourierSignal`; `ControlledAxis<C, H>`; removal
of `PeriodicGenerator` and `GenSample`; the bounded logical output vector;
`Rig::ACTUATORS`; the `SAFETY_GATED` opt-out; all mandatory table-phase
semantics; the per-tick ordering.

**Superseded:** the slot-indexed `SetCoeffs`/`SetIncrement` scheme, which leaves
twelve other variants in place; `ParamStore<P, R>` remaining generic;
`RtProgram`'s parameter methods, replaced by the component's `ParamGroup` half;
retention of `GENERATED_SOURCES` and the table's special-cased command path.

The earlier proposal makes adding a coefficient bank free but leaves adding a
*kind* of thing costly; this makes both free at the cost of a larger initial
refactor.

## Migration plan

1. **`helic-rt` crate created**, initially by moving `Rig`, `TickSource`, and
   the parameter types out of `helic-fw-common` unchanged. No behaviour change;
   proves the portability boundary before anything depends on it.
2. **`DoubleBuffer<T>` with split endpoints into `helic-core`**, with host
   tests. Reimplement `table.rs` on top of it. Verifiable on hardware by
   repeating the existing table re-commit regression.
3. **`RtCommand`, `Payload`, and the size assertion**, with the existing fixed
   dispatch rewritten to match on `(domain, id, payload)` against the current
   `IDX_*` constants. No registry change yet.
4. **`ParamGroup` stage/accept/reject and the `ParamStore` walk.** Convert the
   four existing segments into groups reproducing the current registry exactly.
   Largest single commit; must change no discovered name or index.
5. **`Program` trait and `StandardProgram`**, absorbing phase, coefficients,
   controller, and table player out of `RtLoopState`. Split the table into its
   own group in the same change.
6. **Bounded output vector**, `Rig::ACTUATORS`, slice `actuate`, the vector
   safety gate, and generic source assembly (including `comms/tcp.rs`). Migrate
   all three rigs together.
7. **Const generics** for `HARMONICS` and `MAX_TABLE_LEN`, defaulted.
8. **An out-of-workspace test rig** as the final architectural acceptance test.

Stages 1 to 4 are worth doing regardless of whether MIMO arrives: they remove
the offset arithmetic and the bespoke unsafe module without changing any
externally visible behaviour.

## Tests

- **`DoubleBuffer`**: staging returns `Busy` while a commit is pending; a
  mismatched or stale `CommitToken` is inert; `cancel` restores writability; a
  commit rejected for a non-finite value leaves the active buffer untouched;
  compile-fail tests that two `buffer()` borrows and a `get()` borrow held
  across `activate()` are both rejected by the borrow checker.
- **Transactionality**: a full command queue leaves every shadow reading its
  pre-write value, for each parameter kind; a failed blob enqueue leaves the
  buffer writable and not pending. This is the direct regression test for the
  defect this revision fixes.
- **`ParamStore::locate`** over registries with zero-length groups, one group,
  and groups crossing a discovery page boundary.
- **Golden registry and golden source tests**: the assembled name list and index
  order for each of the three production rigs, asserted verbatim against the
  current registry. Primary regression guard for stages 4 to 6.
- **Command routing**: two groups using the same local id but different domains
  reach different components.
- **`Payload` round trip** for every kind, including `Values` reconstruction at
  `H = 8` and `H = 16`.
- **Vector safety gate**, host-tested: global trip quiets all actuators;
  per-actuator clamping; counters increment per tick not per actuator; a
  non-gated rig applies verbatim.
- **`size_of::<RtCommand>()`** compile-time assertion.

Hardware regression is the full sequential suite in the developer guide, since
stages 5 and 6 touch the entire tick calculation. The `cmd_epoch` coherence
tests for coefficient replacement and table re-commit are the specific evidence
stages 2 and 5 must not degrade; the 34 µs loop maximum with a fixed 36 µs wake
phase is the timing baseline.

## Risks and open questions

- **Larger change than the existing proposal**, touching a tick path with
  verified timing. Mitigated by stage ordering and the golden tests.
- **`Program` risks becoming a god-trait.** If it grows past step, signals, and
  apply, split it rather than letting policy drift back into `rt_loop.rs`.
- **Core-1 inlining is newly load-bearing.** A loop over axes may not unroll as
  today's straight-line code does. Confirm by ELF inspection and timing, not by
  reasoning about the source.
- **`Active::get()` per tick** replaces a cached field. Expected to be one
  inlined load; must be verified, not assumed.
- **`Payload` fixes a vocabulary.** Five variants covering everything current
  and foreseen is a judgement, not a proof.
- **`MAX_RT_VALUES = 33` caps harmonics at 16 for any programme.** Adequate
  today and consistent with the capacity table, but a programme wanting 32
  harmonics is a platform change, not a rig change.
- **Open:** should platform parameters be an ordinary group or a privileged
  first group? Ordinary is more uniform; first guarantees a stable prefix for
  host code that incorrectly caches indices.
- **Open:** does any planned MIMO controller need a payload wider than
  `MAX_RT_VALUES`? Decide before stage 3.

## Review responses

Revision 2 changes, each traceable to a review finding:

- **Parameter writes were no longer transactional.** The previous `set`
  mutated the shadow before the command was enqueued, so a `Busy` rejection
  left a read returning the rejected value, and a failed blob enqueue could
  leave the table permanently pending with no rollback path. Replaced by an
  explicit `stage`/`accept`/`reject` protocol with the ordering written once in
  `ParamStore::set`, so a group cannot invert it. Chosen over passing the
  command producer into each group because it makes the invariant structural
  rather than documented. Added a direct regression test.
- **The payload contradicted variable harmonic counts.**
  `Payload::Coeffs(FourierCoeffs<HARMONICS>)` retained the shared constant the
  proposal elsewhere removed, so the `H = 8` example could not have compiled.
  Replaced by non-generic `Values { len, data: [f32; MAX_RT_VALUES] }`, with
  `MAX_RT_VALUES` added to the published capacity table. Replaced the unfounded
  "queue element size unchanged" claim with a `size_of::<RtCommand>()`
  assertion.
- **The generic `DoubleBuffer<T>` safe API was unsound.** All six specifics were
  correct. Replaced with an SPSC-style `split` into uniquely owned `Staging` and
  `Active` endpoints, borrows tied to endpoint borrows, a non-forgeable
  `CommitToken` that `activate` additionally verifies against pending state, and
  `unsafe impl Sync for DoubleBuffer<T> where T: Send`. Error type is now local
  to `helic-core`, which depends only on `libm`; the previous signature would
  have created a new `helic-core → helic-proto` edge.
- **The MIMO safety contract was missing.** "Semantics retained" is not a
  specification when the scalar interface it refers to is being replaced. Now
  states vector clamping, global per-tick fault latching, per-tick counter
  semantics, `MAX_ACTUATORS`, the `P::OUTPUTS == R::ACTUATORS.len()` assertion,
  and that streamed actuator values are post-safety. The gate becomes a pure
  function in `helic-rt` so the vector cases are host-testable.
- **Command identifiers were not component-local.** Two programme groups could
  both emit local id 0. Added `domain` to the command address, as recommended,
  since it supports independently owned components rather than forcing one
  programme-global namespace.
- **Rig-local programme logic had no host-testable home.** Correct, and it
  violated developer-guide principle 4. Resolved more broadly than the minimum
  fix: the runtime contract types move to a new portable `helic-rt` crate, so a
  rig repository holds a host-tested `<rig>-program` library plus a thin
  `fw-<rig>` binary. Without that crate the contracts would stay in the
  RP2350-specific `helic-fw-common` and no rig programme could be host-tested.
- **The goal needed a capacity qualification.** Adopted the suggested wording
  and added an explicit capacity table, plus a statement that new reusable DSP
  or drivers properly belong in `helic-core`/`helic-drivers` and are not failed
  decoupling.
