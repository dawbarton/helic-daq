//! Generic real-time loop, cross-core mailboxes, records and diagnostics.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::info;
use embassy_time::{Duration, Instant, Ticker};
use heapless::spsc::{Consumer, Producer};
use helic_core::controller::Controller;
use helic_core::generator::FourierCoeffs;
use helic_core::lut::SinLut;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableMode, TablePlayer};
use static_cell::StaticCell;

use crate::rig::{source_count, Rig, TickSource, MAX_SOURCES};
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

static SIN_LUT: StaticCell<SinLut> = StaticCell::new();

pub async fn run_rt_loop<R: Rig>(
    mut rig: R,
    mut tick: R::Tick,
    mut controller: R::Ctrl,
    sample_rate: SampleRate,
    mut commands: CommandConsumer,
    mut records: RecordProducer,
) -> ! {
    #[cfg(feature = "diag-skip-record-enqueue")]
    let _ = &mut records;

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
    let mut last_tick: Option<Instant> = None;

    info!(
        "core 1: RT loop running at {} Hz, {} harmonics, {} sources",
        sample_rate.hz(),
        HARMONICS,
        n_sources
    );

    loop {
        tick.wait().await;
        let t0 = Instant::now();
        rig.tick_start();

        if let Some(last) = last_tick {
            let spacing = (t0 - last).as_micros() as u32;
            let nominal = sample_rate.period_us() as u32;
            if spacing > nominal {
                CLOCK_JITTER_US.fetch_max(spacing - nominal, Ordering::Relaxed);
            }
        }
        last_tick = Some(t0);

        while let Some(command) = commands.dequeue() {
            match command {
                RtCommand::SetIncrement(increment) => phase.set_increment(increment),
                RtCommand::SetTargetCoeffs(coeffs) => target_coeffs = coeffs,
                RtCommand::SetForcingCoeffs(coeffs) => forcing_coeffs = coeffs,
                RtCommand::SetTableIncrement(increment) => table_player.set_increment(increment),
                RtCommand::SetTableGain(gain) => table_player.set_gain(gain),
                RtCommand::SetTableMode(mode) => table_player.set_mode(mode),
                RtCommand::SetTableMultiplier(multiplier) => {
                    table_player.set_multiplier(multiplier)
                }
                RtCommand::SetTablePhase(offset) => table_player.set_phase_offset(offset),
                RtCommand::TriggerTable => table_player.trigger(),
                RtCommand::UseTable(buffer) => active_table = table::activate(buffer),
                RtCommand::ResetController => controller.reset(),
                RtCommand::SetCtrlParam(id, value) => controller.set_param(id, value),
                RtCommand::SetRigParam(id, value) => rig.set_param(id, value),
            }
        }

        let mut values = [0.0; MAX_SOURCES];
        rig.measure(&mut values[..n_inputs]);
        let (theta, period_start) = phase.step();
        let target = target_coeffs.evaluate(lut, theta);
        let forcing = forcing_coeffs.evaluate(lut, theta);
        let controller_out = controller.tick(&values[..n_inputs], target, dt);
        let table_out = table_player.step(active_table, theta, period_start);
        let out = controller_out + forcing + table_out;
        rig.actuate(out);

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
                    index,
                    n: n_sources as u8,
                    values,
                })
                .is_err()
            {
                RECORDS_DROPPED.fetch_add(1, Ordering::Relaxed);
            }
        }
        index = index.wrapping_add(1);
        rig.tick_end();

        let elapsed = t0.elapsed().as_micros() as u32;
        LOOP_TIME_LAST_US.store(elapsed, Ordering::Relaxed);
        LOOP_TIME_MAX_US.fetch_max(elapsed, Ordering::Relaxed);
        if elapsed > sample_rate.period_us() as u32 {
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        }
        TICKS.fetch_add(1, Ordering::Relaxed);
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
