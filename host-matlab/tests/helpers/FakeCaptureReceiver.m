% Table-producing receiver used by MATLAB device orchestration tests.
classdef FakeCaptureReceiver < handle
    properties
        Port = 2351
    end

    methods
        function data = capture(~, nRecords, names)
            indices = uint64(100 + 2 * (0:nRecords - 1).');
            values = zeros(nRecords, numel(names), 'single');
            for source = 1:numel(names)
                values(:, source) = single(source * (0:nRecords - 1).');
            end
            data = array2table(values, 'VariableNames', cellstr(names));
            data = addvars(data, indices, 'Before', 1, ...
                'NewVariableNames', 'index');
            data.Properties.UserData = struct('Dropped', uint32(0), ...
                'LostPackets', 0);
        end
    end
end
