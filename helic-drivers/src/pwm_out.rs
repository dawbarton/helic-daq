//! PWM-backed analogue output scaling; reconstruction filtering is external.

use embedded_hal::pwm::SetDutyCycle;

use crate::AnalogOut;

pub fn duty_for_volts(volts: f32, v_min: f32, v_max: f32, max_duty: u16) -> u16 {
    debug_assert!(v_min < v_max);
    let volts = if volts.is_finite() { volts } else { 0.0 };
    let normalized = ((volts - v_min) / (v_max - v_min)).clamp(0.0, 1.0);
    (normalized * max_duty as f32 + 0.5) as u16
}

pub struct PwmOut<P: SetDutyCycle, const N: usize> {
    channels: [P; N],
    v_min: f32,
    v_max: f32,
}

impl<P: SetDutyCycle, const N: usize> PwmOut<P, N> {
    pub fn new(channels: [P; N], v_min: f32, v_max: f32) -> Self {
        assert!(v_min.is_finite() && v_max.is_finite() && v_min < v_max);
        Self {
            channels,
            v_min,
            v_max,
        }
    }

    pub fn write_volts(&mut self, channel: usize, volts: f32) -> Result<(), P::Error> {
        let pwm = &mut self.channels[channel];
        let duty = duty_for_volts(volts, self.v_min, self.v_max, pwm.max_duty_cycle());
        pwm.set_duty_cycle(duty)
    }

    pub fn zero_all(&mut self) -> Result<(), P::Error> {
        for channel in 0..N {
            self.write_volts(channel, 0.0)?;
        }
        Ok(())
    }
}

impl<P: SetDutyCycle, const N: usize> AnalogOut<N> for PwmOut<P, N> {
    type Error = P::Error;

    fn write(&mut self, channel: usize, code: u16) -> Result<(), Self::Error> {
        let pwm = &mut self.channels[channel];
        let max = pwm.max_duty_cycle() as u32;
        let duty = (code as u32 * max + u16::MAX as u32 / 2) / u16::MAX as u32;
        pwm.set_duty_cycle(duty as u16)
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use embedded_hal::pwm::{ErrorType, SetDutyCycle};

    use super::*;

    struct MockPwm {
        max: u16,
        duty: u16,
    }

    impl ErrorType for MockPwm {
        type Error = Infallible;
    }

    impl SetDutyCycle for MockPwm {
        fn max_duty_cycle(&self) -> u16 {
            self.max
        }

        fn set_duty_cycle(&mut self, duty: u16) -> Result<(), Self::Error> {
            self.duty = duty;
            Ok(())
        }
    }

    fn mock() -> MockPwm {
        MockPwm { max: 1023, duty: 0 }
    }

    #[test]
    fn rails_and_midpoint_map_to_duty() {
        assert_eq!(duty_for_volts(-5.0, -5.0, 5.0, 1023), 0);
        assert_eq!(duty_for_volts(0.0, -5.0, 5.0, 1023), 512);
        assert_eq!(duty_for_volts(5.0, -5.0, 5.0, 1023), 1023);
    }

    #[test]
    fn volts_clamp_and_non_finite_maps_to_zero_volts() {
        assert_eq!(duty_for_volts(-20.0, 0.0, 3.3, 2047), 0);
        assert_eq!(duty_for_volts(20.0, 0.0, 3.3, 2047), 2047);
        assert_eq!(duty_for_volts(f32::NAN, -5.0, 5.0, 1023), 512);
    }

    #[test]
    fn channels_are_independent() {
        let mut output = PwmOut::new([mock(), mock()], 0.0, 4.0);
        output.write_volts(0, 1.0).unwrap();
        output.write_volts(1, 3.0).unwrap();
        assert_eq!(output.channels[0].duty, 256);
        assert_eq!(output.channels[1].duty, 767);
    }

    #[test]
    fn raw_code_spans_the_available_duty() {
        let mut output = PwmOut::new([mock()], 0.0, 4.0);
        output.write(0, u16::MAX).unwrap();
        assert_eq!(output.channels[0].duty, 1023);
    }
}
