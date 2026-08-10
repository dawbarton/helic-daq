# Rig decoupling: component-owned parameters, signals, and buffers

Status: proposed, not implemented. Revision 3. Supersedes parts of
`docs/rt_program_proposal.md` (see "Relationship to the RT programme
proposal"). Revision history and review responses are at the end.

## Goal

One goal, stated as a falsifiable test:

> Composing a rig from existing platform primitives, within documented source,
> output, group, payload, and memory bounds, must require changes only in that
> rig's own repository.

The qualification is not a hedge. Fixed capacities are part of a bounded
real-time platform, and raising one is a deliberate platform change with its own
timing and memory evidence. Equally, a genuinely new reusable DSP algorithm
belongs in `helic-core` and a new peripheral driver in `helic-drivers`; putting
either in a rig crate would be the failure mode, not the success case.

What the test excludes is the situation today, where a second actuator or a
second controlled axis forces edits to the shared command enum, the shared
parameter schema, and the shared source assembly.

### Documented platform capacities

| Bound | Value | Set by |
|---|---|---|
| `MAX_SOURCES` | 24 | protocol discovery headroom |
| `MAX_ACTUATORS` | 4 | record and safety-gate buffers |
| `MAX_GROUPS` | 8 | `ParamStore` registry vector |
| `MAX_RT_VALUES` | 132 | widest command payload; see "Target applications" |
| `COMMAND_QUEUE_LEN` | 32 | existing |
| `COMMANDS_PER_TICK` | 2 | existing WCET bound |

## Target applications and what they constrain

The intended use is experimental structural dynamics: modal testing, force
appropriation (phase-resonance or normal-mode testing), their nonlinear
extensions via phase-locked excitation, and control-based continuation. Those
applications, rather than MIMO in the abstract, fix several of the numbers above
and one interface rule.

### Atomic force-vector replacement sets `MAX_RT_VALUES`

Force appropriation drives `N` shakers with independent amplitude and phase at
one frequency, tuned so the structure responds in a single normal mode. A
partial update to that force vector is a transient in the appropriated mode
shape, which is a corrupted measurement, so the whole vector must take effect at
one sample boundary. The payload is `N · (1 + 2H)` values:

| shakers | harmonics | values | fits 33 | fits 132 |
|---|---|---|---|---|
| 4 | 1 | 12 | yes | yes |
| 4 | 3 | 28 | yes | yes |
| 4 | 5 | 44 | no | yes |
| 4 | 7 | 60 | no | yes |
| 8 | 5 | 88 | no | yes |
| 4 | 16 | 132 | no | yes |

Classical appropriation at the fundamental fits in 33. Multi-harmonic
appropriation does not, and that is exactly what nonlinear phase-resonance
testing needs, since isolating a nonlinear normal mode requires appropriating a
multi-harmonic force. `MAX_RT_VALUES = 33` would therefore have excluded the
main intended application.

`MAX_RT_VALUES = 132` costs `32 × ~540 = 17.3 KB` of queue SRAM, against
roughly 390 KB free after the current ~130 KB. Halving `COMMAND_QUEUE_LEN` to 16
recovers half of that if SRAM later becomes tight, and is the preferred lever
because queue depth is a latency convenience while payload width is a
capability. A `DoubleBuffer` for the force vector was considered and rejected:
at 528 bytes the object is small enough that a wider command is simpler than a
second state machine.

**Consequent WCET item.** Applying two commands per tick now copies up to
1056 bytes. On an aligned word copy this is a few microseconds against the
current 34 µs maximum, but it makes SRAM residency of the compiler's EABI
copy helpers more critical, not less. The layout gate already caught
flash-resident copy helpers once (`notes.md`, 2026-07-15, image `b35d4b8`);
`COMMANDS_PER_TICK` must be re-measured, not assumed, after this change.

### Autonomous programme state is not a parameter shadow

Phase-resonance testing closes a loop on the force–response phase and drives the
excitation frequency to maintain quadrature. The programme therefore writes the
master phase increment from inside `step()`, and the host's `freq` is a setpoint
or initial condition, not the truth.

This breaks an assumption the current registry embeds: that a writable
parameter's shadow *is* its value. The rule becomes:

> Any quantity the programme can change autonomously must be discoverable
> separately as a stream source or read-only telemetry. A writable parameter for
> the same quantity is a setpoint, and must be documented as one.

So a phase-locked programme exposes writable `freq_setpoint` and read-only
`freq_actual`, and never lets a host infer the second from the first.

### Slow modal quantities are telemetry, not sources

`MAX_SOURCES = 24` binds before compute does. A four-shaker rig streaming
per-channel amplitude and phase estimates would need roughly 31 sources. But
modal estimates change at about once per excitation period, so streaming them at
8 kHz is waste. Route them to `ExtraParam` atomics polled over TCP, the
mechanism `whirl-rig` already uses, and reserve stream sources for per-sample
quantities. The worked example below fits in 19 sources on that basis.

### Two-rate structure: the outer loop leaves the tick

Demodulation and force synthesis are `O(N·H)` per tick and belong on core 1. The
appropriation update law, which adjusts the force vector to drive a mode
indicator towards its target, is per-period and belongs on core 0 or the host. A
4×4 complex solve on a `period_start` tick would exceed the 125 µs budget
outright.

This matches existing practice, where control-based continuation already runs
host-side, and it preserves the WCET discipline: core 1 has no rare expensive
tick. Where a programme must branch on `period_start`, that branch is the
worst-case tick and must be bounded and measured.

### Hardware consequence: the vector `actuate` is required, not merely tidy

`helic-drivers/src/ad5064.rs:81` records ~LDAC tied low, so DAC channels are
write-and-update individually with `WORD_SETTLE_US = 3` µs between words
(`ad5064.rs:26`). Four shakers therefore see up to 9 µs of inter-channel skew,
which is 0.32° of phase at 100 Hz and 3.2° at 1 kHz. That is a material
appropriation error.

A multi-shaker rig wants ~LDAC strobed so all channels update simultaneously.
Passing the whole output vector to `Rig::actuate(&[f32])` in one call is what
allows the rig to write N channels and strobe once; a scalar interface cannot
express it. The vector boundary is justified by the hardware before it is
justified by the control structure.

### Two `helic-core` additions this implies

Both are legitimate shared-crate changes under the capacity rule, and neither
is rig-specific.

1. **`FourierEstimator::update_with(&HarmonicFrame<H>, signal)`.** The current
   `update` (`fourier.rs:50`) re-derives sin/cos per harmonic *per channel*, so
   eight response channels do eight times the redundant LUT work. The shared
   harmonic basis makes MIMO demodulation nearly free.
2. **A period-coherent estimator alongside the one-pole IIR.** The IIR has
   documented ripple, about 0.4% at τ = 2 s and f₀ = 20 Hz (`fourier.rs:80`).
   Appropriation convergence criteria want unbiased estimates; exact integration
   over one period, reset on `period_start`, settles in one period with no
   ripple.

### Safety consequences

- **A global trip that quiets every shaker simultaneously is correct here**, not
  a limitation. Asymmetric quieting of an appropriated force vector would itself
  excite modes.
- **Per-shaker force ratings differ**, which `clamp_output(actuator, ·)` covers.
- **Stroke is the gap.** Shaker displacement limits bind at low frequency and an
  amplitude clamp on force does not protect them. A multi-shaker rig needs a
  displacement-based `output_fault`, not only an output clamp.

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

Two separate problems motivate revisiting the crate boundaries.

**Portability.** The runtime *contract* types currently sit in the
RP2350-specific `helic-fw-common`, so any rig programme implementing them is
unhostable by construction. That constraint is what pushed control logic into a
firmware binary crate in revision 1, against developer-guide principle 4
("Logic is portable; wiring is local", `docs/developer_guide.md:24`).

**Discipline.** `helic-fw-common` also conflates the two cores. It holds `net/`,
`comms/`, `laser.rs`, and `time_watchdog.rs` (core 0: Embassy, async, flash
resident) alongside `rt_loop.rs`, `rt_mem.rs`, `analog_spi.rs`, and the PIO
modules (core 1: SRAM, no Embassy, bounded WCET). Those are opposite
disciplines in one dependency set, and `docs/overrun_handoff.md` records an
entire debugging saga caused by core-0 work reaching the tick through the shared
XIP cache.

The split is clean in practice: `rt_loop.rs`'s only `embassy-time` use is
`status_run` (line 7), a core-0 task, and `defmt` appears only in setup and that
same task. Moving `status_run` out leaves `embassy-rp` for `pac::TIMER0` plus
`heapless` and `static_cell`.

| Crate | Contents | Testable |
|---|---|---|
| `helic-proto` | wire protocol, `ErrorCode`; broker protocol feature-gated | host |
| `helic-core` | DSP: generators, controllers, estimators, filters, tables, `DoubleBuffer` | host |
| **`helic-rt`** (new) | `Rig`, `TickSource`, `Program`, `ParamGroup`, `ParamDef`, `Payload`, `RtCommand`, `ParamStore`, safety gate, source assembly | host |
| **`helic-fw-rt`** (new) | core 1: tick sources, `rt_mem`, `analog_spi`, PIO transports, the loop driver | cross-build |
| **`helic-fw-net`** (new) | core 0: `net/`, `comms/`, `time_watchdog`, `status_run` | cross-build |
| `helic-drivers` | chip and sensor logic, pure and host-testable | host |
| `<rig>-program` | that rig's programme, controllers, and parameter shadows | host |
| `fw-<rig>` | that rig's `board.rs`, `config.rs`, `rig.rs`, `main.rs` | cross-build |

A rig repository contains `<rig>-program` and `fw-<rig>`, and its control logic
is `cargo test`-able on the host exactly as `helic-core`'s is. This is still one
codebase per rig.

### Dependency rules this makes enforceable

The value of the split is that these become checkable in CI rather than
maintained by review:

- `helic-core` depends only on `libm`; no protocol, no Embassy.
- `helic-rt` depends on `helic-core`, `helic-proto`, and `heapless`; no Embassy
  at all.
- `helic-fw-rt` depends on `embassy-rp` (for `pac`) but **not** on
  `embassy-net`, `embassy-time`, or `embassy-executor`.
- `helic-fw-net` may not depend on `helic-fw-rt` except through the queue
  endpoint types.

### Two further placement questions

- **`helic-core::rpm`** has exactly one consumer, `whirl-rig` (`rig.rs:10`). Is
  period-to-RPM estimation with staleness reusable DSP, or whirl's experiment
  logic? I would keep it in `helic-core`, since it is generic rotating-machinery
  code, but it is the case the new rule has to adjudicate and the decision
  should be explicit.
- **Device integrations are misfiled as "common".** `laser.rs` is an Embassy
  UART task for one sensor used by one rig, sitting in a crate every rig
  depends on. The portable split itself is right, since
  `helic-drivers/optoncdt.rs` is a pure parser, but the rule should be that
  `helic-fw-*` holds mechanisms *every* rig uses, and a mechanism *some* rig
  uses belongs in a device-integration crate or that rig's repository.
  `analog_spi.rs` deserves the same question.

`helic-core::safety` keeps the primitives (`clamp_channel_command`,
`StaleCounter`); the vector gate that composes them lives in `helic-rt`.

## Design

### 1. One command address, one payload vocabulary

```rust
// helic-rt
pub const MAX_RT_VALUES: usize = 132;  // 4 actuators × (1 + 2·16) harmonics
pub const DOMAIN_RIG: u8 = 0;          // reserved; programmes use 1..

#[derive(Clone, Copy, Debug)]
pub enum Payload {
    Unit,                                          // triggers
    F32(f32),
    U32(u32),                                      // increments, modes, multipliers
    Values { len: u8, data: [f32; MAX_RT_VALUES] },// coefficient banks, force vectors
    Buffer(CommitToken),                           // blob activation
}

#[derive(Clone, Copy, Debug)]
pub struct RtCommand {
    pub domain: u8,
    pub id: u16,
    pub payload: Payload,
}

// The queue is COMMAND_QUEUE_LEN of these; assert rather than assume.
const _: () = assert!(core::mem::size_of::<RtCommand>() <= 560);
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

`Values` is a flat, length-tagged array rather than a `FourierCoeffs<H>`
variant, because the payload must be non-generic to keep `RtCommand` and the
queue non-generic while `HARMONICS` becomes a per-programme const generic (§5).
The receiving component reconstructs its own typed value, since the
`(domain, id)` pair already determines the meaning. One `Values` command can
therefore carry a complete multi-actuator force vector, which is what makes
atomic appropriation updates possible.

Enum-valued parameters are still validated into a concrete Rust type on core 0
and travel as their `u32` discriminant, so core 1 never parses an unvalidated
host value.

### 2. Transactional writes, ordered centrally

The current code enqueues before updating the shadow, and rolls back a blob
commit if the enqueue fails (`firmware/common/src/params.rs:596,623`). That
ordering is a correctness property: a host receiving `Busy` must not then read
back the value it was denied. Distributing `set` to components must not
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

The ordering is written exactly once, in the store:

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

Blob commits use the same path through `stage_commit`, so a failed enqueue calls
`reject`, which returns the `CommitToken` and clears the pending state. The
table can no longer be left permanently pending.

`ParamStore` loses its `<C: Controller, R: Rig>` parameters, `PhantomData`, the
23 `IDX_*` constants, the four-segment arithmetic repeated three times, and both
hundred-line matches. Groups are `StaticCell`-initialised in the experiment's
`main.rs`.

`ParamDef` gains a kind so blob handling stops being an `index == IDX_TABLE`
special case:

```rust
pub enum ParamKind { Scalar, Array(u16), Blob(u32) }
```

### 3. Sound generic double buffering

The existing `firmware/common/src/table.rs` is correct because its call sites are
tightly controlled and its raw accessors are private. That discipline cannot be
exposed as a general safe API: `staging()` returning `&'static mut T` can be
called twice; `active()` returning `&'static T` can outlive the next activation;
an arbitrary `activate(id)` can publish a buffer being written; an arbitrary
`cancel_commit()` can clear a genuinely queued commit.

The fix is the endpoint split the codebase already uses for its cross-core
queues (`rt_loop::init_channels`):

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

Four soundness properties, each enforced rather than documented:

- two simultaneous `&mut T` are impossible, because the borrow is tied to
  `&mut Staging<T>`;
- a live borrow cannot survive activation, because `get` borrows `&self` and
  `activate` requires `&mut self`;
- an arbitrary buffer cannot be activated, because `activate` requires a
  `CommitToken` and verifies it against the pending state;
- a queued commit cannot be cancelled behind the queue's back, because the token
  is moved into the command and `cancel` consumes it.

`CommitToken` crosses cores inside `Payload`, so it is `Copy` plain data and
therefore forgeable within the crate. The pending-state check in `activate` is
the second line of defence, making a duplicated or reordered command inert
rather than unsound.

`BufferError` is local to `helic-core`, which depends only on `libm`; returning
`helic_proto::ErrorCode` would have introduced a `helic-core → helic-proto` edge
that does not currently exist.

Core 1 holds `Active<WaveTable>` and calls `.get()` per tick instead of caching a
`&'static WaveTable` field. The call is `#[inline]` and should resolve to one
load, but it sits in the hot path and must be confirmed by ELF inspection.

### 4. Rig, programme, and the MIMO safety contract

```rust
// helic-rt
pub const MAX_ACTUATORS: usize = 4;

pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];
    const ACTUATORS: &'static [(&'static str, &'static str)];
    const SAFETY_GATED: bool = false;

    fn measure(&mut self, values: &mut [f32]);
    /// All actuators in one call, so a rig can write N channels and strobe
    /// ~LDAC once. See "Hardware consequence" above.
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

The vector safety contract, stated rather than cross-referenced:

- **Output buffer** is `[f32; MAX_ACTUATORS]`, with only the
  `R::ACTUATORS.len()` prefix used.
- **Setup asserts** `P::OUTPUTS == R::ACTUATORS.len()` and
  `P::OUTPUTS <= MAX_ACTUATORS`.
- **Faults are global and per-tick.** `output_fault(inputs)` is called once per
  tick and any fault latches one global trip that quiets *every* actuator. For
  multi-shaker excitation this is the correct behaviour, not a limitation:
  asymmetric quieting of an appropriated force vector would itself excite modes.
  Per-axis trip state is a separate future design.
- **Clamping is per actuator index**, via `clamp_output(actuator, value)`, which
  accommodates differing shaker force ratings.
- **Counters are per tick, not per actuator.** `SAFETY_QUIET_TICKS` increments
  once on a quieted tick; `SAFETY_CLAMP_TICKS` increments once if any output was
  clamped that tick. This preserves the existing counters and `safety` bitfield.
- **Streamed actuator values are the post-safety applied values**, matching
  today's behaviour (`notes.md`, 2026-07-18).
- **A non-gated rig applies every output verbatim** and the gate compiles away,
  so `whirl-rig` and `pico2w-rig` stay unarmed and behaviourally unchanged.

The gate becomes a pure function in `helic-rt`, host-testable over the vector
cases:

```rust
pub fn safety_gate<R: Rig>(
    rig: &mut R, inputs: &[f32], commanded: &[f32], applied: &mut [f32],
) -> SafetyEvents;
```

### 5. Source assembly and shared constants

Source assembly becomes a generic walk over `R::INPUTS`, `P::SIGNALS`,
`R::ACTUATORS`, and `cmd_epoch`, so `GENERATED_SOURCES` disappears. For existing
rigs this reproduces today's names and order exactly. `comms/tcp.rs:16` also
consumes `source`/`source_count`, so it changes with this stage.

`HARMONICS` moves to a const generic on the programme. `MAX_TABLE_LEN` becomes a
const generic on `WaveTable<const N: usize = 4096>`, defaulted so existing code
is unaffected. Both matter for RAM budgeting once several components own
buffers.

## Double buffering: options considered

**The two patterns.** Small POD travels inside the queue element, which is
itself the double buffer. `WaveTable` at 16 KB is far too large for 32 slots and
needs a buffer swap.

**A. Value-in-command.** Retained as the default for coefficient banks, gains,
and force vectors, now expressed as the non-generic `Payload::Values` and sized
by the appropriation requirement above.

**B. Generic `DoubleBuffer<T>` with split endpoints.** Recommended for blobs.

**C. Per-object mailbox polled by core 1.** Drops the activation command and is
immune to a full queue. Rejected because it decouples activation from
`cmd_epoch`: the guarantee that a table swap is visible in exactly the record
whose `cmd_epoch` reports that command is hardware-verified (`notes.md`,
2026-07-16, the live 0.45 V to 1.65 V re-commit, and the `forcing,out,cmd_epoch`
transition at sample 885064).

**D. `Pool<T, N>`.** The generalisation if several blobs must swap
independently. `DoubleBuffer<T>` is `Pool<T, 2>` and the endpoint split extends
unchanged. Not proposed now.

**RAM budget.** One `DoubleBuffer<WaveTable<4096>>` is 32 KB. Four per-axis
tables would be 128 KB on top of the ~130 KB already used, which is why
`MAX_TABLE_LEN` should become a const generic: a MIMO rig can choose
`WaveTable<512>` and pay 4 KB per buffer.

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

### Example 2: force appropriation, entirely in the rig's own crates

Four shakers, four response channels, phase-locked excitation. This exercises
atomic vector update, autonomous frequency, and the two-rate split together.

```rust
// appropriation-program/src/lib.rs   (no_std, host-tested, no Embassy)
use helic_core::{FourierCoeffs, FourierEstimator, FourierSignal, HarmonicGenerator, Pll};
use helic_rt::{ParamDef, ParamGroup, ParamType, Payload, Program, RtCommand, Staged, StepCtx};

const SHAKERS: usize = 4;
const RESPONSES: usize = 4;
const H: usize = 5;
const VECTOR_LEN: usize = SHAKERS * (1 + 2 * H);   // 44 values

pub const DOMAIN: u8 = 1;

mod ids {
    pub const FREQ_SETPOINT: u16 = 0;   // setpoint only; PLL owns the truth
    pub const FORCE_VECTOR: u16 = 1;    // all shakers, all harmonics, atomic
    pub const PLL_GAIN: u16 = 2;
    pub const TARGET_PHASE: u16 = 3;    // −90° for phase resonance
    pub const EXCITATION_MODE: u16 = 4; // fixed frequency | phase-locked
    pub const RESET: u16 = 5;
}

// ---- core 1 -------------------------------------------------------------

pub struct Appropriation {
    harmonics: HarmonicGenerator<H>,
    forces: [FourierSignal<H>; SHAKERS],
    responses: [FourierEstimator<H>; RESPONSES],
    pll: Pll,
    mode: ExcitationMode,
    commanded: [f32; SHAKERS],
    freq_actual: f32,
    phase_error: f32,
}

impl Program for Appropriation {
    const OUTPUTS: usize = SHAKERS;
    // Per-sample quantities only. Modal estimates are telemetry, not sources.
    const SIGNALS: &'static [(&'static str, &'static str)] = &[
        ("f0_cmd", "N"), ("f1_cmd", "N"), ("f2_cmd", "N"), ("f3_cmd", "N"),
        ("freq_actual", "Hz"), ("phase_error", "deg"),
    ];

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply(&mut self, domain: u8, id: u16, payload: Payload) {
        if domain != DOMAIN { return; }
        match (id, payload) {
            // One command replaces the entire force vector at one sample
            // boundary: a partial update would corrupt the mode shape.
            (ids::FORCE_VECTOR, Payload::Values { len, data }) => {
                if len as usize != VECTOR_LEN { return; }
                for (s, force) in self.forces.iter_mut().enumerate() {
                    let base = s * (1 + 2 * H);
                    force.set_coefficients(FourierCoeffs::from_flat(
                        &data[base..base + 1 + 2 * H],
                    ));
                }
            }
            (ids::FREQ_SETPOINT, Payload::U32(inc)) => self.harmonics.set_increment(inc),
            (ids::PLL_GAIN, Payload::F32(v)) => self.pll.set_gain(v),
            (ids::TARGET_PHASE, Payload::F32(v)) => self.pll.set_target_phase(v),
            (ids::EXCITATION_MODE, Payload::U32(m)) => self.mode = ExcitationMode::from_u32(m),
            (ids::RESET, _) => self.reset(),
            _ => {}
        }
    }

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx, outputs: &mut [f32]) {
        let frame = self.harmonics.step(ctx.lut);

        // O(N·H) demodulation against the shared basis: no redundant LUT work.
        for (i, est) in self.responses.iter_mut().enumerate() {
            est.update_with(frame, inputs[SHAKERS + i]);
        }

        // O(1) per tick. The appropriation law that adjusts the force vector
        // is per-period and runs on core 0 or the host; it is not here.
        if self.mode == ExcitationMode::PhaseLocked {
            self.phase_error = self.pll.phase_error(&self.responses[0], frame);
            self.harmonics.set_increment(self.pll.update(self.phase_error, dt));
        }
        self.freq_actual = self.harmonics.frequency_hz(ctx.sample_rate);

        for (j, force) in self.forces.iter().enumerate() {
            self.commanded[j] = force.sample(frame);
            outputs[j] = self.commanded[j];
        }
    }

    fn write_signals(&self, out: &mut [f32]) {
        out[..SHAKERS].copy_from_slice(&self.commanded);
        out[SHAKERS] = self.freq_actual;
        out[SHAKERS + 1] = self.phase_error;
    }
}
```

The core-0 half validates the whole vector before staging it:

```rust
impl ParamGroup for AppropriationShadow {
    fn params(&self) -> &'static [ParamDef] {
        &[
            ParamDef::writable("freq_setpoint", ParamType::F32, 1),
            ParamDef::writable("force_vector", ParamType::F32, VECTOR_LEN as u16),
            ParamDef::writable("pll_gain", ParamType::F32, 1),
            ParamDef::writable("target_phase", ParamType::F32, 1),
            ParamDef::writable("excitation_mode", ParamType::U32, 1),
            ParamDef::writable("ctrl_reset", ParamType::U32, 1),
        ]
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        let cmd = |payload| Staged::Rt(RtCommand { domain: DOMAIN, id, payload });
        match id {
            ids::FORCE_VECTOR => {
                let values = deserialize_f32s::<VECTOR_LEN>(data)?;   // rejects non-finite
                self.pending = Some(Pending::Force(values));
                Ok(cmd(Payload::Values { len: VECTOR_LEN as u8, data: pad(values) }))
            }
            // ... remaining ids
            _ => Err(ErrorCode::BadIndex),
        }
    }

    fn accept(&mut self, _id: u16) { /* publish self.pending into the shadow */ }
    fn reject(&mut self, _id: u16) { self.pending = None; }
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> { /* ... */ }
}
```

Note three things this example establishes.

**`freq_setpoint` is writable but is not the value.** `freq_actual` is a
separate stream source because the PLL owns the truth, per the autonomous-state
rule. A host must never infer the second from the first.

**Per-channel modal estimates are not sources.** Response amplitude and phase
per channel per harmonic would be 40 additional sources. They change at about
once per period, so they are published to `ExtraParam` atomics and polled over
TCP instead. The source budget is then 8 inputs, 6 programme signals, 4
actuators, and `cmd_epoch`, which is 19 of 24.

**Files changed under `helic-core`, `helic-rt`, `helic-fw-rt`, or
`helic-fw-net` to support this rig: none**, given `update_with` and `Pll` exist
as shared primitives. `fw-appropriation-rig` holds only `board.rs`, `config.rs`,
`rig.rs`, and `main.rs`.

### Example 3: a table as an ordinary component

```rust
static TABLE: DoubleBuffer<WaveTable<4096>> =
    DoubleBuffer::new(WaveTable::empty(), WaveTable::empty());

impl ParamGroup for TableShadow {
    fn set_block(&mut self, id: u16, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        write_f32_block(self.staging.buffer().map_err(map_busy)?, offset, data)
    }

    fn stage_commit(&mut self, id: u16, len: u32) -> Result<Staged, ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        self.validate_prefix(len)?;
        let token = self.staging.commit().map_err(map_busy)?;
        self.pending_token = Some(token);
        Ok(Staged::Rt(RtCommand { domain: DOMAIN, id, payload: Payload::Buffer(token) }))
    }

    /// Enqueue failed: hand the buffer back.
    fn reject(&mut self, _id: u16) {
        if let Some(token) = self.pending_token.take() { self.staging.cancel(token); }
    }

    fn accept(&mut self, _id: u16) { self.pending_token = None; }
}
```

### Example 4: what you touch to add a rig

| Task | Today | Proposed |
|---|---|---|
| New SISO rig, existing controller | rig crates only | rig crates only |
| New input source | rig crates only | rig crates only |
| New controller parameter | rig crates only | rig crates only |
| Second actuator | `rig.rs`, `rt_loop.rs`, `params.rs`, `schema.rs` | rig crates only |
| Second controlled axis | `rt_loop.rs`, `params.rs`, `schema.rs` | rig crates only |
| Four-shaker appropriation rig | not expressible | rig crates only |
| Rig with no waveform table | not expressible; params advertised and inert | omit the component |
| Second buffered blob | copy `table.rs` | one `DoubleBuffer<T>` static |
| Different harmonic count (≤16) | `firmware/common/src/lib.rs` | const generic on the programme |
| Shorter tables to save RAM | `helic-core/src/table.rs` | const generic on `WaveTable` |
| >24 sources, >4 actuators, >132 payload values | shared crates | shared crates, deliberately |
| New reusable estimator or controller | `helic-core` | `helic-core`, correctly |

## Repository separation

A rig repository contains `<rig>-program` and `fw-<rig>`, and needs:

1. `helic-core`, `helic-proto`, `helic-rt`, `helic-fw-rt`, `helic-fw-net`, and
   `helic-drivers` published or referenced as git dependencies.
2. Its own `.cargo/config.toml` with the `thumbv8m.main-none-eabihf` target and
   runner, currently shared at `firmware/.cargo/config.toml`.
3. Its own `Cargo.lock`. The pinned Embassy set in `firmware/Cargo.toml`'s
   `[workspace.dependencies]` cannot be inherited across repositories.
4. A copy of, or a shared action for, the `check_rt_layout.py` gate.

Point 3 is the main friction. Pinning exact Embassy versions across repositories
that must interoperate with one `helic-fw-rt` is a real maintenance cost, while
a version range weakens the "one tested dependency set" property. A reasonable
path is to keep the three production rigs here and treat the crate boundary as
the contract that *permits* an external rig, verified by one out-of-workspace
test rig rather than by moving everything at once.

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

Note that `rt_program_proposal.md` anticipated the atomic multi-axis update
requirement ("atomic multi-axis coefficient replacement should use a complete
copied bank or a double-buffered bank swap") but deferred it. Sizing
`MAX_RT_VALUES` from the appropriation case resolves it in favour of the copied
bank.

## Migration plan

1. **`helic-rt` crate created**, initially by moving `Rig`, `TickSource`, and
   the parameter types out of `helic-fw-common` unchanged. Proves the
   portability boundary before anything depends on it.
2. **`helic-fw-common` split** into `helic-fw-rt` and `helic-fw-net`, with the
   dependency rules added to CI. Mechanical; no behaviour change.
3. **`DoubleBuffer<T>` with split endpoints into `helic-core`**, with host
   tests. Reimplement `table.rs` on top of it. Verifiable on hardware by
   repeating the existing table re-commit regression.
4. **`RtCommand`, `Payload`, and the size assertion**, with existing dispatch
   rewritten to match on `(domain, id, payload)`. Re-measure `COMMANDS_PER_TICK`
   WCET at the new payload width and confirm EABI copy helpers remain in SRAM.
5. **`ParamGroup` stage/accept/reject and the `ParamStore` walk.** Convert the
   four existing segments into groups reproducing the current registry exactly.
   Largest single commit; must change no discovered name or index.
6. **`Program` trait and `StandardProgram`**, absorbing phase, coefficients,
   controller, and table player out of `RtLoopState`. Split the table into its
   own group in the same change.
7. **Bounded output vector**, `Rig::ACTUATORS`, slice `actuate`, the vector
   safety gate, and generic source assembly (including `comms/tcp.rs`). Migrate
   all three rigs together.
8. **Const generics** for `HARMONICS` and `MAX_TABLE_LEN`, defaulted.
9. **`helic-core` additions for modal work**: `FourierEstimator::update_with`
   and a period-coherent estimator. Independent of stages 1 to 8.
10. **An out-of-workspace test rig** as the final architectural acceptance test.

Stages 1 to 5 are worth doing regardless of whether MIMO arrives: they remove
the offset arithmetic and the bespoke unsafe module without changing any
externally visible behaviour.

## Tests

- **`DoubleBuffer`**: staging returns `Busy` while a commit is pending; a
  mismatched or stale `CommitToken` is inert; `cancel` restores writability; a
  commit rejected for a non-finite value leaves the active buffer untouched;
  compile-fail tests that two `buffer()` borrows, and a `get()` borrow held
  across `activate()`, are both rejected by the borrow checker.
- **Transactionality**: a full command queue leaves every shadow reading its
  pre-write value, for each parameter kind; a failed blob enqueue leaves the
  buffer writable and not pending.
- **Atomic force vector**: a `Values` command carrying all `SHAKERS` banks
  updates every `FourierSignal` at the same tick, and a length mismatch updates
  none.
- **`ParamStore::locate`** over registries with zero-length groups, one group,
  and groups crossing a discovery page boundary.
- **Golden registry and golden source tests**: assembled name list and index
  order for each production rig, asserted verbatim. Primary regression guard for
  stages 5 to 7.
- **Command routing**: two groups with the same local id but different domains
  reach different components.
- **`Payload` round trip** for every kind, including `Values` at 33 and 132
  lengths.
- **Vector safety gate**, host-tested: global trip quiets all actuators;
  per-actuator clamping; counters per tick not per actuator; non-gated rig
  applies verbatim.
- **`size_of::<RtCommand>()`** compile-time assertion, and a measured
  `COMMANDS_PER_TICK` WCET at full payload width.
- **Dependency rules**: a CI check that `helic-rt` has no Embassy dependency and
  `helic-fw-rt` has no `embassy-net`/`embassy-time`/`embassy-executor`.
- **Estimator equivalence**: `update_with` matches `update` sample for sample;
  the period-coherent estimator is unbiased on a known multi-harmonic signal.

Hardware regression is the full sequential suite in the developer guide, since
stages 6 and 7 touch the entire tick calculation. The `cmd_epoch` coherence
tests for coefficient replacement and table re-commit are the specific evidence
stages 3 and 6 must not degrade; the 34 µs loop maximum with a fixed 36 µs wake
phase is the timing baseline. Stage 4 additionally requires a fresh loop-maximum
measurement, since the payload width changes per-tick copy cost.

## Risks and open questions

- **Larger change than the existing proposal**, touching a tick path with
  verified timing. Mitigated by stage ordering and the golden tests.
- **`Program` risks becoming a god-trait.** If it grows past step, signals, and
  apply, split it rather than letting policy drift back into the loop.
- **Core-1 inlining is newly load-bearing.** A loop over axes or shakers may not
  unroll as today's straight-line code does. Confirm by ELF inspection and
  timing, not by reasoning about the source.
- **The wider payload is the main new timing risk.** Two 528-byte copies per
  tick is a few microseconds in principle, but the EABI copy-helper residency
  problem has bitten once already.
- **`Active::get()` per tick** replaces a cached field; expected to be one
  inlined load, must be verified.
- **`MAX_RT_VALUES = 132` caps a single atomic update at 4 actuators × 16
  harmonics.** Adequate for the intended appropriation work; an 8-shaker
  multi-harmonic rig is a platform change.
- **Open:** should platform parameters be an ordinary group or a privileged
  first group? Ordinary is more uniform; first guarantees a stable prefix for
  host code that incorrectly caches indices.
- **Open:** does `helic-core::rpm` stay, or move to `whirl-rig-program`? The
  answer defines where the reusable/experiment line actually falls.
- **Open:** the PLL and mode-indicator primitives sketched in Example 2 do not
  exist yet. Whether they are `helic-core` primitives or rig-local logic should
  be settled before an appropriation rig is built, not during.

## Revision history

**Revision 3** folds in a crate-boundary review and the constraints implied by
the target applications:

- **Split `helic-fw-common` into `helic-fw-rt` and `helic-fw-net`.** It
  conflated core-0 and core-1 disciplines in one dependency set. The split makes
  "no Embassy on the tick path" a checkable dependency rule rather than a review
  rule, which matters given the flash/XIP contention history in
  `docs/overrun_handoff.md`. Verified clean: `rt_loop.rs`'s only `embassy-time`
  use is the core-0 `status_run`.
- **Added explicit dependency rules** for CI, plus two open placement questions
  (`helic-core::rpm`, and device integrations such as `laser.rs` sitting in a
  crate every rig depends on).
- **Raised `MAX_RT_VALUES` from 33 to 132.** Sized from force appropriation:
  33 admits only single-harmonic appropriation, excluding the multi-harmonic
  case that nonlinear phase-resonance testing requires. This closes the open
  question left in revision 2 in favour of a wider command rather than a
  `DoubleBuffer`, and adds a WCET item for the wider per-tick copy.
- **Added the autonomous-state rule.** Phase-locked excitation means the
  programme owns the frequency, so a writable parameter's shadow is no longer
  necessarily its value. Autonomous quantities must be separately discoverable.
- **Added the two-rate guideline**: demodulation and synthesis on core 1, the
  per-period appropriation law on core 0 or the host.
- **Justified the vector `actuate` from hardware**: ~LDAC tied low gives up to
  9 µs inter-channel skew across four channels, which is 3.2° at 1 kHz.
- **Added a force-appropriation worked example** replacing the generic two-axis
  one, since it exercises atomic vector update, autonomous frequency, and the
  two-rate split together.
- **Added two `helic-core` items**: `FourierEstimator::update_with` for the
  shared basis, and a period-coherent estimator alongside the IIR.
- **Recorded that the global safety trip is correct for multi-shaker work**, and
  that stroke limiting is a gap an amplitude clamp does not cover.

**Revision 2** responded to a review. Each finding was confirmed against the
code and accepted:

- **Parameter writes were not transactional.** The previous `set` mutated the
  shadow before enqueue, so a `Busy` rejection left a read returning the
  rejected value, and a failed blob enqueue could strand the table pending with
  no rollback. Replaced by `stage`/`accept`/`reject` with the ordering written
  once in `ParamStore::set`, chosen over passing the producer into each group
  because it makes the invariant structural rather than documented.
- **The payload contradicted variable harmonic counts.**
  `Payload::Coeffs(FourierCoeffs<HARMONICS>)` retained a constant the proposal
  elsewhere removed, so the example could not have compiled. Replaced by
  non-generic `Values`, and the unfounded "queue size unchanged" claim replaced
  by a `size_of` assertion.
- **The generic `DoubleBuffer<T>` safe API was unsound**, in all six ways
  identified. Replaced with an SPSC-style endpoint split, borrows tied to
  endpoint borrows, a `CommitToken` verified against pending state, and
  `Sync` bounded on `T: Send`. Error type made local, since `helic-core`
  depends only on `libm`.
- **The MIMO safety contract was missing.** Now states vector clamping, global
  per-tick fault latching, per-tick counters, `MAX_ACTUATORS`, the
  `P::OUTPUTS == R::ACTUATORS.len()` assertion, and post-safety streaming.
- **Command identifiers were not component-local.** Added `domain` to the
  command address.
- **Rig-local programme logic had no host-testable home.** Resolved by moving
  the contract types to the portable `helic-rt` crate.
- **The goal needed a capacity qualification.** Adopted, with an explicit
  capacity table.
