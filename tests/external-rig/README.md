# External rig acceptance fixture

This directory is an independent Cargo workspace, not a member of either
HELIC-DAQ workspace. It models a separately maintained rig with its own
programme, controller, firmware, target configuration, lockfile, verification
profiles, and dependency policy.

It holds two firmware members, which together cover the whole platform surface
an out-of-tree rig can use:

- `fw-fixture-rig` links no core-0 support at all. It proves the real-time
  platform (`helic-core`, `helic-rt`, `helic-fw-rt`) stands alone, and its
  dependency policy forbids `helic-fw-support` so that a transitive dependency
  cannot creep in unnoticed.
- `fw-fixture-service-rig` composes the services every production rig actually
  uses: `control_run` over locally defined `Rig`, `Program` and `StandardControl`
  types, UDP streaming, discovery, status, the time watchdog, and build
  identity derived from this workspace. That half is generic, macro-bearing and
  build-script-bearing, which is where boundary defects live, and the first
  fixture is structurally unable to see them.

Neither is flashed. Both are compile, link and layout fixtures; they establish
the repository boundary, not electrical behaviour.

The manifests request exact `=0.3.0` HELIC crate versions. Until those crates
are published, `[patch.crates-io]` substitutes the repository checkout; remove
that table to exercise released packages unchanged.

The verification tools are installed from the host package rather than run out
of the platform checkout, which is how a real out-of-tree rig consumes them.
From this directory, the complete boundary check is:

```sh
pip install -e ../../host-python
# A real out-of-tree rig instead installs it from the platform tag:
#   pip install "helic-daq @ git+<url>@<tag>#subdirectory=host-python"
cargo test -p fixture-rig-program --target x86_64-unknown-linux-gnu
cargo build --release --workspace
helic-deps-check --policy dependency-policy.toml
helic-rt-layout --profile rig-profile.toml --profile service-rig-profile.toml \
  --elf-dir target/thumbv8m.main-none-eabihf/release
python -m unittest test_regression_profile.py
```

Each tool resolves its defaults from the current directory, so none of them
needs to know where the platform checkout is.

The last command uses the fixture's deterministic device and clock; it performs
no network or hardware operation.
