//! Minimal independent programme and controller used by the external-rig
//! acceptance fixtures.
//!
//! The controller exists so that a locally defined `Controller`, with its own
//! host-settable parameter and telemetry, is instantiated through the shared
//! `ControllerGroup` and control service from outside both HELIC-DAQ
//! workspaces.

#![no_std]

use helic_core::controller::Controller;
use helic_rt::{Payload, Program, StepCtx};

/// Pass a commanded bias to the fixture's single actuator.
pub struct FixtureProgram {
    bias: f32,
}

impl FixtureProgram {
    pub const fn new() -> Self {
        Self { bias: 0.0 }
    }
}

impl Default for FixtureProgram {
    fn default() -> Self {
        Self::new()
    }
}

impl Program for FixtureProgram {
    const OUTPUTS: usize = 1;
    const INPUTS_REQUIRED: usize = 1;
    const DOMAINS: &'static [u8] = &[7];
    const SIGNALS: &'static [(&'static str, &'static str)] = &[("bias", "V")];

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply(&mut self, domain: u8, id: u16, payload: Payload) {
        if let (7, 0, Payload::F32(value)) = (domain, id, payload) {
            self.bias = value;
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: &[f32], _dt: f32, _ctx: &StepCtx<'_>, outputs: &mut [f32]) {
        outputs[0] = inputs[0] + self.bias;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn write_signals(&self, out: &mut [f32]) {
        out[0] = self.bias;
    }
}

/// Proportional controller owning one host-settable gain and one telemetry
/// signal, both declared locally rather than by the platform.
#[derive(Clone, Copy, Debug, Default)]
pub struct FixtureController {
    gain: f32,
    last_error: f32,
}

impl FixtureController {
    pub const fn new() -> Self {
        Self {
            gain: 1.0,
            last_error: 0.0,
        }
    }
}

impl Controller for FixtureController {
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn tick(&mut self, inputs: &[f32], reference: f32, _dt: f32) -> f32 {
        self.last_error = reference - inputs[0];
        self.gain * self.last_error
    }

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn reset(&mut self) {
        self.last_error = 0.0;
    }

    fn param_names() -> &'static [&'static str] {
        &["fixture_gain"]
    }

    fn param_value(&self, id: u16) -> Option<f32> {
        (id == 0).then_some(self.gain)
    }

    fn set_param(&mut self, id: u16, value: f32) {
        if id == 0 {
            self.gain = value;
        }
    }

    const TELEMETRY: &'static [(&'static str, &'static str)] = &[("fixture_error", "V")];

    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn telemetry(&self, out: &mut [f32]) {
        out[0] = self.last_error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controller_owns_its_parameter_and_telemetry() {
        let mut controller = FixtureController::new();
        assert_eq!(FixtureController::param_names(), &["fixture_gain"]);
        controller.set_param(0, 2.0);
        assert_eq!(controller.param_value(0), Some(2.0));

        assert_eq!(controller.tick(&[1.0], 3.0, 0.001), 4.0);
        let mut telemetry = [0.0];
        controller.telemetry(&mut telemetry);
        assert_eq!(telemetry, [2.0]);

        controller.reset();
        controller.telemetry(&mut telemetry);
        assert_eq!(telemetry, [0.0]);
    }

    #[test]
    fn programme_contract_is_owned_locally() {
        assert_eq!(FixtureProgram::OUTPUTS, 1);
        assert_eq!(FixtureProgram::INPUTS_REQUIRED, 1);
        assert_eq!(FixtureProgram::signal(0), Some(("bias", "V")));
    }
}
