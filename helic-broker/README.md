# helic-broker

`helic-broker` is the optional long-running host service for HELIC-DAQ. It
connects to one MCU, exposes the same control/discovery/stream interfaces on
loopback to multiple local clients, and records every active stream to HDF5.
It requires no firmware changes.

## Quick start

From the repository root:

```sh
cargo build --release -p helic-broker
target/release/helic-broker \
  --mcu-host 192.168.1.235 \
  --output-dir captures
```

The downstream services listen only on `127.0.0.1`, using the ordinary
control, stream, and discovery ports 2350–2352. Point Python, Julia, MATLAB,
or the Python CLI at `127.0.0.1`. `--help` lists upstream and downstream port
overrides, request/reconnect timeouts, logging, the history duration, and the
soft segment-size limit.

The first `StreamStart` starts the globally configured MCU stream. Further
starts attach clients without restarting it, and any client can stop it.
Recording and recent history are global; UDP endpoints and quietness are per
client. The broker continues streaming and recording with no clients, but
disarms the MCU when the final client disconnects.

Typical recent capture from a second Python process:

```python
from helic_daq import Device

with Device("127.0.0.1") as device:
    print(device.broker_info())
    recent = device.capture_recent(seconds=1.0, port=0)
```

Use `port=0` for concurrent clients so each receiver gets an ephemeral local
UDP port. Broker-only calls give the ordinary unknown-message error when a
client is connected directly to firmware.

## Recording and failures

Each global stream creates a timestamped HDF5 session in `--output-dir`.
Files expose source names and units, record values and sample indices, packet
loss metadata, timestamps, session/segment identifiers, and close status.
Open files end in `.h5.partial`; finalised segments are renamed `.h5`.
Segments roll at a soft 1 GiB by default, checked at the one-second flush
boundary.

An MCU disconnect closes all downstream clients, clears stream state,
finalises an active file as incomplete, and starts reconnecting. A storage
failure terminates the broker rather than silently dropping promised data.
`Ctrl-C` performs graceful stop, finalisation, and disarm.

## Development

```sh
cargo fmt --all -- --check
cargo clippy -p helic-broker --all-targets --all-features -- -D warnings
cargo test -p helic-broker
```

The tests cover bounded exact-record history, HDF5 read-back and segmentation,
and a two-client TCP/UDP system flow with discovery, quiet replay, live
forwarding, global stop, recording, and final-client disarm.

The complete operational semantics, host-language examples, extension wire
format, and HDF5 schema are in the [broker guide](../docs/broker.md). The
[user guide](../docs/user_guide.md) covers normal HELIC-DAQ workflows, and the
[developer guide](../docs/developer_guide.md) describes internal ownership and
failure invariants.
