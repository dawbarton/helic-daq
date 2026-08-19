# Standard control and bounded phase locking

## Status

This is the normative platform contract introduced in 0.3.0. It replaces the
former scalar `helic_core::Controller` abstraction. Rig-specific mode policies,
measurement indices, safety bounds, and commissioning evidence remain in the
owning rig repository.

## `StandardControl`

`helic_rt::StandardControl<H>` is the statically dispatched control seam used
by `StandardProgram`. One step receives the measured inputs, generated
reference and its Fourier mean, forcing and table candidate, shared
`HarmonicFrame<H>`, and the increment which generated that frame.

It returns the complete raw programme output and an optional increment for the
next tick. `None` restores the nominal `freq` increment. A control also owns its
raw command-id space, fixed telemetry, input requirement, reference unit,
lifecycle hooks, and programme fault. There is no reserved reset id and no id
offset. A host-visible reset exists only when its parameter group declares one.

`PassThrough` and `PidController<const FEEDBACK: usize>` implement this trait
in `helic-rt`. The latter fixes its dimensioned feedback input in its type and
exposes only gain and derivative-filter parameters. `ScalarControlGroup`
publishes exactly a control's scalar `f32` metadata; mixed or cross-validated
controls use their own `ParamGroup`.

## `StandardProgram`

The programme advances one `HarmonicGenerator`, projects target and forcing
through its borrowed frame, advances the table player, and calls the control.
A returned increment is installed after the frame borrow ends, so it first
affects the following record. `freq` remains nominal and is restored when the
control returns `None`.

The fixed signal order is control telemetry, `target`, `forcing`, `table`, and
`phase`. The target unit comes from `StandardControl::REFERENCE_UNIT`; the
remaining fixed units are volts, volts, and turns. A control may deliberately
exclude the table candidate from its output, but the candidate remains visible
for diagnosis. `Program::INPUTS_REQUIRED` forwards the control requirement.

## Output lifecycle

The real-time loop loads one `SafetyInputs` snapshot per tick. For a gated rig,
output is enabled only when it is armed and not tripped; an ungated rig is
always enabled. The same snapshot reaches the downstream safety decision.

`StandardProgram` resets a control on a disabled-to-enabled transition,
reports enabled state through `set_output_enabled`, and forces raw output to
zero and nominal frequency while disabled. The harmonic generator and table
player continue advancing. The control step is still called for bounded
lifecycle and fault-pulse bookkeeping, so implementations must freeze dynamic
estimators while disabled.

A rig or programme fault first evaluated after the control step quiets that
tick immediately, but the control has advanced once. The latched trip freezes
it from the following tick. This one-tick qualification avoids a duplicate
pre-step fault path.

## Existing `Pll`, revised in place

`helic_core::Pll` remains the platform's bounded fundamental PLL. It now:

- uses Fourier phase `atan2(-b, a)` and response-minus-excitation phase;
- forms error as corrected measured phase minus target, so the ordinary
  negative phase-frequency slope uses positive gains;
- removes separately estimated DC components from excitation and response;
- applies separate squared amplitude qualifications;
- observes a five-time-constant warm-up before acquisition timeout starts;
- stores exact integer centre and bounds, with PI corrections as `f32` values
  relative to the centre;
- uses conditional-integration anti-windup and unquantised stationarity over
  the lock dwell;
- compensates signed differential instrumentation delay with multi-turn phase
  reduction;
- distinguishes non-faulting acquisition timeout from latched `LockLost`; and
- provides checked live setters, explicit reacquisition, and coherent phase,
  amplitude, increment, state, validity, and saturation views.

`LockLost` can be left only by `reset`; replayed enable and `reacquire` do not
clear it. Frequency gains and stationarity tolerance are supplied in phase
increment units. A rig parameter group should expose hertz units and convert
once on core 0.

Square roots are taken only for telemetry using an SRAM-resident `f32`
implementation. Hardware timing, compiler-generated calls, instrumentation
delay, and the phase convention still require verification in each rig.
