//! Cargo build script which copies the RP2350 memory map for the linker.
//!
//! It runs on the host before the `no_std` target firmware is compiled.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // OUT_DIR is Cargo's package- and profile-specific generated directory.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    // Re-run only if the checked-in memory map changes.
    println!("cargo:rerun-if-changed=memory.x");
}
