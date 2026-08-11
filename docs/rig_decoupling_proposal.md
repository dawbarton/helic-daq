# Rig decoupling: component-owned parameters, signals, and buffers

Status: proposed, not implemented. Revision 5.1. Supersedes parts of
`docs/rt_program_proposal.md`. Revision history and review responses are at the
end.

## Goal

> Composing a rig from existing platform primitives, within documented source,
> output, group, payload, and memory bounds, must require changes only in that
> rig's own repository.

Fixed capacities are part of a bounded real-time platform, and raising one is a
deliberate platform change with its own timing and memory evidence. A genuinely
new reusable DSP algorithm belongs in `helic-core` and a new peripheral driver
in `helic-drivers`; putting either in a rig crate would be the failure mode, not
the success case.

### Documented platform capacities

| Bound | Value | Set by |
|---|---|---|
| `MAX_SOURCES` | 24 | protocol discovery headroom |
| `MAX_ACTUATORS` | 4 | record and safety-gate buffers |
| `MAX_GROUPS` | 8 | `ParamStore` registry vector |
| `MAX_RT_VALUES` | 132 | widest command payload; see "Payload width" |
| `COMMAND_QUEUE_LEN` | 32 | existing |
| `COMMANDS_PER_TICK` | 2 | existing WCET bound |

### What earns a place in a shared crate

> A type earns a place in a shared crate by having **two actual consumers**, not
> by being conceptually general.

This is deliberate: otherwise `helic-core` accumulates speculative API that
constrains refactoring for hypothetical users. It is why `RpmEstimator` moves
out (one consumer) while `Pll` moves in (expected across rigs). Both decisions
are one commit to reverse.

## Target applications and what they constrain

The intended use is experimental structural dynamics: modal testing, force
appropriation, their nonlinear extensions via phase-locked excitation, and
control-based continuation.

### Where computation lives

The governing rule is about *where the inputs exist*, not about which side of
the wire the work happens to be easier on:

> Processing whose inputs exist only on the device stays on the device.
> Processing reconstructible from streamed samples moves host-side.

`RpmEstimator` consumes PIO pulse-period FIFO reads, which are not a sample
stream, so it stays on core 1 regardless of which crate owns it. Response
demodulation consumes ADC samples, which are streamed, so it moves host-side.

This is already the codebase's practice rather than a new policy:
`FourierEstimator` exists in `helic-core` with unit tests and **no firmware
consumer at all**, and control-based continuation already runs host-side. For an
appropriation rig:

| Runs on core 1 | Runs on the host |
|---|---|
| force-vector synthesis at sample rate | multi-channel, multi-harmonic estimation |
| the PLL, including its two-channel fundamental demodulator | mode indicator functions |
| the safety gate | the appropriation update law |
| anything reading a PIO FIFO or peripheral register | all reprocessing and offline analysis |

**Bandwidth, corrected.** The appropriation example below streams 17 sources.
At `17 × 4 B × 8000 Hz` that is 544 kB/s, or **4.352 Mbit/s** before packet
overhead, and about **1.96 GB/hour** of raw values. The hardware evidence in
`notes.md:50` covers **13** sources sustained for 120.25 s with zero UDP loss
and a 35 µs loop maximum. Seventeen sources is a 31% increase on that and is
therefore encouraging evidence, **not** verification of the proposed
configuration; it needs its own run.

Host-side estimation is also better science: `f64`, exact per-period block
integration rather than a one-pole IIR with documented ripple (`fourier.rs:80`),
proper windowing, and the raw time series retained.

**Why the PLL is the exception.** It is a feedback loop, so the network sits
inside it. The estimator settling time is *not* the relevant comparison, since
it applies wherever the loop runs; only the **incremental** delays matter:
record-queue drain, UDP batching, transport, host scheduling, and the command
return path. Those are perhaps 5–50 ms, and the decisive property is that host
scheduling makes them **non-deterministic**, which costs phase margin
unpredictably rather than by a fixed amount. Keeping the PLL on core 1 removes
the variance at the cost of one two-channel demodulator.

**What the device PLL is, precisely.** It is a **single drive-point,
fundamental-only frequency tracker**. It locks the phase difference between one
*measured force* channel and one *measured response* channel. It must not use
the commanded oscillator as the force reference: shaker armature dynamics and
force drop-out near resonance mean DAC-command phase is not applied-force phase,
and that discrepancy is largest exactly where appropriation operates. The full
multipoint, multiharmonic appropriation condition is enforced host-side by the
update law; the device only keeps the drive point at the commanded phase
relationship.

### The master phase must be a stream source

Host-side demodulation must use the same phase reference the device used. With
frequency changing, whether under the PLL or from a host-commanded step applied
at an unknown sample boundary, the host cannot reconstruct phase by counting
samples; `cmd_epoch` reports *that* a command landed, not at what phase.

Every programme owning a master phase accumulator therefore exposes a signal
named `phase`, in turns.

**This is a deliberate interface addition, not a no-op.** `StandardProgram` owns
an accumulator, so CBC, whirl, and Pico 2W all gain a source: CBC goes from 14 to
15. The golden source lists, `docs/protocol.md`, and the host libraries' expected
registries all change, and the all-source hardware regression must be re-run on
each rig. The earlier claim that CBC is unchanged is **withdrawn**.

**Precision.** Converting the `u32` accumulator to `f32` retains 24 bits, so the
worst-case absolute error is `2⁻²⁵` turn, about `1.07 × 10⁻⁵` degrees. The test
asserts that bound directly and propagates it to an estimator-error bound; the
previously proposed "within one sample step" check was far too weak to detect a
conversion mistake.

### Payload width, and the buffer-versus-copy decision

Force appropriation drives `N` shakers with independent amplitude and phase. A
partial update to the force vector is a transient in the appropriated mode shape,
so the whole vector must take effect at one sample boundary. The payload is
`N · (1 + 2H)` values: 44 for four shakers at five harmonics, 132 at sixteen.

Two mechanisms can deliver that atomically. The comparison, which earlier
revisions asserted rather than showed:

| | Copied payload (**chosen**) | `DoubleBuffer` force vector |
|---|---|---|
| `MAX_RT_VALUES` | 132 | 33 |
| Queue SRAM | 32 × ~540 = 17.3 KB | 32 × ~140 = 4.5 KB |
| Buffer SRAM | — | 2 × 528 = 1.1 KB |
| **Total** | **17.3 KB** | **5.6 KB** |
| Per-boundary work | up to 1056 B copy | pointer swap |
| Data path | `FourierSignal` owns its bank | banks live in the buffer, read through `Active` |

The copy costs roughly 2–4 µs worst case against a 125 µs budget with 34 µs
used, and occurs at most once per excitation period, which is 400 ticks at
20 Hz. Twelve kilobytes of roughly 390 KB free is not decisive either.

The deciding argument is **correctness-risk concentration**. Making
`DoubleBuffer` sound has taken three attempts (see "Revision history"), and the
sound version below needs a generation counter, non-`Copy` propagation through
the command type, and explicit `!Sync` markers. Putting the *force vector* on
that mechanism means a token or generation defect applies a wrong force vector
at a wrong `cmd_epoch`, which is materially worse than a wrong table sample and
sits on the path where appropriation correctness is the entire point. A copied
payload is trivially correct by construction.

**This decision is gated on measurement.** Stage 4 measures the actual
`COMMANDS_PER_TICK` WCET at 132-value payloads. If the copy materially exceeds
the 2–4 µs prediction, switch to the buffer. Applying two commands per tick
copies up to 1056 bytes, which makes SRAM residency of the compiler's EABI copy
helpers more critical, not less; the layout gate already caught flash-resident
copy helpers once (`notes.md`, 2026-07-15, image `b35d4b8`).

### Autonomous programme state is not a parameter shadow

> Any quantity the programme can change autonomously must be discoverable
> separately as a stream source or read-only telemetry. A writable parameter for
> the same quantity is a setpoint, and must be documented as one.

A phase-locked programme exposes writable `freq_setpoint` and streams
`freq_actual`. **`freq_actual` describes the increment that produced this
record's phase advance**, not the increment installed for the next tick. Host
demodulation depends on that definition, so it is normative.

### Coherence: the record path, not the telemetry path

A `Record` (`rt_loop.rs:51`) is assembled inside one tick and enqueued as a
single unit, so every value in it comes from the same tick by construction. The
`ExtraParam` path is N independent relaxed loads served from a TCP request, with
nothing tying them to a common instant.

> `ExtraParam` is for independent scalars. Correlated multi-value quantities go
> through the record path.

### Hardware consequence: the vector `actuate` is required

`ad5064.rs:81` records ~LDAC tied low, so DAC channels are write-and-update
individually with `WORD_SETTLE_US = 3` µs between words (`ad5064.rs:26`). Four
shakers see up to 9 µs of inter-channel skew: 0.32° at 100 Hz, 3.2° at 1 kHz.
That is a material appropriation error, so a multi-shaker rig wants ~LDAC
strobed. Passing the whole output vector to `Rig::actuate(&[f32])` in one call
is what allows the rig to write N channels and strobe once.

### Safety consequences

- A global trip quieting every shaker simultaneously is **correct** here:
  asymmetric quieting of an appropriated force vector would itself excite modes.
- Per-shaker force ratings differ, which `clamp_output(actuator, ·)` covers.
- **Stroke is a gap.** Displacement limits bind at low frequency and an
  amplitude clamp on force does not protect them. Mechanism still open.
- **Autonomous excitation must be bounded at source.** The gate clamps
  amplitude and cannot see a frequency excursion sweeping a large force vector
  through a resonance, so the `Pll` clamps its own commanded increment to
  host-set bounds.
- **Non-finite outputs latch a fault.** `ad5064.rs:70` currently maps them
  silently to 0 V, which hides programme divergence rather than reporting it.
- **Faults can originate in the programme.** Loss of PLL lock is invisible to
  `Rig::output_fault(inputs)`, hence `Program::fault()`.

## What currently forces a shared-code edit

| # | Location | Coupling |
|---|---|---|
| 1 | `rig.rs:17` | `GENERATED_SOURCES` is a fixed five-entry list |
| 2 | `rig.rs:29` | `source::<R>()` hand-chains three segments with offset arithmetic |
| 3 | `rig.rs:202` | `actuate(&mut self, out: f32)` is scalar |
| 4 | `rig.rs:198` | `type Ctrl: Controller` binds a rig to one controller |
| 5 | `rt_loop.rs:21` | `RtCommand` enumerates all fourteen operations |
| 6 | `rt_loop.rs:269` | The tick body hardcodes the signal graph and record layout |
| 7 | `params/schema.rs:9` | `BASE_PARAMS` is a fixed 33-entry table with 23 `IDX_*` constants |
| 8 | `params.rs:324,439` | Two index-keyed matches plus four-segment arithmetic, thrice |
| 9 | `table.rs` | A bespoke unsafe double buffer specialised to `WaveTable` |
| 10 | `lib.rs:23`, `helic-core/src/table.rs:5` | `HARMONICS` and `MAX_TABLE_LEN` are shared constants |

Points 1, 2, 5, 6, 7, and 8 are one problem: parameters, sources, and commands
are three separately hand-chained index spaces.

The reusable idea from the earlier `rtc` project is `rtc_data_add_par`, which let
each subsystem contribute entries to one lookup table from its own
initialisation code, with the framework owning the table and never the storage.
The mechanism does not carry across: `rtc` was single-core, so a host write went
straight into a `volatile` pointer. The cross-core value-copy boundary stays.

## Design principle: core 0 may be dynamic, core 1 must not be

- **Core 0** holds a list of trait objects, one per component, and walks it.
- **Core 1** holds one statically dispatched `Program` chosen in `config.rs`.
  No `dyn`, no allocation, no run-time selection.

A component's two halves are written adjacent in one file, sharing one set of
local id constants.

## Crate layout

**Portability.** The runtime contract types currently sit in the RP2350-specific
`helic-fw-common`, so any rig programme implementing them is unhostable by
construction, against developer-guide principle 4 (`developer_guide.md:24`).

**Discipline.** `helic-fw-common` also conflates the two cores: `net/`,
`comms/`, `laser.rs`, `time_watchdog.rs` (core 0, Embassy, flash) alongside
`rt_loop.rs`, `rt_mem.rs`, `analog_spi.rs`, PIO (core 1, SRAM, bounded WCET).
`docs/overrun_handoff.md` records an entire debugging saga caused by core-0 work
reaching the tick. The split is clean: `rt_loop.rs`'s only `embassy-time` use is
`status_run` (line 7).

| Crate | Contents | Testable |
|---|---|---|
| `helic-proto` | wire protocol, `ErrorCode`; broker protocol feature-gated | host |
| `helic-core` | DSP: generators, controllers, estimators, filters, tables, `Pll`, `DoubleBuffer` | host |
| **`helic-rt`** (new) | `Rig`, `TickSource`, `Program`, `ParamGroup`, `ParamDef`, `Payload`, `RtCommand`, `ParamStore`, safety decision, source assembly | host |
| **`helic-fw-rt`** (new) | core 1: tick sources, `rt_mem`, `analog_spi`, PIO, loop driver, safety atomics | cross-build |
| **`helic-fw-support`** (new) | core 0: `net/`, `comms/`, `time_watchdog`, `status_run` | cross-build |
| `helic-drivers` | chip and sensor logic, pure and host-testable | host |
| `<rig>-program` | that rig's programme, controllers, shadows, rig-specific DSP | host |
| `fw-<rig>` | that rig's `board.rs`, `config.rs`, `rig.rs`, `main.rs` | cross-build |

### Dependency rules, CI-checked

- `helic-core` depends only on `libm`.
- `helic-rt` depends on `helic-core`, `helic-proto`, `heapless`; no Embassy.
- `helic-fw-rt` depends on `embassy-rp` (for `pac`) but **not** `embassy-net`,
  `embassy-time`, or `embassy-executor`.
- `helic-fw-support` may not depend on `helic-fw-rt` except through the queue
  endpoints and `&'static RtShared`.

### Naming: why `helic-fw-support`, not `helic-fw-net`

Of the modules on the core-0 side, only `net/` and `comms/` are networking;
`time_watchdog`, `status_run`, and `laser.rs` are not. Naming the crate for two
of its five members would be wrong on the day it was created and worse
afterwards.

The property that actually unites them is that they run on core 0 and must
never be reachable from the tick. `helic-fw-rt` and `helic-fw-support` name that
contrast directly.

**A vague name invites accretion, which is how `helic-fw-common` became the
problem this split exists to fix.** The name is therefore backed by a written
membership rule in the crate's own documentation:

> A module belongs in `helic-fw-support` if it runs on core 0 **and** every rig
> uses it. A module used by *some* rigs belongs in a device-integration crate or
> that rig's repository.

That rule is a test, not a sentiment, and `laser.rs` fails its second clause
today: one rig uses it. The rule does not decide open question 6 by itself, but
it does mean the question must be answered when the module is moved rather than
deferred indefinitely by filing it somewhere plausible-sounding.

### Cross-core shared state: injected, not static

**This must be settled before stage 1, which cannot otherwise be executed.**

`ParamStore` reads twenty distinct items from `rt_loop`: seventeen atomics
(`TICKS`, `LOOP_TIME_*`, `CLOCK_JITTER_US`, `OVERRUNS`, `TICK_TIMEOUTS`,
`RECORDS_DROPPED`, `COMMAND_BACKLOG_MAX`, `WAKE_PHASE_*`, `T_*_MAX_US`,
`SAFETY_*`) plus `reset_diagnostics`, `safety_arm`, and `safety_disarm`.

Stage 1 moves `ParamStore` into `helic-rt`; stage 2 moves `rt_loop` into
`helic-fw-rt`. Leaving the atomics where they are would make `helic-rt` depend
on `helic-fw-rt`, inverting the layering. The shared observation surface must
therefore live in a crate both sides depend on, which is `helic-rt`.

Two ways to do that. Statics in `helic-rt` are simplest and closest to today,
but put mutable global state in a library crate and make host tests share it.
**Injection is chosen**, because `ParamStore` being host-testable without static
state is most of why `helic-rt` exists:

```rust
// helic-rt

/// Everything `diag_reset` clears. Written by the real-time core, read by the
/// control server.
pub struct Diagnostics {
    pub ticks: AtomicU32,               // total; deliberately not reset
    pub loop_time_last_us: AtomicU32,
    pub loop_time_max_us: AtomicU32,
    pub clock_jitter_us: AtomicU32,
    pub overruns: AtomicU32,
    pub tick_timeouts: AtomicU32,
    pub records_dropped: AtomicU32,
    pub command_backlog_max: AtomicU32,
    pub wake_phase_min_us: AtomicU32,
    pub wake_phase_max_us: AtomicU32,
    pub t_measure_max_us: AtomicU32,
    pub t_actuate_max_us: AtomicU32,
    pub t_rest_max_us: AtomicU32,
    pub safety_clamp_ticks: AtomicU32,
    pub safety_quiet_ticks: AtomicU32,
}

/// Latched safety state, which deliberately survives `diag_reset`.
pub struct Safety {
    pub armed: AtomicU32,
    pub tripped: AtomicU32,
}

pub struct RtShared {
    pub diagnostics: Diagnostics,
    pub safety: Safety,
}
```

The split between the two structs is the `diag_reset` lifecycle boundary, which
today is a comment in `rt_loop::reset_diagnostics` warning that armed and
tripped are deliberately left alone. Making it a type boundary means
`diagnostics.reset()` can clear its whole struct with no way to clear `armed` by
accident. Note that `safety_clamp_ticks` and `safety_quiet_ticks` sit in
`Diagnostics` despite their names, because `diag_reset` does clear them.

`fw-<rig>` owns one const-initialised `static SHARED: RtShared` and passes
`&SHARED` to both `ParamStore::new` and the real-time loop. Every field is an
atomic, so `RtShared` is `Sync` automatically and the shared `&'static` is sound
across cores without an `unsafe impl`.

**Hot-path note.** The loop performs roughly ten atomic writes per tick, which
become base-pointer-plus-constant-offset rather than absolute addresses. The
base should stay in a register for the whole tick and cost nothing, but this
lands on the tick path, so it is verified by ELF inspection and a loop-maximum
measurement at stage 2 rather than assumed.

### `helic-core::rpm` moves to `whirl-rig-program`

Under the two-consumer rule, `RpmEstimator` has one (`whirl-rig/src/rig.rs:10`).

**Recorded objection:** a reviewer noted that the type is a hardware-independent
period-to-RPM estimator with staleness, so one *present* consumer does not
establish that it is rig-specific, and moving it is arguably contrary to sharing
reusable code. That has force. The counter is that shared API surface has an
ongoing cost and hypothetical reuse does not pay it; the move is one file with
its tests, reversible in a single commit if a second rotating rig appears. The
objection is recorded so the decision can be revisited rather than re-derived.

### Device integrations misfiled as "common"

`laser.rs` is an Embassy UART task for one sensor used by one rig, in a crate
every rig depends on. `helic-fw-*` should hold mechanisms *every* rig uses; a
mechanism *some* rig uses belongs in a device-integration crate or that rig's
repository. `analog_spi.rs` deserves the same question. Both remain open.

## Design

### 1. Command address and payload

```rust
// helic-rt
pub const MAX_RT_VALUES: usize = 132;
pub const DOMAIN_RIG: u8 = 0;          // reserved; programmes use 1..

/// Deliberately NOT `Copy`: `Buffer` carries a linear `CommitToken` that must
/// be moved, not duplicated.
#[derive(Debug)]
pub enum Payload {
    Unit,
    F32(f32),
    U32(u32),
    Values { len: u8, data: [f32; MAX_RT_VALUES] },
    Buffer(CommitToken),
}

#[derive(Debug)]
pub struct RtCommand {
    pub domain: u8,
    pub id: u16,
    pub payload: Payload,
}

const _: () = assert!(core::mem::size_of::<RtCommand>() <= 560);
```

Core 1 routes:

```rust
if cmd.domain == DOMAIN_RIG {
    rig.apply(cmd.id, cmd.payload);
} else {
    program.apply(cmd.domain, cmd.id, cmd.payload);
}
```

`Values` is flat and length-tagged rather than `FourierCoeffs<H>`, because the
payload must be non-generic while `HARMONICS` becomes a per-programme const
generic. The `(domain, id)` pair determines the meaning, so the receiving
component reconstructs its own typed value. One `Values` command carries a
complete multi-actuator force vector.

### 2. Transactional writes, ordered centrally

`params.rs:596,623` currently enqueues before updating the shadow and rolls back
a blob commit on failure. That ordering is a correctness property: a host
receiving `Busy` must not then read back the value it was denied.

```rust
// helic-rt
pub enum Staged {
    Local(ParamAction),
    Rt(RtCommand),
}

pub enum ParamAction { None, Reboot, ResetDiagnostics }

pub trait ParamGroup {
    fn params(&self) -> &'static [ParamDef];
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode>;

    /// Validate and stage. Must not alter host-observable state.
    ///
    /// **A failing `stage` must leave no pending state**, because the store
    /// returns early on `Err` and never calls `reject`.
    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode>;

    fn accept(&mut self, id: u16);

    /// The command could not be enqueued. `returned` is the command the queue
    /// gave back, so a group holding a linear `CommitToken` can cancel it.
    fn reject(&mut self, id: u16, returned: Option<RtCommand>);

    /// Broadcast target for `ParamAction::ResetDiagnostics`.
    fn reset_diagnostics(&mut self) {}

    fn set_block(&mut self, _id: u16, _offset: u32, _data: &[u8]) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn stage_commit(&mut self, _id: u16, _len: u32) -> Result<Staged, ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
}
```

Ordering is written exactly once:

```rust
impl ParamStore {
    pub fn set(&mut self, index: usize, data: &[u8]) -> Result<ParamAction, ErrorCode> {
        let (g, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        match self.groups[g].stage(id, data)? {
            Staged::Local(ParamAction::ResetDiagnostics) => {
                self.groups[g].accept(id);
                // `diag_reset` spans groups: today it resets both the RT
                // atomics and every experiment-owned event counter
                // (`params.rs:543`). Component ownership makes that a
                // store-level broadcast.
                for group in self.groups.iter_mut() {
                    group.reset_diagnostics();
                }
                Ok(ParamAction::None)
            }
            Staged::Local(action) => { self.groups[g].accept(id); Ok(action) }
            Staged::Rt(cmd) => match self.commands.enqueue(cmd) {
                Ok(()) => { self.groups[g].accept(id); Ok(ParamAction::None) }
                // heapless returns the value on failure, so the linear token
                // travels back to its owner instead of being dropped.
                Err(cmd) => {
                    self.groups[g].reject(id, Some(cmd));
                    Err(ErrorCode::Busy)
                }
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

**`table_len` must be published by core 1.** It is specified as the *active*
table length (`protocol.md:168`) and today core 0 reads it through the shared
static (`params.rs:358`). Once `Active<WaveTable>` belongs exclusively to core 1,
`TableShadow` cannot see it, and updating a shadow on `accept` would report the
*pending* length one activation early. Core 1 therefore stores the active length
into an atomic when it consumes the activation token, and `TableShadow::get`
reads that atomic. It is an independent scalar, so this respects the coherence
rule.

**Parameter index order need not be preserved.** Both host libraries build
name-to-parameter maps at discovery and derive indices from it
(`device.py:160`, `device.jl:191`). Platform parameters can be an ordinary
group, and the golden registry test asserts the *set* of
`(name, type, count, writable)`. Stream **source** order is preserved, since it
defines the record layout.

```rust
pub enum ParamKind { Scalar, Array(u16), Blob(u32) }
```

### 3. Sound generic double buffering

The existing `table.rs` is correct because its accessors are private and its two
call sites are controlled. Exposing that discipline as a safe API has now failed
twice; this version fixes the three remaining defects.

```rust
// helic-core/src/double_buffer.rs
use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::sync::atomic::AtomicU32;

pub enum BufferError { Busy }

pub struct DoubleBuffer<T> {
    buffers: [UnsafeCell<T>; 2],
    /// Active id, pending flag and id, and a generation incremented on every
    /// commit. The generation is what makes a superseded token detectable; a
    /// bare buffer id has only one bit and collides with the next commit.
    state: AtomicU32,
}

unsafe impl<T: Send> Sync for DoubleBuffer<T> {}

/// Linear proof that exactly one commit is outstanding. Neither `Copy` nor
/// `Clone`: it is created by `Staging::commit`, moved into the command, and
/// consumed by exactly one of `Active::activate` or `Staging::cancel`.
/// `Send` (it crosses cores inside `RtCommand`) but not duplicable.
#[derive(Debug)]
pub struct CommitToken {
    owner: usize,      // buffer identity, so a token cannot address another
    generation: u32,   // supersession detection, including the target id
}

impl<T: Send> DoubleBuffer<T> {
    pub const fn new(a: T, b: T) -> Self;

    /// Split **once** into two uniquely owned endpoints. Taking `&'static mut`
    /// makes a second split impossible, exactly as `heapless::Queue::split`
    /// does. Obtain it from a `ConstStaticCell`, which is const-initialised
    /// (no stack copy of a 16 KB table) and panics on a second `take`.
    pub fn split(&'static mut self) -> (Staging<T>, Active<T>);
}

/// `Send` so it can be moved to the owning core; `!Sync` via `Cell` so two
/// threads cannot share one endpoint and obtain `&T` for a `!Sync` `T`.
pub struct Staging<T: 'static> {
    buf: &'static DoubleBuffer<T>,
    _not_sync: PhantomData<Cell<()>>,
}

pub struct Active<T: 'static> {
    buf: &'static DoubleBuffer<T>,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: Send> Staging<T> {
    /// Exclusive access to the inactive buffer; the borrow is tied to
    /// `&mut self`, so a second call cannot alias the first.
    pub fn buffer(&mut self) -> Result<&mut T, BufferError>;
    pub fn commit(&mut self) -> Result<CommitToken, BufferError>;
    pub fn cancel(&mut self, token: CommitToken);
}

impl<T: Send> Active<T> {
    /// Tied to `&self`, and `activate` takes `&mut self`, so no borrow can
    /// survive an activation.
    #[inline]
    pub fn get(&self) -> &T;
    /// Consumes the token. A token from another buffer, or one whose
    /// generation has been superseded, is ignored rather than misapplied.
    pub fn activate(&mut self, token: CommitToken);
}
```

Soundness properties, each enforced rather than documented:

- **one endpoint pair only**, because `split` needs `&'static mut self`;
- **no two `&mut T`**, because the borrow is tied to `&mut Staging<T>`;
- **no borrow across activation**, because `get` takes `&self` and `activate`
  takes `&mut self`;
- **no concurrent `get` on a `!Sync` `T`**, because the endpoints are `!Sync`;
- **no token replay**, because `CommitToken` is linear and carries owner
  identity plus a generation, and at most one is outstanding;
- **no lost token on a full queue**, because `enqueue` returns the command and
  the store hands it to `reject`.

The generation counter is defence-in-depth once the token is linear; it is kept
because it is a few bits and closes the in-crate `Copy`-leak path.

`BufferError` is local to `helic-core`, which depends only on `libm`; returning
`helic_proto::ErrorCode` would create a `helic-core → helic-proto` edge that
does not exist.

### 4. The complete `Rig` and `Program` contracts

These are the definitive contracts, not sketches. Earlier revisions elided rig
methods for brevity, which obscured that `prepare_reboot` is a mandatory safety
path whose symbol the layout gate checks by name.

```rust
// helic-rt
pub const MAX_ACTUATORS: usize = 4;

pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];
    const ACTUATORS: &'static [(&'static str, &'static str)];
    const SAFETY_GATED: bool = false;

    fn init(&mut self);
    fn measure(&mut self, values: &mut [f32]);
    /// All actuators in one call, so a rig can write N channels and strobe
    /// ~LDAC once.
    fn actuate(&mut self, outputs: &[f32]);
    fn apply(&mut self, id: u16, payload: Payload);

    /// One bounded step towards the reboot-safe hardware state. Mandatory;
    /// same SRAM and timing constraints as `actuate`.
    fn prepare_reboot(&mut self, step: u8) -> bool;

    fn clamp_output(&self, _actuator: usize, value: f32) -> f32 { value }
    fn safe_output(&self, _actuator: usize) -> f32 { 0.0 }
    fn output_fault(&mut self, _inputs: &[f32]) -> bool { false }

    fn tick_start(&mut self) {}
    fn tick_end(&mut self) {}
    fn tick_phase_us(&self) -> Option<u32> { None }
}

pub trait Program {
    const OUTPUTS: usize;
    /// Minimum input count this programme indexes. Checked at setup against
    /// `R::INPUTS.len()`, so a hot-loop index cannot fault.
    const INPUTS_REQUIRED: usize;
    /// Command domains this programme claims. Checked for uniqueness and for
    /// not colliding with `DOMAIN_RIG`.
    const DOMAINS: &'static [u8];
    const SIGNALS: &'static [(&'static str, &'static str)];

    fn apply(&mut self, domain: u8, id: u16, payload: Payload);
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx, outputs: &mut [f32]);
    fn write_signals(&self, out: &mut [f32]);
    /// Programme-originated fault the rig cannot observe from its inputs.
    fn fault(&self) -> bool { false }
}
```

### 5. Safety: an atomic wrapper over a pure decision

The gate cannot be wholly pure. `arm` is written from core 0 (`params.rs:552`)
so a control-connection loss can quiet the output without command latency, which
means armed state must live in an atomic shared between cores. The resolution is
to make only the *decision* pure and host-testable:

```rust
// helic-rt: pure, no atomics, no statics
#[derive(Clone, Copy)]
pub struct SafetyState { pub armed: bool, pub tripped: bool }

#[derive(Clone, Copy, Default)]
pub struct SafetyEvents { pub quieted: bool, pub clamped: bool, pub newly_tripped: bool }

pub fn safety_decide<R: Rig>(
    rig: &R,
    state: SafetyState,
    fault: bool,
    commanded: &[f32],
    applied: &mut [f32],
) -> (SafetyState, SafetyEvents);
```

`helic-fw-rt` wraps it: read `shared.safety`, evaluate `rig.output_fault(inputs)`,
`program.fault()`, and a non-finite check over `commanded`, call
`safety_decide`, then write the successor state back to `shared.safety` and the
event counters to `shared.diagnostics`.

Contract:

- output buffer is `[f32; MAX_ACTUATORS]`, only the `R::ACTUATORS.len()` prefix
  used;
- setup asserts `P::OUTPUTS == R::ACTUATORS.len()` and
  `P::OUTPUTS <= MAX_ACTUATORS`;
- **a non-finite commanded output latches the trip**, rather than relying on
  `code_for_volts` silently substituting 0 V;
- faults are global and per-tick and may come from the rig, the programme, or
  the non-finite check; any of them latches one trip that quiets every actuator;
- clamping is per actuator index;
- counters are per tick, not per actuator;
- streamed actuator values are the post-safety applied values;
- a non-gated rig applies every output verbatim and the gate compiles away.

### 6. `Pll`: an explicit state machine

A naive "not locked implies fault" rule deadlocks: entering phase-locked mode
trips immediately, quiets the shakers, and removes the excitation needed to
acquire lock. The state machine makes acquisition a first-class state.

```text
Fixed ──enable──▶ Acquiring ──|error| < lock_tol for lock_dwell──▶ Locked
                      │                                              │
                      │ acquire_timeout exceeded                     │ |error| > unlock_tol
                      │ or input below min_amplitude                 │ for unlock_dwell
                      ▼                                              ▼
                   Fixed (reverts, no trip) ◀────── reset ────── LockLost ──▶ fault()
```

- **Only `LockLost` reports a fault**, and only after lock was previously
  acquired. `Acquiring` never trips, so the excitation that lock depends on is
  never removed by the attempt to lock.
- **Acquisition failure reverts to `Fixed`** at the setpoint frequency rather
  than tripping, so a failed attempt leaves a usable rig.
- Separate `lock_tol`/`unlock_tol` and `lock_dwell`/`unlock_dwell` give
  hysteresis in both amplitude and time.
- `min_amplitude` on the demodulated force and response suppresses lock claims
  on noise; samples below it are counted invalid and stall acquisition rather
  than driving the loop.
- Sustained increment saturation against the configured bounds for
  `saturation_dwell` is treated as loss of lock.
- `reset` returns to `Fixed` and clears the loop filter.

```rust
pub enum PllState { Fixed, Acquiring, Locked, LockLost }

impl<const H: usize> Pll<H> {
    /// Locks the phase difference between a **measured force** channel and a
    /// **measured response** channel. The commanded oscillator is not a valid
    /// force reference: shaker armature dynamics and force drop-out make
    /// DAC-command phase differ from applied-force phase exactly where
    /// appropriation operates.
    pub fn update(
        &mut self,
        frame: &HarmonicFrame<H>,
        force: f32,
        response: f32,
        dt: f32,
    ) -> u32;   // commanded increment, already clamped to configured bounds

    pub fn state(&self) -> PllState;
    pub fn phase_error(&self) -> f32;
    pub fn reset(&mut self);
}
```

### 7. Hot-path SRAM contract

Every method reachable per tick must satisfy the existing SRAM rule, including
the ones earlier revisions left unannotated: `Program::write_signals`,
`Program::fault`, all `Rig` safety hooks, `Active::get`, `safety_decide`, and
the vector `actuate` path.

- Each `<rig>-program` crate declares an `rt-sram` feature, enabled by its
  `fw-<rig>` crate, gating
  `#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]`.
- `check_rt_layout.py` gains required symbols for the programme step, apply,
  signal-writing, fault, safety decision, buffer activation, and vector
  actuation paths.
- Relying on LTO to inline these is contrary to the existing rule and is not
  accepted as evidence.

### 8. Source assembly, shared constants, and setup validation

Source assembly becomes a generic walk over `R::INPUTS`, `P::SIGNALS`,
`R::ACTUATORS`, and `cmd_epoch`. `comms/tcp.rs:16` also consumes
`source`/`source_count` and changes with this stage.

`HARMONICS` moves to a const generic on the programme; `MAX_TABLE_LEN` becomes a
const generic on `WaveTable<const N: usize = 4096>`, defaulted.

**External composition must be validated as centrally as internal composition
is today.** `ParamStore::validate()` and the source assembly run at setup and
check, with a test for each malformed case:

- parameter name uniqueness across all groups, ASCII, and per-category length;
- total parameter count within the `u16` index range;
- each group's parameters fitting the paged discovery budget;
- blob parameter maximum length fitting wire discovery;
- source name uniqueness and total encoded size within `DISCOVERY_HEADROOM`;
- `groups.len() <= MAX_GROUPS`;
- `P::OUTPUTS == R::ACTUATORS.len() <= MAX_ACTUATORS`;
- `P::INPUTS_REQUIRED <= R::INPUTS.len()`;
- source count `<= MAX_SOURCES`;
- **domain uniqueness** across the programme's claimed domains, and no
  collision with `DOMAIN_RIG`, since a duplicate misroutes silently rather than
  failing loudly.

## Examples

### Example 1: CBC after the change

```rust
// fw-cbc-rig/src/main.rs
static TABLE: ConstStaticCell<DoubleBuffer<WaveTable<4096>>> =
    ConstStaticCell::new(DoubleBuffer::new(WaveTable::empty(), WaveTable::empty()));

let (staging, active) = TABLE.take().split();

let mut store = ParamStore::new(channels.command_tx, config::SAMPLE_RATE);
store.push(PLATFORM.init(PlatformGroup::new(config::EXPERIMENT)));
store.push(PROGRAM.init(StandardShadow::new(&program)));
store.push(TABLE_GROUP.init(TableShadow::new(staging)));
store.push(RIG.init(CbcShadow::new()));
store.push(TELEMETRY.init(CbcTelemetry::new()));
store.validate();
```

The parameter registry is unchanged by name and the wire format is unchanged.
**CBC gains one source, `phase`, taking it from 14 to 15**, so the golden source
list, `protocol.md`, and the all-source hardware regression all change.

### Example 2: force appropriation

Four shakers, four response channels, four force channels, phase-locked
excitation.

```rust
// appropriation-program/src/lib.rs   (no_std, host-tested, no Embassy)
const SHAKERS: usize = 4;
const H: usize = 5;
const VECTOR_LEN: usize = SHAKERS * (1 + 2 * H);   // 44 values

pub const DOMAIN: u8 = 1;

mod ids {
    pub const FREQ_SETPOINT: u16 = 0;
    pub const FORCE_VECTOR: u16 = 1;
    pub const PLL_GAIN: u16 = 2;
    pub const TARGET_PHASE: u16 = 3;
    pub const FREQ_MIN: u16 = 4;
    pub const FREQ_MAX: u16 = 5;
    pub const EXCITATION_MODE: u16 = 6;
    pub const RESET: u16 = 7;
}

pub struct Appropriation {
    harmonics: HarmonicGenerator<H>,
    forces: [FourierSignal<H>; SHAKERS],
    pll: Pll<H>,
    mode: ExcitationMode,
    force_channel: usize,      // measured force, the PLL reference
    response_channel: usize,   // measured response
    phase: f32,
    freq_actual: f32,
    phase_error: f32,
}

impl Program for Appropriation {
    const OUTPUTS: usize = SHAKERS;
    const INPUTS_REQUIRED: usize = 2 * SHAKERS;   // forces then responses
    const DOMAINS: &'static [u8] = &[DOMAIN];
    const SIGNALS: &'static [(&'static str, &'static str)] = &[
        ("phase", "turn"), ("freq_actual", "Hz"),
        ("phase_error", "deg"), ("pll_state", "enum"),
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
        // `HarmonicGenerator::step` returns a frame borrowed from the
        // generator, so everything needing the frame happens inside this
        // block and the increment is installed only after the borrow ends.
        let next_increment = {
            let frame = self.harmonics.step(ctx.lut);

            for (j, force) in self.forces.iter().enumerate() {
                outputs[j] = force.sample(frame);
            }
            self.phase = frame.phase_turns();

            if self.mode == ExcitationMode::PhaseLocked {
                let inc = self.pll.update(
                    frame,
                    inputs[self.force_channel],      // measured force, not command
                    inputs[self.response_channel],
                    dt,
                );
                self.phase_error = self.pll.phase_error();
                Some(inc)
            } else {
                None
            }
        };

        // Normative: `freq_actual` is the increment that produced *this*
        // record's phase advance, not the one installed for the next tick.
        self.freq_actual = self.harmonics.frequency_hz(ctx.sample_rate);

        if let Some(inc) = next_increment {
            self.harmonics.set_increment(inc);
        }
    }

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn write_signals(&self, out: &mut [f32]) {
        out[0] = self.phase;
        out[1] = self.freq_actual;
        out[2] = self.phase_error;
        out[3] = self.pll.state() as u8 as f32;
    }

    /// Only a genuine loss of lock trips. `Acquiring` must not, or the trip
    /// removes the excitation that acquisition depends on.
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn fault(&self) -> bool {
        matches!(self.pll.state(), PllState::LockLost)
    }
}
```

Source budget: 8 inputs (4 measured force, 4 response), 4 programme signals, 4
actuators, `cmd_epoch` — **17 of 24**, at 4.352 Mbit/s.

The device PLL keeps one drive point at the commanded force-to-response phase.
The host computes the full multipoint, multiharmonic mode indicator from the
streamed forces and responses against the streamed `phase`, and writes the
resulting force vector as one atomic `Values` command.

### Example 3: a table as an ordinary component

```rust
impl ParamGroup for TableShadow {
    fn set_block(&mut self, id: u16, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        write_f32_block(self.staging.buffer().map_err(map_busy)?, offset, data)
    }

    fn stage_commit(&mut self, id: u16, len: u32) -> Result<Staged, ErrorCode> {
        if id != ids::TABLE { return Err(ErrorCode::BadIndex); }
        // Validate before committing, so a failing `stage_commit` leaves no
        // pending state for the store to have to unwind.
        self.validate_prefix(len)?;
        let token = self.staging.commit().map_err(map_busy)?;
        Ok(Staged::Rt(RtCommand {
            domain: DOMAIN, id, payload: Payload::Buffer(token),
        }))
    }

    /// The queue gave the command back, so the linear token comes home.
    fn reject(&mut self, _id: u16, returned: Option<RtCommand>) {
        if let Some(RtCommand { payload: Payload::Buffer(token), .. }) = returned {
            self.staging.cancel(token);
        }
    }

    fn accept(&mut self, _id: u16) {}

    /// `table_len` is the *active* length, which only core 1 can observe.
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        if id == ids::TABLE_LEN {
            out.copy_from_slice(&TABLE_ACTIVE_LEN.load(Ordering::Relaxed).to_le_bytes());
        }
        // ...
    }
}
```

Core 1, on consuming the token, activates and publishes the length:

```rust
(ids::TABLE, Payload::Buffer(token)) => {
    self.active.activate(token);
    TABLE_ACTIVE_LEN.store(self.active.get().len() as u32, Ordering::Relaxed);
}
```

### Example 4: what you touch to add a rig

| Task | Today | Proposed |
|---|---|---|
| New SISO rig, existing controller | rig crates only | rig crates only |
| Second actuator | `rig.rs`, `rt_loop.rs`, `params.rs`, `schema.rs` | rig crates only |
| Second controlled axis | `rt_loop.rs`, `params.rs`, `schema.rs` | rig crates only |
| Four-shaker appropriation rig | not expressible | rig crates only |
| Rig-specific estimator (e.g. RPM) | `helic-core` | rig crates only |
| Rig with no waveform table | not expressible | omit the component |
| Second buffered blob | copy `table.rs` | one `DoubleBuffer<T>` |
| Different harmonic count (≤16) | `firmware/common/src/lib.rs` | const generic |
| >24 sources, >4 actuators, >132 values | shared crates | shared crates, deliberately |
| New primitive with two consumers | `helic-core` | `helic-core`, correctly |

## Repository separation

A rig repository contains `<rig>-program` and `fw-<rig>`, and needs the shared
crates published or referenced as git dependencies, its own
`.cargo/config.toml`, its own `Cargo.lock`, and access to the layout gate. The
Embassy pinning question remains open. A reasonable path keeps the three
production rigs here and treats the crate boundary as the contract that
*permits* an external rig, verified by one out-of-workspace test rig.

## Migration plan

0. **`RtShared` defined in `helic-rt`** and the diagnostic and safety atomics
   moved into it, with `ParamStore` and the loop taking `&'static RtShared`.
   This is a **prerequisite for stage 1**, not an optional tidy-up: without it
   `ParamStore` cannot leave `helic-fw-common` without inverting the crate
   layering (see "Cross-core shared state").
1. **`helic-rt` created** by moving `Rig`, `TickSource`, `SampleRate`, and the
   parameter types out of `helic-fw-common` unchanged.
2. **`helic-fw-common` split** into `helic-fw-rt` and `helic-fw-support`,
   dependency rules added to CI, and the membership rule written into
   `helic-fw-support`'s crate documentation. Verify by ELF inspection and a
   loop-maximum measurement that injected `&'static RtShared` costs nothing on
   the tick path.
3. **`DoubleBuffer<T>` into `helic-core`** with the endpoint split, linear
   token, and `ConstStaticCell` construction. Reimplement `table.rs` on it.
   Includes the compile-fail tests.
4. **`RtCommand`, `Payload`, non-`Copy` propagation, and the returned-command
   rejection path.** Measure `COMMANDS_PER_TICK` WCET at 132-value payloads and
   confirm EABI copy helpers remain in SRAM. **This measurement gates the
   copy-versus-buffer decision.**
5. **`ParamGroup` stage/accept/reject, the `ParamStore` walk, the
   `ResetDiagnostics` broadcast, and `validate()`.** Largest single commit; must
   change no discovered parameter name and no source order.
6. **`Program` trait and `StandardProgram`**, the `phase` signal, and
   `table_len` published from core 1. Split the table into its own group.
7. **Bounded output vector**, `Rig::ACTUATORS`, slice `actuate`, `safety_decide`
   plus its atomic wrapper, `Program::fault`, the non-finite trip, and generic
   source assembly. Migrate all three rigs together.
8. **Const generics** for `HARMONICS` and `MAX_TABLE_LEN`.
9. **`RpmEstimator` moves** to `whirl-rig-program`.
10. **`Pll` into `helic-core`** with its state machine and bounds.
11. **Layout gate and `rt-sram` features** extended to the new hot-path symbols.
12. **Out-of-workspace test rig** as the final architectural acceptance test.

Stages 1 to 5 are worth doing regardless of MIMO: they remove the offset
arithmetic and the bespoke unsafe module without changing externally visible
behaviour.

## Tests

**`DoubleBuffer`** — `Busy` while pending; a superseded or foreign token is
ignored; `cancel` restores writability; a rejected commit leaves the active
buffer untouched; compile-fail tests for two `buffer()` borrows, a `get()`
borrow held across `activate()`, a second `split()`, and any attempt to copy a
`CommitToken`.

**Transactionality** — a full queue leaves every shadow at its pre-write value
for each parameter kind; a failed blob enqueue returns the token and leaves the
buffer writable and not pending; a failing `stage` leaves no pending state.

**Broadcast** — `diag_reset` clears RT diagnostics *and* every group's event
counters, matching `params.rs:543`.

**`table_len`** — reports the active length, not the pending one, across a
commit that has been staged but not yet activated.

**Atomic force vector** — one `Values` command updates every `FourierSignal` at
the same tick; a length mismatch updates none.

**Golden registry** — the *set* of `(name, type, count, writable)` per rig.
**Golden sources** — names *and order*, updated to include `phase`.

**Validation** — one test per malformed composition in §8, including duplicate
domains and `INPUTS_REQUIRED` exceeding `R::INPUTS.len()`.

**Safety** — `safety_decide` is pure and host-tested: rig fault, programme
fault, and non-finite output each latch the trip and quiet all actuators;
per-actuator clamping; counters per tick; non-gated rig verbatim.

**`Pll`** — acquisition from `Fixed` never reports a fault; lock is claimed only
after `lock_dwell` within `lock_tol`; hysteresis prevents chatter at the
boundary; acquisition timeout reverts to `Fixed` without tripping; only
`LockLost` faults; the commanded increment never leaves its bounds under any
input including divergent and non-finite ones; sub-`min_amplitude` input stalls
acquisition rather than driving the loop.

**Phase fidelity** — the `f32` `phase` source is within `2⁻²⁵` turn of the `u32`
accumulator, with the propagated estimator-error bound asserted.

**Compile-time** — `size_of::<RtCommand>()`; dependency rules in CI.

Hardware regression is the full sequential suite. Stage 4 requires a fresh
loop-maximum measurement; stages 6 and 7 require the complete suite plus a new
all-source run on each rig, since `phase` changes the source set. The `cmd_epoch`
coherence tests for coefficient replacement and table re-commit are the specific
evidence stages 3 and 6 must not degrade; 34 µs loop maximum at a fixed 36 µs
wake phase is the baseline.

## Risks and open questions

**Risks**

- Large change to a tick path with verified timing; mitigated by stage ordering
  and the golden tests.
- Core-1 inlining is newly load-bearing; a loop over shakers may not unroll as
  today's straight-line code does. Confirm by ELF inspection and timing.
- The 132-value payload is the main new timing risk, and stage 4 gates it.
- `Active::get()` per tick replaces a cached field.
- Non-`Copy` `RtCommand` touches every construction site; expected to be
  mechanical, but it is a wide diff.

**Open questions**

1. **Stroke limiting.** An amplitude clamp on force does not protect shaker
   displacement, which binds at low frequency. Rig `output_fault` on measured
   displacement, or a gate-level rate/DC hook? The one open question with a
   physical hazard attached; settle before any shaker rig is built.
2. **Per-actuator trip** remains deferred; confirm the deferral explicitly.
3. **`MAX_ACTUATORS = 4`** derives from the AD5064's channel count, not the
   application.
4. **`MAX_GROUPS = 8`** unjustified; current usage is five.
5. **`MAX_CTRL_PARAMS`/`MAX_RIG_PARAMS`/`MAX_EXTRA_PARAMS`** retire once groups
   own their storage; confirm.
6. **Device integrations** (`laser.rs`, `analog_spi.rs`) in a crate every rig
   depends on.
7. **Embassy pinning across repositories.**
8. **`HarmonicFrame` by borrow or by value.** `rt_program_proposal.md` left this
   open pending evidence that the borrow "forces an unwanted lifetime
   restriction". Example 2 is that evidence: a programme mutating the generator
   within the tick that uses the frame must scope the borrow explicitly. The
   reordering above is correct but fragile; by value costs 40 bytes at `H = 5`
   and 128 at `H = 16`. Decide with stage 6 ELF evidence.

## Revision history

**Revision 5.1** closes the one genuine implementation blocker and renames a
crate:

- **`RtShared` injected, and stage 0 added.** `ParamStore` reads twenty distinct
  items from `rt_loop`, so moving it to `helic-rt` while `rt_loop` goes to
  `helic-fw-rt` would have inverted the crate layering. Stage 1 was therefore
  not executable as written. The shared observation surface moves to `helic-rt`
  and is **injected** as `&'static RtShared` rather than kept as statics, so
  `helic-rt` holds no mutable global state and `ParamStore` is host-testable
  without one. `Diagnostics` and `Safety` are separate structs, with the
  boundary drawn at the `diag_reset` lifecycle, which turns a warning comment in
  `reset_diagnostics` into a type boundary.
- **`helic-fw-net` renamed `helic-fw-support`.** Only two of its five modules
  are networking; `time_watchdog`, `status_run`, and `laser.rs` are not. The
  property that unites them is running on core 0 and never being reachable from
  the tick. Because a vague name invites the accretion that made
  `helic-fw-common` the problem this split exists to fix, the name is backed by
  a written membership rule in the crate documentation: core 0 **and** used by
  every rig. `laser.rs` fails the second clause today, which forces open
  question 6 to be answered when the module moves rather than deferred by
  filing it somewhere plausible-sounding.

**Revision 5** responds to a second review. All seven blocking findings were
verified against the code and accepted:

- **`DoubleBuffer` was still unsound.** `split(&'static self)` was callable
  twice; `CommitToken: Copy` contradicted the claim it was consumed once, and a
  one-bit id collides with the next commit; `Active<T>` auto-derived `Sync` from
  `&'static DoubleBuffer<T>`, so two threads could obtain `&T` for a `!Sync`
  `T`. Now: `split(&'static mut self)` via `ConstStaticCell`, linear non-`Copy`
  token carrying owner identity and a generation, `!Sync` endpoints, non-`Copy`
  `Payload`/`RtCommand`, and the failed command returned to `reject` so the
  token is never dropped.
- **Component-wide actions were unhandled.** `diag_reset` spans groups
  (`params.rs:543`) and needs a store-level broadcast; `table_len` is the
  *active* length (`protocol.md:168`, `params.rs:358`) and must be published by
  core 1 rather than shadowed on accept. Added the requirement that a failing
  `stage` leaves no pending state, since the store cannot call `reject` then.
- **The "pure" safety gate could not implement its contract.** `arm` is written
  from core 0 (`params.rs:552`), so armed state cannot be pure core-1 data. Now
  a pure `safety_decide` under an atomic wrapper. Non-finite outputs latch a
  fault instead of relying on `code_for_volts` substituting 0 V. The `Rig`
  contract is now complete, including the mandatory `prepare_reboot`.
- **The PLL fault rule deadlocked acquisition.** Tripping whenever unlocked
  removes the excitation lock depends on. Replaced with
  `Fixed → Acquiring → Locked → LockLost`, where only `LockLost` faults and
  acquisition failure reverts to `Fixed`.
- **The worked programme did not compile.** `step` returns a frame borrowed from
  the generator, and the example called `set_increment` on the same field while
  the frame was live. Reordered, with `freq_actual` given a normative
  definition. Added `INPUTS_REQUIRED` so a hot-loop index cannot fault.
- **`phase` contradicted the compatibility claims.** Resolved by adding it to
  all three rigs and **withdrawing the "CBC unchanged" claim**; CBC goes from 14
  to 15 sources and the all-source hardware regression is re-run. The precision
  test now asserts the `2⁻²⁵` turn bound directly.
- **Hot-path methods lacked an enforceable SRAM contract.** `write_signals`,
  `fault`, and the rig safety hooks are annotated; rig-program crates gain
  `rt-sram` features and the layout gate gains the new symbols.

Application corrections accepted: bandwidth is 4.352 Mbit/s and 1.96 GB/hour,
not 3.6 and 1.6, and the hardware evidence covers 13 sources rather than 17, so
the proposed configuration is unverified; the PLL latency argument now compares
*incremental* delays and rests on jitter rather than a mean that double-counted
the estimator floor; and **the PLL now demodulates a measured force channel
rather than the commanded oscillator**, since shaker dynamics make DAC-command
phase differ from applied-force phase exactly where appropriation operates. The
device PLL is stated to be a single drive-point fundamental tracker, with the
full multipoint multiharmonic condition enforced host-side.

Two positions retained with reasoning rather than assertion: the **copied force
payload over a second `DoubleBuffer`**, now with the full comparison table and
gated on the stage-4 WCET measurement; and **`RpmEstimator` moving out** under a
stated two-consumer rule, with the reviewer's objection recorded. The
"measurement-side processing is host work" formulation was wrong and is replaced
by a rule about where the *inputs* exist. Setup validation for external
compositions was added, including domain uniqueness.

**Revision 4** settled the host/device division: estimation host-side, the PLL on
core 1, `phase` as a stream source, telemetry coherence resolved by construction,
`RpmEstimator` moved out, and parameter index order relaxed to a set.

**Revision 3** folded in the crate split into `helic-fw-rt`/`helic-fw-support`
(then named `helic-fw-net`),
raised `MAX_RT_VALUES` from 33 to 132 from the appropriation sizing, added the
autonomous-state rule, and justified the vector `actuate` from ~LDAC skew.

**Revision 2** responded to the first review: transactional writes via
`stage`/`accept`/`reject`, non-generic `Values`, the first `DoubleBuffer`
endpoint split, the MIMO safety contract, `domain` in the command address, the
portable `helic-rt` crate, and the capacity-qualified goal.
