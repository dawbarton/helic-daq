"""Julia interface to HELIC-DAQ control, discovery, and streaming services."""
module HelicDAQ

using Sockets
using Tables

include("protocol.jl")
include("stream.jl")
include("device.jl")
include("discovery.jl")

export Capture,
    Device,
    DeviceError,
    DiscoveredDevice,
    Parameter,
    Protocol,
    Source,
    StreamReceiver,
    StreamTimeout,
    capture,
    configure_stream!,
    find_devices,
    getparam,
    getparams,
    parameter,
    prime!,
    receive,
    setparam!,
    start_stream!,
    status,
    stop_stream!,
    upload_table!

end
