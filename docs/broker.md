# HELIC-DAQ shared broker

`helic-broker` is an optional host-side process. It owns the MCU's single TCP
control connection and UDP stream endpoint, while presenting the normal
HELIC-DAQ services to any number of loopback TCP clients. Firmware changes are
not required.

## Running it

Build and run from the repository root:

```sh
cargo run --release -p helic-broker -- \
  --mcu-host 192.168.1.235 \
  --output-dir /data/helic
```

The downstream control, stream, and discovery services bind to
`127.0.0.1:2350`, `127.0.0.1:2351`, and `127.0.0.1:2352`. They are never
exposed on other interfaces. Use `--control-port`, `--stream-port`, and
`--discovery-port` to avoid local conflicts. The corresponding
`--mcu-*-port` options select non-standard upstream ports.

`--history 10s` sets the shared in-memory history target. Durations accept
`ms`, `s`, `m`, or `h`. `--segment-size 1GiB` sets the soft file-rollover
threshold and accepts bytes or `KiB`/`MiB`/`GiB` (decimal units are also
accepted). History evicts whole packets, so retention can differ from the
duration target by one packet; BrokerInfo reports the precise available record
count. The output directory is created at start-up.

The loopback discovery beacon keeps the MCU experiment identity and advertises
the broker firmware identity and downstream control port. It uses the MCU MAC
when upstream discovery succeeds and a locally administered fallback identity
otherwise. The broker does not answer discovery or retain client connections
until its upstream connection is ready.

## Shared stream behaviour

- `StreamSetup` sets one global source list, decimation, and count. It returns
  busy while the global stream is active.
- The first `StreamStart` starts the MCU and recording session. A later
  `StreamStart` only attaches that client to the existing stream.
- Any client can issue `StreamStop`; this stops the global stream, closes its
  recording, clears history, and detaches every client.
- Every client has its own UDP endpoint and quiet flag. Quiet clients are
  still attached, but receive no live packets. Attachment and quietness never
  affect recording.
- A quiet client can request an exact number of recent records. The history
  is global and may contain data from before that client connected. A request
  larger than the retained history returns an error rather than partial data.
- A finite non-zero stream count has its firmware meaning: the stream and
  recording complete after that many decimated records. Count zero is
  continuous.
- If all clients disconnect, the stream and recording continue, but the
  broker writes `arm = 0` when that parameter exists. A new client may attach
  later.
- `SetBlock`/`Commit` ownership is temporary and connection-local, preventing
  two clients from interleaving one staged table transaction. No client is
  otherwise privileged.

Loss of the MCU connection closes all client connections, clears the shared
configuration and history, closes an active file as an incomplete session,
then starts reconnection attempts. Clients reconnect, rediscover state, and
configure a new stream in the normal way. A storage failure is fatal, because
continuing without the promised recording would be misleading. `Ctrl-C`
requests a graceful stream stop, recording finalisation, and disarm.

## Host APIs

The extra operations deliberately fail with unknown-message type when a host
is connected directly to firmware.

Python:

```python
from helic_daq import Device, StreamReceiver

monitor = Device("127.0.0.1")
monitor.stream_setup(["adc0", "out"], count=0)
monitor_rx = StreamReceiver(port=0)
monitor_rx.prime(monitor.host)
monitor.stream_start(monitor_rx.port)  # ordinary live monitor

snapshot = Device("127.0.0.1")
data = snapshot.capture_recent(seconds=1.0, port=0)
# snapshot remains attached and quiet
```

Julia:

```julia
using HelicDAQ

monitor = Device("127.0.0.1")
configure_stream!(monitor, [:adc0, :out]; count = 0)
monitor_rx = StreamReceiver(port = 0)
prime!(monitor_rx, monitor.host)
start_stream!(monitor, monitor_rx.port)

snapshot = Device("127.0.0.1")
data = capture_recent(snapshot; seconds = 1, port = 0)
```

MATLAB:

```matlab
monitor = helicdaq.Device("127.0.0.1");
monitor.configureStream(["adc0", "out"], 'Count', 0);
monitorRx = helicdaq.StreamReceiver('Port', 0);
monitorRx.prime(monitor.Host, helicdaq.Protocol.STREAM_PORT);
monitor.startStream(monitorRx.Port);

snapshot = helicdaq.Device("127.0.0.1");
data = snapshot.captureRecent('Seconds', 1, 'Port', 0);
```

Use `broker_info()` (Python), `broker_info` (Julia), or `brokerInfo` (MATLAB)
to inspect the shared configuration, retained record count, connected-client
count, and the calling client's attachment and quiet flags.

## HDF5 recording layout

One global stream start creates one session. Filenames contain the UTC start
timestamp, experiment name, and four-digit segment number. An open segment has
the suffix `.h5.partial`; a cleanly closed segment is renamed to `.h5`.
Size is checked at the one-second flush boundary, so a segment can exceed the
configured soft threshold by approximately one second of data.

Root attributes describe the format, session UUID, segment index, cumulative
session record offset, experiment and firmware identities, sample rate,
decimation, configured count, and start time. The datasets are:

| path | type and shape | meaning |
|---|---|---|
| `/sources/names` | strings `[source]` | source names in record order |
| `/sources/units` | strings `[source]` | source units |
| `/records/values` | f32 `[record, source]` | record-major measurements |
| `/records/sample_index` | u32 `[record, 1]` | effective MCU sample index |
| `/packets/sequence` | u32 `[packet, 1]` | original packet sequence |
| `/packets/first_index` | u32 `[packet, 1]` | original first sample index |
| `/packets/dropped` | u32 `[packet, 1]` | cumulative MCU-side drops |
| `/packets/record_offset` | u64 `[packet, 1]` | packet offset in the session |
| `/packets/record_count` | u16 `[packet, 1]` | records in the packet |
| `/packets/received_utc_ns` | i64 `[packet, 1]` | broker receive timestamp |
| `/ended_utc_ns` | i64 scalar | segment close time |
| `/close_reason` | u8 scalar | enumerated close reason |
| `/clean_close` | u8 scalar | 1 after finalisation |
| `/session_complete` | u8 scalar | 1 on the final clean session segment |

`close_reason` values are 1 segment limit, 2 explicit `StreamStop`, 3 finite
count complete, 4 upstream loss, and 6 broker shutdown. Value 5 is reserved
for storage failure; a writer failure normally leaves the `.partial` file
unfinalised instead.

The output is standard HDF5 and is readable with Python `h5py`, Julia
`HDF5.jl`, and MATLAB `h5read`/`h5info`. `.partial` files left by process or
host failure are intentionally not renamed or marked clean; preserve them for
forensic recovery rather than treating them as complete captures.

## Test plan

The implementation is checked at four levels:

1. Shared-codec and state tests cover extension payloads, strict setup
   parsing, bounded history, and exact packet trimming.
2. Storage tests read completed files through an independent HDF5 reader and
   force a size rollover to verify linked segment metadata.
3. A loopback system test uses a protocol peer and two real TCP/UDP clients to
   verify ordinary forwarding, quiet attachment, exact replay, quietness
   changes, global stop, disarm-on-final-disconnect, and recorded data.
4. The existing Rust, Python, Julia, and MATLAB suites guard direct-MCU
   behaviour and cross-language codec conventions. No firmware or hardware
   regression run is required because the firmware and real-time path are
   unchanged.
