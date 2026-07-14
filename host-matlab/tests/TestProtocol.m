% Known-answer and validation tests for the MATLAB protocol codec.
classdef TestProtocol < matlab.unittest.TestCase
    methods (Test)
        function crcKnownAnswers(testCase)
            testCase.verifyEqual(helicdaq.Protocol.crc16(uint8('123456789')), ...
                uint16(hex2dec('29B1')));
            testCase.verifyEqual(helicdaq.Protocol.crc16(uint8([])), ...
                uint16(hex2dec('FFFF')));
            testCase.verifyEqual(helicdaq.Protocol.crc16(uint8(0)), ...
                uint16(hex2dec('E1F0')));
        end

        function frameKnownAnswers(testCase)
            testCase.verifyEqual( ...
                helicdaq.Protocol.encodeFrame(helicdaq.Protocol.GET_PARAMS, 1), ...
                TestProtocol.hexBytes('48 4C 01 01 00 00 44 C5'));
            testCase.verifyEqual( ...
                helicdaq.Protocol.encodeFrame(helicdaq.Protocol.GET_SOURCES, 1), ...
                TestProtocol.hexBytes('48 4C 02 01 00 00 98 5E'));
            block = helicdaq.Protocol.encodeSetBlock(12, hex2dec('01020304'), ...
                uint8([hex2dec('AA'), hex2dec('BB')]));
            testCase.verifyEqual( ...
                helicdaq.Protocol.encodeFrame(helicdaq.Protocol.SET_BLOCK, 2, block), ...
                TestProtocol.hexBytes( ...
                '48 4C 05 02 08 00 0C 00 04 03 02 01 AA BB 39 A7'));
            commit = helicdaq.Protocol.encodeCommit(12, hex2dec('01020304'));
            testCase.verifyEqual( ...
                helicdaq.Protocol.encodeFrame(helicdaq.Protocol.COMMIT, 3, commit), ...
                TestProtocol.hexBytes( ...
                '48 4C 06 03 06 00 0C 00 04 03 02 01 08 D1'));
        end

        function frameRoundTripAndValidation(testCase)
            frame = helicdaq.Protocol.encodeFrame(helicdaq.Protocol.GET_PAR, 7, ...
                uint8([1, 0, 2, 0]));
            decoded = helicdaq.Protocol.decodeFrame(frame);
            testCase.verifyEqual(decoded.MessageType, helicdaq.Protocol.GET_PAR);
            testCase.verifyEqual(decoded.Sequence, uint8(7));
            testCase.verifyEqual(decoded.Payload, uint8([1, 0, 2, 0]));
            frame(7) = bitxor(frame(7), uint8(255));
            testCase.verifyError(@() helicdaq.Protocol.decodeFrame(frame), ...
                'helicdaq:ProtocolError');
            testCase.verifyError(@() helicdaq.Protocol.encodeFrame(3, 0, ...
                zeros(1, helicdaq.Protocol.MAX_PAYLOAD + 1, 'uint8')), ...
                'helicdaq:ProtocolError');
        end

        function discoveryEntries(testCase)
            parameters = helicdaq.Protocol.decodeParams( ...
                uint8([uint8('freq'), 0, uint8('f'), 1, 0, 1]));
            testCase.verifyEqual(parameters.Name, "freq");
            testCase.verifyEqual(parameters.TypeCode, "f");
            testCase.verifyEqual(parameters.Count, uint16(1));
            testCase.verifyTrue(parameters.Writable);
            sources = helicdaq.Protocol.decodeSources( ...
                uint8([uint8('adc0'), 0, uint8('V'), 0, ...
                uint8('laser'), 0, uint8('mm'), 0]));
            testCase.verifyEqual(sources.Name, ["adc0"; "laser"]);
            testCase.verifyEqual(sources.Unit, ["V"; "mm"]);
            testCase.verifyError(@() helicdaq.Protocol.decodeSources( ...
                uint8([uint8('adc0'), 0, uint8('V')])), ...
                'helicdaq:ProtocolError');
        end

        function parameterTypesRoundTrip(testCase)
            cases = {
                'B', uint8(200);
                'b', int8(-100);
                'H', uint16(50000);
                'h', int16(-20000);
                'I', uint32(4000000000);
                'i', int32(-2000000000);
                'f', single(1.25)
                };
            for row = 1:size(cases, 1)
                code = cases{row, 1};
                value = cases{row, 2};
                encoded = helicdaq.Protocol.packParameter(code, 1, value);
                testCase.verifyEqual( ...
                    helicdaq.Protocol.unpackParameter(code, 1, encoded), value);
            end
            encoded = helicdaq.Protocol.packParameter('c', 8, "cbc");
            testCase.verifyEqual( ...
                helicdaq.Protocol.unpackParameter('c', 8, encoded), "cbc");
        end

        function beaconKnownAnswer(testCase)
            response = struct('Version', uint8(2), 'ControlPort', uint16(2350), ...
                'Mac', TestProtocol.hexBytes('02 48 4C 00 00 01'), ...
                'Experiment', "cbc-rig", 'Firmware', "helic-daq sim");
            encoded = helicdaq.Protocol.encodeBeaconResponse(response);
            testCase.verifyEqual(encoded, TestProtocol.hexBytes( ...
                ['48 4C 02 02 2E 09 02 48 4C 00 00 01 ' ...
                '63 62 63 2D 72 69 67 00 00 00 00 00 00 00 00 00 ' ...
                '68 65 6C 69 63 2D 64 61 71 20 73 69 6D 00 00 00']));
            decoded = helicdaq.Protocol.decodeBeaconResponse(encoded);
            testCase.verifyEqual(decoded.Experiment, "cbc-rig");
            testCase.verifyEqual(decoded.Firmware, "helic-daq sim");
            testCase.verifyEqual(decoded.Mac, response.Mac);
        end

        function streamHeaderRoundTrip(testCase)
            header = struct('NSources', uint8(12), 'Sequence', uint32(123456), ...
                'FirstIndex', uint32(42), 'Dropped', uint32(3), ...
                'Decimation', uint16(2), 'NRecords', uint16(28));
            decoded = helicdaq.Protocol.decodeStreamHeader( ...
                helicdaq.Protocol.encodeStreamHeader(header));
            testCase.verifyEqual(decoded, header);
        end
    end

    methods (Static, Access = private)
        function bytes = hexBytes(text)
            compact = regexprep(text, '\s', '');
            bytes = uint8(sscanf(compact, '%2x').');
        end
    end
end
