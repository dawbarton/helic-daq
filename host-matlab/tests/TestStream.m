% UDP packet assembly and table-interface tests.
classdef TestStream < matlab.unittest.TestCase
    methods (Test)
        function captureAndLossAccounting(testCase)
            transport = FakeDatagramTransport();
            receiver = helicdaq.StreamReceiver('Timeout', 1, ...
                'Transport', transport);
            receiverCleanup = onCleanup(@() delete(receiver));

            header = struct('NSources', uint8(2), 'Sequence', uint32(0), ...
                'FirstIndex', uint32(100), 'Dropped', uint32(3), ...
                'Decimation', uint16(2), 'NRecords', uint16(2));
            packet = TestStream.packet(header, single([1, 2; 3, 4]));
            transport.send(packet, '127.0.0.1', receiver.Port);
            data = receiver.capture(2, ["adc0", "out"]);

            testCase.verifyEqual(data.Properties.VariableNames, ...
                {'index', 'adc0', 'out'});
            testCase.verifyEqual(data.index, uint64([100; 102]));
            testCase.verifyEqual(data.adc0, single([1; 3]));
            testCase.verifyEqual(data.out, single([2; 4]));
            testCase.verifyEqual(data.Properties.UserData.Dropped, uint32(3));
            testCase.verifyEqual(data.Properties.UserData.LostPackets, 0);

            next = struct('NSources', uint8(1), 'Sequence', uint32(2), ...
                'FirstIndex', uint32(0), 'Dropped', uint32(0), ...
                'Decimation', uint16(1), 'NRecords', uint16(1));
            transport.send(TestStream.packet(next, single(5)), ...
                '127.0.0.1', receiver.Port);
            receiver.receive();
            testCase.verifyEqual(receiver.LostPackets, 1);
        end

        function timeoutIsReported(testCase)
            receiver = helicdaq.StreamReceiver('Timeout', 0.02, ...
                'Transport', FakeDatagramTransport());
            cleanup = onCleanup(@() delete(receiver));
            testCase.verifyError(@() receiver.receive(), 'helicdaq:StreamTimeout');
        end

        function primeUsesTransport(testCase)
            transport = FakeDatagramTransport();
            receiver = helicdaq.StreamReceiver('Timeout', 1, ...
                'Transport', transport);
            cleanup = onCleanup(@() delete(receiver));
            receiver.prime("127.0.0.1", 9999);
            testCase.verifyEqual(transport.PrimeHost, "127.0.0.1");
            testCase.verifyEqual(transport.PrimePort, 9999);
        end

        function nativeUdpLoopback(testCase)
            testCase.assumeTrue(exist('udpport', 'file') == 2, ...
                'Instrument Control Toolbox is not installed.');
            receiver = helicdaq.StreamReceiver('Port', 0, ...
                'BindAddress', "127.0.0.1", 'Timeout', 1);
            receiverCleanup = onCleanup(@() delete(receiver));
            sender = udpport('datagram', 'IPV4');
            senderCleanup = onCleanup(@() delete(sender));
            header = struct('NSources', uint8(1), 'Sequence', uint32(5), ...
                'FirstIndex', uint32(20), 'Dropped', uint32(0), ...
                'Decimation', uint16(1), 'NRecords', uint16(1));
            write(sender, TestStream.packet(header, single(3.5)), 'uint8', ...
                '127.0.0.1', receiver.Port);
            [decoded, values] = receiver.receive();
            testCase.verifyEqual(decoded, header);
            testCase.verifyEqual(values, single(3.5));
        end
    end

    methods (Static, Access = private)
        function packet = packet(header, values)
            rowMajor = reshape(values.', 1, []);
            packet = [helicdaq.Protocol.encodeStreamHeader(header), ...
                helicdaq.Protocol.packLE(rowMajor, 'single')];
        end
    end
end
