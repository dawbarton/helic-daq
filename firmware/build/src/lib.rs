//! Build-script helpers shared by every HELIC-DAQ firmware application.
//!
//! These run on the development computer, before the `no_std` firmware is
//! compiled, and exist so that a rig maintained in its own repository gets the
//! same build identity and linker layout as the in-tree experiments without
//! copying either. Call them from the application's `build.rs`:
//!
//! ```no_run
//! helic_fw_build::emit_identity();
//! helic_fw_build::emit_memory_x();
//! ```
//!
//! Identity is deliberately derived from the *application* crate rather than
//! from a shared crate: the version and git revision that matter are those of
//! the repository which owns the rig, which is not this one for an out-of-tree
//! rig.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The linker layout used by every 2 MiB-flash RP2350 board supported so far.
const MEMORY_X: &str = include_str!("memory.x");

/// Longest firmware identity the discovery protocol can carry, in bytes.
const FIRMWARE_ID_LEN: usize = 16;

/// Export the calling crate's build identity as compile-time environment vars.
///
/// Defines `HELIC_GIT_DESCRIBE` (human-readable, used in the boot banner) and
/// `HELIC_FIRMWARE_ID` (the wire identity, at most 16 bytes). Both describe the
/// repository containing the calling crate, found by walking up from its
/// manifest directory, so an out-of-tree rig reports its own revision.
pub fn emit_identity() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let repo = git_root(&manifest);

    if let Some(repo) = repo.as_deref() {
        watch_head(repo);
    }

    let describe = git(repo.as_deref(), &["describe", "--always", "--dirty"]);
    println!("cargo:rustc-env=HELIC_GIT_DESCRIBE={describe}");

    let hash = git(repo.as_deref(), &["rev-parse", "--short=7", "HEAD"]);
    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let firmware_id = format!("{version} {hash}");
    assert!(
        firmware_id.len() <= FIRMWARE_ID_LEN,
        "firmware wire identity {firmware_id:?} exceeds {FIRMWARE_ID_LEN} bytes; \
         shorten the package version"
    );
    println!("cargo:rustc-env=HELIC_FIRMWARE_ID={firmware_id}");
}

/// Place the shared RP2350 `memory.x` on the linker search path.
///
/// A rig whose board departs from the 2 MiB flash layout should keep its own
/// `memory.x` and copy it itself rather than calling this.
pub fn emit_memory_x() {
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("memory.x"), MEMORY_X).expect("cannot write memory.x");
    println!("cargo:rustc-link-search={}", out.display());
}

/// Find the repository working tree containing `start`, if there is one.
///
/// `.git` is a directory in a normal clone and a file in a worktree or
/// submodule, so test for presence rather than for a directory.
fn git_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Re-run the build script when the checked-out revision changes.
fn watch_head(repo: &Path) {
    let git_dir = repo.join(".git");
    if !git_dir.is_dir() {
        // A worktree or submodule keeps its metadata elsewhere; rebuilding on
        // every revision change is not worth resolving the indirection.
        return;
    }
    let head = git_dir.join("HEAD");
    println!("cargo:rerun-if-changed={}", head.display());
    if let Ok(contents) = fs::read_to_string(&head) {
        if let Some(reference) = contents.strip_prefix("ref: ").map(str::trim) {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(reference).display()
            );
        }
    }
}

/// Run a git command in `repo`, yielding `unknown` when git or the repository
/// is unavailable. Building outside a checkout must not fail the build.
fn git(repo: Option<&Path>, args: &[&str]) -> String {
    let Some(repo) = repo else {
        return "unknown".to_owned();
    };
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}
