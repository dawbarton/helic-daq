# External rig acceptance fixture

This directory is an independent Cargo workspace, not a member of either
HELIC-DAQ workspace. It models a separately maintained rig with its own
programme, firmware, target configuration, lockfile, verification profile, and
dependency policy.

The manifests request exact `=0.1.0` HELIC crate versions. Until those crates
are published, `[patch.crates-io]` substitutes the repository checkout; remove
that table to exercise released packages unchanged.

From this directory, the complete boundary check is:

```sh
cargo test -p fixture-rig-program --target x86_64-unknown-linux-gnu
cargo build --release --workspace
uv run --no-project python ../../firmware/tools/check_dependencies.py \
  --workspace . --policy dependency-policy.toml
uv run --no-project python ../../firmware/tools/check_rt_layout.py \
  --profile rig-profile.toml \
  --elf-dir target/thumbv8m.main-none-eabihf/release
PYTHONPATH=../../host-python:../../firmware/tools \
  uv run --project ../../host-python --python 3.12 python -m unittest \
  test_regression_profile.py
```

The last command uses the fixture's deterministic device and clock; it performs
no network or hardware operation.
