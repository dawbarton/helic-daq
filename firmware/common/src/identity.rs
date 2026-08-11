//! Firmware build identity generated from the application repository.

/// Human-readable firmware identification used in the boot banner.
pub const FIRMWARE_BANNER: &str = concat!(
    "helic-daq ",
    env!("CARGO_PKG_VERSION"),
    " ",
    env!("HELIC_GIT_DESCRIBE")
);

/// Firmware identification string, padded or truncated to 16 bytes on wire.
pub const FIRMWARE_VERSION: &str = env!("HELIC_FIRMWARE_ID");
