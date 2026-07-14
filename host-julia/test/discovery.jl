"""Directed-loopback tests for HELIC-DAQ UDP discovery."""

@testset "discovery" begin
    port = 32352
    server = UDPSocket()
    bind(server, ip"127.0.0.1", port)
    task = @async begin
        peer, request = recvfrom(server)
        @test request == P.BEACON_REQUEST
        response = P.BeaconResponse(
            0x02,
            0x092e,
            (0x02, 0x48, 0x4c, 0x00, 0x00, 0x01),
            "cbc-rig",
            "helic-daq test",
        )
        send(server, peer.host, peer.port, P.encode_beacon_response(response))
    end
    try
        devices = find_devices(; timeout=0.2, port, addresses=["127.0.0.1"])
        @test length(devices) == 1
        @test devices[1].address == ip"127.0.0.1"
        @test devices[1].control_port == 2350
        @test devices[1].mac == "02:48:4c:00:00:01"
        @test devices[1].experiment == "cbc-rig"
    finally
        wait(task)
        close(server)
    end
end
