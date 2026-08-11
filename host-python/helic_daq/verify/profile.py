"""Load and validate rig-owned real-time verification profiles."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import TypeAlias

import tomllib

PROFILE_SCHEMA_VERSION = 1
#: Ethernet controllers a wired rig may select through its board features.
BOARDS = frozenset({"w5500", "w6100"})
Scalar: TypeAlias = bool | int | float | str


class ProfileError(ValueError):
    """A rig profile is missing required data or contains invalid data."""


@dataclass(frozen=True)
class LayoutProfile:
    """Named ELF and symbols which must be realised in SRAM."""

    elf: str
    required_patterns: tuple[tuple[str, ...], ...]
    optional_patterns: tuple[tuple[str, ...], ...]
    exact_symbols: tuple[str, ...]


@dataclass(frozen=True)
class QuietWrite:
    """One ordered write used to place a rig in its quiet state."""

    name: str
    value: Scalar | None = None
    zeros: bool = False


@dataclass(frozen=True)
class RegressionProfile:
    """Hardware identity, capture, acceptance, and quieting settings."""

    sample_rate_hz: int
    default_host: str | None
    capture_sources: tuple[str, ...]
    wired: bool
    #: Ethernet controller the rig's *default* build targets. The runner adds
    #: explicit board features only when asked for a different one, so a rig
    #: that defaults to W6100 is flashed correctly without the tool knowing
    #: anything about that rig.
    default_board: str
    max_loop_us: int | None
    quiet: tuple[QuietWrite, ...]


@dataclass(frozen=True)
class RigProfile:
    """A complete rig-owned static and hardware verification contract."""

    path: Path
    name: str
    package: str
    experiment: str
    layout: LayoutProfile
    regression: RegressionProfile


def _table(value: object, field: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ProfileError(f"{field} must be a TOML table")
    return value


def _string(value: object, field: str) -> str:
    if not isinstance(value, str) or not value:
        raise ProfileError(f"{field} must be a non-empty string")
    return value


def _integer(value: object, field: str, *, minimum: int = 1) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < minimum:
        raise ProfileError(f"{field} must be an integer >= {minimum}")
    return value


def _optional_integer(value: object, field: str) -> int | None:
    if value is None:
        return None
    return _integer(value, field)


def _strings(value: object, field: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not value:
        raise ProfileError(f"{field} must be a non-empty array of strings")
    return tuple(_string(item, f"{field}[]") for item in value)


def _patterns(
    value: object, field: str, *, allow_empty: bool = False
) -> tuple[tuple[str, ...], ...]:
    if not isinstance(value, list) or (not value and not allow_empty):
        raise ProfileError(f"{field} must be a non-empty array of string arrays")
    patterns = []
    for item in value:
        if not isinstance(item, list) or not item:
            raise ProfileError(f"{field}[] must be a non-empty array of strings")
        patterns.append(tuple(_string(part, f"{field}[][]") for part in item))
    return tuple(patterns)


def _quiet_writes(value: object, field: str) -> tuple[QuietWrite, ...]:
    if not isinstance(value, list) or not value:
        raise ProfileError(f"{field} must be a non-empty array of tables")
    writes = []
    for index, raw in enumerate(value):
        entry = _table(raw, f"{field}[{index}]")
        name = _string(entry.get("name"), f"{field}[{index}].name")
        zeros = entry.get("zeros", False)
        if not isinstance(zeros, bool):
            raise ProfileError(f"{field}[{index}].zeros must be a boolean")
        has_value = "value" in entry
        if zeros == has_value:
            raise ProfileError(
                f"{field}[{index}] must set exactly one of 'zeros = true' or 'value'"
            )
        scalar = entry.get("value")
        if has_value and not isinstance(scalar, (bool, int, float, str)):
            raise ProfileError(f"{field}[{index}].value must be a TOML scalar")
        writes.append(QuietWrite(name=name, value=scalar, zeros=zeros))
    return tuple(writes)


def load_profile(path: str | Path) -> RigProfile:
    """Load one schema-versioned profile and reject incomplete contracts."""

    profile_path = Path(path)
    try:
        with profile_path.open("rb") as stream:
            root = tomllib.load(stream)
    except (OSError, tomllib.TOMLDecodeError) as error:
        raise ProfileError(f"cannot load {profile_path}: {error}") from error

    schema_version = _integer(root.get("schema_version"), "schema_version")
    if schema_version != PROFILE_SCHEMA_VERSION:
        raise ProfileError(
            f"unsupported schema_version {schema_version}; expected {PROFILE_SCHEMA_VERSION}"
        )

    layout_raw = _table(root.get("layout"), "layout")
    regression_raw = _table(root.get("regression"), "regression")
    default_host = regression_raw.get("default_host")
    if default_host is not None:
        default_host = _string(default_host, "regression.default_host")
    wired = regression_raw.get("wired")
    if not isinstance(wired, bool):
        raise ProfileError("regression.wired must be a boolean")
    default_board = regression_raw.get("default_board", "w5500")
    if default_board not in BOARDS:
        raise ProfileError(
            f"regression.default_board must be one of {sorted(BOARDS)}; "
            f"found {default_board!r}"
        )

    return RigProfile(
        path=profile_path,
        name=_string(root.get("name"), "name"),
        package=_string(root.get("package"), "package"),
        experiment=_string(root.get("experiment"), "experiment"),
        layout=LayoutProfile(
            elf=_string(layout_raw.get("elf"), "layout.elf"),
            required_patterns=_patterns(
                layout_raw.get("required_patterns"), "layout.required_patterns"
            ),
            optional_patterns=_patterns(
                layout_raw.get("optional_patterns", []),
                "layout.optional_patterns",
                allow_empty=True,
            ),
            exact_symbols=_strings(
                layout_raw.get("exact_symbols"), "layout.exact_symbols"
            ),
        ),
        regression=RegressionProfile(
            sample_rate_hz=_integer(
                regression_raw.get("sample_rate_hz"), "regression.sample_rate_hz"
            ),
            default_host=default_host,
            capture_sources=_strings(
                regression_raw.get("capture_sources"), "regression.capture_sources"
            ),
            wired=wired,
            default_board=default_board,
            max_loop_us=_optional_integer(
                regression_raw.get("max_loop_us"), "regression.max_loop_us"
            ),
            quiet=_quiet_writes(regression_raw.get("quiet"), "regression.quiet"),
        ),
    )


def load_profiles(paths: list[Path]) -> dict[str, RigProfile]:
    """Load profiles keyed by unique command-line name."""

    profiles: dict[str, RigProfile] = {}
    for path in paths:
        profile = load_profile(path)
        if profile.name in profiles:
            raise ProfileError(
                f"duplicate profile name {profile.name!r}: "
                f"{profiles[profile.name].path} and {profile.path}"
            )
        profiles[profile.name] = profile
    if not profiles:
        raise ProfileError("no rig profiles found")
    return profiles
