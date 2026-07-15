//! Whirl latest-value and diagnostic atomics exposed through discovery.

use core::sync::atomic::AtomicU32;

use helic_fw_common::params::ExtraParam;

pub static PITCH_VALUE: AtomicU32 = AtomicU32::new(0);
pub static YAW_VALUE: AtomicU32 = AtomicU32::new(0);
pub static REV_PERIOD_VALUE: AtomicU32 = AtomicU32::new(0);
pub static RPM_VALUE: AtomicU32 = AtomicU32::new(0);
pub static SSI_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static PULSE_COUNT: AtomicU32 = AtomicU32::new(0);
pub static PULSE_GLITCHES: AtomicU32 = AtomicU32::new(0);
pub static PULSE_ERRORS: AtomicU32 = AtomicU32::new(0);

pub const EXTRA_PARAMS: &[ExtraParam] = &[
    ExtraParam::f32("pitch", &PITCH_VALUE),
    ExtraParam::f32("yaw", &YAW_VALUE),
    ExtraParam::f32("rev_period", &REV_PERIOD_VALUE),
    ExtraParam::f32("rpm", &RPM_VALUE),
    ExtraParam::u32("ssi_errors", &SSI_ERRORS),
    ExtraParam::u32("pulse_count", &PULSE_COUNT),
    ExtraParam::u32("pulse_glitches", &PULSE_GLITCHES),
    ExtraParam::u32("pulse_errors", &PULSE_ERRORS),
];
