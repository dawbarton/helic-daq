//! Pure, vector-valued safety decisions for the real-time output boundary.

use crate::{Rig, SafetyInputs};

/// Per-tick events emitted by [`safety_decide`].
///
/// There is deliberately no armed successor state: core 0 alone owns arming
/// and disarming, while core 1 may only request the monotonic trip latch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SafetyOutcome {
    pub newly_tripped: bool,
    pub quieted: bool,
    pub clamped: bool,
}

/// Decide the complete applied actuator vector without touching shared state.
///
/// `fault` already combines rig- and programme-originated faults. A non-finite
/// command is also a fault, so it cannot reach a DAC driver's fallback
/// conversion. The returned trip request deliberately remains true while a
/// fault remains present, even if the snapshot was already tripped: a
/// concurrent core-0 re-arm must be re-latched rather than masking the fault.
#[inline]
#[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
pub fn safety_decide<R: Rig>(
    rig: &R,
    inputs: SafetyInputs,
    fault: bool,
    commanded: &[f32],
    applied: &mut [f32],
) -> SafetyOutcome {
    debug_assert_eq!(commanded.len(), applied.len());
    debug_assert_eq!(commanded.len(), R::ACTUATORS.len());

    if !R::SAFETY_GATED {
        applied.copy_from_slice(commanded);
        return SafetyOutcome::default();
    }

    let newly_tripped = fault || commanded.iter().any(|value| !value.is_finite());
    if newly_tripped || inputs.tripped || !inputs.armed {
        for (actuator, value) in applied.iter_mut().enumerate() {
            *value = rig.safe_output(actuator);
        }
        SafetyOutcome {
            newly_tripped,
            quieted: true,
            clamped: false,
        }
    } else {
        let mut clamped = false;
        for (actuator, (commanded, applied)) in commanded.iter().zip(applied.iter_mut()).enumerate()
        {
            *applied = rig.clamp_output(actuator, *commanded);
            clamped |= *applied != *commanded;
        }
        SafetyOutcome {
            newly_tripped: false,
            quieted: false,
            clamped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct VectorRig;

    impl Rig for VectorRig {
        const INPUTS: &'static [(&'static str, &'static str)] = &[];
        const ACTUATORS: &'static [(&'static str, &'static str)] = &[("left", "V"), ("right", "V")];
        const SAFETY_GATED: bool = true;

        fn init(&mut self) {}
        fn measure(&mut self, _values: &mut [f32]) {}
        fn actuate(&mut self, _outputs: &[f32]) {}
        fn prepare_reboot(&mut self, _step: u8) -> bool {
            true
        }
        fn clamp_output(&self, actuator: usize, output: f32) -> f32 {
            let limit = actuator as f32 + 1.0;
            output.clamp(-limit, limit)
        }
        fn safe_output(&self, actuator: usize) -> f32 {
            10.0 + actuator as f32
        }
    }

    #[test]
    fn clamps_each_actuator_and_counts_one_tick() {
        let mut applied = [0.0; 2];
        let outcome = safety_decide(
            &VectorRig,
            SafetyInputs {
                armed: true,
                tripped: false,
            },
            false,
            &[3.0, -3.0],
            &mut applied,
        );
        assert_eq!(applied, [1.0, -2.0]);
        assert_eq!(
            outcome,
            SafetyOutcome {
                newly_tripped: false,
                quieted: false,
                clamped: true,
            }
        );
    }

    #[test]
    fn every_fault_source_quiets_the_complete_vector() {
        for (inputs, fault, commanded) in [
            (
                SafetyInputs {
                    armed: false,
                    tripped: false,
                },
                false,
                [0.5, -0.5],
            ),
            (
                SafetyInputs {
                    armed: true,
                    tripped: true,
                },
                false,
                [0.5, -0.5],
            ),
            (
                SafetyInputs {
                    armed: true,
                    tripped: false,
                },
                true,
                [0.5, -0.5],
            ),
            (
                SafetyInputs {
                    armed: true,
                    tripped: false,
                },
                false,
                [f32::NAN, -0.5],
            ),
        ] {
            let mut applied = [0.0; 2];
            let outcome = safety_decide(&VectorRig, inputs, fault, &commanded, &mut applied);
            assert_eq!(applied, [10.0, 11.0]);
            assert!(outcome.quieted);
            assert!(!outcome.clamped);
            assert_eq!(outcome.newly_tripped, fault || commanded[0].is_nan());
        }
    }

    #[test]
    fn persistent_fault_requests_relatched_trip_after_stale_snapshot() {
        let mut applied = [0.0; 2];
        let outcome = safety_decide(
            &VectorRig,
            SafetyInputs {
                armed: true,
                tripped: true,
            },
            true,
            &[0.0, 0.0],
            &mut applied,
        );
        assert!(outcome.newly_tripped);
    }

    struct NonGatedRig;

    impl Rig for NonGatedRig {
        const INPUTS: &'static [(&'static str, &'static str)] = &[];
        const ACTUATORS: &'static [(&'static str, &'static str)] = &[("left", "V"), ("right", "V")];

        fn init(&mut self) {}
        fn measure(&mut self, _values: &mut [f32]) {}
        fn actuate(&mut self, _outputs: &[f32]) {}
        fn prepare_reboot(&mut self, _step: u8) -> bool {
            true
        }
        fn clamp_output(&self, _actuator: usize, _output: f32) -> f32 {
            panic!("a non-gated rig must not clamp")
        }
        fn safe_output(&self, _actuator: usize) -> f32 {
            panic!("a non-gated rig must not substitute a safe value")
        }
    }

    #[test]
    fn non_gated_rig_applies_all_outputs_verbatim() {
        let commanded = [f32::NAN, f32::INFINITY];
        let mut applied = [0.0; 2];
        let outcome = safety_decide(
            &NonGatedRig,
            SafetyInputs {
                armed: false,
                tripped: true,
            },
            true,
            &commanded,
            &mut applied,
        );
        assert!(applied[0].is_nan());
        assert_eq!(applied[1], f32::INFINITY);
        assert_eq!(outcome, SafetyOutcome::default());
    }
}
