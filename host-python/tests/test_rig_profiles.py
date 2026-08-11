"""Tests for rig-owned layout and hardware-regression profiles."""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

from helic_daq.verify import discover_profiles, layout, regression
from helic_daq.verify.profile import ProfileError, load_profile, load_profiles

REPOSITORY = Path(__file__).resolve().parents[2]
FIRMWARE = REPOSITORY / "firmware"
PROFILE_PATHS = discover_profiles(FIRMWARE)


class ProfileLoadingTests(unittest.TestCase):
    def test_production_profiles_are_rig_owned_data(self) -> None:
        profiles = load_profiles(PROFILE_PATHS)

        self.assertEqual(set(profiles), {"cbc", "pico2w"})
        self.assertEqual(profiles["cbc"].regression.max_loop_us, 60)
        self.assertEqual(profiles["cbc"].regression.capture_sources[0], "adc0")
        self.assertIsNone(profiles["pico2w"].regression.default_host)
        self.assertIn(
            ("table_buffer", "Active", "3get"),
            profiles["cbc"].layout.required_patterns,
        )

    def test_duplicate_names_are_rejected(self) -> None:
        with self.assertRaisesRegex(ProfileError, "duplicate profile name"):
            load_profiles([PROFILE_PATHS[0], PROFILE_PATHS[0]])

    def test_quiet_write_requires_exactly_one_value_form(self) -> None:
        source = (
            PROFILE_PATHS[0]
            .read_text()
            .replace(
                'name = "forcing_coeffs"\nzeros = true',
                'name = "forcing_coeffs"\nzeros = true\nvalue = 0',
            )
        )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "invalid.toml"
            path.write_text(source)
            with self.assertRaisesRegex(ProfileError, "exactly one"):
                load_profile(path)


class LayoutProfileTests(unittest.TestCase):
    def test_checker_uses_profile_patterns_and_exact_symbols(self) -> None:
        profile = load_profile(
            FIRMWARE / "experiments" / "pico2w-rig" / "rig-profile.toml"
        )
        realised = [
            (layout.SRAM_START, "prefix" + "_".join(pattern))
            for pattern in (
                profile.layout.required_patterns + profile.layout.optional_patterns
            )
        ]
        realised.extend(
            (layout.SRAM_START, symbol)
            for symbol in profile.layout.exact_symbols
        )
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / profile.layout.elf).touch()
            with mock.patch.object(layout, "symbols", return_value=realised):
                self.assertEqual(
                    layout.check_elf(profile, Path(directory), "nm"), []
                )

    def test_checker_rejects_a_profiled_symbol_in_flash(self) -> None:
        profile = load_profile(
            FIRMWARE / "experiments" / "cbc-rig" / "rig-profile.toml"
        )
        realised = [
            (layout.SRAM_START, "prefix" + "_".join(pattern))
            for pattern in profile.layout.required_patterns
        ]
        realised.extend(
            (layout.SRAM_START, symbol)
            for symbol in profile.layout.exact_symbols
        )
        realised[0] = (0x1000_0000, realised[0][1])
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / profile.layout.elf).touch()
            with mock.patch.object(layout, "symbols", return_value=realised):
                errors = layout.check_elf(profile, Path(directory), "nm")
        self.assertTrue(any("outside SRAM" in error for error in errors))

    def test_checker_rejects_an_optional_hot_symbol_when_emitted_in_flash(self) -> None:
        profile = load_profile(
            FIRMWARE / "experiments" / "cbc-rig" / "rig-profile.toml"
        )
        realised = [
            (layout.SRAM_START, "prefix" + "_".join(pattern))
            for pattern in profile.layout.required_patterns
        ]
        realised.extend(
            (layout.SRAM_START, symbol)
            for symbol in profile.layout.exact_symbols
        )
        realised.append((0x1000_0000, "prefix_run_rt_tick"))
        with tempfile.TemporaryDirectory() as directory:
            (Path(directory) / profile.layout.elf).touch()
            with mock.patch.object(layout, "symbols", return_value=realised):
                errors = layout.check_elf(profile, Path(directory), "nm")
        self.assertTrue(any("run_rt_tick" in error for error in errors))


class BoardSelectionTests(unittest.TestCase):
    """A rig's default build, not the tool, decides which controller is flashed."""

    def profile_with_board(self, default_board: str | None) -> object:
        source = load_profile(
            FIRMWARE / "experiments" / "cbc-rig" / "rig-profile.toml"
        ).path.read_text()
        if default_board is not None:
            source = source.replace(
                "wired = true", f'wired = true\ndefault_board = "{default_board}"'
            )
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "board.toml"
            path.write_text(source)
            return load_profile(path)

    def flash_command(self, profile: object, board: str) -> list[str]:
        recorded: list[list[str]] = []

        def fake_run(cmd, firmware_dir, timeout=None):  # type: ignore[no-untyped-def]
            recorded.append(cmd)
            return SimpleNamespace(stdout="")

        with mock.patch.object(regression, "run", fake_run):
            regression.flash(profile, board, 1.0, Path("."))  # type: ignore[arg-type]
        return recorded[0]

    def test_default_board_defaults_to_w5500(self) -> None:
        profile = self.profile_with_board(None)
        self.assertEqual(profile.regression.default_board, "w5500")  # type: ignore[attr-defined]

    def test_matching_board_uses_the_default_build(self) -> None:
        profile = self.profile_with_board("w6100")
        self.assertNotIn("--no-default-features", self.flash_command(profile, "w6100"))

    def test_other_board_is_selected_explicitly(self) -> None:
        profile = self.profile_with_board("w6100")
        command = self.flash_command(profile, "w5500")
        self.assertIn("--no-default-features", command)
        self.assertIn("board-w5500", command)

    def test_unknown_board_is_rejected(self) -> None:
        with self.assertRaisesRegex(ProfileError, "default_board"):
            self.profile_with_board("w5100")


class RegressionProfileTests(unittest.TestCase):
    def test_quiet_sequence_is_driven_by_profile(self) -> None:
        profile = load_profile(
            FIRMWARE / "experiments" / "cbc-rig" / "rig-profile.toml"
        )

        class FakeDevice:
            def __init__(self) -> None:
                self.writes: list[tuple[str, object]] = []

            def param(self, _name: str) -> SimpleNamespace:
                return SimpleNamespace(count=3)

            def set(self, name: str, value: object) -> None:
                self.writes.append((name, value))

        device = FakeDevice()
        regression.quiet_outputs(device, profile.regression)  # type: ignore[arg-type]

        self.assertEqual(
            device.writes,
            [
                ("forcing_coeffs", [0.0, 0.0, 0.0]),
                ("target_coeffs", [0.0, 0.0, 0.0]),
                ("table_mode", 0),
            ],
        )


if __name__ == "__main__":
    unittest.main()
