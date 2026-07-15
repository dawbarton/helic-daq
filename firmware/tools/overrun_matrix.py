"""Run sequential CBC hardware overrun-isolation firmware variants.

The script builds and flashes one `fw-cbc-rig` variant at a time, then uses a
single host connection to collect counter deltas for idle, TCP polling, and,
when supported by the variant, a short UDP capture. It is intended for
hardware sessions on the W5500 CBC rig.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

from helic_daq import Device
from helic_daq import protocol


ROOT = Path(__file__).resolve().parents[2]
FIRMWARE = ROOT / "firmware"
BASE_FEATURES = ("board-w5500",)
COUNTERS = (
    "ticks",
    "loop_time_last",
    "loop_time_max",
    "clock_jitter",
    "overruns",
    "tick_timeouts",
    "records_dropped",
    "adc_errors",
)


@dataclass(frozen=True)
class Variant:
    name: str
    features: tuple[str, ...]
    capture: bool = True


VARIANTS = (
    Variant("baseline", ()),
    Variant("no_status_log", ("diag-no-status-log",)),
    Variant("no_udp_task", ("diag-no-udp",), capture=False),
    Variant("sample_4k", ("diag-sample-4k",)),
    Variant("skip_adc", ("diag-skip-adc",)),
    Variant("skip_dac", ("diag-skip-dac",)),
    Variant("skip_record_enqueue", ("diag-skip-record-enqueue",), capture=False),
    Variant("rt_sram", ("diag-rt-sram",)),
    Variant("wiznet_10mhz", ("diag-wiznet-10mhz",)),
)


def run(cmd: list[str], timeout: float | None = None) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        cmd,
        cwd=FIRMWARE,
        check=True,
        timeout=timeout,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
    )


def flash(variant: Variant, flash_timeout: float) -> str:
    features = ",".join(BASE_FEATURES + variant.features)
    cmd = [
        "cargo",
        "run",
        "--release",
        "-p",
        "fw-cbc-rig",
        "--no-default-features",
        "--features",
        features,
    ]
    try:
        return run(cmd, timeout=flash_timeout).stdout
    except subprocess.TimeoutExpired as exc:
        output = exc.stdout or ""
        if isinstance(output, bytes):
            return output.decode(errors="replace")
        return output


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


def snapshot(dev: Device) -> dict[str, int]:
    return dict(zip(COUNTERS, dev.get(*COUNTERS)))


def delta(before: dict[str, int], after: dict[str, int], elapsed_s: float) -> dict[str, float]:
    ticks = after["ticks"] - before["ticks"]
    overruns = after["overruns"] - before["overruns"]
    return {
        "elapsed_s": elapsed_s,
        "ticks": ticks,
        "ticks_per_s": ticks / elapsed_s if elapsed_s > 0 else 0.0,
        "overruns": overruns,
        "overruns_per_s": overruns / elapsed_s if elapsed_s > 0 else 0.0,
        "tick_timeouts": after["tick_timeouts"] - before["tick_timeouts"],
        "records_dropped": after["records_dropped"] - before["records_dropped"],
        "adc_errors": after["adc_errors"] - before["adc_errors"],
        "loop_time_last": after["loop_time_last"],
        "loop_time_max": after["loop_time_max"],
        "clock_jitter": after["clock_jitter"],
    }


def quiet_outputs(dev: Device) -> None:
    coeffs = dev.param("forcing_coeffs").count
    zeros = [0.0] * coeffs
    dev.set("forcing_coeffs", zeros)
    dev.set("target_coeffs", zeros)
    dev.set("table_mode", 0)


def measure_variant(args: argparse.Namespace, variant: Variant) -> dict[str, object]:
    flash_log = flash(variant, args.flash_timeout)
    result: dict[str, object] = {
        "variant": variant.name,
        "features": list(BASE_FEATURES + variant.features),
        "flash_tail": flash_log.splitlines()[-12:],
    }

    with connect(args.host, args.connect_timeout) as dev:
        result["status"] = dev.status()
        result["firmware"] = dev.get("firmware")
        quiet_outputs(dev)

        start = snapshot(dev)
        t0 = time.monotonic()
        time.sleep(args.idle_seconds)
        idle_end = snapshot(dev)
        result["idle"] = delta(start, idle_end, time.monotonic() - t0)

        poll_start = snapshot(dev)
        t0 = time.monotonic()
        end = t0 + args.poll_seconds
        polls = 0
        while time.monotonic() < end:
            snapshot(dev)
            polls += 1
            time.sleep(args.poll_interval)
        poll_end = snapshot(dev)
        poll_delta = delta(poll_start, poll_end, time.monotonic() - t0)
        poll_delta["polls"] = polls
        result["poll"] = poll_delta

        if variant.capture:
            capture_start = snapshot(dev)
            t0 = time.monotonic()
            data = dev.capture(["adc0", "out"], samples=args.capture_samples, port=protocol.STREAM_PORT)
            capture_end = snapshot(dev)
            capture_delta = delta(capture_start, capture_end, time.monotonic() - t0)
            capture_delta["records"] = int(len(data["index"]))
            capture_delta["lost_packets"] = int(data["lost_packets"])
            capture_delta["capture_dropped"] = int(data["dropped"])
            result["capture"] = capture_delta
        else:
            result["capture"] = None

        quiet_outputs(dev)
        result["final"] = snapshot(dev)

    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default=os.environ.get("HELIC_DAQ_HOST", "192.168.1.235"))
    parser.add_argument("--variant", action="append", choices=[v.name for v in VARIANTS])
    parser.add_argument("--idle-seconds", type=float, default=3.0)
    parser.add_argument("--poll-seconds", type=float, default=2.0)
    parser.add_argument("--poll-interval", type=float, default=0.05)
    parser.add_argument("--capture-samples", type=int, default=1000)
    parser.add_argument("--flash-timeout", type=float, default=10.0)
    parser.add_argument("--connect-timeout", type=float, default=10.0)
    args = parser.parse_args()

    selected = [v for v in VARIANTS if args.variant is None or v.name in args.variant]
    results = []
    for variant in selected:
        print(f"=== {variant.name} ===", flush=True)
        result = measure_variant(args, variant)
        results.append(result)
        print(json.dumps(result, indent=2), flush=True)
    print("=== summary ===")
    print(json.dumps(results, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
