# UDP stream reception and the Tables.jl-compatible capture container.

"""No stream packet arrived within the receiver timeout; the receiver is closed."""
struct StreamTimeout <: Exception
    seconds::Float64
end

Base.showerror(io::IO, error::StreamTimeout) =
    print(io, "no HELIC-DAQ stream packet received within $(error.seconds) seconds")

"""A bound UDP receiver that tracks packet-sequence loss.

Pass `port = 0` to bind an ephemeral port; `receiver.port` is always the
actual bound port.
"""
mutable struct StreamReceiver
    socket::UDPSocket
    port::UInt16
    timeout::Float64
    last_sequence::Union{Nothing, UInt32}
    lost_packets::Int
end

# Enlarge the OS receive buffer (best effort) so bursty streams are less
# likely to be dropped in the kernel before `receive` runs.
function _set_receive_buffer(socket::UDPSocket, bytes::Integer)
    size = Ref{Cint}(bytes)
    ccall(:uv_recv_buffer_size, Cint, (Ptr{Cvoid}, Ref{Cint}), socket.handle, size)
    return nothing
end

# Sockets.getsockname does not support UDPSocket (as of Julia 1.10), so ask
# libuv directly. The port is at byte offset 2 in network order for both
# sockaddr_in and sockaddr_in6.
function _bound_port(socket::UDPSocket)
    storage = zeros(UInt8, 128)
    storage_length = Ref{Cint}(length(storage))
    rc = ccall(
        :uv_udp_getsockname,
        Cint,
        (Ptr{Cvoid}, Ptr{UInt8}, Ref{Cint}),
        socket.handle,
        storage,
        storage_length,
    )
    rc == 0 || throw(Base.IOError("could not read the bound stream port", rc))
    return UInt16(storage[3]) << 8 | UInt16(storage[4])
end

function StreamReceiver(;
        port::Integer = Protocol.STREAM_PORT,
        bind_address::IPAddr = ip"0.0.0.0",
        timeout::Real = 2.0,
    )
    0 <= port <= typemax(UInt16) ||
        throw(ArgumentError("port must be between 0 and $(typemax(UInt16))"))
    timeout > 0 || throw(ArgumentError("timeout must be positive"))
    socket = UDPSocket()
    try
        # bind returns false (rather than throwing) when the address is in
        # use or inaccessible.
        bind(socket, bind_address, port) || throw(
            Base.IOError("could not bind stream receiver to $bind_address:$port", 0),
        )
        _set_receive_buffer(socket, 1 << 20)
        return StreamReceiver(socket, _bound_port(socket), Float64(timeout), nothing, 0)
    catch
        close(socket)
        rethrow()
    end
end

Base.close(receiver::StreamReceiver) = close(receiver.socket)
Base.isopen(receiver::StreamReceiver) = isopen(receiver.socket)

function Base.open(f::Function, ::Type{StreamReceiver}; kwargs...)
    receiver = StreamReceiver(; kwargs...)
    try
        return f(receiver)
    finally
        close(receiver)
    end
end

# Julia sockets have no native read timeout, so a Timer closes the socket to
# interrupt the blocking recvfrom; a timed-out receiver is therefore unusable.
function _recvfrom_timeout(socket::UDPSocket, timeout::Float64)
    expired = Ref(false)
    timer = Timer(timeout) do _
        expired[] = true
        isopen(socket) && close(socket)
    end
    try
        return recvfrom(socket)
    catch error
        if expired[] && (error isa Base.IOError || error isa EOFError)
            throw(StreamTimeout(timeout))
        end
        rethrow()
    finally
        close(timer)
    end
end

"""Receive and decode one stream packet as a header and record-major matrix.

Throws [`StreamTimeout`](@ref) and closes the receiver if nothing arrives
within the receiver's timeout.
"""
function receive(receiver::StreamReceiver)
    _, packet = _recvfrom_timeout(receiver.socket, receiver.timeout)
    header = Protocol.decode_stream_header(packet)
    expected = Protocol.STREAM_HEADER_LEN +
        4 * Int(header.n_sources) * Int(header.n_records)
    length(packet) == expected || throw(
        Protocol.ProtocolError(
            "stream packet length $(length(packet)) does not match expected $expected",
        ),
    )

    if !isnothing(receiver.last_sequence)
        gap = header.seq - receiver.last_sequence - UInt32(1)
        # Count small forward gaps as loss; huge wrapped gaps are stream
        # restarts (sequence reset to zero) or reordering, not loss.
        if 0 < gap < (UInt32(1) << 16)
            receiver.lost_packets += Int(gap)
        end
    end
    receiver.last_sequence = header.seq

    io = IOBuffer(@view packet[(Protocol.STREAM_HEADER_LEN + 1):end])
    values = Matrix{Float32}(undef, Int(header.n_records), Int(header.n_sources))
    for record in axes(values, 1), source in axes(values, 2)
        values[record, source] = Protocol._read_le(io, Float32)
    end
    return header, values
end

"""A Tables.jl-compatible finite capture with stream-loss metadata."""
struct Capture{C <: NamedTuple}
    columns::C
    dropped::UInt32
    lost_packets::Int
end

Tables.istable(::Type{<:Capture}) = true
Tables.columnaccess(::Type{<:Capture}) = true
Tables.columns(capture::Capture) = capture.columns
Tables.columnnames(capture::Capture) = keys(capture.columns)
Tables.getcolumn(capture::Capture, index::Int) = getfield(capture.columns, index)
Tables.getcolumn(capture::Capture, name::Symbol) = getproperty(capture.columns, name)
Tables.schema(capture::Capture) = Tables.schema(capture.columns)

Base.length(capture::Capture) = length(first(values(capture.columns)))
Base.getindex(capture::Capture, name::Symbol) = getproperty(capture.columns, name)
Base.getindex(capture::Capture, name::AbstractString) = capture[Symbol(name)]

function Base.show(io::IO, capture::Capture)
    return print(
        io,
        "Capture($(length(capture)) rows, $(length(capture.columns) - 1) sources, ",
        "dropped=$(capture.dropped), lost_packets=$(capture.lost_packets))",
    )
end

"""Collect `n_records` from an already configured and started stream."""
function capture(receiver::StreamReceiver, n_records::Integer, names)
    n_records > 0 || throw(ArgumentError("n_records must be positive"))
    source_names = String.(names)
    symbols = Symbol.(source_names)
    length(unique(symbols)) == length(symbols) ||
        throw(ArgumentError("source names must be unique"))
    :index in symbols && throw(ArgumentError("source name 'index' is reserved"))

    indices = Vector{UInt64}(undef, n_records)
    columns = [Vector{Float32}(undef, n_records) for _ in source_names]
    initial_lost = receiver.lost_packets
    dropped = UInt32(0)
    offset = 0
    while offset < n_records
        header, values = receive(receiver)
        Int(header.n_sources) == length(source_names) || throw(
            Protocol.ProtocolError(
                "packet has $(header.n_sources) sources, expected $(length(source_names))",
            ),
        )
        packet_records = Int(header.n_records)
        packet_records > 0 || throw(Protocol.ProtocolError("stream packet has no records"))
        taken = min(packet_records, n_records - offset)
        for row in 1:taken
            indices[offset + row] = UInt64(header.first_index) +
                UInt64(row - 1) * UInt64(header.decimation)
        end
        for source in eachindex(source_names)
            copyto!(columns[source], offset + 1, @view(values[1:taken, source]), 1, taken)
        end
        offset += taken
        dropped = header.dropped
    end

    names_tuple = Tuple([:index; symbols])
    values_tuple = Tuple(Any[indices, columns...])
    table = NamedTuple{names_tuple}(values_tuple)
    return Capture(table, dropped, receiver.lost_packets - initial_lost)
end
