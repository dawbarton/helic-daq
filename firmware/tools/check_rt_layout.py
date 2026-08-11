"""Fail when named real-time firmware symbols are linked outside RP2350 SRAM."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path

from rig_profile import ProfileError, RigProfile, load_profiles

FIRMWARE = Path(__file__).resolve().parents[1]
DEFAULT_ELF_DIR = FIRMWARE / "target" / "thumbv8m.main-none-eabihf" / "release"
DEFAULT_PROFILE_PATHS = sorted((FIRMWARE / "experiments").glob("*/rig-profile.toml"))
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
    parser.add_argument("--elf-dir", type=Path, default=DEFAULT_ELF_DIR)
    parser.add_argument("--nm", default="nm", help="nm-compatible executable")
    parser.add_argument(
        "--profile",
        action="append",
        type=Path,
        help="rig profile to check (repeatable; default: production profiles)",
    )
    args = parser.parse_args()

    try:
        profiles = load_profiles(args.profile or DEFAULT_PROFILE_PATHS)
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
