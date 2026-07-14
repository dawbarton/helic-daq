% Run the HELIC-DAQ MATLAB package test suite with text diagnostics.
function results = runTests()
%RUNTESTS Add package paths, run all tests, and fail on any unsuccessful test.

root = fileparts(mfilename('fullpath'));
originalPath = addpath(root, fullfile(root, 'tests', 'helpers'));
cleanup = onCleanup(@() path(originalPath));
suite = testsuite(fullfile(root, 'tests'), 'IncludeSubfolders', true);
runner = testrunner('textoutput');
results = runner.run(suite);
if any([results.Failed])
    error('helicdaq:TestsFailed', 'One or more HELIC-DAQ MATLAB tests failed.');
end
end
