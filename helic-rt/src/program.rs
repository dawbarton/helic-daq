//! Statically dispatched real-time programmes and the standard SISO composition.

use core::sync::atomic::Ordering;

use helic_core::controller::Controller;
use helic_core::lut::SinLut;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableInterpolation, TableMode, TablePlayer};
use helic_core::MAX_TABLE_LEN;

use crate::{
    command_id, ActiveCoeffs, ActiveTable, Payload, RtShared, SampleRate, DEFAULT_HARMONICS,
    DOMAIN_CONTROLLER, DOMAIN_GENERATOR, DOMAIN_TABLE, MAX_HARMONICS,
};

/// Immutable services supplied to one programme step by the loop driver.
pub struct StepCtx<'a> {
    pub lut: &'a SinLut,
    pub sample_rate: SampleRate,
}

/// Statically selected computation between rig measurement and actuation.
pub trait Program {
    const OUTPUTS: usize;
    const INPUTS_REQUIRED: usize;
    const DOMAINS: &'static [u8];
    const SIGNALS: &'static [(&'static str, &'static str)];

    fn apply(&mut self, domain: u8, id: u16, payload: Payload);
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx<'_>, outputs: &mut [f32]);
    fn write_signals(&self, out: &mut [f32]);

    /// Number of programme-owned stream signals.
    fn signal_count() -> usize
    where
        Self: Sized,
    {
        Self::SIGNALS.len()
    }

    /// Discover one programme-owned stream signal.
    fn signal(index: usize) -> Option<(&'static str, &'static str)>
    where
        Self: Sized,
    {
        Self::SIGNALS.get(index).copied()
    }

    #[inline(always)]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn fault(&self) -> bool {
        false
    }
}

const STANDARD_SIGNALS: &[(&str, &str)] = &[
    ("target", "V"),
    ("forcing", "V"),
    ("table", "V"),
    ("phase", "turn"),
];

#[inline(always)]
fn phase_turns(phase: u32) -> f32 {
    phase as f32 * (1.0 / 4_294_967_296.0)
}

/// Current controller, Fourier generator, and waveform-table programme.
pub struct StandardProgram<
    C: Controller,
    const H: usize = DEFAULT_HARMONICS,
    const N: usize = MAX_TABLE_LEN,
> {
    controller: C,
    phase: PhaseAccumulator,
    target_coeffs: ActiveCoeffs<H>,
    forcing_coeffs: ActiveCoeffs<H>,
    table_player: TablePlayer,
    active_table: ActiveTable<N>,
    shared: &'static RtShared,
    target: f32,
    forcing: f32,
    table: f32,
    phase_turns: f32,
}

impl<C: Controller, const H: usize, const N: usize> StandardProgram<C, H, N> {
    pub fn new(
        controller: C,
        target_coeffs: ActiveCoeffs<H>,
        forcing_coeffs: ActiveCoeffs<H>,
        active_table: ActiveTable<N>,
        shared: &'static RtShared,
    ) -> Self {
        assert!(H <= MAX_HARMONICS);
        Self {
            controller,
            phase: PhaseAccumulator::new(),
            target_coeffs,
            forcing_coeffs,
            table_player: TablePlayer::new(),
            active_table,
            shared,
            target: 0.0,
            forcing: 0.0,
            table: 0.0,
            phase_turns: 0.0,
        }
    }
}

impl<C: Controller, const H: usize, const N: usize> Program for StandardProgram<C, H, N> {
    const OUTPUTS: usize = 1;
    const INPUTS_REQUIRED: usize = 0;
    const DOMAINS: &'static [u8] = &[DOMAIN_GENERATOR, DOMAIN_TABLE, DOMAIN_CONTROLLER];
    // Controller telemetry is prepended by `signal` and `write_signals`; this
    // slice names the fixed part shared by every controller selection.
    const SIGNALS: &'static [(&'static str, &'static str)] = STANDARD_SIGNALS;

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply(&mut self, domain: u8, id: u16, payload: Payload) {
        match (domain, id, payload) {
            (DOMAIN_GENERATOR, command_id::generator::SET_INCREMENT, Payload::U32(increment)) => {
                self.phase.set_increment(increment)
            }
            (DOMAIN_GENERATOR, command_id::generator::SET_TARGET, Payload::Buffer(token)) => {
                self.target_coeffs.activate(token);
            }
            (DOMAIN_GENERATOR, command_id::generator::SET_FORCING, Payload::Buffer(token)) => {
                self.forcing_coeffs.activate(token);
            }
            #[cfg(feature = "diag-max-command-burst")]
            (
                DOMAIN_GENERATOR,
                command_id::generator::DIAGNOSTIC_VALUES,
                Payload::Values { len, data },
            ) => {
                debug_assert_eq!(len as usize, 1 + 2 * H);
                core::hint::black_box(data);
            }
            (DOMAIN_TABLE, command_id::table::SET_INCREMENT, Payload::U32(increment)) => {
                self.table_player.set_increment(increment)
            }
            (DOMAIN_TABLE, command_id::table::SET_GAIN, Payload::F32(gain)) => {
                self.table_player.set_gain(gain)
            }
            (DOMAIN_TABLE, command_id::table::SET_INTERPOLATION, Payload::U32(value)) => {
                if let Some(interpolation) = TableInterpolation::from_u32(value) {
                    self.table_player.set_interpolation(interpolation);
                }
            }
            (DOMAIN_TABLE, command_id::table::SET_MODE, Payload::U32(value)) => {
                if let Some(mode) = TableMode::from_u32(value) {
                    self.table_player.set_mode(mode);
                }
            }
            (DOMAIN_TABLE, command_id::table::SET_MULTIPLIER, Payload::U32(multiplier)) => {
                self.table_player.set_multiplier(multiplier)
            }
            (DOMAIN_TABLE, command_id::table::SET_PHASE, Payload::U32(offset)) => {
                self.table_player.set_phase_offset(offset)
            }
            (DOMAIN_TABLE, command_id::table::TRIGGER, Payload::Unit) => {
                self.table_player.trigger()
            }
            (DOMAIN_TABLE, command_id::table::ACTIVATE, Payload::Buffer(token)) => {
                self.active_table.activate(token);
                self.shared
                    .live
                    .active_table_len
                    .store(self.active_table.get().len() as u32, Ordering::Relaxed);
            }
            (DOMAIN_CONTROLLER, command_id::controller::RESET, Payload::Unit) => {
                self.controller.reset()
            }
            (DOMAIN_CONTROLLER, id, Payload::F32(value)) if id != command_id::controller::RESET => {
                self.controller.set_param(id - 1, value)
            }
            _ => {}
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(&mut self, inputs: &[f32], dt: f32, ctx: &StepCtx<'_>, outputs: &mut [f32]) {
        debug_assert!(!outputs.is_empty());
        let (phase, period_start) = self.phase.step();
        self.target = self.target_coeffs.get().evaluate(ctx.lut, phase);
        self.forcing = self.forcing_coeffs.get().evaluate(ctx.lut, phase);
        let controller = self.controller.tick(inputs, self.target, dt);
        self.table = self
            .table_player
            .step(self.active_table.get(), phase, period_start);
        self.phase_turns = phase_turns(phase);
        outputs[0] = controller + self.forcing + self.table;
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn write_signals(&self, out: &mut [f32]) {
        let telemetry = C::TELEMETRY.len();
        debug_assert!(out.len() >= telemetry + STANDARD_SIGNALS.len());
        self.controller.telemetry(&mut out[..telemetry]);
        out[telemetry] = self.target;
        out[telemetry + 1] = self.forcing;
        out[telemetry + 2] = self.table;
        out[telemetry + 3] = self.phase_turns;
    }

    fn signal_count() -> usize {
        C::TELEMETRY.len() + STANDARD_SIGNALS.len()
    }

    fn signal(index: usize) -> Option<(&'static str, &'static str)> {
        C::TELEMETRY
            .get(index)
            .or_else(|| STANDARD_SIGNALS.get(index.checked_sub(C::TELEMETRY.len())?))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use std::boxed::Box;

    use helic_core::generator::FourierCoeffs;
    use helic_core::{DoubleBuffer, TableBuffer};

    use super::*;
    const TEST_HARMONICS: usize = 4;

    #[derive(Default)]
    struct TestController {
        observed: f32,
    }

    impl Controller for TestController {
        fn tick(&mut self, inputs: &[f32], reference: f32, _dt: f32) -> f32 {
            self.observed = inputs[0];
            reference
        }

        const TELEMETRY: &'static [(&'static str, &'static str)] = &[("observed", "V")];

        fn telemetry(&self, out: &mut [f32]) {
            out[0] = self.observed;
        }
    }

    fn program() -> (
        StandardProgram<TestController, TEST_HARMONICS, 8>,
        crate::CoeffStaging<TEST_HARMONICS>,
        crate::CoeffStaging<TEST_HARMONICS>,
        helic_core::Staging<helic_core::WaveTable<8>>,
        &'static RtShared,
    ) {
        let (target_staging, target_active) = Box::leak(Box::new(DoubleBuffer::from_banks(
            FourierCoeffs::<TEST_HARMONICS>::zero(),
            FourierCoeffs::<TEST_HARMONICS>::zero(),
        )))
        .split();
        let (forcing_staging, forcing_active) = Box::leak(Box::new(DoubleBuffer::from_banks(
            FourierCoeffs::<TEST_HARMONICS>::zero(),
            FourierCoeffs::<TEST_HARMONICS>::zero(),
        )))
        .split();
        let (table_staging, active_table) = Box::leak(Box::new(TableBuffer::<8>::new())).split();
        let shared = Box::leak(Box::new(RtShared::new()));
        (
            StandardProgram::new(
                TestController::default(),
                target_active,
                forcing_active,
                active_table,
                shared,
            ),
            target_staging,
            forcing_staging,
            table_staging,
            shared,
        )
    }

    #[test]
    fn standard_program_owns_commands_step_and_signals() {
        let (mut program, mut target, mut forcing, mut table, shared) = program();
        *target.buffer().unwrap() = FourierCoeffs {
            mean: 0.25,
            a: [0.0; TEST_HARMONICS],
            b: [0.0; TEST_HARMONICS],
        };
        let target_token = target.commit().unwrap();
        program.apply(
            DOMAIN_GENERATOR,
            command_id::generator::SET_TARGET,
            Payload::Buffer(target_token),
        );
        *forcing.buffer().unwrap() = FourierCoeffs {
            mean: 0.125,
            a: [0.0; TEST_HARMONICS],
            b: [0.0; TEST_HARMONICS],
        };
        let forcing_token = forcing.commit().unwrap();
        program.apply(
            DOMAIN_GENERATOR,
            command_id::generator::SET_FORCING,
            Payload::Buffer(forcing_token),
        );
        assert!(table.buffer().unwrap().write_block(0, &[1.0, 1.0]));
        assert!(table.buffer().unwrap().set_len(2));
        let table_token = table.commit().unwrap();
        program.apply(
            DOMAIN_TABLE,
            command_id::table::ACTIVATE,
            Payload::Buffer(table_token),
        );
        program.apply(
            DOMAIN_TABLE,
            command_id::table::SET_MODE,
            Payload::U32(TableMode::Loop as u32),
        );
        program.apply(
            DOMAIN_GENERATOR,
            command_id::generator::SET_INCREMENT,
            Payload::U32(1 << 30),
        );

        let lut = SinLut::new();
        let ctx = StepCtx {
            lut: &lut,
            sample_rate: SampleRate::Hz8000,
        };
        let mut output = [0.0];
        program.step(&[2.0], SampleRate::Hz8000.dt(), &ctx, &mut output);
        assert_eq!(output[0], 1.375);
        let mut signals = [0.0; 5];
        program.write_signals(&mut signals);
        assert_eq!(signals, [2.0, 0.25, 0.125, 1.0, 0.25]);
        assert_eq!(shared.live.active_table_len.load(Ordering::Relaxed), 2);
        assert_eq!(
            StandardProgram::<TestController, TEST_HARMONICS, 8>::signal_count(),
            5
        );
        assert_eq!(
            StandardProgram::<TestController, TEST_HARMONICS, 8>::signal(0),
            Some(("observed", "V"))
        );
        assert_eq!(
            StandardProgram::<TestController, TEST_HARMONICS, 8>::signal(4),
            Some(("phase", "turn"))
        );
    }

    #[test]
    fn phase_signal_meets_the_half_f32_ulp_bound() {
        for phase in [0, 1, (1 << 24) - 1, 1 << 30, u32::MAX - 1, u32::MAX] {
            let exact = phase as f64 / 4_294_967_296.0;
            let error = (phase_turns(phase) as f64 - exact).abs();
            assert!(
                error <= 2.0_f64.powi(-25),
                "phase={phase:#x}, error={error}"
            );
        }
    }
}
