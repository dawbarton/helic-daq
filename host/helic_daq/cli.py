"""Command-line interface: ``helic-daq <command>``.

The device address comes from ``--host`` or the ``HELIC_DAQ_HOST`` environment
variable (default 192.168.1.235).
"""

from __future__ import annotations

import argparse
import os
import sys

from . import protocol
from .device import Device, DeviceError
from .discovery import find_devices


def _connect(args) -> Device:
    return Device(args.host, args.port)


def cmd_list(args) -> None:
    with _connect(args) as dev:
        print(f"{'idx':>3}  {'name':<16} {'type':<6} {'rw':<3} value")
        for p in dev.params:
            if p.size > protocol.MAX_PAYLOAD:
                shown = "<block parameter>"
            else:
                value = dev.get(p.index)
                shown = value
            if isinstance(shown, list) and len(shown) > 4:
                shown = f"[{shown[0]:g}, {shown[1]:g}, ... x{len(shown)}]"
            elif isinstance(shown, float):
                shown = f"{shown:g}"
            else:
                shown = str(shown)
            ty = f"{p.type_code}x{p.count}" if p.count > 1 else p.type_code
            print(f"{p.index:>3}  {p.name:<16} {ty:<6} {'rw' if p.writable else 'ro':<3} {shown}")


def cmd_get(args) -> None:
    with _connect(args) as dev:
        for name in args.names:
            print(f"{name} = {dev.get(name)}")


def _parse_values(text: str):
    parts = [float(v) for v in text.replace(",", " ").split()]
    return parts[0] if len(parts) == 1 else parts


def cmd_set(args) -> None:
    with _connect(args) as dev:
        dev.set(args.name, _parse_values(args.value))
        print(f"{args.name} = {dev.get(args.name)}")


def cmd_status(args) -> None:
    with _connect(args) as dev:
        for key, value in dev.status().items():
            print(f"{key}: {value}")
        print(f"firmware: {dev.get('firmware')}")


def cmd_sources(args) -> None:
    with _connect(args) as dev:
        print(f"{'id':>3}  {'name':<16} unit")
        for source in dev.sources:
            print(f"{source.index:>3}  {source.name:<16} {source.unit}")


def cmd_find(args) -> None:
    devices = find_devices(args.timeout, args.discovery_port, args.address)
    print(f"{'address':<15} {'port':>5}  {'experiment':<16} {'firmware':<16} mac")
    for device in devices:
        print(
            f"{device.address:<15} {device.control_port:>5}  "
            f"{device.experiment:<16} {device.firmware:<16} {device.mac}"
        )


def cmd_sine(args) -> None:
    """Quick smoke test: sinusoidal forcing on the output channel."""
    with _connect(args) as dev:
        coeffs = dev.param("forcing_coeffs")
        n = coeffs.count  # 1 + 2K: mean, a[1..K], b[1..K]
        harmonics = (n - 1) // 2
        if not 1 <= args.harmonic <= harmonics:
            raise DeviceError(f"--harmonic must be between 1 and {harmonics}")
        values = [0.0] * n
        values[1 + harmonics + (args.harmonic - 1)] = args.amplitude  # b_k (sin)
        dev.set("freq", args.freq)
        dev.set("forcing_coeffs", values)
        print(f"forcing: {args.amplitude} V sine at {args.freq} Hz (harmonic {args.harmonic})")


def cmd_stop(args) -> None:
    with _connect(args) as dev:
        coeffs = dev.param("forcing_coeffs")
        dev.set("forcing_coeffs", [0.0] * coeffs.count)
        dev.set("target_coeffs", [0.0] * coeffs.count)
        print("forcing and target zeroed")


def cmd_stream(args) -> None:
    sources = args.sources.split(",")
    with _connect(args) as dev:
        if args.seconds is not None:
            data = dev.capture(sources, seconds=args.seconds, decimation=args.decimation)
        else:
            data = dev.capture(sources, samples=args.samples, decimation=args.decimation)
    n = len(data["index"])
    # `dropped` is the device's cumulative since-boot drop counter, not the
    # number of drops during this capture.
    print(
        f"captured {n} records (source drops: {data['dropped']}, "
        f"UDP packets lost: {data['lost_packets']})"
    )
    if args.output:
        import numpy as np

        np.savez(args.output, **data)
        print(f"saved to {args.output}")
    else:
        head = min(n, 5)
        for i in range(head):
            row = ", ".join(f"{data[s][i]:+.4f}" for s in sources)
            print(f"  [{data['index'][i]}] {row}")
        if n > head:
            print(f"  ... {n - head} more")
    if args.plot:
        import matplotlib.pyplot as plt

        fs = None
        with _connect(args) as dev:
            fs = dev.status()["sample_rate"]
        t = data["index"] / fs
        for s in sources:
            plt.plot(t, data[s], label=s)
        plt.xlabel("time [s]")
        plt.ylabel("value")
        plt.legend()
        plt.show()


def cmd_upload(args) -> None:
    import numpy as np

    values = np.load(args.path)
    with _connect(args) as dev:
        dev.upload_table(
            values.ravel(),
            duration=args.duration,
            freq=args.freq,
            gain=args.gain,
            mode=args.mode,
            mult=args.mult,
            phase=args.phase,
        )
    print(f"uploaded {values.size} table samples in {args.mode} mode")


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="helic-daq", description=__doc__)
    parser.add_argument(
        "--host",
        default=os.environ.get("HELIC_DAQ_HOST", "192.168.1.235"),
        help="device IP address (default: $HELIC_DAQ_HOST or 192.168.1.235)",
    )
    parser.add_argument(
        "--port", type=int, default=protocol.CONTROL_PORT, help="control TCP port"
    )
    sub = parser.add_subparsers(dest="command", required=True)

    sub.add_parser("list", help="list all parameters and their values").set_defaults(fn=cmd_list)

    p = sub.add_parser("get", help="read parameter(s)")
    p.add_argument("names", nargs="+")
    p.set_defaults(fn=cmd_get)

    p = sub.add_parser("set", help="write a parameter")
    p.add_argument("name")
    p.add_argument("value", help='value, or comma/space-separated list for arrays')
    p.set_defaults(fn=cmd_set)

    sub.add_parser("status", help="device status").set_defaults(fn=cmd_status)
    p = sub.add_parser("find", help="discover HELIC-DAQ devices")
    p.add_argument("--timeout", type=float, default=1.0)
    p.add_argument("--discovery-port", type=int, default=protocol.DISCOVERY_PORT)
    p.add_argument("--address", action="append", help="query this address instead of broadcast")
    p.set_defaults(fn=cmd_find)
    sub.add_parser("sources", help="list discoverable stream sources").set_defaults(
        fn=cmd_sources
    )

    p = sub.add_parser("sine", help="output a sine wave (smoke test)")
    p.add_argument("freq", type=float, help="frequency in Hz")
    p.add_argument("amplitude", type=float, help="amplitude in volts")
    p.add_argument("--harmonic", type=int, default=1)
    p.set_defaults(fn=cmd_sine)

    sub.add_parser("stop", help="zero the forcing and target").set_defaults(fn=cmd_stop)

    p = sub.add_parser("capture", help="capture streamed data")
    p.add_argument("--sources", default="adc0,out", help="comma-separated (default adc0,out)")
    group = p.add_mutually_exclusive_group(required=True)
    group.add_argument("--seconds", type=float)
    group.add_argument("--samples", type=int)
    p.add_argument("--decimation", type=int, default=1, help="keep every n-th sample")
    p.add_argument("--output", "-o", help="save to .npz file")
    p.add_argument("--plot", action="store_true", help="plot with matplotlib")
    p.set_defaults(fn=cmd_stream)

    p = sub.add_parser("upload", help="upload a .npy arbitrary waveform")
    p.add_argument("path")
    timing = p.add_mutually_exclusive_group()
    timing.add_argument("--duration", type=float, help="free-running period in seconds")
    timing.add_argument("--freq", type=float, help="free-running playback frequency in Hz")
    p.add_argument("--gain", type=float, default=1.0)
    p.add_argument(
        "--mode",
        choices=["off", "loop", "one-shot", "locked", "locked-one-shot"],
        default="loop",
    )
    p.add_argument("--mult", type=int, default=1, help="locked frequency multiplier")
    p.add_argument("--phase", type=float, default=0.0, help="locked phase offset in turns")
    p.set_defaults(fn=cmd_upload)

    args = parser.parse_args(argv)
    try:
        args.fn(args)
    except DeviceError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    except (ConnectionError, OSError) as e:
        print(f"connection error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
