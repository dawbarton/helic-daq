# TCP device connection, discovered parameters, and capture orchestration.

"""A device-reported error or broken control-channel invariant.

`code` is the wire error code for device-reported errors, `nothing` otherwise.
"""
struct DeviceError <: Exception
    message::String
    code::Union{Nothing, UInt8}
end

DeviceError(message::AbstractString) = DeviceError(String(message), nothing)
Base.showerror(io::IO, error::DeviceError) = print(io, error.message)

"""A discovered parameter definition whose `index` is the zero-based wire id."""
struct Parameter
    index::UInt16
    name::String
    type_code::Char
    count::Int
    writable::Bool
end

"""A discovered stream source whose `index` is the zero-based wire id."""
struct Source
    index::UInt8
    name::String
    unit::String
end

"""A HELIC-DAQ TCP control connection with connection-local discovery tables.

Connecting and every request give up after `timeout` seconds; a timed-out
connection is closed and must be reopened.
"""
mutable struct Device
    socket::TCPSocket
    host::String
    port::UInt16
    timeout::Float64
    sequence::UInt8
    parameters::Vector{Parameter}
    parameter_by_name::Dict{String, Parameter}
    sources::Vector{Source}
    source_by_name::Dict{String, Source}
end

# Julia's connect has no timeout argument, so run it in a task and give up
# after `timeout` seconds, closing the socket if it ever appears late.
function _connect_timeout(host::AbstractString, port::Integer, timeout::Real)
    task = @async connect(host, port)
    if timedwait(() -> istaskdone(task), Float64(timeout)) === :timed_out
        @async try
            close(fetch(task))
        catch
        end
        throw(DeviceError("connection to $host:$port timed out"))
    end
    return try
        fetch(task)
    catch error
        error isa TaskFailedException ? throw(error.task.exception) : rethrow()
    end
end

function Device(
        host::AbstractString;
        port::Integer = Protocol.CONTROL_PORT,
        timeout::Real = 5.0,
    )
    1 <= port <= typemax(UInt16) ||
        throw(ArgumentError("port must be between 1 and $(typemax(UInt16))"))
    timeout > 0 || throw(ArgumentError("timeout must be positive"))
    socket = _connect_timeout(host, port, timeout)
    Sockets.nagle(socket, false)  # request/response protocol: no batching delay
    device = Device(
        socket,
        String(host),
        UInt16(port),
        Float64(timeout),
        UInt8(0),
        Parameter[],
        Dict{String, Parameter}(),
        Source[],
        Dict{String, Source}(),
    )
    try
        initial_status = _decode_status(_request(device, Protocol.STATUS))
        initial_status.protocol_version == Protocol.VERSION || throw(
            DeviceError(
                "protocol version mismatch: device $(initial_status.protocol_version), " *
                    "host $(Protocol.VERSION)",
            ),
        )
        _discover!(device, initial_status.n_params)
        length(device.parameters) == initial_status.n_params ||
            throw(Protocol.ProtocolError("parameter discovery length does not match Status"))
        length(device.sources) == initial_status.n_sources ||
            throw(Protocol.ProtocolError("source discovery length does not match Status"))
        return device
    catch
        close(socket)
        rethrow()
    end
end

Base.close(device::Device) = close(device.socket)
Base.isopen(device::Device) = isopen(device.socket)

function Base.open(f::Function, ::Type{Device}, host::AbstractString; kwargs...)
    device = Device(host; kwargs...)
    try
        return f(device)
    finally
        close(device)
    end
end

function _request(device::Device, message_type, payload = UInt8[])
    device.sequence += 0x01  # UInt8 arithmetic wraps 255 -> 0
    # Julia sockets have no native read timeout, so a Timer closes the socket
    # to interrupt a blocked read; a timed-out connection stays closed.
    expired = Ref(false)
    timer = Timer(device.timeout) do _
        expired[] = true
        isopen(device.socket) && close(device.socket)
    end
    response = try
        write(device.socket, Protocol.encode_frame(message_type, device.sequence, payload))
        flush(device.socket)
        header = read(device.socket, Protocol.HEADER_LEN)
        length(header) == Protocol.HEADER_LEN ||
            throw(DeviceError("connection closed by device"))
        payload_length = Int(UInt16(header[5]) | (UInt16(header[6]) << 8))
        rest = read(device.socket, payload_length + Protocol.TRAILER_LEN)
        length(rest) == payload_length + Protocol.TRAILER_LEN ||
            throw(DeviceError("connection closed by device"))
        Protocol.decode_frame([header; rest])
    catch
        expired[] &&
            throw(DeviceError("no response from device within $(device.timeout) seconds"))
        rethrow()
    finally
        close(timer)
    end
    response.sequence == device.sequence || throw(
        DeviceError(
            "sequence mismatch: sent $(device.sequence), received $(response.sequence)",
        ),
    )
    if response.message_type == UInt8(Protocol.ERROR)
        code = isempty(response.payload) ? UInt8(0) : response.payload[1]
        name = get(Protocol.ERROR_NAMES, code, "code $code")
        throw(DeviceError("device error: $name", code))
    end
    response.message_type == UInt8(message_type) || throw(
        DeviceError(
            "response type mismatch: $(response.message_type) != $(UInt8(message_type))",
        ),
    )
    return response.payload
end

function _discover!(device::Device, n_params::Integer)
    definitions = NamedTuple[]
    start = 0
    while start < n_params
        page = Protocol.decode_param_page(
            _request(device, Protocol.GET_PARAMS, Protocol.encode_param_page_request(start)),
        )
        page.start == start || throw(
            Protocol.ProtocolError(
                "parameter page starts at $(page.start), expected $start",
            ),
        )
        page.next_index <= n_params ||
            throw(Protocol.ProtocolError("parameter page exceeds Status parameter count"))
        page.next_index > start ||
            throw(Protocol.ProtocolError("parameter page did not advance"))
        length(page.definitions) == page.next_index - start || throw(
            Protocol.ProtocolError(
                "parameter page definition count does not match its indices",
            ),
        )
        append!(definitions, page.definitions)
        start = Int(page.next_index)
    end
    allunique(definition.name for definition in definitions) ||
        throw(Protocol.ProtocolError("parameter discovery contains duplicate names"))
    device.parameters = [
        Parameter(UInt16(index - 1), def.name, def.type_code, Int(def.count), def.writable) for
            (index, def) in enumerate(definitions)
    ]
    device.parameter_by_name = Dict(parameter.name => parameter for parameter in device.parameters)

    definitions = Protocol.decode_sources(_request(device, Protocol.GET_SOURCES))
    device.sources = [
        Source(UInt8(index - 1), def.name, def.unit) for
            (index, def) in enumerate(definitions)
    ]
    device.source_by_name = Dict(source.name => source for source in device.sources)
    return device
end

"""Return the discovered definition for a parameter name."""
function parameter(device::Device, name::Union{AbstractString, Symbol})
    key = String(name)
    haskey(device.parameter_by_name, key) ||
        throw(DeviceError("no parameter named '$key'"))
    return device.parameter_by_name[key]
end

function _source(device::Device, name::Union{AbstractString, Symbol})
    key = String(name)
    haskey(device.source_by_name, key) || begin
        choices = join(("$(source.name) [$(source.unit)]" for source in device.sources), ", ")
        throw(DeviceError("unknown source '$key'; discovered sources: $choices"))
    end
    return device.source_by_name[key]
end

const WIRE_TYPES = Dict(
    'B' => UInt8,
    'b' => Int8,
    'H' => UInt16,
    'h' => Int16,
    'I' => UInt32,
    'i' => Int32,
    'f' => Float32,
    'c' => UInt8,
)

_wire_type(code::Char) = get(WIRE_TYPES, code) do
    throw(Protocol.ProtocolError("invalid parameter type code '$code'"))
end

# Encoded size of a parameter value on the wire (not the struct's own size).
_wire_size(parameter::Parameter) = sizeof(_wire_type(parameter.type_code)) * parameter.count

function _pack_value(parameter::Parameter, value)
    if parameter.type_code == 'c'
        value isa Union{AbstractString, Symbol} ||
            throw(ArgumentError("character parameters take a string or symbol"))
        text = String(value)
        isascii(text) || throw(ArgumentError("character parameters must be ASCII"))
        bytes = collect(codeunits(text))
        resize!(bytes, min(length(bytes), parameter.count))
        append!(bytes, zeros(UInt8, parameter.count - length(bytes)))
        return bytes
    end

    values = parameter.count == 1 ? Any[value] : collect(value)
    length(values) == parameter.count || throw(
        DeviceError(
            "parameter '$(parameter.name)' expects $(parameter.count) values, " *
                "received $(length(values))",
        ),
    )
    type = _wire_type(parameter.type_code)
    io = IOBuffer()
    for item in values
        Protocol._write_le(io, convert(type, item))
    end
    return take!(io)
end

function _unpack_value(parameter::Parameter, bytes::AbstractVector{UInt8})
    length(bytes) == _wire_size(parameter) ||
        throw(Protocol.ProtocolError("parameter value has an invalid length"))
    if parameter.type_code == 'c'
        ending = something(findfirst(==(0x00), bytes), length(bytes) + 1)
        return String(@view bytes[1:(ending - 1)])
    end
    type = _wire_type(parameter.type_code)
    io = IOBuffer(bytes)
    values = [Protocol._read_le(io, type) for _ in 1:parameter.count]
    return parameter.count == 1 ? only(values) : values
end

"""Read several named parameters in one round trip and return a named tuple."""
function getparams(device::Device, names)
    parameters = [parameter(device, name) for name in names]
    isempty(parameters) && throw(ArgumentError("at least one parameter is required"))
    allunique(definition.index for definition in parameters) ||
        throw(ArgumentError("parameter names must be unique"))
    total_size = sum(_wire_size, parameters)
    total_size <= Protocol.MAX_PAYLOAD || throw(
        DeviceError(
            "requested values need $total_size bytes; responses are limited to " *
                "$(Protocol.MAX_PAYLOAD) bytes",
        ),
    )
    request = IOBuffer()
    for definition in parameters
        Protocol._write_le(request, definition.index)
    end
    response = _request(device, Protocol.GET_PAR, take!(request))
    length(response) == total_size ||
        throw(Protocol.ProtocolError("GetPar response has an invalid length"))

    offset = 1
    result = Any[]
    for definition in parameters
        ending = offset + _wire_size(definition) - 1
        push!(result, _unpack_value(definition, @view response[offset:ending]))
        offset = ending + 1
    end
    keys = Tuple(Symbol(definition.name) for definition in parameters)
    return NamedTuple{keys}(Tuple(result))
end

"""Read one parameter by its discovered name."""
getparam(device::Device, name::Union{AbstractString, Symbol}) =
    first(values(getparams(device, (name,))))

"""Write one parameter by its discovered name."""
function setparam!(device::Device, name::Union{AbstractString, Symbol}, value)
    definition = parameter(device, name)
    definition.writable || throw(DeviceError("parameter '$(definition.name)' is read-only"))
    payload = IOBuffer()
    Protocol._write_le(payload, definition.index)
    write(payload, _pack_value(definition, value))
    _request(device, Protocol.SET_PAR, take!(payload))
    return device
end

Base.getindex(device::Device, name::Union{AbstractString, Symbol}) = getparam(device, name)

function Base.setindex!(device::Device, value, name::Union{AbstractString, Symbol})
    setparam!(device, name, value)
    return value
end

function _decode_status(payload::AbstractVector{UInt8})
    length(payload) == 12 || throw(Protocol.ProtocolError("invalid Status payload length"))
    io = IOBuffer(payload)
    return (
        protocol_version = Protocol._read_le(io, UInt8),
        n_params = Int(Protocol._read_le(io, UInt16)),
        n_sources = Int(Protocol._read_le(io, UInt8)),
        sample_rate = Protocol._read_le(io, Float32),
        uptime = Protocol._read_le(io, UInt32) / 1000,
    )
end

"""Return protocol, registry, sample-rate, and uptime status as a named tuple."""
status(device::Device) = _decode_status(_request(device, Protocol.STATUS))

"""Configure selected discovered sources, decimation, and record count."""
function configure_stream!(device::Device, sources; decimation::Integer = 1, count::Integer = 0)
    1 <= decimation <= typemax(UInt16) ||
        throw(ArgumentError("decimation must fit a positive UInt16"))
    0 <= count <= typemax(UInt32) || throw(ArgumentError("count must fit a UInt32"))
    resolved = [source isa Source ? source : _source(device, source) for source in sources]
    1 <= length(resolved) <= typemax(UInt8) ||
        throw(ArgumentError("between 1 and $(typemax(UInt8)) sources are required"))
    payload = IOBuffer()
    Protocol._write_le(payload, UInt16(decimation))
    Protocol._write_le(payload, UInt32(count))
    Protocol._write_le(payload, UInt8(length(resolved)))
    write(payload, UInt8[source.index for source in resolved])
    _request(device, Protocol.STREAM_SETUP, take!(payload))
    return resolved
end

"""Start the configured stream to the TCP peer address and given UDP port."""
function start_stream!(device::Device, port::Integer = Protocol.STREAM_PORT)
    payload = IOBuffer()
    Protocol._write_le(payload, UInt16(port))
    _request(device, Protocol.STREAM_START, take!(payload))
    return device
end

"""Stop an active stream."""
stop_stream!(device::Device) = (_request(device, Protocol.STREAM_STOP); device)

"""Configure, receive, and stop a finite capture by sample count or duration."""
function capture(
        device::Device,
        sources;
        samples::Union{Nothing, Integer} = nothing,
        seconds::Union{Nothing, Real} = nothing,
        decimation::Integer = 1,
        port::Integer = Protocol.STREAM_PORT,
        timeout::Real = 2.0,
    )
    isnothing(samples) == isnothing(seconds) &&
        throw(ArgumentError("specify exactly one of samples or seconds"))
    if isnothing(samples)
        seconds > 0 || throw(ArgumentError("seconds must be positive"))
        samples = max(1, floor(Int, seconds * status(device).sample_rate / decimation))
    end
    samples > 0 || throw(ArgumentError("samples must be positive"))
    samples <= typemax(UInt32) || throw(ArgumentError("samples must fit a UInt32"))
    resolved = configure_stream!(device, sources; decimation, count = samples)
    receiver = StreamReceiver(; port, timeout)
    started = false
    try
        prime!(receiver, device.host)
        start_stream!(device, receiver.port)
        started = true
        return capture(receiver, samples, (source.name for source in resolved))
    finally
        if started && isopen(device)
            stop_stream!(device)
        end
        isopen(receiver) && close(receiver)
    end
end

function _request_with_busy_retry(
        device::Device,
        message_type,
        payload;
        timeout::Real = 1.0,
    )
    deadline = time() + timeout
    while true
        try
            return _request(device, message_type, payload)
        catch error
            if !(error isa DeviceError && error.code == Protocol.ERROR_BUSY && time() < deadline)
                rethrow()
            end
            sleep(0.005)
        end
    end
    return  # unreachable; Runic requires an explicit return
end

"""Stage and atomically activate a finite arbitrary waveform table.

`interpolation` is `:linear` or zero-order `:hold`.
"""
function upload_table!(
        device::Device,
        values;
        duration::Union{Nothing, Real} = nothing,
        frequency::Union{Nothing, Real} = nothing,
        gain::Real = 1,
        mode::Symbol = :loop,
        interpolation::Symbol = :linear,
        multiplier::Integer = 1,
        phase::Real = 0,
    )
    table_values = Float32.(values)
    table = parameter(device, "table")
    2 <= length(table_values) <= table.count ||
        throw(ArgumentError("table length must be between 2 and $(table.count)"))
    all(isfinite, table_values) || throw(ArgumentError("table values must be finite"))
    !isnothing(duration) && !isnothing(frequency) &&
        throw(ArgumentError("specify at most one of duration or frequency"))
    if !isnothing(duration)
        duration > 0 || throw(ArgumentError("duration must be positive"))
        frequency = inv(duration)
    end
    isnothing(frequency) || frequency > 0 ||
        throw(ArgumentError("frequency must be positive"))
    modes = Dict(:off => 0, :loop => 1, :one_shot => 2, :locked => 3, :locked_one_shot => 4)
    haskey(modes, mode) || throw(ArgumentError("unknown table mode :$mode"))
    interpolations = Dict(:hold => 0, :linear => 1)
    haskey(interpolations, interpolation) ||
        throw(ArgumentError("unknown table interpolation :$interpolation"))
    multiplier >= 1 || throw(ArgumentError("multiplier must be at least 1"))
    0 <= phase < 1 || throw(ArgumentError("phase must be in [0, 1)"))

    raw = IOBuffer()
    for value in table_values
        Protocol._write_le(raw, value)
    end
    bytes = take!(raw)
    chunk_size = div(Protocol.MAX_PAYLOAD - 6, 4) * 4
    for byte_offset in 0:chunk_size:(length(bytes) - 1)
        ending = min(byte_offset + chunk_size, length(bytes))
        payload = Protocol.encode_set_block(
            table.index,
            div(byte_offset, 4),
            @view(bytes[(byte_offset + 1):ending]),
        )
        _request_with_busy_retry(device, Protocol.SET_BLOCK, payload)
    end
    _request_with_busy_retry(
        device,
        Protocol.COMMIT,
        Protocol.encode_commit(table.index, length(table_values)),
    )
    !isnothing(frequency) && setparam!(device, "table_freq", frequency)
    setparam!(device, "table_gain", gain)
    setparam!(device, "table_interp", interpolations[interpolation])
    setparam!(device, "table_mult", multiplier)
    setparam!(device, "table_phase", phase)
    mode_value = modes[mode]
    setparam!(device, "table_mode", mode_value)
    mode_value in (2, 4) && setparam!(device, "table_trigger", 1)
    return device
end
