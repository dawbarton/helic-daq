//! Compile-time choices for the Pico 2W signal generator.
//!
//! The CYW43439 radio provides the only network transport for this rig. See
//! the `pico2w-rig` connection instructions in `docs/user_guide.md`.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

/// Name advertised to discovery clients.
pub const EXPERIMENT: &str = "pico2w-rig";
/// AD5064 channel which receives the generated output.
pub const OUTPUT_CHANNEL: usize = 0;
/// Full measuring range used to scale optoNCDT UART samples.
pub const LASER_RANGE_MM: f32 = 50.0;

/// Wi-Fi uses DHCP by default. Credentials are supplied through the build
/// environment and compiled into the image without editing tracked source.
pub const NET_CONFIG: NetConfig = NetConfig::Dhcp;

pub fn wifi_credentials() -> (&'static str, &'static str) {
    let ssid = match option_env!("HELIC_WIFI_SSID") {
        Some(value) => value,
        None => panic!("set HELIC_WIFI_SSID when building fw-pico2w-rig"),
    };
    let password = match option_env!("HELIC_WIFI_PASSWORD") {
        Some(value) => value,
        None => panic!("set HELIC_WIFI_PASSWORD when building fw-pico2w-rig"),
    };
    assert!(!ssid.is_empty(), "HELIC_WIFI_SSID must not be empty");
    (ssid, password)
}

/// Statically selected controller for the bounded real-time path.
pub type ActiveController = PassThrough;

/// Construct the controller which will be moved to core 1.
pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Hardware-paced control and DAC update rate.
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
