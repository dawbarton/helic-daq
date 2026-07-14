//! Compile-time configuration: sample-rate preset, harmonic count, and the
//! active controller. This is the one file a user edits to reconfigure the
//! instrument for their experiment.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

pub const EXPERIMENT: &str = "encoder-rig";

/// DAC channel driven by the control output (0 and 2 are bipolar).
pub const OUTPUT_CHANNEL: usize = 0;

/// Measuring range of the attached optoNCDT sensor in mm (model-dependent:
/// 10/25/50/100/200/500).
pub const LASER_RANGE_MM: f32 = 50.0;

/// Provisional RMB20 format. Confirm against the ordered part before bring-up.
pub const ENCODER_BITS: u8 = 13;
pub const ENCODER_GRAY: bool = true;
pub const ENCODER_BIT_RATE_HZ: u32 = 500_000;
pub const ENCODER_COUNTS_PER_REV: u32 = 1 << ENCODER_BITS;

/// Static IPv4 address and prefix length (flash-stored config is a future
/// milestone; until then, edit and reflash).
pub const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 238],
    prefix: 24,
};

/// Locally administered MAC address.
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x04];

/// The controller that runs inside every sample tick. Swap the alias (and
/// `make_controller`) to change the control law at compile time, e.g.:
///
/// ```ignore
/// pub type ActiveController = helic_core::controller::PidController;
/// pub fn make_controller() -> ActiveController {
///     PidController::new(Pid::new(PidConfig { kp: 1.0, ..Default::default() }), 0)
/// }
/// ```
pub type ActiveController = PassThrough;

pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Selected sample-rate preset.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
