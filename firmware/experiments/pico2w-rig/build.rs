//! Cargo build script for the embedded linker layout and build identity.
//!
//! Build scripts run on the development computer using `std`, before the
//! `no_std` firmware is compiled. Both inputs come from the application crate:
//! the identity so that it describes this rig's repository rather than the
//! shared platform crates, and the linker layout so that every 2 MiB-flash
//! board shares one description.

fn main() {
    helic_fw_build::emit_identity();
    helic_fw_build::emit_memory_x();
}
