//! Compile-time choices for `encoder-rig`.
//!
//! This is `cbc-rig` plus an SSI absolute encoder. Read `cbc-rig/config.rs`
//! for the common pattern and `notes.md` before using the provisional encoder
//! format on hardware.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

/// Name advertised to discovery clients.
pub const EXPERIMENT: &str = "encoder-rig";

/// DAC channel driven by the control output. Its polarity is defined in
/// `board.rs` for the fitted analogue output stage.
pub const OUTPUT_CHANNEL: usize = 0;

/// Measuring range of the attached optoNCDT sensor in mm (model-dependent:
/// 10/25/50/100/200/500).
pub const LASER_RANGE_MM: f32 = 50.0;

/// Provisional RMB20 frame format. Confirm it against the ordered part before
/// bring-up; changing these constants does not require changes to the SSI
/// driver or wire protocol.
pub const ENCODER_BITS: u8 = 13;
pub const ENCODER_GRAY: bool = true;
pub const ENCODER_BIT_RATE_HZ: u32 = 500_000;
pub const ENCODER_COUNTS_PER_REV: u32 = 1 << ENCODER_BITS;

/// Static IPv4 address and prefix length. Configuration is not persisted;
/// edit and reflash to change it.
pub const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 238],
    prefix: 24,
};

/// Locally administered MAC address.
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x04];

/// Concrete controller selected at compile time. See the "Writing a
/// controller" section of `docs/developer_guide.md`.
///
/// Swap the alias and constructor together, for example:
///
/// ```ignore
/// pub type ActiveController = helic_core::controller::PidController;
/// pub fn make_controller() -> ActiveController {
///     PidController::new(Pid::new(PidConfig { kp: 1.0, ..Default::default() }), 0)
/// }
/// ```
pub type ActiveController = PassThrough;

/// Build the controller that will be moved to the real-time core.
pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Hardware sample-clock preset shared by ADC conversion and the RT loop.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
