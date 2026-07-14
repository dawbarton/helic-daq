"""Command-line interface: ``helic-daq <command>``.

The device address comes from ``--host`` or the ``CBC_DAQ_HOST`` environment
variable (default 192.168.1.235).
"""

from __future__ import annotations

import argparse
import os
import sys

from .device import Device, DeviceError


def _connect(args) -> Device:
    return Device(args.host)


def cmd_list(args) -> None:
    with _connect(args) as dev:
        print(f"{'idx':>3}  {'name':<16} {'type':<6} {'rw':<3} value")
        for p in dev.params:
            value = dev.get(p.index)
            if isinstance(value, list) and len(value) > 4:
                shown = f"[{value[0]:g}, {value[1]:g}, ... x{len(value)}]"
            elif isinstance(value, float):
                shown = f"{value:g}"
            else:
                shown = str(value)
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


def cmd_sine(args) -> None:
    """Quick smoke test: sinusoidal forcing on the output channel."""
    with _connect(args) as dev:
        coeffs = dev._param("forcing_coeffs")
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
        coeffs = dev._param("forcing_coeffs")
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
    print(f"captured {n} records (source drop counter: {data['dropped']})")
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


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(prog="helic-daq", description=__doc__)
    parser.add_argument(
        "--host",
        default=os.environ.get("CBC_DAQ_HOST", "192.168.1.235"),
        help="device IP address (default: $CBC_DAQ_HOST or 192.168.1.235)",
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

    p = sub.add_parser("sine", help="output a sine wave (smoke test)")
    p.add_argument("freq", type=float, help="frequency in Hz")
    p.add_argument("amplitude", type=float, help="amplitude in volts")
    p.add_argument("--harmonic", type=int, default=1)
    p.set_defaults(fn=cmd_sine)

    sub.add_parser("stop", help="zero the forcing and target").set_defaults(fn=cmd_stop)

    p = sub.add_parser("stream", help="capture streamed data")
    p.add_argument("--sources", default="adc0,out", help="comma-separated (default adc0,out)")
    group = p.add_mutually_exclusive_group(required=True)
    group.add_argument("--seconds", type=float)
    group.add_argument("--samples", type=int)
    p.add_argument("--decimation", type=int, default=1, help="keep every n-th sample")
    p.add_argument("--output", "-o", help="save to .npz file")
    p.add_argument("--plot", action="store_true", help="plot with matplotlib")
    p.set_defaults(fn=cmd_stream)

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
