//! Portable per-experiment hardware and sample-clock contracts.

use helic_core::controller::Controller;

pub const MAX_SOURCES: usize = 24;
const DISCOVERY_HEADROOM: usize = helic_proto::MAX_PAYLOAD * 3 / 4;
const MAX_SOURCE_REGISTRY_ENCODED_LEN: usize =
    MAX_SOURCES * (helic_proto::payload::MAX_NAME_LEN + helic_proto::payload::MAX_UNIT_LEN + 2);
const _: () = assert!(MAX_SOURCE_REGISTRY_ENCODED_LEN <= DISCOVERY_HEADROOM);
pub const GENERATED_SOURCES: &[(&str, &str)] = &[
    ("target", "V"),
    ("forcing", "V"),
    ("table", "V"),
    ("out", "V"),
    ("cmd_epoch", "count"),
];

pub const fn source_count<R: Rig>() -> usize {
    R::INPUTS.len() + R::Ctrl::TELEMETRY.len() + GENERATED_SOURCES.len()
}

pub fn source<R: Rig>(index: usize) -> Option<(&'static str, &'static str)> {
    if let Some(source) = R::INPUTS.get(index) {
        return Some(*source);
    }
    let index = index - R::INPUTS.len();
    if let Some(source) = R::Ctrl::TELEMETRY.get(index) {
        return Some(*source);
    }
    GENERATED_SOURCES
        .get(index - R::Ctrl::TELEMETRY.len())
        .copied()
}

pub fn validate_sources<R: Rig>() {
    assert!(
        source_count::<R>() <= MAX_SOURCES,
        "experiment exposes more stream sources than supported"
    );
    let mut encoded_len = 0;
    for i in 0..source_count::<R>() {
        let (name, unit) = source::<R>(i).unwrap();
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
                source::<R>(j).unwrap().0,
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
