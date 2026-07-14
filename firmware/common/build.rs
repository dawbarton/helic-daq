use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.join("../..");
    let git_dir = repo.join(".git");
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    if let Ok(head) = fs::read_to_string(git_dir.join("HEAD")) {
        if let Some(reference) = head.strip_prefix("ref: ").map(str::trim) {
            println!(
                "cargo:rerun-if-changed={}",
                git_dir.join(reference).display()
            );
        }
    }

    let describe = Command::new("git")
        .args(["describe", "--always", "--dirty"])
        .current_dir(&repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    println!("cargo:rustc-env=HELIC_GIT_DESCRIBE={describe}");

    let hash = Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .current_dir(&repo)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_owned());
    let version = env::var("CARGO_PKG_VERSION").unwrap();
    let firmware_id = format!("{version} {hash}");
    assert!(
        firmware_id.len() <= 16,
        "firmware wire identity exceeds c×16"
    );
    println!("cargo:rustc-env=HELIC_FIRMWARE_ID={firmware_id}");
}
