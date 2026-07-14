"""HELIC-DAQ Julia package test entry point."""

using HelicDAQ
using Sockets
using Tables
using Test

include("protocol.jl")
include("stream.jl")
include("device.jl")
include("discovery.jl")
