# HELIC-DAQ Julia package test entry point.

using HelicDAQ
using Sockets
using Tables
using Test

@testset "public exports" begin
    exported = Set(names(HelicDAQ))
    @test Set((
        :broker_info,
        :capture,
        :capture_recent,
        :configure_stream!,
        :find_devices,
        :getparam,
        :getparams,
        :setparam!,
        :set_stream_quiet!,
        :start_stream!,
        :start_stream_quiet!,
        :stop_stream!,
        :upload_table!,
    )) ⊆ exported
    @test isempty(intersect(
        exported,
        Set((
            :Capture,
            :Device,
            :DeviceError,
            :DiscoveredDevice,
            :Parameter,
            :Protocol,
            :Source,
            :StreamReceiver,
            :StreamTimeout,
            :parameter,
            :prime!,
            :receive,
            :reboot!,
            :status,
        )),
    ))
end

include("protocol.jl")
include("stream.jl")
include("device.jl")
include("discovery.jl")
