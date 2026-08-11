#!/usr/bin/env python3
"""Enforce the HELIC-DAQ crate-layering rules from Cargo metadata and source."""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import Any

FIRMWARE = Path(__file__).resolve().parents[1]


def metadata() -> dict[str, Any]:
    output = subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1"],
        cwd=FIRMWARE,
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
    root = next(package_id for package_id, name in packages.items() if name == root_name)
    pending = [root]
    visited: set[str] = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        pending.extend(nodes[package_id]["dependencies"])
    return {packages[package_id] for package_id in visited}


def main() -> None:
    data = metadata()
    packages = {package["name"]: package for package in data["packages"]}

    require_exact(packages["helic-core"], {"libm"})
    require_exact(packages["helic-rt"], {"heapless", "helic-core", "helic-proto"})

    forbidden_rt = {"embassy-executor", "embassy-net", "embassy-time", "helic-fw-support"}
    direct_rt = normal_dependencies(packages["helic-fw-rt"])
    if overlap := direct_rt & forbidden_rt:
        raise SystemExit(f"helic-fw-rt has forbidden dependencies: {sorted(overlap)}")
    require_absent_from_sources(FIRMWARE / "rt" / "src", forbidden_rt)

    support_reachable = reachable_names(data, "helic-fw-support")
    if "helic-fw-rt" in support_reachable:
        raise SystemExit("helic-fw-support must not depend on helic-fw-rt")

    print("crate dependency rules passed")


if __name__ == "__main__":
    main()
