# helic-daq (host package)

Python interface to the HELIC-DAQ device: parameter access over TCP,
sample streaming over UDP, and a `helic-daq` command-line tool.

Install (from the repository root):

```sh
pip install -e host-python   # add [plot] for the plotting extra
```

Quick start:

```python
import helic_daq as hdaq

dev = hdaq.Device("192.168.1.235")
print(dev.params)                 # discovered parameter list
dev.par.freq = 10.0               # attribute-style access
data = dev.capture(["laser", "out"], seconds=2.0)
dev.upload_table(
    [0.0, 1.0, 0.0, -1.0],
    duration=0.2,
    interpolation="hold",
)
# Quiesces outputs, schedules reset, and closes dev:
dev.reboot()
```

For host-side development without hardware, run the protocol-v3 simulator:

```sh
python3 -m helic_daq.sim
helic-daq --host 127.0.0.1 capture --sources laser,out --samples 1000
```

See the repository [user guide](../docs/user_guide.md) for installation,
discovery, capture and waveform details, and the [wire protocol](../docs/protocol.md)
for the authoritative binary formats.

After another client has started a stream through the optional local broker,
`broker_info`,
`start_stream_quiet`, `set_stream_quiet`, and `capture_recent` expose shared
stream state and per-client quietness. Use `port=0` for concurrent receivers:

```python
import helic_daq as hdaq

with hdaq.Device("127.0.0.1") as dev:
    recent = dev.capture_recent(seconds=1.0, port=0)
```

See the [broker guide](../docs/broker.md) for starting the daemon, global
start/stop semantics, and the HDF5 layout. These extension calls return the
normal unknown-message error on a direct firmware connection.
