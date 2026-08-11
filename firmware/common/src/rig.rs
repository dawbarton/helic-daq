//! Per-experiment hardware and sample-clock contracts.

use embassy_rp::gpio::{Input, Pin, Pull};
use embassy_rp::pac;
use embassy_rp::pwm::{self, Pwm, Slice};
use embassy_rp::Peri;
use fixed::traits::ToFixed;
use helic_rt::{SampleRate, TickSource};

/// [`TickSource`] on the BUSY falling edge, using the IO bank's raw
/// edge-detect latch. Because the latch is armed continuously (not re-armed
/// per wait as the async `InputFuture` is), an edge that arrives while the
/// previous tick body is still running is not lost: the next wait returns
/// immediately and the loop catches up instead of silently skipping samples.
pub struct BusyEdgeSpinTick {
    /// Keeps the pin configured (pull-down so a missing ADC reads idle).
    _busy: Input<'static>,
    pin: u8,
    timeout_us: u32,
}

impl BusyEdgeSpinTick {
    /// Take ownership of the BUSY pin and configure its disconnected state.
    /// The GPIO number used for raw latch access is derived before the typed
    /// pin is erased, so it cannot disagree with the owned input.
    pub fn new<P: Pin>(busy: Peri<'static, P>, sample_rate: SampleRate) -> Self {
        let pin = busy.pin();
        let this = Self {
            _busy: Input::new(busy, Pull::Down),
            pin,
            timeout_us: 2 * sample_rate.period_us() as u32,
        };
        // Discard any edge latched before the loop starts.
        pac::IO_BANK0
            .intr((this.pin / 8) as usize)
            .write(|w| w.set_edge_low((this.pin % 8) as usize, true));
        this
    }
}

impl TickSource for BusyEdgeSpinTick {
    #[unsafe(link_section = ".data.ram_func")]
    fn wait(&mut self) -> bool {
        let intr = pac::IO_BANK0.intr((self.pin / 8) as usize);
        let group = (self.pin % 8) as usize;
        let start = pac::TIMER0.timerawl().read();
        loop {
            if intr.read().edge_low(group) {
                // The edge latch is write-one-to-clear.
                intr.write(|w| w.set_edge_low(group, true));
                return true;
            }
            if pac::TIMER0.timerawl().read().wrapping_sub(start) > self.timeout_us {
                return false;
            }
        }
    }
}

/// Synchronous PWM-wrap tick for an ADC-free rig on a dedicated core.
///
/// The PWM peripheral owns the sample instant. Its raw wrap flag remains
/// latched while the tick body runs, so polling it from SRAM avoids the
/// executor, interrupt dispatch, waker and cross-core critical section used
/// by an interrupt-driven or executor-driven wait.
pub struct PwmWrapSpinTick {
    _pwm: Pwm<'static>,
    mask: u32,
    timeout_us: u32,
}

impl PwmWrapSpinTick {
    pub fn new<T: Slice>(slice: Peri<'static, T>, sample_rate: SampleRate) -> Self {
        let mask = 1 << slice.number();
        let (divider, top) = sample_rate.pwm_params();
        let mut config = pwm::Config::default();
        config.divider = divider.to_fixed();
        config.top = top;
        let pwm = Pwm::new_free(slice, config);

        // The synchronous path consumes the raw flag directly; leave the
        // processor-facing PWM interrupt disabled and discard any startup
        // wrap before beginning the loop.
        pac::PWM.irq0_inte().modify(|w| w.0 &= !mask);
        pac::PWM.intr().write(|w| w.0 = mask);

        Self {
            _pwm: pwm,
            mask,
            timeout_us: 2 * sample_rate.period_us() as u32,
        }
    }
}

impl TickSource for PwmWrapSpinTick {
    #[unsafe(link_section = ".data.ram_func")]
    fn wait(&mut self) -> bool {
        let start = pac::TIMER0.timerawl().read();
        loop {
            if pac::PWM.intr().read().0 & self.mask != 0 {
                // The raw wrap flag is write-one-to-clear and remains latched
                // while the previous tick body is running.
                pac::PWM.intr().write(|w| w.0 = self.mask);
                return true;
            }
            if pac::TIMER0.timerawl().read().wrapping_sub(start) > self.timeout_us {
                return false;
            }
        }
    }
}
