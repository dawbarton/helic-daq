//! Per-experiment hardware and sample-clock contracts.

use embassy_rp::gpio::{Input, Pin, Pull};
use embassy_rp::pac;
use embassy_rp::pwm::{self, Pwm, Slice};
use embassy_rp::Peri;
use fixed::traits::ToFixed;
use helic_core::controller::Controller;

use crate::SampleRate;

pub const MAX_SOURCES: usize = 24;
const DISCOVERY_HEADROOM: usize = helic_proto::MAX_PAYLOAD * 3 / 4;
const MAX_SOURCE_REGISTRY_ENCODED_LEN: usize =
    MAX_SOURCES * (helic_proto::payload::MAX_NAME_LEN + helic_proto::payload::MAX_UNIT_LEN + 2);
const _: () = assert!(MAX_SOURCE_REGISTRY_ENCODED_LEN <= DISCOVERY_HEADROOM);
pub const GENERATED_SOURCES: &[(&str, &str)] = &[
    ("target", "V"),
    ("forcing", "V"),
    ("table", "V"),
    ("out", "V"),
];

pub const fn source_count<R: Rig>() -> usize {
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

pub fn validate_sources<R: Rig>() {
    assert!(
        source_count::<R>() <= MAX_SOURCES,
        "experiment exposes more stream sources than supported"
    );
    let mut encoded_len = 0;
    for i in 0..source_count::<R>() {
        let (name, unit) = source::<R>(i).unwrap();
        assert!(
            name.len() <= helic_proto::payload::MAX_NAME_LEN
                && unit.len() <= helic_proto::payload::MAX_UNIT_LEN
                && name.is_ascii()
                && unit.is_ascii(),
            "source names/units exceed protocol text limits"
        );
        encoded_len += name.len() + unit.len() + 2;
        for j in 0..i {
            assert_ne!(
                name,
                source::<R>(j).unwrap().0,
                "source names must be unique"
            );
        }
    }
    assert!(
        encoded_len <= DISCOVERY_HEADROOM,
        "source registry exceeds its discovery headroom"
    );
}

/// Synchronous (busy-polling) tick source for a dedicated real-time core.
///
/// Waiting spins in SRAM instead of suspending an executor task: no interrupt
/// dispatch, waker registration, timer queue, or cross-core critical section
/// is involved. Every production experiment gives core 1 exclusively to this
/// contract; there is deliberately no asynchronous fallback.
pub trait TickSource {
    /// Block until the next hardware tick. Returns `false` if the tick had
    /// to be forced by timeout because no edge arrived.
    fn wait(&mut self) -> bool;
}

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

pub trait Rig {
    const INPUTS: &'static [(&'static str, &'static str)];

    type Ctrl: Controller;

    fn init(&mut self);
    fn measure(&mut self, values: &mut [f32]);
    fn actuate(&mut self, out: f32);

    fn tick_start(&mut self) {}
    fn tick_end(&mut self) {}

    /// Phase of the hardware sample clock in microseconds since the
    /// conversion trigger, if the rig can report it. Used by the loop's
    /// wake-latency diagnostics; `None` disables them.
    fn tick_phase_us(&self) -> Option<u32> {
        None
    }

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

    fn normalise_param(id: u16, value: f32) -> Option<f32>
    where
        Self: Sized,
    {
        (Self::param_names().get(id as usize).is_some() && value.is_finite()).then_some(value)
    }

    fn set_param(&mut self, _id: u16, _value: f32) {}
}
