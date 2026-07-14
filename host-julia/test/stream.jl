"""UDP packet assembly and Tables.jl interface tests."""

function stream_packet(header::P.StreamHeader, rows)
    io = IOBuffer()
    write(io, P.encode_stream_header(header))
    for row in axes(rows, 1), source in axes(rows, 2)
        P._write_le(io, Float32(rows[row, source]))
    end
    return take!(io)
end

@testset "stream timeout" begin
    receiver = StreamReceiver(; port=32355, bind_address=ip"127.0.0.1", timeout=0.02)
    try
        @test_throws StreamTimeout receive(receiver)
        @test !isopen(receiver)
    finally
        isopen(receiver) && close(receiver)
    end
end

@testset "stream receiver and Tables.jl" begin
    port = 32351
    receiver = StreamReceiver(; port, bind_address=ip"127.0.0.1", timeout=1)
    sender = UDPSocket()
    try
        first_packet = stream_packet(
            P.StreamHeader(0x02, 0x00000000, 0x00000064, 0x00000003, 0x0002, 0x0002),
            Float32[1 2; 3 4],
        )
        send(sender, ip"127.0.0.1", port, first_packet)
        result = capture(receiver, 2, ["adc0", "out"])
        @test length(result) == 2
        @test result[:index] == UInt64[100, 102]
        @test result["adc0"] == Float32[1, 3]
        @test result[:out] == Float32[2, 4]
        @test result.dropped == 3
        @test result.lost_packets == 0
        @test Tables.istable(typeof(result))
        @test Tables.columnnames(result) == (:index, :adc0, :out)
        @test Tables.columntable(result) == result.columns

        send(
            sender,
            ip"127.0.0.1",
            port,
            stream_packet(
                P.StreamHeader(0x01, 0x00000002, 0, 0, 1, 1),
                Float32[5],
            ),
        )
        receive(receiver)
        @test receiver.lost_packets == 1
    finally
        close(sender)
        isopen(receiver) && close(receiver)
    end
end
