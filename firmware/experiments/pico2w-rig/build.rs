//! Cargo build script for the embedded linker layout and build identity.
//!
//! Build scripts run on the development computer using `std`, before the
//! `no_std` firmware is compiled. Identity is emitted here, in the application
//! crate, so that it describes this rig's repository rather than the shared
//! platform crates.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    helic_fw_build::emit_identity();

    // OUT_DIR is Cargo-managed and unique to this package/build profile.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");
}
