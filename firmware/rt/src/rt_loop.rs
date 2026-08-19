//! Generic SRAM-resident real-time loop and cross-core queue storage.

use core::sync::atomic::Ordering;

use defmt::info;
use embassy_rp::pac;
use heapless::spsc::Queue;
use helic_core::lut::SinLut;
use helic_core::{DoubleBuffer, FourierCoeffs};
use helic_rt::{
    safety_decide, source_count, CommandConsumer, Payload, Program, Record, RecordProducer, Rig,
    RtChannels, RtCommand, RtShared, SafetyInputs, SampleRate, StepCtx, TickSource,
    COMMANDS_PER_TICK, COMMAND_QUEUE_LEN, DOMAIN_RIG, MAX_ACTUATORS, MAX_SOURCES, RECORD_QUEUE_LEN,
};
use static_cell::StaticCell;

/// Mask for a command epoch that remains exactly representable as `f32`.
const COMMAND_EPOCH_MASK: u32 = (1 << 24) - 1;

static COMMAND_QUEUE: StaticCell<Queue<RtCommand, COMMAND_QUEUE_LEN>> = StaticCell::new();
static RECORD_QUEUE: StaticCell<Queue<Record, RECORD_QUEUE_LEN>> = StaticCell::new();
/// Initialise the platform's single pair of cross-core queues.
///
/// Queue storage is universal; coefficient buffers are supplied by the
/// experiment so its harmonic count determines their SRAM footprint.
pub fn init_channels<const H: usize>(
    target: &'static mut DoubleBuffer<FourierCoeffs<H>>,
    forcing: &'static mut DoubleBuffer<FourierCoeffs<H>>,
) -> RtChannels<H> {
    let (command_tx, command_rx) = COMMAND_QUEUE.init(Queue::new()).split();
    let (record_tx, record_rx) = RECORD_QUEUE.init(Queue::new()).split();
    let (target_staging, target_active) = target.split();
    let (forcing_staging, forcing_active) = forcing.split();
    RtChannels {
        command_tx,
        command_rx,
        record_tx,
        record_rx,
        target_staging,
        target_active,
        forcing_staging,
        forcing_active,
    }
}

/// Per-tick output safety gate: decide what is actually driven from the
/// complete actuator command. Runs on core 1 inside the tick, before `actuate`,
/// only for a rig with `Rig::SAFETY_GATED` set.
///
/// Rig and programme faults, and non-finite commands, latch `SAFETY_TRIPPED`;
/// while tripped or disarmed every actuator is held at its safe output;
/// otherwise each command is passed through its actuator-specific hard clamp.
#[unsafe(link_section = ".data.ram_func")]
#[inline]
fn safety_gate<R: Rig>(
    rig: &mut R,
    shared: &RtShared,
    safety_inputs: SafetyInputs,
    inputs: &[f32],
    program_fault: bool,
    commanded: &[f32],
    applied: &mut [f32],
) {
    let fault = rig.output_fault(inputs) || program_fault;
    let outcome = safety_decide(rig, safety_inputs, fault, commanded, applied);
    if outcome.newly_tripped {
        shared.safety.latch_trip();
    }
    if outcome.quieted {
        shared
            .diagnostics
            .safety_quiet_ticks
            .fetch_add(1, Ordering::Relaxed);
    }
    if outcome.clamped {
        shared
            .diagnostics
            .safety_clamp_ticks
            .fetch_add(1, Ordering::Relaxed);
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
#[inline(never)]
fn apply_program(program: &mut impl Program, domain: u8, id: u16, payload: Payload) {
    program.apply(domain, id, payload);
}

#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn step_program(
    program: &mut impl Program,
    inputs: &[f32],
    output_enabled: bool,
    ctx: &StepCtx<'_>,
    outputs: &mut [f32],
) {
    program.step(inputs, output_enabled, ctx, outputs);
}

#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn write_program_signals(program: &impl Program, out: &mut [f32]) {
    program.write_signals(out);
}

#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn program_fault(program: &impl Program) -> bool {
    core::hint::black_box(program.fault())
}

#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn set_rig_param(rig: &mut impl Rig, id: u16, value: f32) {
    // Black-box the arguments, not the (unit) result: this forces the id/value
    // pair to materialise before the call so its WCET stays representative even
    // for a rig whose set_param the optimiser could otherwise prove trivial.
    // `black_box(())` after the call would not do this - a `()` carries no
    // data, so it cannot pin anything that preceded it.
    core::hint::black_box((id, value));
    rig.set_param(id, value);
}

#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn measure_rig(rig: &mut impl Rig, values: &mut [f32]) {
    rig.measure(values);
}

#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn actuate_rig(rig: &mut impl Rig, outputs: &[f32]) {
    // Black-box the argument, not the (unit) result; see set_rig_param above.
    core::hint::black_box(outputs);
    rig.actuate(outputs);
}

#[unsafe(link_section = ".data.ram_func")]
#[allow(clippy::too_many_arguments)]
fn run_rt_tick<R: Rig>(
    rig: &mut R,
    shared: &RtShared,
    program: &mut impl Program,
    sample_rate: SampleRate,
    commands: &mut CommandConsumer,
    records: &mut RecordProducer,
    ctx: &StepCtx<'_>,
    command_epoch: &mut u32,
    index: &mut u32,
    last_tick: &mut Option<u32>,
    n_inputs: usize,
    n_program_signals: usize,
    n_actuators: usize,
    n_sources: usize,
) {
    #[cfg(feature = "diag-skip-record-enqueue")]
    let _ = &mut *records;
    #[cfg(feature = "diag-skip-record-enqueue")]
    let _ = n_sources;

    // Timestamp immediately after the hardware wake. Keeping the spacing
    // observation ahead of diagnostic atomics avoids making clock jitter
    // depend on their execution time or on where the PWM-to-TIMER phase lands
    // after an otherwise unrelated core-0 layout change.
    let t0 = now_us();
    rig.tick_start();

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
        let RtCommand {
            domain,
            id,
            payload,
        } = command;
        match (domain, payload) {
            (DOMAIN_RIG, Payload::F32(value)) => set_rig_param(rig, id, value),
            (DOMAIN_RIG, _) => {}
            (_, payload) => apply_program(program, domain, id, payload),
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
    measure_rig(rig, &mut values[..n_inputs]);
    let measure_us = now_us().wrapping_sub(m0);
    let safety_inputs = shared.safety.load_inputs();
    let output_enabled = !R::SAFETY_GATED || (safety_inputs.armed && !safety_inputs.tripped);
    let mut commanded = [0.0; MAX_ACTUATORS];
    step_program(
        program,
        &values[..n_inputs],
        output_enabled,
        ctx,
        &mut commanded[..n_actuators],
    );
    let mut applied = [0.0; MAX_ACTUATORS];
    // Hard output safety stage. For a non-gated rig this is a compile-time
    // no-op (the const is false), so every command is applied verbatim.
    if R::SAFETY_GATED {
        safety_gate::<R>(
            rig,
            shared,
            safety_inputs,
            &values[..n_inputs],
            program_fault(program),
            &commanded[..n_actuators],
            &mut applied[..n_actuators],
        );
    } else {
        applied[..n_actuators].copy_from_slice(&commanded[..n_actuators]);
    }
    let a0 = now_us();
    actuate_rig(rig, &applied[..n_actuators]);
    let actuate_us = now_us().wrapping_sub(a0);

    write_program_signals(program, &mut values[n_inputs..n_inputs + n_program_signals]);
    let generated = n_inputs + n_program_signals;
    values[generated..generated + n_actuators].copy_from_slice(&applied[..n_actuators]);
    values[generated + n_actuators] = *command_epoch as f32;
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

struct RtLoopState<R: Rig, P: Program, T: TickSource> {
    rig: R,
    program: P,
    shared: &'static RtShared,
    tick: T,
    sample_rate: SampleRate,
    commands: CommandConsumer,
    records: RecordProducer,
    ctx: StepCtx<'static>,
    command_epoch: u32,
    index: u32,
    last_tick: Option<u32>,
    n_inputs: usize,
    n_program_signals: usize,
    n_actuators: usize,
    n_sources: usize,
}

/// Perform all fallible, logging, and Embassy-dependent setup in flash before
/// entering the SRAM hot loop. Keeping this boundary explicit makes it harder
/// for future initialisation work to become reachable from a sample tick.
#[allow(clippy::too_many_arguments)]
pub fn run_rt_loop<R: Rig, P: Program, T: TickSource>(
    mut rig: R,
    tick: T,
    program: P,
    sample_rate: SampleRate,
    shared: &'static RtShared,
    commands: CommandConsumer,
    records: RecordProducer,
) -> ! {
    let n_inputs = R::INPUTS.len();
    let n_program_signals = P::signal_count();
    let n_actuators = R::ACTUATORS.len();
    let n_sources = source_count::<R, P>();
    assert!(n_sources <= MAX_SOURCES);
    assert_eq!(P::OUTPUTS, n_actuators);
    assert!(n_actuators <= MAX_ACTUATORS);
    assert!(P::INPUTS_REQUIRED <= n_inputs);

    rig.init();
    let lut = SIN_LUT.init(SinLut::new());
    info!(
        "core 1: SRAM RT loop running at {} Hz, {} sources",
        sample_rate.hz(),
        n_sources
    );

    run_hot_loop(RtLoopState {
        rig,
        program,
        shared,
        tick,
        sample_rate,
        commands,
        records,
        ctx: StepCtx { lut, sample_rate },
        command_epoch: 0,
        index: 0,
        last_tick: None,
        n_inputs,
        n_program_signals,
        n_actuators,
        n_sources,
    })
}

/// The only infinite core-1 loop. Everything it calls per tick must remain in
/// SRAM and must not use Embassy, logging, allocation, or critical sections.
#[unsafe(link_section = ".data.ram_func")]
#[inline(never)]
fn run_hot_loop<R: Rig, P: Program, T: TickSource>(mut state: RtLoopState<R, P, T>) -> ! {
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
            &mut state.program,
            state.sample_rate,
            &mut state.commands,
            &mut state.records,
            &state.ctx,
            &mut state.command_epoch,
            &mut state.index,
            &mut state.last_tick,
            state.n_inputs,
            state.n_program_signals,
            state.n_actuators,
            state.n_sources,
        );
    }
}
