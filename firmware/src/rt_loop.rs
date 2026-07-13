//! The core-1 real-time loop and its cross-core interfaces.
//!
//! Timing architecture (`docs/implementation_plan.md` §1): a PWM slice
//! generates CONVST at the sample rate, so the AD7609 sampling instant is
//! crystal-timed regardless of software. The loop waits for BUSY to fall,
//! then runs read → generators → controller → DAC write.
//!
//! Cross-core rules: core 0 never touches the loop state directly. Parameter
//! changes arrive through a lock-free SPSC command mailbox and are applied
//! at a sample boundary (coefficient sets swap as whole values, so a tick
//! never sees a half-updated array). Sample records leave through a lock-free
//! SPSC ring buffer; when it is full, records are dropped and counted, and
//! the loop never blocks. Diagnostics are plain atomics.

use core::sync::atomic::{AtomicU32, Ordering};

use cbc_core::controller::{Controller, Measurements};
use cbc_core::generator::FourierCoeffs;
use cbc_core::lut::SinLut;
use cbc_core::phase::PhaseAccumulator;
use cbc_drivers::ad7609::{InputRange, Oversampling};
use cbc_drivers::AnalogIn;
use defmt::{info, warn};
use embassy_time::{with_timeout, Delay, Duration, Instant};
use heapless::spsc::{Consumer, Producer};
use static_cell::StaticCell;

use crate::board::AnalogParts;
use crate::config::{self, HARMONICS, OUTPUT_CHANNEL, SAMPLE_RATE};

/// Commands applied by the loop at sample boundaries (core 0 → core 1).
/// Array-valued parameters travel by value: enqueueing a whole coefficient
/// set is the lock-free equivalent of the double-buffer-and-swap.
#[allow(dead_code)] // remaining variants are driven by host commands (milestone 5)
#[derive(Clone, Copy, Debug)]
pub enum RtCommand {
    /// Phase-accumulator increment (from `PhaseAccumulator::increment_for`).
    /// Phase-continuous: the accumulator never resets.
    SetIncrement(u32),
    /// Replace the controller reference (target) coefficient set.
    SetTargetCoeffs(FourierCoeffs<HARMONICS>),
    /// Replace the feed-forward forcing coefficient set.
    SetForcingCoeffs(FourierCoeffs<HARMONICS>),
    /// Reset controller state (integrators, filter history).
    ResetController,
    /// Set a controller parameter (id per `Controller::param_names`).
    SetCtrlParam(u16, f32),
}

pub const COMMAND_QUEUE_LEN: usize = 32;
pub type CommandProducer = Producer<'static, RtCommand>;
pub type CommandConsumer = Consumer<'static, RtCommand>;

/// One tick's streamed record (core 1 → core 0).
#[derive(Clone, Copy, Debug, Default)]
pub struct Record {
    pub index: u32,
    pub adc: [f32; 8],
    pub laser: f32,
    pub target: f32,
    pub forcing: f32,
    pub out: f32,
}

pub const RECORD_QUEUE_LEN: usize = 256;
pub type RecordProducer = Producer<'static, Record>;
pub type RecordConsumer = Consumer<'static, Record>;

// Diagnostics, readable from either core (future registry parameters).
/// Last tick processing time, µs (BUSY fall → DAC written).
pub static LOOP_TIME_LAST_US: AtomicU32 = AtomicU32::new(0);
/// Maximum observed tick processing time, µs.
pub static LOOP_TIME_MAX_US: AtomicU32 = AtomicU32::new(0);
/// Ticks whose processing exceeded the sample period.
pub static OVERRUNS: AtomicU32 = AtomicU32::new(0);
/// Worst observed excess of tick-to-tick spacing over the nominal period, µs.
pub static CLOCK_JITTER_US: AtomicU32 = AtomicU32::new(0);
/// BUSY-edge waits that timed out (ADC absent or not converting).
pub static BUSY_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
/// Records dropped because the stream ring buffer was full.
pub static RECORDS_DROPPED: AtomicU32 = AtomicU32::new(0);
/// Total ticks processed.
pub static TICKS: AtomicU32 = AtomicU32::new(0);

/// Latest laser reading (f32 bits), written by the core-0 UART task.
pub static LASER_VALUE: AtomicU32 = AtomicU32::new(0);

static SIN_LUT: StaticCell<SinLut> = StaticCell::new();

/// The real-time loop task, running alone on core 1.
#[embassy_executor::task]
pub async fn rt_loop(
    analog: AnalogParts,
    mut commands: CommandConsumer,
    mut records: RecordProducer,
) -> ! {
    let mut rt = analog.build();
    let lut: &'static SinLut = SIN_LUT.init(SinLut::new());

    rt.adc.init(
        InputRange::Bipolar10V,
        Oversampling::for_sample_rate(SAMPLE_RATE.hz()),
        &mut Delay,
    );
    if rt.dac.zero_all_with_delay(&mut Delay).is_err() {
        warn!("DAC zeroing failed");
    }

    // Loop state.
    let mut phase = PhaseAccumulator::new();
    let mut target_coeffs = FourierCoeffs::<HARMONICS>::zero();
    let mut forcing_coeffs = FourierCoeffs::<HARMONICS>::zero();
    let mut controller = config::make_controller();
    let dt = SAMPLE_RATE.dt();
    let period = Duration::from_micros(SAMPLE_RATE.period_us());
    let adc_scale = rt.adc.scale();
    let mut index: u32 = 0;
    let mut last_tick: Option<Instant> = None;

    // Start the hardware sample clock last: conversions begin immediately.
    let (pwm_div, pwm_top) = SAMPLE_RATE.pwm_params();
    let _convst = rt.start_convst_pwm(pwm_div, pwm_top);
    info!(
        "core 1: RT loop running at {} Hz, {} harmonics",
        SAMPLE_RATE.hz(),
        HARMONICS
    );

    loop {
        // Wait for conversion-complete. The timeout keeps the loop alive
        // (at half rate) when no ADC is attached, e.g. bench bring-up.
        if with_timeout(period * 2, rt.adc_busy.wait_for_falling_edge())
            .await
            .is_err()
        {
            BUSY_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        }

        let t0 = Instant::now();
        rt.tick_pin.set_high();

        // Tick-to-tick spacing check (sampling itself is PWM-timed; this
        // watches the software's ability to keep up).
        if let Some(last) = last_tick {
            let spacing = (t0 - last).as_micros() as u32;
            let nominal = SAMPLE_RATE.period_us() as u32;
            if spacing > nominal {
                CLOCK_JITTER_US.fetch_max(spacing - nominal, Ordering::Relaxed);
            }
        }
        last_tick = Some(t0);

        // Apply pending parameter updates at the sample boundary.
        while let Some(cmd) = commands.dequeue() {
            match cmd {
                RtCommand::SetIncrement(inc) => phase.set_increment(inc),
                RtCommand::SetTargetCoeffs(c) => target_coeffs = c,
                RtCommand::SetForcingCoeffs(c) => forcing_coeffs = c,
                RtCommand::ResetController => controller.reset(),
                RtCommand::SetCtrlParam(id, value) => controller.set_param(id, value),
            }
        }

        // Measure.
        let frame = rt.adc.read_frame().unwrap_or_default();
        let mut m = Measurements {
            laser: f32::from_bits(LASER_VALUE.load(Ordering::Relaxed)),
            ..Default::default()
        };
        for (v, raw) in m.adc.iter_mut().zip(frame) {
            *v = raw as f32 * adc_scale;
        }

        // Generate: target (reference) and forcing share one phase, so all
        // harmonics of both stay locked together.
        let (theta, _period_start) = phase.step();
        let target = target_coeffs.evaluate(lut, theta);
        let forcing = forcing_coeffs.evaluate(lut, theta);

        // Control and output.
        let out = controller.tick(&m, target, dt) + forcing;
        let _ = rt.dac.write_volts(OUTPUT_CHANNEL, out);

        // Stream.
        let record = Record {
            index,
            adc: m.adc,
            laser: m.laser,
            target,
            forcing,
            out,
        };
        if records.enqueue(record).is_err() {
            RECORDS_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
        index = index.wrapping_add(1);

        rt.tick_pin.set_low();

        // Diagnostics.
        let elapsed = t0.elapsed().as_micros() as u32;
        LOOP_TIME_LAST_US.store(elapsed, Ordering::Relaxed);
        LOOP_TIME_MAX_US.fetch_max(elapsed, Ordering::Relaxed);
        if elapsed > SAMPLE_RATE.period_us() as u32 {
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        }
        TICKS.fetch_add(1, Ordering::Relaxed);
    }
}
