//! Reusable parameter groups for the platform and current standard programme.

use core::marker::PhantomData;
use core::sync::atomic::Ordering;

use heapless::Vec;
use helic_core::generator::FourierCoeffs;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableInterpolation, TableMode};
use helic_core::{BufferError, Staging as TableStaging, WaveTable, MAX_TABLE_LEN};
use helic_proto::{ErrorCode, ParamType};

use super::{
    CommandTarget, ExtraParam, ParamAction, ParamDef, ParamGroup, Staged, MAX_CTRL_PARAMS,
    MAX_EXTRA_PARAMS, MAX_RIG_PARAMS,
};
use crate::{
    CoeffStaging, Payload, Rig, RtShared, SampleRate, StandardControl, DEFAULT_HARMONICS,
    DOMAIN_CONTROLLER, DOMAIN_GENERATOR, DOMAIN_TABLE, MAX_HARMONICS,
};

const PLATFORM_PARAMS: &[ParamDef] = &[
    ParamDef::read_only("firmware", ParamType::Char, 16),
    ParamDef::read_only("experiment", ParamType::Char, 16),
    ParamDef::read_only("sample_freq", ParamType::F32, 1),
    ParamDef::read_only("ticks", ParamType::U32, 1),
    ParamDef::read_only("loop_time_last", ParamType::U32, 1),
    ParamDef::read_only("loop_time_max", ParamType::U32, 1),
    ParamDef::read_only("clock_jitter", ParamType::U32, 1),
    ParamDef::read_only("overruns", ParamType::U32, 1),
    ParamDef::read_only("tick_timeouts", ParamType::U32, 1),
    ParamDef::read_only("records_dropped", ParamType::U32, 1),
    ParamDef::read_only("wake_phase_min", ParamType::U32, 1),
    ParamDef::read_only("wake_phase_max", ParamType::U32, 1),
    ParamDef::read_only("t_measure_max", ParamType::U32, 1),
    ParamDef::read_only("t_actuate_max", ParamType::U32, 1),
    ParamDef::read_only("t_rest_max", ParamType::U32, 1),
    ParamDef::writable("diag_reset", ParamType::U32, 1),
    ParamDef::read_only("cmd_backlog_max", ParamType::U32, 1),
    ParamDef::writable("arm", ParamType::U32, 1),
    ParamDef::read_only("safety", ParamType::U32, 1),
    ParamDef::writable("mcu_reboot", ParamType::U32, 1),
    ParamDef::read_only("table_len", ParamType::U16, 1),
];

const PLATFORM_DIAG_RESET: u16 = 15;
const PLATFORM_ARM: u16 = 17;
const PLATFORM_MCU_REBOOT: u16 = 19;
const PLATFORM_TABLE_LEN: u16 = 20;

#[derive(Clone, Copy)]
enum PlatformPending {
    None,
    Arm(bool),
}

/// Core-0-owned identity, timing diagnostics, safety state, and reboot action.
pub struct PlatformGroup {
    shared: &'static RtShared,
    sample_rate: SampleRate,
    firmware_version: &'static str,
    experiment: &'static str,
    pending: PlatformPending,
}

impl PlatformGroup {
    pub const fn new(
        shared: &'static RtShared,
        sample_rate: SampleRate,
        firmware_version: &'static str,
        experiment: &'static str,
    ) -> Self {
        Self {
            shared,
            sample_rate,
            firmware_version,
            experiment,
            pending: PlatformPending::None,
        }
    }
}

impl ParamGroup for PlatformGroup {
    fn params(&self) -> &[ParamDef] {
        PLATFORM_PARAMS
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let size = checked_output(PLATFORM_PARAMS, id, out)?;
        let out = &mut out[..size];
        match id {
            0 => write_string(out, self.firmware_version),
            1 => write_string(out, self.experiment),
            2 => write_u32(out, self.sample_rate.hz().to_bits()),
            3 => write_u32(out, self.shared.live.ticks.load(Ordering::Relaxed)),
            4 => write_u32(
                out,
                self.shared.live.loop_time_last_us.load(Ordering::Relaxed),
            ),
            5 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .loop_time_max_us
                    .load(Ordering::Relaxed),
            ),
            6 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .clock_jitter_us
                    .load(Ordering::Relaxed),
            ),
            7 => write_u32(
                out,
                self.shared.diagnostics.overruns.load(Ordering::Relaxed),
            ),
            8 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .tick_timeouts
                    .load(Ordering::Relaxed),
            ),
            9 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .records_dropped
                    .load(Ordering::Relaxed),
            ),
            10 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .wake_phase_min_us
                    .load(Ordering::Relaxed),
            ),
            11 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .wake_phase_max_us
                    .load(Ordering::Relaxed),
            ),
            12 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .t_measure_max_us
                    .load(Ordering::Relaxed),
            ),
            13 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .t_actuate_max_us
                    .load(Ordering::Relaxed),
            ),
            14 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .t_rest_max_us
                    .load(Ordering::Relaxed),
            ),
            PLATFORM_DIAG_RESET | PLATFORM_MCU_REBOOT => write_u32(out, 0),
            16 => write_u32(
                out,
                self.shared
                    .diagnostics
                    .command_backlog_max
                    .load(Ordering::Relaxed),
            ),
            PLATFORM_ARM => write_u32(out, self.shared.safety.load_inputs().armed as u32),
            18 => write_u32(out, self.shared.safety.flags(&self.shared.diagnostics)),
            PLATFORM_TABLE_LEN => out.copy_from_slice(
                &(self.shared.live.active_table_len.load(Ordering::Relaxed) as u16).to_le_bytes(),
            ),
            _ => return Err(ErrorCode::BadIndex),
        }
        Ok(size)
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        let value = read_u32(data)?;
        match id {
            PLATFORM_DIAG_RESET => Ok(Staged::Local(if value == 0 {
                ParamAction::None
            } else {
                ParamAction::ResetDiagnostics
            })),
            PLATFORM_ARM => {
                self.pending = PlatformPending::Arm(value != 0);
                Ok(Staged::Local(ParamAction::None))
            }
            PLATFORM_MCU_REBOOT if value == helic_proto::MCU_REBOOT_CONFIRMATION => {
                Ok(Staged::Local(ParamAction::Reboot))
            }
            PLATFORM_MCU_REBOOT => Err(ErrorCode::BadValue),
            _ => Err(ErrorCode::BadIndex),
        }
    }

    fn accept(&mut self, _id: u16) {
        match core::mem::replace(&mut self.pending, PlatformPending::None) {
            PlatformPending::None => {}
            PlatformPending::Arm(true) => self.shared.safety.arm(),
            PlatformPending::Arm(false) => self.shared.safety.disarm(),
        }
    }

    fn reject(&mut self, _id: u16, _returned: Option<Payload>) {
        self.pending = PlatformPending::None;
    }

    fn reset_diagnostics(&mut self) {
        self.shared.diagnostics.reset();
    }
}

const fn generator_params<const H: usize>() -> [ParamDef; 3] {
    [
        ParamDef::writable("freq", ParamType::F32, 1),
        ParamDef::writable("target_coeffs", ParamType::F32, (1 + 2 * H) as u16),
        ParamDef::writable("forcing_coeffs", ParamType::F32, (1 + 2 * H) as u16),
    ]
}

#[derive(Clone, Copy)]
enum GeneratorPending<const H: usize> {
    None,
    Frequency(f32),
    Target(FourierCoeffs<H>),
    Forcing(FourierCoeffs<H>),
}

/// Standard Fourier-generator parameter shadow and coefficient staging banks.
pub struct GeneratorGroup<const H: usize = DEFAULT_HARMONICS> {
    target_buffer: CoeffStaging<H>,
    forcing_buffer: CoeffStaging<H>,
    defs: [ParamDef; 3],
    sample_rate: SampleRate,
    frequency: f32,
    target: FourierCoeffs<H>,
    forcing: FourierCoeffs<H>,
    forcing_amplitude_limit: f32,
    pending: GeneratorPending<H>,
}

impl<const H: usize> GeneratorGroup<H> {
    pub const fn new(
        target_buffer: CoeffStaging<H>,
        forcing_buffer: CoeffStaging<H>,
        sample_rate: SampleRate,
    ) -> Self {
        assert!(H <= MAX_HARMONICS);
        Self {
            target_buffer,
            forcing_buffer,
            defs: generator_params::<H>(),
            sample_rate,
            frequency: 0.0,
            target: FourierCoeffs::zero(),
            forcing: FourierCoeffs::zero(),
            forcing_amplitude_limit: f32::MAX,
            pending: GeneratorPending::None,
        }
    }

    /// Reject forcing coefficients whose conservative absolute bound exceeds
    /// the rig's independently established output window.
    pub fn set_forcing_amplitude_limit(&mut self, limit: f32) {
        assert!(limit.is_finite() && limit >= 0.0);
        self.forcing_amplitude_limit = limit;
    }
}

impl<const H: usize> ParamGroup for GeneratorGroup<H> {
    fn target(&self) -> CommandTarget {
        CommandTarget::Program(DOMAIN_GENERATOR)
    }

    fn params(&self) -> &[ParamDef] {
        &self.defs
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let size = checked_output(&self.defs, id, out)?;
        let out = &mut out[..size];
        match id {
            0 => out.copy_from_slice(&self.frequency.to_le_bytes()),
            1 => serialize_coeffs(&self.target, out),
            2 => serialize_coeffs(&self.forcing, out),
            _ => return Err(ErrorCode::BadIndex),
        }
        Ok(size)
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        match id {
            0 => {
                let frequency = read_f32(data)?;
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&frequency) {
                    return Err(ErrorCode::BadValue);
                }
                self.pending = GeneratorPending::Frequency(frequency);
                Ok(Staged::Rt(Payload::U32(PhaseAccumulator::increment_for(
                    frequency as f64,
                    self.sample_rate.hz() as f64,
                ))))
            }
            1 | 2 => {
                let coefficients = deserialize_coeffs(data)?;
                if id == 2 && coefficients.amplitude_bound() > self.forcing_amplitude_limit {
                    return Err(ErrorCode::BadValue);
                }
                let buffer = if id == 1 {
                    &mut self.target_buffer
                } else {
                    &mut self.forcing_buffer
                };
                *buffer.buffer().map_err(map_buffer_error)? = coefficients;
                let token = buffer.commit().map_err(map_buffer_error)?;
                self.pending = if id == 1 {
                    GeneratorPending::Target(coefficients)
                } else {
                    GeneratorPending::Forcing(coefficients)
                };
                Ok(Staged::Rt(Payload::Buffer(token)))
            }
            _ => Err(ErrorCode::BadIndex),
        }
    }

    fn accept(&mut self, _id: u16) {
        match core::mem::replace(&mut self.pending, GeneratorPending::None) {
            GeneratorPending::None => {}
            GeneratorPending::Frequency(value) => self.frequency = value,
            GeneratorPending::Target(value) => self.target = value,
            GeneratorPending::Forcing(value) => self.forcing = value,
        }
    }

    fn reject(&mut self, id: u16, returned: Option<Payload>) {
        if let Some(Payload::Buffer(token)) = returned {
            if id == 1 {
                self.target_buffer.cancel(token);
            } else if id == 2 {
                self.forcing_buffer.cancel(token);
            }
        }
        self.pending = GeneratorPending::None;
    }
}

const fn table_params<const N: usize>() -> [ParamDef; 8] {
    [
        ParamDef::blob("table", ParamType::F32, N as u16, N as u32),
        ParamDef::writable("table_freq", ParamType::F32, 1),
        ParamDef::writable("table_gain", ParamType::F32, 1),
        ParamDef::writable("table_interp", ParamType::U32, 1),
        ParamDef::writable("table_mode", ParamType::U32, 1),
        ParamDef::writable("table_mult", ParamType::U32, 1),
        ParamDef::writable("table_phase", ParamType::F32, 1),
        ParamDef::writable("table_trigger", ParamType::U32, 1),
    ]
}

#[derive(Clone, Copy)]
enum TablePending {
    None,
    Frequency(f32),
    Gain(f32),
    Interpolation(u32),
    Mode(u32),
    Multiplier(u32),
    Phase(f32),
}

/// Waveform-table upload state and scalar playback shadow.
pub struct TableGroup<const N: usize = MAX_TABLE_LEN> {
    staging: TableStaging<WaveTable<N>>,
    defs: [ParamDef; 8],
    sample_rate: SampleRate,
    frequency: f32,
    gain: f32,
    interpolation: u32,
    mode: u32,
    multiplier: u32,
    phase: f32,
    pending: TablePending,
}

impl<const N: usize> TableGroup<N> {
    pub const fn new(staging: TableStaging<WaveTable<N>>, sample_rate: SampleRate) -> Self {
        Self {
            staging,
            defs: table_params::<N>(),
            sample_rate,
            frequency: 0.0,
            gain: 1.0,
            interpolation: TableInterpolation::Linear as u32,
            mode: 0,
            multiplier: 1,
            phase: 0.0,
            pending: TablePending::None,
        }
    }
}

impl<const N: usize> ParamGroup for TableGroup<N> {
    fn target(&self) -> CommandTarget {
        CommandTarget::Program(DOMAIN_TABLE)
    }

    fn params(&self) -> &[ParamDef] {
        &self.defs
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let size = checked_output(&self.defs, id, out)?;
        let out = &mut out[..size];
        match id {
            0 => return Err(ErrorCode::BadLength),
            1 => out.copy_from_slice(&self.frequency.to_le_bytes()),
            2 => out.copy_from_slice(&self.gain.to_le_bytes()),
            3 => write_u32(out, self.interpolation),
            4 => write_u32(out, self.mode),
            5 => write_u32(out, self.multiplier),
            6 => out.copy_from_slice(&self.phase.to_le_bytes()),
            7 => write_u32(out, 0),
            _ => return Err(ErrorCode::BadIndex),
        }
        Ok(size)
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        match id {
            0 => Err(ErrorCode::BadLength),
            1 => {
                let frequency = read_f32(data)?;
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&frequency) {
                    return Err(ErrorCode::BadValue);
                }
                self.pending = TablePending::Frequency(frequency);
                Ok(Staged::Rt(Payload::U32(PhaseAccumulator::increment_for(
                    frequency as f64,
                    self.sample_rate.hz() as f64,
                ))))
            }
            2 => {
                let gain = read_f32(data)?;
                if !gain.is_finite() {
                    return Err(ErrorCode::BadValue);
                }
                self.pending = TablePending::Gain(gain);
                Ok(Staged::Rt(Payload::F32(gain)))
            }
            3 => {
                let interpolation = read_u32(data)?;
                TableInterpolation::from_u32(interpolation).ok_or(ErrorCode::BadValue)?;
                self.pending = TablePending::Interpolation(interpolation);
                Ok(Staged::Rt(Payload::U32(interpolation)))
            }
            4 => {
                let mode = read_u32(data)?;
                TableMode::from_u32(mode).ok_or(ErrorCode::BadValue)?;
                self.pending = TablePending::Mode(mode);
                Ok(Staged::Rt(Payload::U32(mode)))
            }
            5 => {
                let multiplier = read_u32(data)?;
                if multiplier == 0 {
                    return Err(ErrorCode::BadValue);
                }
                self.pending = TablePending::Multiplier(multiplier);
                Ok(Staged::Rt(Payload::U32(multiplier)))
            }
            6 => {
                let phase = read_f32(data)?;
                if !(0.0..1.0).contains(&phase) {
                    return Err(ErrorCode::BadValue);
                }
                self.pending = TablePending::Phase(phase);
                Ok(Staged::Rt(Payload::U32(
                    (phase as f64 * 4_294_967_296.0) as u32,
                )))
            }
            7 => {
                if read_u32(data)? == 0 {
                    Ok(Staged::Local(ParamAction::None))
                } else {
                    Ok(Staged::Rt(Payload::Unit))
                }
            }
            _ => Err(ErrorCode::BadIndex),
        }
    }

    fn accept(&mut self, _id: u16) {
        match core::mem::replace(&mut self.pending, TablePending::None) {
            TablePending::None => {}
            TablePending::Frequency(value) => self.frequency = value,
            TablePending::Gain(value) => self.gain = value,
            TablePending::Interpolation(value) => self.interpolation = value,
            TablePending::Mode(value) => self.mode = value,
            TablePending::Multiplier(value) => self.multiplier = value,
            TablePending::Phase(value) => self.phase = value,
        }
    }

    fn reject(&mut self, id: u16, returned: Option<Payload>) {
        if id == 0 {
            if let Some(Payload::Buffer(token)) = returned {
                self.staging.cancel(token);
            }
        }
        self.pending = TablePending::None;
    }

    fn set_block(&mut self, id: u16, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if id != 0 {
            return Err(ErrorCode::BadIndex);
        }
        if !data.len().is_multiple_of(4) {
            return Err(ErrorCode::BadLength);
        }
        let offset = offset as usize;
        let count = data.len() / 4;
        if offset.checked_add(count).is_none_or(|end| end > N) {
            return Err(ErrorCode::BadLength);
        }
        let staging = self.staging.buffer().map_err(map_buffer_error)?;
        for (index, raw) in data.chunks_exact(4).enumerate() {
            let value = f32::from_le_bytes(raw.try_into().unwrap());
            let written = staging.write_block(offset + index, &[value]);
            debug_assert!(written);
        }
        Ok(())
    }

    fn stage_commit(&mut self, id: u16, len: u32) -> Result<Staged, ErrorCode> {
        if id != 0 {
            return Err(ErrorCode::BadIndex);
        }
        let len = len as usize;
        if !(2..=N).contains(&len) {
            return Err(ErrorCode::BadValue);
        }
        {
            let staging = self.staging.buffer().map_err(map_buffer_error)?;
            if !staging
                .prefix(len)
                .unwrap()
                .iter()
                .all(|value| value.is_finite())
            {
                return Err(ErrorCode::BadValue);
            }
            let length_set = staging.set_len(len);
            debug_assert!(length_set);
        }
        let token = self.staging.commit().map_err(map_buffer_error)?;
        Ok(Staged::Rt(Payload::Buffer(token)))
    }
}

/// Experiment-owned read-only atomics, including diagnostic event counters.
pub struct TelemetryGroup {
    values: &'static [ExtraParam],
    defs: Vec<ParamDef, MAX_EXTRA_PARAMS>,
}

impl TelemetryGroup {
    pub fn new(values: &'static [ExtraParam]) -> Self {
        let mut defs = Vec::new();
        for value in values {
            assert!(
                defs.push(value.def()).is_ok(),
                "too many telemetry parameters"
            );
        }
        Self { values, defs }
    }
}

impl ParamGroup for TelemetryGroup {
    fn params(&self) -> &[ParamDef] {
        &self.defs
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let size = checked_output(&self.defs, id, out)?;
        self.values
            .get(id as usize)
            .ok_or(ErrorCode::BadIndex)?
            .get(&mut out[..size]);
        Ok(size)
    }

    fn stage(&mut self, _id: u16, _data: &[u8]) -> Result<Staged, ErrorCode> {
        Err(ErrorCode::ReadOnly)
    }

    fn accept(&mut self, _id: u16) {}

    fn reject(&mut self, _id: u16, _returned: Option<Payload>) {}

    fn reset_diagnostics(&mut self) {
        for value in self.values {
            value.reset_diagnostic();
        }
    }
}

/// Experiment hardware parameter shadow, addressed to [`Rig::set_param`].
pub struct RigGroup<R: Rig> {
    defs: Vec<ParamDef, MAX_RIG_PARAMS>,
    values: [f32; MAX_RIG_PARAMS],
    pending: Option<(usize, f32)>,
    rig: PhantomData<R>,
}

impl<R: Rig> RigGroup<R> {
    pub fn new() -> Self {
        let names = R::param_names();
        let defaults = R::param_defaults();
        assert!(names.len() <= MAX_RIG_PARAMS, "too many rig parameters");
        assert!(
            defaults.is_empty() || defaults.len() == names.len(),
            "rig parameter defaults must be empty or match names"
        );
        let mut defs = Vec::new();
        for name in names {
            assert!(
                defs.push(ParamDef::writable(name, ParamType::F32, 1))
                    .is_ok(),
                "too many rig parameters"
            );
        }
        let mut values = [0.0; MAX_RIG_PARAMS];
        values[..defaults.len()].copy_from_slice(defaults);
        Self {
            defs,
            values,
            pending: None,
            rig: PhantomData,
        }
    }
}

impl<R: Rig> Default for RigGroup<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Rig> ParamGroup for RigGroup<R> {
    fn target(&self) -> CommandTarget {
        CommandTarget::Rig
    }

    fn params(&self) -> &[ParamDef] {
        &self.defs
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let size = checked_output(&self.defs, id, out)?;
        out[..size].copy_from_slice(
            &self
                .values
                .get(id as usize)
                .ok_or(ErrorCode::BadIndex)?
                .to_le_bytes(),
        );
        Ok(size)
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        let value = R::normalise_param(id, read_f32(data)?).ok_or(ErrorCode::BadValue)?;
        self.pending = Some((id as usize, value));
        Ok(Staged::Rt(Payload::F32(value)))
    }

    fn accept(&mut self, _id: u16) {
        if let Some((id, value)) = self.pending.take() {
            self.values[id] = value;
        }
    }

    fn reject(&mut self, _id: u16, _returned: Option<Payload>) {
        self.pending = None;
    }
}

/// Scalar parameter shadow for a statically selected standard control.
pub struct ScalarControlGroup<C: StandardControl<H>, const H: usize> {
    defs: Vec<ParamDef, MAX_CTRL_PARAMS>,
    values: [f32; MAX_CTRL_PARAMS],
    input_count: usize,
    pending: Option<(usize, f32)>,
    controller: PhantomData<(C, [f32; H])>,
}

impl<C: StandardControl<H>, const H: usize> ScalarControlGroup<C, H> {
    pub fn new(controller: &C, input_count: usize) -> Self {
        assert!(
            C::scalar_param_names().len() <= MAX_CTRL_PARAMS,
            "too many controller parameters"
        );
        let mut defs = Vec::new();
        let mut values = [0.0; MAX_CTRL_PARAMS];
        for (index, name) in C::scalar_param_names().iter().enumerate() {
            defs.push(ParamDef::writable(name, ParamType::F32, 1))
                .ok()
                .unwrap();
            values[index] = controller
                .scalar_param_value(index as u16)
                .expect("controller parameters must report initial values");
        }
        Self {
            defs,
            values,
            input_count,
            pending: None,
            controller: PhantomData,
        }
    }
}

impl<C: StandardControl<H>, const H: usize> ParamGroup for ScalarControlGroup<C, H> {
    fn target(&self) -> CommandTarget {
        CommandTarget::Program(DOMAIN_CONTROLLER)
    }

    fn params(&self) -> &[ParamDef] {
        &self.defs
    }

    fn get(&self, id: u16, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let size = checked_output(&self.defs, id, out)?;
        out[..size].copy_from_slice(
            &self
                .values
                .get(id as usize)
                .ok_or(ErrorCode::BadIndex)?
                .to_le_bytes(),
        );
        Ok(size)
    }

    fn stage(&mut self, id: u16, data: &[u8]) -> Result<Staged, ErrorCode> {
        let value = C::normalise_scalar_param(id, read_f32(data)?, self.input_count)
            .ok_or(ErrorCode::BadValue)?;
        self.pending = Some((id as usize, value));
        Ok(Staged::Rt(Payload::F32(value)))
    }

    fn accept(&mut self, _id: u16) {
        if let Some((id, value)) = self.pending.take() {
            self.values[id] = value;
        }
    }

    fn reject(&mut self, _id: u16, _returned: Option<Payload>) {
        self.pending = None;
    }
}

fn checked_output(defs: &[ParamDef], id: u16, out: &[u8]) -> Result<usize, ErrorCode> {
    let def = defs.get(id as usize).ok_or(ErrorCode::BadIndex)?;
    let size = def.ty.size() * def.count as usize;
    if out.len() < size {
        return Err(ErrorCode::BadLength);
    }
    Ok(size)
}

fn read_f32(data: &[u8]) -> Result<f32, ErrorCode> {
    Ok(f32::from_le_bytes(
        data.try_into().map_err(|_| ErrorCode::BadLength)?,
    ))
}

fn read_u32(data: &[u8]) -> Result<u32, ErrorCode> {
    Ok(u32::from_le_bytes(
        data.try_into().map_err(|_| ErrorCode::BadLength)?,
    ))
}

fn write_u32(out: &mut [u8], value: u32) {
    out.copy_from_slice(&value.to_le_bytes());
}

fn write_string(out: &mut [u8], value: &str) {
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = value.as_bytes().get(index).copied().unwrap_or(0);
    }
}

fn map_buffer_error(error: BufferError) -> ErrorCode {
    match error {
        BufferError::Busy => ErrorCode::Busy,
    }
}

fn serialize_coeffs<const H: usize>(coefficients: &FourierCoeffs<H>, out: &mut [u8]) {
    out[0..4].copy_from_slice(&coefficients.mean.to_le_bytes());
    for harmonic in 0..H {
        out[4 + 4 * harmonic..8 + 4 * harmonic]
            .copy_from_slice(&coefficients.a[harmonic].to_le_bytes());
        let offset = 4 + 4 * (H + harmonic);
        out[offset..offset + 4].copy_from_slice(&coefficients.b[harmonic].to_le_bytes());
    }
}

fn deserialize_coeffs<const H: usize>(data: &[u8]) -> Result<FourierCoeffs<H>, ErrorCode> {
    if data.len() != (1 + 2 * H) * 4 {
        return Err(ErrorCode::BadLength);
    }
    let value =
        |index: usize| f32::from_le_bytes(data[4 * index..4 * index + 4].try_into().unwrap());
    let mut coefficients = FourierCoeffs::zero();
    coefficients.mean = value(0);
    for harmonic in 0..H {
        coefficients.a[harmonic] = value(1 + harmonic);
        coefficients.b[harmonic] = value(1 + H + harmonic);
    }
    if coefficients.mean.is_finite()
        && coefficients.a.iter().all(|value| value.is_finite())
        && coefficients.b.iter().all(|value| value.is_finite())
    {
        Ok(coefficients)
    } else {
        Err(ErrorCode::BadValue)
    }
}
