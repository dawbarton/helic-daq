//! Statically dispatched real-time programmes and the standard SISO composition.

use core::sync::atomic::Ordering;

use helic_core::lut::SinLut;
use helic_core::table::{TableInterpolation, TableMode, TablePlayer};
use helic_core::{HarmonicGenerator, MAX_TABLE_LEN};

use crate::{
    command_id, ActiveCoeffs, ActiveTable, ControlStep, Payload, RtShared, SampleRate,
    StandardControl, StandardControlInputs, DEFAULT_HARMONICS, DOMAIN_CONTROLLER, DOMAIN_GENERATOR,
    DOMAIN_TABLE, MAX_HARMONICS,
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
    fn step(
        &mut self,
        inputs: &[f32],
        output_enabled: bool,
        ctx: &StepCtx<'_>,
        outputs: &mut [f32],
    );
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

/// Current controller, Fourier generator, and waveform-table programme.
pub struct StandardProgram<
    C: StandardControl<H>,
    const H: usize = DEFAULT_HARMONICS,
    const N: usize = MAX_TABLE_LEN,
> {
    controller: C,
    generator: HarmonicGenerator<H>,
    nominal_increment: u32,
    frequency_overridden: bool,
    target_coeffs: ActiveCoeffs<H>,
    forcing_coeffs: ActiveCoeffs<H>,
    table_player: TablePlayer,
    active_table: ActiveTable<N>,
    shared: &'static RtShared,
    target: f32,
    forcing: f32,
    table: f32,
    phase_turns: f32,
    output_enabled: bool,
}

impl<C: StandardControl<H>, const H: usize, const N: usize> StandardProgram<C, H, N> {
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
            generator: HarmonicGenerator::new(),
            nominal_increment: 0,
            frequency_overridden: false,
            target_coeffs,
            forcing_coeffs,
            table_player: TablePlayer::new(),
            active_table,
            shared,
            target: 0.0,
            forcing: 0.0,
            table: 0.0,
            phase_turns: 0.0,
            output_enabled: false,
        }
    }
}

impl<C: StandardControl<H>, const H: usize, const N: usize> Program for StandardProgram<C, H, N> {
    const OUTPUTS: usize = 1;
    const INPUTS_REQUIRED: usize = C::INPUTS_REQUIRED;
    const DOMAINS: &'static [u8] = &[DOMAIN_GENERATOR, DOMAIN_TABLE, DOMAIN_CONTROLLER];
    const SIGNALS: &'static [(&'static str, &'static str)] = &[
        ("target", C::REFERENCE_UNIT),
        ("forcing", "V"),
        ("table", "V"),
        ("phase", "turn"),
    ];

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn apply(&mut self, domain: u8, id: u16, payload: Payload) {
        match (domain, id, payload) {
            (DOMAIN_GENERATOR, command_id::generator::SET_INCREMENT, Payload::U32(increment)) => {
                self.nominal_increment = increment;
                if !self.frequency_overridden {
                    self.generator.set_increment(increment);
                }
            }
            (DOMAIN_GENERATOR, command_id::generator::SET_TARGET, Payload::Buffer(token)) => {
                self.target_coeffs.activate(token);
            }
            (DOMAIN_GENERATOR, command_id::generator::SET_FORCING, Payload::Buffer(token)) => {
                self.forcing_coeffs.activate(token);
            }
            #[cfg(feature = "diag-max-command-burst")]
            (DOMAIN_GENERATOR, command_id::generator::DIAGNOSTIC_BURST, Payload::F32(v)) => {
                core::hint::black_box(v);
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
            (DOMAIN_CONTROLLER, id, payload) => self.controller.apply(id, payload),
            _ => {}
        }
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn step(
        &mut self,
        inputs: &[f32],
        output_enabled: bool,
        ctx: &StepCtx<'_>,
        outputs: &mut [f32],
    ) {
        debug_assert!(!outputs.is_empty());
        let current_increment = self.generator.increment();
        let frame = self.generator.step(ctx.lut);
        let target_coeffs = self.target_coeffs.get();
        self.target = frame.project(target_coeffs);
        self.forcing = frame.project(self.forcing_coeffs.get());
        self.table =
            self.table_player
                .step(self.active_table.get(), frame.phase, frame.period_start);
        self.phase_turns = frame.phase_turns();

        if output_enabled && !self.output_enabled {
            self.controller.reset();
        }
        self.controller.set_output_enabled(output_enabled);
        self.output_enabled = output_enabled;
        let ControlStep {
            output,
            next_increment,
        } = self.controller.step(
            StandardControlInputs {
                measured: inputs,
                reference: self.target,
                reference_mean: target_coeffs.mean,
                forcing: self.forcing,
                table: self.table,
                frame,
                current_increment,
            },
            ctx,
        );
        self.frequency_overridden = output_enabled && next_increment.is_some();
        self.generator.set_increment(if output_enabled {
            next_increment.unwrap_or(self.nominal_increment)
        } else {
            self.nominal_increment
        });
        outputs[0] = if output_enabled { output } else { 0.0 };
    }

    #[inline]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn write_signals(&self, out: &mut [f32]) {
        let telemetry = C::TELEMETRY.len();
        debug_assert!(out.len() >= telemetry + Self::SIGNALS.len());
        self.controller.telemetry(&mut out[..telemetry]);
        out[telemetry] = self.target;
        out[telemetry + 1] = self.forcing;
        out[telemetry + 2] = self.table;
        out[telemetry + 3] = self.phase_turns;
    }

    fn signal_count() -> usize {
        C::TELEMETRY.len() + Self::SIGNALS.len()
    }

    fn signal(index: usize) -> Option<(&'static str, &'static str)> {
        C::TELEMETRY
            .get(index)
            .or_else(|| Self::SIGNALS.get(index.checked_sub(C::TELEMETRY.len())?))
            .copied()
    }

    #[inline(always)]
    #[cfg_attr(feature = "rt-sram", unsafe(link_section = ".data.ram_func"))]
    fn fault(&self) -> bool {
        self.controller.fault()
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

    impl StandardControl<TEST_HARMONICS> for TestController {
        const TELEMETRY: &'static [(&'static str, &'static str)] = &[("observed", "V")];

        fn step(
            &mut self,
            inputs: StandardControlInputs<'_, TEST_HARMONICS>,
            _ctx: &StepCtx<'_>,
        ) -> ControlStep {
            self.observed = inputs.measured[0];
            ControlStep {
                output: inputs.reference + inputs.forcing + inputs.table,
                next_increment: None,
            }
        }

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
        program.step(&[2.0], true, &ctx, &mut output);
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
            let turns = phase as f32 * (1.0 / 4_294_967_296.0);
            let error = (turns as f64 - exact).abs();
            assert!(
                error <= 2.0_f64.powi(-25),
                "phase={phase:#x}, error={error}"
            );
        }
    }
}
