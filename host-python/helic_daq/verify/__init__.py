"""Verification tools shared by in-tree and out-of-tree HELIC-DAQ rigs.

These ship with the host package so that a rig maintained in its own
repository installs them, rather than copying them, and runs the same gates as
the production experiments:

* ``helic-rt-layout`` checks that named real-time symbols are SRAM-resident;
* ``helic-rt-regression`` runs the sequential hardware regression;
* ``helic-deps-check`` enforces a workspace's crate-layering rules.

Every path a tool needs is either given on the command line or resolved
relative to the current working directory, so none of them assume they are
running inside this repository. Rigs are discovered as data through
``rig-profile.toml``; no tool holds a registry of rig names.
"""

from __future__ import annotations

from pathlib import Path

from .profile import (
    LayoutProfile,
    ProfileError,
    QuietWrite,
    RegressionProfile,
    RigProfile,
    load_profile,
    load_profiles,
)

__all__ = [
    "LayoutProfile",
    "ProfileError",
    "QuietWrite",
    "RegressionProfile",
    "RigProfile",
    "discover_profiles",
    "load_profile",
    "load_profiles",
]

#: Release directory Cargo writes firmware ELFs to, relative to a workspace.
RELEASE_DIR = Path("target") / "thumbv8m.main-none-eabihf" / "release"


def discover_profiles(workspace: Path | None = None) -> list[Path]:
    """Find the rig profiles owned by the experiments in a Cargo workspace.

    Defaults to the current working directory, which is the firmware workspace
    for this repository's own invocations and the rig's workspace for an
    out-of-tree rig. A rig whose profile lives elsewhere passes ``--profile``.
    """
    root = Path.cwd() if workspace is None else workspace
    return sorted(root.glob("experiments/*/rig-profile.toml")) or sorted(
        root.glob("rig-profile.toml")
    )
