# HELIC-DAQ for MATLAB

MATLAB interface to HELIC-DAQ protocol v2. It discovers parameter and source
registries on every connection, supports atomic waveform-table upload, and
returns finite captures as native MATLAB tables. UDP discovery and streaming
require Instrument Control Toolbox for `udpport`.

Add the package directory to the MATLAB path from the repository root:

```matlab
addpath("host-matlab")
```

Discover, connect, and capture:

```matlab
devices = helicdaq.findDevices();

device = helicdaq.Device("192.168.1.235");
cleanup = onCleanup(@() delete(device));

information = device.status()
device.setParameter("freq", 10);

coefficients = zeros(1, 33, "single");
coefficients(18) = 1; % b1 with one-based indexing
device.setParameter("forcing_coeffs", coefficients);

data = device.capture(["adc0", "out"], 'Seconds', 2);
mean(data.adc0)
data.Properties.VariableUnits
data.Properties.UserData
```

`Device.Parameters` and `Device.Sources` are discovery tables. `getParameters`
returns a table with `Name` and `Value` columns. A capture has `index` followed
by the requested sources; device-side drops and UDP packet loss are stored in
`data.Properties.UserData` rather than repeated in every row.

Upload an arbitrary waveform with:

```matlab
device.uploadTable(single([0, 1, 0, -1]), ...
    'Duration', 0.2, 'Gain', 1.5, 'Mode', "loop", ...
    'Interpolation', "hold");
```

For continuous acquisition, combine `configureStream`, `StreamReceiver`,
`startStream`, `receive`, and `stopStream`. Source selection remains by
discovered name; wire indices are never cached across connections.

Run the package test suite with:

```sh
matlab -batch "cd('host-matlab'); runTests()"
```

The suite uses deterministic in-memory transports for protocol behaviour and
also runs native UDP loopback tests when Instrument Control Toolbox is present.
