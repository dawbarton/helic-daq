//! The controller abstraction: what runs inside each sample tick, between
//! measurement and DAC output. Implementations are swapped at **compile
//! time** via the `ActiveController` alias in the firmware's `config.rs`.

use crate::pid::Pid;

/// Number of ADC input channels (AD7608).
pub const ADC_CHANNELS: usize = 8;

/// One tick's worth of measurements, in engineering units. The controller
/// picks what it uses (agreed design: feedback source is configurable).
#[derive(Clone, Copy, Debug, Default)]
pub struct Measurements {
    /// ADC inputs in volts.
    pub adc: [f32; ADC_CHANNELS],
    /// Latest laser displacement value (sensor units; may be up to one
    /// sample old relative to `adc`).
    pub laser: f32,
}

/// A controller that runs once per sample tick.
///
/// `reference` is the current value of the periodic reference generator;
/// `dt` is the sample period in seconds. The return value is the controller's
/// contribution to the output in volts (the firmware adds any feed-forward
/// forcing and handles DAC scaling/clamping).
pub trait Controller {
    fn tick(&mut self, m: &Measurements, reference: f32, dt: f32) -> f32;

    /// Reset internal state (integrators, filter history). Called when
    /// control is enabled or re-armed.
    fn reset(&mut self) {}
}

/// Open-loop pass-through: output is the reference itself. The default
/// controller for bring-up and pure signal-generation use.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassThrough;

impl Controller for PassThrough {
    #[inline]
    fn tick(&mut self, _m: &Measurements, reference: f32, _dt: f32) -> f32 {
        reference
    }
}

/// PID feedback on a selectable ADC channel (or the laser input), tracking
/// the reference.
#[derive(Clone, Copy, Debug)]
pub struct PidController {
    pub pid: Pid,
    /// Which measurement is fed back: `Some(ch)` = ADC channel, `None` = laser.
    pub feedback: Option<usize>,
}

impl PidController {
    pub fn new(pid: Pid, feedback: Option<usize>) -> Self {
        Self { pid, feedback }
    }

    #[inline]
    fn measurement(&self, m: &Measurements) -> f32 {
        match self.feedback {
            Some(ch) => m.adc[ch],
            None => m.laser,
        }
    }
}

impl Controller for PidController {
    #[inline]
    fn tick(&mut self, m: &Measurements, reference: f32, dt: f32) -> f32 {
        let error = reference - self.measurement(m);
        self.pid.update(error, dt)
    }

    fn reset(&mut self) {
        self.pid.reset();
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
        let m = Measurements::default();
        assert_eq!(c.tick(&m, 1.25, DT), 1.25);
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
            Some(0),
        );
        let tau = 0.05f32;
        let mut m = Measurements::default();
        for _ in 0..80_000 {
            let u = c.tick(&m, 1.0, DT);
            m.adc[0] += (u - m.adc[0]) / tau * DT;
        }
        assert!((m.adc[0] - 1.0).abs() < 1e-3, "settled at {}", m.adc[0]);
    }

    #[test]
    fn feedback_source_selects_laser() {
        let mut c = PidController::new(
            Pid::new(PidConfig {
                kp: 1.0,
                ..Default::default()
            }),
            None,
        );
        let m = Measurements {
            laser: 0.25,
            ..Default::default()
        };
        assert_eq!(c.tick(&m, 1.0, DT), 0.75);
    }
}
