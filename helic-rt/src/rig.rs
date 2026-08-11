//! Portable per-experiment hardware and sample-clock contracts.

use helic_core::controller::Controller;

use crate::Program;

pub const MAX_SOURCES: usize = 24;
const DISCOVERY_HEADROOM: usize = helic_proto::MAX_PAYLOAD * 3 / 4;
const MAX_SOURCE_REGISTRY_ENCODED_LEN: usize =
    MAX_SOURCES * (helic_proto::payload::MAX_NAME_LEN + helic_proto::payload::MAX_UNIT_LEN + 2);
const _: () = assert!(MAX_SOURCE_REGISTRY_ENCODED_LEN <= DISCOVERY_HEADROOM);
const OUTPUT_SOURCE: (&str, &str) = ("out", "V");
const COMMAND_EPOCH_SOURCE: (&str, &str) = ("cmd_epoch", "count");

pub fn source_count<R: Rig, P: Program>() -> usize {
    R::INPUTS.len() + P::signal_count() + 2
}

pub fn source<R: Rig, P: Program>(index: usize) -> Option<(&'static str, &'static str)> {
    if let Some(source) = R::INPUTS.get(index) {
        return Some(*source);
    }
    let index = index - R::INPUTS.len();
    if let Some(source) = P::signal(index) {
        return Some(source);
    }
    match index.checked_sub(P::signal_count())? {
        0 => Some(OUTPUT_SOURCE),
        1 => Some(COMMAND_EPOCH_SOURCE),
        _ => None,
    }
}

pub fn validate_sources<R: Rig, P: Program>() {
    assert!(
        source_count::<R, P>() <= MAX_SOURCES,
        "experiment exposes more stream sources than supported"
    );
    assert!(
        P::INPUTS_REQUIRED <= R::INPUTS.len(),
        "programme requires more inputs than the rig provides"
    );
    let mut encoded_len = 0;
    for i in 0..source_count::<R, P>() {
        let (name, unit) = source::<R, P>(i).unwrap();
        assert!(
            name.len() <= helic_proto::payload::MAX_NAME_LEN
                && unit.len() <= helic_proto::payload::MAX_UNIT_LEN
                && name.is_ascii()
                && unit.is_ascii(),
            "source names/units exceed protocol text limits"
        );
        encoded_len += name.len() + unit.len() + 2;
        for j in 0..i {
            assert_ne!(
                name,
                source::<R, P>(j).unwrap().0,
                "source names must be unique"
            );
        }
    }
    assert!(
        encoded_len <= DISCOVERY_HEADROOM,
        "source registry exceeds its discovery headroom"
    );
}

/// Synchronous (busy-polling) tick source for a dedicated real-time core.
///
/// Waiting spins in SRAM instead of suspending an executor task: no interrupt
/// dispatch, waker registration, timer queue, or cross-core critical section
/// is involved. Every production experiment gives core 1 exclusively to this
/// contract; there is deliberately no asynchronous fallback.
pub trait TickSource {
    /// Block until the next hardware tick. Returns `false` if the tick had
    /// to be forced by timeout because no edge arrived.
    fn wait(&mut self) -> bool;
}

/// Statically dispatched physical experiment contract.
pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];

    /// Opt in to the shared per-tick safety gate. When `false` (the default),
    /// the summed actuator command is applied verbatim and the gate compiles
    /// away. A rig setting this to `true` must implement meaningful limits and
    /// a safe state through the hooks below.
    const SAFETY_GATED: bool = false;

    type Ctrl: Controller;

    fn init(&mut self);
    fn measure(&mut self, values: &mut [f32]);
    fn actuate(&mut self, out: f32);

    /// Perform one bounded step towards the experiment's reboot-safe hardware
    /// state, returning `true` when no further steps are required.
    ///
    /// This runs on core 1 at sample boundaries and must obey the same SRAM,
    /// timing, and dependency constraints as [`actuate`](Self::actuate).
    fn prepare_reboot(&mut self, step: u8) -> bool;

    /// Hard output limit applied after signal summation and before actuation.
    /// A buggy or unstable controller cannot drive beyond what this returns.
    fn clamp_output(&self, out: f32) -> f32 {
        out
    }

    /// Value actuated while disarmed or after a fault has latched. It should
    /// correspond to zero drive for the fitted output stage.
    fn safe_output(&self) -> f32 {
        0.0
    }

    /// Latching fault condition evaluated on this tick's measured inputs.
    /// `&mut self` permits per-tick staleness or other fault state.
    fn output_fault(&mut self, _inputs: &[f32]) -> bool {
        false
    }

    fn tick_start(&mut self) {}
    fn tick_end(&mut self) {}

    /// Phase of the hardware sample clock in microseconds since its trigger.
    /// `None` disables the loop's wake-latency diagnostics.
    fn tick_phase_us(&self) -> Option<u32> {
        None
    }

    fn param_names() -> &'static [&'static str]
    where
        Self: Sized,
    {
        &[]
    }

    fn param_defaults() -> &'static [f32]
    where
        Self: Sized,
    {
        &[]
    }

    fn normalise_param(id: u16, value: f32) -> Option<f32>
    where
        Self: Sized,
    {
        (Self::param_names().get(id as usize).is_some() && value.is_finite()).then_some(value)
    }

    fn set_param(&mut self, _id: u16, _value: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Payload, StepCtx};

    struct TestRig;

    impl Rig for TestRig {
        const INPUTS: &'static [(&'static str, &'static str)] = &[("input", "V")];
        type Ctrl = helic_core::controller::PassThrough;

        fn init(&mut self) {}
        fn measure(&mut self, _values: &mut [f32]) {}
        fn actuate(&mut self, _out: f32) {}
        fn prepare_reboot(&mut self, _step: u8) -> bool {
            true
        }
    }

    struct TestProgram;

    impl Program for TestProgram {
        const OUTPUTS: usize = 1;
        const INPUTS_REQUIRED: usize = 1;
        const DOMAINS: &'static [u8] = &[1];
        const SIGNALS: &'static [(&'static str, &'static str)] = &[("phase", "turn")];

        fn apply(&mut self, _domain: u8, _id: u16, _payload: Payload) {}
        fn step(&mut self, _inputs: &[f32], _dt: f32, _ctx: &StepCtx<'_>, _outputs: &mut [f32]) {}
        fn write_signals(&self, _out: &mut [f32]) {}
    }

    #[test]
    fn source_walk_preserves_segment_order() {
        validate_sources::<TestRig, TestProgram>();
        assert_eq!(source_count::<TestRig, TestProgram>(), 4);
        assert_eq!(source::<TestRig, TestProgram>(0), Some(("input", "V")));
        assert_eq!(source::<TestRig, TestProgram>(1), Some(("phase", "turn")));
        assert_eq!(source::<TestRig, TestProgram>(2), Some(("out", "V")));
        assert_eq!(
            source::<TestRig, TestProgram>(3),
            Some(("cmd_epoch", "count"))
        );
        assert_eq!(source::<TestRig, TestProgram>(4), None);
    }

    struct TooManyInputs;

    impl Program for TooManyInputs {
        const OUTPUTS: usize = 1;
        const INPUTS_REQUIRED: usize = 2;
        const DOMAINS: &'static [u8] = &[];
        const SIGNALS: &'static [(&'static str, &'static str)] = &[];

        fn apply(&mut self, _domain: u8, _id: u16, _payload: Payload) {}
        fn step(&mut self, _inputs: &[f32], _dt: f32, _ctx: &StepCtx<'_>, _outputs: &mut [f32]) {}
        fn write_signals(&self, _out: &mut [f32]) {}
    }

    #[test]
    #[should_panic(expected = "programme requires more inputs")]
    fn validation_rejects_insufficient_rig_inputs() {
        validate_sources::<TestRig, TooManyInputs>();
    }
}
