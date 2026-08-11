"""Fail when named real-time firmware symbols are linked outside RP2350 SRAM."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path

FIRMWARE = Path(__file__).resolve().parents[1]
DEFAULT_ELF_DIR = FIRMWARE / "target" / "thumbv8m.main-none-eabihf" / "release"
SRAM_START = 0x2000_0000
SRAM_END = 0x2008_2000

# These names deliberately match stable source identifiers rather than complete
# Rust symbols. Generic instantiations are mangled, while the source identifiers
# remain visible in `nm` output. Linker-generated flash-to-SRAM thunks are
# ignored: SRAM callers reach these definitions directly, so the veneers are
# not part of the tick call graph.
HOT_SYMBOLS = (
    ("run_hot_loop",),
    ("run_rt_tick",),
    ("run_reboot_quiesce",),
    ("reboot_quiesce_step",),
    ("prepare_reboot",),
    ("transfer_in_place",),
    ("apply_program",),
    ("set_rig_param",),
    ("measure_rig",),
    ("step_program",),
    ("program_fault",),
    ("safety_decide",),
    ("actuate_rig",),
    ("write_program_signals",),
    ("table_buffer", "Active", "3get"),
    ("table_buffer", "Active", "8activate"),
)

EABI_HOT_SYMBOLS = (
    "__aeabi_memcpy",
    "__aeabi_memcpy4",
    "__aeabi_memcpy8",
    "__aeabi_memclr",
    "__aeabi_memclr4",
    "__aeabi_memclr8",
)

REQUIRED_SYMBOLS = {
    "fw-cbc-rig": (
        ("run_hot_loop",),
        ("run_reboot_quiesce",),
        ("transfer_in_place",),
        ("apply_program",),
        ("set_rig_param",),
        ("measure_rig",),
        ("step_program",),
        ("program_fault",),
        ("safety_decide",),
        ("actuate_rig",),
        ("write_program_signals",),
        ("table_buffer", "Active", "3get"),
        ("table_buffer", "Active", "8activate"),
        *((symbol,) for symbol in EABI_HOT_SYMBOLS),
    ),
    "fw-whirl-rig": (
        ("run_hot_loop",),
        ("run_reboot_quiesce",),
        ("apply_program",),
        ("set_rig_param",),
        ("measure_rig",),
        ("step_program",),
        ("actuate_rig",),
        ("write_program_signals",),
        ("table_buffer", "Active", "3get"),
        ("table_buffer", "Active", "8activate"),
        *((symbol,) for symbol in EABI_HOT_SYMBOLS),
    ),
    "fw-pico2w-rig": (
        ("run_hot_loop",),
        ("run_reboot_quiesce",),
        ("transfer_in_place",),
        ("apply_program",),
        ("set_rig_param",),
        ("measure_rig",),
        ("step_program",),
        ("actuate_rig",),
        ("write_program_signals",),
        ("table_buffer", "Active", "3get"),
        ("table_buffer", "Active", "8activate"),
        *((symbol,) for symbol in EABI_HOT_SYMBOLS),
    ),
}


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


def check_elf(package: str, elf_dir: Path, nm: str) -> list[str]:
    elf = elf_dir / package
    if not elf.is_file():
        return [f"{package}: release ELF not found at {elf}"]

    found = symbols(elf, nm)
    errors: list[str] = []
    for required in REQUIRED_SYMBOLS[package]:
        if len(required) == 1 and required[0] in EABI_HOT_SYMBOLS:
            present = any(required[0] == name for _, name in found)
        else:
            present = any(matches(required, name) and "Thunk" not in name for _, name in found)
        if not present:
            errors.append(f"{package}: required symbol pattern {required!r} is absent")

    for address, name in found:
        is_hot = name in EABI_HOT_SYMBOLS or any(matches(pattern, name) for pattern in HOT_SYMBOLS)
        if "Thunk" in name or not is_hot:
            continue
        if not SRAM_START <= address < SRAM_END:
            errors.append(f"{package}: hot symbol at 0x{address:08x} is outside SRAM: {name}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--elf-dir", type=Path, default=DEFAULT_ELF_DIR)
    parser.add_argument("--nm", default="nm", help="nm-compatible executable")
    args = parser.parse_args()

    errors = [
        error
        for package in REQUIRED_SYMBOLS
        for error in check_elf(package, args.elf_dir, args.nm)
    ]
    if errors:
        for error in errors:
            print(error)
        return 1

    print("real-time layout check passed for cbc-rig, whirl-rig, and pico2w-rig")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
