//! Discrete PID controller with filtered derivative and anti-windup.

/// PID gains and limits. `kd` acts on the error derivative through a
/// first-order low-pass with time constant `tau_d` (set `tau_d = 0.0` for an
/// unfiltered finite difference). Output is clamped to `[out_min, out_max]`
/// with conditional integration for anti-windup.
#[derive(Clone, Copy, Debug)]
pub struct PidConfig {
    pub kp: f32,
    pub ki: f32,
    pub kd: f32,
    /// Derivative filter time constant in seconds.
    pub tau_d: f32,
    pub out_min: f32,
    pub out_max: f32,
}

impl Default for PidConfig {
    fn default() -> Self {
        Self {
            kp: 0.0,
            ki: 0.0,
            kd: 0.0,
            tau_d: 0.0,
            out_min: f32::MIN,
            out_max: f32::MAX,
        }
    }
}

/// PID state. `update` is RT-safe (no allocation, no branches beyond clamps).
#[derive(Clone, Copy, Debug)]
pub struct Pid {
    pub config: PidConfig,
    integral: f32,
    prev_error: f32,
    deriv: f32,
    first: bool,
}

// Manual impl: the derived `Default` would set `first: false`, skipping the
// first-sample derivative-spike suppression that `new` establishes.
impl Default for Pid {
    fn default() -> Self {
        Self::new(PidConfig::default())
    }
}

impl Pid {
    pub fn new(config: PidConfig) -> Self {
        Self {
            config,
            integral: 0.0,
            prev_error: 0.0,
            deriv: 0.0,
            first: true,
        }
    }

    /// Reset integrator and derivative history (e.g. when enabling control).
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
        self.deriv = 0.0;
        self.first = true;
    }

    /// Advance one sample: `error` = setpoint − measurement, `dt` = sample
    /// period in seconds. Returns the clamped controller output.
    #[inline]
    #[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
    pub fn update(&mut self, error: f32, dt: f32) -> f32 {
        let c = &self.config;

        // Filtered derivative; suppress the spike on the very first sample.
        let raw_deriv = if self.first {
            self.first = false;
            0.0
        } else {
            (error - self.prev_error) / dt
        };
        self.prev_error = error;
        let alpha = if c.tau_d > 0.0 {
            dt / (c.tau_d + dt)
        } else {
            1.0
        };
        self.deriv += alpha * (raw_deriv - self.deriv);

        let unclamped = c.kp * error + self.integral + c.kd * self.deriv;
        let out = unclamped.clamp(c.out_min, c.out_max);

        // Conditional integration: freeze the integrator when it would push
        // the output further into saturation.
        let saturated_high = unclamped > c.out_max && error > 0.0;
        let saturated_low = unclamped < c.out_min && error < 0.0;
        if !(saturated_high || saturated_low) {
            self.integral += c.ki * error * dt;
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 8000.0;

    #[test]
    fn proportional_only() {
        let mut pid = Pid::new(PidConfig {
            kp: 2.5,
            ..Default::default()
        });
        assert_eq!(pid.update(1.0, DT), 2.5);
        assert_eq!(pid.update(-0.4, DT), -1.0);
    }

    #[test]
    fn integral_accumulates_to_expected_value() {
        let mut pid = Pid::new(PidConfig {
            ki: 10.0,
            ..Default::default()
        });
        // Constant error 1.0 for exactly 1 second: integral term → ki * 1 s.
        let mut out = 0.0;
        for _ in 0..8000 {
            out = pid.update(1.0, DT);
        }
        assert!((out - 10.0).abs() < 0.01, "out {out}");
    }

    #[test]
    fn derivative_of_ramp_is_slope() {
        let mut pid = Pid::new(PidConfig {
            kd: 1.0,
            ..Default::default() // tau_d = 0: unfiltered
        });
        let mut out = 0.0;
        for i in 0..100 {
            out = pid.update(3.0 * i as f32 * DT, DT); // error ramps at 3.0/s
        }
        assert!((out - 3.0).abs() < 1e-3, "out {out}");
    }

    #[test]
    fn derivative_filter_smooths_a_step() {
        let mut pid = Pid::new(PidConfig {
            kd: 1.0,
            tau_d: 0.01,
            ..Default::default()
        });
        pid.update(0.0, DT);
        // A step in error would give a one-sample spike of 1/dt = 8000
        // unfiltered; with tau_d = 10 ms the first-sample response is smaller
        // by roughly dt/tau_d.
        let out = pid.update(1.0, DT);
        assert!(out < 8000.0 * 0.05, "out {out}");
        assert!(out > 0.0);
    }

    #[test]
    fn output_clamps_and_integrator_does_not_wind_up() {
        let mut pid = Pid::new(PidConfig {
            kp: 1.0,
            ki: 100.0,
            out_min: -1.0,
            out_max: 1.0,
            ..Default::default()
        });
        // Large error saturates output; integrator must freeze.
        for _ in 0..8000 {
            assert_eq!(pid.update(10.0, DT), 1.0);
        }
        // On reversal the output must leave saturation promptly, not after
        // unwinding a huge integral.
        let out = pid.update(-10.0, DT);
        assert!(out <= -1.0 + 1e-6, "out {out} still saturated high");
    }

    #[test]
    fn default_suppresses_first_derivative_spike() {
        // Regression: the derived Default left `first: false`, so a
        // default-constructed Pid produced a (error - 0)/dt kick.
        let mut pid = Pid::default();
        pid.config.kd = 1.0;
        assert_eq!(pid.update(1.0, DT), 0.0);
    }

    #[test]
    fn reset_clears_state() {
        let mut pid = Pid::new(PidConfig {
            ki: 1.0,
            ..Default::default()
        });
        for _ in 0..1000 {
            pid.update(5.0, DT);
        }
        pid.reset();
        assert_eq!(pid.update(0.0, DT), 0.0);
    }
}
