//! Generic real-time loop, cross-core mailboxes, records and diagnostics.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::info;
use embassy_rp::pac;
use embassy_time::{Duration, Ticker};
use heapless::spsc::{Consumer, Producer};
use helic_core::controller::Controller;
use helic_core::generator::FourierCoeffs;
use helic_core::lut::SinLut;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableMode, TablePlayer, WaveTable};
use static_cell::StaticCell;

use crate::rig::{source_count, Rig, SyncTickSource, TickSource, MAX_SOURCES};
use crate::table;
use crate::{SampleRate, HARMONICS};

#[derive(Clone, Copy, Debug)]
pub enum RtCommand {
    SetIncrement(u32),
    SetTargetCoeffs(FourierCoeffs<HARMONICS>),
    SetForcingCoeffs(FourierCoeffs<HARMONICS>),
    SetTableIncrement(u32),
    SetTableGain(f32),
    SetTableMode(TableMode),
    SetTableMultiplier(u32),
    SetTablePhase(u32),
    TriggerTable,
    UseTable(u8),
    ResetController,
    SetCtrlParam(u16, f32),
    SetRigParam(u16, f32),
}

pub const COMMAND_QUEUE_LEN: usize = 32;
pub type CommandProducer = Producer<'static, RtCommand>;
pub type CommandConsumer = Consumer<'static, RtCommand>;

#[derive(Clone, Copy, Debug, Default)]
pub struct Record {
    pub index: u32,
    pub n: u8,
    pub values: [f32; MAX_SOURCES],
}

pub const RECORD_QUEUE_LEN: usize = 256;
pub type RecordProducer = Producer<'static, Record>;
pub type RecordConsumer = Consumer<'static, Record>;

pub static LOOP_TIME_LAST_US: AtomicU32 = AtomicU32::new(0);
pub static LOOP_TIME_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static OVERRUNS: AtomicU32 = AtomicU32::new(0);
pub static CLOCK_JITTER_US: AtomicU32 = AtomicU32::new(0);
pub static TICK_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub static RECORDS_DROPPED: AtomicU32 = AtomicU32::new(0);
pub static TICKS: AtomicU32 = AtomicU32::new(0);

// Phase-resolved timing diagnostics. The wake phase is measured against the
// hardware sample clock (CONVST PWM counter), so it separates "the tick body
// started late" from "the tick body ran long"; the sub-phase maxima separate
// the two SPI transactions from the arithmetic between them.
pub static WAKE_PHASE_MIN_US: AtomicU32 = AtomicU32::new(u32::MAX);
pub static WAKE_PHASE_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static T_MEASURE_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static T_ACTUATE_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static T_REST_MAX_US: AtomicU32 = AtomicU32::new(0);

/// Reset the resettable timing diagnostics (maxima and event counters) so a
/// test condition can be measured from a clean slate. Total counters such as
/// `TICKS` are deliberately left running. Safe to call from core 0.
pub fn reset_diagnostics() {
    LOOP_TIME_MAX_US.store(0, Ordering::Relaxed);
    CLOCK_JITTER_US.store(0, Ordering::Relaxed);
    OVERRUNS.store(0, Ordering::Relaxed);
    TICK_TIMEOUTS.store(0, Ordering::Relaxed);
    RECORDS_DROPPED.store(0, Ordering::Relaxed);
    WAKE_PHASE_MIN_US.store(u32::MAX, Ordering::Relaxed);
    WAKE_PHASE_MAX_US.store(0, Ordering::Relaxed);
    T_MEASURE_MAX_US.store(0, Ordering::Relaxed);
    T_ACTUATE_MAX_US.store(0, Ordering::Relaxed);
    T_REST_MAX_US.store(0, Ordering::Relaxed);
}

static SIN_LUT: StaticCell<SinLut> = StaticCell::new();

/// Raw microsecond timestamp (TIMER0 low word). Wraps every ~71.6 minutes
/// and is only ever used in wrapping differences. Reading the register
/// directly keeps flash-resident embassy-time code off the tick path.
#[inline(always)]
fn now_us() -> u32 {
    pac::TIMER0.timerawl().read()
}

#[cfg_attr(feature = "diag-rt-sram", unsafe(link_section = ".data.ram_func"))]
#[allow(clippy::too_many_arguments)]
fn run_rt_tick<R: Rig>(
    rig: &mut R,
    controller: &mut R::Ctrl,
    sample_rate: SampleRate,
    dt: f32,
    commands: &mut CommandConsumer,
    records: &mut RecordProducer,
    lut: &SinLut,
    phase: &mut PhaseAccumulator,
    target_coeffs: &mut FourierCoeffs<HARMONICS>,
    forcing_coeffs: &mut FourierCoeffs<HARMONICS>,
    table_player: &mut TablePlayer,
    active_table: &mut &'static WaveTable,
    index: &mut u32,
    last_tick: &mut Option<u32>,
    n_inputs: usize,
    n_telemetry: usize,
    n_sources: usize,
) {
    #[cfg(feature = "diag-skip-record-enqueue")]
    let _ = &mut *records;
    #[cfg(feature = "diag-skip-record-enqueue")]
    let _ = n_sources;

    if let Some(phase) = rig.tick_phase_us() {
        WAKE_PHASE_MAX_US.fetch_max(phase, Ordering::Relaxed);
        WAKE_PHASE_MIN_US.fetch_min(phase, Ordering::Relaxed);
    }
    let t0 = now_us();
    rig.tick_start();

    if let Some(last) = *last_tick {
        let spacing = t0.wrapping_sub(last);
        let nominal = sample_rate.period_us() as u32;
        if spacing > nominal {
            CLOCK_JITTER_US.fetch_max(spacing - nominal, Ordering::Relaxed);
        }
    }
    *last_tick = Some(t0);

    while let Some(command) = commands.dequeue() {
        match command {
            RtCommand::SetIncrement(increment) => phase.set_increment(increment),
            RtCommand::SetTargetCoeffs(coeffs) => *target_coeffs = coeffs,
            RtCommand::SetForcingCoeffs(coeffs) => *forcing_coeffs = coeffs,
            RtCommand::SetTableIncrement(increment) => table_player.set_increment(increment),
            RtCommand::SetTableGain(gain) => table_player.set_gain(gain),
            RtCommand::SetTableMode(mode) => table_player.set_mode(mode),
            RtCommand::SetTableMultiplier(multiplier) => table_player.set_multiplier(multiplier),
            RtCommand::SetTablePhase(offset) => table_player.set_phase_offset(offset),
            RtCommand::TriggerTable => table_player.trigger(),
            RtCommand::UseTable(buffer) => *active_table = table::activate(buffer),
            RtCommand::ResetController => controller.reset(),
            RtCommand::SetCtrlParam(id, value) => controller.set_param(id, value),
            RtCommand::SetRigParam(id, value) => rig.set_param(id, value),
        }
    }

    let mut values = [0.0; MAX_SOURCES];
    let m0 = now_us();
    rig.measure(&mut values[..n_inputs]);
    let measure_us = now_us().wrapping_sub(m0);
    let (theta, period_start) = phase.step();
    let target = target_coeffs.evaluate(lut, theta);
    let forcing = forcing_coeffs.evaluate(lut, theta);
    let controller_out = controller.tick(&values[..n_inputs], target, dt);
    let table_out = table_player.step(active_table, theta, period_start);
    let out = controller_out + forcing + table_out;
    let a0 = now_us();
    rig.actuate(out);
    let actuate_us = now_us().wrapping_sub(a0);

    controller.telemetry(&mut values[n_inputs..n_inputs + n_telemetry]);
    let generated = n_inputs + n_telemetry;
    values[generated] = target;
    values[generated + 1] = forcing;
    values[generated + 2] = table_out;
    values[generated + 3] = out;
    #[cfg(feature = "diag-skip-record-enqueue")]
    let _ = &values;

    #[cfg(not(feature = "diag-skip-record-enqueue"))]
    {
        if records
            .enqueue(Record {
                index: *index,
                n: n_sources as u8,
                values,
            })
            .is_err()
        {
            RECORDS_DROPPED.fetch_add(1, Ordering::Relaxed);
        }
    }
    *index = (*index).wrapping_add(1);
    rig.tick_end();

    let elapsed = now_us().wrapping_sub(t0);
    T_MEASURE_MAX_US.fetch_max(measure_us, Ordering::Relaxed);
    T_ACTUATE_MAX_US.fetch_max(actuate_us, Ordering::Relaxed);
    T_REST_MAX_US.fetch_max(
        elapsed
            .saturating_sub(measure_us)
            .saturating_sub(actuate_us),
        Ordering::Relaxed,
    );
    LOOP_TIME_LAST_US.store(elapsed, Ordering::Relaxed);
    LOOP_TIME_MAX_US.fetch_max(elapsed, Ordering::Relaxed);
    if elapsed > sample_rate.period_us() as u32 {
        OVERRUNS.fetch_add(1, Ordering::Relaxed);
    }
    TICKS.fetch_add(1, Ordering::Relaxed);
}

pub async fn run_rt_loop<R: Rig>(
    mut rig: R,
    mut tick: R::Tick,
    mut controller: R::Ctrl,
    sample_rate: SampleRate,
    mut commands: CommandConsumer,
    mut records: RecordProducer,
) -> ! {
    let n_inputs = R::INPUTS.len();
    let n_telemetry = R::Ctrl::TELEMETRY.len();
    let n_sources = source_count::<R>();
    assert!(n_sources <= MAX_SOURCES);

    rig.init();
    let lut = SIN_LUT.init(SinLut::new());
    let mut phase = PhaseAccumulator::new();
    let mut target_coeffs = FourierCoeffs::<HARMONICS>::zero();
    let mut forcing_coeffs = FourierCoeffs::<HARMONICS>::zero();
    let mut table_player = TablePlayer::new();
    let mut active_table = table::active();
    let dt = sample_rate.dt();
    let mut index = 0u32;
    let mut last_tick: Option<u32> = None;

    info!(
        "core 1: RT loop running at {} Hz, {} harmonics, {} sources",
        sample_rate.hz(),
        HARMONICS,
        n_sources
    );

    loop {
        tick.wait().await;
        run_rt_tick::<R>(
            &mut rig,
            &mut controller,
            sample_rate,
            dt,
            &mut commands,
            &mut records,
            lut,
            &mut phase,
            &mut target_coeffs,
            &mut forcing_coeffs,
            &mut table_player,
            &mut active_table,
            &mut index,
            &mut last_tick,
            n_inputs,
            n_telemetry,
            n_sources,
        );
    }
}

/// Synchronous variant of [`run_rt_loop`] for a core dedicated to the
/// real-time loop, with no executor running at all.
///
/// Placed in SRAM together with a [`SyncTickSource`] and SRAM SPI transfers
/// (see `analog_spi`), the entire per-tick instruction stream then executes
/// from SRAM: core-0 traffic cannot delay the tick through the shared XIP
/// cache, and no cross-core critical section or timer-queue operation is
/// taken between ticks. Phase-resolved diagnostics on the async version
/// measured every tick phase (SPI, arithmetic and wake-up alike) stretching
/// roughly tenfold under core-0 TCP traffic, which this removes.
#[unsafe(link_section = ".data.ram_func")]
pub fn run_rt_loop_sync<R: Rig, T: SyncTickSource>(
    mut rig: R,
    mut tick: T,
    mut controller: R::Ctrl,
    sample_rate: SampleRate,
    mut commands: CommandConsumer,
    mut records: RecordProducer,
) -> ! {
    let n_inputs = R::INPUTS.len();
    let n_telemetry = R::Ctrl::TELEMETRY.len();
    let n_sources = source_count::<R>();
    assert!(n_sources <= MAX_SOURCES);

    rig.init();
    let lut = SIN_LUT.init(SinLut::new());
    let mut phase = PhaseAccumulator::new();
    let mut target_coeffs = FourierCoeffs::<HARMONICS>::zero();
    let mut forcing_coeffs = FourierCoeffs::<HARMONICS>::zero();
    let mut table_player = TablePlayer::new();
    let mut active_table = table::active();
    let dt = sample_rate.dt();
    let mut index = 0u32;
    let mut last_tick: Option<u32> = None;

    info!(
        "core 1: synchronous RT loop running at {} Hz, {} harmonics, {} sources",
        sample_rate.hz(),
        HARMONICS,
        n_sources
    );

    loop {
        if !tick.wait() {
            TICK_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        }
        run_rt_tick::<R>(
            &mut rig,
            &mut controller,
            sample_rate,
            dt,
            &mut commands,
            &mut records,
            lut,
            &mut phase,
            &mut target_coeffs,
            &mut forcing_coeffs,
            &mut table_player,
            &mut active_table,
            &mut index,
            &mut last_tick,
            n_inputs,
            n_telemetry,
            n_sources,
        );
    }
}

pub async fn status_run() -> ! {
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        ticker.next().await;
        info!(
            "ticks {} | loop {}/{} us | jitter {} us | overruns {} | tick timeouts {} | dropped {}",
            TICKS.load(Ordering::Relaxed),
            LOOP_TIME_LAST_US.load(Ordering::Relaxed),
            LOOP_TIME_MAX_US.load(Ordering::Relaxed),
            CLOCK_JITTER_US.load(Ordering::Relaxed),
            OVERRUNS.load(Ordering::Relaxed),
            TICK_TIMEOUTS.load(Ordering::Relaxed),
            RECORDS_DROPPED.load(Ordering::Relaxed),
        );
    }
}
