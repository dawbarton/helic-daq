# Host API migration for 0.2

The Python and Julia host packages move to 0.2 because this release deliberately
breaks their public APIs. MATLAB already had the intended names and namespace,
so its API is unchanged. The three clients now use the same vocabulary, adjusted
for snake case, Julia mutation marks, and MATLAB camel case.

## Python

Import the package namespace for normal use:

```python
import helic_daq as hdaq

with hdaq.Device("192.168.1.235") as device:
    device.set_parameter("freq", 10.0)
    value = device.get_parameter("freq")
```

The top-level public surface is `Device`, `DeviceError`, `StreamReceiver`, and
`find_devices`. `Parameter` and `Source` remain available from
`helic_daq.device`, and the low-level protocol remains `hdaq.protocol`, but
none is promoted by `from helic_daq import *`.

| Before 0.2 | 0.2 |
|---|---|
| `device.param(name)` | `device.parameter(name)` |
| `device.get(name)` | `device.get_parameter(name)` |
| `device.get(*names)` | `device.get_parameters(names)` |
| `device.set(name, value)` | `device.set_parameter(name, value)` |
| `device.stream_setup(...)` | `device.configure_stream(...)` |
| `device.stream_start(port)` | `device.start_stream(port)` |
| `device.stream_start_quiet(port)` | `device.start_stream_quiet(port)` |
| `device.stream_set_quiet(quiet)` | `device.set_stream_quiet(quiet)` |
| `device.stream_stop()` | `device.stop_stream()` |
| `receiver.recv()` | `receiver.receive()` |

There are no compatibility aliases. In particular, `get_parameters` takes one
iterable of names and always returns a list; `get_parameter` returns the single
value. `__version__` is now read from installed package metadata rather than a
second, manually maintained constant.

## Julia

Types and lower-level operations must now be qualified. Import only the common
operations that a session uses:

```julia
import HelicDAQ
using HelicDAQ: capture, getparam, setparam!

open(HelicDAQ.Device, "192.168.1.235") do device
    @show HelicDAQ.status(device)
    setparam!(device, :freq, 10f0)
    data = capture(device, [:laser, :out]; seconds = 2)
end
```

The exported operations are `broker_info`, `capture`, `capture_recent`,
`configure_stream!`, `find_devices`, `getparam`, `getparams`, `setparam!`,
`set_stream_quiet!`, `start_stream!`, `start_stream_quiet!`, `stop_stream!`,
and `upload_table!`.

`Capture`, `Device`, `DeviceError`, `DiscoveredDevice`, `Parameter`, `Protocol`,
`Source`, `StreamReceiver`, and `StreamTimeout` are no longer exported. Neither
are the lower-level `parameter`, `prime!`, `receive`, `reboot!`, and `status`
operations. Their definitions and behaviour are unchanged, and all remain
available through `HelicDAQ.`, for example, `HelicDAQ.Device` and
`HelicDAQ.reboot!`.

## Cross-language names

| Operation | Python | Julia | MATLAB |
|---|---|---|---|
| Parameter definition | `parameter` | `parameter` | `parameter` |
| Read one | `get_parameter` | `getparam` | `getParameter` |
| Read several | `get_parameters` | `getparams` | `getParameters` |
| Write | `set_parameter` | `setparam!` | `setParameter` |
| Configure stream | `configure_stream` | `configure_stream!` | `configureStream` |
| Start stream | `start_stream` | `start_stream!` | `startStream` |
| Quiet start | `start_stream_quiet` | `start_stream_quiet!` | `startStreamQuiet` |
| Change quietness | `set_stream_quiet` | `set_stream_quiet!` | `setStreamQuiet` |
| Stop stream | `stop_stream` | `stop_stream!` | `stopStream` |
| Receive packet | `receive` | `receive` | `receive` |
