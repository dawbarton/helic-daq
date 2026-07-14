"""Known-answer and validation tests for the Julia protocol codec."""

const P = HelicDAQ.Protocol

@testset "protocol" begin
    @test P.crc16(codeunits("123456789")) == 0x29b1
    @test P.crc16(UInt8[]) == 0xffff
    @test P.crc16(UInt8[0]) == 0xe1f0

    @test P.encode_frame(P.GET_PARAMS, 1) == hex2bytes("484c0101000044c5")
    @test P.encode_frame(P.GET_SOURCES, 1) == hex2bytes("484c02010000985e")
    block = P.encode_set_block(12, 0x01020304, UInt8[0xaa, 0xbb])
    @test P.encode_frame(P.SET_BLOCK, 2, block) ==
          hex2bytes("484c050208000c0004030201aabb39a7")
    commit = P.encode_commit(12, 0x01020304)
    @test P.encode_frame(P.COMMIT, 3, commit) ==
          hex2bytes("484c060306000c000403020108d1")

    frame = P.encode_frame(P.GET_PAR, 7, UInt8[1, 0, 2, 0])
    decoded = P.decode_frame(frame)
    @test decoded == (message_type=UInt8(P.GET_PAR), sequence=0x07, payload=UInt8[1, 0, 2, 0])
    corrupt = copy(frame)
    corrupt[7] ⊻= 0xff
    @test_throws P.ProtocolError P.decode_frame(corrupt)
    @test_throws P.ProtocolError P.encode_frame(P.GET_PAR, 0, zeros(UInt8, P.MAX_PAYLOAD + 1))

    params = P.decode_params(UInt8[codeunits("freq"); 0; UInt8('f'); 1; 0; 1])
    @test params == [(name="freq", type_code='f', count=UInt16(1), writable=true)]
    sources = P.decode_sources(UInt8[codeunits("adc0"); 0; codeunits("V"); 0])
    @test sources == [(name="adc0", unit="V")]
    @test_throws P.ProtocolError P.decode_sources(UInt8[codeunits("adc0"); 0; UInt8('V')])

    for (code, value) in (
        ('B', UInt8(200)),
        ('b', Int8(-100)),
        ('H', UInt16(50000)),
        ('h', Int16(-20000)),
        ('I', UInt32(4_000_000_000)),
        ('i', Int32(-2_000_000_000)),
        ('f', Float32(1.25)),
    )
        definition = Parameter(0, "value", code, 1, true)
        @test HelicDAQ._unpack_value(definition, HelicDAQ._pack_value(definition, value)) == value
    end
    text = Parameter(0, "text", 'c', 8, true)
    @test HelicDAQ._unpack_value(text, HelicDAQ._pack_value(text, "cbc")) == "cbc"

    beacon = P.BeaconResponse(
        0x02,
        0x092e,
        (0x02, 0x48, 0x4c, 0x00, 0x00, 0x01),
        "cbc-rig",
        "helic-daq sim",
    )
    encoded_beacon = P.encode_beacon_response(beacon)
    @test encoded_beacon == hex2bytes(
        "484c02022e0902484c0000016362632d726967000000000000000000" *
        "68656c69632d6461712073696d000000",
    )
    decoded_beacon = P.decode_beacon_response(encoded_beacon)
    @test decoded_beacon.version == beacon.version
    @test decoded_beacon.mac == beacon.mac
    @test decoded_beacon.experiment == beacon.experiment

    header = P.StreamHeader(0x02, 0x00000007, 0x0000002a, 0x00000003, 0x0002, 0x001c)
    @test P.decode_stream_header(P.encode_stream_header(header)) == header
end
