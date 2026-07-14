% TCP control connection and high-level host operations for HELIC-DAQ.
classdef Device < handle
    %DEVICE Discover and control one HELIC-DAQ protocol-v2 device.

    properties (SetAccess = private)
        Host
        Port
        Parameters
        Sources
    end

    properties (Access = private)
        Client
        Sequence = uint8(0)
        Timeout
    end

    methods
        function obj = Device(host, varargin)
            %DEVICE Connect, validate the protocol version, and discover registries.
            parser = inputParser;
            parser.addRequired('host', @(value) ischar(value) || isstring(value));
            parser.addParameter('Port', helicdaq.Protocol.CONTROL_PORT, ...
                @(value) isscalar(value) && value >= 1 && value <= 65535);
            parser.addParameter('Timeout', 5, ...
                @(value) isscalar(value) && isfinite(value) && value > 0);
            parser.addParameter('Transport', [], @(value) true);
            parser.parse(host, varargin{:});

            obj.Host = string(host);
            obj.Port = double(parser.Results.Port);
            obj.Timeout = double(parser.Results.Timeout);
            if isempty(parser.Results.Transport)
                obj.Client = tcpclient(char(obj.Host), obj.Port, ...
                    'Timeout', obj.Timeout);
            else
                % The transport hook keeps protocol tests independent of hardware.
                obj.Client = parser.Results.Transport;
            end

            try
                initialStatus = obj.status();
                if initialStatus.ProtocolVersion ~= helicdaq.Protocol.VERSION
                    error('helicdaq:ProtocolVersion', ...
                        'Protocol version mismatch: device %d, host %d.', ...
                        initialStatus.ProtocolVersion, helicdaq.Protocol.VERSION);
                end
                obj.discover();
                if height(obj.Parameters) ~= initialStatus.ParameterCount || ...
                        height(obj.Sources) ~= initialStatus.SourceCount
                    error('helicdaq:ProtocolError', ...
                        'Discovery table lengths do not match Status.');
                end
            catch exception
                obj.close();
                rethrow(exception);
            end
        end

        function delete(obj)
            %DELETE Release the TCP connection.
            obj.close();
        end

        function close(obj)
            %CLOSE Release the TCP connection without deleting the Device object.
            obj.Client = [];
        end

        function definition = parameter(obj, name)
            %PARAMETER Return one discovered parameter-definition table row.
            key = string(name);
            row = find(obj.Parameters.Name == key, 1);
            if isempty(row)
                error('helicdaq:UnknownParameter', ...
                    'No parameter named ''%s'' was discovered.', key);
            end
            definition = obj.Parameters(row, :);
        end

        function value = getParameter(obj, name)
            %GETPARAMETER Read one parameter by its discovered name.
            definition = obj.parameter(name);
            payload = helicdaq.Protocol.packLE(definition.Index, 'uint16');
            response = obj.request(helicdaq.Protocol.GET_PAR, payload);
            if numel(response) ~= definition.Size
                error('helicdaq:ProtocolError', ...
                    'GetPar response has an invalid length.');
            end
            value = helicdaq.Protocol.unpackParameter(definition.TypeCode, ...
                definition.Count, response);
        end

        function result = getParameters(obj, names)
            %GETPARAMETERS Read several parameters in one round trip.
            names = reshape(string(names), [], 1);
            if isempty(names)
                error('helicdaq:UnknownParameter', ...
                    'At least one parameter name is required.');
            end
            if numel(unique(names)) ~= numel(names)
                error('helicdaq:UnknownParameter', ...
                    'Parameter names must be unique.');
            end
            definitions = obj.parameterRows(names);
            totalSize = sum(definitions.Size);
            if totalSize > helicdaq.Protocol.MAX_PAYLOAD
                error('helicdaq:PayloadSize', ...
                    'Requested values need %d bytes; responses are limited to %d.', ...
                    totalSize, helicdaq.Protocol.MAX_PAYLOAD);
            end
            payload = uint8([]);
            for row = 1:height(definitions)
                payload = [payload, helicdaq.Protocol.packLE( ...
                    definitions.Index(row), 'uint16')]; %#ok<AGROW>
            end
            response = obj.request(helicdaq.Protocol.GET_PAR, payload);
            if numel(response) ~= totalSize
                error('helicdaq:ProtocolError', ...
                    'GetPar response has an invalid length.');
            end
            values = cell(height(definitions), 1);
            offset = 1;
            for row = 1:height(definitions)
                final = offset + definitions.Size(row) - 1;
                values{row} = helicdaq.Protocol.unpackParameter( ...
                    definitions.TypeCode(row), definitions.Count(row), ...
                    response(offset:final));
                offset = final + 1;
            end
            result = table(names, values, 'VariableNames', {'Name', 'Value'});
        end

        function setParameter(obj, name, value)
            %SETPARAMETER Write one parameter by its discovered name.
            definition = obj.parameter(name);
            if ~definition.Writable
                error('helicdaq:ReadOnly', ...
                    'Parameter ''%s'' is read-only.', definition.Name);
            end
            raw = helicdaq.Protocol.packParameter(definition.TypeCode, ...
                definition.Count, value);
            payload = [helicdaq.Protocol.packLE(definition.Index, 'uint16'), raw];
            obj.request(helicdaq.Protocol.SET_PAR, payload);
        end

        function information = status(obj)
            %STATUS Return protocol, registry, sample-rate, and uptime status.
            payload = obj.request(helicdaq.Protocol.STATUS, uint8([]));
            if numel(payload) ~= 12
                error('helicdaq:ProtocolError', 'Status payload has invalid length.');
            end
            offset = 1;
            protocolVersion = payload(offset);
            [parameterCount, offset] = helicdaq.Protocol.unpackLE( ...
                payload, 'uint16', 1, offset + 1);
            sourceCount = payload(offset);
            [sampleRate, offset] = helicdaq.Protocol.unpackLE( ...
                payload, 'single', 1, offset + 1);
            [uptimeMs, ~] = helicdaq.Protocol.unpackLE( ...
                payload, 'uint32', 1, offset);
            information = struct('ProtocolVersion', protocolVersion, ...
                'ParameterCount', double(parameterCount), ...
                'SourceCount', double(sourceCount), 'SampleRate', sampleRate, ...
                'Uptime', seconds(double(uptimeMs) / 1000));
        end

        function resolved = configureStream(obj, sources, varargin)
            %CONFIGURESTREAM Select sources, decimation, and record count.
            parser = inputParser;
            parser.addParameter('Decimation', 1, ...
                @(value) isscalar(value) && isfinite(value) && ...
                fix(value) == value && value >= 1 && value <= 65535);
            parser.addParameter('Count', 0, ...
                @(value) isscalar(value) && isfinite(value) && fix(value) == value && ...
                value >= 0 && value <= double(intmax('uint32')));
            parser.parse(varargin{:});

            sourceNames = reshape(string(sources), [], 1);
            if isempty(sourceNames) || numel(sourceNames) > 255
                error('helicdaq:UnknownSource', ...
                    'Between one and 255 source names are required.');
            end
            if numel(unique(sourceNames)) ~= numel(sourceNames)
                error('helicdaq:UnknownSource', 'Source names must be unique.');
            end
            rows = zeros(numel(sourceNames), 1);
            for index = 1:numel(sourceNames)
                row = find(obj.Sources.Name == sourceNames(index), 1);
                if isempty(row)
                    choices = join(obj.Sources.Name + " [" + obj.Sources.Unit + "]", ', ');
                    error('helicdaq:UnknownSource', ...
                        'Unknown source ''%s''; discovered sources: %s.', ...
                        sourceNames(index), choices);
                end
                rows(index) = row;
            end
            resolved = obj.Sources(rows, :);
            payload = [helicdaq.Protocol.packLE( ...
                uint16(parser.Results.Decimation), 'uint16'), ...
                helicdaq.Protocol.packLE(uint32(parser.Results.Count), 'uint32'), ...
                uint8(height(resolved)), reshape(resolved.Index, 1, [])];
            obj.request(helicdaq.Protocol.STREAM_SETUP, payload);
        end

        function startStream(obj, port)
            %STARTSTREAM Start the configured stream to the given UDP port.
            validateattributes(port, {'numeric'}, ...
                {'scalar', 'integer', 'positive', '<=', 65535});
            obj.request(helicdaq.Protocol.STREAM_START, ...
                helicdaq.Protocol.packLE(uint16(port), 'uint16'));
        end

        function stopStream(obj)
            %STOPSTREAM Stop an active stream.
            obj.request(helicdaq.Protocol.STREAM_STOP, uint8([]));
        end

        function data = capture(obj, sources, varargin)
            %CAPTURE Configure and collect a finite stream as a MATLAB table.
            parser = inputParser;
            parser.addParameter('Samples', [], ...
                @(value) isempty(value) || (isscalar(value) && value > 0));
            parser.addParameter('Seconds', [], ...
                @(value) isempty(value) || (isscalar(value) && value > 0));
            parser.addParameter('Decimation', 1, ...
                @(value) isscalar(value) && isfinite(value) && ...
                fix(value) == value && value >= 1 && value <= 65535);
            parser.addParameter('Port', helicdaq.Protocol.STREAM_PORT, ...
                @(value) isscalar(value) && isfinite(value) && ...
                fix(value) == value && value >= 0 && value <= 65535);
            parser.addParameter('Timeout', 2, ...
                @(value) isscalar(value) && isfinite(value) && value > 0);
            parser.addParameter('Receiver', [], @(value) true);
            parser.parse(varargin{:});
            options = parser.Results;
            if isempty(options.Samples) == isempty(options.Seconds)
                error('helicdaq:CaptureLength', ...
                    'Specify exactly one of Samples or Seconds.');
            end
            if isempty(options.Samples)
                information = obj.status();
                sampleRate = double(information.SampleRate);
                nRecords = max(1, floor(options.Seconds * sampleRate / options.Decimation));
            else
                nRecords = options.Samples;
            end
            validateattributes(nRecords, {'numeric'}, ...
                {'scalar', 'integer', 'positive', '<=', double(intmax('uint32'))});

            resolved = obj.configureStream(sources, 'Decimation', options.Decimation, ...
                'Count', nRecords);
            if isempty(options.Receiver)
                receiver = helicdaq.StreamReceiver('Timeout', options.Timeout, ...
                    'Port', options.Port);
                receiverCleanup = onCleanup(@() delete(receiver));
            else
                % An injected receiver separates capture orchestration from UDP tests.
                receiver = options.Receiver;
            end
            obj.startStream(receiver.Port);
            streamCleanup = onCleanup(@() obj.stopStreamSafely());
            data = receiver.capture(nRecords, resolved.Name);
            data.Properties.VariableUnits = cellstr([""; resolved.Unit].');
        end

        function uploadTable(obj, values, varargin)
            %UPLOADTABLE Stage and atomically activate an arbitrary waveform.
            parser = inputParser;
            parser.addParameter('Duration', [], ...
                @(value) isempty(value) || ...
                (isscalar(value) && isfinite(value) && value > 0));
            parser.addParameter('Frequency', [], ...
                @(value) isempty(value) || ...
                (isscalar(value) && isfinite(value) && value > 0));
            parser.addParameter('Gain', 1, ...
                @(value) isscalar(value) && isfinite(value));
            parser.addParameter('Mode', "loop", ...
                @(value) ischar(value) || isstring(value));
            parser.addParameter('Multiplier', 1, ...
                @(value) isscalar(value) && isfinite(value) && ...
                value >= 1 && fix(value) == value);
            parser.addParameter('Phase', 0, ...
                @(value) isscalar(value) && value >= 0 && value < 1);
            parser.parse(varargin{:});
            options = parser.Results;

            values = reshape(single(values), 1, []);
            definition = obj.parameter('table');
            if numel(values) < 2 || numel(values) > definition.Count
                error('helicdaq:TableLength', ...
                    'Table length must be between 2 and %d.', definition.Count);
            end
            if any(~isfinite(values))
                error('helicdaq:TableValue', 'Table values must be finite.');
            end
            if ~isempty(options.Duration) && ~isempty(options.Frequency)
                error('helicdaq:TableTiming', ...
                    'Specify at most one of Duration or Frequency.');
            end
            frequency = options.Frequency;
            if ~isempty(options.Duration)
                frequency = 1 / options.Duration;
            end
            switch string(options.Mode)
                case "off"
                    mode = 0;
                case "loop"
                    mode = 1;
                case "one-shot"
                    mode = 2;
                case "locked"
                    mode = 3;
                case "locked-one-shot"
                    mode = 4;
                otherwise
                    error('helicdaq:TableMode', ...
                        'Unknown table mode ''%s''.', string(options.Mode));
            end

            bytes = helicdaq.Protocol.packLE(values, 'single');
            chunkSize = floor((helicdaq.Protocol.MAX_PAYLOAD - 6) / 4) * 4;
            for first = 1:chunkSize:numel(bytes)
                final = min(first + chunkSize - 1, numel(bytes));
                payload = helicdaq.Protocol.encodeSetBlock(definition.Index, ...
                    floor((first - 1) / 4), bytes(first:final));
                obj.requestWithBusyRetry(helicdaq.Protocol.SET_BLOCK, payload);
            end
            obj.requestWithBusyRetry(helicdaq.Protocol.COMMIT, ...
                helicdaq.Protocol.encodeCommit(definition.Index, numel(values)));
            if ~isempty(frequency)
                obj.setParameter('table_freq', frequency);
            end
            obj.setParameter('table_gain', options.Gain);
            obj.setParameter('table_mult', options.Multiplier);
            obj.setParameter('table_phase', options.Phase);
            obj.setParameter('table_mode', mode);
            if mode == 2 || mode == 4
                obj.setParameter('table_trigger', 1);
            end
        end
    end

    methods (Access = private)
        function discover(obj)
            parameters = helicdaq.Protocol.decodeParams( ...
                obj.request(helicdaq.Protocol.GET_PARAMS, uint8([])));
            sizes = zeros(height(parameters), 1);
            for row = 1:height(parameters)
                sizes(row) = obj.parameterSize(parameters.TypeCode(row), ...
                    parameters.Count(row));
            end
            parameters.Size = sizes;
            obj.Parameters = parameters;
            obj.Sources = helicdaq.Protocol.decodeSources( ...
                obj.request(helicdaq.Protocol.GET_SOURCES, uint8([])));
        end

        function definitions = parameterRows(obj, names)
            rows = zeros(numel(names), 1);
            for index = 1:numel(names)
                row = find(obj.Parameters.Name == names(index), 1);
                if isempty(row)
                    error('helicdaq:UnknownParameter', ...
                        'No parameter named ''%s'' was discovered.', names(index));
                end
                rows(index) = row;
            end
            definitions = obj.Parameters(rows, :);
        end

        function payload = request(obj, messageType, payload)
            if isempty(obj.Client)
                error('helicdaq:ConnectionClosed', 'Device connection is closed.');
            end
            obj.Sequence = uint8(mod(double(obj.Sequence) + 1, 256));
            frame = helicdaq.Protocol.encodeFrame(messageType, obj.Sequence, payload);
            write(obj.Client, frame, 'uint8');
            header = obj.readExact(helicdaq.Protocol.HEADER_LENGTH);
            [payloadLength, ~] = helicdaq.Protocol.unpackLE(header, 'uint16', 1, 5);
            rest = obj.readExact(double(payloadLength) + helicdaq.Protocol.TRAILER_LENGTH);
            response = helicdaq.Protocol.decodeFrame([header, rest]);
            if response.Sequence ~= obj.Sequence
                error('helicdaq:DeviceError', ...
                    'Response sequence %d does not match request %d.', ...
                    response.Sequence, obj.Sequence);
            end
            if response.MessageType == helicdaq.Protocol.ERROR
                if isempty(response.Payload)
                    code = uint8(0);
                else
                    code = response.Payload(1);
                end
                if code == helicdaq.Protocol.ERROR_BUSY
                    identifier = 'helicdaq:DeviceBusy';
                else
                    identifier = 'helicdaq:DeviceError';
                end
                error(identifier, 'Device error: %s.', helicdaq.Protocol.errorName(code));
            end
            if response.MessageType ~= uint8(messageType)
                error('helicdaq:DeviceError', ...
                    'Response type %d does not match request type %d.', ...
                    response.MessageType, messageType);
            end
            payload = response.Payload;
        end

        function bytes = readExact(obj, count)
            try
                bytes = reshape(uint8(read(obj.Client, count, 'uint8')), 1, []);
            catch exception
                error('helicdaq:TransportError', ...
                    'Failed to read %d bytes: %s', count, exception.message);
            end
            if numel(bytes) ~= count
                error('helicdaq:TransportError', ...
                    'Connection closed after %d of %d bytes.', numel(bytes), count);
            end
        end

        function requestWithBusyRetry(obj, messageType, payload)
            started = tic;
            while true
                try
                    obj.request(messageType, payload);
                    return
                catch exception
                    if ~strcmp(exception.identifier, 'helicdaq:DeviceBusy') || toc(started) >= 1
                        rethrow(exception);
                    end
                    pause(0.005);
                end
            end
        end

        function stopStreamSafely(obj)
            try
                obj.stopStream();
            catch
                % Preserve the original capture error during cleanup.
            end
        end
    end

    methods (Static, Access = private)
        function sizeBytes = parameterSize(typeCode, count)
            if char(typeCode) == 'c'
                elementSize = 1;
            else
                switch char(typeCode)
                    case {'B', 'b'}
                        elementSize = 1;
                    case {'H', 'h'}
                        elementSize = 2;
                    case {'I', 'i', 'f'}
                        elementSize = 4;
                    otherwise
                        error('helicdaq:ProtocolError', ...
                            'Invalid parameter type code ''%s''.', char(typeCode));
                end
            end
            sizeBytes = double(count) * elementSize;
        end
    end
end
