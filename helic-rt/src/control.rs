//! Harmonic-frame-aware controls used by [`crate::StandardProgram`].

use helic_core::{HarmonicFrame, Pid};

use crate::{Payload, StepCtx};

/// Inputs shared by every standard control policy for one sample.
pub struct StandardControlInputs<'a, const H: usize> {
    pub measured: &'a [f32],
    pub reference: f32,
    pub reference_mean: f32,
    pub forcing: f32,
    pub table: f32,
    pub frame: &'a HarmonicFrame<H>,
    /// Increment which generated the current harmonic frame.
    pub current_increment: u32,
}

/// Complete result of one standard control step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlStep {
    /// Complete raw programme output, before the rig safety gate.
    pub output: f32,
    /// Increment for the next tick; `None` restores the nominal generator value.
    pub next_increment: Option<u32>,
}

/// Statically dispatched control policy for [`crate::StandardProgram`].
pub trait StandardControl<const H: usize> {
    const INPUTS_REQUIRED: usize = 0;
    const REFERENCE_UNIT: &'static str = "V";
    const TELEMETRY: &'static [(&'static str, &'static str)] = &[];

    fn step(&mut self, inputs: StandardControlInputs<'_, H>, ctx: &StepCtx<'_>) -> ControlStep;

    fn apply(&mut self, _id: u16, _payload: Payload) {}
    fn reset(&mut self) {}
    fn set_output_enabled(&mut self, _enabled: bool) {}
    fn telemetry(&self, _out: &mut [f32]) {}

    #[inline(always)]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn fault(&self) -> bool {
        false
    }

    fn scalar_param_names() -> &'static [&'static str]
    where
        Self: Sized,
    {
        &[]
    }

    fn scalar_param_value(&self, _id: u16) -> Option<f32> {
        None
    }

    fn normalise_scalar_param(id: u16, value: f32, _input_count: usize) -> Option<f32>
    where
        Self: Sized,
    {
        (Self::scalar_param_names().get(id as usize).is_some() && value.is_finite())
            .then_some(value)
    }
}

/// Open-loop reference plus additive forcing and table playback.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassThrough;

impl<const H: usize> StandardControl<H> for PassThrough {
    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: StandardControlInputs<'_, H>, _ctx: &StepCtx<'_>) -> ControlStep {
        ControlStep {
            output: inputs.reference + inputs.forcing + inputs.table,
            next_increment: None,
        }
    }
}

/// PID feedback on one compile-time input slot, tracking the reference.
#[derive(Clone, Copy, Debug)]
pub struct PidController<const FEEDBACK: usize> {
    pub pid: Pid,
    error: f32,
    output_enabled: bool,
}

impl<const FEEDBACK: usize> PidController<FEEDBACK> {
    pub const fn new(pid: Pid) -> Self {
        Self {
            pid,
            error: f32::NAN,
            output_enabled: false,
        }
    }
}

impl<const H: usize, const FEEDBACK: usize> StandardControl<H> for PidController<FEEDBACK> {
    const INPUTS_REQUIRED: usize = FEEDBACK + 1;
    const TELEMETRY: &'static [(&'static str, &'static str)] = &[("error", "V")];

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: StandardControlInputs<'_, H>, ctx: &StepCtx<'_>) -> ControlStep {
        if !self.output_enabled {
            return ControlStep {
                output: 0.0,
                next_increment: None,
            };
        }
        self.error = inputs.reference - inputs.measured[FEEDBACK];
        ControlStep {
            output: self.pid.update(self.error, ctx.sample_rate.dt())
                + inputs.forcing
                + inputs.table,
            next_increment: None,
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply(&mut self, id: u16, payload: Payload) {
        let Payload::F32(value) = payload else {
            return;
        };
        match id {
            0 => self.pid.config.kp = value,
            1 => self.pid.config.ki = value,
            2 => self.pid.config.kd = value,
            3 => self.pid.config.tau_d = value,
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.pid.reset();
        self.error = f32::NAN;
    }

    fn set_output_enabled(&mut self, enabled: bool) {
        self.output_enabled = enabled;
    }

    fn telemetry(&self, out: &mut [f32]) {
        if let Some(value) = out.first_mut() {
            *value = self.error;
        }
    }

    fn scalar_param_names() -> &'static [&'static str] {
        &["ctrl_kp", "ctrl_ki", "ctrl_kd", "ctrl_tau_d"]
    }

    fn scalar_param_value(&self, id: u16) -> Option<f32> {
        match id {
            0 => Some(self.pid.config.kp),
            1 => Some(self.pid.config.ki),
            2 => Some(self.pid.config.kd),
            3 => Some(self.pid.config.tau_d),
            _ => None,
        }
    }

    fn normalise_scalar_param(id: u16, value: f32, _input_count: usize) -> Option<f32> {
        if !value.is_finite() {
            return None;
        }
        match id {
            0..=2 => Some(value),
            3 if value >= 0.0 => Some(value),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use helic_core::{HarmonicFrame, PidConfig, SinLut};

    use super::*;
    use crate::SampleRate;

    const H: usize = 2;

    fn inputs<'a>(
        measured: &'a [f32],
        frame: &'a HarmonicFrame<H>,
    ) -> StandardControlInputs<'a, H> {
        StandardControlInputs {
            measured,
            reference: 1.0,
            reference_mean: 0.0,
            forcing: 0.25,
            table: 0.5,
            frame,
            current_increment: 123,
        }
    }

    fn ctx(lut: &SinLut) -> StepCtx<'_> {
        StepCtx {
            lut,
            sample_rate: SampleRate::Hz8000,
        }
    }

    #[test]
    fn pass_through_composes_reference_forcing_and_table() {
        let mut control = PassThrough;
        let frame = HarmonicFrame::zero();
        let lut = SinLut::new();
        let result = <PassThrough as StandardControl<H>>::step(
            &mut control,
            inputs(&[], &frame),
            &ctx(&lut),
        );
        assert_eq!(result.output, 1.75);
        assert_eq!(result.next_increment, None);
        assert!(<PassThrough as StandardControl<H>>::scalar_param_names().is_empty());
    }

    #[test]
    fn fixed_input_pid_uses_raw_parameter_ids_and_complete_composition() {
        let mut control = PidController::<1>::new(Pid::new(PidConfig::default()));
        <PidController<1> as StandardControl<H>>::apply(&mut control, 0, Payload::F32(2.0));
        <PidController<1> as StandardControl<H>>::set_output_enabled(&mut control, true);
        let frame = HarmonicFrame::zero();
        let lut = SinLut::new();
        let result = <PidController<1> as StandardControl<H>>::step(
            &mut control,
            inputs(&[99.0, 0.5], &frame),
            &ctx(&lut),
        );
        assert_eq!(result.output, 1.75);
        assert_eq!(<PidController<1> as StandardControl<H>>::INPUTS_REQUIRED, 2);
        assert_eq!(
            <PidController<1> as StandardControl<H>>::scalar_param_names(),
            &["ctrl_kp", "ctrl_ki", "ctrl_kd", "ctrl_tau_d"]
        );
    }
}
