//! Generic real-time loop, cross-core mailboxes, records and diagnostics.

use core::sync::atomic::{AtomicU32, Ordering};

use defmt::info;
use embassy_rp::pac;
use embassy_time::{Duration, Ticker};
use heapless::spsc::{Consumer, Producer, Queue};
use helic_core::controller::Controller;
use helic_core::generator::FourierCoeffs;
use helic_core::lut::SinLut;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableInterpolation, TableMode, TablePlayer, WaveTable};
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
    SetTableInterpolation(TableInterpolation),
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
/// Maximum number of host commands applied at one sample boundary.
///
/// A finite queue alone is not a useful 125 µs WCET bound: draining a burst of
/// coefficient sets could consume the whole period. Remaining commands stay in
/// FIFO order for the next tick and are observable through the backlog maximum.
pub const COMMANDS_PER_TICK: usize = 2;
/// Mask for a command epoch that remains exactly representable as `f32`.
const COMMAND_EPOCH_MASK: u32 = (1 << 24) - 1;
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

static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

/// The four uniquely owned queue endpoints connecting the two cores.
pub struct RtChannels {
    pub command_tx: CommandProducer,
    pub command_rx: CommandConsumer,
    pub record_tx: RecordProducer,
    pub record_rx: RecordConsumer,
}

/// Initialise the platform's single pair of cross-core queues.
///
/// Keeping storage here makes capacities and direction part of the reusable
/// runtime. The returned SPSC endpoint types still make it impossible for an
/// experiment to use one producer or consumer from both cores.
pub fn init_channels() -> RtChannels {
    let (command_tx, command_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (record_tx, record_rx) = RECORD_QUEUE.init(Queue::new()).split();
    RtChannels {
        command_tx,
        command_rx,
        record_tx,
        record_rx,
    }
}

pub static LOOP_TIME_LAST_US: AtomicU32 = AtomicU32::new(0);
pub static LOOP_TIME_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static OVERRUNS: AtomicU32 = AtomicU32::new(0);
pub static CLOCK_JITTER_US: AtomicU32 = AtomicU32::new(0);
pub static TICK_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
pub static RECORDS_DROPPED: AtomicU32 = AtomicU32::new(0);
pub static TICKS: AtomicU32 = AtomicU32::new(0);
pub static COMMAND_BACKLOG_MAX: AtomicU32 = AtomicU32::new(0);

// Phase-resolved timing diagnostics. The wake phase is measured against the
// hardware sample clock (CONVST PWM counter), so it separates "the tick body
// started late" from "the tick body ran long"; the sub-phase maxima separate
// the two SPI transactions from the arithmetic between them.
pub static WAKE_PHASE_MIN_US: AtomicU32 = AtomicU32::new(u32::MAX);
pub static WAKE_PHASE_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static T_MEASURE_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static T_ACTUATE_MAX_US: AtomicU32 = AtomicU32::new(0);
pub static T_REST_MAX_US: AtomicU32 = AtomicU32::new(0);

// Output safety state (see `safety_gate`). Only consulted for a rig whose
// `Rig::SAFETY_GATED` is set; on other experiments these stay at their defaults
// and the gate is compiled out. `SAFETY_ARMED` starts 0 so the output is quiet
// after every flash/reset until the host explicitly arms it.
pub static SAFETY_ARMED: AtomicU32 = AtomicU32::new(0);
pub static SAFETY_TRIPPED: AtomicU32 = AtomicU32::new(0);
pub static SAFETY_CLAMP_TICKS: AtomicU32 = AtomicU32::new(0);
pub static SAFETY_QUIET_TICKS: AtomicU32 = AtomicU32::new(0);

/// Arm the output and clear any latched fault trip. Called from core 0 when
/// the host writes the `arm` parameter. The trip is cleared first so that a
/// still-present fault re-latches on the next tick rather than being masked.
pub fn safety_arm() {
    SAFETY_TRIPPED.store(0, Ordering::Relaxed);
    SAFETY_ARMED.store(1, Ordering::Relaxed);
}

/// Disarm the output immediately (quiet the actuator). Called from core 0 on
/// an explicit `arm = 0` and on control-connection loss. The latched trip, if
/// any, is left set so it remains visible until a deliberate re-arm.
pub fn safety_disarm() {
    SAFETY_ARMED.store(0, Ordering::Relaxed);
}

/// Per-tick output safety gate: decide what is actually driven from the
/// summed actuator command. Runs on core 1 inside the tick, before `actuate`,
/// only for a rig with `Rig::SAFETY_GATED` set.
///
/// - a fault reported by the rig latches `SAFETY_TRIPPED`;
/// - while tripped or disarmed the actuator is held at the rig's safe output;
/// - otherwise the command is passed through the rig's hard clamp.
#[unsafe(link_section = ".data.ram_func")]
#[inline]
fn safety_gate<R: Rig>(rig: &mut R, inputs: &[f32], out_cmd: f32) -> f32 {
    if rig.output_fault(inputs) {
        SAFETY_TRIPPED.store(1, Ordering::Relaxed);
    }
    let tripped = SAFETY_TRIPPED.load(Ordering::Relaxed) != 0;
    let armed = SAFETY_ARMED.load(Ordering::Relaxed) != 0;
    if tripped || !armed {
        SAFETY_QUIET_TICKS.fetch_add(1, Ordering::Relaxed);
        rig.safe_output()
    } else {
        let applied = rig.clamp_output(out_cmd);
        if applied != out_cmd {
            SAFETY_CLAMP_TICKS.fetch_add(1, Ordering::Relaxed);
        }
        applied
    }
}

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
    COMMAND_BACKLOG_MAX.store(0, Ordering::Relaxed);
    // Safety-event tick counters are resettable diagnostics; the armed and
    // latched-trip states are deliberately left untouched by a diag reset.
    SAFETY_CLAMP_TICKS.store(0, Ordering::Relaxed);
    SAFETY_QUIET_TICKS.store(0, Ordering::Relaxed);
}

static SIN_LUT: StaticCell<SinLut> = StaticCell::new();

/// Raw microsecond timestamp (TIMER0 low word). Wraps every ~71.6 minutes
/// and is only ever used in wrapping differences. Reading the register
/// directly keeps flash-resident embassy-time code off the tick path.
#[inline(always)]
fn now_us() -> u32 {
    pac::TIMER0.timerawl().read()
}

#[unsafe(link_section = ".data.ram_func")]
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
    command_epoch: &mut u32,
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

    let mut commands_applied = 0;
    for _ in 0..COMMANDS_PER_TICK {
        let Some(command) = commands.dequeue() else {
            break;
        };
        commands_applied += 1;
        match command {
            RtCommand::SetIncrement(increment) => phase.set_increment(increment),
            RtCommand::SetTargetCoeffs(coeffs) => *target_coeffs = coeffs,
            RtCommand::SetForcingCoeffs(coeffs) => *forcing_coeffs = coeffs,
            RtCommand::SetTableIncrement(increment) => table_player.set_increment(increment),
            RtCommand::SetTableGain(gain) => table_player.set_gain(gain),
            RtCommand::SetTableInterpolation(interpolation) => {
                table_player.set_interpolation(interpolation)
            }
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
    if commands_applied != 0 {
        // Avoid an atomic read-modify-write on every quiet tick. On the rare
        // command tick, applied + remaining reconstructs the queue depth at
        // the boundary while the fixed loop above still bounds the work.
        let backlog = commands_applied + commands.len();
        COMMAND_BACKLOG_MAX.fetch_max(backlog as u32, Ordering::Relaxed);
        // Every value through 2^24 - 1 is exactly representable in the f32
        // stream. Wrapping there preserves exact modular deltas indefinitely.
        *command_epoch =
            (*command_epoch).wrapping_add(commands_applied as u32) & COMMAND_EPOCH_MASK;
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
    let out_cmd = controller_out + forcing + table_out;
    // Hard output safety stage. For a non-gated rig this is a compile-time
    // no-op (the const is false), so the summed command is applied verbatim.
    let out = if R::SAFETY_GATED {
        safety_gate::<R>(rig, &values[..n_inputs], out_cmd)
    } else {
        out_cmd
    };
    let a0 = now_us();
    rig.actuate(out);
    let actuate_us = now_us().wrapping_sub(a0);

    controller.telemetry(&mut values[n_inputs..n_inputs + n_telemetry]);
    let generated = n_inputs + n_telemetry;
    values[generated] = target;
    values[generated + 1] = forcing;
    values[generated + 2] = table_out;
    values[generated + 3] = out;
    values[generated + 4] = *command_epoch as f32;
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

struct RtLoopState<R: Rig, T: TickSource> {
    rig: R,
    tick: T,
    controller: R::Ctrl,
    sample_rate: SampleRate,
    dt: f32,
    commands: CommandConsumer,
    records: RecordProducer,
    lut: &'static SinLut,
    phase: PhaseAccumulator,
    target_coeffs: FourierCoeffs<HARMONICS>,
    forcing_coeffs: FourierCoeffs<HARMONICS>,
    command_epoch: u32,
    table_player: TablePlayer,
    active_table: &'static WaveTable,
    index: u32,
    last_tick: Option<u32>,
    n_inputs: usize,
    n_telemetry: usize,
    n_sources: usize,
}

/// Perform all fallible, logging, and Embassy-dependent setup in flash before
/// entering the SRAM hot loop. Keeping this boundary explicit makes it harder
/// for future initialisation work to become reachable from a sample tick.
pub fn run_rt_loop<R: Rig, T: TickSource>(
    mut rig: R,
    tick: T,
    controller: R::Ctrl,
    sample_rate: SampleRate,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    let n_inputs = R::INPUTS.len();
    let n_telemetry = R::Ctrl::TELEMETRY.len();
    let n_sources = source_count::<R>();
    assert!(n_sources <= MAX_SOURCES);

    rig.init();
    let lut = SIN_LUT.init(SinLut::new());
    info!(
        "core 1: SRAM RT loop running at {} Hz, {} harmonics, {} sources",
        sample_rate.hz(),
        HARMONICS,
        n_sources
    );

    run_hot_loop(RtLoopState {
        rig,
        tick,
        controller,
        sample_rate,
        dt: sample_rate.dt(),
        commands,
        records,
        lut,
        phase: PhaseAccumulator::new(),
        target_coeffs: FourierCoeffs::zero(),
        forcing_coeffs: FourierCoeffs::zero(),
        command_epoch: 0,
        table_player: TablePlayer::new(),
        active_table: table::active(),
        index: 0,
        last_tick: None,
        n_inputs,
        n_telemetry,
        n_sources,
    })
}

/// The only infinite core-1 loop. Everything it calls per tick must remain in
/// SRAM and must not use Embassy, logging, allocation, or critical sections.
#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn run_hot_loop<R: Rig, T: TickSource>(mut state: RtLoopState<R, T>) -> ! {
    loop {
        if !state.tick.wait() {
            TICK_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        }
        run_rt_tick::<R>(
            &mut state.rig,
            &mut state.controller,
            state.sample_rate,
            state.dt,
            &mut state.commands,
            &mut state.records,
            state.lut,
            &mut state.phase,
            &mut state.target_coeffs,
            &mut state.forcing_coeffs,
            &mut state.command_epoch,
            &mut state.table_player,
            &mut state.active_table,
            &mut state.index,
            &mut state.last_tick,
            state.n_inputs,
            state.n_telemetry,
            state.n_sources,
        );
    }
}

pub async fn status_run() -> ! {
    let mut ticker = Ticker::every(Duration::from_secs(1));
    loop {
        ticker.next().await;
        info!(
            "ticks {} | loop {}/{} us | jitter {} us | overruns {} | tick timeouts {} | dropped {} | cmd backlog {} | armed {} tripped {} clamp {} quiet {}",
            TICKS.load(Ordering::Relaxed),
            LOOP_TIME_LAST_US.load(Ordering::Relaxed),
            LOOP_TIME_MAX_US.load(Ordering::Relaxed),
            CLOCK_JITTER_US.load(Ordering::Relaxed),
            OVERRUNS.load(Ordering::Relaxed),
            TICK_TIMEOUTS.load(Ordering::Relaxed),
            RECORDS_DROPPED.load(Ordering::Relaxed),
            COMMAND_BACKLOG_MAX.load(Ordering::Relaxed),
            SAFETY_ARMED.load(Ordering::Relaxed),
            SAFETY_TRIPPED.load(Ordering::Relaxed),
            SAFETY_CLAMP_TICKS.load(Ordering::Relaxed),
            SAFETY_QUIET_TICKS.load(Ordering::Relaxed),
        );
    }
}
