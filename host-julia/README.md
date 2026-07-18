# HelicDAQ.jl

Julia interface to HELIC-DAQ protocol v3. It discovers parameter and source
registries on every connection, supports atomic waveform-table upload, and
returns streamed captures through the Tables.jl interface.

Develop the package from the repository root:

```julia
using Pkg
Pkg.develop(path="host-julia")
```

Connect and capture:

```julia
using HelicDAQ, Tables

open(Device, "192.168.1.235") do device
    @show status(device)
    device[:freq] = 10f0

    coefficients = zeros(Float32, 33)
    coefficients[18] = 1f0  # b₁ in Julia's one-based indexing
    device[:forcing_coeffs] = coefficients

    data = capture(device, [:adc0, :out]; seconds=2)
    columns = Tables.columntable(data)
    @show columns.adc0[1:5]
    @show data.dropped data.lost_packets
end
```

`Capture` is a column-access table whose first column is `index`; each
requested source follows by its discovered name. Device-side source-ring drops
and UDP packet loss are capture metadata rather than repeated table columns.
Any Tables.jl consumer can use the result directly.

Parameters can be read with `device[:name]` or `getparam`, and written with
assignment or `setparam!`. `getparams(device, (:name1, :name2))` performs one
round trip and returns a named tuple. Source selection is by name or by a
discovered `Source`, never by a cached registry index. For continuous streams,
combine `configure_stream!`, `StreamReceiver`, `start_stream!`, `receive`, and
`stop_stream!`.

Discover devices and upload an arbitrary waveform with:

```julia
devices = find_devices()
upload_table!(
    device,
    Float32[0, 1, 0, -1];
    duration=0.2,
    mode=:loop,
    interpolation=:hold,
)
```

The Python package retains the `helic-daq` command-line interface and simulator;
Julia can connect to that simulator like physical firmware. Test this package
with:

```sh
julia --project=host-julia -e 'using Pkg; Pkg.instantiate(); Pkg.test()'
```
