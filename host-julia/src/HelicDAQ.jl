"""Julia interface to HELIC-DAQ control, discovery, and streaming services."""
module HelicDAQ

using Sockets
using Tables

include("protocol.jl")
include("stream.jl")
include("device.jl")
include("discovery.jl")

export broker_info,
    capture,
    capture_recent,
    configure_stream!,
    find_devices,
    getparam,
    getparams,
    setparam!,
    set_stream_quiet!,
    start_stream!,
    start_stream_quiet!,
    stop_stream!,
    upload_table!

end
