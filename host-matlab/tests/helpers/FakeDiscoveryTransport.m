% In-memory UDP transport used by MATLAB discovery tests.
classdef FakeDiscoveryTransport < handle
    properties
        Timeout = 1
    end

    properties (Access = private)
        Packet = uint8([])
    end

    methods
        function send(obj, data, ~, ~)
            if ~isequal(uint8(data), helicdaq.Protocol.BEACON_REQUEST)
                return
            end
            response = struct('Version', helicdaq.Protocol.VERSION, ...
                'ControlPort', uint16(2350), ...
                'Mac', uint8([2, 72, 76, 0, 0, 1]), ...
                'Experiment', "cbc-rig", 'Firmware', "helic-daq test");
            obj.Packet = helicdaq.Protocol.encodeBeaconResponse(response);
        end

        function [data, address, port] = receive(obj)
            if isempty(obj.Packet)
                error('helicdaq:StreamTimeout', 'No queued discovery response.');
            end
            data = obj.Packet;
            obj.Packet = uint8([]);
            address = "127.0.0.1";
            port = 2354;
        end
    end
end
