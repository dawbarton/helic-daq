//! Compile-time timing, encoder, RPM-estimator and network choices for
//! `whirl-rig`.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

pub const EXPERIMENT: &str = "whirl-rig";

pub const ENCODER_BITS: u8 = 12;
pub const ENCODER_BIT_RATE_HZ: u32 = 1_000_000;
pub const ENCODER_COUNTS_PER_REV: u32 = 1 << ENCODER_BITS;

pub const PULSE_COUNTER_HZ: u32 = 1_000_000;
// The PIO reload and branch sequence omits three nominal counter iterations.
// Verify this fixed timing correction against the optical input on hardware.
pub const PULSE_COUNTER_OFFSET_TICKS: u32 = 3;
pub const RPM_TAU_S: f32 = 0.25;
pub const RPM_STALE_AFTER_S: f32 = 0.1;
pub const RPM_MIN_PERIOD_S: f32 = 0.005;

pub const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 238],
    prefix: 24,
};

pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x04];

pub type ActiveController = PassThrough;

pub fn make_controller() -> ActiveController {
    PassThrough
}

pub const SAMPLE_RATE: SampleRate = SampleRate::Hz2000;
