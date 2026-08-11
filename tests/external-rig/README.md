# External rig acceptance fixture

This directory is an independent Cargo workspace, not a member of either
HELIC-DAQ workspace. It models a separately maintained rig with its own
programme, firmware, target configuration, lockfile, verification profile, and
dependency policy.

The manifests request exact `=0.1.0` HELIC crate versions. Until those crates
are published, `[patch.crates-io]` substitutes the repository checkout; remove
that table to exercise released packages unchanged.

From this directory, the complete boundary check is:

The verification tools are installed from the host package rather than run out
of the platform checkout, which is how a real out-of-tree rig consumes them:

```sh
pip install -e ../../host-python   # or: pip install "helic-daq @ git+<url>@<tag>"
cargo test -p fixture-rig-program --target x86_64-unknown-linux-gnu
cargo build --release --workspace
helic-deps-check --policy dependency-policy.toml
helic-rt-layout --profile rig-profile.toml \
  --elf-dir target/thumbv8m.main-none-eabihf/release
python -m unittest test_regression_profile.py
```

Each tool resolves its defaults from the current directory, so none of them
needs to know where the platform checkout is.

The last command uses the fixture's deterministic device and clock; it performs
no network or hardware operation.
