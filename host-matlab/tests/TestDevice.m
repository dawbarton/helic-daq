% Device API integration tests against an in-memory protocol peer.
classdef TestDevice < matlab.unittest.TestCase
    methods (Test)
        function discoveryParametersAndStatus(testCase)
            transport = FakeTransport();
            device = helicdaq.Device("test", 'Transport', transport);
            cleanup = onCleanup(@() delete(device));

            testCase.verifyEqual(height(device.Parameters), 13);
            testCase.verifyEqual(device.parameter('freq').Index, uint16(2));
            testCase.verifyEqual(device.getParameter('firmware'), "helic-daq test");
            information = device.status();
            testCase.verifyEqual(information.ProtocolVersion, uint8(2));
            testCase.verifyEqual(information.ParameterCount, 13);
            testCase.verifyEqual(information.SourceCount, 2);
            testCase.verifyEqual(information.SampleRate, single(1000));
            testCase.verifyEqual(information.Uptime, seconds(42));

            device.setParameter('freq', 12.5);
            testCase.verifyEqual(device.getParameter('freq'), single(12.5));
            values = device.getParameters(["firmware", "freq"]);
            testCase.verifyEqual(values.Value{1}, "helic-daq test");
            testCase.verifyEqual(values.Value{2}, single(12.5));
            testCase.verifyError(@() device.setParameter('firmware', "x"), ...
                'helicdaq:ReadOnly');
        end

        function tableUpload(testCase)
            transport = FakeTransport();
            device = helicdaq.Device("test", 'Transport', transport);
            cleanup = onCleanup(@() delete(device));
            device.uploadTable([0, 1, 0, -1], 'Duration', 0.2, ...
                'Gain', 2, 'Mode', "one-shot", 'Interpolation', "hold");
            testCase.verifyEqual(transport.parameterValue('table_len'), uint16(4));
            testCase.verifyEqual(transport.parameterValue('table_freq'), single(5));
            testCase.verifyEqual(transport.parameterValue('table_gain'), single(2));
            testCase.verifyEqual(transport.parameterValue('table_interp'), uint32(0));
            testCase.verifyEqual(transport.parameterValue('table_mode'), uint32(2));
            testCase.verifyEqual(transport.parameterValue('table_trigger'), uint32(1));
        end

        function finiteCaptureReturnsTable(testCase)
            transport = FakeTransport();
            device = helicdaq.Device("test", 'Transport', transport);
            cleanup = onCleanup(@() delete(device));
            receiver = FakeCaptureReceiver();
            data = device.capture(["adc0", "out"], 'Samples', 4, ...
                'Decimation', 2, 'Timeout', 1, 'Receiver', receiver);
            testCase.verifyEqual(receiver.PrimeHost, "test");
            testCase.verifyEqual(receiver.PrimePort, ...
                double(helicdaq.Protocol.STREAM_PORT));
            testCase.verifyEqual(data.index, uint64([100; 102; 104; 106]));
            testCase.verifyEqual(data.adc0, single([0; 1; 2; 3]));
            testCase.verifyEqual(data.out, single([0; 2; 4; 6]));
            testCase.verifyEqual(data.Properties.VariableUnits, {'', 'V', 'V'});
            testCase.verifyEqual(data.Properties.UserData.Dropped, uint32(0));
            testCase.verifyEqual(data.Properties.UserData.LostPackets, 0);
        end

        function finiteCaptureAcceptsDuration(testCase)
            transport = FakeTransport();
            device = helicdaq.Device("test", 'Transport', transport);
            cleanup = onCleanup(@() delete(device));
            receiver = FakeCaptureReceiver();
            data = device.capture("adc0", 'Seconds', 0.008, ...
                'Decimation', 2, 'Timeout', 1, 'Receiver', receiver);
            testCase.verifyEqual(height(data), 4);
            testCase.verifyEqual(data.index, uint64([100; 102; 104; 106]));
        end
    end
end
