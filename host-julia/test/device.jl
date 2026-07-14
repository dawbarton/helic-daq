"""End-to-end device API tests against a small in-process protocol peer."""

function mock_raw(type_code::Char, count::Int, value)
    if type_code == 'c'
        bytes = collect(codeunits(String(value)))
        resize!(bytes, min(length(bytes), count))
        append!(bytes, zeros(UInt8, count - length(bytes)))
        return bytes
    end
    values = count == 1 ? Any[value] : collect(value)
    type = HelicDAQ._wire_type(type_code)
    io = IOBuffer()
    for item in values
        P._write_le(io, convert(type, item))
    end
    return take!(io)
end

function mock_discovery_payload(parameters)
    io = IOBuffer()
    for parameter in parameters
        write(io, codeunits(parameter.name))
        write(io, UInt8(0))
        write(io, UInt8(parameter.type_code))
        P._write_le(io, UInt16(parameter.count))
        write(io, UInt8(parameter.writable))
    end
    return take!(io)
end

function mock_sources_payload(sources)
    io = IOBuffer()
    for source in sources
        write(io, codeunits(source.name))
        write(io, UInt8(0))
        write(io, codeunits(source.unit))
        write(io, UInt8(0))
    end
    return take!(io)
end

function mock_status_payload(n_params, n_sources)
    io = IOBuffer()
    P._write_le(io, P.VERSION)
    P._write_le(io, UInt16(n_params))
    P._write_le(io, UInt8(n_sources))
    P._write_le(io, Float32(1000))
    P._write_le(io, UInt32(42000))
    return take!(io)
end

function read_mock_request(socket)
    header = read(socket, P.HEADER_LEN)
    isempty(header) && return nothing
    length(header) == P.HEADER_LEN || error("truncated test request")
    payload_length = Int(UInt16(header[5]) | UInt16(header[6]) << 8)
    return P.decode_frame([header; read(socket, payload_length + P.TRAILER_LEN)])
end

function send_mock_stream(port::UInt16, ids, count::UInt32, decimation::UInt16)
    socket = UDPSocket()
    try
        header = P.StreamHeader(
            UInt8(length(ids)),
            UInt32(0),
            UInt32(100),
            UInt32(0),
            decimation,
            UInt16(count),
        )
        io = IOBuffer()
        write(io, P.encode_stream_header(header))
        for record in 0:(Int(count) - 1), id in ids
            value = id == 0 ? Float32(record) : Float32(2record)
            P._write_le(io, value)
        end
        send(socket, ip"127.0.0.1", port, take!(io))
    finally
        close(socket)
    end
end

function start_mock_device()
    parameters = [
        (name="firmware", type_code='c', count=16, writable=false, value="helic-daq test"),
        (name="sample_freq", type_code='f', count=1, writable=false, value=Float32(1000)),
        (name="freq", type_code='f', count=1, writable=true, value=Float32(0)),
        (name="forcing_coeffs", type_code='f', count=5, writable=true, value=zeros(Float32, 5)),
        (name="table", type_code='f', count=8, writable=true, value=zeros(Float32, 8)),
        (name="table_len", type_code='H', count=1, writable=false, value=UInt16(0)),
        (name="table_freq", type_code='f', count=1, writable=true, value=Float32(0)),
        (name="table_gain", type_code='f', count=1, writable=true, value=Float32(1)),
        (name="table_mode", type_code='I', count=1, writable=true, value=UInt32(0)),
        (name="table_mult", type_code='I', count=1, writable=true, value=UInt32(1)),
        (name="table_phase", type_code='f', count=1, writable=true, value=Float32(0)),
        (name="table_trigger", type_code='I', count=1, writable=true, value=UInt32(0)),
    ]
    sources = [(name="adc0", unit="V"), (name="out", unit="V")]
    values = [mock_raw(p.type_code, p.count, p.value) for p in parameters]
    staged = copy(values[5])
    listener = listen(ip"127.0.0.1", 0)
    _, port = getsockname(listener)

    task = errormonitor(@async begin
        socket = accept(listener)
        selected_ids = UInt8[]
        stream_count = UInt32(0)
        stream_decimation = UInt16(1)
        try
            while true
                request = try
                    read_mock_request(socket)
                catch error
                    error isa EOFError ? nothing : rethrow()
                end
                isnothing(request) && break
                payload = UInt8[]
                send_stream_to = nothing
                if request.message_type == UInt8(P.STATUS)
                    payload = mock_status_payload(length(parameters), length(sources))
                elseif request.message_type == UInt8(P.GET_PARAMS)
                    payload = mock_discovery_payload(parameters)
                elseif request.message_type == UInt8(P.GET_SOURCES)
                    payload = mock_sources_payload(sources)
                elseif request.message_type == UInt8(P.GET_PAR)
                    io = IOBuffer(request.payload)
                    output = IOBuffer()
                    while !eof(io)
                        index = Int(P._read_le(io, UInt16)) + 1
                        write(output, values[index])
                    end
                    payload = take!(output)
                elseif request.message_type == UInt8(P.SET_PAR)
                    io = IOBuffer(request.payload)
                    index = Int(P._read_le(io, UInt16)) + 1
                    values[index] = read(io)
                elseif request.message_type == UInt8(P.SET_BLOCK)
                    io = IOBuffer(request.payload)
                    index = Int(P._read_le(io, UInt16)) + 1
                    @test parameters[index].name == "table"
                    offset = Int(P._read_le(io, UInt32)) * 4
                    data = read(io)
                    copyto!(staged, offset + 1, data, 1, length(data))
                elseif request.message_type == UInt8(P.COMMIT)
                    io = IOBuffer(request.payload)
                    index = Int(P._read_le(io, UInt16)) + 1
                    @test parameters[index].name == "table"
                    commit_length = P._read_le(io, UInt32)
                    values[5] = copy(staged)
                    values[6] = mock_raw('H', 1, UInt16(commit_length))
                elseif request.message_type == UInt8(P.STREAM_SETUP)
                    io = IOBuffer(request.payload)
                    stream_decimation = P._read_le(io, UInt16)
                    stream_count = P._read_le(io, UInt32)
                    n_sources = Int(P._read_le(io, UInt8))
                    selected_ids = read(io, n_sources)
                elseif request.message_type == UInt8(P.STREAM_START)
                    io = IOBuffer(request.payload)
                    send_stream_to = P._read_le(io, UInt16)
                elseif request.message_type != UInt8(P.STREAM_STOP)
                    error("unsupported mock request $(request.message_type)")
                end
                write(socket, P.encode_frame(request.message_type, request.sequence, payload))
                flush(socket)
                if !isnothing(send_stream_to)
                    send_mock_stream(
                        send_stream_to,
                        selected_ids,
                        stream_count,
                        stream_decimation,
                    )
                end
            end
        finally
            close(socket)
        end
    end)
    return (; listener, port=Int(port), task, values)
end

@testset "device" begin
    mock = start_mock_device()
    device = Device("127.0.0.1"; port=mock.port)
    try
        @test length(device.parameters) == 12
        @test parameter(device, :freq).index == 2
        @test device["firmware"] == "helic-daq test"
        @test status(device) == (
            protocol_version=UInt8(2),
            n_params=12,
            n_sources=2,
            sample_rate=Float32(1000),
            uptime=42.0,
        )
        device[:freq] = 12.5f0
        @test device[:freq] == 12.5f0
        values = getparams(device, (:firmware, :freq))
        @test values.firmware == "helic-daq test"
        @test values.freq == 12.5f0
        @test_throws DeviceError setparam!(device, :firmware, "x")

        upload_table!(device, [0, 1, 0, -1]; duration=0.2, gain=2, mode=:one_shot)
        @test device[:table_len] == 4
        @test device[:table_freq] == 5.0f0
        @test device[:table_gain] == 2.0f0
        @test device[:table_mode] == UInt32(2)
        @test device[:table_trigger] == UInt32(1)

        result = capture(
            device,
            [:adc0, :out];
            samples=4,
            decimation=2,
            port=32353,
            timeout=1,
        )
        @test result[:index] == UInt64[100, 102, 104, 106]
        @test result[:adc0] == Float32[0, 1, 2, 3]
        @test result[:out] == Float32[0, 2, 4, 6]
    finally
        close(device)
        wait(mock.task)
        close(mock.listener)
    end

    opened = start_mock_device()
    answer = open(Device, "127.0.0.1"; port=opened.port) do connected
        connected[:freq]
    end
    @test answer == 0.0f0
    wait(opened.task)
    close(opened.listener)
end
