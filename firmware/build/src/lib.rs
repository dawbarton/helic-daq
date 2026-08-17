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
        watch_repository(repo);
    }

    let describe = git_value(repo.as_deref(), &["describe", "--always", "--dirty"]);
    println!("cargo:rustc-env=HELIC_GIT_DESCRIBE={describe}");

    let version = env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
    let revision = wire_revision(repo.as_deref());
    let firmware_id = format_firmware_id(
        &version,
        revision
            .as_ref()
            .map(|(hash, dirty)| (hash.as_str(), *dirty)),
    );
    println!("cargo:rustc-env=HELIC_FIRMWARE_ID={firmware_id}");
}

/// Format the compact wire identity without ever claiming an uncertain commit.
fn format_firmware_id(version: &str, revision: Option<(&str, bool)>) -> String {
    let revision = match revision {
        Some((hash, false)) => hash.to_owned(),
        Some((hash, true)) => format!("{hash}+"),
        None => "?".to_owned(),
    };
    let firmware_id = format!("{version} {revision}");
    assert!(
        firmware_id.len() <= FIRMWARE_ID_LEN,
        "firmware wire identity {firmware_id:?} exceeds {FIRMWARE_ID_LEN} bytes; \
         shorten the package version"
    );
    firmware_id
}

/// Return the checked-out commit and whether tracked content differs from it.
///
/// Untracked files are deliberately outside the repository state represented
/// by this identity, so an unrelated scratch file does not taint an image.
/// If either query fails, the whole revision is unknown: a known hash without
/// a trustworthy cleanliness result would still make a dirty build look clean.
fn wire_revision(repo: Option<&Path>) -> Option<(String, bool)> {
    let hash =
        git_output(repo, &["rev-parse", "--short=7", "HEAD"]).filter(|value| !value.is_empty())?;
    let status = git_output(
        repo,
        &[
            "--no-optional-locks",
            "status",
            "--porcelain",
            "--untracked-files=no",
        ],
    )?;
    Some((hash, !status.is_empty()))
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

/// Re-run the build script when the revision or any tracked content changes.
///
/// Emitting even one `rerun-if-changed` line disables Cargo's default package
/// scan. The complete tracked-file list is therefore part of the identity
/// contract: otherwise Cargo can rebuild dirty firmware with cached clean
/// environment variables. Git metadata is resolved through `rev-parse` so
/// ordinary clones, worktrees and submodules use their actual metadata paths.
fn watch_repository(repo: &Path) {
    for name in ["HEAD", "index", "refs", "packed-refs"] {
        if let Some(path) = git_path(repo, name).filter(|path| path.exists()) {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }

    let Some(files) = git_bytes(Some(repo), &["ls-files", "-z"]) else {
        return;
    };
    for file in files
        .split(|byte| *byte == 0)
        .filter(|file| !file.is_empty())
    {
        if let Ok(file) = str::from_utf8(file) {
            println!("cargo:rerun-if-changed={}", repo.join(file).display());
        }
    }
}

/// Resolve one path in Git's per-worktree or common metadata directory.
fn git_path(repo: &Path, name: &str) -> Option<PathBuf> {
    let path = PathBuf::from(git_output(Some(repo), &["rev-parse", "--git-path", name])?);
    Some(if path.is_absolute() {
        path
    } else {
        repo.join(path)
    })
}

/// Run a git command in `repo`, preserving successful empty output.
fn git_bytes(repo: Option<&Path>, args: &[&str]) -> Option<Vec<u8>> {
    let repo = repo?;
    Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| output.stdout)
}

/// Run a git command and decode its trimmed textual output.
fn git_output(repo: Option<&Path>, args: &[&str]) -> Option<String> {
    git_bytes(repo, args)
        .and_then(|output| String::from_utf8(output).ok())
        .map(|output| output.trim().to_owned())
}

/// Run a git command, retaining the banner's established `unknown` fallback.
fn git_value(repo: Option<&Path>, args: &[&str]) -> String {
    git_output(repo, args)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_identity_distinguishes_clean_dirty_and_unknown() {
        assert_eq!(
            format_firmware_id("0.2.5", Some(("abcdef0", false))),
            "0.2.5 abcdef0"
        );
        assert_eq!(
            format_firmware_id("0.2.5", Some(("abcdef0", true))),
            "0.2.5 abcdef0+"
        );
        assert_eq!(format_firmware_id("0.2.5", None), "0.2.5 ?");
        assert_eq!(
            format_firmware_id("0.100.0", Some(("abcdef0", true))).len(),
            FIRMWARE_ID_LEN
        );
    }

    #[test]
    #[should_panic(expected = "exceeds 16 bytes")]
    fn compact_identity_rejects_an_overlong_dirty_version() {
        format_firmware_id("0.1000.0", Some(("abcdef0", true)));
    }
}
