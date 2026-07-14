"""UDP discovery of HELIC-DAQ devices on IPv4 networks."""

"""Identity and connection details returned by a discovery beacon."""
struct DiscoveredDevice
    address::IPv4
    version::UInt8
    control_port::UInt16
    mac::String
    experiment::String
    firmware::String
end

function _ipv4(address)
    address isa IPv4 && return address
    resolved = getaddrinfo(String(address), IPv4)
    resolved isa IPv4 || throw(ArgumentError("'$address' did not resolve to IPv4"))
    return resolved
end

"""Query IPv4 discovery targets and return unique responses before timeout."""
function find_devices(;
    timeout::Real=1.0,
    port::Integer=Protocol.DISCOVERY_PORT,
    addresses=nothing,
)
    timeout > 0 || throw(ArgumentError("timeout must be positive"))
    1 <= port <= typemax(UInt16) ||
        throw(ArgumentError("port must be between 1 and $(typemax(UInt16))"))
    targets = isnothing(addresses) ? IPv4[ip"255.255.255.255", ip"127.0.0.1"] :
              _ipv4.(addresses)
    socket = UDPSocket()
    bind(socket, ip"0.0.0.0", 0)
    Sockets.setopt(socket; enable_broadcast=true)
    timer = Timer(timeout) do _
        isopen(socket) && close(socket)
    end

    try
        for target in targets
            try
                send(socket, target, port, Protocol.BEACON_REQUEST)
            catch error
                error isa Base.IOError || rethrow()
            end
        end

        found = Dict{Tuple{IPv4,NTuple{6,UInt8}},DiscoveredDevice}()
        while true
            peer, payload = try
                recvfrom(socket)
            catch error
                (error isa Base.IOError || error isa EOFError) || rethrow()
                break
            end
            beacon = try
                Protocol.decode_beacon_response(payload)
            catch error
                error isa Protocol.ProtocolError || rethrow()
                continue
            end
            peer.host isa IPv4 || continue
            mac = join((string(byte; base=16, pad=2) for byte in beacon.mac), ":")
            found[(peer.host, beacon.mac)] = DiscoveredDevice(
                peer.host,
                beacon.version,
                beacon.control_port,
                mac,
                beacon.experiment,
                beacon.firmware,
            )
        end
        return sort!(collect(values(found)); by=device -> (device.address.host, device.mac))
    finally
        close(timer)
        isopen(socket) && close(socket)
    end
end
