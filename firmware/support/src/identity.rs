//! Build identity owned by the firmware application rather than the platform.
//!
//! The version and revision worth reporting are those of the repository which
//! owns the rig. For an out-of-tree rig that is not this repository, so the
//! values are materialised in the application crate by
//! [`firmware_identity!`](crate::firmware_identity) and threaded through the
//! services that publish them, instead of being constants of this crate.

/// Build identity of one firmware application.
#[derive(Clone, Copy, Debug)]
pub struct Identity {
    /// Human-readable identification used in the boot banner.
    pub banner: &'static str,
    /// Identity published on the wire, padded or truncated to 16 bytes.
    pub version: &'static str,
    /// Version of the HELIC-DAQ platform the application was built against.
    pub platform: &'static str,
}

/// Version of the platform, taken from this crate rather than the application.
pub const PLATFORM_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build an [`Identity`] from the calling crate's package and git revision.
///
/// The application's `build.rs` must call `helic_fw_build::emit_identity()`,
/// which defines the environment variables this expansion reads. Because the
/// expansion happens in the calling crate, every value describes the
/// application and its repository.
#[macro_export]
macro_rules! firmware_identity {
    () => {
        $crate::identity::Identity {
            banner: concat!(
                env!("CARGO_PKG_NAME"),
                " ",
                env!("CARGO_PKG_VERSION"),
                " ",
                env!("HELIC_GIT_DESCRIBE")
            ),
            version: env!("HELIC_FIRMWARE_ID"),
            platform: $crate::identity::PLATFORM_VERSION,
        }
    };
}
