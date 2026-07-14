use helic_core::controller::PassThrough;
pub use helic_fw_common::SampleRate;

pub const EXPERIMENT: &str = "sig-gen";
pub const OUTPUT_CHANNEL: usize = 0;
pub const LASER_RANGE_MM: f32 = 50.0;

pub const IP_ADDR: [u8; 4] = [192, 168, 1, 236];
pub const IP_PREFIX: u8 = 24;
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x02];

pub type ActiveController = PassThrough;

pub fn make_controller() -> ActiveController {
    PassThrough
}

pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
