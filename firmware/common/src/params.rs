//! Name-based parameter registry (design adopted from rtc, see
//! `docs/implementation_plan.md` §5a).
//!
//! The host discovers parameters at connect (`GetParams`)
//! and addresses them by index thereafter. Reads are served from core-0
//! state: diagnostics come from atomics the RT loop maintains, writable
//! values from the shadow copies kept here. Writes update the shadow and
//! translate to an [`RtCommand`], which core 1 applies at a sample boundary
//! — coefficient sets travel by value, so a tick never sees a torn array.

use core::marker::PhantomData;
use core::sync::atomic::Ordering;

use helic_core::controller::Controller;
use helic_core::generator::FourierCoeffs;
use helic_core::phase::PhaseAccumulator;
use helic_core::table::{TableMode, MAX_TABLE_LEN};
use helic_proto::{ErrorCode, ParamType};

use crate::rig::Rig;
use crate::rt_loop::{self, CommandProducer, RtCommand};
use crate::table;
use crate::{SampleRate, HARMONICS};

/// Firmware identification string, padded/truncated to 16 chars on the wire.
pub const FIRMWARE_BANNER: &str = concat!(
    "helic-daq ",
    env!("CARGO_PKG_VERSION"),
    " ",
    env!("HELIC_GIT_DESCRIBE")
);
pub const FIRMWARE_VERSION: &str = env!("HELIC_FIRMWARE_ID");

/// Serialized size of a coefficient set: mean + a[K] + b[K].
pub const COEFF_COUNT: u16 = (1 + 2 * HARMONICS) as u16;

#[derive(Clone, Copy)]
pub struct ParamDef {
    pub name: &'static str,
    pub ty: ParamType,
    pub count: u16,
    pub writable: bool,
}

#[derive(Clone, Copy)]
pub struct ExtraParam {
    pub def: ParamDef,
    pub get: fn(&mut [u8]),
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

/// The fixed (platform) part of the registry. Controller parameters are
/// appended after these, so indices below must match `get`/`set`.
pub const BASE_PARAMS: &[ParamDef] = &[
    ParamDef {
        name: "firmware",
        ty: ParamType::Char,
        count: 16,
        writable: false,
    },
    ParamDef {
        name: "experiment",
        ty: ParamType::Char,
        count: 16,
        writable: false,
    },
    ParamDef {
        name: "sample_freq",
        ty: ParamType::F32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "ticks",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "loop_time_last",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "loop_time_max",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "clock_jitter",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "overruns",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "tick_timeouts",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "records_dropped",
        ty: ParamType::U32,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "freq",
        ty: ParamType::F32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "target_coeffs",
        ty: ParamType::F32,
        count: COEFF_COUNT,
        writable: true,
    },
    ParamDef {
        name: "forcing_coeffs",
        ty: ParamType::F32,
        count: COEFF_COUNT,
        writable: true,
    },
    ParamDef {
        name: "ctrl_reset",
        ty: ParamType::U32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "table",
        ty: ParamType::F32,
        count: MAX_TABLE_LEN as u16,
        writable: true,
    },
    ParamDef {
        name: "table_len",
        ty: ParamType::U16,
        count: 1,
        writable: false,
    },
    ParamDef {
        name: "table_freq",
        ty: ParamType::F32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "table_gain",
        ty: ParamType::F32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "table_mode",
        ty: ParamType::U32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "table_mult",
        ty: ParamType::U32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "table_phase",
        ty: ParamType::F32,
        count: 1,
        writable: true,
    },
    ParamDef {
        name: "table_trigger",
        ty: ParamType::U32,
        count: 1,
        writable: true,
    },
];

const IDX_FREQ: usize = 10;
const IDX_TARGET: usize = 11;
const IDX_FORCING: usize = 12;
const IDX_CTRL_RESET: usize = 13;
const IDX_TABLE: usize = 14;
const IDX_TABLE_LEN: usize = 15;
const IDX_TABLE_FREQ: usize = 16;
const IDX_TABLE_GAIN: usize = 17;
const IDX_TABLE_MODE: usize = 18;
const IDX_TABLE_MULT: usize = 19;
const IDX_TABLE_PHASE: usize = 20;
const IDX_TABLE_TRIGGER: usize = 21;

/// Maximum number of controller parameters supported.
pub const MAX_CTRL_PARAMS: usize = 8;
pub const MAX_RIG_PARAMS: usize = 8;

#[derive(Clone, Copy)]
enum ShadowUpdate {
    None,
    Freq(f32),
    Target(FourierCoeffs<HARMONICS>),
    Forcing(FourierCoeffs<HARMONICS>),
    TableFreq(f32),
    TableGain(f32),
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
    sample_rate: SampleRate,
    experiment: &'static str,
    extras: &'static [ExtraParam],
    freq_hz: f32,
    target: FourierCoeffs<HARMONICS>,
    forcing: FourierCoeffs<HARMONICS>,
    table_freq_hz: f32,
    table_gain: f32,
    table_mode: u32,
    table_mult: u32,
    table_phase: f32,
    rig_params: [f32; MAX_RIG_PARAMS],
    ctrl_params: [f32; MAX_CTRL_PARAMS],
    types: PhantomData<(C, R)>,
}

impl<C: Controller, R: Rig> ParamStore<C, R> {
    pub fn new(
        commands: CommandProducer,
        sample_rate: SampleRate,
        experiment: &'static str,
        extras: &'static [ExtraParam],
    ) -> Self {
        assert!(
            Self::rig_names().len() <= MAX_RIG_PARAMS,
            "rig exposes more parameters than ParamStore can shadow"
        );
        assert!(
            Self::ctrl_names().len() <= MAX_CTRL_PARAMS,
            "controller exposes more parameters than ParamStore can shadow"
        );
        let mut rig_params = [0.0; MAX_RIG_PARAMS];
        let defaults = R::param_defaults();
        assert!(
            defaults.is_empty() || defaults.len() == Self::rig_names().len(),
            "rig parameter defaults must be empty or match param_names"
        );
        rig_params[..defaults.len()].copy_from_slice(defaults);
        Self {
            commands,
            sample_rate,
            experiment,
            extras,
            freq_hz: 0.0,
            target: FourierCoeffs::zero(),
            forcing: FourierCoeffs::zero(),
            table_freq_hz: 0.0,
            table_gain: 1.0,
            table_mode: 0,
            table_mult: 1,
            table_phase: 0.0,
            rig_params,
            ctrl_params: [0.0; MAX_CTRL_PARAMS],
            types: PhantomData,
        }
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

    /// Definition of parameter `index` (base or controller).
    pub fn def(&self, index: usize) -> Option<ParamDef> {
        if index < BASE_PARAMS.len() {
            Some(BASE_PARAMS[index])
        } else if index < BASE_PARAMS.len() + self.extras.len() {
            Some(self.extras[index - BASE_PARAMS.len()].def)
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
            0 => {
                write_string(out, FIRMWARE_VERSION);
            }
            1 => write_string(out, self.experiment),
            2 => out.copy_from_slice(&self.sample_rate.hz().to_le_bytes()),
            3 => out.copy_from_slice(&rt_loop::TICKS.load(Ordering::Relaxed).to_le_bytes()),
            4 => out.copy_from_slice(
                &rt_loop::LOOP_TIME_LAST_US
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            5 => out.copy_from_slice(
                &rt_loop::LOOP_TIME_MAX_US
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            6 => out.copy_from_slice(
                &rt_loop::CLOCK_JITTER_US
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            7 => out.copy_from_slice(&rt_loop::OVERRUNS.load(Ordering::Relaxed).to_le_bytes()),
            8 => out.copy_from_slice(&rt_loop::TICK_TIMEOUTS.load(Ordering::Relaxed).to_le_bytes()),
            9 => out.copy_from_slice(
                &rt_loop::RECORDS_DROPPED
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            IDX_FREQ => out.copy_from_slice(&self.freq_hz.to_le_bytes()),
            IDX_TARGET => serialize_coeffs(&self.target, out),
            IDX_FORCING => serialize_coeffs(&self.forcing, out),
            IDX_CTRL_RESET => out.copy_from_slice(&0u32.to_le_bytes()),
            IDX_TABLE_LEN => out.copy_from_slice(&table::active_len().to_le_bytes()),
            IDX_TABLE_FREQ => out.copy_from_slice(&self.table_freq_hz.to_le_bytes()),
            IDX_TABLE_GAIN => out.copy_from_slice(&self.table_gain.to_le_bytes()),
            IDX_TABLE_MODE => out.copy_from_slice(&self.table_mode.to_le_bytes()),
            IDX_TABLE_MULT => out.copy_from_slice(&self.table_mult.to_le_bytes()),
            IDX_TABLE_PHASE => out.copy_from_slice(&self.table_phase.to_le_bytes()),
            IDX_TABLE_TRIGGER => out.copy_from_slice(&0u32.to_le_bytes()),
            i if i < BASE_PARAMS.len() + self.extras.len() => {
                (self.extras[i - BASE_PARAMS.len()].get)(out)
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
    pub fn set(&mut self, index: usize, data: &[u8]) -> Result<(), ErrorCode> {
        let def = self.def(index).ok_or(ErrorCode::BadIndex)?;
        if !def.writable {
            return Err(ErrorCode::ReadOnly);
        }
        if data.len() != def.ty.size() * def.count as usize {
            return Err(ErrorCode::BadLength);
        }
        let (cmd, shadow) = match index {
            IDX_FREQ => {
                let freq = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&freq) {
                    return Err(ErrorCode::BadValue);
                }
                (
                    RtCommand::SetIncrement(PhaseAccumulator::increment_for(
                        freq as f64,
                        self.sample_rate.hz() as f64,
                    )),
                    ShadowUpdate::Freq(freq),
                )
            }
            IDX_TARGET => {
                let coeffs = deserialize_coeffs(data)?;
                (
                    RtCommand::SetTargetCoeffs(coeffs),
                    ShadowUpdate::Target(coeffs),
                )
            }
            IDX_FORCING => {
                let coeffs = deserialize_coeffs(data)?;
                (
                    RtCommand::SetForcingCoeffs(coeffs),
                    ShadowUpdate::Forcing(coeffs),
                )
            }
            IDX_CTRL_RESET => {
                if u32::from_le_bytes(data.try_into().unwrap()) == 0 {
                    return Ok(());
                }
                (RtCommand::ResetController, ShadowUpdate::None)
            }
            IDX_TABLE => return Err(ErrorCode::BadLength),
            IDX_TABLE_FREQ => {
                let freq = f32::from_le_bytes(data.try_into().unwrap());
                if !(0.0..self.sample_rate.hz() / 2.0).contains(&freq) {
                    return Err(ErrorCode::BadValue);
                }
                (
                    RtCommand::SetTableIncrement(PhaseAccumulator::increment_for(
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
                (RtCommand::SetTableGain(gain), ShadowUpdate::TableGain(gain))
            }
            IDX_TABLE_MODE => {
                let mode = u32::from_le_bytes(data.try_into().unwrap());
                let mode_value = TableMode::from_u32(mode).ok_or(ErrorCode::BadValue)?;
                (
                    RtCommand::SetTableMode(mode_value),
                    ShadowUpdate::TableMode(mode),
                )
            }
            IDX_TABLE_MULT => {
                let multiplier = u32::from_le_bytes(data.try_into().unwrap());
                if multiplier == 0 {
                    return Err(ErrorCode::BadValue);
                }
                (
                    RtCommand::SetTableMultiplier(multiplier),
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
                    RtCommand::SetTablePhase(offset),
                    ShadowUpdate::TablePhase(phase),
                )
            }
            IDX_TABLE_TRIGGER => {
                if u32::from_le_bytes(data.try_into().unwrap()) == 0 {
                    return Ok(());
                }
                (RtCommand::TriggerTable, ShadowUpdate::None)
            }
            i if i < BASE_PARAMS.len() + self.extras.len() + Self::rig_names().len() => {
                let id = (i - BASE_PARAMS.len() - self.extras.len()) as u16;
                let value = f32::from_le_bytes(data.try_into().unwrap());
                if !value.is_finite() {
                    return Err(ErrorCode::BadValue);
                }
                (
                    RtCommand::SetRigParam(id, value),
                    ShadowUpdate::RigParam(id as usize, value),
                )
            }
            i => {
                let id =
                    (i - BASE_PARAMS.len() - self.extras.len() - Self::rig_names().len()) as u16;
                let value = f32::from_le_bytes(data.try_into().unwrap());
                if !value.is_finite() {
                    return Err(ErrorCode::BadValue);
                }
                (
                    RtCommand::SetCtrlParam(id, value),
                    ShadowUpdate::CtrlParam(id as usize, value),
                )
            }
        };
        self.commands.enqueue(cmd).map_err(|_| ErrorCode::Busy)?;
        match shadow {
            ShadowUpdate::None => {}
            ShadowUpdate::Freq(freq) => self.freq_hz = freq,
            ShadowUpdate::Target(coeffs) => self.target = coeffs,
            ShadowUpdate::Forcing(coeffs) => self.forcing = coeffs,
            ShadowUpdate::TableFreq(freq) => self.table_freq_hz = freq,
            ShadowUpdate::TableGain(gain) => self.table_gain = gain,
            ShadowUpdate::TableMode(mode) => self.table_mode = mode,
            ShadowUpdate::TableMult(multiplier) => self.table_mult = multiplier,
            ShadowUpdate::TablePhase(phase) => self.table_phase = phase,
            ShadowUpdate::RigParam(id, value) => self.rig_params[id] = value,
            ShadowUpdate::CtrlParam(id, value) => self.ctrl_params[id] = value,
        }
        Ok(())
    }

    pub fn set_block(&mut self, index: usize, offset: u32, data: &[u8]) -> Result<(), ErrorCode> {
        if index != IDX_TABLE {
            return Err(ErrorCode::BadIndex);
        }
        table::set_block(offset, data)
    }

    pub fn commit(&mut self, index: usize, len: u32) -> Result<(), ErrorCode> {
        if index != IDX_TABLE {
            return Err(ErrorCode::BadIndex);
        }
        let buffer = table::begin_commit(len)?;
        if self.commands.enqueue(RtCommand::UseTable(buffer)).is_err() {
            table::cancel_commit();
            return Err(ErrorCode::Busy);
        }
        Ok(())
    }

    pub const fn sample_rate(&self) -> SampleRate {
        self.sample_rate
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
        ParamStore::set(self, index, data)
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
