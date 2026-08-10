# Rig decoupling: component-owned parameters, signals, and buffers

Status: proposed, not implemented. Revision 4. Supersedes parts of
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
and two interface rules.

### Where computation lives: the host by default

The governing principle is that work belongs on the host unless something forces
it onto the device. Two things force it onto the device: sample-rate synthesis,
and feedback loops whose bandwidth the network cannot carry. Everything else,
including all measurement-side processing, is host work.

This is already the codebase's practice rather than a new policy.
`FourierEstimator` exists in `helic-core` with unit tests and **no firmware
consumer at all**; control-based continuation already runs host-side. The
division for an appropriation rig is therefore:

| Runs on core 1 | Runs on the host |
|---|---|
| force-vector synthesis at sample rate | multi-channel, multi-harmonic response estimation |
| one-channel fundamental demodulation, inside the PLL | mode indicator functions |
| the PLL loop filter and NCO | the appropriation update law |
| the safety gate | all reprocessing and offline analysis |

Sustained raw streaming is hardware-proven, not assumed: `notes.md:50` records
960 000 records of all 13 CBC sources over 120.25 s at 8 kHz with zero UDP loss,
zero record drops, and a 35 µs loop maximum. A four-shaker rig needs 17 sources
at roughly 3.6 Mbit/s, which is the configuration already verified.

Host-side estimation is also scientifically better: `f64` rather than `f32`,
exact per-period block integration rather than a one-pole IIR with documented
ripple (`fourier.rs:80`), proper windowing, and the raw time series retained for
reprocessing. At about 1.6 GB/hour to the broker's HDF5 that is unremarkable
storage, and far more useful than keeping only reduced estimates.

**Why the PLL is the exception.** It is a feedback loop, so putting the network
inside it costs phase margin. A phase estimate needs at least one excitation
period regardless of where it runs, so 50 ms at 20 Hz is the floor; nonlinear
phase-resonance testing typically runs loop bandwidths of order 0.1–1 Hz. A
50 ms round trip is then about 18° of phase margin at a 1 Hz crossover and under
2° at 0.1 Hz, which is tolerable but not free, and host scheduling jitter is
less predictable than its mean. Keeping the PLL on core 1 removes the question
entirely, at the cost of one on-device demodulator.

That demodulator is the minimum: one channel at the fundamental, owned privately
by the `Pll`, not a general estimator bank. It should run `O(1)` every tick
rather than performing a large update on `period_start`, so the tick stays
uniform and no rare-expensive-tick pattern is reintroduced.

### The master phase must be a stream source

Host-side demodulation must use the same phase reference the device used. With
frequency changing, whether under the PLL or from a host-commanded step applied
at an unknown sample boundary, the host cannot reconstruct phase by counting
samples; `cmd_epoch` reports *that* a command landed, not at what phase.

So any programme owning a master phase accumulator exposes it as a signal named
`phase`, in turns. As `f32` this retains 24 bits, about 2×10⁻⁵ degrees, against
roughly 9 bits needed at 400 samples per period. This makes host-side estimation
exact even while the PLL retunes continuously, which would otherwise be the main
objection to moving estimation off the device. The master phase is independently
useful for other host-side processing.

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

`MAX_RT_VALUES = 132` costs `32 × ~540 = 17.3 KB` of queue SRAM, against roughly
390 KB free after the current ~130 KB. Halving `COMMAND_QUEUE_LEN` to 16 recovers
half of that if SRAM later becomes tight, and is the preferred lever because
queue depth is a latency convenience while payload width is a capability. A
`DoubleBuffer` for the force vector was considered and rejected: at 528 bytes the
object is small enough that a wider command is simpler than a second state
machine.

**Consequent WCET item.** Applying two commands per tick now copies up to
1056 bytes. On an aligned word copy this is a few microseconds against the
current 34 µs maximum, but it makes SRAM residency of the compiler's EABI copy
helpers more critical, not less. The layout gate already caught flash-resident
copy helpers once (`notes.md`, 2026-07-15, image `b35d4b8`);
`COMMANDS_PER_TICK` must be re-measured, not assumed.

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

So a phase-locked programme exposes writable `freq_setpoint` and streams
`freq_actual`, and never lets a host infer the second from the first.

### Coherence: the record path, not the telemetry path

A `Record` (`rt_loop.rs:51`) is assembled inside one tick and enqueued as a
single unit, so every value in it comes from the same tick by construction. The
`ExtraParam` path has no equivalent: it is N independent relaxed loads served
from a TCP request, with nothing tying them to a common instant.

For a scalar such as whirl-rig's RPM that distinction does not matter. For
correlated quantities, such as amplitude and phase across several channels, it
does: a poll can straddle an update and return amplitude from one period with
phase from the next. The rule is therefore:

> `ExtraParam` is for independent scalars. Correlated multi-value quantities go
> through the record path.

With estimation host-side and the PLL streaming its own outputs, nothing in the
present design needs correlated telemetry, so this is a constraint that keeps a
latent trap closed rather than a problem to solve.

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

### Safety consequences

- **A global trip that quiets every shaker simultaneously is correct here**, not
  a limitation. Asymmetric quieting of an appropriated force vector would itself
  excite modes.
- **Per-shaker force ratings differ**, which `clamp_output(actuator, ·)` covers.
- **Stroke is a gap.** Shaker displacement limits bind at low frequency and an
  amplitude clamp on force does not protect them. A multi-shaker rig needs a
  displacement-based fault, not only an output clamp. The mechanism is still
  open (see "Open questions").
- **A device-side PLL adds a frequency-excursion failure mode.** The gate clamps
  output *amplitude*; a PLL that loses lock or diverges sweeps the excitation
  *frequency*, potentially dragging a large force vector through a resonance
  with every existing check passing. This requires hard minimum and maximum
  increment bounds inside the `Pll`, plus a loss-of-lock condition that can
  latch the trip.
- **Faults can now originate in the programme, not only the rig.** Loss of PLL
  lock is a programme condition; `Rig::output_fault(inputs)` cannot see it. The
  safety contract therefore gains `Program::fault()`, consulted alongside the
  rig's, with either latching the global trip.

### One `helic-core` addition this implies

`Pll`: a self-contained phase-locked loop owning its own single-harmonic
quadrature demodulator, loop filter, and NCO, with configurable target phase,
loop gain, and hard frequency bounds. It is generic and expected to be reused
across rigs, so it belongs in `helic-core` rather than any rig crate.

`FourierEstimator::update_with(&HarmonicFrame<H>, signal)` is *not* required by
this design, since no device-side consumer remains once estimation moves
host-side. It stays a reasonable future optimisation if a rig ever needs a
device-side estimator bank. Likewise, a period-coherent estimator is now a host
library concern rather than a `helic-core` one.

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
modules (core 1: SRAM, no Embassy, bounded WCET). Those are opposite disciplines
in one dependency set, and `docs/overrun_handoff.md` records an entire debugging
saga caused by core-0 work reaching the tick through the shared XIP cache.

The split is clean in practice: `rt_loop.rs`'s only `embassy-time` use is
`status_run` (line 7), a core-0 task, and `defmt` appears only in setup and that
same task. Moving `status_run` out leaves `embassy-rp` for `pac::TIMER0` plus
`heapless` and `static_cell`.

| Crate | Contents | Testable |
|---|---|---|
| `helic-proto` | wire protocol, `ErrorCode`; broker protocol feature-gated | host |
| `helic-core` | DSP: generators, controllers, estimators, filters, tables, `Pll`, `DoubleBuffer` | host |
| **`helic-rt`** (new) | `Rig`, `TickSource`, `Program`, `ParamGroup`, `ParamDef`, `Payload`, `RtCommand`, `ParamStore`, safety gate, source assembly | host |
| **`helic-fw-rt`** (new) | core 1: tick sources, `rt_mem`, `analog_spi`, PIO transports, the loop driver | cross-build |
| **`helic-fw-net`** (new) | core 0: `net/`, `comms/`, `time_watchdog`, `status_run` | cross-build |
| `helic-drivers` | chip and sensor logic, pure and host-testable | host |
| `<rig>-program` | that rig's programme, controllers, parameter shadows, and rig-specific DSP | host |
| `fw-<rig>` | that rig's `board.rs`, `config.rs`, `rig.rs`, `main.rs` | cross-build |

A rig repository contains `<rig>-program` and `fw-<rig>`, and its control logic
is `cargo test`-able on the host exactly as `helic-core`'s is. This is still one
codebase per rig.

### Dependency rules this makes enforceable

- `helic-core` depends only on `libm`; no protocol, no Embassy.
- `helic-rt` depends on `helic-core`, `helic-proto`, and `heapless`; no Embassy
  at all.
- `helic-fw-rt` depends on `embassy-rp` (for `pac`) but **not** on
  `embassy-net`, `embassy-time`, or `embassy-executor`.
- `helic-fw-net` may not depend on `helic-fw-rt` except through the queue
  endpoint types.

### `helic-core::rpm` moves out

`RpmEstimator` has exactly one consumer, `whirl-rig` (`rig.rs:10`), and RPM
estimation from optical revolution pulses is whirl-specific instrumentation
rather than general DSP. It moves to `whirl-rig-program`. This is the first
concrete application of the reusable/experiment rule, and it removes
`helic-core`'s only module with no second consumer.

### Device integrations misfiled as "common"

`laser.rs` is an Embassy UART task for one sensor used by one rig, sitting in a
crate every rig depends on. The portable split itself is right, since
`helic-drivers/optoncdt.rs` is a pure parser, but the rule should be that
`helic-fw-*` holds mechanisms *every* rig uses, and a mechanism *some* rig uses
belongs in a device-integration crate or that rig's repository.
`analog_spi.rs` deserves the same question. Both remain open.

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

### 2. Transactional writes, ordered centrally

The current code enqueues before updating the shadow, and rolls back a blob
commit if the enqueue fails (`firmware/common/src/params.rs:596,623`). That
ordering is a correctness property: a host receiving `Busy` must not then read
back the value it was denied. Distributing `set` to components must not
distribute that rule, or it becomes N places to get wrong.

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
    fn accept(&mut self, id: u16);
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
`reject`, which returns the `CommitToken` and clears the pending state.

**Parameter index order need not be preserved.** Both host libraries build
name-to-parameter maps at discovery and derive indices from discovery order
(`host-python/helic_daq/device.py:160`, `host-julia/src/device.jl:191`); neither
hard-codes an index. Platform parameters can therefore be an ordinary group, and
the golden registry test asserts the *set* of `(name, type, count, writable)`
rather than a verbatim order. Stream **source** order is a different matter and
is preserved, since it defines the record layout.

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
the second line of defence.

`BufferError` is local to `helic-core`, which depends only on `libm`; returning
`helic_proto::ErrorCode` would have introduced a `helic-core → helic-proto` edge
that does not currently exist.

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

    /// Programme-originated fault: loss of PLL lock, divergence, or any
    /// condition the rig cannot observe from its inputs. Consulted by the
    /// safety gate alongside `Rig::output_fault`.
    fn fault(&self) -> bool { false }
}
```

`Rig` loses `type Ctrl`: a rig describes hardware, and how many controllers run
against it is the programme's business.

The vector safety contract:

- **Output buffer** is `[f32; MAX_ACTUATORS]`, with only the
  `R::ACTUATORS.len()` prefix used.
- **Setup asserts** `P::OUTPUTS == R::ACTUATORS.len()` and
  `P::OUTPUTS <= MAX_ACTUATORS`.
- **Faults are global and per-tick**, and may originate in either the rig or the
  programme. Either latches one global trip that quiets *every* actuator. For
  multi-shaker excitation this is correct, not a limitation: asymmetric quieting
  of an appropriated force vector would itself excite modes.
- **Clamping is per actuator index**, via `clamp_output(actuator, value)`.
- **Counters are per tick, not per actuator.**
- **Streamed actuator values are the post-safety applied values**, matching
  today's behaviour (`notes.md`, 2026-07-18).
- **A non-gated rig applies every output verbatim** and the gate compiles away.
- **Autonomous excitation is bounded at source.** A programme that drives its own
  frequency must clamp the commanded increment to host-set minimum and maximum
  bounds inside the `Pll`, independently of the output gate, because the gate
  limits amplitude and cannot see a frequency excursion.

The gate becomes a pure function in `helic-rt`, host-testable:

```rust
pub fn safety_gate<R: Rig, P: Program>(
    rig: &mut R, program: &P, inputs: &[f32], commanded: &[f32], applied: &mut [f32],
) -> SafetyEvents;
```

### 5. Source assembly and shared constants

Source assembly becomes a generic walk over `R::INPUTS`, `P::SIGNALS`,
`R::ACTUATORS`, and `cmd_epoch`, so `GENERATED_SOURCES` disappears. For existing
rigs this reproduces today's names and order exactly. `comms/tcp.rs:16` also
consumes `source`/`source_count`, so it changes with this stage.

`HARMONICS` moves to a const generic on the programme. `MAX_TABLE_LEN` becomes a
const generic on `WaveTable<const N: usize = 4096>`, defaulted so existing code
is unaffected.

## Double buffering: options considered

**A. Value-in-command.** Retained as the default for coefficient banks, gains,
and force vectors, now expressed as the non-generic `Payload::Values` and sized
by the appropriation requirement above.

**B. Generic `DoubleBuffer<T>` with split endpoints.** Recommended for blobs.

**C. Per-object mailbox polled by core 1.** Rejected because it decouples
activation from `cmd_epoch`: the guarantee that a table swap is visible in
exactly the record whose `cmd_epoch` reports that command is hardware-verified
(`notes.md`, 2026-07-16, the live 0.45 V to 1.65 V re-commit, and the
`forcing,out,cmd_epoch` transition at sample 885064).

**D. `Pool<T, N>`.** The generalisation if several blobs must swap
independently. `DoubleBuffer<T>` is `Pool<T, 2>`. Not proposed now.

**RAM budget.** One `DoubleBuffer<WaveTable<4096>>` is 32 KB, which is why
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

### Example 2: force appropriation with host-side estimation

Four shakers, four response channels, phase-locked excitation. The device
synthesises the force vector, runs the PLL, and streams raw data plus phase; the
host does all estimation and the appropriation law.

```rust
// appropriation-program/src/lib.rs   (no_std, host-tested, no Embassy)
use helic_core::{FourierCoeffs, FourierSignal, HarmonicGenerator, Pll};
use helic_rt::{ParamDef, ParamGroup, ParamType, Payload, Program, RtCommand, Staged, StepCtx};

const SHAKERS: usize = 4;
const H: usize = 5;
const VECTOR_LEN: usize = SHAKERS * (1 + 2 * H);   // 44 values

pub const DOMAIN: u8 = 1;

mod ids {
    pub const FREQ_SETPOINT: u16 = 0;   // setpoint only; the PLL owns the truth
    pub const FORCE_VECTOR: u16 = 1;    // all shakers, all harmonics, atomic
    pub const PLL_GAIN: u16 = 2;
    pub const TARGET_PHASE: u16 = 3;    // −90° for phase resonance
    pub const FREQ_MIN: u16 = 4;        // hard NCO bounds: see safety contract
    pub const FREQ_MAX: u16 = 5;
    pub const EXCITATION_MODE: u16 = 6; // fixed frequency | phase-locked
    pub const RESET: u16 = 7;
}

pub struct Appropriation {
    harmonics: HarmonicGenerator<H>,
    forces: [FourierSignal<H>; SHAKERS],
    pll: Pll,                    // owns its own single-harmonic demodulator
    mode: ExcitationMode,
    pll_channel: usize,
    phase: f32,
    freq_actual: f32,
    phase_error: f32,
}

impl Program for Appropriation {
    const OUTPUTS: usize = SHAKERS;
    // Per-sample quantities only. All estimation is host-side, computed from
    // the streamed responses against the streamed `phase`.
    const SIGNALS: &'static [(&'static str, &'static str)] = &[
        ("phase", "turn"), ("freq_actual", "Hz"),
        ("phase_error", "deg"), ("pll_locked", "bool"),
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
            (ids::FREQ_MIN, Payload::U32(inc)) => self.pll.set_min_increment(inc),
            (ids::FREQ_MAX, Payload::U32(inc)) => self.pll.set_max_increment(inc),
            (ids::EXCITATION_MODE, Payload::U32(m)) => self.mode = ExcitationMode::from_u32(m),
            (ids::RESET, _) => self.reset(),
            _ => {}
        }
    }

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx, outputs: &mut [f32]) {
        let frame = self.harmonics.step(ctx.lut);

        // O(1) per tick: the PLL demodulates one channel at the fundamental
        // internally. It clamps its own output to the configured bounds.
        if self.mode == ExcitationMode::PhaseLocked {
            let increment = self.pll.update(frame, inputs[self.pll_channel], dt);
            self.harmonics.set_increment(increment);
            self.phase_error = self.pll.phase_error();
        }

        self.phase = frame.phase_turns();
        self.freq_actual = self.harmonics.frequency_hz(ctx.sample_rate);

        for (j, force) in self.forces.iter().enumerate() {
            outputs[j] = force.sample(frame);
        }
    }

    fn write_signals(&self, out: &mut [f32]) {
        out[0] = self.phase;
        out[1] = self.freq_actual;
        out[2] = self.phase_error;
        out[3] = if self.pll.locked() { 1.0 } else { 0.0 };
    }

    /// Loss of lock is a programme condition the rig cannot see from its
    /// inputs, so it reaches the safety gate through here.
    fn fault(&self) -> bool {
        self.mode == ExcitationMode::PhaseLocked && !self.pll.locked()
    }
}
```

The core-0 half validates the whole vector before staging it:

```rust
impl ParamGroup for AppropriationShadow {
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

Four things this example establishes.

**The source budget is comfortable.** 8 inputs (4 force, 4 response), 4
programme signals, 4 actuators, and `cmd_epoch` is 17 of 24. Moving estimation
host-side is what makes that work: on-device per-channel, per-harmonic estimates
would have needed roughly 31 sources.

**`freq_setpoint` is writable but is not the value.** `freq_actual` is streamed
because the PLL owns the truth, per the autonomous-state rule.

**`phase` is streamed so host demodulation stays exact** while the PLL retunes
continuously.

**Nothing correlated goes through `ExtraParam`.** All coupled quantities travel
in the record, which is coherent per tick by construction.

Files changed under `helic-core`, `helic-rt`, `helic-fw-rt`, or `helic-fw-net`
to support this rig: none, given `Pll` exists as a shared primitive.

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
| Rig-specific estimator (e.g. RPM) | `helic-core` | rig crates only |
| Rig with no waveform table | not expressible; params advertised and inert | omit the component |
| Second buffered blob | copy `table.rs` | one `DoubleBuffer<T>` static |
| Different harmonic count (≤16) | `firmware/common/src/lib.rs` | const generic on the programme |
| Shorter tables to save RAM | `helic-core/src/table.rs` | const generic on `WaveTable` |
| >24 sources, >4 actuators, >132 payload values | shared crates | shared crates, deliberately |
| New genuinely reusable primitive (e.g. `Pll`) | `helic-core` | `helic-core`, correctly |

## Repository separation

A rig repository contains `<rig>-program` and `fw-<rig>`, and needs:

1. `helic-core`, `helic-proto`, `helic-rt`, `helic-fw-rt`, `helic-fw-net`, and
   `helic-drivers` published or referenced as git dependencies.
2. Its own `.cargo/config.toml` with the `thumbv8m.main-none-eabihf` target and
   runner, currently shared at `firmware/.cargo/config.toml`.
3. Its own `Cargo.lock`. The pinned Embassy set in `firmware/Cargo.toml`'s
   `[workspace.dependencies]` cannot be inherited across repositories.
4. A copy of, or a shared action for, the `check_rt_layout.py` gate.

Point 3 is the main friction and remains open. A reasonable path is to keep the
three production rigs here and treat the crate boundary as the contract that
*permits* an external rig, verified by one out-of-workspace test rig rather than
by moving everything at once.

## Relationship to the RT programme proposal

**Retained:** `HarmonicFrame`/`HarmonicGenerator` and one shared basis per tick;
`FourierSignal`; `ControlledAxis<C, H>`; removal of `PeriodicGenerator` and
`GenSample`; the bounded logical output vector; `Rig::ACTUATORS`; the
`SAFETY_GATED` opt-out; all mandatory table-phase semantics; the per-tick
ordering.

**Superseded:** the slot-indexed `SetCoeffs`/`SetIncrement` scheme;
`ParamStore<P, R>` remaining generic; `RtProgram`'s parameter methods, replaced
by the component's `ParamGroup` half; retention of `GENERATED_SOURCES` and the
table's special-cased command path.

`rt_program_proposal.md` anticipated the atomic multi-axis update requirement
but deferred it. Sizing `MAX_RT_VALUES` from the appropriation case resolves it
in favour of the copied bank.

## Migration plan

1. **`helic-rt` crate created**, initially by moving `Rig`, `TickSource`, and
   the parameter types out of `helic-fw-common` unchanged.
2. **`helic-fw-common` split** into `helic-fw-rt` and `helic-fw-net`, with the
   dependency rules added to CI. Mechanical; no behaviour change.
3. **`DoubleBuffer<T>` with split endpoints into `helic-core`**, with host
   tests. Reimplement `table.rs` on top of it.
4. **`RtCommand`, `Payload`, and the size assertion.** Re-measure
   `COMMANDS_PER_TICK` WCET at the new payload width and confirm EABI copy
   helpers remain in SRAM.
5. **`ParamGroup` stage/accept/reject and the `ParamStore` walk.** Largest
   single commit; must change no discovered parameter *name* and no source order.
6. **`Program` trait and `StandardProgram`**, absorbing phase, coefficients,
   controller, and table player out of `RtLoopState`. Add the `phase` signal
   convention. Split the table into its own group in the same change.
7. **Bounded output vector**, `Rig::ACTUATORS`, slice `actuate`, the vector
   safety gate including `Program::fault`, and generic source assembly
   (including `comms/tcp.rs`). Migrate all three rigs together.
8. **Const generics** for `HARMONICS` and `MAX_TABLE_LEN`, defaulted.
9. **`RpmEstimator` moves** from `helic-core` to `whirl-rig-program`.
10. **`Pll` added to `helic-core`**, with host tests including its frequency
    bounds and lock detection. Independent of stages 1 to 9.
11. **An out-of-workspace test rig** as the final architectural acceptance test.

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
- **Golden registry test**: the *set* of `(name, type, count, writable)` for each
  production rig, asserted against the current registry. Order is deliberately
  not asserted, since both host libraries discover by name.
- **Golden source test**: names *and order*, asserted verbatim, since source
  order defines the record layout.
- **Command routing**: two groups with the same local id but different domains
  reach different components.
- **`Payload` round trip** for every kind, including `Values` at 33 and 132
  lengths.
- **Vector safety gate**, host-tested: a fault from either the rig or the
  programme quiets all actuators; per-actuator clamping; counters per tick not
  per actuator; non-gated rig applies verbatim.
- **`Pll`**: locks to a known phase offset; the commanded increment never leaves
  the configured bounds under any input including divergent ones; loss of lock
  is reported.
- **Phase source fidelity**: `f32` `phase` reconstructs the accumulator to well
  within one sample step at the highest supported excitation frequency.
- **`size_of::<RtCommand>()`** compile-time assertion, and a measured
  `COMMANDS_PER_TICK` WCET at full payload width.
- **Dependency rules**: a CI check that `helic-rt` has no Embassy dependency and
  `helic-fw-rt` has no `embassy-net`/`embassy-time`/`embassy-executor`.

Hardware regression is the full sequential suite in the developer guide, since
stages 6 and 7 touch the entire tick calculation. The `cmd_epoch` coherence tests
for coefficient replacement and table re-commit are the specific evidence stages
3 and 6 must not degrade; the 34 µs loop maximum with a fixed 36 µs wake phase is
the timing baseline. Stage 4 additionally requires a fresh loop-maximum
measurement.

## Risks and open questions

**Risks**

- Larger change than the existing proposal, touching a tick path with verified
  timing. Mitigated by stage ordering and the golden tests.
- `Program` risks becoming a god-trait. If it grows past step, signals, apply,
  and fault, split it rather than letting policy drift back into the loop.
- Core-1 inlining is newly load-bearing; a loop over shakers may not unroll as
  today's straight-line code does. Confirm by ELF inspection and timing.
- The wider payload is the main new timing risk; the EABI copy-helper residency
  problem has bitten once already.
- `Active::get()` per tick replaces a cached field; expected to be one inlined
  load, must be verified.
- `MAX_RT_VALUES = 132` caps a single atomic update at 4 actuators × 16
  harmonics. An 8-shaker multi-harmonic rig is a platform change.

**Open questions**

1. **Stroke limiting mechanism.** An amplitude clamp on force does not protect
   shaker displacement, which binds at low frequency. Is this a rig
   `output_fault` on measured displacement, which requires the displacement to
   be instrumented, or does the gate need a rate or DC-offset hook? This is the
   one open question with a physical hazard attached and should be settled
   before any shaker rig is built.
2. **Per-actuator trip** remains deferred from `rt_program_proposal.md`. The
   global trip is correct for appropriation; confirm the deferral explicitly
   rather than inheriting it silently.
3. **`MAX_ACTUATORS = 4`** derives from the AD5064 having four channels, not
   from the application; ground vibration testing routinely uses more shakers. A
   larger rig needs different DAC hardware anyway, so the coupling may be
   genuine, but it is cheap to raise now and expensive later.
4. **`MAX_GROUPS = 8`** is unjustified. Current usage is five.
5. **`MAX_CTRL_PARAMS`/`MAX_RIG_PARAMS`/`MAX_EXTRA_PARAMS`** become meaningless
   once groups own their storage; only the `u16` total and the discovery page
   budget survive. Confirm they retire.
6. **Device integrations** (`laser.rs`, `analog_spi.rs`) sitting in a crate every
   rig depends on.
7. **Embassy pinning across repositories.**

## Revision history

**Revision 4** settles where computation lives and follows the consequences:

- **Estimation moves host-side by default.** Confirmed as existing practice
  rather than a new policy: `FourierEstimator` has no firmware consumer at all,
  and sustained all-source streaming at 8 kHz is hardware-proven
  (`notes.md:50`). Host-side is also scientifically better: `f64`, exact
  per-period integration, and the raw time series retained.
- **The PLL stays on core 1**, because it is a feedback loop and the network
  inside it costs phase margin. It owns a private single-harmonic demodulator,
  which is the minimum device-side estimation the arrangement requires, and runs
  `O(1)` per tick rather than in a burst on `period_start`.
- **`phase` becomes a stream source**, as turns in `f32`. This makes host-side
  demodulation exact while the PLL retunes, which would otherwise have been the
  main objection to moving estimation off the device. It is independently useful
  for other host-side processing.
- **Telemetry coherence is resolved by construction** and demoted to a
  documented constraint: the record path is coherent per tick, `ExtraParam` is
  not, so `ExtraParam` is for independent scalars only. The previous revision's
  recommendation to publish correlated modal estimates that way was unsafe and
  is withdrawn.
- **Two new safety requirements from the device-side PLL.** Hard minimum and
  maximum increment bounds inside the `Pll`, because the output gate limits
  amplitude and cannot see a frequency excursion sweeping a large force vector
  through a resonance; and `Program::fault()`, because loss of lock is a
  programme condition `Rig::output_fault(inputs)` cannot observe.
- **`helic-core::rpm` moves to `whirl-rig-program`.** RPM estimation from
  optical revolution pulses is whirl-specific instrumentation, not general DSP.
  This is the first application of the reusable/experiment rule and removes
  `helic-core`'s only module with no second consumer.
- **Parameter index order need not be preserved.** Both host libraries discover
  by name (`device.py:160`, `device.jl:191`). The golden registry test therefore
  asserts a set, not an order; the golden *source* test still asserts order,
  since that defines the record layout. Platform parameters can be an ordinary
  group.
- **`helic-core` additions reduced to `Pll`.** `FourierEstimator::update_with`
  is no longer required, since no device-side estimator bank remains, and the
  period-coherent estimator becomes a host library concern.
- Example 2 recast with host-side estimation, which simplifies the programme
  substantially and brings the source budget to 17 of 24.

**Revision 3** folded in a crate-boundary review and application-driven sizing:
the `helic-fw-common` split into `helic-fw-rt` and `helic-fw-net` with
CI-checkable dependency rules; `MAX_RT_VALUES` raised from 33 to 132 because 33
admits only single-harmonic appropriation; the autonomous-state rule; the
host/device division of labour; the vector `actuate` justified from ~LDAC
skew; and a force-appropriation worked example.

**Revision 2** responded to a review. Each finding was confirmed against the
code and accepted: parameter writes were not transactional, and were replaced by
`stage`/`accept`/`reject` with the ordering written once in `ParamStore::set`;
`Payload::Coeffs(FourierCoeffs<HARMONICS>)` contradicted the const-generic `H`
proposed elsewhere and became non-generic `Values`; the generic `DoubleBuffer<T>`
safe API was unsound in all six ways identified and was replaced by an
SPSC-style endpoint split; the MIMO safety contract was missing and is now
stated; command identifiers were not component-local and gained a `domain`;
rig-local programme logic had no host-testable home, resolved by the portable
`helic-rt` crate; and the goal gained a capacity qualification.
