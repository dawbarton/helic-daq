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

// --- Output safety limits (enforced by the firmware safety gate) ---------
//
// These are hard constraints applied on core 1 after the controller/forcing/
// table sum, before the DAC write. They are compile-time here (edit and
// reflash to change). See `docs/firmware-safety-stage-design.md`.

/// Upper bound on the DAC output voltage driven to the exciter current
/// controller (channel A). Set below the 4.096 V DAC rail. The gate clamps the
/// logical differential command so that `MID_RAIL + out` never exceeds this.
pub const DAC_OUT_CEILING_V: f32 = 4.0;

/// Lower bound on the same channel voltage. Chosen symmetric about `MID_RAIL`
/// for the interim unipolar output stage, giving a symmetric differential
/// drive window. A future bipolar output stage will re-home the common mode
/// and turn these into independent ± limits.
pub const DAC_OUT_FLOOR_V: f32 = 0.096;

/// Safe tip-displacement window (laser, mm). Outside this the gate latches a
/// fault and holds the actuator quiet until the host re-arms. Conservative
/// bounds about the ~25 mm resting point.
pub const DISPLACEMENT_MIN_MM: f32 = 10.0;
pub const DISPLACEMENT_MAX_MM: f32 = 40.0;

/// Quiet the actuator if the laser has published no new frame for this long
/// (blind-feedback guard). Converted to a tick count from the sample rate in
/// `rig.rs`.
pub const LASER_STALE_AFTER_S: f32 = 0.02;

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
