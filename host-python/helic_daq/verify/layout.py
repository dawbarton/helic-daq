"""Fail when named real-time firmware symbols are linked outside RP2350 SRAM."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path

from . import RELEASE_DIR, discover_profiles
from .profile import ProfileError, RigProfile, load_profiles

SRAM_START = 0x2000_0000
SRAM_END = 0x2008_2000


def matches(pattern: tuple[str, ...], name: str) -> bool:
    return all(part in name for part in pattern)


def symbols(elf: Path, nm: str) -> list[tuple[int, str]]:
    output = subprocess.run(
        [nm, "-n", str(elf)],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
    ).stdout
    parsed: list[tuple[int, str]] = []
    for line in output.splitlines():
        fields = line.split(maxsplit=2)
        if len(fields) != 3:
            continue
        address, _kind, name = fields
        try:
            parsed.append((int(address, 16), name))
        except ValueError:
            continue
    return parsed


def check_elf(profile: RigProfile, elf_dir: Path, nm: str) -> list[str]:
    elf = elf_dir / profile.layout.elf
    if not elf.is_file():
        return [f"{profile.name}: release ELF not found at {elf}"]

    found = symbols(elf, nm)
    errors: list[str] = []
    for pattern in profile.layout.required_patterns:
        realised = [
            (address, name)
            for address, name in found
            if matches(pattern, name) and "Thunk" not in name
        ]
        if not realised:
            errors.append(
                f"{profile.name}: required symbol pattern {pattern!r} is absent"
            )
        for address, name in realised:
            if not SRAM_START <= address < SRAM_END:
                errors.append(
                    f"{profile.name}: hot symbol at 0x{address:08x} is outside SRAM: {name}"
                )
    for pattern in profile.layout.optional_patterns:
        for address, name in found:
            if (
                matches(pattern, name)
                and "Thunk" not in name
                and not SRAM_START <= address < SRAM_END
            ):
                errors.append(
                    f"{profile.name}: hot symbol at 0x{address:08x} is outside SRAM: {name}"
                )
    for symbol in profile.layout.exact_symbols:
        realised = [(address, name) for address, name in found if name == symbol]
        if not realised:
            errors.append(f"{profile.name}: required exact symbol {symbol!r} is absent")
        for address, name in realised:
            if not SRAM_START <= address < SRAM_END:
                errors.append(
                    f"{profile.name}: hot symbol at 0x{address:08x} is outside SRAM: {name}"
                )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--elf-dir",
        type=Path,
        default=RELEASE_DIR,
        help="directory holding release ELFs (default: the release directory "
        "of the workspace in the current directory)",
    )
    parser.add_argument("--nm", default="nm", help="nm-compatible executable")
    parser.add_argument(
        "--profile",
        action="append",
        type=Path,
        help="rig profile to check (repeatable; default: profiles discovered "
        "below the current directory)",
    )
    args = parser.parse_args()

    paths = args.profile or discover_profiles()
    if not paths:
        parser.error(
            "no rig profile found below the current directory; pass --profile"
        )
    try:
        profiles = load_profiles(paths)
    except ProfileError as error:
        parser.error(str(error))
    errors = [
        error
        for profile in profiles.values()
        for error in check_elf(profile, args.elf_dir, args.nm)
    ]
    if errors:
        for error in errors:
            print(error)
        return 1

    print(f"real-time layout check passed for {', '.join(profiles)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
