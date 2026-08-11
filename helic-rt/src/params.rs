//! Name-based, discoverable parameter registry derived from rtc's host
//! interface. Indices are connection-local.
//!
//! The host discovers parameters at connect (`GetParams`)
//! and addresses them by index thereafter. Reads are served from core-0
//! state: diagnostics come from atomics the RT loop maintains, writable
//! values from the shadow copies kept here. Writes update the shadow and
//! translate to an [`RtCommand`], which core 1 applies at a sample boundary
//! — coefficient sets travel by value, so a tick never sees a torn array.

use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

use crate::RtShared;
use helic_core::controller::Controller;
use helic_core::generator::FourierCoeffs;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableInterpolation, TableMode};
use helic_core::{BufferError, Staging as TableStaging, MAX_TABLE_LEN};
use helic_proto::{ErrorCode, ParamType};

use crate::{
    command_id, validate_sources, CommandProducer, Payload, Rig, RtCommand, SampleRate,
    DOMAIN_CONTROLLER, DOMAIN_GENERATOR, DOMAIN_RIG, DOMAIN_TABLE, HARMONICS, MAX_RT_VALUES,
};

mod schema;

pub use schema::BASE_PARAMS;
use schema::*;

/// Serialized size of a coefficient set: mean + a[K] + b[K].
pub const COEFF_COUNT: u16 = (1 + 2 * HARMONICS) as u16;

#[derive(Clone, Copy)]
pub struct ParamDef {
    pub name: &'static str,
    pub ty: ParamType,
    pub count: u16,
    pub writable: bool,
}

impl ParamDef {
    const fn read_only(name: &'static str, ty: ParamType, count: u16) -> Self {
        Self {
            name,
            ty,
            count,
            writable: false,
        }
    }

    const fn writable(name: &'static str, ty: ParamType, count: u16) -> Self {
        Self {
            name,
            ty,
            count,
            writable: true,
        }
    }
}

/// One experiment-owned, read-only scalar backed by an atomic word.
///
/// Separate constructors make it impossible to declare an unsupported size,
/// a writable value without a setter, or a definition whose byte count does
/// not match the storage read by the registry.
#[derive(Clone, Copy)]
pub struct ExtraParam {
    name: &'static str,
    ty: ParamType,
    value: &'static AtomicU32,
    reset_on_diag: bool,
}

impl ExtraParam {
    pub const fn f32(name: &'static str, value: &'static AtomicU32) -> Self {
        Self {
            name,
            ty: ParamType::F32,
            value,
            reset_on_diag: false,
        }
    }

    pub const fn u32(name: &'static str, value: &'static AtomicU32) -> Self {
        Self {
            name,
            ty: ParamType::U32,
            value,
            reset_on_diag: false,
        }
    }

    /// Declare a read-only event counter cleared by `diag_reset`.
    pub const fn u32_event(name: &'static str, value: &'static AtomicU32) -> Self {
        Self {
            name,
            ty: ParamType::U32,
            value,
            reset_on_diag: true,
        }
    }

    const fn def(self) -> ParamDef {
        ParamDef {
            name: self.name,
            ty: self.ty,
            count: 1,
            writable: false,
        }
    }

    fn get(self, out: &mut [u8]) {
        out.copy_from_slice(&self.value.load(Ordering::Relaxed).to_le_bytes());
    }

    fn reset_diagnostic(self) {
        if self.reset_on_diag {
            self.value.store(0, Ordering::Relaxed);
        }
    }
}

pub trait ParamRegistry {
    fn count(&self) -> usize;
    fn def(&self, index: usize) -> Option<ParamDef>;
    fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode>;
    fn set(&mut self, index: usize, data: &[u8]) -> Result<(), ErrorCode>;
    fn set_block(&mut self, _index: usize, _offset: u32, _data: &[u8]) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn commit(&mut self, _index: usize, _len: u32) -> Result<(), ErrorCode> {
        Err(ErrorCode::UnknownType)
    }
    fn sample_rate(&self) -> SampleRate;
}

/// An action which the control server must perform after accepting a write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamAction {
    None,
    Reboot,
}

#[derive(Clone, Copy)]
enum ShadowUpdate {
    None,
    Freq(f32),
    Target(FourierCoeffs<HARMONICS>),
    Forcing(FourierCoeffs<HARMONICS>),
    TableFreq(f32),
    TableGain(f32),
    TableInterpolation(u32),
    TableMode(u32),
    TableMult(u32),
    TablePhase(f32),
    RigParam(usize, f32),
    CtrlParam(usize, f32),
}

/// Registry state: shadow copies of the writable parameters plus the
/// command producer that forwards writes to the RT loop.
pub struct ParamStore<C: Controller, R: Rig> {
    commands: CommandProducer,
    shared: &'static RtShared,
    table: TableStaging,
    sample_rate: SampleRate,
    firmware_version: &'static str,
    experiment: &'static str,
    extras: &'static [ExtraParam],
    freq_hz: f32,
    target: FourierCoeffs<HARMONICS>,
    forcing: FourierCoeffs<HARMONICS>,
    table_freq_hz: f32,
    table_gain: f32,
    table_interpolation: u32,
    table_mode: u32,
    table_mult: u32,
    table_phase: f32,
    rig_params: [f32; MAX_RIG_PARAMS],
    ctrl_params: [f32; MAX_CTRL_PARAMS],
    types: PhantomData<(C, R)>,
}

impl<C: Controller, R: Rig> ParamStore<C, R> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        commands: CommandProducer,
        shared: &'static RtShared,
        table: TableStaging,
        sample_rate: SampleRate,
        firmware_version: &'static str,
        experiment: &'static str,
        extras: &'static [ExtraParam],
        controller: &C,
    ) -> Self {
        assert!(
            Self::rig_names().len() <= MAX_RIG_PARAMS,
            "rig exposes more parameters than ParamStore can shadow"
        );
        assert!(
            Self::ctrl_names().len() <= MAX_CTRL_PARAMS,
            "controller exposes more parameters than ParamStore can shadow"
        );
        assert!(
            extras.len() <= MAX_EXTRA_PARAMS,
            "experiment exposes more extra parameters than supported"
        );
        let mut rig_params = [0.0; MAX_RIG_PARAMS];
        let mut ctrl_params = [0.0; MAX_CTRL_PARAMS];
        let defaults = R::param_defaults();
        assert!(
            defaults.is_empty() || defaults.len() == Self::rig_names().len(),
            "rig parameter defaults must be empty or match param_names"
        );
        rig_params[..defaults.len()].copy_from_slice(defaults);
        for (id, value) in ctrl_params[..Self::ctrl_names().len()]
            .iter_mut()
            .enumerate()
        {
            *value = controller
                .param_value(id as u16)
                .expect("controllers exposing parameters must report their initial values");
        }
        let store = Self {
            commands,
            shared,
            table,
            sample_rate,
            firmware_version,
            experiment,
            extras,
            freq_hz: 0.0,
            target: FourierCoeffs::zero(),
            forcing: FourierCoeffs::zero(),
            table_freq_hz: 0.0,
            table_gain: 1.0,
            table_interpolation: TableInterpolation::Linear as u32,
            table_mode: 0,
            table_mult: 1,
            table_phase: 0.0,
            rig_params,
            ctrl_params,
            types: PhantomData,
        };
        store.validate_registry();
        validate_sources::<R>();
        store
    }

    /// Shared state used by the control server for connection-loss quieting
    /// and the ordered reboot handshake.
    pub fn shared(&self) -> &'static RtShared {
        self.shared
    }

    fn ctrl_names() -> &'static [&'static str] {
        C::param_names()
    }

    fn rig_names() -> &'static [&'static str] {
        R::param_names()
    }

    pub fn count(&self) -> usize {
        BASE_PARAMS.len() + self.extras.len() + Self::rig_names().len() + Self::ctrl_names().len()
    }

    fn validate_registry(&self) {
        assert!(
            self.count() <= u16::MAX as usize,
            "parameter registry exceeds the protocol index range"
        );
        for i in 0..self.count() {
            let def = self.def(i).unwrap();
            let max_name_len =
                if (BASE_PARAMS.len()..BASE_PARAMS.len() + self.extras.len()).contains(&i) {
                    helic_proto::payload::MAX_PARAM_NAME_LEN
                } else {
                    helic_proto::payload::MAX_NAME_LEN
                };
            assert!(
                def.name.len() <= max_name_len && def.name.is_ascii(),
                "parameter name is non-ASCII or exceeds its category limit"
            );
            for j in 0..i {
                assert_ne!(
                    def.name,
                    self.def(j).unwrap().name,
                    "parameter names must be unique"
                );
            }
        }
    }

    /// Definition of parameter `index` (base or controller).
    pub fn def(&self, index: usize) -> Option<ParamDef> {
        if index < BASE_PARAMS.len() {
            Some(BASE_PARAMS[index])
        } else if index < BASE_PARAMS.len() + self.extras.len() {
            Some(self.extras[index - BASE_PARAMS.len()].def())
        } else if index < BASE_PARAMS.len() + self.extras.len() + Self::rig_names().len() {
            Self::rig_names()
                .get(index - BASE_PARAMS.len() - self.extras.len())
                .map(|name| ParamDef {
                    name,
                    ty: ParamType::F32,
                    count: 1,
                    writable: true,
                })
        } else {
            Self::ctrl_names()
                .get(index - BASE_PARAMS.len() - self.extras.len() - Self::rig_names().len())
                .map(|name| ParamDef {
                    name,
                    ty: ParamType::F32,
                    count: 1,
                    writable: true,
                })
        }
    }

    /// Serialize the value of parameter `index` into `out`; returns the
    /// number of bytes written.
    pub fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode> {
        let def = self.def(index).ok_or(ErrorCode::BadIndex)?;
        let size = def.ty.size() * def.count as usize;
        if out.len() < size {
            return Err(ErrorCode::BadLength);
        }
        let out = &mut out[..size];
        match index {
            0 => write_string(out, self.firmware_version),
            1 => write_string(out, self.experiment),
            2 => out.copy_from_slice(&self.sample_rate.hz().to_le_bytes()),
            3 => out.copy_from_slice(&self.shared.live.ticks.load(Ordering::Relaxed).to_le_bytes()),
            4 => out.copy_from_slice(
                &self
                    .shared
                    .live
                    .loop_time_last_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            5 => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .loop_time_max_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            6 => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .clock_jitter_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            7 => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .overruns
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            8 => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .tick_timeouts
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            9 => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .records_dropped
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_FREQ => out.copy_from_slice(&self.freq_hz.to_le_bytes()),
            IDX_TARGET => serialize_coeffs(&self.target, out),
            IDX_FORCING => serialize_coeffs(&self.forcing, out),
            IDX_CTRL_RESET => out.copy_from_slice(&0u32.to_le_bytes()),
            IDX_TABLE => return Err(ErrorCode::BadLength),
            IDX_TABLE_LEN => out.copy_from_slice(
                &(self.shared.live.active_table_len.load(Ordering::Relaxed) as u16).to_le_bytes(),
            ),
            IDX_TABLE_FREQ => out.copy_from_slice(&self.table_freq_hz.to_le_bytes()),
            IDX_TABLE_GAIN => out.copy_from_slice(&self.table_gain.to_le_bytes()),
            IDX_TABLE_INTERPOLATION => out.copy_from_slice(&self.table_interpolation.to_le_bytes()),
            IDX_TABLE_MODE => out.copy_from_slice(&self.table_mode.to_le_bytes()),
            IDX_TABLE_MULT => out.copy_from_slice(&self.table_mult.to_le_bytes()),
            IDX_TABLE_PHASE => out.copy_from_slice(&self.table_phase.to_le_bytes()),
            IDX_TABLE_TRIGGER => out.copy_from_slice(&0u32.to_le_bytes()),
            IDX_WAKE_PHASE_MIN => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .wake_phase_min_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_WAKE_PHASE_MAX => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .wake_phase_max_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_T_MEASURE_MAX => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .t_measure_max_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_T_ACTUATE_MAX => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .t_actuate_max_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_T_REST_MAX => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .t_rest_max_us
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_DIAG_RESET => out.copy_from_slice(&0u32.to_le_bytes()),
            IDX_COMMAND_BACKLOG_MAX => out.copy_from_slice(
                &self
                    .shared
                    .diagnostics
                    .command_backlog_max
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            // `arm` reads back the current shared state; there is no separate
            // core-0 shadow that could disagree with the safety gate.
            IDX_ARM => {
                out.copy_from_slice(&(self.shared.safety.load_inputs().armed as u32).to_le_bytes())
            }
            // `safety` packs the whole gate state into one pollable word:
            // bit0 armed, bit1 latched trip, bit2 clamped since last reset,
            // bit3 quieted since last reset. The exact clamp/quiet tick counts
            // remain in the RT atomics and the status log.
            IDX_SAFETY => {
                let flags = self.shared.safety.flags(&self.shared.diagnostics);
                out.copy_from_slice(&flags.to_le_bytes());
            }
            IDX_MCU_REBOOT => out.copy_from_slice(&0u32.to_le_bytes()),
            i if i < BASE_PARAMS.len() + self.extras.len() => {
                self.extras[i - BASE_PARAMS.len()].get(out)
            }
            i if i < BASE_PARAMS.len() + self.extras.len() + Self::rig_names().len() => out
                .copy_from_slice(
                    &self.rig_params[i - BASE_PARAMS.len() - self.extras.len()].to_le_bytes(),
                ),
            i => out.copy_from_slice(
                &self.ctrl_params
                    [i - BASE_PARAMS.len() - self.extras.len() - Self::rig_names().len()]
                .to_le_bytes(),
            ),
        }
        Ok(size)
    }

    /// Write parameter `index` from raw little-endian bytes and forward the
    /// change to the RT loop.
    pub fn set(&mut self, index: usize, data: &[u8]) -> Result<ParamAction, ErrorCode> {
        let def = self.def(index).ok_or(ErrorCode::BadIndex)?;
        if !def.writable {
            return Err(ErrorCode::ReadOnly);
        }
        if data.len() != def.ty.size() * def.count as usize {
            return Err(ErrorCode::BadLength);
        }
        let (domain, id, payload, shadow) = match index {
            IDX_FREQ => {
                let freq = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&freq) {
                    return Err(ErrorCode::BadValue);
                }
                (
                    DOMAIN_GENERATOR,
                    command_id::generator::SET_INCREMENT,
                    Payload::U32(PhaseAccumulator::increment_for(
                        freq as f64,
                        self.sample_rate.hz() as f64,
                    )),
                    ShadowUpdate::Freq(freq),
                )
            }
            IDX_TARGET => {
                let coeffs = deserialize_coeffs(data)?;
                let payload = coeff_payload(&coeffs);
                (
                    DOMAIN_GENERATOR,
                    command_id::generator::SET_TARGET,
                    payload,
                    ShadowUpdate::Target(coeffs),
                )
            }
            IDX_FORCING => {
                let coeffs = deserialize_coeffs(data)?;
                let payload = coeff_payload(&coeffs);
                (
                    DOMAIN_GENERATOR,
                    command_id::generator::SET_FORCING,
                    payload,
                    ShadowUpdate::Forcing(coeffs),
                )
            }
            IDX_CTRL_RESET => {
                if u32::from_le_bytes(data.try_into().unwrap()) == 0 {
                    return Ok(ParamAction::None);
                }
                (
                    DOMAIN_CONTROLLER,
                    command_id::controller::RESET,
                    Payload::Unit,
                    ShadowUpdate::None,
                )
            }
            IDX_TABLE => return Err(ErrorCode::BadLength),
            IDX_TABLE_FREQ => {
                let freq = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&freq) {
                    return Err(ErrorCode::BadValue);
                }
                (
                    DOMAIN_TABLE,
                    command_id::table::SET_INCREMENT,
                    Payload::U32(PhaseAccumulator::increment_for(
                        freq as f64,
                        self.sample_rate.hz() as f64,
                    )),
                    ShadowUpdate::TableFreq(freq),
                )
            }
            IDX_TABLE_GAIN => {
                let gain = f32::from_le_bytes(data.try_into().unwrap());
                if !gain.is_finite() {
                    return Err(ErrorCode::BadValue);
                }
                (
                    DOMAIN_TABLE,
                    command_id::table::SET_GAIN,
                    Payload::F32(gain),
                    ShadowUpdate::TableGain(gain),
                )
            }
            IDX_TABLE_INTERPOLATION => {
                let interpolation = u32::from_le_bytes(data.try_into().unwrap());
                TableInterpolation::from_u32(interpolation).ok_or(ErrorCode::BadValue)?;
                (
                    DOMAIN_TABLE,
                    command_id::table::SET_INTERPOLATION,
                    Payload::U32(interpolation),
                    ShadowUpdate::TableInterpolation(interpolation),
                )
            }
            IDX_TABLE_MODE => {
                let mode = u32::from_le_bytes(data.try_into().unwrap());
                TableMode::from_u32(mode).ok_or(ErrorCode::BadValue)?;
                (
                    DOMAIN_TABLE,
                    command_id::table::SET_MODE,
                    Payload::U32(mode),
                    ShadowUpdate::TableMode(mode),
                )
            }
            IDX_TABLE_MULT => {
                let multiplier = u32::from_le_bytes(data.try_into().unwrap());
                if multiplier == 0 {
                    return Err(ErrorCode::BadValue);
                }
                (
                    DOMAIN_TABLE,
                    command_id::table::SET_MULTIPLIER,
                    Payload::U32(multiplier),
                    ShadowUpdate::TableMult(multiplier),
                )
            }
            IDX_TABLE_PHASE => {
                let phase = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..1.0).contains(&phase) {
                    return Err(ErrorCode::BadValue);
                }
                let offset = (phase as f64 * 4294967296.0) as u32;
                (
                    DOMAIN_TABLE,
                    command_id::table::SET_PHASE,
                    Payload::U32(offset),
                    ShadowUpdate::TablePhase(phase),
                )
            }
            IDX_TABLE_TRIGGER => {
                if u32::from_le_bytes(data.try_into().unwrap()) == 0 {
                    return Ok(ParamAction::None);
                }
                (
                    DOMAIN_TABLE,
                    command_id::table::TRIGGER,
                    Payload::Unit,
                    ShadowUpdate::None,
                )
            }
            IDX_DIAG_RESET => {
                // Resets are applied directly: the diagnostics are atomics
                // maintained by core 1 but safely writable from here.
                if u32::from_le_bytes(data.try_into().unwrap()) != 0 {
                    self.shared.diagnostics.reset();
                    for extra in self.extras {
                        extra.reset_diagnostic();
                    }
                    #[cfg(feature = "diag-max-command-burst")]
                    self.enqueue_max_command_burst()?;
                }
                return Ok(ParamAction::None);
            }
            IDX_ARM => {
                // Applied directly on core 0 (like `diag_reset`) so the
                // safety-critical disarm path has no command-queue latency.
                if u32::from_le_bytes(data.try_into().unwrap()) != 0 {
                    self.shared.safety.arm();
                } else {
                    self.shared.safety.disarm();
                }
                return Ok(ParamAction::None);
            }
            IDX_MCU_REBOOT => {
                if u32::from_le_bytes(data.try_into().unwrap())
                    != helic_proto::MCU_REBOOT_CONFIRMATION
                {
                    return Err(ErrorCode::BadValue);
                }
                return Ok(ParamAction::Reboot);
            }
            i if (BASE_PARAMS.len() + self.extras.len()
                ..BASE_PARAMS.len() + self.extras.len() + Self::rig_names().len())
                .contains(&i) =>
            {
                let id = (i - BASE_PARAMS.len() - self.extras.len()) as u16;
                let value = f32::from_le_bytes(data.try_into().unwrap());
                let value = R::normalise_param(id, value).ok_or(ErrorCode::BadValue)?;
                (
                    DOMAIN_RIG,
                    id,
                    Payload::F32(value),
                    ShadowUpdate::RigParam(id as usize, value),
                )
            }
            i if (BASE_PARAMS.len() + self.extras.len() + Self::rig_names().len()
                ..self.count())
                .contains(&i) =>
            {
                let id =
                    (i - BASE_PARAMS.len() - self.extras.len() - Self::rig_names().len()) as u16;
                let value = f32::from_le_bytes(data.try_into().unwrap());
                let value =
                    C::normalise_param(id, value, R::INPUTS.len()).ok_or(ErrorCode::BadValue)?;
                (
                    DOMAIN_CONTROLLER,
                    id,
                    Payload::F32(value),
                    ShadowUpdate::CtrlParam(id as usize, value),
                )
            }
            _ => return Err(ErrorCode::BadIndex),
        };
        let command = RtCommand {
            domain,
            id,
            payload,
        };
        if let Err(returned) = self.commands.enqueue(command) {
            self.reject_command(returned);
            return Err(ErrorCode::Busy);
        }
        match shadow {
            ShadowUpdate::None => {}
            ShadowUpdate::Freq(freq) => self.freq_hz = freq,
            ShadowUpdate::Target(coeffs) => self.target = coeffs,
            ShadowUpdate::Forcing(coeffs) => self.forcing = coeffs,
            ShadowUpdate::TableFreq(freq) => self.table_freq_hz = freq,
            ShadowUpdate::TableGain(gain) => self.table_gain = gain,
            ShadowUpdate::TableInterpolation(interpolation) => {
                self.table_interpolation = interpolation
            }
            ShadowUpdate::TableMode(mode) => self.table_mode = mode,
            ShadowUpdate::TableMult(multiplier) => self.table_mult = multiplier,
            ShadowUpdate::TablePhase(phase) => self.table_phase = phase,
            ShadowUpdate::RigParam(id, value) => self.rig_params[id] = value,
            ShadowUpdate::CtrlParam(id, value) => self.ctrl_params[id] = value,
        }
        Ok(ParamAction::None)
    }

    pub fn set_block(&mut self, index: usize, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if index != IDX_TABLE {
            return Err(ErrorCode::BadIndex);
        }
        if !data.len().is_multiple_of(4) {
            return Err(ErrorCode::BadLength);
        }
        let offset = offset as usize;
        let count = data.len() / 4;
        if offset
            .checked_add(count)
            .is_none_or(|end| end > MAX_TABLE_LEN)
        {
            return Err(ErrorCode::BadLength);
        }
        let staging = self.table.buffer().map_err(map_buffer_error)?;
        for (index, raw) in data.chunks_exact(4).enumerate() {
            let value = f32::from_le_bytes(raw.try_into().unwrap());
            let written = staging.write_block(offset + index, &[value]);
            debug_assert!(written);
        }
        Ok(())
    }

    pub fn commit(&mut self, index: usize, len: u32) -> Result<(), ErrorCode> {
        if index != IDX_TABLE {
            return Err(ErrorCode::BadIndex);
        }
        let len = len as usize;
        if !(2..=MAX_TABLE_LEN).contains(&len) {
            return Err(ErrorCode::BadValue);
        }
        {
            let staging = self.table.buffer().map_err(map_buffer_error)?;
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
        let token = self.table.commit().map_err(map_buffer_error)?;
        let command = RtCommand {
            domain: DOMAIN_TABLE,
            id: command_id::table::ACTIVATE,
            payload: Payload::Buffer(token),
        };
        match self.commands.enqueue(command) {
            Ok(()) => {}
            Err(returned) => {
                self.reject_command(returned);
                return Err(ErrorCode::Busy);
            }
        }
        Ok(())
    }

    /// Return ownership-bearing payloads after a command fails to reach core 1.
    ///
    /// Scalar and copied payloads require no unwind. A table activation token
    /// must be cancelled through its unique staging endpoint, or the inactive
    /// bank would remain permanently busy.
    fn reject_command(&mut self, command: RtCommand) {
        if let (DOMAIN_TABLE, command_id::table::ACTIVATE, Payload::Buffer(token)) =
            (command.domain, command.id, command.payload)
        {
            self.table.cancel(token);
        }
    }

    /// Queue the exact two-command WCET case immediately after `diag_reset`.
    ///
    /// This diagnostic-only path changes no registry entry. Checking capacity
    /// before either enqueue makes the pair transactional with respect to the
    /// producer; the concurrent consumer can only increase available space.
    #[cfg(feature = "diag-max-command-burst")]
    fn enqueue_max_command_burst(&mut self) -> Result<(), ErrorCode> {
        if self.commands.capacity() - self.commands.len() < crate::COMMANDS_PER_TICK {
            return Err(ErrorCode::Busy);
        }
        let ids = if cfg!(feature = "diag-wide-command-payload") {
            [
                command_id::generator::DIAGNOSTIC_VALUES,
                command_id::generator::DIAGNOSTIC_VALUES,
            ]
        } else {
            [
                command_id::generator::SET_TARGET,
                command_id::generator::SET_FORCING,
            ]
        };
        for id in ids {
            let result = self.commands.enqueue(RtCommand {
                domain: DOMAIN_GENERATOR,
                id,
                payload: Payload::Values {
                    len: COEFF_COUNT as u8,
                    data: [0.0; MAX_RT_VALUES],
                },
            });
            debug_assert!(result.is_ok());
        }
        Ok(())
    }

    pub const fn sample_rate(&self) -> SampleRate {
        self.sample_rate
    }
}

fn map_buffer_error(error: BufferError) -> ErrorCode {
    match error {
        BufferError::Busy => ErrorCode::Busy,
    }
}

fn coeff_payload(coeffs: &FourierCoeffs<HARMONICS>) -> Payload {
    let mut data = [0.0; MAX_RT_VALUES];
    data[0] = coeffs.mean;
    data[1..1 + HARMONICS].copy_from_slice(&coeffs.a);
    data[1 + HARMONICS..1 + 2 * HARMONICS].copy_from_slice(&coeffs.b);
    Payload::Values {
        len: COEFF_COUNT as u8,
        data,
    }
}

impl<C: Controller, R: Rig> ParamRegistry for ParamStore<C, R> {
    fn count(&self) -> usize {
        ParamStore::count(self)
    }

    fn def(&self, index: usize) -> Option<ParamDef> {
        ParamStore::def(self, index)
    }

    fn get(&self, index: usize, out: &mut [u8]) -> Result<usize, ErrorCode> {
        ParamStore::get(self, index, out)
    }

    fn set(&mut self, index: usize, data: &[u8]) -> Result<(), ErrorCode> {
        ParamStore::set(self, index, data).map(|_| ())
    }

    fn set_block(&mut self, index: usize, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        ParamStore::set_block(self, index, offset, data)
    }

    fn commit(&mut self, index: usize, len: u32) -> Result<(), ErrorCode> {
        ParamStore::commit(self, index, len)
    }

    fn sample_rate(&self) -> SampleRate {
        ParamStore::sample_rate(self)
    }
}

fn write_string(out: &mut [u8], value: &str) {
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = value.as_bytes().get(i).copied().unwrap_or(0);
    }
}

/// Wire layout of a coefficient set: mean, a[1..=K], b[1..=K], all f32 LE.
fn serialize_coeffs(c: &FourierCoeffs<HARMONICS>, out: &mut [u8]) {
    out[0..4].copy_from_slice(&c.mean.to_le_bytes());
    for k in 0..HARMONICS {
        out[4 + 4 * k..8 + 4 * k].copy_from_slice(&c.a[k].to_le_bytes());
        let off = 4 + 4 * (HARMONICS + k);
        out[off..off + 4].copy_from_slice(&c.b[k].to_le_bytes());
    }
}

/// Non-finite coefficients are rejected: a NaN would propagate through the
/// generators to `code_for_volts`, and an infinity pins the output at a rail.
fn deserialize_coeffs(data: &[u8]) -> Result<FourierCoeffs<HARMONICS>, ErrorCode> {
    let f = |i: usize| f32::from_le_bytes(data[4 * i..4 * i + 4].try_into().unwrap());
    let mut c = FourierCoeffs::zero();
    c.mean = f(0);
    for k in 0..HARMONICS {
        c.a[k] = f(1 + k);
        c.b[k] = f(1 + HARMONICS + k);
    }
    let finite = c.mean.is_finite()
        && c.a.iter().all(|v| v.is_finite())
        && c.b.iter().all(|v| v.is_finite());
    if finite {
        Ok(c)
    } else {
        Err(ErrorCode::BadValue)
    }
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicU32;
    use std::boxed::Box;

    use heapless::spsc::Queue;
    use helic_core::controller::PassThrough;
    use helic_core::TableBuffer;

    use super::*;
    use crate::{source, source_count, RtCommand, COMMAND_QUEUE_LEN};

    static EXTRA_VALUE: AtomicU32 = AtomicU32::new(0);
    static EXTRAS: &[ExtraParam] = &[ExtraParam::f32("extra", &EXTRA_VALUE)];

    struct TestRig;

    impl Rig for TestRig {
        const INPUTS: &'static [(&'static str, &'static str)] = &[("adc0", "V")];
        type Ctrl = PassThrough;

        fn init(&mut self) {}
        fn measure(&mut self, _values: &mut [f32]) {}
        fn actuate(&mut self, _out: f32) {}
        fn prepare_reboot(&mut self, _step: u8) -> bool {
            true
        }

        fn param_names() -> &'static [&'static str] {
            &["rig_gain"]
        }

        fn param_defaults() -> &'static [f32] {
            &[1.0]
        }
    }

    fn store_with_table() -> (
        ParamStore<PassThrough, TestRig>,
        crate::CommandConsumer,
        helic_core::ActiveTable,
    ) {
        let queue = Box::leak(Box::new(Queue::<RtCommand, COMMAND_QUEUE_LEN>::new()));
        let (tx, rx) = queue.split();
        let shared = Box::leak(Box::new(RtShared::new()));
        let (table, active) = Box::leak(Box::new(TableBuffer::new())).split();
        (
            ParamStore::new(
                tx,
                shared,
                table,
                SampleRate::Hz8000,
                "0.1.0 test",
                "test-rig",
                EXTRAS,
                &PassThrough,
            ),
            rx,
            active,
        )
    }

    fn store() -> (ParamStore<PassThrough, TestRig>, crate::CommandConsumer) {
        let (store, commands, _active) = store_with_table();
        (store, commands)
    }

    #[test]
    fn platform_registry_is_wire_stable_after_relocation() {
        let expected = [
            ("firmware", ParamType::Char, 16, false),
            ("experiment", ParamType::Char, 16, false),
            ("sample_freq", ParamType::F32, 1, false),
            ("ticks", ParamType::U32, 1, false),
            ("loop_time_last", ParamType::U32, 1, false),
            ("loop_time_max", ParamType::U32, 1, false),
            ("clock_jitter", ParamType::U32, 1, false),
            ("overruns", ParamType::U32, 1, false),
            ("tick_timeouts", ParamType::U32, 1, false),
            ("records_dropped", ParamType::U32, 1, false),
            ("freq", ParamType::F32, 1, true),
            ("target_coeffs", ParamType::F32, COEFF_COUNT, true),
            ("forcing_coeffs", ParamType::F32, COEFF_COUNT, true),
            ("ctrl_reset", ParamType::U32, 1, true),
            (
                "table",
                ParamType::F32,
                helic_core::table::MAX_TABLE_LEN as u16,
                true,
            ),
            ("table_len", ParamType::U16, 1, false),
            ("table_freq", ParamType::F32, 1, true),
            ("table_gain", ParamType::F32, 1, true),
            ("table_interp", ParamType::U32, 1, true),
            ("table_mode", ParamType::U32, 1, true),
            ("table_mult", ParamType::U32, 1, true),
            ("table_phase", ParamType::F32, 1, true),
            ("table_trigger", ParamType::U32, 1, true),
            ("wake_phase_min", ParamType::U32, 1, false),
            ("wake_phase_max", ParamType::U32, 1, false),
            ("t_measure_max", ParamType::U32, 1, false),
            ("t_actuate_max", ParamType::U32, 1, false),
            ("t_rest_max", ParamType::U32, 1, false),
            ("diag_reset", ParamType::U32, 1, true),
            ("cmd_backlog_max", ParamType::U32, 1, false),
            ("arm", ParamType::U32, 1, true),
            ("safety", ParamType::U32, 1, false),
            ("mcu_reboot", ParamType::U32, 1, true),
        ];
        assert_eq!(BASE_PARAMS.len(), expected.len());
        for (definition, expected) in BASE_PARAMS.iter().zip(expected) {
            assert_eq!(
                (
                    definition.name,
                    definition.ty,
                    definition.count,
                    definition.writable,
                ),
                expected
            );
        }
    }

    #[test]
    fn registry_and_sources_preserve_segment_order() {
        let (store, _rx) = store();
        assert_eq!(store.count(), BASE_PARAMS.len() + 2);
        assert_eq!(store.def(BASE_PARAMS.len()).unwrap().name, "extra");
        assert_eq!(store.def(BASE_PARAMS.len() + 1).unwrap().name, "rig_gain");

        let sources: std::vec::Vec<_> = (0..source_count::<TestRig>())
            .map(source::<TestRig>)
            .collect();
        assert_eq!(
            sources,
            [
                Some(("adc0", "V")),
                Some(("target", "V")),
                Some(("forcing", "V")),
                Some(("table", "V")),
                Some(("out", "V")),
                Some(("cmd_epoch", "count")),
            ]
        );
    }

    #[test]
    fn host_tested_store_still_enqueues_sample_boundary_commands() {
        let (mut store, mut rx) = store();
        store.set(IDX_FREQ, &20.0f32.to_le_bytes()).unwrap();
        let expected = PhaseAccumulator::increment_for(20.0, 8000.0);
        assert!(matches!(
            rx.dequeue(),
            Some(RtCommand {
                domain: DOMAIN_GENERATOR,
                id: command_id::generator::SET_INCREMENT,
                payload: Payload::U32(value),
            }) if value == expected
        ));
    }

    #[test]
    fn coefficient_write_uses_bounded_addressed_payload() {
        let (mut store, mut rx) = store();
        let coeffs = FourierCoeffs {
            mean: 0.25,
            a: core::array::from_fn(|i| i as f32 + 1.0),
            b: core::array::from_fn(|i| -(i as f32) - 1.0),
        };
        let mut bytes = [0_u8; COEFF_COUNT as usize * 4];
        serialize_coeffs(&coeffs, &mut bytes);
        store.set(IDX_TARGET, &bytes).unwrap();

        let Some(RtCommand {
            domain: DOMAIN_GENERATOR,
            id: command_id::generator::SET_TARGET,
            payload: Payload::Values { len, data },
        }) = rx.dequeue()
        else {
            panic!("target write was not routed to the generator");
        };
        assert_eq!(len as u16, COEFF_COUNT);
        assert_eq!(data[0], coeffs.mean);
        assert_eq!(&data[1..1 + HARMONICS], &coeffs.a);
        assert_eq!(&data[1 + HARMONICS..COEFF_COUNT as usize], &coeffs.b);
        assert!(data[COEFF_COUNT as usize..]
            .iter()
            .all(|value| *value == 0.0));
    }

    #[cfg(feature = "diag-max-command-burst")]
    #[test]
    fn diagnostic_reset_preloads_exact_per_tick_command_limit() {
        let (mut store, mut rx) = store();
        store.set(IDX_DIAG_RESET, &1_u32.to_le_bytes()).unwrap();
        let expected_ids = if cfg!(feature = "diag-wide-command-payload") {
            [
                command_id::generator::DIAGNOSTIC_VALUES,
                command_id::generator::DIAGNOSTIC_VALUES,
            ]
        } else {
            [
                command_id::generator::SET_TARGET,
                command_id::generator::SET_FORCING,
            ]
        };
        for expected_id in expected_ids {
            assert!(matches!(
                rx.dequeue(),
                Some(RtCommand {
                    domain: DOMAIN_GENERATOR,
                    id,
                    payload: Payload::Values { len, .. },
                }) if id == expected_id && len as u16 == COEFF_COUNT
            ));
        }
        assert!(rx.dequeue().is_none());
    }

    #[test]
    fn full_command_queue_returns_table_token_and_restores_staging() {
        let (mut store, _rx) = store();
        store
            .set_block(
                IDX_TABLE,
                0,
                &[1.0f32.to_le_bytes(), 2.0f32.to_le_bytes()].concat(),
            )
            .unwrap();

        loop {
            match store.set(IDX_FREQ, &20.0f32.to_le_bytes()) {
                Ok(ParamAction::None) => {}
                Err(ErrorCode::Busy) => break,
                result => panic!("unexpected queue-fill result: {result:?}"),
            }
        }
        assert_eq!(store.commit(IDX_TABLE, 2), Err(ErrorCode::Busy));
        assert!(store.set_block(IDX_TABLE, 0, &3.0f32.to_le_bytes()).is_ok());
    }

    #[test]
    fn full_command_queue_does_not_update_scalar_shadow() {
        let (mut store, _rx) = store();
        loop {
            match store.set(IDX_FREQ, &20.0f32.to_le_bytes()) {
                Ok(ParamAction::None) => {}
                Err(ErrorCode::Busy) => break,
                result => panic!("unexpected queue-fill result: {result:?}"),
            }
        }
        assert_eq!(
            store.set(IDX_FREQ, &21.0f32.to_le_bytes()),
            Err(ErrorCode::Busy)
        );
        let mut out = [0_u8; 4];
        store.get(IDX_FREQ, &mut out).unwrap();
        assert_eq!(f32::from_le_bytes(out), 20.0);
    }

    #[test]
    fn table_length_changes_only_after_core_one_activation() {
        let (mut store, mut rx, mut active) = store_with_table();
        let bytes: std::vec::Vec<_> = [1.0f32, 2.0, 3.0]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .collect();
        store.set_block(IDX_TABLE, 0, &bytes).unwrap();
        store.commit(IDX_TABLE, 3).unwrap();

        let mut out = [0_u8; 2];
        store.get(IDX_TABLE_LEN, &mut out).unwrap();
        assert_eq!(u16::from_le_bytes(out), 0);

        let Some(RtCommand {
            domain: DOMAIN_TABLE,
            id: command_id::table::ACTIVATE,
            payload: Payload::Buffer(token),
        }) = rx.dequeue()
        else {
            panic!("table activation command was not enqueued");
        };
        active.activate(token);
        store
            .shared
            .live
            .active_table_len
            .store(active.get().len() as u32, Ordering::Relaxed);
        store.get(IDX_TABLE_LEN, &mut out).unwrap();
        assert_eq!(u16::from_le_bytes(out), 3);
    }
}
