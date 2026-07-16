//! Compile-time choices for this experiment.
//!
//! Start here when adapting `cbc-rig`: most laboratory choices are constants,
//! while physical pin assignments and analogue polarity live in `board.rs`.
//! The host discovers the resulting parameter and source tables, so these
//! choices do not require host-side indices. See "Things you set at compile
//! time" in `docs/user_guide.md` and "Adding an experiment" in
//! `docs/developer_guide.md`.

use helic_core::controller::PassThrough;
use helic_fw_common::net::NetConfig;
pub use helic_fw_common::SampleRate;

/// Name advertised during discovery. Protocol names are short ASCII strings.
pub const EXPERIMENT: &str = "cbc-rig";

/// DAC channel driven by the control output. Its polarity is defined in
/// `board.rs` for the fitted analogue output stage.
pub const OUTPUT_CHANNEL: usize = 0;

/// Measuring range of the attached optoNCDT sensor in mm (model-dependent:
/// 10/25/50/100/200/500).
pub const LASER_RANGE_MM: f32 = 50.0;

/// optoNCDT measuring-rate command matched to the hardware sample clock.
///
/// The sensor command uses kHz, and must end in LF.
pub const LASER_MEASRATE_COMMAND: &[u8] = match SAMPLE_RATE {
    SampleRate::Hz1000 => b"MEASRATE 1\n",
    SampleRate::Hz2000 => b"MEASRATE 2\n",
    SampleRate::Hz4000 => b"MEASRATE 4\n",
    SampleRate::Hz8000 => b"MEASRATE 8\n",
};

/// Static IPv4 address and prefix length. Configuration is not persisted;
/// edit and reflash to change it.
pub const NET_CONFIG: NetConfig = NetConfig::Static {
    address: [192, 168, 1, 235],
    prefix: 24,
};

/// Locally administered MAC address.
pub const MAC_ADDR: [u8; 6] = [0x02, 0x48, 0x4C, 0x00, 0x00, 0x01];

/// The controller that runs inside every sample tick.
///
/// `type` gives a concrete Rust type a local name. Selecting it at compile
/// time lets Rust specialise the real-time loop, avoiding dynamic dispatch in
/// the 125 microsecond tick budget. Swap this alias and `make_controller()`
/// together, for example:
///
/// ```ignore
/// pub type ActiveController = helic_core::controller::PidController;
/// pub fn make_controller() -> ActiveController {
///     PidController::new(Pid::new(PidConfig { kp: 1.0, ..Default::default() }), 0)
/// }
/// ```
pub type ActiveController = PassThrough;

/// Construct the one controller instance which is later moved to core 1.
///
/// Keep constructor defaults consistent with the controller's `param_value`
/// implementation so the host-visible parameter shadow starts correctly.
pub fn make_controller() -> ActiveController {
    PassThrough
}

/// Selected sample-rate preset. The preset supplies exact PWM divider values;
/// do not replace the hardware-timed clock with a software timer.
#[cfg(feature = "diag-sample-4k")]
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz4000;
#[cfg(not(feature = "diag-sample-4k"))]
pub const SAMPLE_RATE: SampleRate = SampleRate::Hz8000;
