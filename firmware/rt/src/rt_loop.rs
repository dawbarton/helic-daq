//! Generic SRAM-resident real-time loop and cross-core queue storage.

use core::sync::atomic::Ordering;

use defmt::info;
use embassy_rp::pac;
use heapless::spsc::Queue;
use helic_core::controller::Controller;
use helic_core::generator::FourierCoeffs;
use helic_core::lut::SinLut;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::TablePlayer;
use helic_core::ActiveTable;
use helic_rt::{
    command_id, source_count, CommandConsumer, Payload, Record, RecordProducer, Rig, RtChannels,
    RtCommand, RtShared, SampleRate, TickSource, COMMANDS_PER_TICK, COMMAND_QUEUE_LEN,
    DOMAIN_CONTROLLER, DOMAIN_GENERATOR, DOMAIN_RIG, DOMAIN_TABLE, HARMONICS, MAX_SOURCES,
    RECORD_QUEUE_LEN,
};
use static_cell::StaticCell;

/// Mask for a command epoch that remains exactly representable as `f32`.
const COMMAND_EPOCH_MASK: u32 = (1 << 24) - 1;

static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();

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

/// Per-tick output safety gate: decide what is actually driven from the
/// summed actuator command. Runs on core 1 inside the tick, before `actuate`,
/// only for a rig with `Rig::SAFETY_GATED` set.
///
/// - a fault reported by the rig latches `SAFETY_TRIPPED`;
/// - while tripped or disarmed the actuator is held at the rig's safe output;
/// - otherwise the command is passed through the rig's hard clamp.
#[unsafe(link_section = ".data.ram_func")]
#[inline]
fn safety_gate<R: Rig>(rig: &mut R, shared: &RtShared, inputs: &[f32], out_cmd: f32) -> f32 {
    if rig.output_fault(inputs) {
        shared.safety.latch_trip();
    }
    let safety = shared.safety.load_inputs();
    if safety.tripped || !safety.armed {
        shared
            .diagnostics
            .safety_quiet_ticks
            .fetch_add(1, Ordering::Relaxed);
        rig.safe_output()
    } else {
        let applied = rig.clamp_output(out_cmd);
        if applied != out_cmd {
            shared
                .diagnostics
                .safety_clamp_ticks
                .fetch_add(1, Ordering::Relaxed);
        }
        applied
    }
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
    shared: &RtShared,
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
    active_table: &mut ActiveTable,
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
        shared
            .diagnostics
            .wake_phase_max_us
            .fetch_max(phase, Ordering::Relaxed);
        shared
            .diagnostics
            .wake_phase_min_us
            .fetch_min(phase, Ordering::Relaxed);
    }
    let t0 = now_us();
    rig.tick_start();

    if let Some(last) = *last_tick {
        let spacing = t0.wrapping_sub(last);
        let nominal = sample_rate.period_us() as u32;
        if spacing > nominal {
            shared
                .diagnostics
                .clock_jitter_us
                .fetch_max(spacing - nominal, Ordering::Relaxed);
        }
    }
    *last_tick = Some(t0);

    let mut commands_applied = 0;
    for _ in 0..COMMANDS_PER_TICK {
        let Some(command) = commands.dequeue() else {
            break;
        };
        commands_applied += 1;
        match (command.domain, command.id, command.payload) {
            (DOMAIN_GENERATOR, command_id::generator::SET_INCREMENT, Payload::U32(increment)) => {
                phase.set_increment(increment)
            }
            (DOMAIN_GENERATOR, command_id::generator::SET_TARGET, payload) => {
                if let Some(coeffs) = coeffs_from_payload(payload) {
                    *target_coeffs = coeffs;
                }
            }
            (DOMAIN_GENERATOR, command_id::generator::SET_FORCING, payload) => {
                if let Some(coeffs) = coeffs_from_payload(payload) {
                    *forcing_coeffs = coeffs;
                }
            }
            #[cfg(feature = "diag-max-command-burst")]
            (
                DOMAIN_GENERATOR,
                command_id::generator::DIAGNOSTIC_VALUES,
                Payload::Values { len, data },
            ) => {
                debug_assert_eq!(len as usize, 1 + 2 * HARMONICS);
                // Force every byte of the inline payload to materialise. This
                // models installing a complete copied force vector without
                // adding arithmetic that would inflate the copy WCET.
                core::hint::black_box(data);
            }
            (DOMAIN_TABLE, command_id::table::SET_INCREMENT, Payload::U32(increment)) => {
                table_player.set_increment(increment)
            }
            (DOMAIN_TABLE, command_id::table::SET_GAIN, Payload::F32(gain)) => {
                table_player.set_gain(gain)
            }
            (DOMAIN_TABLE, command_id::table::SET_INTERPOLATION, Payload::U32(value)) => {
                if let Some(interpolation) = helic_core::table::TableInterpolation::from_u32(value)
                {
                    table_player.set_interpolation(interpolation);
                }
            }
            (DOMAIN_TABLE, command_id::table::SET_MODE, Payload::U32(value)) => {
                if let Some(mode) = helic_core::table::TableMode::from_u32(value) {
                    table_player.set_mode(mode);
                }
            }
            (DOMAIN_TABLE, command_id::table::SET_MULTIPLIER, Payload::U32(multiplier)) => {
                table_player.set_multiplier(multiplier)
            }
            (DOMAIN_TABLE, command_id::table::SET_PHASE, Payload::U32(offset)) => {
                table_player.set_phase_offset(offset)
            }
            (DOMAIN_TABLE, command_id::table::TRIGGER, Payload::Unit) => table_player.trigger(),
            (DOMAIN_TABLE, command_id::table::ACTIVATE, Payload::Buffer(token)) => {
                active_table.activate(token);
                shared
                    .live
                    .active_table_len
                    .store(active_table.get().len() as u32, Ordering::Relaxed);
            }
            (DOMAIN_CONTROLLER, command_id::controller::RESET, Payload::Unit) => controller.reset(),
            (DOMAIN_CONTROLLER, id, Payload::F32(value)) => controller.set_param(id, value),
            (DOMAIN_RIG, id, Payload::F32(value)) => rig.set_param(id, value),
            _ => {}
        }
    }
    if commands_applied != 0 {
        // Avoid an atomic read-modify-write on every quiet tick. On the rare
        // command tick, applied + remaining reconstructs the queue depth at
        // the boundary while the fixed loop above still bounds the work.
        let backlog = commands_applied + commands.len();
        shared
            .diagnostics
            .command_backlog_max
            .fetch_max(backlog as u32, Ordering::Relaxed);
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
    let table_out = table_player.step(active_table.get(), theta, period_start);
    let out_cmd = controller_out + forcing + table_out;
    // Hard output safety stage. For a non-gated rig this is a compile-time
    // no-op (the const is false), so the summed command is applied verbatim.
    let out = if R::SAFETY_GATED {
        safety_gate::<R>(rig, shared, &values[..n_inputs], out_cmd)
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
            shared
                .diagnostics
                .records_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    *index = (*index).wrapping_add(1);
    rig.tick_end();

    let elapsed = now_us().wrapping_sub(t0);
    shared
        .diagnostics
        .t_measure_max_us
        .fetch_max(measure_us, Ordering::Relaxed);
    shared
        .diagnostics
        .t_actuate_max_us
        .fetch_max(actuate_us, Ordering::Relaxed);
    shared.diagnostics.t_rest_max_us.fetch_max(
        elapsed
            .saturating_sub(measure_us)
            .saturating_sub(actuate_us),
        Ordering::Relaxed,
    );
    shared
        .live
        .loop_time_last_us
        .store(elapsed, Ordering::Relaxed);
    shared
        .diagnostics
        .loop_time_max_us
        .fetch_max(elapsed, Ordering::Relaxed);
    if elapsed > sample_rate.period_us() as u32 {
        shared.diagnostics.overruns.fetch_add(1, Ordering::Relaxed);
    }
    shared.live.ticks.fetch_add(1, Ordering::Relaxed);
}

#[unsafe(link_section = ".data.ram_func")]
fn coeffs_from_payload(payload: Payload) -> Option<FourierCoeffs<HARMONICS>> {
    let Payload::Values { len, data } = payload else {
        return None;
    };
    if len as usize != 1 + 2 * HARMONICS {
        return None;
    }
    let mut coeffs = FourierCoeffs::zero();
    coeffs.mean = data[0];
    coeffs.a.copy_from_slice(&data[1..1 + HARMONICS]);
    coeffs
        .b
        .copy_from_slice(&data[1 + HARMONICS..1 + 2 * HARMONICS]);
    Some(coeffs)
}

/// Run one bounded, experiment-specific output-quiescence step.
///
/// The stable source name is also checked in every production ELF by the
/// real-time layout gate, because a network-triggered reboot must not make
/// core 1 execute flash-resident code while network traffic occupies XIP.
#[unsafe(link_section = ".data.ram_func")]
#[inline]
fn reboot_quiesce_step<R: Rig>(rig: &mut R, step: u8) -> bool {
    rig.prepare_reboot(step)
}

#[unsafe(link_section = ".data.ram_func")]
#[unsafe(export_name = "helic_run_reboot_quiesce")]
#[inline(never)]
fn run_reboot_quiesce(shared: &RtShared) -> ! {
    shared.reboot.mark_quiesced();
    loop {
        core::hint::spin_loop();
    }
}

struct RtLoopState<R: Rig, T: TickSource> {
    rig: R,
    shared: &'static RtShared,
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
    active_table: ActiveTable,
    index: u32,
    last_tick: Option<u32>,
    n_inputs: usize,
    n_telemetry: usize,
    n_sources: usize,
}

/// Perform all fallible, logging, and Embassy-dependent setup in flash before
/// entering the SRAM hot loop. Keeping this boundary explicit makes it harder
/// for future initialisation work to become reachable from a sample tick.
#[allow(clippy::too_many_arguments)]
pub fn run_rt_loop<R: Rig, T: TickSource>(
    mut rig: R,
    tick: T,
    controller: R::Ctrl,
    sample_rate: SampleRate,
    shared: &'static RtShared,
    commands: CommandConsumer,
    records: RecordProducer,
    active_table: ActiveTable,
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
        shared,
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
        active_table,
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
    let mut reboot_step = 0;
    loop {
        if !state.tick.wait() {
            state
                .shared
                .diagnostics
                .tick_timeouts
                .fetch_add(1, Ordering::Relaxed);
        }
        if state.shared.reboot.is_requested() {
            if reboot_quiesce_step(&mut state.rig, reboot_step) {
                run_reboot_quiesce(state.shared);
            }
            reboot_step = reboot_step.saturating_add(1);
            continue;
        }
        run_rt_tick::<R>(
            &mut state.rig,
            state.shared,
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
