% Directed-loopback tests for HELIC-DAQ UDP discovery.
classdef TestDiscovery < matlab.unittest.TestCase
    methods (Test)
        function directedDiscovery(testCase)
            server = FakeDiscoveryTransport();
            cleanup = onCleanup(@() delete(server));
            devices = helicdaq.findDevices('Timeout', 0.2, ...
                'Port', 2354, 'Addresses', "127.0.0.1", ...
                'Transport', server);

            testCase.verifyEqual(height(devices), 1);
            testCase.verifyEqual(devices.Address, "127.0.0.1");
            testCase.verifyEqual(devices.ControlPort, uint16(2350));
            testCase.verifyEqual(devices.Mac, "02:48:4c:00:00:01");
            testCase.verifyEqual(devices.Experiment, "cbc-rig");
        end

        function nativeDirectedDiscovery(testCase)
            testCase.assumeTrue(exist('udpport', 'file') == 2, ...
                'Instrument Control Toolbox is not installed.');
            server = udpport('datagram', 'IPV4');
            cleanup = onCleanup(@() delete(server));
            configureCallback(server, 'datagram', 1, ...
                @(source, event) TestDiscovery.respond(source, event));
            % Allow callback registration and toolbox initialisation to settle.
            pause(0.05);
            devices = helicdaq.findDevices('Timeout', 0.5, ...
                'Port', server.LocalPort, 'Addresses', "127.0.0.1");
            configureCallback(server, 'off');
            testCase.verifyEqual(height(devices), 1);
            testCase.verifyEqual(devices.Experiment, "cbc-rig");
        end
    end

    methods (Static, Access = private)
        function respond(server, ~)
            datagram = read(server, 1, 'uint8');
            if ~isequal(uint8(datagram.Data), helicdaq.Protocol.BEACON_REQUEST)
                return
            end
            response = struct('Version', helicdaq.Protocol.VERSION, ...
                'ControlPort', uint16(2350), ...
                'Mac', uint8([2, 72, 76, 0, 0, 1]), ...
                'Experiment', "cbc-rig", 'Firmware', "helic-daq test");
            write(server, helicdaq.Protocol.encodeBeaconResponse(response), ...
                'uint8', datagram.SenderAddress, datagram.SenderPort);
        end
    end
end
