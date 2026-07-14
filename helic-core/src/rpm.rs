//! Fixed-state RPM estimation from a once-per-revolution pulse period.
//!
//! The estimator retains the latest raw period, applies a time-normalised
//! EWMA to RPM, rejects implausible periods and invalidates stale estimates.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RpmEstimate {
    pub period_s: f32,
    pub rpm: f32,
    pub valid: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct RpmEstimator {
    tau_s: f32,
    stale_after_s: f32,
    min_period_s: f32,
    estimate: RpmEstimate,
    age_s: f32,
}

impl RpmEstimator {
    pub const fn new(tau_s: f32, stale_after_s: f32, min_period_s: f32) -> Self {
        Self {
            tau_s,
            stale_after_s,
            min_period_s,
            estimate: RpmEstimate {
                period_s: 0.0,
                rpm: 0.0,
                valid: false,
            },
            age_s: 0.0,
        }
    }

    pub fn observe(&mut self, period_s: f32) -> bool {
        if !period_s.is_finite() || period_s < self.min_period_s || period_s >= self.stale_after_s {
            return false;
        }
        let instantaneous = 60.0 / period_s;
        if self.estimate.valid {
            let alpha = period_s / (self.tau_s + period_s);
            self.estimate.rpm += alpha * (instantaneous - self.estimate.rpm);
        } else {
            self.estimate.rpm = instantaneous;
        }
        self.estimate.period_s = period_s;
        self.estimate.valid = true;
        self.age_s = 0.0;
        true
    }

    pub fn tick(&mut self, dt_s: f32) {
        self.age_s += dt_s;
        if self.age_s >= self.stale_after_s {
            self.estimate.rpm = 0.0;
            self.estimate.valid = false;
        }
    }

    pub const fn estimate(&self) -> RpmEstimate {
        self.estimate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAU: f32 = 0.25;
    const STALE: f32 = 0.1;
    const MIN_PERIOD: f32 = 0.005;

    #[test]
    fn first_period_sets_raw_period_and_rpm() {
        let mut estimator = RpmEstimator::new(TAU, STALE, MIN_PERIOD);
        assert!(estimator.observe(0.03));
        assert_eq!(
            estimator.estimate(),
            RpmEstimate {
                period_s: 0.03,
                rpm: 2000.0,
                valid: true,
            }
        );
    }

    #[test]
    fn ewma_uses_elapsed_time_weighting() {
        let mut estimator = RpmEstimator::new(TAU, STALE, MIN_PERIOD);
        estimator.observe(0.03);
        estimator.observe(0.01);
        let expected = 2000.0 + (0.01 / 0.26) * 4000.0;
        assert!((estimator.estimate().rpm - expected).abs() < 0.001);
        assert_eq!(estimator.estimate().period_s, 0.01);
    }

    #[test]
    fn glitches_do_not_change_or_refresh_the_estimate() {
        let mut estimator = RpmEstimator::new(TAU, STALE, MIN_PERIOD);
        estimator.observe(0.02);
        estimator.tick(0.09);
        assert!(!estimator.observe(0.004));
        estimator.tick(0.01);
        assert_eq!(estimator.estimate().period_s, 0.02);
        assert_eq!(estimator.estimate().rpm, 0.0);
        assert!(!estimator.estimate().valid);
    }

    #[test]
    fn stale_estimate_keeps_the_last_raw_period() {
        let mut estimator = RpmEstimator::new(TAU, STALE, MIN_PERIOD);
        estimator.observe(0.01);
        for _ in 0..200 {
            estimator.tick(0.0005);
        }
        assert_eq!(estimator.estimate().period_s, 0.01);
        assert_eq!(estimator.estimate().rpm, 0.0);
        assert!(!estimator.estimate().valid);
    }

    #[test]
    fn expected_operating_range_is_accepted() {
        let mut estimator = RpmEstimator::new(TAU, STALE, MIN_PERIOD);
        for rpm in [2000.0, 4000.0, 6000.0] {
            assert!(estimator.observe(60.0 / rpm));
        }
    }

    #[test]
    fn periods_longer_than_the_stale_limit_are_rejected() {
        let mut estimator = RpmEstimator::new(TAU, STALE, MIN_PERIOD);
        assert!(!estimator.observe(STALE));
        assert!(!estimator.observe(1.0));
        assert!(!estimator.estimate().valid);
    }
}
