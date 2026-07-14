//! Cargo build script which exposes the RP2350 `memory.x` to the linker.
//!
//! This program runs on the development computer, so it uses `std` even
//! though the resulting firmware is `no_std`.

use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    // Copy into Cargo's generated output directory, then add it to link search.
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    fs::copy("memory.x", out.join("memory.x")).unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    // Tell Cargo when this build-script result becomes stale.
    println!("cargo:rerun-if-changed=memory.x");
}
