"""Drive the shared regression runner from the fixture's local profile."""

from __future__ import annotations

import contextlib
import io
import json
import sys
import unittest
from pathlib import Path
from types import SimpleNamespace
from typing import Self
from unittest import mock

from helic_daq.verify import regression

FIXTURE = Path(__file__).resolve().parent


class FakeClock:
    def __init__(self) -> None:
        self.now = 0.0

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.now += seconds


class FakeDevice:
    def __init__(self, clock: FakeClock) -> None:
        self.clock = clock
        self.writes: list[tuple[str, object]] = []
        self.sources = [SimpleNamespace(name="sense"), SimpleNamespace(name="drive")]

    def __enter__(self) -> Self:
        return self

    def __exit__(self, *_exc: object) -> None:
        return None

    def status(self) -> dict[str, object]:
        return {
            "protocol_version": 3,
            "n_params": 1,
            "n_sources": 2,
            "sample_rate": 1000.0,
            "uptime_s": self.clock.now,
        }

    def get(self, *names: str) -> object:
        ticks = int(self.clock.now * 1000.0)
        values: dict[str, object] = {
            "experiment": "fixture-rig",
            "ticks": ticks,
            "loop_time_last": 100,
            "loop_time_max": 100,
            "clock_jitter": 0,
            "overruns": 0,
            "tick_timeouts": 0,
            "records_dropped": 0,
            "cmd_backlog_max": 0,
            "wake_phase_min": 10,
            "wake_phase_max": 10,
            "t_measure_max": 2,
            "t_actuate_max": 1,
            "t_rest_max": 97,
        }
        selected = [values[name] for name in names]
        return selected[0] if len(selected) == 1 else selected

    def set(self, name: str, value: object) -> None:
        self.writes.append((name, value))

    def capture(
        self, sources: list[str], *, samples: int, port: int
    ) -> dict[str, object]:
        self.clock.sleep(samples / 1000.0)
        return {
            "index": list(range(samples)),
            "lost_packets": 0,
            "dropped": 0,
            "sources": sources,
            "port": port,
        }


class RegressionDryRunTests(unittest.TestCase):
    def test_local_profile_drives_unmodified_runner(self) -> None:
        clock = FakeClock()
        device = FakeDevice(clock)
        argv = [
            "helic-rt-regression",
            "--profile",
            str(FIXTURE / "rig-profile.toml"),
            "--no-flash",
            "--idle-seconds",
            "0.1",
            "--poll-seconds",
            "0.1",
            "--poll-interval",
            "0.02",
            "--capture-samples",
            "8",
        ]
        output = io.StringIO()
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(regression, "connect", return_value=device),
            mock.patch.object(regression.time, "monotonic", clock.monotonic),
            mock.patch.object(regression.time, "sleep", clock.sleep),
            contextlib.redirect_stdout(output),
        ):
            result = regression.main()

        report = json.loads(output.getvalue())
        self.assertEqual(result, 0)
        self.assertEqual(report["rig"], "fixture")
        self.assertEqual(report["experiment"], "fixture-rig")
        self.assertEqual(report["capture"]["records"], 8)
        self.assertEqual(report["acceptance_errors"], [])
        self.assertEqual(
            [write for write in device.writes if write[0] == "drive_enable"],
            [("drive_enable", 0), ("drive_enable", 0)],
        )


if __name__ == "__main__":
    unittest.main()
