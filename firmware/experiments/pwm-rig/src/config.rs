//! Compile-time choices for the filtered-PWM experiment.
//!
//! Unlike `sig-gen`, this variant needs no external DAC. See the PWM section
//! of `docs/user_guide.md` for the required reconstruction filter and voltage
//! limitations.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

/// Name advertised to discovery clients.
pub const EXPERIMENT: &str = "pwm-rig";
/// Full measuring range used to scale optoNCDT UART samples.
pub const LASER_RANGE_MM: f32 = 50.0;
/// Voltage interval mapped linearly to PWM duty cycle.
pub const PWM_V_MIN: f32 = 0.0;
pub const PWM_V_MAX: f32 = 3.3;

/// Static IPv4 configuration for the wired network interface.
pub const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 237],
    prefix: 24,
};
/// Locally administered MAC address; keep it unique on a network.
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x03];

/// Statically selected controller for the bounded real-time path.
pub type ActiveController = PassThrough;

/// Construct the controller which will be moved to core 1.
pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Hardware-paced control and output update rate.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
