//! Compile-time choices for the wired, ADC-free signal generator.
//!
//! Physical wiring stays in `board.rs`. The full list of supported settings
//! and host commands is in `docs/user_guide.md`.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

/// Name advertised to discovery clients.
pub const EXPERIMENT: &str = "sig-gen";
/// AD5064 channel which receives the generated output.
pub const OUTPUT_CHANNEL: usize = 0;
/// Full measuring range used to scale optoNCDT UART samples.
pub const LASER_RANGE_MM: f32 = 50.0;

/// Static IPv4 configuration for the wired reference transport.
pub const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 236],
    prefix: 24,
};
/// Locally administered MAC address; keep it unique on a network.
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x02];

/// Statically selected controller. `PassThrough` makes output equal to target
/// plus forcing and table contributions.
pub type ActiveController = PassThrough;

/// Construct the controller which will be moved to core 1.
pub fn make_controller() -> ActiveController {
    PassThrough
}

/// The PWM-wrap interrupt provides this hardware-paced RT tick rate.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
