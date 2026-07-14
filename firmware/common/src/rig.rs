//! Per-experiment hardware and sample-clock contracts.

use core::future::poll_fn;
use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Poll;

use embassy_rp::gpio::Input;
use embassy_rp::interrupt::typelevel::PWM_IRQ_WRAP_0;
use embassy_rp::interrupt::typelevel::{Handler, Interrupt};
use embassy_rp::pac;
use embassy_rp::pwm::{self, Pwm, Slice};
use embassy_rp::Peri;
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::{with_timeout, Duration};
use fixed::traits::ToFixed;
use helic_core::controller::Controller;

use crate::rt_loop::TICK_TIMEOUTS;
use crate::SampleRate;

pub const MAX_SOURCES: usize = 24;
pub const GENERATED_SOURCES: &[(&str, &str)] = &[
    ("target", "V"),
    ("forcing", "V"),
    ("table", "V"),
    ("out", "V"),
];

pub fn source_count<R: Rig>() -> usize {
    R::INPUTS.len() + R::Ctrl::TELEMETRY.len() + GENERATED_SOURCES.len()
}

pub fn source<R: Rig>(index: usize) -> Option<(&'static str, &'static str)> {
    if let Some(source) = R::INPUTS.get(index) {
        return Some(*source);
    }
    let index = index - R::INPUTS.len();
    if let Some(source) = R::Ctrl::TELEMETRY.get(index) {
        return Some(*source);
    }
    GENERATED_SOURCES
        .get(index - R::Ctrl::TELEMETRY.len())
        .copied()
}

#[allow(async_fn_in_trait)]
pub trait TickSource {
    async fn wait(&mut self);
}

pub struct BusyEdgeTick {
    busy: Input<'static>,
    timeout: Duration,
}

impl BusyEdgeTick {
    pub fn new(busy: Input<'static>, sample_rate: SampleRate) -> Self {
        Self {
            busy,
            timeout: Duration::from_micros(2 * sample_rate.period_us()),
        }
    }
}

impl TickSource for BusyEdgeTick {
    async fn wait(&mut self) {
        if with_timeout(self.timeout, self.busy.wait_for_falling_edge())
            .await
            .is_err()
        {
            TICK_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

static PWM_WRAP_WAKER: AtomicWaker = AtomicWaker::new();
static PWM_WRAP_MASK: AtomicU32 = AtomicU32::new(0);
static PWM_WRAP_EVENTS: AtomicU32 = AtomicU32::new(0);

pub struct PwmWrapInterruptHandler;

impl Handler<PWM_IRQ_WRAP_0> for PwmWrapInterruptHandler {
    unsafe fn on_interrupt() {
        let pending = pac::PWM.irq0_ints().read().0 & PWM_WRAP_MASK.load(Ordering::Acquire);
        pac::PWM.intr().write(|w| w.0 = pending);
        PWM_WRAP_EVENTS.fetch_or(pending, Ordering::Release);
        PWM_WRAP_WAKER.wake();
    }
}

pub struct PwmWrapTick {
    _pwm: Pwm<'static>,
    mask: u32,
}

impl PwmWrapTick {
    pub fn new<T: Slice>(slice: Peri<'static, T>, sample_rate: SampleRate) -> Self {
        let mask = 1 << slice.number();
        let (divider, top) = sample_rate.pwm_params();
        let mut config = pwm::Config::default();
        config.divider = divider.to_fixed();
        config.top = top;
        let pwm = Pwm::new_free(slice, config);

        PWM_WRAP_MASK.store(mask, Ordering::Release);
        PWM_WRAP_EVENTS.fetch_and(!mask, Ordering::Relaxed);
        pac::PWM.intr().write(|w| w.0 = mask);
        pac::PWM.irq0_inte().modify(|w| w.0 |= mask);
        PWM_IRQ_WRAP_0::unpend();
        unsafe { PWM_IRQ_WRAP_0::enable() };

        Self { _pwm: pwm, mask }
    }
}

impl TickSource for PwmWrapTick {
    async fn wait(&mut self) {
        poll_fn(|cx| {
            PWM_WRAP_WAKER.register(cx.waker());
            if PWM_WRAP_EVENTS.fetch_and(!self.mask, Ordering::Acquire) & self.mask != 0 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

impl Drop for PwmWrapTick {
    fn drop(&mut self) {
        pac::PWM.irq0_inte().modify(|w| w.0 &= !self.mask);
    }
}

pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];

    type Tick: TickSource;
    type Ctrl: Controller;

    fn init(&mut self);
    fn measure(&mut self, values: &mut [f32]);
    fn actuate(&mut self, out: f32);

    fn tick_start(&mut self) {}
    fn tick_end(&mut self) {}

    fn param_names() -> &'static [&'static str]
    where
        Self: Sized,
    {
        &[]
    }

    fn param_defaults() -> &'static [f32]
    where
        Self: Sized,
    {
        &[]
    }

    fn set_param(&mut self, _id: u16, _value: f32) {}
}
