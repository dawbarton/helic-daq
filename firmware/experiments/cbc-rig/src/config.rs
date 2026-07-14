//! Compile-time configuration: sample-rate preset, harmonic count, and the
//! active controller. This is the one file a user edits to reconfigure the
//! instrument for their experiment.

use helic_core::controller::PassThrough;
pub use helic_fw_common::{SampleRate, HARMONICS};

/// DAC channel driven by the control output (0 and 2 are bipolar).
pub const OUTPUT_CHANNEL: usize = 0;

/// Measuring range of the attached optoNCDT sensor in mm (model-dependent:
/// 10/25/50/100/200/500).
pub const LASER_RANGE_MM: f32 = 50.0;

/// Static IPv4 address and prefix length (flash-stored config is a future
/// milestone; until then, edit and reflash).
pub const IP_ADDR: [u8; 4] = [192, 168, 1, 235];
pub const IP_PREFIX: u8 = 24;

/// Locally administered MAC address.
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x01];

/// The controller that runs inside every sample tick. Swap the alias (and
/// `make_controller`) to change the control law at compile time, e.g.:
///
/// ```ignore
/// pub type ActiveController = helic_core::controller::PidController;
/// pub fn make_controller() -> ActiveController {
///     PidController::new(Pid::new(PidConfig { kp: 1.0, ..Default::default() }), Some(0))
/// }
/// ```
pub type ActiveController = PassThrough;

pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Selected sample-rate preset.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
