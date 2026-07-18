//! Fixed platform parameter schema, indices, and registry capacity checks.

use helic_core::table::MAX_TABLE_LEN;
use helic_proto::ParamType;

use super::{ParamDef, COEFF_COUNT};

/// Platform parameters precede experiment and controller extensions.
pub const BASE_PARAMS: &[ParamDef] = &[
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
    ParamDef::writable("freq", ParamType::F32, 1),
    ParamDef::writable("target_coeffs", ParamType::F32, COEFF_COUNT),
    ParamDef::writable("forcing_coeffs", ParamType::F32, COEFF_COUNT),
    ParamDef::writable("ctrl_reset", ParamType::U32, 1),
    ParamDef::writable("table", ParamType::F32, MAX_TABLE_LEN as u16),
    ParamDef::read_only("table_len", ParamType::U16, 1),
    ParamDef::writable("table_freq", ParamType::F32, 1),
    ParamDef::writable("table_gain", ParamType::F32, 1),
    ParamDef::writable("table_interp", ParamType::U32, 1),
    ParamDef::writable("table_mode", ParamType::U32, 1),
    ParamDef::writable("table_mult", ParamType::U32, 1),
    ParamDef::writable("table_phase", ParamType::F32, 1),
    ParamDef::writable("table_trigger", ParamType::U32, 1),
    ParamDef::read_only("wake_phase_min", ParamType::U32, 1),
    ParamDef::read_only("wake_phase_max", ParamType::U32, 1),
    ParamDef::read_only("t_measure_max", ParamType::U32, 1),
    ParamDef::read_only("t_actuate_max", ParamType::U32, 1),
    ParamDef::read_only("t_rest_max", ParamType::U32, 1),
    ParamDef::writable("diag_reset", ParamType::U32, 1),
    ParamDef::read_only("cmd_backlog_max", ParamType::U32, 1),
    // Output safety stage. Present on every experiment for a uniform host
    // interface, but only acted on when the rig sets `Rig::SAFETY_GATED`;
    // otherwise `arm` writes are inert and `safety` reads 0. `safety` is a
    // bitfield (see IDX_SAFETY) so the whole gate state is one pollable word;
    // the exact clamp/quiet tick counts stay in the RT atomics and the status
    // log. Kept to two entries because the base registry is near its
    // single-frame discovery budget.
    ParamDef::writable("arm", ParamType::U32, 1),
    ParamDef::read_only("safety", ParamType::U32, 1),
];

pub(super) const IDX_FREQ: usize = 10;
pub(super) const IDX_TARGET: usize = 11;
pub(super) const IDX_FORCING: usize = 12;
pub(super) const IDX_CTRL_RESET: usize = 13;
pub(super) const IDX_TABLE: usize = 14;
pub(super) const IDX_TABLE_LEN: usize = 15;
pub(super) const IDX_TABLE_FREQ: usize = 16;
pub(super) const IDX_TABLE_GAIN: usize = 17;
pub(super) const IDX_TABLE_INTERPOLATION: usize = 18;
pub(super) const IDX_TABLE_MODE: usize = 19;
pub(super) const IDX_TABLE_MULT: usize = 20;
pub(super) const IDX_TABLE_PHASE: usize = 21;
pub(super) const IDX_TABLE_TRIGGER: usize = 22;
pub(super) const IDX_WAKE_PHASE_MIN: usize = 23;
pub(super) const IDX_WAKE_PHASE_MAX: usize = 24;
pub(super) const IDX_T_MEASURE_MAX: usize = 25;
pub(super) const IDX_T_ACTUATE_MAX: usize = 26;
pub(super) const IDX_T_REST_MAX: usize = 27;
pub(super) const IDX_DIAG_RESET: usize = 28;
pub(super) const IDX_COMMAND_BACKLOG_MAX: usize = 29;
pub(super) const IDX_ARM: usize = 30;
pub(super) const IDX_SAFETY: usize = 31;

pub(super) const MAX_CTRL_PARAMS: usize = 8;
// Reserve reclaimed from 8 to make single-frame discovery room for the two
// safety base params; no experiment declares more than two rig params, so six
// remains ample headroom.
pub(super) const MAX_RIG_PARAMS: usize = 6;
pub(super) const MAX_EXTRA_PARAMS: usize = 8;
pub(super) const DISCOVERY_HEADROOM: usize = helic_proto::MAX_PAYLOAD * 3 / 4;

const fn encoded_defs_len(defs: &[ParamDef]) -> usize {
    let mut total = 0;
    let mut i = 0;
    while i < defs.len() {
        total += defs[i].name.len() + 5;
        i += 1;
    }
    total
}

const MAX_REGISTRY_ENCODED_LEN: usize = encoded_defs_len(BASE_PARAMS)
    + MAX_EXTRA_PARAMS * (helic_proto::payload::MAX_PARAM_NAME_LEN + 5)
    + (MAX_RIG_PARAMS + MAX_CTRL_PARAMS) * (helic_proto::payload::MAX_NAME_LEN + 5);
const _: () = assert!(MAX_REGISTRY_ENCODED_LEN <= helic_proto::MAX_PAYLOAD);
