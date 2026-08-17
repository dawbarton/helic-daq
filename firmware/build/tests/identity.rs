//! End-to-end checks for firmware identity across incremental Cargo builds.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[test]
fn incremental_build_marks_tracked_changes_but_not_untracked_files() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock precedes Unix epoch")
        .as_nanos();
    let root = env::temp_dir().join(format!("helic-identity-{}-{nonce}", std::process::id()));
    fs::create_dir_all(root.join("src")).expect("create temporary crate");

    let dependency = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dependency = dependency.display().to_string().replace('\\', "\\\\");
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname = \"identity-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[build-dependencies]\nhelic-fw-build = {{ path = \"{dependency}\" }}\n"
        ),
    )
    .expect("write temporary manifest");
    fs::write(
        root.join("build.rs"),
        "fn main() { helic_fw_build::emit_identity(); }\n",
    )
    .expect("write temporary build script");
    let clean_source = "fn main() { println!(\"{}\", env!(\"HELIC_FIRMWARE_ID\")); }\n";
    fs::write(root.join("src/main.rs"), clean_source).expect("write temporary source");

    git(&root, &["init", "-q"]);
    git(&root, &["config", "user.name", "HELIC-DAQ test"]);
    git(&root, &["config", "user.email", "test@example.invalid"]);
    git(&root, &["add", "Cargo.toml", "build.rs", "src/main.rs"]);
    git(&root, &["commit", "-qm", "initial"]);

    let clean = cargo_identity(&root);
    assert!(clean.starts_with("0.1.0 "), "unexpected identity {clean:?}");
    assert!(!clean.ends_with('+'));

    fs::write(root.join("scratch.txt"), "untracked\n").expect("write untracked file");
    cargo(&root, &["clean", "-q"]);
    assert_eq!(cargo_identity(&root), clean);

    std::thread::sleep(Duration::from_millis(1_100));
    fs::write(
        root.join("src/main.rs"),
        "fn main() { println!(\"dirty {}\", env!(\"HELIC_FIRMWARE_ID\")); }\n",
    )
    .expect("modify tracked source");
    assert_eq!(cargo_identity(&root), format!("dirty {clean}+"));

    std::thread::sleep(Duration::from_millis(1_100));
    fs::write(root.join("src/main.rs"), clean_source).expect("restore tracked source");
    assert_eq!(cargo_identity(&root), clean);

    fs::remove_dir_all(root).expect("remove temporary crate");
}

fn cargo_identity(root: &Path) -> String {
    let output = cargo(root, &["run", "-q"]);
    String::from_utf8(output.stdout)
        .expect("temporary binary output is UTF-8")
        .trim()
        .to_owned()
}

fn cargo(root: &Path, args: &[&str]) -> Output {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    checked(Command::new(cargo).args(args).current_dir(root))
}

fn git(root: &Path, args: &[&str]) -> Output {
    checked(Command::new("git").args(args).current_dir(root))
}

fn checked(command: &mut Command) -> Output {
    let output = command.output().expect("run child command");
    assert!(
        output.status.success(),
        "command failed with {}:\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
