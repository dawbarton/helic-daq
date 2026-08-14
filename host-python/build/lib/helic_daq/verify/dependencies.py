#!/usr/bin/env python3
"""Enforce the HELIC-DAQ crate-layering rules from Cargo metadata and source."""

from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path
from typing import Any

import tomllib


def metadata(workspace: Path) -> dict[str, Any]:
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=workspace,
        text=True,
    )
    return json.loads(output)


def normal_dependencies(package: dict[str, Any]) -> set[str]:
    return {
        dependency["name"]
        for dependency in package["dependencies"]
        if dependency["kind"] is None and dependency["target"] is None
    }


def require_exact(package: dict[str, Any], expected: set[str]) -> None:
    actual = normal_dependencies(package)
    if actual != expected:
        raise SystemExit(
            f"{package['name']} normal dependencies differ: "
            f"expected {sorted(expected)}, found {sorted(actual)}"
        )


def require_absent_from_sources(crate: Path, forbidden: set[str]) -> None:
    for source in crate.rglob("*.rs"):
        text = source.read_text()
        for dependency in forbidden:
            rust_name = dependency.replace("-", "_")
            if rust_name in text:
                raise SystemExit(f"{source}: forbidden core-1 dependency {dependency}")


def reachable_names(data: dict[str, Any], root_name: str) -> set[str]:
    packages = {package["id"]: package["name"] for package in data["packages"]}
    nodes = {node["id"]: node for node in data["resolve"]["nodes"]}
    root = next(
        package_id for package_id, name in packages.items() if name == root_name
    )
    pending = [root]
    visited: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        pending.extend(nodes[package_id]["dependencies"])
    return {packages[package_id] for package_id in visited}


def check_default_policy(data: dict[str, Any], workspace: Path) -> None:
    packages = {package["name"]: package for package in data["packages"]}

    require_exact(packages["helic-core"], {"libm"})
    require_exact(packages["helic-rt"], {"heapless", "helic-core", "helic-proto"})

    forbidden_rt = {
        "embassy-executor",
        "embassy-net",
        "embassy-time",
        "helic-fw-support",
    }
    direct_rt = normal_dependencies(packages["helic-fw-rt"])
    if overlap := direct_rt & forbidden_rt:
        raise SystemExit(f"helic-fw-rt has forbidden dependencies: {sorted(overlap)}")
    require_absent_from_sources(workspace / "rt" / "src", forbidden_rt)

    support_reachable = reachable_names(data, "helic-fw-support")
    if "helic-fw-rt" in support_reachable:
        raise SystemExit("helic-fw-support must not depend on helic-fw-rt")


def require_nonblocking_rtt(data: dict[str, Any]) -> None:
    """Fail a build whose `defmt-rtt` can block the core it logs from.

    Unconditional, and deliberately not something a rig opts into: the hazard
    is a property of the transport, every rig hits it, and the failure is
    invisible in exactly the way that stops anyone finding it.

    `defmt-rtt` initialises its control block to non-blocking, so a rig that
    boots with no debugger is safe. probe-rs then writes the blocking mode
    when it attaches, and nothing rewrites it when the debugger goes away, so
    from the first `cargo run` until the next reset the firmware believes a
    host is draining a buffer that nobody is reading. Once it fills, the next
    `info!` blocks inside a critical section and the whole core stops. On this
    platform that is core 0: network, record drain and device services, while
    core 1 keeps ticking at full rate and every real-time counter stays clean.

    Nothing else catches it. It builds, it lints, it passes the layout gate,
    and attaching a probe to investigate drains the buffer and releases the
    block, so the fault disappears on inspection and a reset destroys the
    evidence. It presents only as a rig that intermittently vanishes from the
    network while apparently healthy.
    """
    names = {package["id"]: package["name"] for package in data["packages"]}
    nodes = [
        node for node in data["resolve"]["nodes"] if names[node["id"]] == "defmt-rtt"
    ]
    for node in nodes:
        if "disable-blocking-mode" not in set(node.get("features", [])):
            raise SystemExit(
                "defmt-rtt is resolved without `disable-blocking-mode`, so a "
                "detached debugger will stall the core once the RTT buffer "
                "fills. Declare it as:\n"
                '  defmt-rtt = { version = "1.3.0", '
                'features = ["disable-blocking-mode"] }'
            )


def load_policy(path: Path) -> dict[str, Any]:
    try:
        with path.open("rb") as stream:
            policy = tomllib.load(stream)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise SystemExit(f"cannot load dependency policy {path}: {error}") from error
    if policy.get("schema_version") != 1:
        raise SystemExit(f"{path}: dependency policy requires schema_version = 1")
    return policy


def check_external_policy(
    data: dict[str, Any], workspace: Path, policy_path: Path
) -> None:
    policy = load_policy(policy_path)
    packages = {package["name"]: package for package in data["packages"]}
    for rule in policy.get("exact", []):
        package_name = rule["package"]
        try:
            package = packages[package_name]
        except KeyError:
            raise SystemExit(
                f"dependency policy package not found: {package_name}"
            ) from None
        require_exact(package, set(rule["dependencies"]))
    for rule in policy.get("forbidden_source", []):
        require_absent_from_sources(workspace / rule["path"], set(rule["dependencies"]))
    for rule in policy.get("unreachable", []):
        reachable = reachable_names(data, rule["root"])
        if overlap := reachable & set(rule["dependencies"]):
            raise SystemExit(
                f"{rule['root']} reaches forbidden dependencies: {sorted(overlap)}"
            )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--workspace",
        type=Path,
        default=Path.cwd(),
        help="Cargo workspace to inspect (default: the current directory)",
    )
    parser.add_argument("--policy", type=Path)
    args = parser.parse_args()

    data = metadata(args.workspace)
    # Applies to every workspace, with or without a policy file, so a new rig
    # inherits it without having to know it exists.
    require_nonblocking_rtt(data)
    if args.policy is None:
        check_default_policy(data, args.workspace)
    else:
        check_external_policy(data, args.workspace, args.policy)
    print("crate dependency rules passed")


if __name__ == "__main__":
    main()
