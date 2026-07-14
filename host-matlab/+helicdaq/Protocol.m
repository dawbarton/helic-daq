% Wire-level codecs for HELIC-DAQ protocol v2.
classdef Protocol
    %PROTOCOL Constants and stateless binary codecs for HELIC-DAQ.

    properties (Constant)
        MAGIC = uint16(hex2dec('4C48'))
        VERSION = uint8(2)
        CONTROL_PORT = 2350
        STREAM_PORT = 2351
        DISCOVERY_PORT = 2352
        HEADER_LENGTH = 6
        TRAILER_LENGTH = 2
        MAX_PAYLOAD = 1024
        STREAM_HEADER_LENGTH = 20
        ERROR_BUSY = uint8(7)

        GET_PARAMS = uint8(1)
        GET_SOURCES = uint8(2)
        GET_PAR = uint8(3)
        SET_PAR = uint8(4)
        SET_BLOCK = uint8(5)
        COMMIT = uint8(6)
        STREAM_SETUP = uint8(7)
        STREAM_START = uint8(8)
        STREAM_STOP = uint8(9)
        STATUS = uint8(10)
        ERROR = uint8(255)

        BEACON_REQUEST = uint8([hex2dec('48'), hex2dec('4C'), 1])
    end

    methods (Static)
        function crc = crc16(data)
            %CRC16 Compute CRC-16/CCITT-FALSE.
            data = reshape(uint8(data), 1, []);
            crc = uint16(hex2dec('FFFF'));
            polynomial = uint16(hex2dec('1021'));
            for byte = data
                crc = bitxor(crc, bitshift(uint16(byte), 8));
                for bit = 1:8
                    if bitand(crc, uint16(hex2dec('8000'))) ~= 0
                        crc = bitxor(bitshift(crc, 1), polynomial);
                    else
                        crc = bitshift(crc, 1);
                    end
                end
            end
        end

        function frame = encodeFrame(messageType, sequence, payload)
            %ENCODEFRAME Encode one control-channel frame.
            if nargin < 3
                payload = uint8([]);
            end
            payload = reshape(uint8(payload), 1, []);
            if numel(payload) > helicdaq.Protocol.MAX_PAYLOAD
                error('helicdaq:ProtocolError', ...
                    'Payload is too long (%d > %d).', numel(payload), ...
                    helicdaq.Protocol.MAX_PAYLOAD);
            end
            body = [uint8(messageType), uint8(mod(double(sequence), 256)), ...
                helicdaq.Protocol.packLE(uint16(numel(payload)), 'uint16'), payload];
            frame = [helicdaq.Protocol.packLE(helicdaq.Protocol.MAGIC, 'uint16'), ...
                body, helicdaq.Protocol.packLE(helicdaq.Protocol.crc16(body), 'uint16')];
        end

        function decoded = decodeFrame(frame)
            %DECODEFRAME Validate and decode one complete control frame.
            frame = reshape(uint8(frame), 1, []);
            minimum = helicdaq.Protocol.HEADER_LENGTH + helicdaq.Protocol.TRAILER_LENGTH;
            if numel(frame) < minimum
                error('helicdaq:ProtocolError', 'Frame is truncated.');
            end
            [magic, offset] = helicdaq.Protocol.unpackLE(frame, 'uint16', 1, 1);
            if magic ~= helicdaq.Protocol.MAGIC
                error('helicdaq:ProtocolError', 'Frame has bad magic.');
            end
            messageType = frame(offset);
            sequence = frame(offset + 1);
            [payloadLength, ~] = helicdaq.Protocol.unpackLE(frame, 'uint16', 1, offset + 2);
            expected = helicdaq.Protocol.HEADER_LENGTH + double(payloadLength) + ...
                helicdaq.Protocol.TRAILER_LENGTH;
            if numel(frame) ~= expected
                error('helicdaq:ProtocolError', 'Frame length does not match its header.');
            end
            payloadStart = helicdaq.Protocol.HEADER_LENGTH + 1;
            payloadEnd = payloadStart + double(payloadLength) - 1;
            if payloadLength == 0
                payload = uint8([]);
            else
                payload = frame(payloadStart:payloadEnd);
            end
            [storedCrc, ~] = helicdaq.Protocol.unpackLE(frame, 'uint16', 1, payloadEnd + 1);
            crcEnd = helicdaq.Protocol.HEADER_LENGTH + double(payloadLength);
            if helicdaq.Protocol.crc16(frame(3:crcEnd)) ~= storedCrc
                error('helicdaq:ProtocolError', 'Frame CRC does not match.');
            end
            decoded = struct('MessageType', messageType, 'Sequence', sequence, ...
                'Payload', payload);
        end

        function definitions = decodeParams(payload)
            %DECODEPARAMS Decode a GetParams response into a table.
            payload = reshape(uint8(payload), 1, []);
            names = strings(0, 1);
            typeCodes = strings(0, 1);
            counts = uint16(zeros(0, 1));
            writable = false(0, 1);
            offset = 1;
            while offset <= numel(payload)
                [name, offset] = helicdaq.Protocol.decodeNulString(payload, offset);
                if offset + 3 > numel(payload)
                    error('helicdaq:ProtocolError', ...
                        'Parameter definition is truncated.');
                end
                typeCode = char(payload(offset));
                if ~contains('BbHhIifc', typeCode)
                    error('helicdaq:ProtocolError', ...
                        'Parameter has invalid type code ''%s''.', typeCode);
                end
                [count, ~] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, offset + 1);
                writableByte = payload(offset + 3);
                if writableByte > 1
                    error('helicdaq:ProtocolError', 'Writable flag is invalid.');
                end
                names(end + 1, 1) = name; %#ok<AGROW>
                typeCodes(end + 1, 1) = string(typeCode); %#ok<AGROW>
                counts(end + 1, 1) = count; %#ok<AGROW>
                writable(end + 1, 1) = logical(writableByte); %#ok<AGROW>
                offset = offset + 4;
            end
            indices = uint16((0:numel(names) - 1).');
            definitions = table(indices, names, typeCodes, counts, writable, ...
                'VariableNames', {'Index', 'Name', 'TypeCode', 'Count', 'Writable'});
        end

        function sources = decodeSources(payload)
            %DECODESOURCES Decode a GetSources response into a table.
            payload = reshape(uint8(payload), 1, []);
            names = strings(0, 1);
            units = strings(0, 1);
            offset = 1;
            while offset <= numel(payload)
                [name, offset] = helicdaq.Protocol.decodeNulString(payload, offset);
                [unit, offset] = helicdaq.Protocol.decodeNulString(payload, offset);
                names(end + 1, 1) = name; %#ok<AGROW>
                units(end + 1, 1) = unit; %#ok<AGROW>
            end
            indices = uint8((0:numel(names) - 1).');
            sources = table(indices, names, units, ...
                'VariableNames', {'Index', 'Name', 'Unit'});
        end

        function payload = encodeSetBlock(index, offset, data)
            %ENCODESETBLOCK Encode a staged block-write payload.
            payload = [helicdaq.Protocol.packLE(uint16(index), 'uint16'), ...
                helicdaq.Protocol.packLE(uint32(offset), 'uint32'), ...
                reshape(uint8(data), 1, [])];
        end

        function payload = encodeCommit(index, length)
            %ENCODECOMMIT Encode a staged-table commit payload.
            payload = [helicdaq.Protocol.packLE(uint16(index), 'uint16'), ...
                helicdaq.Protocol.packLE(uint32(length), 'uint32')];
        end

        function bytes = packParameter(typeCode, count, value)
            %PACKPARAMETER Encode a parameter value using its discovered type.
            typeCode = char(typeCode);
            count = double(count);
            if typeCode == 'c'
                bytes = helicdaq.Protocol.asciiBytes(value);
                bytes = bytes(1:min(numel(bytes), count));
                bytes(end + 1:count) = uint8(0);
                return
            end
            typeName = helicdaq.Protocol.typeName(typeCode);
            if numel(value) ~= count
                error('helicdaq:ParameterSize', ...
                    'Parameter expects %d values, received %d.', count, numel(value));
            end
            converted = cast(reshape(value, 1, []), typeName);
            if isinteger(converted) && any(double(converted) ~= double(value(:).'))
                error('helicdaq:ParameterValue', ...
                    'Parameter value is not exactly representable as %s.', typeName);
            end
            bytes = helicdaq.Protocol.packLE(converted, typeName);
        end

        function value = unpackParameter(typeCode, count, bytes)
            %UNPACKPARAMETER Decode a parameter value using its discovered type.
            typeCode = char(typeCode);
            count = double(count);
            bytes = reshape(uint8(bytes), 1, []);
            if typeCode == 'c'
                ending = find(bytes == 0, 1) - 1;
                if isempty(ending)
                    ending = numel(bytes);
                end
                value = string(char(bytes(1:ending)));
                return
            end
            [value, ~] = helicdaq.Protocol.unpackLE(bytes, ...
                helicdaq.Protocol.typeName(typeCode), count, 1);
            if count == 1
                value = value(1);
            end
        end

        function response = decodeBeaconResponse(payload)
            %DECODEBEACONRESPONSE Decode one fixed-size discovery response.
            payload = reshape(uint8(payload), 1, []);
            if numel(payload) ~= 44
                error('helicdaq:ProtocolError', 'Beacon response has bad length.');
            end
            [magic, offset] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, 1);
            kind = payload(offset);
            if magic ~= helicdaq.Protocol.MAGIC || kind ~= 2
                error('helicdaq:ProtocolError', 'Beacon response has bad magic or type.');
            end
            version = payload(offset + 1);
            [controlPort, offset] = helicdaq.Protocol.unpackLE(payload, ...
                'uint16', 1, offset + 2);
            mac = payload(offset:offset + 5);
            experiment = helicdaq.Protocol.decodeFixedAscii(payload(offset + 6:offset + 21));
            firmware = helicdaq.Protocol.decodeFixedAscii(payload(offset + 22:offset + 37));
            response = struct('Version', version, 'ControlPort', controlPort, ...
                'Mac', mac, 'Experiment', experiment, 'Firmware', firmware);
        end

        function payload = encodeBeaconResponse(response)
            %ENCODEBEACONRESPONSE Encode a response for tests and simulators.
            payload = [helicdaq.Protocol.packLE(helicdaq.Protocol.MAGIC, 'uint16'), ...
                uint8(2), uint8(response.Version), ...
                helicdaq.Protocol.packLE(uint16(response.ControlPort), 'uint16'), ...
                reshape(uint8(response.Mac), 1, []), ...
                helicdaq.Protocol.fixedAscii(response.Experiment, 16), ...
                helicdaq.Protocol.fixedAscii(response.Firmware, 16)];
        end

        function header = decodeStreamHeader(payload)
            %DECODESTREAMHEADER Decode and validate a stream packet header.
            payload = reshape(uint8(payload), 1, []);
            if numel(payload) < helicdaq.Protocol.STREAM_HEADER_LENGTH
                error('helicdaq:ProtocolError', 'Stream packet is too short.');
            end
            [magic, offset] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, 1);
            version = payload(offset);
            if magic ~= helicdaq.Protocol.MAGIC || version ~= helicdaq.Protocol.VERSION
                error('helicdaq:ProtocolError', ...
                    'Stream packet has bad magic or protocol version.');
            end
            nSources = payload(offset + 1);
            [sequence, offset] = helicdaq.Protocol.unpackLE(payload, 'uint32', 1, offset + 2);
            [firstIndex, offset] = helicdaq.Protocol.unpackLE(payload, 'uint32', 1, offset);
            [dropped, offset] = helicdaq.Protocol.unpackLE(payload, 'uint32', 1, offset);
            [decimation, offset] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, offset);
            [nRecords, ~] = helicdaq.Protocol.unpackLE(payload, 'uint16', 1, offset);
            header = struct('NSources', nSources, 'Sequence', sequence, ...
                'FirstIndex', firstIndex, 'Dropped', dropped, ...
                'Decimation', decimation, 'NRecords', nRecords);
        end

        function payload = encodeStreamHeader(header)
            %ENCODESTREAMHEADER Encode a stream header for tests and simulators.
            payload = [helicdaq.Protocol.packLE(helicdaq.Protocol.MAGIC, 'uint16'), ...
                helicdaq.Protocol.VERSION, uint8(header.NSources), ...
                helicdaq.Protocol.packLE(uint32(header.Sequence), 'uint32'), ...
                helicdaq.Protocol.packLE(uint32(header.FirstIndex), 'uint32'), ...
                helicdaq.Protocol.packLE(uint32(header.Dropped), 'uint32'), ...
                helicdaq.Protocol.packLE(uint16(header.Decimation), 'uint16'), ...
                helicdaq.Protocol.packLE(uint16(header.NRecords), 'uint16')];
        end

        function bytes = packLE(value, typeName)
            %PACKLE Encode numeric values in little-endian byte order.
            value = cast(value, typeName);
            [~, ~, endian] = computer;
            if endian == 'B'
                value = swapbytes(value);
            end
            bytes = reshape(typecast(value(:), 'uint8'), 1, []);
        end

        function [value, nextOffset] = unpackLE(bytes, typeName, count, offset)
            %UNPACKLE Decode little-endian numeric values from a byte vector.
            if nargin < 3
                count = 1;
            end
            if nargin < 4
                offset = 1;
            end
            sizeBytes = helicdaq.Protocol.typeSize(typeName);
            final = offset + double(count) * sizeBytes - 1;
            if offset < 1 || final > numel(bytes)
                error('helicdaq:ProtocolError', 'Binary value is truncated.');
            end
            value = typecast(uint8(bytes(offset:final)), typeName);
            [~, ~, endian] = computer;
            if endian == 'B'
                value = swapbytes(value);
            end
            value = reshape(value, 1, []);
            nextOffset = final + 1;
        end

        function name = errorName(code)
            %ERRORNAME Return the protocol description for a device error code.
            names = ["bad frame", "unknown message type", ...
                "bad parameter index", "bad length", "parameter is read-only", ...
                "bad value", "device busy"];
            if code >= 1 && code <= numel(names)
                name = names(double(code));
            else
                name = "code " + string(code);
            end
        end
    end

    methods (Static, Access = private)
        function [value, nextOffset] = decodeNulString(bytes, offset)
            ending = find(bytes(offset:end) == 0, 1);
            if isempty(ending)
                error('helicdaq:ProtocolError', ...
                    'Discovery string is not NUL-terminated.');
            end
            ending = offset + ending - 1;
            raw = bytes(offset:ending - 1);
            if any(raw >= 128)
                error('helicdaq:ProtocolError', 'Discovery string is not ASCII.');
            end
            value = string(char(raw));
            nextOffset = ending + 1;
        end

        function value = decodeFixedAscii(bytes)
            ending = find(bytes == 0, 1) - 1;
            if isempty(ending)
                ending = numel(bytes);
            end
            raw = bytes(1:ending);
            if any(raw >= 128)
                error('helicdaq:ProtocolError', 'Beacon identity is not ASCII.');
            end
            value = string(char(raw));
        end

        function bytes = fixedAscii(value, count)
            bytes = helicdaq.Protocol.asciiBytes(value);
            bytes = bytes(1:min(numel(bytes), count));
            bytes(end + 1:count) = uint8(0);
        end

        function bytes = asciiBytes(value)
            if ~isscalar(string(value))
                error('helicdaq:ProtocolError', 'ASCII value must be scalar.');
            end
            characters = char(string(value));
            if any(double(characters) >= 128)
                error('helicdaq:ProtocolError', 'Value is not ASCII.');
            end
            bytes = uint8(characters);
        end

        function name = typeName(typeCode)
            switch char(typeCode)
                case 'B'
                    name = 'uint8';
                case 'b'
                    name = 'int8';
                case 'H'
                    name = 'uint16';
                case 'h'
                    name = 'int16';
                case 'I'
                    name = 'uint32';
                case 'i'
                    name = 'int32';
                case 'f'
                    name = 'single';
                case 'c'
                    name = 'uint8';
                otherwise
                    error('helicdaq:ProtocolError', ...
                        'Invalid parameter type code ''%s''.', char(typeCode));
            end
        end

        function count = typeSize(typeName)
            switch char(typeName)
                case {'uint8', 'int8'}
                    count = 1;
                case {'uint16', 'int16'}
                    count = 2;
                case {'uint32', 'int32', 'single'}
                    count = 4;
                otherwise
                    error('helicdaq:ProtocolError', ...
                        'Unsupported wire type ''%s''.', char(typeName));
            end
        end
    end
end
