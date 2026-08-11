# Rig decoupling: component-owned parameters, signals, and buffers

Status: implementation in progress; stages 0–6 completed 2026-08-11. Revision
11. Supersedes parts of `docs/rt_program_proposal.md`. Revision history and
review responses are at the end.

## Goal

> Composing a rig from existing platform primitives, within documented source,
> output, group, payload, and memory bounds, must require changes only in that
> rig's own repository.

Fixed capacities are part of a bounded real-time platform, and raising one is a
deliberate platform change with its own timing and memory evidence. A genuinely
new reusable DSP algorithm belongs in `helic-core` and a new peripheral driver
in `helic-drivers`; putting either in a rig crate would be the failure mode, not
the success case.

### Shared crates evolve additively

The goal prohibits a *new* rig forcing changes that *existing* rigs must absorb.
It does not prohibit the shared crates from growing, and conflating the two
leads to bad conclusions.

Promoting a proven module from one rig's repository into `helic-core` is purely
additive: no existing API changes, no existing rig compiles differently, and each
rig upgrades its dependency when it chooses. Development on the shared crates
therefore continues freely; a rig in another repository is affected only when it
decides to be.

This is what makes the multi-repository arrangement safe, and it implies a
compatibility discipline the shared crates must actually keep:

- **Minor version** — new types, new trait methods *with defaults*, new
  optional parameters, and a *newly introduced* capacity constant. Additive; no
  consumer changes.
- **Major version** — any change to an existing signature, any trait method
  without a default, any change to a wire-visible name or to the meaning of an
  existing parameter or source, and **any change to an existing capacity, in
  either direction**.
- Rigs pin a major version and upgrade deliberately.

Two of these deserve their reasons stated, because both are easy to get wrong.

Adding a trait method without a default breaks every rig implementing that
trait, which is why the `Rig` and `Program` contracts here give defaults
wherever a rig could reasonably not care.

**Capacity *increases* are breaking, not additive**, which is the
counter-intuitive one. Raising `MAX_RT_VALUES` changes
`size_of::<RtCommand>()` and therefore every command queue's SRAM footprint;
raising `MAX_ACTUATORS` changes the record and gate buffers; both change the
WCET. A rig that fitted its memory and timing budget can stop fitting after a
change it did not ask for. Where such a bound plausibly needs to vary per rig,
prefer a const generic with a default over a shared constant, so each rig pays
only for what it uses.

### Documented platform capacities

| Bound | Value | Set by |
|---|---|---|
| `MAX_SOURCES` | 24 | protocol discovery headroom |
| `MAX_ACTUATORS` | 4 | record and safety-gate buffers |
| `MAX_GROUPS` | 8 | `ParamStore` registry vector |
| `MAX_RT_VALUES` | 33 | widest copied payload; see "Payload width" |
| `MAX_FORCE_VALUES` | 132 | widest buffered force vector |
| `COMMAND_QUEUE_LEN` | 32 | existing |
| `COMMANDS_PER_TICK` | 2 | existing WCET bound |

### What earns a place in a shared crate

> A type earns a place in a shared crate by having **two actual consumers**, not
> by being conceptually general.

Otherwise `helic-core` accumulates speculative API that constrains refactoring
for users who never arrive. `RpmEstimator` therefore moves out to
`whirl-rig-program`, having one consumer (`whirl-rig/src/rig.rs:10`).

**Why this rule is cheap, which is the part that makes it work.** A reviewer
objected that moving `RpmEstimator` out means a second rotating rig would later
force a shared-code move, which sounds like exactly what the goal forbids. It is
not, because of the additive-evolution principle above: promoting the module
back into `helic-core` when a second consumer appears changes no existing API,
breaks no other rig, and requires no other repository to do anything. The rule
costs one additive minor release at the moment it bites, and in exchange keeps
speculative API out of the shared crates in every case where the second consumer
never appears.

`Pll` is the mirror image and is placed in `helic-core` **deliberately against
the letter of the rule**, because it is being specified here as a shared
primitive with the reuse intent stated up front rather than discovered. If that
intent proves wrong it moves into the appropriation rig's crate, which is the
same one-commit operation in the other direction.

The rule now settles the buffer in the other direction. Stage 4 measured two
copied 33-value coefficient commands at 73 µs and two copied 132-value commands
at 95–96 µs, both beyond CBC's unchanged 60 µs gate. Target coefficients,
forcing coefficients, and the waveform table are therefore three actual
consumers of one `DoubleBuffer<T>` protocol. `TableBuffer` remains a convenient
alias, and force vectors use the same proven owner-checked mechanism.

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

| | Copied payload (**rejected**) | Buffered force vector (**chosen**) |
|---|---|---|
| `MAX_RT_VALUES` | 132 | 33 |
| Queue SRAM | 32 × ~540 = 17.3 KB | 32 × ~140 = 4.5 KB |
| Buffer SRAM | — | 2 × 528 = 1.1 KB |
| **Total** | **17.3 KB** | **5.6 KB** |
| Per-boundary work | up to 1056 B copy | pointer swap |
| Data path | `FourierSignal` owns its bank | banks live in the buffer, read through `Active` |

Stage 4 resolved the measurement gate. At a 34 µs steady baseline, two fully
materialised 132-value commands took 95–96 µs and introduced 1 µs clock jitter;
two production-shaped 33-value target/forcing commands still took 73 µs.
Both materially exceed the predicted 2–4 µs copy increment and CBC's unchanged
60 µs acceptance limit. Two owner-checked coefficient-buffer activations took
55–56 µs, with fixed 36 µs wake phase and no jitter or faults. The buffered
choice is therefore measured rather than precautionary.

The correctness risk is controlled by using the Stage-3 protocol unchanged:
one `&'static mut` split, non-`Sync` endpoints, a linear owner-checked token,
and returned-command cancellation. Generalisation was earned only after the
measurement created the second and third consumers. The production command
queue falls from roughly 17.3 KB in the rejected wide build to 4.5 KB; a
132-value force buffer costs a further 1.1 KB.

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
| `helic-core` | DSP: generators, controllers, estimators, filters, tables, `Pll`, `DoubleBuffer<T>` | host |
| **`helic-rt`** (new) | `Rig`, `TickSource`, `Program`, `ParamGroup`, `ParamDef`, `Payload`, `RtCommand`, `ParamStore`, safety decision, source assembly | host |
| **`helic-fw-rt`** (new) | core 1: tick sources, `rt_mem`, `analog_spi`, PIO, loop driver, safety wrapper | cross-build |
| **`helic-fw-support`** (new) | core 0: `net/`, `comms/`, `time_watchdog`, `status_run` | cross-build |
| `helic-drivers` | chip and sensor logic, pure and host-testable | host |
| `<rig>-program` | that rig's programme, controllers, shadows, rig-specific DSP | host |
| `fw-<rig>` | that rig's `board.rs`, `config.rs`, `rig.rs`, `main.rs` | cross-build |

### Dependency rules, CI-checked

- `helic-core` depends only on `libm`.
- `helic-rt` depends on `helic-core`, `helic-proto`, `heapless`; no Embassy.
- `helic-fw-rt` depends on `embassy-rp` (for `pac`) but has no direct dependency
  on, or source use of, `embassy-net`, `embassy-time`, or `embassy-executor`.
  (`embassy-rp` itself brings `embassy-time` transitively.)
- `helic-fw-support` has **no** dependency on `helic-fw-rt`. Both consume the
  queue endpoint types and `&'static RtShared` from `helic-rt`, so no exception
  is needed.

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

/// Current and lifetime values. Never cleared by `diag_reset`.
pub struct Live {
    pub ticks: AtomicU32,
    pub loop_time_last_us: AtomicU32,
}

/// Exactly the thirteen values `diag_reset` clears, and nothing else, so
/// `Diagnostics::reset()` owns the whole struct.
pub struct Diagnostics {
    pub loop_time_max_us: AtomicU32,
    pub clock_jitter_us: AtomicU32,
    pub overruns: AtomicU32,
    pub tick_timeouts: AtomicU32,
    pub records_dropped: AtomicU32,
    pub command_backlog_max: AtomicU32,
    pub wake_phase_min_us: AtomicU32,   // resets to u32::MAX, not 0
    pub wake_phase_max_us: AtomicU32,
    pub t_measure_max_us: AtomicU32,
    pub t_actuate_max_us: AtomicU32,
    pub t_rest_max_us: AtomicU32,
    pub safety_clamp_ticks: AtomicU32,
    pub safety_quiet_ticks: AtomicU32,
}

/// Latched safety state, which survives `diag_reset`.
///
/// **Ownership is asymmetric and load-bearing**, so the atomics are private and
/// reached only through role-named operations. `armed` is written *only* by
/// core 0; `tripped` is latched 0→1 by core 1 and cleared only by core 0 on a
/// deliberate re-arm.
///
/// This is **encapsulation, not enforcement**: nothing stops a determined
/// caller invoking `disarm` from the wrong core. What it removes is the casual
/// path — a bare `pub` atomic that any code can simply store into, which is
/// precisely what admitted the stale write-back defect in revision 5.1. A
/// stronger scheme with separate per-core capability handles was considered and
/// rejected: it threads two more types through construction to guard against a
/// mistake that four role-named methods already make unmissable.
pub struct Safety {
    armed: AtomicU32,
    tripped: AtomicU32,
}

impl Safety {
    /// Core 1, once per tick.
    pub fn load_inputs(&self) -> SafetyInputs;
    /// Core 1 only. Monotonic 0→1; cannot clear an existing trip.
    pub fn latch_trip(&self);
    /// Core 0 only. Clears the trip first, so a still-present fault re-latches
    /// on the next tick rather than being masked.
    pub fn arm(&self);
    /// Core 0 only. Called on `arm = 0` and on control-connection loss.
    pub fn disarm(&self);
    /// Core 0, for the `safety` bitfield.
    pub fn flags(&self, diagnostics: &Diagnostics) -> u32;
}

/// `IDLE → REQUESTED → QUIESCED`, preserving the existing Release/Acquire
/// protocol from `reboot.rs`. Core 0 requests and awaits; core 1 observes and
/// acknowledges. The crate split separates those two sides, so this state has
/// to be injected like the rest.
///
/// **The operational sequence is normative**, because a literal reading of
/// "`ParamStore` returns `ParamAction::Reboot`" would otherwise have core 0
/// waiting on state nobody requested. On accepting the confirmation token, the
/// control server must, in order:
///
/// 1. `safety.disarm()`, so the actuator is quiet before anything else moves;
/// 2. `reboot.request()`;
/// 3. await `reboot.is_quiesced()`, bounded by a timeout;
/// 4. schedule the ROM reset.
///
/// Step 1 precedes step 2 because quiescence is a hardware-sequencing step, not
/// an output-safety one; the output must already be safe when it begins.
pub struct RebootShared { state: AtomicU32 }

impl RebootShared {
    pub fn request(&self);              // core 0, idempotent
    pub fn is_requested(&self) -> bool;  // core 1, in `.data.ram_func`
    pub fn mark_quiesced(&self);         // core 1, in `.data.ram_func`
    pub fn is_quiesced(&self) -> bool;   // core 0
}

pub struct RtShared {
    pub live: Live,
    pub diagnostics: Diagnostics,
    pub safety: Safety,
    pub reboot: RebootShared,
}
```

The boundaries between the structs are lifecycle boundaries. `reset_diagnostics`
today clears thirteen values and touches neither `TICKS` nor
`LOOP_TIME_LAST_US`, so those live in `Live`; a struct claiming to be
"everything `diag_reset` clears" must not contain them, or `reset()` cannot own
it. `safety_clamp_ticks` and `safety_quiet_ticks` sit in `Diagnostics` despite
their names, because `diag_reset` does clear them. What today is a warning
comment in `reset_diagnostics` becomes a type boundary with no reachable path
from `reset()` to `armed` or `ticks`.

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
pub const MAX_RT_VALUES: usize = 33;
pub const MAX_FORCE_VALUES: usize = 132;
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

const _: () = assert!(core::mem::size_of::<RtCommand>() <= 160);
```

Core 1 routes:

```rust
if cmd.domain == DOMAIN_RIG {
    rig.apply(cmd.id, cmd.payload);
} else {
    program.apply(cmd.domain, cmd.id, cmd.payload);
}
```

`Values` is retained for copied arrays no wider than one 16-harmonic
coefficient set. The `(domain, id)` pair determines its meaning. Wider arrays,
including a complete multi-actuator force vector, travel as `Buffer` tokens;
target and forcing coefficients also use buffers because their measured
two-command copied WCET exceeded the CBC gate.

### 2. Transactional writes, ordered centrally

`params.rs:596,623` currently enqueues before updating the shadow and rolls back
a blob commit on failure. That ordering is a correctness property: a host
receiving `Busy` must not then read back the value it was denied.

```rust
// helic-rt
pub enum Staged {
    Local(ParamAction),
    /// Payload only. The store builds the address from the group's declared
    /// domain and the local id it has already resolved, so a group **cannot**
    /// misaddress a command: an earlier revision let each group construct a
    /// complete `RtCommand`, which meant `validate()` was checking a
    /// declaration that nothing bound to behaviour.
    Rt(Payload),
}

pub enum ParamAction { None, Reboot, ResetDiagnostics }

/// Where a group's real-time commands go. An earlier revision used
/// `Option<u8>` plus a rule forbidding any group from claiming `DOMAIN_RIG`,
/// which rejected exactly the rig parameter group that has to claim it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CommandTarget {
    /// Every parameter is handled on core 0; this group never stages an
    /// `Rt` command.
    Core0,
    /// Rig hardware state, dispatched to `Rig::apply`.
    Rig,
    /// Programme sub-component, dispatched to `Program::apply`.
    Program(u8),
}

pub trait ParamGroup {
    /// Read **once** at registration and stored by `ParamStore`, so validation
    /// is authoritative rather than advisory: a later change to this method
    /// cannot alter routing behind `validate()`'s back.
    fn target(&self) -> CommandTarget { CommandTarget::Core0 }

    fn params(&self) -> &'static [ParamDef];
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode>;

    /// Validate and stage. Must not alter host-observable state.
    ///
    /// **A failing `stage` must leave no pending state**, because the store
    /// returns early on `Err` and never calls `reject`.
    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode>;

    fn accept(&mut self, id: u16);

    /// The staged command did not reach core 1, for any reason. `returned` is
    /// the payload, so a group holding a linear `CommitToken` can cancel it.
    /// Called on **every** post-`stage` failure path, not only a full queue.
    fn reject(&mut self, id: u16, returned: Option<Payload>);

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
/// A group and the target captured when it was registered, kept together so
/// there is no parallel-array alignment invariant to maintain.
struct GroupEntry {
    group: &'static mut dyn ParamGroup,
    target: CommandTarget,
}

impl ParamStore {
    pub fn set(&mut self, index: usize, data: &[u8]) -> Result<ParamAction, ErrorCode> {
        let (g, id) = self.locate(index).ok_or(ErrorCode::BadIndex)?;
        let entry = &mut self.entries[g];
        match entry.group.stage(id, data)? {
            Staged::Local(ParamAction::ResetDiagnostics) => {
                entry.group.accept(id);
                // `diag_reset` spans groups: today it resets both the RT
                // atomics and every experiment-owned event counter
                // (`params.rs:543`). Component ownership makes that a
                // store-level broadcast.
                for entry in self.entries.iter_mut() {
                    entry.group.reset_diagnostics();
                }
                Ok(ParamAction::None)
            }
            Staged::Local(action) => { entry.group.accept(id); Ok(action) }
            Staged::Rt(payload) => {
                // The address is built here, from the target captured at
                // registration and the id already resolved by `locate`.
                let domain = match entry.target {
                    CommandTarget::Rig => DOMAIN_RIG,
                    CommandTarget::Program(d) => d,
                    // A `Core0` group staging an RT command is a programming
                    // error `validate()` cannot catch, because it is dynamic.
                    // It must still unwind: returning early here without
                    // `reject` would strand a `CommitToken` and leave the
                    // buffer permanently unwritable. **Every** post-`stage`
                    // failure path returns the payload to its owner.
                    CommandTarget::Core0 => {
                        entry.group.reject(id, Some(payload));
                        return Err(ErrorCode::BadIndex);
                    }
                };
                match self.commands.enqueue(RtCommand { domain, id, payload }) {
                    Ok(()) => { entry.group.accept(id); Ok(ParamAction::None) }
                    // heapless returns the value on failure, so the linear
                    // token travels back to its owner instead of being dropped.
                    Err(cmd) => {
                        entry.group.reject(id, Some(cmd.payload));
                        Err(ErrorCode::Busy)
                    }
                }
            }
        }
    }

    /// The only index arithmetic in the firmware.
    fn locate(&self, index: usize) -> Option<(usize, u16)> {
        let mut base = 0;
        for (g, entry) in self.entries.iter().enumerate() {
            let n = entry.group.params().len();
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

**Parameter index order need not be preserved.** All three host libraries
resolve parameters by discovered name (`device.py:160`, `device.jl:191`,
`+helicdaq/Device.m:71`); none caches a fixed index. Platform parameters can be an ordinary
group, and the golden registry test asserts the *set* of
`(name, type, count, writable)`. Stream **source** order is preserved, since it
defines the record layout.

```rust
pub enum ParamKind { Scalar, Array(u16), Blob(u32) }
```

### 3. The owner-checked double buffer

Stage 3 first landed the proven concrete `TableBuffer`. Stage 4's failed copy
gate then created actual second and third consumers: the target and forcing
coefficient sets. The implementation consequently generalises the same code to
`DoubleBuffer<T>`, with `TableBuffer = DoubleBuffer<WaveTable>` as an alias and
`ValueBuffer<N> = DoubleBuffer<[f32; N]>` for force vectors. The endpoint split,
linear owner-checked token, and normative ordering protocol are unchanged.

**A note on the diagnosis, because it affects the lesson.** Of the six defects
found across those revisions — `split` callable twice, a `Copy` token,
auto-`Sync`, missing ordering, foreign-token `cancel`, and generation wrap —
only auto-`Sync` was actually caused by genericity. The other five lived in the
endpoint and token protocol and would have occurred identically in a specialised
type. The lesson is therefore not "avoid generics" but **start from the proven
concrete code**: every one of those defects came from re-deriving `table.rs`
rather than transcribing it.

**Two atomics, no packed state word, no CAS.** `table.rs`'s layout has `active`
written only by core 1, and `pending` written by both cores but always as a
whole-word store. There is no cross-core read-modify-write, so nothing for a
`compare_exchange` to defend. Implementation changed the idle encoding from 2
to 0, storing a pending bank as `bank + 1`: keeping the non-zero sentinel inside
the same object as two zeroed 16 KiB banks forced the complete `ConstStaticCell`
into `.data` rather than `.bss`, contrary to its purpose. The state machine is
otherwise unchanged.

**No generation counter.** An earlier revision encoded
`(generation << 1) | bank` in an `AtomicU32`. That was doubly broken: at
`generation = 0x7FFFFFFF` with `bank = 1` a live commit encodes as exactly the
`u32::MAX` sentinel, and from `2³¹` the shift discards the high bit so the
comparison can never match again — either way the buffer is permanently pending
and unusable. At one commit per sample at 8 kHz that is 3.1 days; at 20
commits/s, 3.4 years.

It was also unnecessary. The generation defended against a *duplicated* token,
which stopped being possible once the token became linear. Because `commit`
returns `Busy` while a commit is outstanding, and the token is consumed by
exactly one of `activate` or `cancel`, no stale same-buffer token can exist in
safe code. The counter was defence that had already been made redundant and was
never removed.

```rust
// helic-core/src/table_buffer.rs
use core::cell::{Cell, UnsafeCell};
use core::marker::PhantomData;
use core::sync::atomic::AtomicU8;

pub enum BufferError { Busy }

const NO_PENDING: u8 = 0;   // pending stores bank + 1, keeping new() all-zero

pub struct DoubleBuffer<T> {
    banks: [UnsafeCell<T>; 2],
    /// Written only by core 1, at activation.
    active: AtomicU8,
    /// Bank id plus one, or `NO_PENDING`. Written by core 0 on commit and
    /// cancel and by core 1 on activation, always as a whole-word store.
    pending: AtomicU8,
}

// Values move between uniquely owned core endpoints; `T: Send` is therefore
// the only value-type bound needed by the sharing discipline.
unsafe impl<T: Send> Sync for DoubleBuffer<T> {}

pub type TableBuffer = DoubleBuffer<WaveTable>;
pub type ValueBuffer<const N: usize> = DoubleBuffer<[f32; N]>;

/// Linear proof that exactly one commit is outstanding. Neither `Copy` nor
/// `Clone`: created by `Staging::commit`, moved into the command, and consumed
/// by exactly one of `Active::activate` or `Staging::cancel`. `Send`, because
/// it crosses cores inside `RtCommand`, but not duplicable.
#[derive(Debug)]
pub struct CommitToken {
    /// Address-derived identity of the owning buffer, checked by **both**
    /// consuming operations. Checking it in only one lets a token from another
    /// buffer clear this buffer's pending flag, after which core 0 can obtain
    /// `&mut` to a bank core 1 is about to read.
    owner: usize,
    bank: u8,
}

impl<T: 'static> DoubleBuffer<T> {
    pub const fn from_banks(first: T, second: T) -> Self;

    /// Split **once** into two uniquely owned endpoints. `&'static mut` makes a
    /// second split impossible, as `heapless::Queue::split` does. Obtain it
    /// from a `ConstStaticCell`: const-initialised, so no stack copy of a
    /// 16 KB table, and it panics on a second `take`.
    pub fn split(&'static mut self) -> (Staging<T>, Active<T>);
}

/// `Send`, so it can be moved to its owning core; `!Sync` via `Cell`, so one
/// endpoint cannot be shared between threads.
pub struct Staging<T> {
    buf: &'static DoubleBuffer<T>,
    _not_sync: PhantomData<Cell<()>>,
}

pub struct Active<T> {
    buf: &'static DoubleBuffer<T>,
    /// Cached bank id, updated only in `activate`, so `get()` performs no
    /// atomic operation on the tick path.
    current: u8,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T: 'static> Staging<T> {
    /// Exclusive access to the inactive bank; the borrow is tied to
    /// `&mut self`, so a second call cannot alias the first.
    pub fn buffer(&mut self) -> Result<&mut T, BufferError>;
    pub fn commit(&mut self) -> Result<CommitToken, BufferError>;
    /// Ignores a token belonging to another buffer.
    pub fn cancel(&mut self, token: CommitToken);
}

impl<T: 'static> Active<T> {
    /// Tied to `&self`, and `activate` takes `&mut self`, so no borrow can
    /// survive an activation.
    #[inline]
    pub fn get(&self) -> &T;
    /// Ignores a token belonging to another buffer.
    pub fn activate(&mut self, token: CommitToken);
}
```

**The transition protocol and its memory ordering are normative.** The borrow
API prevents local aliasing and says nothing about cross-core visibility.
Without Release/Acquire pairing, core 1 may not observe the staged writes and
core 0 may select a bank from a stale active id. `table.rs:13` states this
requirement today; this transcribes it.

```text
identity()                          // no storage: the buffer's own address
    self.buf as *const _ as usize

Staging::buffer()
    if pending.load(Acquire) != NO_PENDING { Busy }   // pairs with activate
    &mut banks[active.load(Acquire) ^ 1]

Staging::commit()
    if pending.load(Relaxed) != NO_PENDING { Busy }
    bank = active.load(Relaxed) ^ 1
    pending.store(bank + 1, Release) // Release: staged writes happen-before
                                     // the publication core 1 will Acquire
    CommitToken { owner: identity(), bank }

Staging::cancel(token)
    if token.owner != identity() { return }
    pending.store(NO_PENDING, Release)

Active::activate(token)
    if token.owner != identity() { return }
    p = pending.load(Acquire)        // pairs with commit's Release
    if p == NO_PENDING || p - 1 != token.bank { return }
    self.current = p - 1             // cached for get()
    active.store(self.current, Release)
    pending.store(NO_PENDING, Release)

Active::get()
    &banks[self.current]             // plain read, no atomic
```

The synchronisation cost is paid once per activation, not once per tick. Core 0
cannot write while `pending` is set, and by the time it clears, core 0's next
Acquire load observes both the cleared flag and the new active id, so the two
cores never target the same bank.

Soundness properties and how each is obtained:

| Property | Mechanism |
|---|---|
| one endpoint pair only | `split` takes `&'static mut self` |
| no two `&mut WaveTable` | borrow tied to `&mut Staging` |
| no borrow across activation | `get(&self)` versus `activate(&mut self)` |
| no endpoint shared between threads | endpoints are `!Sync` |
| no token replay or cross-buffer use | linear token, owner checked in both `activate` and `cancel` |
| no lost token on a full queue | `enqueue` returns the command; the store hands it to `reject` |
| no cross-core clobbering | single-writer `active`; whole-word stores to `pending` |

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

**Core 1 must never write `armed`.** An earlier revision had the decision
function return a successor `SafetyState` containing `armed`, which the wrapper
wrote back wholesale. That admits the interleaving: core 1 reads `armed = true`;
core 0 handles connection loss and stores `armed = false`; core 1 writes its
stale `armed = true`. The output is re-armed with no host action, during
precisely the event that is supposed to quiet it. Today's code does not have
this defect because `safety_gate` only ever reads `SAFETY_ARMED`.

The guarantee is therefore made structural rather than tested: **the outcome
type has no `armed` field**, so there is nothing core 1 could write back.

```rust
// helic-rt: pure, no atomics, no statics
#[derive(Clone, Copy)]
pub struct SafetyInputs { pub armed: bool, pub tripped: bool }

/// Deliberately carries no `armed`: it is core-0-owned, and `newly_tripped` is
/// a monotonic 0→1 latch rather than a value to publish.
#[derive(Clone, Copy, Default)]
pub struct SafetyOutcome { pub newly_tripped: bool, pub quieted: bool, pub clamped: bool }

pub fn safety_decide<R: Rig>(
    rig: &R,
    inputs: SafetyInputs,
    fault: bool,
    commanded: &[f32],
    applied: &mut [f32],
) -> SafetyOutcome;
```

`helic-fw-rt` wraps it:

```rust
let inputs = shared.safety.load_inputs();
let outcome = safety_decide(rig, inputs, fault, commanded, &mut applied);
if outcome.newly_tripped {
    // Monotonic 0→1 latch, never a plain store, so a concurrent core-0 re-arm
    // cannot be silently reverted and a stale read cannot clear the trip.
    shared.safety.latch_trip();
}
if outcome.quieted { shared.diagnostics.safety_quiet_ticks.fetch_add(1, Ordering::Relaxed); }
if outcome.clamped { shared.diagnostics.safety_clamp_ticks.fetch_add(1, Ordering::Relaxed); }
// There is no `armed` in `SafetyOutcome` to write back, and no public atomic
// to write it to. `arm`/`disarm` are core-0 operations.
```

The clear-then-arm order inside `Safety::arm` is preserved and remains correct
against this latch: a still-present fault re-latches on the next tick rather
than being masked.

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
                      │ (invalid samples stall the state;            │ OR invalid samples,
                      │  they do not themselves end it)              │ for unlock_dwell
                      ▼                                              ▼
                   Fixed (reverts, no trip) ◀────── reset ────── LockLost ──▶ fault()
```

**Invalid samples**, meaning non-finite, or force or response demodulated below
`min_amplitude`, are defined in every state:

| State | Behaviour on an invalid sample |
|---|---|
| `Fixed` | ignored; the loop is not running |
| `Acquiring` | does not drive the loop and does not count toward `lock_dwell`; acquisition stalls, and only `acquire_timeout` ends the state |
| `Locked` | counts toward `unlock_dwell`, exactly as an out-of-tolerance error does, and then enters `LockLost` |
| `LockLost` | ignored; the state is latched until `reset` |

- **Only `LockLost` reports a fault**, and only after lock was previously
  acquired. `Acquiring` never trips, so the excitation that lock depends on is
  never removed by the attempt to lock.
- **Acquisition failure reverts to `Fixed`** at the setpoint frequency rather
  than tripping, so a failed attempt leaves a usable rig. Re-entering
  `Acquiring` needs a fresh `set_enabled(true)`; `update` cannot enable itself,
  or the timeout would be meaningless.
- **`Fixed → Acquiring` happens only via `set_enabled(true)`**, which the
  programme calls when the excitation-mode command lands, never implicitly from
  `update`. The call is **state-guarded and idempotent**: `true` is a no-op in
  `Acquiring`, `Locked`, and `LockLost`, so replaying an unchanged
  `excitation_mode` cannot discard a valid lock.
- Separate `lock_tol`/`unlock_tol` and `lock_dwell`/`unlock_dwell` give
  hysteresis in **phase-error tolerance and in time**. Neither is an amplitude
  threshold; `min_amplitude` is the separate validity gate below.
- `min_amplitude` on the demodulated force and response suppresses lock claims
  on noise; see the invalid-sample table above for its effect in each state.
- Sustained increment saturation against the configured bounds for
  `saturation_dwell` is a loss-of-lock condition **only while `Locked`**. While
  `Acquiring`, saturation is expected as the loop slews toward the resonance and
  must not end the state; only `acquire_timeout` does.
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

    /// Called when the excitation-mode command lands. It is the only way into
    /// `Acquiring`, so `update` never enables itself: if it could, an
    /// acquisition timeout would achieve nothing, because the next tick would
    /// restart acquisition and the "reverts to a usable rig" property would be
    /// lost.
    ///
    /// **State-guarded, and therefore idempotent.** The transition depends on
    /// the current state, not on the command being an edge:
    ///
    /// - `true` from `Fixed` enters `Acquiring`;
    /// - `true` while `Acquiring` or `Locked` is a **no-op**;
    /// - `true` while `LockLost` is a no-op: that state is latched until
    ///   `reset()` or an explicit `set_enabled(false)`, matching the deliberate
    ///   re-arm required after a safety trip;
    /// - after an acquisition timeout the state is already `Fixed`, so a repeat
    ///   `true` retries without needing an intervening `false`.
    ///
    /// The no-op while `Locked` is the point. A host or broker replaying its
    /// configuration on reconnect re-sends `excitation_mode`; if that restarted
    /// acquisition it would discard a valid lock mid-measurement, and because
    /// `Acquiring` deliberately does not fault, it would do so silently.
    /// Forcing re-acquisition remains possible and stays explicit:
    /// `set_enabled(false)` then `true`, or `reset()`.
    pub fn set_enabled(&mut self, enabled: bool);

    pub fn state(&self) -> PllState;
    pub fn phase_error(&self) -> f32;
    /// Returns to `Fixed` and clears the loop filter.
    pub fn reset(&mut self);

    // Configuration, all validated on core 0 before the command is queued.
    pub fn set_gain(&mut self, gain: f32);
    pub fn set_target_phase(&mut self, degrees: f32);
    pub fn set_min_increment(&mut self, increment: u32);
    pub fn set_max_increment(&mut self, increment: u32);
    pub fn set_lock_tolerance(&mut self, degrees: f32);
    pub fn set_unlock_tolerance(&mut self, degrees: f32);
    pub fn set_lock_dwell(&mut self, seconds: f32);
    pub fn set_unlock_dwell(&mut self, seconds: f32);
    pub fn set_acquire_timeout(&mut self, seconds: f32);
    pub fn set_min_amplitude(&mut self, amplitude: f32);
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
- **each individual parameter definition fitting one discovery page.** Not each
  group: paged discovery advances until `next == n_params` (`protocol.md:136`),
  so registries and groups are expected to span pages. The real constraint is
  that a single definition must fit, or `next` could never advance past it;
- blob parameter maximum length fitting wire discovery;
- source name uniqueness and total encoded size within `DISCOVERY_HEADROOM`;
- `groups.len() <= MAX_GROUPS`;
- `P::OUTPUTS == R::ACTUATORS.len() <= MAX_ACTUATORS`;
- `P::INPUTS_REQUIRED <= R::INPUTS.len()`;
- source count `<= MAX_SOURCES`;
- **command target binding**, over the targets captured at registration. Because
  the store builds every command address from them, these checks govern actual
  routing rather than a declaration nothing is bound to:
  - `Program::DOMAINS` are unique and all non-zero, since zero is `DOMAIN_RIG`;
  - the set of `CommandTarget::Program(d)` group targets is exactly
    `Program::DOMAINS`, so no group addresses a sub-component the programme
    does not claim, and no claimed sub-component is unreachable;
  - at most one group targets `CommandTarget::Rig`;
  - `CommandTarget::Core0` groups are unconstrained here, since they never
    stage an `Rt` command; one that does anyway is rejected transactionally at
    run time, with its payload returned to `reject`.

  Note that a rig parameter group **must** be able to target `Rig`: `CbcShadow`
  and its equivalents change hardware state on core 1 and their commands have to
  reach `Rig::apply`.

## Examples

### Example 1: CBC after the change

```rust
// fw-cbc-rig/src/main.rs
static TABLE: ConstStaticCell<TableBuffer> =
    ConstStaticCell::new(TableBuffer::new());

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
    force_vector: ActiveValues<VECTOR_LEN>,
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
            // One token activates the complete force vector at one sample
            // boundary: a partial update would corrupt the mode shape.
            (ids::FORCE_VECTOR, Payload::Buffer(token)) => {
                self.force_vector.activate(token);
            }
            (ids::FREQ_SETPOINT, Payload::U32(inc)) => self.harmonics.set_increment(inc),
            (ids::PLL_GAIN, Payload::F32(v)) => self.pll.set_gain(v),
            (ids::TARGET_PHASE, Payload::F32(v)) => self.pll.set_target_phase(v),
            (ids::FREQ_MIN, Payload::U32(inc)) => self.pll.set_min_increment(inc),
            (ids::FREQ_MAX, Payload::U32(inc)) => self.pll.set_max_increment(inc),
            // Acquisition never starts implicitly from `step`, so an
            // acquisition timeout leaves the rig at its setpoint frequency
            // until the host asks again. `set_enabled` is state-guarded, so
            // this stays correct when a reconnecting host replays an unchanged
            // `excitation_mode`: it will not disturb an existing lock.
            (ids::EXCITATION_MODE, Payload::U32(m)) => {
                self.mode = ExcitationMode::from_u32(m);
                self.pll.set_enabled(self.mode == ExcitationMode::PhaseLocked);
            }
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
            let vector = self.force_vector.get();

            for (j, force) in self.forces.iter().enumerate() {
                let base = j * (1 + 2 * H);
                outputs[j] = force.sample_coefficients(
                    frame,
                    &vector[base..base + 1 + 2 * H],
                );
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

Its core-0 half, in the same file, sharing `mod ids` and the domain constant:

```rust
pub struct AppropriationShadow {
    staging: ValueStaging<VECTOR_LEN>,
    /* shadows + `pending` */
}

impl ParamGroup for AppropriationShadow {
    /// Declared once, and the only place this group says where its commands
    /// go. `ParamStore` builds every address from it.
    fn target(&self) -> CommandTarget { CommandTarget::Program(DOMAIN) }

    fn params(&self) -> &'static [ParamDef] {
        &[
            ParamDef::writable("freq_setpoint", ParamType::F32, 1),
            ParamDef::writable("force_vector", ParamType::F32, VECTOR_LEN as u16),
            ParamDef::writable("pll_gain", ParamType::F32, 1),
            ParamDef::writable("target_phase", ParamType::F32, 1),
            ParamDef::writable("freq_min", ParamType::F32, 1),
            ParamDef::writable("freq_max", ParamType::F32, 1),
            ParamDef::writable("excitation_mode", ParamType::U32, 1),
            ParamDef::writable("ctrl_reset", ParamType::U32, 1),
        ]
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        match id {
            ids::FORCE_VECTOR => {
                // Rejects non-finite values. Nothing host-observable changes
                // until `accept`.
                let values = deserialize_f32s::<VECTOR_LEN>(data)?;
                self.staging.buffer().map_err(map_busy)?.copy_from_slice(&values);
                let token = self.staging.commit().map_err(map_busy)?;
                self.pending = Some(Pending::Force(values));
                Ok(Staged::Rt(Payload::Buffer(token)))
            }
            // ... remaining ids, each returning `Staged::Rt(payload)`
            _ => Err(ErrorCode::BadIndex),
        }
    }

    fn accept(&mut self, _id: u16) { /* publish `self.pending` into the shadow */ }
    fn reject(&mut self, _id: u16, returned: Option<Payload>) {
        if let Some(Payload::Buffer(token)) = returned {
            self.staging.cancel(token);
        }
        self.pending = None;
    }
    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> { /* ... */ }
}
```

Source budget: 8 inputs (4 measured force, 4 response), 4 programme signals, 4
actuators, `cmd_epoch` — **17 of 24**, at 4.352 Mbit/s. That figure is an
extrapolation from the 13-source configuration verified in `notes.md:50`, not
itself verified.

The device PLL keeps one drive point at the commanded force-to-response phase.
The host computes the full multipoint, multiharmonic mode indicator from the
streamed forces and responses against the streamed `phase`, and writes the
resulting force vector as one atomic buffered activation command.

### Example 3: a table as an ordinary component

```rust
impl ParamGroup for TableShadow {
    /// Declared once. The group never constructs a command address itself.
    fn target(&self) -> CommandTarget { CommandTarget::Program(DOMAIN) }

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
        Ok(Staged::Rt(Payload::Buffer(token)))
    }

    /// The payload came back, so the linear token comes home.
    fn reject(&mut self, _id: u16, returned: Option<Payload>) {
        if let Some(Payload::Buffer(token)) = returned {
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
| Second buffered blob | copy `table.rs` | instantiate `DoubleBuffer<T>` |
| Different harmonic count (≤16) | `firmware/common/src/lib.rs` | const generic |
| >24 sources, >4 actuators, >132 force values | shared crates | shared crates, deliberately |
| New primitive with two consumers | `helic-core` | `helic-core`, correctly |

## Repository separation

A rig repository contains `<rig>-program` and `fw-<rig>`, and needs the shared
crates published or referenced as git dependencies, its own
`.cargo/config.toml`, its own `Cargo.lock`, and a layout gate and regression
runner it can drive from a rig-local profile rather than by editing this
repository's tooling (stage 12). The
Embassy pinning question remains open. A reasonable path keeps the three
production rigs here and treats the crate boundary as the contract that
*permits* an external rig, verified by one out-of-workspace test rig.

## Migration plan

0. **Completed 2026-08-11: create the `helic-rt` skeleton crate and define
   `RtShared`** (`Live`,
   `Diagnostics`, `Safety`, `RebootShared`), moving the diagnostic, safety, and
   reboot atomics into it, with `ParamStore` and the loop taking
   `&'static RtShared`. The crate must exist before anything can be defined in
   it, and this state must move before `ParamStore` can: otherwise stage 1
   inverts the crate layering (see "Cross-core shared state"). The three
   production firmware crates now each own one `RtShared`; host tests cover
   diagnostic lifecycle boundaries, safety ownership, flags, and the reboot
   handshake. Release ELFs pass the real-time layout gate. Hardware timing is
   deliberately deferred to the stage-2 measurement gate, where the complete
   firmware crate split will have landed.
1. **Completed 2026-08-11: move the existing portable runtime types** — `Rig`,
   `TickSource`, `SampleRate`, the command and record types, queue endpoints,
   source assembly, `ParamStore`, and the parameter types — out of
   `helic-fw-common` into `helic-rt`, unchanged. The legacy table module moved
   mechanically with `ParamStore` to avoid a reverse dependency; stage 3 still
   replaces it with the reviewed `TableBuffer` contract. The earlier wording
   also listed `Program` and `ParamGroup` here, but neither exists in the
   pre-migration code: `ParamGroup` remains deliberately introduced in stage
   5, and `Program` in stage 6. Firmware identity remains in firmware support
   and is injected into `ParamStore`, so an external rig does not inherit the
   shared runtime crate's identity. Host golden tests now pin the fixed
   parameter schema and current source-segment order.
2. **Completed 2026-08-11: split `helic-fw-common`** into `helic-fw-rt` and
   `helic-fw-support`, with the non-universal optoNCDT UART service in its own
   integration crate. Dependency rules are enforced in CI, and the membership
   rule is in `helic-fw-support`'s crate documentation. All three production
   ELFs pass the SRAM layout gate; the CBC hot loop, analogue transfer, reboot
   hand-off, and ARM EABI copy/clear helpers were also inspected explicitly in
   SRAM. On the W5500 CBC rig, the 8000-record all-source and 60000-record
   continuity regressions ran at 8 kHz with 34–35 us loop maxima, fixed 36 us
   wake phase, and no timing faults, drops, loss, or index gaps. This matches
   the 32–34 us reference within measurement granularity, so the injected
   `&'static RtShared` and crate boundary have no observed tick-path cost.
3. **Completed 2026-08-11: move the table buffer into `helic-core` as
   `TableBuffer`**, with a one-time endpoint split, a linear owner-checked
   token, and per-rig `ConstStaticCell` construction. Five state-machine tests,
   a 100000-cycle liveness test, and four compile-fail doctests cover the
   reviewed contract. The minimal non-`Copy` command change required to carry
   the token landed here; the general payload/address redesign remains stage
   4. `table_len` publication by core 1 also landed early to preserve its
   active-not-pending wire semantics during migration. Explicit disassembly
   found the resulting generic EABI copy call still reaching flash, so the SRAM
   shims and layout gate now cover all generic, 4-byte, and 8-byte copy/clear
   variants. W5500 CBC activation reached 46 us; steady all-source and
   60000-record regressions reached 34–35 us, with fixed 36 us wake phase and
   no timing faults, loss, drops, or gaps. Capacity remains fixed until the
   stage-8 const-generic migration. The original `pending = 2` idle encoding
   put the whole 32 KiB object in `.data`; zero-idle, `bank + 1` pending
   encoding restores the intended `.bss` construction without changing the
   state machine.
4. **Completed 2026-08-11: `RtCommand`/`Payload` redesign, address routing,
   returned-command rejection, and the copy-versus-buffer gate.** The exact
   two-command diagnostic measured copied 132-value payloads at 95–96 us with
   1 us jitter and copied 33-value coefficient payloads at 73 us. Both failed
   the unchanged 60 us CBC gate. Target and forcing sets now use
   `DoubleBuffer<FourierCoeffs<16>>`; two activations measured 55–56 us with
   fixed 36 us wake phase and zero faults. The production queue occupies
   0x118c bytes (4.5 KB), all realised EABI helpers remain in SRAM, and the
   final default image passed 8000-record all-source and 60000-record sustained
   regressions at 34–36 us with no timing faults, loss, drops, or gaps.
5. **Completed 2026-08-11: `ParamGroup` stage/accept/reject, the `ParamStore`
   walk, the `ResetDiagnostics` broadcast, and `validate()`.** Six component
   groups now own definitions, shadows, and staging. `ParamStore` resolves a
   global index once and constructs every real-time address directly from the
   captured target and group-local ID; groups cannot remap an accepted address.
   Host tests cover queue-full scalar and buffered rejection, active-only table
   length publication, diagnostic broadcast, registry preservation, direct
   table/controller/rig routing, and each malformed composition required by
   the design. The discovered parameter-name set and source order are
   unchanged. W5500 CBC regressions for all 14 sources and for 60000 sustained
   `adc0,out` records reached 35 us and 34 us respectively, with fixed 36 us
   wake phase and no timing faults, loss, drops, or gaps. A release-only table
   staging regression introduced during the group move was caught on hardware:
   mutations inside `debug_assert!` had again been optimised away. The calls are
   unconditional, and the table transaction test now also passes under
   `cargo test --release -p helic-rt`.
6. **Completed 2026-08-11: `Program` trait and `StandardProgram`**, the `phase`
   signal, and retention of core-1 `table_len` publication. `StandardProgram`
   now owns the controller, master accumulator, active coefficient and table
   endpoints, table player, and cached programme signals. The common loop
   routes claimed command domains through the statically selected programme,
   validates its input requirement, and assembles its discovered signals
   between rig inputs and the still-scalar applied output. `StepCtx`, named but
   not defined in the reviewed design, is the minimal immutable pair of sine
   LUT and sample rate. CBC consequently exposes 15 sources. Exact W5500
   firmware `0.1.0 77fa0e4` passed the 8000-record all-source and 60000-record
   sustained regressions at 34–35 us, with fixed 36 us wake phase and no timing
   faults, loss, drops, or gaps. Only CBC W5500 hardware was available; the
   other production ELFs and both wired W6100 variants have software evidence,
   not new physical evidence.
7. **Bounded output vector**, `Rig::ACTUATORS`, slice `actuate`, `safety_decide`
   plus its atomic wrapper, `Program::fault`, the non-finite trip, and generic
   source assembly. Migrate all three rigs together.
8. **Const generics** for `HARMONICS` and `MAX_TABLE_LEN`.
9. **`RpmEstimator` moves** to `whirl-rig-program`.
10. **`Pll` into `helic-core`** with its state machine and bounds.
11. **Layout gate and `rt-sram` features** extended to the new hot-path symbols.
12. **Decouple the safety and regression tooling.** `check_rt_layout.py` keys
    `REQUIRED_SYMBOLS` on the three package names (`check_rt_layout.py:31`) and
    `rt_regression.py` hard-codes three `RigProfile` entries with
    `--rig choices=RIGS`. Neither can serve a rig in another repository without
    editing this one. Both should take a rig-local profile — a small manifest
    file, or CLI-supplied ELF path and symbol list — with the current three
    profiles becoming data rather than code. Keep the mechanism proportionate: a
    profile file and arguments, not a plugin system.
13. **Out-of-workspace test rig** as the final architectural acceptance test. It
    must demonstrate all four of:
    - building against released or pinned shared crates;
    - running the layout checker over its own ELF from a rig-local profile;
    - defining a hardware-regression profile locally and having the runner
      **load and dry-run it against a mocked transport**;
    - running the dependency-rule CI check,

    each **without editing or copying anything in this repository**.

    This fixture tests the *repository boundary*, not the rig, so it
    deliberately does not require real hardware: a dry run proves the profile
    loads and the runner drives it. Actual hardware regression remains mandatory
    when a rig becomes production-supported, and is covered by the normal
    sequential suite. Until the four criteria hold, the Rust composition is
    decoupled but a fully supported rig still needs shared-repository changes,
    and the repository-separation claim is only partly realised.

Stages 1 to 5 are worth doing regardless of MIMO: they remove the offset
arithmetic and the bespoke unsafe module without changing externally visible
behaviour.

## Tests

**`TableBuffer`** — `Busy` while pending; `cancel` restores writability; a
rejected commit leaves the active bank untouched; a long run of
commit/activate cycles never reaches a state where a commit cannot be consumed.
**Ordering**: a host-side test that the values written before `commit` are
exactly the values observed after `activate`, exercised under `loom` if it can be
made to run on the state machine, and at minimum asserted by inspection of the
emitted orderings against the protocol in §3.

**Borrow rules (compile-fail doctests)** — two `buffer()` borrows, a `get()`
borrow held across `activate()`, a second `split()`, and any attempt to copy a
`CommitToken`. These earn a compile-fail harness because they are the
user-facing half of an `unsafe impl Sync`; a test that merely restates ordinary
field privacy would not.

**Cross-buffer token rejection** — table, coefficient, and value buffers, with
foreign same-type and cross-type tokens passed to `cancel` **and** to
`activate`. All must be ignored. This is its own test because checking the
owner in only one of the two consuming operations was a real unsoundness: a
foreign token could clear a buffer's pending flag, after which core 0 could
obtain `&mut` to a bank core 1 was about to read.

**Reboot handshake** — `request` → `is_requested` → `mark_quiesced` →
`is_quiesced` across the `helic-rt` boundary, with the idempotent repeat-request
behaviour of `reboot.rs` preserved.

**Transactionality** — a full queue leaves every shadow at its pre-write value
for each parameter kind; a failed blob enqueue returns the token and leaves the
buffer writable and not pending; a failing `stage` leaves no pending state.

**Broadcast** — `diag_reset` clears RT diagnostics *and* every group's event
counters, matching `params.rs:543`.

**`table_len`** — reports the active length, not the pending one, across a
commit that has been staged but not yet activated.

**Atomic force vector** — one owner-checked buffer token activates every force
coefficient at the same tick; rejection returns the token and leaves the active
bank untouched.

**Golden registry** — the *set* of `(name, type, count, writable)` per rig.
**Golden sources** — names *and order*, updated to include `phase`.

**Validation** — one test per malformed composition in §8: a duplicate
`Program(d)` across two groups; a zero entry in `Program::DOMAINS`; a
`Program(d)` group target absent from `Program::DOMAINS` and vice versa; more
than one group targeting `Rig`; a `Core0` group staging an `Rt` command, which
must be rejected transactionally with its payload returned; a single parameter
definition too large for one discovery page; and `INPUTS_REQUIRED` exceeding
`R::INPUTS.len()`.

**Safety** — `safety_decide` is pure and host-tested: rig fault, programme
fault, and non-finite output each latch the trip and quiet all actuators;
per-actuator clamping; counters per tick; non-gated rig verbatim.

**Safety ownership** — a behavioural concurrency test that a core-0 disarm
occurring between the wrapper's `load_inputs` and its `latch_trip` leaves the
output disarmed, and that `latch_trip` never clears an existing trip. No
compile-fail harness: `Safety`'s atomics are private, and Rust already proves
that a private field cannot be written from outside its module.

**`Pll`** — acquisition from `Fixed` never reports a fault; lock is claimed only
after `lock_dwell` within `lock_tol`; hysteresis prevents chatter at the
boundary; acquisition timeout reverts to `Fixed` without tripping; only
`LockLost` faults; the commanded increment never leaves its bounds under any
input including divergent and non-finite ones; sub-`min_amplitude` input stalls
acquisition rather than driving the loop. **Entry**: after an acquisition
timeout, repeated `update` calls leave the state at `Fixed`, and only a fresh
`set_enabled(true)` re-enters `Acquiring`. **Idempotence**: `set_enabled(true)`
while `Locked` leaves the state, the loop filter, and the commanded increment
untouched — the direct regression test for a host or broker replaying an
unchanged `excitation_mode` on reconnect — and is likewise a no-op in
`Acquiring` and `LockLost`. **Saturation**: sustained saturation while
`Acquiring` does not end the state, while the same saturation once `Locked`
produces `LockLost`.

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
- The 132-value copied payload failed its timing gate and is retained only as a
  diagnostic comparison feature; production force vectors are buffered.
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
8. **Resolved at Stage 6: retain `HarmonicFrame` by borrow.** `StandardProgram`
   does not need a frame, and its realised CBC hot loop introduced no
   flash-resident calls. The appropriation sketch already demonstrates the
   only lifetime restriction: all frame consumers sit in one explicit scope,
   after which the generator increment may change. Copying 40 bytes at `H = 5`
   or 128 bytes at `H = 16` would add work and potential EABI helpers to the
   mandatory path merely to avoid that visible scope. Revisit only if the
   Stage-10 implementation produces contrary compiler evidence.

## Revision history

**Revision 11** records the completed programme extraction. The statically
selected `Program` owns controller, Fourier, table, and signal state;
`StandardProgram` preserves sample-boundary command routing and active
`table_len` publication. The new coherent `phase` source raises CBC discovery
from 14 to 15 sources. Exact W5500 hardware passed focused phase/forcing and
table checks plus the all-source and sustained regression gates with 34–35 us
maxima and no faults or continuity failures. The reviewed design named but did
not define `StepCtx`; the implementation supplies only the immutable sine LUT
and sample rate. `HarmonicFrame` remains borrowed because the explicit scope is
cheap and avoids an otherwise unmotivated hot-path copy.

**Revision 10** records the completed component-owned parameter composition.
`ParamStore` now walks six statically allocated groups and binds commands to
the located target and local ID, with no post-validation remapping hook.
`table_len` is platform live telemetry, leaving the table group's eight local
IDs identical to its eight core-1 commands; controller reset is local ID zero,
and controller-specific parameters follow from one. The W5500 hardware gate
also caught a migration regression in which table writes and length publication
were placed inside `debug_assert!` and therefore removed from release firmware.
The mutations are unconditional, the optimised host test passes, and the final
hardware image activates and streams the complete table while disarmed.

**Revision 9** records the Stage-4 measurement result and the consequent design
change. Two fully materialised 132-value copied commands measured 95–96 us, and
two 33-value coefficient commands measured 73 us, against CBC's unchanged 60 us
limit. Both force vectors and coefficient sets therefore use owner-checked
double buffers. Two coefficient activations measured 55–56 us. The previously
concrete table implementation is now `DoubleBuffer<T>` because target,
forcing, and table storage are three actual consumers; `TableBuffer` remains an
alias. `MAX_RT_VALUES` returns to 33 for copied arrays, and
`MAX_FORCE_VALUES = 132` names the buffered force-vector capacity.

**Revision 8.1** settles one behavioural point raised alongside the fifth
review. Revision 8 said every `set_enabled(true)` restarts acquisition, so a
host or broker replaying its configuration on reconnect would re-send an
unchanged `excitation_mode` and discard a valid lock. Worse, because `Acquiring`
deliberately does not fault, it would do so **silently**, corrupting an
appropriation measurement with no diagnostic.

`set_enabled` is now **state-guarded and idempotent**: `true` enters `Acquiring`
only from `Fixed`, and is a no-op in `Acquiring`, `Locked`, and `LockLost`.
Retry after an acquisition timeout still needs no intervening `false`, because
the timeout already returns the state to `Fixed`. Forcing re-acquisition stays
possible and explicit via `set_enabled(false)` then `true`, or `reset()`. This
needs less state than the previous rule, not more, and makes the parameter
behave like every other setter in the registry.

**Revision 8** responds to a fifth review, which approved the direction subject
to one buffer correction.

- **The generation counter is removed, and it carried a real defect.** The
  encoding `(generation << 1) | bank` in an `AtomicU32` with a `u32::MAX`
  sentinel fails twice: at `generation = 0x7FFFFFFF` with `bank = 1` a live
  commit encodes as exactly the sentinel, and from `2³¹` the shift discards the
  high bit so the comparison can never match again. Either way the buffer is
  permanently pending. At one commit per sample at 8 kHz that is 3.1 days; at
  20 commits/s, 3.4 years — inside the life of the equipment.

  It was also unnecessary. The generation defended against a *duplicated*
  token, which stopped being possible when the token became linear. The token
  is now `{ owner, bank }` over `active: AtomicU8` and `pending: AtomicU8`.
  Implementation retained the two-atomic state machine but changed the idle
  encoding from the old `2` sentinel to zero for `.bss` placement, as recorded
  in §3.
- **`DoubleBuffer<T>` becomes concrete `TableBuffer`.** Four revisions were
  spent making a generalisation safe for a design with zero consumers of it,
  which the document's own two-consumer rule forbids. The endpoint split, the
  linear owner-checked token, and the ordering protocol are all kept; only the
  type parameter goes, to return additively when a second buffered component
  exists.

  The diagnosis is recorded precisely, because it changes the lesson: of the six
  defects across those revisions, **only auto-`Sync` was caused by genericity**.
  The rest lived in the endpoint and token protocol and would have occurred
  identically in a specialised type. The lesson is not "avoid generics" but
  *start from the proven concrete code* — every defect came from re-deriving
  `table.rs` rather than transcribing it. Stage 3 now says transcribe.

Clean-ups: the validation *test* list was still describing the pre-
`CommandTarget` scheme and now matches it; `GroupEntry` replaces parallel
`groups`/`targets` arrays and the alignment invariant they implied;
`set_enabled` is documented as state-guarded (see revision 8.1), since after a timeout a
repeat mode command must restart acquisition without an intervening `false`;
the privacy compile-fail test is dropped, because Rust already proves private
fields are private, while the buffer's borrow-rule doctests stay as the
user-facing half of an `unsafe impl`; and stage 13 requires a dry run against a
mocked transport rather than real hardware, since it tests the repository
boundary rather than a rig.

**Revision 7** responds to a fourth review. All five must-fix findings accepted;
two of the reviewer's suggested *mechanisms* were declined in favour of simpler
ones, with the reasoning recorded so they are not re-proposed.

- **[P0] A foreign token could cancel another buffer.** `activate` checked
  `token.owner` and `cancel` did not, so a same-generation token from another
  buffer could clear this buffer's pending flag, after which core 0 could take
  `&mut` to a bank core 1 was about to read. A safe API must be sound under
  misuse, so this was genuine unsoundness. The owner check is now made in both
  consuming operations, with a dedicated cross-buffer test.
  **The `compare_exchange` suggested alongside it is declined**, and the layout
  reverted instead: the read-modify-write hazard it would defend against was
  introduced by a packed single state word of my own invention. `table.rs` uses
  a core-1-only `active` plus a `pending` written only by whole-word stores, so
  no cross-core read-modify-write exists and no CAS is needed. That is both
  simpler and the layout already proven on hardware. `Active` also gains the
  `current` field its own pseudocode used; `id` derives from the buffer address
  and needs no storage.
- **[P1] Domain validation rejected the rig group it had to support.** Requiring
  every group domain to differ from `DOMAIN_RIG` excluded `CbcShadow`, which
  must reach `Rig::apply`. `Option<u8>` becomes
  `CommandTarget { Core0, Rig, Program(u8) }`, captured at registration so
  `validate()` is authoritative. The failure-path leak is closed by returning
  the payload to `reject` on **every** post-`stage` failure, not only a full
  queue; the `Core0`-group-stages-`Rt` case now unwinds instead of stranding a
  token.
- **[P1] Safety ownership was not structural, and the claim was wrong.** With
  `pub` atomics the proposed compile-fail test could not fail to compile. The
  atomics are now private behind `load_inputs`, `latch_trip`, `arm`, and
  `disarm`. **Separate per-core capability handles are declined**: they thread
  two more types through construction to guard a mistake four role-named
  methods already make unmissable. The guarantee is now described honestly as
  encapsulation rather than enforcement.
- **[P1] PLL entry was unspecified.** The diagram's `enable` edge existed
  nowhere in the API, and an `update` that enabled itself would make the
  acquisition timeout meaningless. `set_enabled(bool)` is edge-triggered and
  called when the mode command lands; the configuration methods the example
  already used are added to the definitive API.
- **[P1] The tooling is not decoupled.** `check_rt_layout.py:31` keys required
  symbols on the three package names and `rt_regression.py` hard-codes three rig
  profiles, so no external rig can use either without editing this repository.
  Stage 12 now decouples both onto rig-local profiles, and stage 13's acceptance
  criteria are explicit about running them unmodified.

Smaller corrections: capacity *increases* are breaking, not additive, since they
change `size_of::<RtCommand>()`, buffer sizes, and WCET for rigs that did not
ask; the reboot sequence is normative as disarm, request, await quiescence,
reset; and PLL hysteresis is in phase-error tolerance and time, with saturation
a loss-of-lock condition only while `Locked`.

**A pattern worth recording.** Three revisions running, a property was described
as guaranteed by construction when it was guaranteed by convention: the safety
write-back, the domain declaration nothing was bound to, and `pub` atomics
behind an impossible compile-fail test. The related tendency shows in the buffer:
four attempts, each moving further from a design already working on hardware,
with the correct fix being to revert to it. Prefer the proven concrete
implementation over a generalisation of it, and do not claim enforcement where
there is only encapsulation.

**Revision 6** responds to a third review. All five must-fix findings were
verified against the code and accepted; three of them were defects introduced by
revision 5.1's own tidying, which is itself the lesson.

- **[P0] The safety wrapper could undo a concurrent disarm.** Returning a
  successor `SafetyState` containing `armed` turned a read-only input into a
  read-write round trip, admitting: core 1 reads `armed = true`; core 0 stores
  `armed = false` on connection loss; core 1 writes back `true`. Today's
  `safety_gate` has no such defect because it only ever *reads* `SAFETY_ARMED`.
  Fixed structurally rather than by test: `SafetyOutcome` has no `armed` field,
  and `tripped` is latched with `fetch_or`.
- **[P0] `DoubleBuffer` had no memory-ordering specification.** The borrow API
  prevents local aliasing and says nothing about cross-core visibility.
  `table.rs:13` already states the requirement precisely for the concrete case;
  §3 now gives the state word, the transition protocol, and the
  Acquire/Release pairing normatively, including that `Active` caches `current`
  so `get()` needs no atomic on the tick path.
- **[P1] Reboot coordination was missing.** `reboot.rs` holds shared
  `IDLE → REQUESTED → QUIESCED` state with core 0 requesting and core 1
  acknowledging; the crate split separated those without providing for it.
  `RebootShared` joins `RtShared`.
- **[P1] Domain validation could not validate routing.** Groups constructed
  their own complete `RtCommand`, so `validate()` checked a declaration nothing
  was bound to. `Staged::Rt` now carries the payload only, groups declare
  `domain()`, and `ParamStore` builds every address.
- **[P1] The diagnostics lifecycle was self-contradictory.** A struct described
  as "everything `diag_reset` clears" contained `ticks` and
  `loop_time_last_us`, which it clears neither of. Split into `Live` and
  `Diagnostics`, so `reset()` genuinely owns its whole struct.

Refinements accepted: the PLL invalid-sample policy is now defined in every
state, with a table, resolving a diagram/prose contradiction; validation
requires each *parameter definition* to fit a page rather than each group, since
`protocol.md:136` has registries spanning pages by design; stage 0 creates the
crate before stage 1 populates it; `helic-fw-support` has no dependency on
`helic-fw-rt` at all; the copied force payload's once-per-period rate is stated
as an operating expectation with two maximum-width commands per tick retained as
the WCET case; and the parameter-order evidence now covers all three host
libraries, MATLAB included (`+helicdaq/Device.m:71`).

**The two-consumer rule is retained, and its justification corrected.** A
reviewer objected that moving `RpmEstimator` out means a second rotating rig
would force a shared-code move. The objection dissolves under the
additive-evolution principle now stated in the goal: promoting a module back
into `helic-core` changes no existing API and requires no other repository to do
anything. The goal forbids a *new* rig imposing changes on *existing* rigs, not
the shared crates growing. `RpmEstimator` therefore stays in
`whirl-rig-program`, and the rule keeps speculative API out of `helic-core` in
every case where the second consumer never arrives.

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
