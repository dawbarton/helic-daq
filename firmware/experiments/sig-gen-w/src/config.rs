//! Compile-time choices for the Pico 2W signal generator.
//!
//! This variant replaces the wired WIZnet transport with the CYW43439 radio.
//! See the `sig-gen-w` connection instructions in `docs/user_guide.md`.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

/// Name advertised to discovery clients.
pub const EXPERIMENT: &str = "sig-gen-w";
/// AD5064 channel which receives the generated output.
pub const OUTPUT_CHANNEL: usize = 0;
/// Full measuring range used to scale optoNCDT UART samples.
pub const LASER_RANGE_MM: f32 = 50.0;

/// Wi-Fi uses DHCP by default. Credentials are compiled into the image, so do
/// not commit real laboratory credentials to source control.
pub const NET_CONFIG: NetConfig = NetConfig::Dhcp;
pub const WIFI_SSID: &str = "replace-me";
pub const WIFI_PASSWORD: &str = "replace-me";

/// Statically selected controller for the bounded real-time path.
pub type ActiveController = PassThrough;

/// Construct the controller which will be moved to core 1.
pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Hardware-paced control and DAC update rate.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
