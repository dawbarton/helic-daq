% In-memory datagram transport used by MATLAB stream tests.
classdef FakeDatagramTransport < handle
    properties
        Port = 2351
        Timeout = 1
        PrimeHost = ""
        PrimePort = []
    end

    properties (Access = private)
        Packets = {}
    end

    methods
        function send(obj, data, ~, ~)
            obj.Packets{end + 1} = reshape(uint8(data), 1, []);
        end

        function data = receive(obj)
            if isempty(obj.Packets)
                error('helicdaq:StreamTimeout', ...
                    'No HELIC-DAQ stream packet arrived within %.3g seconds.', ...
                    obj.Timeout);
            end
            data = obj.Packets{1};
            obj.Packets(1) = [];
        end

        function prime(obj, host, port)
            obj.PrimeHost = string(host);
            obj.PrimePort = double(port);
        end
    end
end
