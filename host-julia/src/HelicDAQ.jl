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
    broker_info,
    capture,
    capture_recent,
    configure_stream!,
    find_devices,
    getparam,
    getparams,
    parameter,
    prime!,
    receive,
    setparam!,
    set_stream_quiet!,
    start_stream!,
    start_stream_quiet!,
    status,
    stop_stream!,
    upload_table!

end
