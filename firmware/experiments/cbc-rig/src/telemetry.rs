//! CBC scalar state shared between the RT core, laser task, and parameter view.

use core::sync::atomic::AtomicU32;

use helic_fw_common::params::ExtraParam;

pub static LASER_VALUE: AtomicU32 = AtomicU32::new(0);
pub static LASER_RANGE_MM: AtomicU32 = AtomicU32::new(0);
pub static LASER_FRAMES_RECEIVED: AtomicU32 = AtomicU32::new(0);
pub static LASER_UART_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static LASER_PARSE_ERRORS: AtomicU32 = AtomicU32::new(0);
pub static LASER_INVALID_FRAMES: AtomicU32 = AtomicU32::new(0);
pub static LASER_UNEXPECTED_VALUES: AtomicU32 = AtomicU32::new(0);
pub static LASER_SYNC_ERRORS: AtomicU32 = AtomicU32::new(0);

pub const EXTRA_PARAMS: &[ExtraParam] = &[
    ExtraParam::f32("laser", &LASER_VALUE),
    ExtraParam::u32("laser_frames_received", &LASER_FRAMES_RECEIVED),
    ExtraParam::u32_event("laser_uart_errors", &LASER_UART_ERRORS),
    ExtraParam::u32_event("laser_parse_errors", &LASER_PARSE_ERRORS),
    ExtraParam::u32_event("laser_invalid_frames", &LASER_INVALID_FRAMES),
    ExtraParam::u32_event("laser_unexpected_values", &LASER_UNEXPECTED_VALUES),
    ExtraParam::u32_event("laser_sync_errors", &LASER_SYNC_ERRORS),
];
