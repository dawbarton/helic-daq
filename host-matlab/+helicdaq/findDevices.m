% IPv4 UDP discovery for HELIC-DAQ devices.
function devices = findDevices(varargin)
%FINDDEVICES Return unique discovery responses received before a timeout.

parser = inputParser;
parser.addParameter('Timeout', 1, ...
    @(value) isscalar(value) && isfinite(value) && value > 0);
parser.addParameter('Port', helicdaq.Protocol.DISCOVERY_PORT, ...
    @(value) isscalar(value) && isfinite(value) && ...
    fix(value) == value && value >= 1 && value <= 65535);
parser.addParameter('Addresses', string.empty(1, 0), ...
    @(value) ischar(value) || isstring(value) || iscellstr(value));
parser.addParameter('Transport', [], @(value) true);
parser.parse(varargin{:});

addresses = reshape(string(parser.Results.Addresses), 1, []);
if isempty(addresses)
    addresses = ["255.255.255.255", "127.0.0.1"];
end
if isempty(parser.Results.Transport)
    socket = udpport('datagram', 'IPV4', 'Timeout', parser.Results.Timeout);
    socket.EnableBroadcast = true;
    nativeSocket = true;
    % Injected transports stay caller-owned; only release our own socket.
    cleanup = onCleanup(@() delete(socket));
else
    % The transport hook permits deterministic discovery tests.
    socket = parser.Results.Transport;
    socket.Timeout = parser.Results.Timeout;
    nativeSocket = false;
end
for address = addresses
    try
        if nativeSocket
            write(socket, helicdaq.Protocol.BEACON_REQUEST, 'uint8', ...
                char(address), parser.Results.Port);
        else
            socket.send(helicdaq.Protocol.BEACON_REQUEST, address, ...
                parser.Results.Port);
        end
    catch
        % One unavailable interface or target must not prevent other queries.
    end
end

foundAddress = strings(0, 1);
versions = uint8(zeros(0, 1));
controlPorts = uint16(zeros(0, 1));
macs = strings(0, 1);
experiments = strings(0, 1);
firmware = strings(0, 1);
started = tic;
while toc(started) < parser.Results.Timeout
    socket.Timeout = max(0.001, parser.Results.Timeout - toc(started));
    try
        if nativeSocket
            if socket.NumDatagramsAvailable < 1
                pause(min(0.005, parser.Results.Timeout / 20));
                continue
            end
            datagram = read(socket, 1, 'uint8');
            packet = datagram.Data;
            address = string(datagram.SenderAddress);
        else
            [packet, address] = socket.receive();
        end
        response = helicdaq.Protocol.decodeBeaconResponse(packet);
    catch exception
        if strcmp(exception.identifier, 'helicdaq:StreamTimeout')
            break
        end
        if strcmp(exception.identifier, 'helicdaq:ProtocolError')
            continue
        end
        rethrow(exception);
    end
    mac = lower(join(compose('%02X', response.Mac), ':'));
    keyMatches = foundAddress == address & macs == mac;
    if any(keyMatches)
        row = find(keyMatches, 1);
        versions(row) = response.Version;
        controlPorts(row) = response.ControlPort;
        experiments(row) = response.Experiment;
        firmware(row) = response.Firmware;
    else
        foundAddress(end + 1, 1) = address; %#ok<AGROW>
        versions(end + 1, 1) = response.Version; %#ok<AGROW>
        controlPorts(end + 1, 1) = response.ControlPort; %#ok<AGROW>
        macs(end + 1, 1) = mac; %#ok<AGROW>
        experiments(end + 1, 1) = response.Experiment; %#ok<AGROW>
        firmware(end + 1, 1) = response.Firmware; %#ok<AGROW>
    end
end

devices = table(foundAddress, versions, controlPorts, macs, experiments, firmware, ...
    'VariableNames', {'Address', 'Version', 'ControlPort', 'Mac', ...
    'Experiment', 'Firmware'});
devices = sortrows(devices, {'Address', 'Mac'});
end
