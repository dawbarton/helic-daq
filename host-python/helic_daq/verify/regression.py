"""Flash and exercise one production rig's real-time path sequentially.

The control server is single-client. This tool therefore performs firmware
flashing, idle measurement, TCP polling, and UDP capture in strict order and
fails if common real-time acceptance criteria regress.
"""

from __future__ import annotations

import argparse
import itertools
import json
import os
import subprocess
import sys
import time
from pathlib import Path

from helic_daq import Device, protocol

from . import discover_profiles
from .profile import (
    ProfileError,
    RegressionProfile,
    RigProfile,
    load_profiles,
)

COUNTERS = (
    "ticks",
    "loop_time_last",
    "loop_time_max",
    "clock_jitter",
    "overruns",
    "tick_timeouts",
    "records_dropped",
    "cmd_backlog_max",
)
PHASE_COUNTERS = (
    "wake_phase_min",
    "wake_phase_max",
    "t_measure_max",
    "t_actuate_max",
    "t_rest_max",
)


def run(
    cmd: list[str], firmware_dir: Path, timeout: float | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=firmware_dir,
        check=True,
        timeout=timeout,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


def flash(profile: RigProfile, board: str, timeout: float, firmware_dir: Path) -> str:
    cmd = ["cargo", "run", "--release", "-p", profile.package]
    # Only override when the requested controller differs from the one the
    # rig's default build already targets. Assuming a default here would flash
    # the wrong Ethernet image at a rig that defaults to the other controller.
    if profile.regression.wired and board != profile.regression.default_board:
        cmd.extend(["--no-default-features", "--features", f"board-{board}"])
    try:
        return run(cmd, firmware_dir, timeout=timeout).stdout
    except subprocess.TimeoutExpired as error:
        # `cargo run` remains attached to defmt after flashing. A timeout is the
        # normal way to detach before opening the single host connection.
        output = error.stdout or ""
        return output.decode(errors="replace") if isinstance(output, bytes) else output


def connect(host: str, deadline_s: float) -> Device:
    deadline = time.monotonic() + deadline_s
    last_error: Exception | None = None
    while time.monotonic() < deadline:
        try:
            return Device(host)
        except (ConnectionError, OSError) as error:
            last_error = error
            time.sleep(0.25)
    raise RuntimeError(f"could not connect to {host}: {last_error}")


def snapshot(device: Device) -> dict[str, int]:
    return dict(zip(COUNTERS, device.get(*COUNTERS)))


def reset_diagnostics(device: Device) -> None:
    device.set("diag_reset", 1)


def phase_snapshot(device: Device) -> dict[str, int]:
    return dict(zip(PHASE_COUNTERS, device.get(*PHASE_COUNTERS)))


def delta(
    before: dict[str, int], after: dict[str, int], elapsed_s: float
) -> dict[str, float]:
    ticks = after["ticks"] - before["ticks"]
    return {
        "elapsed_s": elapsed_s,
        "ticks": ticks,
        "ticks_per_s": ticks / elapsed_s,
        "overruns": after["overruns"] - before["overruns"],
        "tick_timeouts": after["tick_timeouts"] - before["tick_timeouts"],
        "records_dropped": after["records_dropped"] - before["records_dropped"],
        "loop_time_last": after["loop_time_last"],
        "loop_time_max": after["loop_time_max"],
        "clock_jitter": after["clock_jitter"],
        "cmd_backlog_max": after["cmd_backlog_max"],
    }


def quiet_outputs(device: Device, profile: RegressionProfile) -> None:
    for write in profile.quiet:
        if write.zeros:
            value: object = [0.0] * device.param(write.name).count
        else:
            value = write.value
        device.set(write.name, value)


def measure_phase(
    device: Device, seconds: float, poll_interval: float | None
) -> dict[str, object]:
    reset_diagnostics(device)
    before = snapshot(device)
    started = time.monotonic()
    polls = 0
    if poll_interval is None:
        time.sleep(seconds)
    else:
        while time.monotonic() - started < seconds:
            snapshot(device)
            polls += 1
            time.sleep(poll_interval)
    result: dict[str, object] = delta(
        before, snapshot(device), time.monotonic() - started
    )
    result["phase"] = phase_snapshot(device)
    if poll_interval is not None:
        result["polls"] = polls
    return result


def acceptance_errors(
    result: dict[str, object], profile: RegressionProfile
) -> list[str]:
    errors: list[str] = []
    for phase_name in ("idle", "poll", "capture"):
        phase = result[phase_name]
        if not isinstance(phase, dict):
            continue
        for counter in ("overruns", "tick_timeouts", "records_dropped", "clock_jitter"):
            if phase[counter] != 0:
                errors.append(f"{phase_name}: {counter}={phase[counter]}")
        # Capture setup and teardown are deliberately inside its measurement
        # window, so only the fixed-duration phases are suitable rate checks.
        if phase_name != "capture":
            rate = float(phase["ticks_per_s"])
            if not (
                profile.sample_rate_hz * 0.98 <= rate <= profile.sample_rate_hz * 1.02
            ):
                errors.append(
                    f"{phase_name}: ticks_per_s={rate:.1f}, "
                    f"expected {profile.sample_rate_hz}"
                )
        if (
            profile.max_loop_us is not None
            and phase["loop_time_max"] > profile.max_loop_us
        ):
            errors.append(
                f"{phase_name}: loop_time_max={phase['loop_time_max']} us, "
                f"limit {profile.max_loop_us} us"
            )
        timing = phase["phase"]
        if (
            isinstance(timing, dict)
            and timing["wake_phase_max"] - timing["wake_phase_min"] > 2
        ):
            errors.append(f"{phase_name}: wake phase spread exceeds 2 us")
    capture = result["capture"]
    if isinstance(capture, dict):
        for counter in ("lost_packets", "capture_dropped", "index_gaps"):
            if capture[counter] != 0:
                errors.append(f"capture: {counter}={capture[counter]}")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group()
    selection.add_argument(
        "--rig", default="pico2w", help="discovered production profile name"
    )
    selection.add_argument("--profile", type=Path, help="path to a rig-owned profile")
    parser.add_argument("--host", default=os.environ.get("HELIC_DAQ_HOST"))
    parser.add_argument(
        "--board",
        choices=("w5500", "w6100"),
        help="Ethernet controller to flash (default: the profile's "
        "regression.default_board)",
    )
    parser.add_argument(
        "--firmware-dir",
        type=Path,
        default=Path.cwd(),
        help="Cargo workspace used for flashing (default: the current directory)",
    )
    parser.add_argument("--no-flash", action="store_true")
    parser.add_argument("--idle-seconds", type=float, default=5.0)
    parser.add_argument("--poll-seconds", type=float, default=5.0)
    parser.add_argument("--poll-interval", type=float, default=0.05)
    parser.add_argument("--capture-samples", type=int, default=8_000)
    parser.add_argument(
        "--capture-sources",
        help="comma-separated source names, or 'all' (default: rig smoke-test set)",
    )
    parser.add_argument("--flash-timeout", type=float, default=10.0)
    parser.add_argument("--connect-timeout", type=float, default=15.0)
    args = parser.parse_args()

    try:
        if args.profile is not None:
            profiles = load_profiles([args.profile])
            profile = next(iter(profiles.values()))
        else:
            paths = discover_profiles()
            if not paths:
                parser.error(
                    "no rig profile found below the current directory; "
                    "pass --profile"
                )
            profiles = load_profiles(paths)
            try:
                profile = profiles[args.rig]
            except KeyError:
                parser.error(
                    f"unknown rig profile {args.rig!r}; choose from {', '.join(profiles)}"
                )
    except ProfileError as error:
        parser.error(str(error))

    regression = profile.regression
    host = args.host or regression.default_host
    if host is None:
        parser.error(
            "--host or HELIC_DAQ_HOST is required when the profile has no host"
        )
    board = args.board or regression.default_board
    if not regression.wired and args.board is not None:
        parser.error("--board applies only to wired rigs")

    result: dict[str, object] = {
        "rig": profile.name,
        "package": profile.package,
        "host": host,
    }
    if not args.no_flash:
        result["board"] = board
        result["flash_tail"] = flash(
            profile, board, args.flash_timeout, args.firmware_dir
        ).splitlines()[-12:]

    with connect(host, args.connect_timeout) as device:
        status = device.status()
        result["status"] = status
        experiment = device.get("experiment")
        result["experiment"] = experiment
        if experiment != profile.experiment:
            raise RuntimeError(
                f"connected to {experiment!r}, expected {profile.experiment!r}"
            )
        quiet_outputs(device, regression)
        result["idle"] = measure_phase(device, args.idle_seconds, None)
        result["poll"] = measure_phase(device, args.poll_seconds, args.poll_interval)

        reset_diagnostics(device)
        before = snapshot(device)
        started = time.monotonic()
        if args.capture_sources == "all":
            capture_sources = [source.name for source in device.sources]
        elif args.capture_sources:
            capture_sources = args.capture_sources.split(",")
        else:
            capture_sources = list(regression.capture_sources)
        capture = device.capture(
            capture_sources,
            samples=args.capture_samples,
            port=protocol.STREAM_PORT,
        )
        captured = delta(before, snapshot(device), time.monotonic() - started)
        indices = capture["index"]
        captured.update(
            records=len(indices),
            lost_packets=int(capture["lost_packets"]),
            capture_dropped=int(capture["dropped"]),
            index_gaps=sum(
                int(b) != int(a) + 1 for a, b in itertools.pairwise(indices)
            ),
            sources=capture_sources,
            phase=phase_snapshot(device),
        )
        result["capture"] = captured
        quiet_outputs(device, regression)

    errors = acceptance_errors(result, regression)
    result["acceptance_errors"] = errors
    print(json.dumps(result, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
