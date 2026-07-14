use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

pub const EXPERIMENT: &str = "sig-gen-w";
pub const OUTPUT_CHANNEL: usize = 0;
pub const LASER_RANGE_MM: f32 = 50.0;

pub const NET_CONFIG: NetConfig = NetConfig::Dhcp;
pub const WIFI_SSID: &str = "replace-me";
pub const WIFI_PASSWORD: &str = "replace-me";

pub type ActiveController = PassThrough;

pub fn make_controller() -> ActiveController {
    PassThrough
}

pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
