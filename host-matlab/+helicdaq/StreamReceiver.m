% UDP stream reception and table assembly for HELIC-DAQ.
classdef StreamReceiver < handle
    %STREAMRECEIVER Receive stream packets and account for UDP sequence loss.

    properties (SetAccess = private)
        Port
        Timeout
        LostPackets = 0
    end

    properties (Access = private)
        Socket
        LastSequence = []
    end

    methods
        function obj = StreamReceiver(varargin)
            %STREAMRECEIVER Bind an IPv4 UDP datagram receiver.
            parser = inputParser;
            parser.addParameter('Port', helicdaq.Protocol.STREAM_PORT, ...
                @(value) isscalar(value) && isfinite(value) && ...
                fix(value) == value && value >= 0 && value <= 65535);
            parser.addParameter('BindAddress', "0.0.0.0", ...
                @(value) ischar(value) || isstring(value));
            parser.addParameter('Timeout', 2, ...
                @(value) isscalar(value) && isfinite(value) && value > 0);
            parser.addParameter('Transport', [], @(value) true);
            parser.parse(varargin{:});

            obj.Timeout = double(parser.Results.Timeout);
            if isempty(parser.Results.Transport)
                obj.Socket = udpport('datagram', 'IPV4', ...
                    'LocalHost', char(string(parser.Results.BindAddress)), ...
                    'LocalPort', double(parser.Results.Port), ...
                    'Timeout', obj.Timeout);
                obj.Port = obj.Socket.LocalPort;
            else
                % The transport hook keeps packet tests independent of hardware.
                obj.Socket = parser.Results.Transport;
                obj.Socket.Timeout = obj.Timeout;
                obj.Port = obj.Socket.Port;
            end
        end

        function delete(obj)
            %DELETE Release the UDP socket.
            if ~isempty(obj.Socket)
                try
                    delete(obj.Socket);
                catch
                    % The socket may already have been deleted explicitly.
                end
                obj.Socket = [];
            end
        end

        function [header, values] = receive(obj)
            %RECEIVE Return one decoded header and record-major single matrix.
            if isa(obj.Socket, 'udpport')
                started = tic;
                while obj.Socket.NumDatagramsAvailable < 1
                    if toc(started) >= obj.Timeout
                        error('helicdaq:StreamTimeout', ...
                            'No HELIC-DAQ stream packet arrived within %.3g seconds.', ...
                            obj.Timeout);
                    end
                    pause(min(0.001, obj.Timeout / 20));
                end
                datagram = read(obj.Socket, 1, 'uint8');
                packet = reshape(uint8(datagram.Data), 1, []);
            else
                packet = obj.Socket.receive();
            end
            header = helicdaq.Protocol.decodeStreamHeader(packet);
            expected = helicdaq.Protocol.STREAM_HEADER_LENGTH + ...
                4 * double(header.NSources) * double(header.NRecords);
            if numel(packet) ~= expected
                error('helicdaq:ProtocolError', ...
                    'Stream packet length %d does not match expected length %d.', ...
                    numel(packet), expected);
            end

            if ~isempty(obj.LastSequence)
                gap = mod(double(header.Sequence) - double(obj.LastSequence) - 1, 2^32);
                if gap > 0 && gap < 2^16
                    obj.LostPackets = obj.LostPackets + gap;
                end
            end
            obj.LastSequence = header.Sequence;

            payload = packet(helicdaq.Protocol.STREAM_HEADER_LENGTH + 1:end);
            count = double(header.NSources) * double(header.NRecords);
            [wireValues, ~] = helicdaq.Protocol.unpackLE(payload, 'single', count, 1);
            values = reshape(wireValues, double(header.NSources), ...
                double(header.NRecords)).';
        end

        function data = capture(obj, nRecords, names)
            %CAPTURE Collect a finite stream into a MATLAB table.
            validateattributes(nRecords, {'numeric'}, ...
                {'scalar', 'integer', 'positive', 'finite'});
            names = reshape(string(names), 1, []);
            if isempty(names) || numel(unique(names)) ~= numel(names)
                error('helicdaq:SourceNames', 'Source names must be non-empty and unique.');
            end
            if any(names == "index")
                error('helicdaq:SourceNames', 'Source name ''index'' is reserved.');
            end

            indices = zeros(nRecords, 1, 'uint64');
            values = zeros(nRecords, numel(names), 'single');
            initialLost = obj.LostPackets;
            dropped = uint32(0);
            offset = 0;
            while offset < nRecords
                [header, block] = obj.receive();
                if double(header.NSources) ~= numel(names)
                    error('helicdaq:ProtocolError', ...
                        'Packet has %d sources; capture expects %d.', ...
                        header.NSources, numel(names));
                end
                packetRecords = double(header.NRecords);
                if packetRecords < 1
                    error('helicdaq:ProtocolError', 'Stream packet has no records.');
                end
                taken = min(packetRecords, nRecords - offset);
                rows = offset + (1:taken);
                indices(rows) = uint64(header.FirstIndex) + ...
                    uint64(0:taken - 1).' * uint64(header.Decimation);
                values(rows, :) = block(1:taken, :);
                offset = offset + taken;
                dropped = header.Dropped;
            end

            data = array2table(values, 'VariableNames', cellstr(names));
            data = addvars(data, indices, 'Before', 1, 'NewVariableNames', 'index');
            data.Properties.UserData = struct('Dropped', dropped, ...
                'LostPackets', obj.LostPackets - initialLost);
        end
    end
end
