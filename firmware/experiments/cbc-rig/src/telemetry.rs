//! CBC scalar state shared between the RT core, laser task, and parameter view.

use core::sync::atomic::AtomicU32;

use helic_fw_common::params::ExtraParam;

pub static LASER_VALUE: AtomicU32 = AtomicU32::new(0);
pub static LASER_RANGE_MM: AtomicU32 = AtomicU32::new(0);

pub const EXTRA_PARAMS: &[ExtraParam] = &[ExtraParam::f32("laser", &LASER_VALUE)];
