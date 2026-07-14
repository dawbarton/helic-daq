//! Cargo build script for the embedded linker layout.
//!
//! Build scripts run on the development computer using `std`, before the
//! `no_std` firmware is compiled. Every experiment needs this small script so
//! the RP2350 linker can find its local `memory.x` description.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // OUT_DIR is Cargo-managed and unique to this package/build profile.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    // `cargo:` lines communicate build metadata back to Cargo.
    println!("cargo:rustc-link-search={}", out.display());
    // Re-run this script only when the source memory map changes.
    println!("cargo:rerun-if-changed=memory.x");
}
