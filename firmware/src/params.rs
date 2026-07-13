//! Name-based parameter registry (design adopted from rtc, see
//! `docs/implementation_plan.md` §5a).
//!
//! The host discovers parameters at connect (`GetParNames` / `GetParInfo`)
//! and addresses them by index thereafter. Reads are served from core-0
//! state: diagnostics come from atomics the RT loop maintains, writable
//! values from the shadow copies kept here. Writes update the shadow and
//! translate to an [`RtCommand`], which core 1 applies at a sample boundary
//! — coefficient sets travel by value, so a tick never sees a torn array.

use core::sync::atomic::Ordering;

use cbc_core::controller::Controller;
use cbc_core::generator::FourierCoeffs;
use cbc_core::phase::PhaseAccumulator;
use cbc_proto::{ErrorCode, ParamType};

use crate::config::{ActiveController, HARMONICS, SAMPLE_RATE};
use crate::rt_loop::{self, CommandProducer, RtCommand};

/// Firmware identification string, padded/truncated to 16 chars on the wire.
pub const FIRMWARE_VERSION: &str = concat!("cbc-daq ", env!("CARGO_PKG_VERSION"));

/// Serialized size of a coefficient set: mean + a[K] + b[K].
pub const COEFF_COUNT: u16 = (1 + 2 * HARMONICS) as u16;

#[derive(Clone, Copy)]
pub struct ParamDef {
    pub name: &'static str,
    pub ty: ParamType,
    pub count: u16,
    pub writable: bool,
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
        name: "busy_timeouts",
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
        name: "laser",
        ty: ParamType::F32,
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
];

const IDX_FREQ: usize = 10;
const IDX_TARGET: usize = 11;
const IDX_FORCING: usize = 12;
const IDX_CTRL_RESET: usize = 13;

/// Maximum number of controller parameters supported.
pub const MAX_CTRL_PARAMS: usize = 8;

#[derive(Clone, Copy)]
enum ShadowUpdate {
    None,
    Freq(f32),
    Target(FourierCoeffs<HARMONICS>),
    Forcing(FourierCoeffs<HARMONICS>),
    CtrlParam(usize, f32),
}

/// Registry state: shadow copies of the writable parameters plus the
/// command producer that forwards writes to the RT loop.
pub struct ParamStore {
    commands: CommandProducer,
    freq_hz: f32,
    target: FourierCoeffs<HARMONICS>,
    forcing: FourierCoeffs<HARMONICS>,
    ctrl_params: [f32; MAX_CTRL_PARAMS],
}

impl ParamStore {
    pub fn new(commands: CommandProducer) -> Self {
        assert!(
            Self::ctrl_names().len() <= MAX_CTRL_PARAMS,
            "controller exposes more parameters than ParamStore can shadow"
        );
        Self {
            commands,
            freq_hz: 0.0,
            target: FourierCoeffs::zero(),
            forcing: FourierCoeffs::zero(),
            ctrl_params: [0.0; MAX_CTRL_PARAMS],
        }
    }

    fn ctrl_names() -> &'static [&'static str] {
        <ActiveController as Controller>::param_names()
    }

    pub fn count(&self) -> usize {
        BASE_PARAMS.len() + Self::ctrl_names().len()
    }

    /// Definition of parameter `index` (base or controller).
    pub fn def(&self, index: usize) -> Option<ParamDef> {
        if index < BASE_PARAMS.len() {
            Some(BASE_PARAMS[index])
        } else {
            Self::ctrl_names()
                .get(index - BASE_PARAMS.len())
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
                let bytes = FIRMWARE_VERSION.as_bytes();
                for (i, o) in out.iter_mut().enumerate() {
                    *o = bytes.get(i).copied().unwrap_or(0);
                }
            }
            1 => out.copy_from_slice(&SAMPLE_RATE.hz().to_le_bytes()),
            2 => out.copy_from_slice(&rt_loop::TICKS.load(Ordering::Relaxed).to_le_bytes()),
            3 => out.copy_from_slice(
                &rt_loop::LOOP_TIME_LAST_US
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            4 => out.copy_from_slice(
                &rt_loop::LOOP_TIME_MAX_US
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            5 => out.copy_from_slice(
                &rt_loop::CLOCK_JITTER_US
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            6 => out.copy_from_slice(&rt_loop::OVERRUNS.load(Ordering::Relaxed).to_le_bytes()),
            7 => out.copy_from_slice(&rt_loop::BUSY_TIMEOUTS.load(Ordering::Relaxed).to_le_bytes()),
            8 => out.copy_from_slice(
                &rt_loop::RECORDS_DROPPED
                    .load(Ordering::Relaxed)
                    .to_le_bytes(),
            ),
            9 => out.copy_from_slice(&rt_loop::LASER_VALUE.load(Ordering::Relaxed).to_le_bytes()),
            IDX_FREQ => out.copy_from_slice(&self.freq_hz.to_le_bytes()),
            IDX_TARGET => serialize_coeffs(&self.target, out),
            IDX_FORCING => serialize_coeffs(&self.forcing, out),
            IDX_CTRL_RESET => out.copy_from_slice(&0u32.to_le_bytes()),
            i => out.copy_from_slice(&self.ctrl_params[i - BASE_PARAMS.len()].to_le_bytes()),
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
                if !(0.0..SAMPLE_RATE.hz() / 2.0).contains(&freq) {
                    return Err(ErrorCode::BadValue);
                }
                (
                    RtCommand::SetIncrement(PhaseAccumulator::increment_for(
                        freq as f64,
                        SAMPLE_RATE.hz() as f64,
                    )),
                    ShadowUpdate::Freq(freq),
                )
            }
            IDX_TARGET => {
                let coeffs = deserialize_coeffs(data);
                (
                    RtCommand::SetTargetCoeffs(coeffs),
                    ShadowUpdate::Target(coeffs),
                )
            }
            IDX_FORCING => {
                let coeffs = deserialize_coeffs(data);
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
            i => {
                let id = (i - BASE_PARAMS.len()) as u16;
                let value = f32::from_le_bytes(data.try_into().unwrap());
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
            ShadowUpdate::CtrlParam(id, value) => self.ctrl_params[id] = value,
        }
        Ok(())
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

fn deserialize_coeffs(data: &[u8]) -> FourierCoeffs<HARMONICS> {
    let f = |i: usize| f32::from_le_bytes(data[4 * i..4 * i + 4].try_into().unwrap());
    let mut c = FourierCoeffs::zero();
    c.mean = f(0);
    for k in 0..HARMONICS {
        c.a[k] = f(1 + k);
        c.b[k] = f(1 + HARMONICS + k);
    }
    c
}
