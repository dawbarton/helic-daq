# Selectable control and phase-locked excitation

This is a copy of the reviewed design record from the magneto-elastic rig.
References to `docs/control.md` and the CBC Duffing method note describe the
source repositories in which the proposal was developed; they are retained as
provenance rather than current platform documentation.

## Status and scope

This document is the reviewed design record for the implemented run-time
choice between no feedback controller, PID
displacement control, and phase-locked excitation for the magneto-elastic rig.
It covers the required changes to the HELIC-DAQ platform and this rig, including
an in-place revision of `helic_core::Pll`. It does not specify numerical gains
or operating bounds. Those require simulation and hardware evidence. The
software implementation is present locally against unreleased platform 0.3.0;
`docs/control.md` states the resulting operator contract and the deliberate
PID/PLL commissioning blockers.

The release clean-up subsequently made `helic_core::Pll` non-generic because
it stores no harmonic-sized state; `Pll::update<const H>` accepts the shared
frame. Any `Pll<H>` spellings in the proposal and review record below are
historical sketches, not the released API.

The phase-locked use case is the backbone and nonlinear frequency-response
method described by the CBC Duffing project's `docs/methods/phase-locked-loop.md`:
fundamental excitation-to-response phase is controlled through excitation
frequency, excitation amplitude is stepped for backbone tracing, and a slower
outer amplitude loop is used when a nonlinear FRF requires constant response
amplitude.

## Decision

`StandardProgram` should change, but it should not contain a PLL or know about
the rig's three control modes. It should acquire a generic, harmonic-frame-aware
control seam with the following responsibilities:

- receive the generated reference, the current harmonic frame, the measured
  inputs, forcing, table value, and the sample interval;
- return the complete raw programme output, so mode-specific composition rules
  can be enforced rather than left to operator discipline;
- optionally return the phase increment to use on the next tick;
- expose fixed telemetry, a reset hook, command application, and a fault;
- declare its minimum measured-input count; and
- remain statically dispatched, allocation-free, and SRAM-resident.

The selectable policy is specific to this rig and belongs here. The underlying
PID and PLL remain reusable primitives in `helic-core`. In particular, the
existing `helic_core::Pll` is to be changed in place rather than supplemented by
a second PLL implementation.

This split keeps the common target, forcing, table, phase, command-routing, and
telemetry machinery in the platform. It also avoids copying `StandardProgram`
into the rig merely to obtain access to its private phase accumulator.

## Control semantics

The proposed `control_mode` parameter is a writable `u32` enum:

| Value | Mode | Raw programme output | Master frequency |
|---:|---|---|---|
| 0 | `None` | `forcing + table` | standard `freq` parameter |
| 1 | `Pid` | `PID(target - laser) + forcing + table` | standard `freq` parameter |
| 2 | `Pll` | `forcing`; table is ignored | bounded output of the PLL |

Consequently:

- `target_coeffs` is always a displacement reference in millimetres. It is
  consumed only in PID mode.
- `forcing_coeffs` is always open-loop excitation in volts. It supplies the
  excitation in both `None` and `Pll` modes and remains additive feed-forward
  in PID mode.
- `table` remains an additive voltage in `None` and `Pid` modes. It is ignored
  in `Pll` mode, irrespective of `table_mode`, because a second waveform path
  must not silently invalidate the phase detector. A deliberately locked table
  may be admitted later, after its effect on the measured excitation reference
  is tested.

This gives each source one stable unit in every mode. It deliberately does not
preserve the present interpretation in which `PassThrough` sends `target`
straight to the actuator. Existing open-loop host code should use `forcing`, as
the current `sine` helper already does. Boot defaults remain zero, so this
change cannot energise the rig unexpectedly.

The outer amplitude controller needed for a constant-response-amplitude NLFRF
is not the PID mode above. It is a much slower host-side PI loop which adjusts
the fundamental coefficient in `forcing_coeffs`. Keeping it host-side makes the
required bandwidth separation explicit and leaves the firmware phase loop
deterministic. Fixed-excitation-amplitude backbone tracing needs only stepped
host writes to `forcing_coeffs`.

## Platform programme changes

### A harmonic-frame-aware control trait

Add a trait in `helic-rt`, provisionally named `StandardControl`, and make
`StandardProgram` generic over it. The exact Rust surface should be settled by
implementation, but the semantic contract is:

```rust
pub struct StandardControlInputs<'a, const H: usize> {
    pub measured: &'a [f32],
    pub reference: f32,
    pub reference_mean: f32,
    pub forcing: f32,
    pub table: f32,
    pub frame: &'a HarmonicFrame<H>,
    pub current_increment: u32,
}

pub struct ControlStep {
    /// Complete raw programme output, before the rig safety gate.
    pub output: f32,
    /// Increment for the next tick. `None` means the nominal `freq` increment,
    /// not "retain the last increment".
    pub next_increment: Option<u32>,
}

pub trait StandardControl<const H: usize> {
    const INPUTS_REQUIRED: usize = 0;
    const REFERENCE_UNIT: &'static str = "V";
    const TELEMETRY: &'static [(&'static str, &'static str)] = &[];

    fn step(&mut self, inputs: StandardControlInputs<'_, H>, ctx: &StepCtx<'_>)
        -> ControlStep;

    fn apply(&mut self, id: u16, payload: Payload);
    fn reset(&mut self) {}
    fn set_output_enabled(&mut self, _enabled: bool) {}
    fn telemetry(&self, _out: &mut [f32]) {}
    fn fault(&self) -> bool { false }

    // Optional core-0 metadata used only by the simple all-f32 group.
    fn scalar_param_names() -> &'static [&'static str] { &[] }
    fn scalar_param_value(&self, _id: u16) -> Option<f32> { None }
    fn normalise_scalar_param(
        _id: u16,
        _value: f32,
        _input_count: usize,
    ) -> Option<f32> { None }
}
```

`None` has deliberately positive semantics: it restores the nominal increment.
If it meant "retain", leaving PLL mode could silently leave the generator at
the last locked frequency.

`reference_mean` is retained deliberately. Unlike the rejected
`forcing_changed` callback, it is a generic, already-computed property of every
Fourier reference. It lets a control validate an absolute reference without
reaching into the generator or adding a rig-shaped event to the platform.

Delete the old `helic_core::Controller` trait rather than adapting it. Move or
rewrite `PassThrough` and `PidController` where they can implement
`StandardControl` directly; the numerical `Pid` primitive remains in
`helic-core`. `PassThrough` returns `reference + forcing + table` with no
frequency override. `PidController<const FEEDBACK: usize>` returns
`pid + forcing + table`, fixes its feedback input in its type, and drops the
writable `ctrl_feedback` index. It declares the corresponding
`INPUTS_REQUIRED`. These are intentional breaking changes: this rig is the
only controller consumer, and retaining two traits, an overlap rule, and an id
translation has no second user to justify them.

The rewritten `PidController` declares `ctrl_kp`, `ctrl_ki`, `ctrl_kd`, and
`ctrl_tau_d` at raw ids 0 to 3; `PassThrough` declares none. Both rely on the
standard output lifecycle for reset. Neither receives an implicit host reset
command.

`StandardProgram::apply` forwards every `DOMAIN_CONTROLLER` id and payload
verbatim to `StandardControl::apply`; it reserves no reset id and performs no
offset. Each control owns its complete command-id space. The rig-specific group
therefore legitimately assigns id 0 to `control_mode` and id 1 to
`ctrl_reset`. Replace the old generic group with
`ScalarControlGroup<C: StandardControl<H>, const H: usize>` for simple
all-`f32` controls. It publishes exactly the control's declared scalar
parameters, injects no reset parameter, and preserves ids. The
mixed-type, cross-validated selectable control continues to use its dedicated
group. Reset remains a lifecycle method; a host-visible reset exists only when
the owning parameter group explicitly declares and routes one.

`StandardProgram<C, H, N>` forwards `C::INPUTS_REQUIRED` as its own
`Program::INPUTS_REQUIRED`; the existing `validate_sources` check therefore
remains the binding guard. The optional scalar metadata above exists only so
`ScalarControlGroup` can shadow simple `f32` parameters. It does not
define command ids for a control which uses a dedicated typed group.

### Harmonic generation inside `StandardProgram`

Replace `StandardProgram`'s bare `PhaseAccumulator` with
`HarmonicGenerator<H>`. Per tick it should:

1. advance the generator once and borrow the resulting `HarmonicFrame<H>`;
2. project `target_coeffs` and `forcing_coeffs` through that shared frame;
3. step the table using the frame's phase and wrap flag;
4. call the selected control policy with the reference mean, forcing, table,
   and frame while the frame is borrowed;
5. record the increment which generated the current sample;
6. release the frame borrow and install either the returned next increment or
   the nominal `freq` increment; and
7. use the complete output returned by the control policy.

The returned increment takes effect on the following tick. `freq_actual`
telemetry must likewise describe the increment which generated the phase in
the current record, not the increment just installed for the next record.

This refactor should reduce duplicated sine/cosine lookups because target,
forcing, and PLL demodulation share one harmonic basis. That is an expectation,
not a timing result; the release build and hardware timing gate remain
authoritative.

### Output-enabled lifecycle

`StandardProgram` cannot presently see the safety lifecycle. Keep `StepCtx` as
the loop-invariant immutable services value it is today.
Instead, the RT loop loads one `SafetyInputs` snapshot per tick, calculates
`output_enabled` as:

```text
not safety-gated, or (armed and not tripped).
```

and passes the flag separately to `Program::step`. The same safety snapshot is
then used by the downstream gate, so a core-0 arm change cannot land between
two inconsistent reads. This keeps ungated rigs permanently enabled.
`StandardProgram` tracks the previous value and:

- does not advance PID, PLL, or other dynamic control state while output is
  disabled, although lifecycle and fault-pulse bookkeeping still runs;
- calls `reset` on a disabled-to-enabled transition;
- calls `set_output_enabled` after transition handling; and
- uses zero raw programme output and the nominal frequency while disabled.

The harmonic generator and table player still advance while output is
disabled, so `phase`, `target`, `forcing`, and the diagnostic `table` candidate
retain their ordinary time-base meaning. Only the state of the selected
control, including PID and PLL estimators, is frozen.

On the first enabled PLL tick, the phase increment may still be the nominal
`freq` value; the PLL centre increment takes effect on the following tick. The
resulting one-sample phase difference is
`(freq - pll_centre_freq) / sample_rate` turns, phase remains continuous, and
no second increment-override path is justified to remove it. Bound and test
this transient.

The forcing and table values may still be calculated for telemetry, but the
existing safety gate remains the sole authority which decides the applied
output. The programme observes the gate state and never arms or clears a trip.
There is one deliberate one-tick qualification: a rig or programme fault first
evaluated after `step` quiets that tick's output, but the control has already
advanced once. The newly latched trip makes `output_enabled` false on the next
tick. Duplicating rig fault evaluation ahead of the control is not justified;
tests must pin this one-tick state advance and immediate output quieting.

### Source units and built-in controls

`StandardProgram` currently labels `target` as volts. Its source discovery must
take the target unit from `StandardControl::REFERENCE_UNIT`; the rewritten
built-in controls retain volts, while this rig declares millimetres. `forcing`
and `table` remain volts, and `phase` remains turns. In PLL mode the published
`table` signal is the table player's generated candidate, for diagnostics only;
the control deliberately excludes it from the raw output.

Test the rewritten `PassThrough` and `PidController` directly for their output
composition, raw command ids, table behaviour, signal ordering, telemetry, and
`INPUTS_REQUIRED`. The intended lifecycle change remains that a safety-gated
control no longer integrates while its output is disabled and is reset on
re-arm.

## Changes to the existing `helic_core::Pll`

### Phase convention

Adopt the conventional Fourier phase used by the CBC host analysis:

```text
x(t) = mean + a cos(theta) + b sin(theta)
phase(x) = atan2(-b, a).
```

The measured PLL phase is then

```text
phase(response) - phase(excitation), wrapped to [-180, 180) degrees.
```

A displacement response lagging measured excitation by 90 degrees therefore
reports `-90 deg`, exactly matching the phase-resonance setpoint. The existing
demodulator uses `atan2(b, a)` and consequently reports the opposite sign; its
documentation and tests must change with the implementation.

Define the loop error as

```text
phase_error = corrected_measured_phase - target_phase.
```

The usual resonance has a negative phase-frequency slope. With this error
definition, positive `pll_kp` and `pll_ki` increase frequency when the measured
phase lies above the target, which is the expected feedback direction. Signed
gains remain valid for an experimentally established positive slope, but zero
is the unevidenced default and the ordinary case uses positive gains.

### DC rejection before synchronous demodulation

The laser reports an absolute distance with a large static component. Mixing
that value directly with sine and cosine leaves carrier-frequency leakage after
the first-order demodulation filter, potentially much larger than the dynamic
response.

Add one DC estimator to each input of the existing PLL demodulator. On the
first valid sample, seed the estimator from that sample. Thereafter use

```text
mean += beta * (sample - mean)
ac = sample - mean
beta = dt / (dc_time_constant + dt)
```

and demodulate `ac`. The same high-pass operation on excitation and response
preserves their relative steady-state phase. `dc_time_constant` must be
strictly positive and slow relative to the lowest permitted carrier. The
filtered PLL amplitude is used for qualification and telemetry, not as the
authoritative backbone amplitude; settled host-side harmonic fitting of the
captured raw response remains authoritative.

Resetting the PLL resets both mean estimators and seeds them afresh on the next
valid sample. Tests must cover a response of the form
`25 mm + small sinusoid`, non-zero excitation offset, and slow mean drift.

Seeding from one sample still leaves a mean error of order the oscillation
amplitude. After every full reset or explicit reacquisition, remain in
`Acquiring` and mark observations invalid for at least several times
`max(dc_time_constant, demod_time_constant)`. Use one named, tested warm-up
factor rather than relying on `lock_dwell` to hide filter transients. The
acquisition-timeout clock starts after warm-up, and telemetry distinguishes
warm-up from a valid observation. This conservative rule can be relaxed only
after simulation demonstrates an equivalent state-dependent criterion.

### Separate amplitude qualifications

Replace `min_amplitude` with:

- `min_excitation_amplitude`, in the excitation sensor's units; and
- `min_response_amplitude`, in the response sensor's units.

The demodulator should return an observation containing measured phase and both
fundamental amplitudes. Keep qualification in squared-amplitude form, comparing
`a^2 + b^2` with the corresponding squared threshold, as the current code does.
Take square roots only to populate amplitude telemetry and getters. An invalid
or sub-threshold observation resets the consecutive acquisition dwell. It
continues to count towards loss of an established lock, as it does now.

### PI loop filter and anti-windup

Replace the single `gain` with proportional and integral gains. Host-visible
gains use interpretable frequency units:

- `pll_kp`: Hz/degree; and
- `pll_ki`: Hz/(degree second).

The rig parameter group converts these once to phase-increment units using the
sample rate. No `f64` conversion occurs on the tick path.

The existing `Pll` retains a floating-point integral correction in
phase-increment units, relative to an exact `u32` centre. Do not represent the
absolute command as `f32`: at 8 kHz its precision becomes coarser than one
phase-increment quantum above 15.625 Hz. On each valid observation compute,
schematically:

```text
i_candidate = i + Ki * phase_error * dt
correction = Kp * phase_error + i_candidate
bounded_correction = clamp(correction, min_increment - centre_increment,
                            max_increment - centre_increment)
command = centre_increment + round(bounded_correction)
```

Use conditional-integration anti-windup: accept `i_candidate` if the command is
not saturated or if the error drives the command back into the admissible
window. Form the signed correction bounds without unsigned subtraction, and
clamp the final integer sum to the configured inclusive `u32` bounds. At the
few-hertz correction scale, `f32` retains sub-quantum information which the
integrator can accumulate. Do not retain the current correction remainder: the
relative floating-point state already carries that information, and the
absolute reconstruction rounds only once.

The gains may be signed because an unusual measured phase-frequency slope may
reverse the required direction. Validation must reject non-finite gains. The
slope and expected positive sign must be confirmed by a low-amplitude open-loop
frequency sweep before PLL operation.

### Frequency setpoint and bounds

The PLL has its own centre-frequency parameter, `pll_centre_freq`, matching the
VCO centre in the experimental method. In PLL mode it is the sole centre
frequency; the standard generator `freq` remains authoritative only in `None`
and `Pid` modes. This avoids cross-group validation between `freq` and the PLL
window.

The parameter group enforces

```text
0 <= pll_freq_min <= pll_centre_freq <= pll_freq_max < sample_rate / 2.
```

Scalar writes must preserve that invariant at every accepted intermediate
state. A host moving the complete window may widen it, move the centre, and
then narrow it. Public run-time setters in `Pll` must not panic on bad values;
use checked setters and share their validation rules with the core-0 parameter
group. A failure after a command was acknowledged indicates an internal
contract error and must produce a programme fault rather than a panic on core
1.

### Instrumentation delay and phase compensation

The excitation and response do not presently share an acquisition time base.
The excitation reference is sampled by the AD7609, including its oversampling
group delay. The laser is exposed and processed in the optoNCDT, transported by
UART on core 0, and read by core 1 through a latest-value atomic which may be up
to one sample old. These paths add a differential delay which is separate from
the question of whether `drive` represents force.

For a differential delay `tau_inst`, the raw measured phase contains a bias
`-360 f tau_inst` degrees. Add a signed `pll_delay_s` to the existing PLL and
correct its measured phase by `+360 f tau_inst` before forming the error. Reduce
the corrected phase with a full finite modulo into `[-180, 180)`, not the
existing single-add-or-subtract helper, because the delay term can exceed one
turn. This constant-delay model must be fitted over the intended frequency
window during the open-loop commissioning sweep. If the residual is not
consistent with a pure delay, replace it with an evidenced calibration model
before interpreting quadrature as phase resonance. Quote the uncertainty in
the fitted delay as the systematic phase uncertainty
`360 f uncertainty(tau_inst)` on every backbone point.

### Loop-bandwidth hierarchy

Initial PI gains come from the measured phase-frequency slope and a deliberately
quasi-static hierarchy, not from trial-and-error simulation alone. In angular
frequency terms require

```text
omega_amplitude << omega_pll
omega_pll << 1 / demod_time_constant << omega_carrier
omega_pll << zeta * omega_n.
```

The host amplitude loop is at least five to ten times slower than the PLL. The
PLL is well below both the demodulator pole and the plant's resonant settling
rate, otherwise it acts on filter or mechanical transients as though phase were
a static function of frequency and can false-lock. Establish candidate gains
from this hierarchy, then test them in simulation and at low amplitude.

### Lock qualification

`Locked` must mean more than small phase error. Acquisition requires all of the
following continuously for `pll_lock_dwell`:

- a valid observation with both amplitudes above their thresholds;
- absolute phase error below `pll_lock_phase_tol`; and
- peak-to-peak unquantised frequency command over the candidate dwell below
  `pll_lock_freq_tol`, in Hz.

Any invalid sample or failed condition resets the acquisition dwell rather than
merely pausing it. Track stationarity from the floating-point PI command before
rounding to a phase increment. If its running span exceeds the tolerance,
restart the dwell and extrema at the current command. This measures
"stationary for the dwell" without the 0.0149 Hz/s single-tick floor produced
by one increment quantum at 8 kHz.

Once locked, retain separate unlock tolerance and dwell hysteresis. Loss of a
valid observation, excessive phase error, or sustained saturation enters the
latched `LockLost` state. Add loss on sustained frequency non-stationarity only
if simulation shows that it discriminates genuine loss of lock without
reacting to ordinary PI correction; frequency stationarity is mandatory for
declaring acquisition but not yet established as a useful trip condition.

Acquisition timeout remains non-faulting and returns to fixed operation at the
PLL centre frequency. `LockLost`, and only `LockLost`, contributes a programme
fault and trips the output safety gate. It is deliberately latched: only a full
`ctrl_reset`, or an accepted disarmed mode change which resets both algorithms,
can leave `LockLost`. `pll_reacquire` has no effect in that state. Re-arming
without first resetting therefore immediately re-latches the trip.

### Reacquisition and parameter changes

Retain the replay-safe semantics of `set_enabled(true)`: replaying an unchanged
enable must not disturb an existing lock. Add an explicit `reacquire` operation
which:

- enters `Acquiring` from `Locked` or `Fixed`;
- preserves the current bounded frequency as the initial condition;
- clears acquisition, unlock, and saturation dwell timers; and
- retains useful demodulator state unless a full `reset` was requested.

Changing the target phase while PLL mode is active invokes `reacquire`
automatically. Forcing-coefficient changes do not have a platform callback:
sweep orchestration is host-side and explicitly pulses `pll_reacquire` after
the complete coefficient update. This avoids surprising lock loss after an
unrelated harmonic trim and keeps the platform control trait free of a
rig-shaped hook. `ctrl_reset` remains the full reset, returning to the centre
frequency and clearing filters and integrator state.

### Telemetry getters

The revised existing `Pll` exposes at least:

- state;
- measured excitation-to-response phase;
- phase error;
- excitation fundamental amplitude;
- response fundamental amplitude;
- commanded phase increment; and
- whether the frequency command is saturated.

Frequency conversion to hertz belongs in the programme, which knows the sample
rate.

## Rig-specific selectable control

### Structure

Add a dependency-light, host-testable rig control type, preferably in a local
library target rather than `main.rs`:

```rust
pub enum ControlMode { None = 0, Pid = 1, Pll = 2 }

pub struct MagnetoelasticControl<const H: usize> {
    mode: ControlMode,
    pid: Pid,
    pll: Pll<H>,
    output_enabled: bool,
    pid_error: f32,
    // Fixed telemetry and a bounded mode-transition fault pulse.
}
```

Its `StandardControl` implementation declares `REFERENCE_UNIT = "mm"` and
`INPUTS_REQUIRED` as one past the greatest compile-time measurement index it
reads, at least the fixed laser and excitation-reference channels. Therefore
the platform's existing source validation rejects any input-table refactor
which leaves the control's indices out of range. It:

- returns `forcing + table` and no frequency override in `None` mode;
- applies the PID to `target - laser`, returns
  `pid + forcing + table`, and returns no frequency override in `Pid` mode;
  and
- returns `forcing`, ignores the table contribution, and returns the PLL
  increment in `Pll` mode.

The control inputs include forcing and table so PID anti-windup can use the
remaining actuator headroom. Define `SAFE_OUT_MIN` and `SAFE_OUT_MAX` once in
the rig, from the fitted DAC window, and use those exact constants both here and
in `Rig::clamp_output`. On every PID tick first form
`feed_forward = forcing + table`. A non-finite result raises a programme fault
and skips `Pid::update`. Otherwise set the PID limits to

```text
pid_min = (SAFE_OUT_MIN - feed_forward).min(0)
pid_max = (SAFE_OUT_MAX - feed_forward).max(0).
```

For finite feed-forward the unexpanded residual interval is already ordered,
because the same value is subtracted from both endpoints; it cannot invert as
the review suggested. Expanding it to contain zero is nevertheless the right
policy when feed-forward alone is out of range: the PID may cancel that excess
but is never forced to do so, and its integrator cannot drive farther outwards.
The downstream safety clamp remains the final backstop. Core 0 should also use
`FourierCoeffs::amplitude_bound` to reject a forcing set which alone exceeds
the same window, as defence in depth; the core-1 finite check remains necessary
because forcing and table are composed only there.

The `Pid` anti-windup test must use the sign of the proposed integral increment,
`pid_ki * error * dt`, rather than the sign of `error` alone. The current test is
only correct for positive `pid_ki`; using the actual increment makes the
conditional-integration rule correct for either evidenced feedback sign.

The PID feedback channel is fixed to the named laser input at compile time.
Do not retain a freely writable numeric feedback index: it permits
dimensionally invalid feedback and makes saved parameter sets dependent on
source ordering.

PID entry is also firmware-checked against a short-window laser mean, not one
instantaneous sample. On the first enabled PID tick, hold the raw output at zero
for an evidence-backed `PID_ENTRY_DWELL_S`, accumulate the laser mean and
peak-to-peak range, and require both:

```text
laser_peak_to_peak <= PID_ENTRY_QUIET_MM
abs(target_coeffs.mean - laser_mean) <= PID_ENTRY_ERROR_MAX_MM.
```

Only then begin PID updates. A failed qualification raises a programme fault
before non-zero output. If `reference_mean` changes while PID is already
enabled, fault and quiet immediately; the host must disarm, let the plant
settle, update the target, and re-arm through the same qualification. Changes
to zero-mean harmonic coefficients need not trigger this particular interlock.
This turns the 25 mm absolute-laser offset trap into an enforced condition
without weakening the bound by the current response amplitude. A bumpless
initial PID contribution is desirable, but does not replace the mean check.

The PLL response channel is likewise the laser. The excitation reference is a
hardware decision which remains unresolved:

- `drive` is a command-path loopback, not measured force; and
- `coil` is a specimen-side pickup, not an established force measurement.

Using `drive` would control command-to-displacement phase. It supports a
repeatable engineering PLL only after its powered phase transfer is measured,
but a `-90 deg` lock would not yet be evidence of force appropriation. A
scientifically defensible phase-resonance result requires a measured exciter
current or force channel, or an experimentally validated correction from
`drive` phase to applied-force phase. The selected source and evidence must be
recorded in `notes.md` before enabling PLL hardware acceptance.

### Parameter group

Replace `ControllerGroup<ActiveController>` in this rig with a fixed-capacity
`MagnetoelasticControlGroup`. It retains accepted core-0 shadows, validates
before acknowledgement, converts frequency quantities to phase-increment
units, and sends commands to `DOMAIN_CONTROLLER` for sample-boundary
application.

The proposed host-visible parameters are:

| Parameter | Type | Unit or values |
|---|---|---|
| `control_mode` | `u32` | 0 none, 1 PID, 2 PLL |
| `ctrl_reset` | `u32` | non-zero pulse |
| `pll_reacquire` | `u32` | non-zero pulse |
| `pid_kp` | `f32` | V/mm |
| `pid_ki` | `f32` | V/(mm s) |
| `pid_kd` | `f32` | V s/mm |
| `pid_tau_d` | `f32` | s |
| `pll_centre_freq` | `f32` | Hz |
| `pll_freq_min` | `f32` | Hz |
| `pll_freq_max` | `f32` | Hz |
| `pll_kp` | `f32` | Hz/degree, normally positive |
| `pll_ki` | `f32` | Hz/(degree s), normally positive |
| `pll_target_phase` | `f32` | degree, conventional response minus excitation |
| `pll_delay_s` | `f32` | signed response-path minus excitation-path delay, s |
| `pll_dc_tau` | `f32` | s |
| `pll_demod_tau` | `f32` | s |
| `pll_excitation_min` | `f32` | excitation-source unit |
| `pll_response_min` | `f32` | mm |
| `pll_lock_phase_tol` | `f32` | degree |
| `pll_unlock_phase_tol` | `f32` | degree |
| `pll_lock_freq_tol` | `f32` | Hz peak-to-peak over lock dwell |
| `pll_lock_dwell` | `f32` | s |
| `pll_unlock_dwell` | `f32` | s |
| `pll_acquire_timeout` | `f32` | s |
| `pll_saturation_dwell` | `f32` | s |

All amplitudes, tolerances, dwell times, and non-delay time constants are finite
and non-negative, with `pll_dc_tau` strictly positive. `pll_delay_s` is finite
and signed. Phase lies in `[-180, 180)`. Unlock phase tolerance is not smaller
than lock phase tolerance. Defaults must be identical in the core-0 shadow and
the constructed core-1 object. PID gains default to zero. No unevidenced PLL
gain or frequency window is made a production default merely to make
acquisition appear to work.

The core-0 group rejects `control_mode` with `BadValue` while its safety
snapshot is armed. This makes the ordinary error synchronous and informative.
There remains a race in which the mode write is accepted while disarmed but the
host arms before core 1 consumes it. The RT backstop handles that case: `apply`
sets `mode_change_pending`; `step` observes that pending change together with
enabled output, moves it into `mode_change_fault`, and emits safe raw output;
`Program::fault` reads the flag later in the same tick; and only the following
`step` clears it. Test this exact apply, step, fault, gate ordering.

Every accepted mode change resets both algorithms. A raced armed change
therefore latches the safety trip and requires explicit re-arm, while an
ordinary disarmed change does not turn routine configuration into a safety
recovery. Only the host arms or clears the trip.

### Fixed telemetry and source budget

Source discovery cannot change with mode. Expose this fixed control telemetry
in every mode:

| Source | Unit | Inactive value |
|---|---|---|
| `control_mode` | enum | current mode |
| `pid_error` | mm | `NaN` outside PID mode |
| `pll_phase` | degree | `NaN` before a valid observation |
| `pll_phase_error` | degree | `NaN` before a valid observation |
| `pll_exc_amp` | V | `NaN` before valid observation |
| `pll_resp_amp` | mm | `NaN` before valid observation |
| `pll_freq_actual` | Hz | current programme frequency |
| `pll_state` | enum | current PLL state |

Do not spend six stream slots on physically unused `adc2` to `adc7`. The ADC
frame is still acquired in full, but `Rig::measure` copies only the named ADC
channels into the compact output slice, then writes laser and stator at their
new indices; `Rig::INPUTS`, the slice writes, the named index constants, and
`StandardControl::INPUTS_REQUIRED` change together. Reserve one spare physical
ADC input for a measured exciter-current or force signal and name it when
wired; this replaces a spare
rather than growing the source table. A likely commissioned set is `coil`,
`drive`, `current`, `laser`, and `stator`, giving five rig inputs, twelve
programme signals, one actuator, and `cmd_epoch`: 19 of the 24 reviewed slots.
Until `current` is physically present and calibrated, omit it rather than
publishing a plausible zero.

Keep the eight control signals coherent in the capture for initial
commissioning. If later source pressure is real, the slowly varying amplitude
and state views can move to typed `ExtraParam` atomics, but that is not needed
to create room for the already anticipated current channel.

`pll_freq_actual` describes the frequency used for the current record. The
shortened amplitude names respect the protocol's 15-character source-name
limit; parameter names have the separate 23-character limit.

The authoritative applied excitation remains `out`, after the safety gate.
Captures used as evidence include at least the excitation reference, `laser`,
`forcing`, `phase`, all PLL telemetry, `out`, and `cmd_epoch`.

## Safety and operating sequence

Boot selects `None`, with zero target, forcing, and table, and remains
disarmed. Before first entering PID mode in a session, measure the laser resting
position and set the mean of `target_coeffs` to it while PID gains remain zero.
A zero-mean displacement target against an absolute laser reading near 25 mm
would otherwise command a large, dimensionally valid error as soon as non-zero
gain is applied. Wait for the disarmed structure to become quiescent before
arming. The firmware's quiet-window and mean checks are authoritative; this
operating sequence explains how to satisfy them.

The normal PLL sequence is:

1. establish the laser resting point and validate the chosen excitation-phase
   measurement;
2. configure the PLL while disarmed, including a conservative frequency
   window, evidenced instrumentation-delay compensation, and small forcing
   amplitude;
3. select PLL mode, clear diagnostics, and arm through the persistent host;
4. observe `Acquiring`, and accept a point only after `Locked` and independent
   host settling checks;
5. step forcing amplitude for a backbone, or target phase plus the slow outer
   amplitude loop for an NLFRF;
6. pulse `pll_reacquire` after each completed forcing-coefficient update; and
7. on `LockLost`, displacement trip, current excursion, or communication loss,
   remain quiet until the cause is understood, reset, and explicitly re-arm.

The existing per-tick voltage clamp, displacement window, laser-staleness
guard, non-finite output guard, arm state, and communication-loss disarm remain
downstream of the complete programme output. PLL frequency bounds supplement
these protections but are not a substitute for output and displacement limits.

`LockLost` remains a programme fault which trips the output. Near a fold, loss
of phase lock can precede a jump to a distant, high-amplitude attractor, so
falling back to fixed excitation while hoping the displacement window catches
the jump is not the conservative choice. The operational cost is deliberate:
any established-lock loss aborts an unattended sweep, leaves the output quiet,
and requires diagnosis and explicit host re-arm. Acquisition failure before
lock remains non-faulting at the bounded centre frequency.

The firmware currently has no calibrated current safety input. Until one is
available, the host sweep must retain its guarded, short-capture current and
displacement checks. Do not represent `drive` as measured current.

## Verification

### Pure PLL tests

Extend the existing `helic-core` tests to cover:

- the conventional phase sign at `-90`, `0`, and `+90 deg`;
- wrapping immediately either side of `-180/180 deg`;
- a small sinusoid about a 25 mm response mean;
- non-zero and slowly drifting means on both inputs;
- warm-up after reset and reacquisition, including timeout starting only after
  warm-up;
- independent excitation and response squared-amplitude thresholds, with
  square roots confined to telemetry;
- constant instrumentation-delay compensation, sign, uncertainty, and
  positive and negative corrections exceeding one turn;
- PI convergence for both signs of a monotone phase-frequency slope;
- proportional response, integral convergence, conditional anti-windup, and
  recovery from both frequency limits;
- phase-increment quantisation while a centre-relative floating-point
  integrator accumulates a sub-quantum correction to a whole quantum at a
  carrier above 15.625 Hz;
- continuous valid dwell, with one invalid sample resetting acquisition dwell;
- phase and unquantised frequency-span lock qualification;
- lock/unlock hysteresis, acquisition timeout, saturation dwell, and latched
  lock loss, including `reacquire` being unable to leave `LockLost`;
- replayed enable preserving lock and explicit reacquisition preserving the
  current frequency; and
- every public run-time setter rejecting invalid values without panicking.

### Programme and registry tests

Add host tests for:

- direct output, telemetry, and raw-id tests for the rewritten `PassThrough`
  and fixed-input `PidController`;
- shared harmonic projection matching the previous target and forcing values;
- an increment returned on tick `n` first affecting tick `n + 1`;
- leaving PLL mode restoring the standard nominal frequency;
- no dynamic control-state evolution while output is disabled, continued
  generator and table advancement, reset on re-arm, and the documented one-tick
  state advance when a downstream fault first appears;
- the exact output expression in all three modes;
- table suppression in PLL mode;
- residual PID output limits after forcing and table, feed-forward beyond the
  window in both directions, non-finite feed-forward, signed-integral PID
  anti-windup, and the quiet-window target-mean interlock;
- armed mode-write rejection, the arm/write race, fault-pulse ordering, and
  explicit re-arm behaviour;
- verbatim routing of every controller id and payload, including
  `control_mode = 0` and `ctrl_reset = 1`;
- parameter type, bound, cross-parameter, shadow, and command-epoch semantics;
- fixed source names, units, ordering, the diagnostic-only meaning of `table`
  in PLL mode, compact `measure` writes, `INPUTS_REQUIRED`, omission of unused
  ADC sources, and the 24-source limit; and
- `Program::fault` being true only for the mode-transition pulse, internal
  contract failure, or `PllState::LockLost`.

### Simulation

Before hardware operation, close the revised PLL around both a linear
second-order plant and the project's Duffing virtual rig. Generate time-series
figures of displacement, excitation, measured phase, phase error, commanded
frequency, amplitudes, state, saturation, and applied output. Use the
simulations to establish conservative starting gains and confirm:

- acquisition from both sides of resonance;
- no false lock during slow frequency drift;
- stable reacquisition following an amplitude or phase step;
- the PI bandwidth remaining below both demodulator and mechanical settling
  bandwidths;
- at least a factor of five to ten between phase-loop and host amplitude-loop
  bandwidths; and
- correct behaviour near a frequency bound and on observation loss.

The simulation establishes plausibility, not hardware stability.

### Software and hardware gates

Run the complete check set in `README.md` for both supported boards. Update
`rig-profile.toml` for every new required hot symbol and for the revised capture
source set. Inspect compiler-generated calls reachable from the new control
path, then run `helic-rt-layout`.

On hardware, commission sequentially at the existing safe starting amplitude:

1. verify `None` mode, mode-change quieting, and all source metadata;
2. verify PID reset, sign, low gains, clamp, trip, and re-arm;
3. measure the powered excitation-reference transfer and the open-loop
   phase-frequency slope;
4. acquire PLL lock at one low-amplitude point from above and below resonance;
5. provoke acquisition timeout, frequency saturation, observation loss, and
   lock loss, verifying the intended safe output each time;
6. run a short stepped-amplitude backbone segment with independent host
   settling and spectrum estimation; and
7. record loop timing, overruns, drops, source captures, and electrical evidence
   in `notes.md`.

The 8 kHz worst-case PLL path, including command-application ticks, must remain
below the existing 60 microsecond hardware regression limit with zero overruns.
The former 63 to 64 microsecond command-copy cost was removed in platform
v0.2.1; this rig subsequently measured quiet and hundreds of write ticks at
about 44 to 45 microseconds, and v0.2.5 retains that compact command envelope.
This leaves about 15 microseconds, approximately 2250 cycles at 150 MHz, for
the complete added PLL path; the shared harmonic frame repays 32 LUT lookups
from the present target and forcing projections. The 60 microsecond acceptance
is current and testable, not merely a quiet-tick limit. Software checks alone
do not establish that the revised PLL fits the stated headroom.

## Delivery sequence and repository ownership

This document is the cross-repository design record requested before work
begins. Before implementation, split the normative platform half into
HELIC-DAQ beside `docs/proposals/rt_program_proposal.md` and
`docs/proposals/rig_decoupling_proposal.md`; retain here only the rig composition,
parameters, source choices, safety decisions, and commissioning contract. The
platform pull request is reviewed and merged before the rig is repinned.

`Pll` does not yet have two firmware consumers. The developer guide permits a
second route into `helic-core`, deliberate acceptance as a platform primitive,
which already applies: `Pll` is exported, tested, and documented there. Its
single immediate consumer is stated rather than hidden, and avoiding relocation
churn is supporting rather than primary justification. No second PLL
implementation is created.

1. In the HELIC-DAQ platform proposal and implementation, replace `Controller`
   with `StandardControl`, rewrite `PassThrough`, `PidController`, and their
   scalar parameter group, add shared-frame `StandardProgram`, a coherent
   per-tick output-enabled flag, and direct tests. No compatibility adapter or
   reserved controller id remains.
2. In the same platform, revise the existing `helic_core::Pll` and its tests.
   Do not add a second PLL type.
3. Release a platform tag. Update this repository's `Cargo.toml`, CI workflow,
   and README installation command together, after grepping for the old tag.
4. In this repository, add the selectable control and parameter group, compose
   it in `main.rs`, update source metadata and `rig-profile.toml`, and add host
   tests in the local library target.
5. Add the slow outer amplitude controller and PLL orchestration to the CBC
   Duffing host library, then update its method note to say the phase detector
   and VCO now run in firmware while sweep orchestration remains host-side.
6. Simulate, plot, commission on hardware, and record evidence before declaring
   PID or PLL mode verified.

The platform tag removes the obsolete `Controller` documentation and updates
the developer guide's control lifecycle, standard-signal ordering, target-unit
description, raw command-id ownership, and deliberate `Pll` placement. These
changes are called out in the tag message.

If the rig work must begin before the platform release, use a temporary
`[patch.crates-io]` against the platform branch, record why it exists, and
remove it immediately after repinning to the released tag.

## Deferred extensions

The first implementation is a fundamental, linear-PI PLL. It deliberately
defers:

- nonlinear-controller PLL laws;
- multi-harmonic phase criteria or force appropriation;
- automatic continuation and sweep scheduling in firmware;
- a firmware response-amplitude loop; and
- dynamic selection of measurement channels.

Strong odd harmonics are expected from the magnetic nonlinearity. Every locked
point therefore retains full-spectrum host analysis. If PLL and CBC backbone
results disagree beyond experimental uncertainty, the fundamental phase
criterion and excitation-force measurement are investigated before adding an
NCPLL or multi-harmonic controller.

## Review, 2026-08-18

Read against the platform at the pinned tag: `helic-core`'s `pll.rs`,
`harmonics.rs`, `pid.rs`, and `controller.rs`; `helic-rt`'s `program.rs`,
`params/groups.rs`, `rig.rs`, and `safety.rs`; `firmware/rt/src/rt_loop.rs`;
this rig's `config.rs`, `rig.rs`, and `main.rs`; and both `AGENTS.md` files.

The architecture holds and the repository split follows the platform's own
rules. The loop design has correctness gaps that are cheap to close now and
expensive later, and the safety story leans on operator prose in the two places
where this rig has most recently decided the opposite.

### Verified, so it need not be rechecked

- **The blanket adapter compiles.** A two-crate fixture with an upstream
  blanket `impl<C: Controller, const H: usize> StandardControl<H> for C`, a
  downstream local implementation, and both `StandardProgram<PassThrough, 16>`
  and `StandardProgram<RigControl<16>, 16>` builds without a coherence error.
  A downstream local type cannot acquire a `Controller` implementation from
  elsewhere, so the overlap check is satisfied. Existing rig aliases survive.
  The one lasting constraint is that no type may implement both traits.
- **The shared-frame refactor completes an existing intent rather than adding
  machinery.** `HarmonicFrame`, `HarmonicGenerator`, and `Pll` are all
  currently dead code in `helic-core`: exported from `lib.rs` and used by
  nothing. This proposal is their first consumer.
- **The sine and cosine saving is real.** `FourierCoeffs::evaluate` walks all
  `H` harmonics per call, so target and forcing cost 64 lookups per tick at
  `HARMONICS = 16`. One shared frame costs 32.
- **The budget arithmetic is right.** Ten inputs, twelve programme signals, one
  actuator, and `cmd_epoch` is exactly `MAX_SOURCES = 24`. `pll_freq_actual`
  and `pll_phase_error` are exactly 15 characters; `pll_lock_freq_rate_tol` is
  22 against the 23-character parameter limit. `MAX_CTRL_PARAMS = 17` does not
  bind a rig-owned group, and `MAX_GROUPS = 8` is not reached at six groups.
- **The DC-rejection argument is quantitatively necessary, not defensive.** A
  mean `D` leaks through synchronous demodulation as roughly
  `2 D / (omega tau_demod)`, so 25 mm at 30 Hz with `tau_demod = 0.1 s` gives
  about 2.7 mm of carrier-rate ripple against a sub-millimetre response.

### Correctness gaps

**The mode-change fault pulse can be cleared before it is ever read.** One tick
runs as apply commands, `step_program`, `program_fault`, then the gate. If
`apply` sets the pulse and `step` clears it, `Program::fault` never observes it
and the interlock is decorative. The rule must be stated and tested: `apply`
sets pending, `step` moves pending into the flag `fault` reads, and the
following `step` clears that flag.

**The PID's own anti-windup is left disarmed.** `Pid` implements conditional
integration against `config.out_min` and `out_max`, which default to plus and
minus `f32::MAX`. The parameter table exposes the four gains and no limits, so
whenever the safety gate is clamping but not tripped the integrator winds up
without bound. Reset on re-arm does not cover that case, because no transition
occurs. Bind the limits to `DAC_OUT_FLOOR_V` and `DAC_OUT_CEILING_V` at
construction, or expose them as parameters.

**Instrumentation phase bias is unaccounted for and lands on the phase
criterion.** The excitation reference reaches the loop through the AD7609 at
OS8, a group delay of tens of microseconds, under a degree at tens of hertz.
The response reaches it from the optoNCDT over UART, published by core 0 as a
latest-value atomic, at most one tick stale, plus the sensor's own exposure and
processing latency, which is nowhere measured in this repository. The two
channels therefore do not share a time base, and the measured
excitation-to-response phase carries an unknown fixed delay. A pure delay
`tau` biases the phase by `-360 f tau` degrees, growing linearly along the
backbone. This is separate from, and additional to, the `drive`-is-not-force
problem already recorded above. The open-loop sweep of commissioning step 3
already produces the necessary data; what is missing is the requirement that
the fitted phase be decomposed into plant phase plus instrumentation delay
before `-90 deg` means anything, and that the delay be either compensated in
`pll_target_phase` or bounded and quoted as a systematic uncertainty on every
backbone point.

**The loop-bandwidth hierarchy is under-specified.** Only the factor of five to
ten between phase and amplitude loops is given. Two further constraints matter
more, and both belong in the tuning section:

- `omega_loop` well below `1 / tau_demod`, itself well below the carrier, or
  the demodulator pole sits inside the loop; and
- `omega_loop` well below `zeta omega_n`, the plant's own settling rate.

The PI acts on the assumption that response phase is a static function of
excitation frequency. That holds only quasi-statically. A loop faster than the
resonance settles chases its own transient, which is the classic false-lock
mechanism the simulation section is asked to rule out but is given no criterion
for. Starting gains should be derived from this hierarchy and then confirmed by
simulation, not chosen by simulation alone.

**The frequency-slew lock criterion has a quantisation floor.** One
phase-increment quantum at 8 kHz is `8000 / 2^32`, that is 1.86 microhertz, so
a single-tick slew estimated from the quantised increment reads 0.0149 Hz/s
whenever the command moves at all. Computing the slew from the unquantised PI
output in hertz, and across the lock dwell rather than one tick, removes the
floor entirely and measures what "stationary for the dwell" actually means.

**The DC estimator's warm-up is a real transient.** Seeding from the first
sample sets the mean to a point on the waveform, leaving an error of order the
signal amplitude which decays over `dc_time_constant`. With that constant slow
relative to the carrier, as required, the transient outlives a 0.1 s lock
dwell. Require an explicit warm-up of several times
`max(dc_time_constant, demod_time_constant)` during which no observation is
valid, and test it. Otherwise the first `Locked` after every arm or reacquire
rests on corrupted phase.

### Simplifications

- **Drop the correction remainder.** With `f = f_centre + Kp e + I`
  reconstructed absolutely each tick, rounding once loses nothing: the
  integrator is floating point and carries the fractional information, and the
  quantum is 1.86 microhertz. The remainder is a leftover of the current
  incremental design, `commanded_increment += gain * error * dt`, and keeping
  it on top of an absolute computation double-counts. The incremental
  formulation is discarded, not adapted.
- **Drop `forcing_changed`.** It is a rig-shaped hook in a platform trait,
  redundant with the `pll_reacquire` pulse specified in the same document, and
  surprising in use, since any coefficient write including a small harmonic
  trim would drop the lock. Sweep orchestration is already host-side; let the
  host pulse `pll_reacquire`.
- **Drop `disabled_increment`.** It buys one sample of frequency accuracy at
  arm. Phase is continuous across an increment change, so one sample at the
  nominal increment is a phase error of `(f_nom - f_centre) / f_s` turns, which
  is negligible. It costs a second increment-override path whose semantics
  differ from `next_increment`.
- **Shrink the `step` signature.** Seven positional arguments, two of them
  redundant, since `dt` is `sample_rate.dt()`. Pass `&StepCtx` plus one small
  inputs struct. Redundant parameters that must agree are how they eventually
  disagree.
- **Say what happens to `StepCtx`.** It is currently loop-invariant, built once
  in `run_rt_loop` and stored in `RtLoopState`, and is documented as immutable
  services. Adding `output_enabled` makes it per-tick state. Either construct
  it per tick, which costs two words, or pass the flag separately, but choose
  deliberately rather than contradicting the type's stated character.
- **Specify who handles `command_id::controller::RESET`.** Today
  `StandardProgram::apply` special-cases it and offsets every other id by one.
  If `StandardControl::apply` sees raw ids, a rig control can silently fail to
  honour `ctrl_reset`. State that the programme keeps routing `RESET` to
  `reset`, forwards ids of one and above unchanged, and test that mapping
  specifically rather than under the general heading of commands.
- **Fix the sign convention so that positive gains are the expected case.**
  Signed gains are correct, but an operator entering the wrong sign gets a
  runaway to a frequency bound. Define the error so that a system with the
  usual negative phase-frequency slope needs positive `pll_kp` and `pll_ki`,
  and say so beside the parameters.

### Two decisions worth arguing

**Using the safety latch as a state-machine device.** A mode change while armed
raising a fault works, but it overloads a mechanism whose documented meaning is
that something unsafe happened, and it turns a routine host action into an
operator recovery. Core 0 can read `shared.safety.load_inputs().armed`, so the
natural design is to reject the write with `BadValue` while armed, which is
synchronous and informative, and keep the real-time fault only as the backstop
for the case where arming races the write. Same guarantee, better ergonomics.

**`LockLost` tripping the output.** Probably right for a hardening system,
where losing lock near a fold can precipitate a jump to a large-amplitude
branch, but the document asserts it rather than arguing it, and does not state
the operational consequence: every lost lock during an amplitude step ends an
unattended sweep until a human re-arms. Either justify it on the fold hazard
explicitly, or make lock loss a latched status with a fallback to fixed
operation at the last bounded frequency, leaving the displacement window to do
the safety work it already does.

### Rules that should be firmware, not prose

This rig's most recent precedent is `MAX_STATOR_RATE_MM_S`, where measured
evidence moved out of `notes.md` into an enforced bound with the reasoning
recorded beside the constant. Two rules here are left as operator instructions,
and both bite hard:

- **A free-running table must be off in PLL mode.** Nothing enforces it. Either
  ignore the table contribution in PLL mode, or refuse entry to PLL mode while
  `table_mode` is non-zero.
- **The target mean must be set to the laser resting position before PID
  mode.** This is the largest single hazard in the document. A zero-mean target
  against a 25 mm absolute reading is a dimensionally valid 25 mm error that
  commands full output the instant a gain becomes non-zero. Enforce it with an
  entry check that `|target_mean - laser|` lies within a bound, or with a
  bumpless transfer that initialises the integrator so the output is continuous
  at mode entry.

### Budget and gates

- **The source budget is exactly full, and this document says another channel
  is needed.** A calibrated excitation-current measurement is required before
  the science is defensible, and there is no slot for it. Six of the ten inputs
  are unused spare ADC channels, `adc2` to `adc7`, streamed at 8 kHz. Free
  them, or move the slow PLL telemetry, `pll_state`, `pll_exc_amp`, and
  `pll_resp_amp`, which evolve on `demod_time_constant`, to `ExtraParam`
  atomics, which is the platform's idiom for slow read-only values. Arriving at
  exactly the cap while knowing another channel is coming is a decision to make
  now rather than during commissioning.
- **The 60 microsecond acceptance is stated against a limit already
  exceeded.** `notes.md` records that `max_loop_us = 60` is exceeded by any
  parameter write, at 63 to 64 microseconds, from the fixed 140-byte
  `RtCommand` copy, against 44 to 45 quiet. As written the acceptance is
  untestable. Phrase it as quiet-tick `loop_time_max` with the known write-tick
  exception, and quote both.

### Repository ownership

- **The platform half of this document belongs in `helic-daq`**, beside
  `docs/proposals/rt_program_proposal.md` and
  `docs/proposals/rig_decoupling_proposal.md`. This
  repository's `AGENTS.md` is explicit that platform mechanisms go upstream as
  a pull request rather than being designed here. Split it: `StandardControl`,
  the `StandardProgram` refactor, `StepCtx`, and the `Pll` revision upstream;
  the selectable mode, parameter group, telemetry, and commissioning sequence
  stay.
- **The `helic-core` placement rule is two actual consumers.** `Pll` has none
  today and one after this work, and the CBC Duffing contribution in the
  delivery sequence is host-side, so it does not become the second firmware
  consumer. Revising in place remains the right call, because the code is
  already there and moving it is churn, but say so rather than leaving the rule
  apparently satisfied.
- **Two documentation debts travel with the platform tag.**
  `Controller::reset` claims a lifecycle the platform does not currently
  implement, and the developer guide's description of the standard signal
  ordering changes with the reference unit. Both belong in the tag message.

### Suggested order of work

1. The three that change behaviour rather than shape: fault-pulse ordering, PID
   output limits, and the slew criterion computed from the unquantised command.
2. The bandwidth hierarchy and the instrumentation-delay decomposition, making
   commissioning step 3 explicitly responsible for measuring the delay.
3. Prune `forcing_changed`, `disabled_increment`, and the correction remainder,
   after which the trait has one increment path and no rig-shaped hooks.
4. Settle the source budget before implementation rather than after.
5. Split the document and open the platform half upstream.

## Response to review, 2026-08-18

The review materially improves the design. The proposal above has been revised
rather than leaving accepted comments as implementation folklore. Dispositions
follow in the review's order.

### Correctness gaps

- **Fault-pulse ordering: accepted.** The normative sequence is now
  `apply: pending`, `step: pending -> visible fault`, `fault: read`, and only
  the following `step: clear`. It is the race backstop rather than the ordinary
  mode-change path.
- **PID anti-windup: accepted and strengthened.** The PID limit is the residual
  actuator headroom after forcing and table, derived every tick from the
  compile-time safe output window. This covers additive feed-forward saturation
  which binding the PID to the full window alone would miss.
- **Instrumentation delay: accepted.** The revised existing PLL has a signed
  delay compensation, commissioning fits the delay over frequency, and every
  backbone result carries its systematic phase uncertainty. This remains
  distinct from whether the excitation channel measures force.
- **Bandwidth hierarchy: accepted.** Gain selection now begins from the
  demodulator, mechanical-settling, carrier, and outer-amplitude bandwidth
  ordering and is subsequently tested by simulation.
- **Quantised slew: accepted.** Lock qualification now uses the span of the
  unquantised floating-point frequency command across a continuous dwell. The
  `pll_lock_freq_rate_tol` parameter is replaced by `pll_lock_freq_tol` in Hz.
- **DC warm-up: accepted.** Reset and reacquisition have an explicit filter
  warm-up, observations are invalid during it, and acquisition timeout begins
  afterwards.

### Simplifications

All six are accepted. The correction remainder, `forcing_changed`, and
`disabled_increment` are removed. The control call takes one inputs struct and
the immutable `StepCtx`; output-enabled state is a separate per-tick flag from
one safety snapshot. `StandardProgram` retains and tests reset/id routing. The
phase error is measured minus target, making positive gains the expected case
for the usual negative phase-frequency slope.

The simplifications reveal a cleaner output contract: the control policy
returns the complete raw programme output rather than only a contribution.
That lets PLL mode enforce `output = forcing` and ignore table playback without
a rig-shaped table hook in the platform.

### Safety decisions

- **Mode change: revised as suggested.** Core 0 rejects an armed write
  synchronously. The RT fault pulse covers only the arm/write race, where the
  write was valid when accepted but output is enabled before core 1 applies it.
- **Lock loss: trip retained and justified.** Near a fold, fixed-frequency
  fallback can drive a jump to a remote high-amplitude attractor before the
  displacement guard reacts. An established-lock loss therefore aborts an
  unattended sweep and requires explicit diagnosis and re-arm. Acquisition
  failure before lock remains non-faulting.

### Firmware-enforced rules

Both comments are accepted. PLL mode ignores table output in firmware. PID
entry checks the active target mean against the live laser with an
evidence-backed bound and faults before output if it fails. Operator prose now
explains how to satisfy these interlocks rather than carrying the safety case.

### Source budget and timing gate

- **Source budget: accepted.** Unconnected ADC channels are omitted from
  `Rig::INPUTS`, and a physical spare is reserved for a named measured-current
  channel. The likely commissioned source set uses 19 of 24 slots, so coherent
  PLL telemetry need not be demoted prematurely to atomics.
- **Timing claim: corrected, not accepted.** The review cites valid measurements
  from platform v0.1.3, but the later entry `Platform upgraded to v0.2.1` records
  the command-envelope fix and 366 write ticks at 45 microseconds. The v0.2.3
  regression repeated 606 writes at 45 microseconds, and this repository is now
  pinned to v0.2.5. The 60 microsecond gate is current, includes a write phase,
  and remains the acceptance threshold for both quiet and command ticks. The
  proposal now states that provenance explicitly.

### Repository ownership

Accepted. This file remains the requested cross-repository decision record
while the design is being settled, but its normative platform half is split
into HELIC-DAQ before implementation and reviewed there. The rig half remains
here. The existing PLL is revised in place despite having only one immediate
firmware consumer because it is already an exported, tested `helic-core`
primitive; this is avoiding relocation churn, not pretending a second consumer
exists. The tag also fixes the `Controller::reset` lifecycle and developer-guide
signal-unit documentation debts identified by the review.

## Second review, 2026-08-19

Read against the same pinned tag, with the platform working tree confirmed
identical to `v0.2.5`. This pass checks the revised proposal, so it does not
repeat the first review's accepted points. It assumes what the first review
did not: that this is the only rig with a controller, that breaking platform
changes are therefore free, and that a compatibility layer has to earn its
place like any other code.

The revision holds up. The composition contract is better than the original,
the fault-pulse ordering matches what `run_rt_tick` actually does, and the
timing correction is right where the first review was wrong. Five things are
wrong or dangerous rather than merely improvable, and one large simplification
is still on the table.

### Verified against the code, so it need not be rechecked

- **The fault-pulse ordering is implementable as written.** `run_rt_tick`
  applies commands, calls `step_program`, then evaluates `program_fault` as an
  argument to `safety_gate`, then actuates. `apply` sets pending, `step`
  publishes, `fault` reads, and the following `step` clears is exactly right
  for that sequence.
- **The timing correction is sound and the first review's figure was stale.**
  `notes.md` records the 63 to 64 microsecond command-copy cost at v0.1.3 and
  44 to 45 microseconds since v0.2.1, including write phases of 366, 606, and
  630 writes, against the profile's 60 microsecond limit.
- **`Pll`, `HarmonicFrame`, and `HarmonicGenerator` are exported from
  `helic-core/src/lib.rs` and used by nothing else in the workspace.** This
  work is their first consumer.
- **The sine and cosine saving is real.** `FourierCoeffs::evaluate` walks all
  `K` harmonics unconditionally, so target and forcing cost 64 lookups per tick
  at `HARMONICS = 16` against 32 for one shared frame.
- **The source and name budgets are right.** `MAX_SOURCES = 24`,
  `MAX_NAME_LEN = 15`, `MAX_PARAM_NAME_LEN = 23`. The commissioned set of five
  inputs, twelve programme signals, one actuator, and `cmd_epoch` is 19 of 24.
  `pll_freq_actual` and `pll_phase_error` are exactly 15; the longest parameter
  name, `pll_saturation_dwell`, is 20.
- **The phase convention is self-consistent.** With the demodulator's
  `2 x cos` and `2 x sin` estimates standing for `a` and `b`, an excitation of
  `cos(theta)` and a response lagging it by 90 degrees give
  `atan2(-b, a)` of 0 and -90 degrees respectively, so the measured phase is
  -90 degrees as claimed, and `measured - target` with a negative
  phase-frequency slope does need positive gains.

### Correctness

**`control_mode` is placed on a reserved command id.** `ParamStore::finish_staged`
uses the group-local parameter id verbatim as the `RtCommand` id, and
`command_id::controller::RESET` is 0. The proposal's own routing rule keeps
`StandardProgram::apply` intercepting id 0 and forwarding ids of one and above,
yet its parameter table lists `control_mode` first and `ctrl_reset` second.
Mode writes would therefore be acknowledged by core 0 and either swallowed as a
reset or, because the payload is `U32` rather than `Unit`, dropped by the
catch-all arm. The mode would never change and nothing would say so. Reorder so
`ctrl_reset` is id 0, or remove the reserved id entirely by taking the
simplification below.

**The residual PID limits can panic core 1.** The proposal sets
`pid_max = SAFE_OUT_MAX - forcing - table` and `pid_min = SAFE_OUT_MIN - forcing - table`
every tick, and `Pid::update` calls `unclamped.clamp(c.out_min, c.out_max)`.
`f32::clamp` panics when `min > max`. `GeneratorGroup::stage` validates
`forcing_coeffs` for finiteness alone, and `TableGroup` validates `table_gain`
the same way, so a host write of a large forcing mean inverts the window on the
next tick and kills the firmware. Clamp the residual window so it always
contains zero:

```text
pid_max = (SAFE_OUT_MAX - forcing - table).max(0.0)
pid_min = (SAFE_OUT_MIN - forcing - table).min(0.0)
```

Test feed-forward beyond the window in both directions. `SAFE_OUT_MIN` and
`SAFE_OUT_MAX` must be one definition shared with `Rig::clamp_output`, not a
second derivation from `DAC_OUT_FLOOR_V` and `DAC_OUT_CEILING_V` that can drift
away from it. `FourierCoeffs::amplitude_bound` already exists and is unused; a
core-0 rejection of coefficient sets whose bound exceeds the window is worth
considering as defence in depth, but it does not replace the core-1 fix,
because the table contributes to the same sum.

**An absolute `f32` frequency carries no sub-quantum information in this rig's
operating band.** The proposal drops the correction remainder on the grounds
that "the floating-point integrator already carries fractional information".
That is false as stated. At 8 kHz, `increment = f * 2^32 / 8000`, so one f32
ulp equals one increment quantum at `2^23` increments, which is `f_s / 512`,
that is **15.625 Hz**, and two quanta from 31.25 to 62.5 Hz. Holding the
command in hertz instead is identical: the f32 ulp at 30 Hz is 1.91 microhertz
against a 1.86 microhertz quantum. An absolute f32 PI command is therefore
coarser than the `u32` it rounds to, throughout the intended band.

The conclusion survives, the representation does not. Hold the PI state as a
correction relative to the `u32` centre rather than as an absolute frequency:

```text
i += Ki * phase_error * dt
correction = Kp * phase_error + i
command = clamp(centre + round(correction), min, max)
```

At a correction of a few hertz the f32 ulp is about an eighth of a quantum, so
the integrator does then carry what the argument claims. The reconstruction is
still absolute each tick, so the remainder still goes, and the lock-stationarity
span is still read from the unquantised correction. Add a test that a
sub-quantum integral accumulates to a whole quantum, at a carrier well above
15.625 Hz, so the property is pinned rather than assumed.

**Instrumentation-delay compensation can exceed one wrap.** `wrap_degrees` is a
two-branch plus or minus 360 adjustment. That is sound for `measured - target`,
where both operands lie in `[-180, 180)` and the raw difference cannot leave
`(-360, 360)`. It is not sound for `+360 f tau_inst`, which at 30 Hz and 40
milliseconds is 432 degrees, leaving the corrected phase outside the range and
the error wrong by a whole turn. Either reduce the corrected phase with a full
modulo, which is two lines, or bound `pll_delay` at the parameter group so that
`360 f_max |tau| < 180` and say so beside the parameter. The first costs
nothing in operating window and should be preferred.

**The output-enabled flag lags a rig fault by one tick.** `output_enabled` is
derived from the safety snapshot, but the gate also quiets on
`Rig::output_fault` and `Program::fault`, both evaluated after `step`. On the
tick a laser-staleness or displacement excursion first appears, the control
still advances against an output that is in fact zero; the trip latches and the
flag is correct from the next tick onwards. One tick is acceptable and no second
mechanism is justified, but say it, because "does not advance controller or PLL
state while output is disabled" currently reads as an invariant and is not one.

### The simplification still on the table

**Delete `Controller` rather than adapting it.** The proposal treats "adapt
every existing `Controller` without changing its enabled-path behaviour" as a
requirement, and the first review verified that the blanket adapter compiles.
Both are true; neither establishes that the adapter should exist. Its standing
cost is two traits with the same job, a coherence rule that no type may
implement both and which is invisible until the day it bites, an id offset that
exists in one path and not the other, and a reserved id 0 whose only remaining
purpose is to serve the adapter. It buys compatibility for `PassThrough` and
`PidController`, which are about thirty and seventy lines, in a workspace with
one rig that uses a controller.

Port both to `StandardControl`, make `ControllerGroup<C: StandardControl>`, and
let `StandardProgram::apply` forward every `DOMAIN_CONTROLLER` command verbatim
with no reserved id and no offset. The control then owns its whole id space,
the `control_mode` collision above cannot exist, the "first and last parameter
id" routing test becomes unnecessary, and the sample-for-sample compatibility
tests reduce to ordinary tests of two small controllers. `PidController`'s
freely writable `ctrl_feedback`, which this proposal already rejects for the
rig, goes at the same time.

If the adapter is kept anyway, the reason should be recorded, because the
document's own framing does not supply one.

### Specification gaps

- **`LockLost` has no stated exit.** `reacquire` is specified to enter
  `Acquiring` from `Locked` or `Fixed`, so the only ways out are `ctrl_reset`
  and a mode change. That is the right choice and should be written down: the
  operating sequence says "reset, and explicitly re-arm", and an operator who
  re-arms without resetting first will find `Program::fault` still true and the
  trip re-latched on the next tick.
- **`Program::INPUTS_REQUIRED` is the platform's mechanism for exactly this
  binding and the proposal does not use it.** The control indexes a fixed laser
  slot, and this proposal renumbers the inputs by dropping `adc2` to `adc7`,
  moving `LASER_INPUT` from 8 to 2. Forward `C::INPUTS_REQUIRED` through
  `StandardProgram` so `validate_sources` refuses a rig whose input table no
  longer reaches the channel the control reads.
- **Dropping the spare ADC sources is a change to `measure`, not only to
  `INPUTS`.** `measure` currently writes `values[..8]` from the decoded frame
  and reaches the laser and stator at fixed indices 8 and 9. Both the slice and
  the two index constants move with the source table.
- **The PID entry check compares a mean against an instantaneous sample.**
  `|target_coeffs.mean - laser|` uses the live reading, which under oscillation
  departs from its own mean by the response amplitude, so
  `PID_ENTRY_ERROR_MAX_MM` has to exceed that amplitude and the check is
  weakened exactly where it is doing the safety work. Either require entry from
  quiescence and say so, or compare against a short-window laser mean; the DC
  estimator specified for the PLL is the same filter.
- **Say what the `table` signal means in `Pll` mode.** The control policy now
  owns composition, so a PLL capture publishes a `table` value that was never
  added to `out`. State it in the source table, or the first person to read such
  a capture will believe the table contributed.
- **Say that the generator and table player still advance while output is
  disabled**, since the control does not. Otherwise the `phase` signal's
  meaning while disarmed is undefined.

### Smaller points

- **Keep the amplitude qualification squared.** The demodulator currently
  compares `a^2 + b^2` against `min^2` and never takes a root. Returning both
  amplitudes costs two `sqrtf` per tick, which the M33's FPU affords, but
  compute them for telemetry only and leave the threshold test as it is.
- **Justify `reference_mean` where `forcing_changed` was rejected.** It is a
  field in a platform trait that exists for one rig's interlock, which is the
  category the first review pruned. It is more defensible, because the DC
  component of the reference is a generic property of the generator rather than
  a rig-shaped callback, and it is free. Keep it, and say that, or the exclusion
  of the other reads as inconsistent.
- **The `helic-core` placement argument is stronger than the document makes
  it.** The developer guide's rule is two actual consumers **or** deliberate
  acceptance as a platform primitive. `Pll` is already exported, tested, and
  documented as one, so the second clause applies on its own; the churn argument
  supports it rather than carrying it.
- **Quote the timing headroom, not only the limit.** 44 to 45 microseconds
  measured against 60 leaves about 15 microseconds, roughly 2250 cycles at
  150 MHz, for the whole PLL path, against which the shared frame returns 32 LUT
  lookups. A stated budget fails informatively; a pass/fail assertion does not.
- **`pll_delay` and `instrument_delay_s` are the same quantity under two
  names.** Pick one and use it in both halves of the document.

### Suggested order of work

1. Decide the `Controller` question first, because the id-space and test
   consequences of the collision, the routing rule, and the compatibility tests
   all follow from it.
2. The three that are defects rather than shape: the residual-limit panic, the
   correction-relative PI representation, and the delay-compensation wrap.
3. The specification gaps, all of which are sentences in this document rather
   than code.
4. Split the platform half upstream, as already agreed, with the revised
   routing rule in it.

## Response to second review, 2026-08-19

The proposal above has been revised again. The scope clarification removes the
only plausible reason to preserve `Controller`, so the first response's adapter
decision is superseded. The second review is otherwise accepted with one
correction to its PID-panic argument.

### Control API and command ids

- **Delete `Controller`: accepted.** `PassThrough` and
  `PidController<const FEEDBACK: usize>` are rewritten as direct
  `StandardControl` implementations, `ctrl_feedback` disappears, and the
  numerical `Pid` remains in `helic-core`.
- **Remove reserved routing: accepted.** `StandardProgram` forwards every raw
  controller id and payload. The scalar group injects no reset, while the
  selectable rig group owns id 0 as `control_mode`, id 1 as `ctrl_reset`, and
  its remaining mixed-type space. This resolves the silent id-0 collision
  rather than merely reordering around it.
- **Input binding: accepted.** `StandardControl::INPUTS_REQUIRED` is forwarded
  by `StandardProgram`, and the fixed-input PID and rig control both declare
  their actual minimum.

### Correctness

- **PID residual limits: conclusion accepted, mechanism corrected.** For finite
  `feed`, subtracting it from both ordered endpoints cannot make
  `pid_min > pid_max`; the review's stated panic mechanism is algebraically
  impossible. Non-finite composed feed-forward can, however, give `clamp` a
  `NaN` bound. The revised design checks finiteness before `Pid::update`, uses
  one shared definition of the rig's safe output bounds, and expands the finite
  residual interval to contain zero. It also rejects an individually excessive
  forcing bound on core 0 and tests both directions plus non-finite composition.
  The pass also fixes `Pid` anti-windup to test the proposed integral increment,
  rather than assuming `pid_ki` is positive from the sign of the error.
- **Centre-relative PI representation: accepted.** Centre and bounds remain
  exact integer increments, while proportional and integral corrections are
  floating point relative to the centre. The correction remainder still goes,
  and a test above 15.625 Hz proves accumulation from below one quantum.
- **Delay wrapping: accepted.** `pll_delay_s` is the one name in both halves,
  and corrected phase uses a full finite modulo, including multi-turn tests in
  both directions.
- **One-tick lifecycle lag: accepted.** A newly observed downstream fault
  quiets output immediately but freezes control state only from the following
  tick. That exception is now normative and tested; no duplicate pre-step rig
  fault path is added.

### State, safety, and source semantics

- **`LockLost` exit: specified.** Only full reset or an accepted disarmed mode
  change can leave it. `pll_reacquire` cannot, and re-arming without reset
  re-latches the trip.
- **PID entry: strengthened.** Entry holds output at zero while measuring a
  short-window laser mean and peak-to-peak quietness. An enabled change of the
  reference mean faults immediately and requires disarm, settling, and the same
  requalification. This avoids comparing a DC target with an oscillating
  instantaneous sample.
- **Compact inputs: accepted.** `measure`, `Rig::INPUTS`, laser and stator
  indices, and `INPUTS_REQUIRED` now form one required edit. Decoding all eight
  ADC values no longer implies publishing all eight.
- **Signal evolution: specified.** The generator and table player continue
  advancing while disarmed. In PLL mode, `table` is explicitly a generated
  diagnostic candidate which was not included in `out`.

### Smaller points

All five are accepted. Threshold qualification stays squared, with square
roots only for telemetry. `reference_mean` is justified as a free, generic
Fourier-reference property rather than a rig event. `Pll` placement rests on
its deliberate acceptance as a platform primitive. The timing section now
states approximately 15 microseconds, or 2250 cycles at 150 MHz, of measured
headroom and the 32-LUT saving from the shared frame. `pll_delay_s` is used
consistently.
