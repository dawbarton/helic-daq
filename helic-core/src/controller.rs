//! The controller abstraction: what runs inside each sample tick, between
//! measurement and DAC output. Implementations are swapped at **compile
//! time** via the `ActiveController` alias in the firmware's `config.rs`.

use crate::pid::Pid;

/// A controller that runs once per sample tick.
///
/// `reference` is the current value of the periodic reference generator;
/// `dt` is the sample period in seconds. The return value is the controller's
/// contribution to the output in volts (the firmware adds any feed-forward
/// forcing and handles DAC scaling/clamping).
pub trait Controller {
    fn tick(&mut self, inputs: &[f32], reference: f32, dt: f32) -> f32;

    /// Reset internal state (integrators, filter history). Called when
    /// control is enabled or re-armed.
    fn reset(&mut self) {}

    /// Names of the controller's host-settable parameters, in `set_param`
    /// id order. The firmware appends these to its parameter registry, so
    /// adding a gain here makes it host-visible with no protocol changes.
    fn param_names() -> &'static [&'static str]
    where
        Self: Sized,
    {
        &[]
    }

    /// Set a controller parameter by id (index into `param_names`).
    /// Unknown ids are ignored.
    fn set_param(&mut self, _id: u16, _value: f32) {}

    /// Per-tick internal signals exposed after the experiment inputs.
    const TELEMETRY: &'static [(&'static str, &'static str)] = &[];

    fn telemetry(&self, _out: &mut [f32]) {}
}

/// Open-loop pass-through: output is the reference itself. The default
/// controller for bring-up and pure signal-generation use.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassThrough;

impl Controller for PassThrough {
    #[inline]
    fn tick(&mut self, _inputs: &[f32], reference: f32, _dt: f32) -> f32 {
        reference
    }
}

/// PID feedback on a selectable input slot, tracking the reference.
#[derive(Clone, Copy, Debug)]
pub struct PidController {
    pub pid: Pid,
    pub feedback: usize,
    error: f32,
}

impl PidController {
    pub fn new(pid: Pid, feedback: usize) -> Self {
        Self {
            pid,
            feedback,
            error: 0.0,
        }
    }

    #[inline]
    fn measurement(&self, inputs: &[f32]) -> f32 {
        inputs.get(self.feedback).copied().unwrap_or(0.0)
    }
}

impl Controller for PidController {
    #[inline]
    fn tick(&mut self, inputs: &[f32], reference: f32, dt: f32) -> f32 {
        self.error = reference - self.measurement(inputs);
        self.pid.update(self.error, dt)
    }

    fn reset(&mut self) {
        self.pid.reset();
    }

    fn param_names() -> &'static [&'static str] {
        &[
            "ctrl_kp",
            "ctrl_ki",
            "ctrl_kd",
            "ctrl_tau_d",
            "ctrl_feedback",
        ]
    }

    fn set_param(&mut self, id: u16, value: f32) {
        match id {
            0 => self.pid.config.kp = value,
            1 => self.pid.config.ki = value,
            2 => self.pid.config.kd = value,
            3 => self.pid.config.tau_d = value,
            // Rust's float-to-integer conversion truncates towards zero and
            // saturates outside the usize range, so malformed values cannot
            // wrap into an arbitrary slot.
            4 => self.feedback = value as usize,
            _ => {}
        }
    }

    const TELEMETRY: &'static [(&'static str, &'static str)] = &[("error", "V")];

    fn telemetry(&self, out: &mut [f32]) {
        if let Some(slot) = out.first_mut() {
            *slot = self.error;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pid::PidConfig;

    const DT: f32 = 1.0 / 8000.0;

    #[test]
    fn pass_through_returns_reference() {
        let mut c = PassThrough;
        assert_eq!(c.tick(&[], 1.25, DT), 1.25);
    }

    #[test]
    fn pid_controller_regulates_a_first_order_plant() {
        // Plant: dx/dt = (u - x)/tau. P control must converge towards the
        // setpoint (with the expected steady-state offset for pure P).
        let mut c = PidController::new(
            Pid::new(PidConfig {
                kp: 20.0,
                ki: 50.0,
                ..Default::default()
            }),
            0,
        );
        let tau = 0.05f32;
        let mut inputs = [0.0];
        for _ in 0..80_000 {
            let u = c.tick(&inputs, 1.0, DT);
            inputs[0] += (u - inputs[0]) / tau * DT;
        }
        assert!((inputs[0] - 1.0).abs() < 1e-3, "settled at {}", inputs[0]);
    }

    #[test]
    fn feedback_selects_input_slot_and_reports_error() {
        let mut c = PidController::new(
            Pid::new(PidConfig {
                kp: 1.0,
                ..Default::default()
            }),
            1,
        );
        assert_eq!(c.tick(&[10.0, 0.25], 1.0, DT), 0.75);
        let mut telemetry = [0.0];
        c.telemetry(&mut telemetry);
        assert_eq!(telemetry, [0.75]);
        assert_eq!(PidController::TELEMETRY, &[("error", "V")]);
    }

    #[test]
    fn feedback_parameter_truncates_to_slot_index() {
        let mut c = PidController::new(Pid::default(), 0);
        c.set_param(4, 2.9);
        assert_eq!(c.feedback, 2);
        assert_eq!(PidController::param_names()[4], "ctrl_feedback");
    }
}
