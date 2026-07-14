% In-memory protocol-v2 transport used by MATLAB device integration tests.
classdef FakeTransport < handle
    properties (Dependent)
        NumBytesAvailable
    end

    properties
        Timeout = 1
    end

    properties (Access = private)
        Buffer = uint8([])
        Definitions
        Sources
        Values
        TableStaging
        StreamIds = uint8([])
        StreamCount = uint32(0)
        StreamDecimation = uint16(1)
    end

    methods
        function obj = FakeTransport()
            names = ["firmware"; "sample_freq"; "freq"; "forcing_coeffs"; ...
                "table"; "table_len"; "table_freq"; "table_gain"; ...
                "table_mode"; "table_mult"; "table_phase"; "table_trigger"];
            typeCodes = ["c"; "f"; "f"; "f"; "f"; "H"; ...
                "f"; "f"; "I"; "I"; "f"; "I"];
            counts = uint16([16; 1; 1; 5; 8; 1; 1; 1; 1; 1; 1; 1]);
            writable = logical([0; 0; 1; 1; 1; 0; 1; 1; 1; 1; 1; 1]);
            indices = uint16((0:numel(names) - 1).');
            obj.Definitions = table(indices, names, typeCodes, counts, writable, ...
                'VariableNames', {'Index', 'Name', 'TypeCode', 'Count', 'Writable'});
            initial = {
                "helic-daq test";
                single(1000);
                single(0);
                zeros(1, 5, 'single');
                zeros(1, 8, 'single');
                uint16(0);
                single(0);
                single(1);
                uint32(0);
                uint32(1);
                single(0);
                uint32(0)
                };
            obj.Values = cell(height(obj.Definitions), 1);
            for row = 1:height(obj.Definitions)
                obj.Values{row} = helicdaq.Protocol.packParameter( ...
                    obj.Definitions.TypeCode(row), obj.Definitions.Count(row), ...
                    initial{row});
            end
            obj.TableStaging = obj.Values{5};
            obj.Sources = table(uint8([0; 1]), ["adc0"; "out"], ["V"; "V"], ...
                'VariableNames', {'Index', 'Name', 'Unit'});
        end

        function count = get.NumBytesAvailable(obj)
            count = numel(obj.Buffer);
        end

        function write(obj, data, ~)
            request = helicdaq.Protocol.decodeFrame(data);
            payload = uint8([]);
            streamPort = [];
            switch request.MessageType
                case helicdaq.Protocol.STATUS
                    payload = obj.statusPayload();
                case helicdaq.Protocol.GET_PARAMS
                    payload = obj.parameterPayload();
                case helicdaq.Protocol.GET_SOURCES
                    payload = obj.sourcePayload();
                case helicdaq.Protocol.GET_PAR
                    payload = obj.getPayload(request.Payload);
                case helicdaq.Protocol.SET_PAR
                    obj.setPayload(request.Payload);
                case helicdaq.Protocol.SET_BLOCK
                    obj.setBlock(request.Payload);
                case helicdaq.Protocol.COMMIT
                    obj.commit(request.Payload);
                case helicdaq.Protocol.STREAM_SETUP
                    obj.streamSetup(request.Payload);
                case helicdaq.Protocol.STREAM_START
                    [streamPort, ~] = helicdaq.Protocol.unpackLE( ...
                        request.Payload, 'uint16', 1, 1);
                case helicdaq.Protocol.STREAM_STOP
                    % No state is needed for the finite mock stream.
                otherwise
                    error('FakeTransport:Unsupported', ...
                        'Unsupported message type %d.', request.MessageType);
            end
            response = helicdaq.Protocol.encodeFrame(request.MessageType, ...
                request.Sequence, payload);
            obj.Buffer = [obj.Buffer, response];
            if ~isempty(streamPort)
                obj.sendStream(streamPort);
            end
        end

        function data = read(obj, count, ~)
            if numel(obj.Buffer) < count
                error('FakeTransport:Truncated', ...
                    'Only %d bytes are available; %d requested.', ...
                    numel(obj.Buffer), count);
            end
            data = obj.Buffer(1:count);
            obj.Buffer(1:count) = [];
        end

        function value = parameterValue(obj, name)
            row = find(obj.Definitions.Name == string(name), 1);
            value = helicdaq.Protocol.unpackParameter( ...
                obj.Definitions.TypeCode(row), obj.Definitions.Count(row), ...
                obj.Values{row});
        end
    end

    methods (Access = private)
        function payload = statusPayload(obj)
            payload = [helicdaq.Protocol.VERSION, ...
                helicdaq.Protocol.packLE(uint16(height(obj.Definitions)), 'uint16'), ...
                uint8(height(obj.Sources)), ...
                helicdaq.Protocol.packLE(single(1000), 'single'), ...
                helicdaq.Protocol.packLE(uint32(42000), 'uint32')];
        end

        function payload = parameterPayload(obj)
            payload = uint8([]);
            for row = 1:height(obj.Definitions)
                payload = [payload, uint8(char(obj.Definitions.Name(row))), uint8(0), ...
                    uint8(char(obj.Definitions.TypeCode(row))), ...
                    helicdaq.Protocol.packLE(obj.Definitions.Count(row), 'uint16'), ...
                    uint8(obj.Definitions.Writable(row))]; %#ok<AGROW>
            end
        end

        function payload = sourcePayload(obj)
            payload = uint8([]);
            for row = 1:height(obj.Sources)
                payload = [payload, uint8(char(obj.Sources.Name(row))), uint8(0), ...
                    uint8(char(obj.Sources.Unit(row))), uint8(0)]; %#ok<AGROW>
            end
        end

        function payload = getPayload(obj, request)
            payload = uint8([]);
            offset = 1;
            while offset <= numel(request)
                [index, offset] = helicdaq.Protocol.unpackLE( ...
                    request, 'uint16', 1, offset);
                payload = [payload, obj.Values{double(index) + 1}]; %#ok<AGROW>
            end
        end

        function setPayload(obj, payload)
            [index, offset] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, 1);
            obj.Values{double(index) + 1} = payload(offset:end);
        end

        function setBlock(obj, payload)
            [index, offset] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, 1);
            if obj.Definitions.Name(double(index) + 1) ~= "table"
                error('FakeTransport:Parameter', 'SetBlock target is not table.');
            end
            [elementOffset, offset] = helicdaq.Protocol.unpackLE( ...
                payload, 'uint32', 1, offset);
            first = double(elementOffset) * 4 + 1;
            data = payload(offset:end);
            obj.TableStaging(first:first + numel(data) - 1) = data;
        end

        function commit(obj, payload)
            [index, offset] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, 1);
            if obj.Definitions.Name(double(index) + 1) ~= "table"
                error('FakeTransport:Parameter', 'Commit target is not table.');
            end
            [tableLength, ~] = helicdaq.Protocol.unpackLE(payload, 'uint32', 1, offset);
            obj.Values{5} = obj.TableStaging;
            obj.Values{6} = helicdaq.Protocol.packParameter('H', 1, uint16(tableLength));
        end

        function streamSetup(obj, payload)
            [obj.StreamDecimation, offset] = helicdaq.Protocol.unpackLE( ...
                payload, 'uint16', 1, 1);
            [obj.StreamCount, offset] = helicdaq.Protocol.unpackLE( ...
                payload, 'uint32', 1, offset);
            nSources = double(payload(offset));
            obj.StreamIds = payload(offset + 1:offset + nSources);
        end

        function sendStream(~, ~)
            % The injected capture receiver supplies stream records.
        end
    end
end
