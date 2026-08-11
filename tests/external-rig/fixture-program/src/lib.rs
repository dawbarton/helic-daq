//! Minimal independent programme used by the external-rig acceptance fixture.

#![no_std]

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn programme_contract_is_owned_locally() {
        assert_eq!(FixtureProgram::OUTPUTS, 1);
        assert_eq!(FixtureProgram::INPUTS_REQUIRED, 1);
        assert_eq!(FixtureProgram::signal(0), Some(("bias", "V")));
    }
}
