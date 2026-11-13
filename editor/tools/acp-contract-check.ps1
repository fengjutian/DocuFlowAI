# The contract tests against the real ACP endpoint.
#
# The tests spawn the endpoint as a child process, and a child process can
# only start a program with at least as much rights as itself. The workspace
# carries a low integrity label, so a test built there runs with less rights
# than the endpoint it has to start: the endpoint is refused with "access
# denied", and the tests quietly do nothing.
#
# The test is therefore run from a folder of its own, outside the workspace,
# where it has the rights a program normally has. The test says when the
# endpoint was not available, and this check treats that as a failure: a test
# that did not run is not a test that passed.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$report = Join-Path $logs 'acp-contract-check.json'

New-Item -ItemType Directory -Force -Path $logs | Out-Null
New-Item -ItemType Directory -Force -Path $run | Out-Null

$built = @()
# The build writes to the error stream, which is not an error here.
$ErrorActionPreference = 'Continue'
$built = cargo test --manifest-path (Join-Path $root 'Cargo.toml') `
    -p codeweaver-agent-acp --test acp_contract --no-run 2>&1
$ErrorActionPreference = 'Stop'
$line = ($built | Select-String 'Executable' | Select-Object -Last 1).ToString()
if ($line -notmatch '\(([^)]+\.exe)\)') {
    throw "the contract tests were not built: $built"
}
$relative = $Matches[1]
# Cargo names the test relative to the folder it was run in.
$built_exe = $relative
if (-not [System.IO.Path]::IsPathRooted($relative)) {
    $built_exe = Join-Path (Get-Location).Path $relative
    if (-not (Test-Path $built_exe)) {
        $built_exe = Join-Path $root $relative
    }
}
if (-not (Test-Path $built_exe)) {
    throw "the contract test is not where it was said to be: $built_exe"
}

$copy = Join-Path $run 'acp-contract.exe'
Copy-Item $built_exe $copy -Force

$output = & $copy --nocapture 2>&1
$output | ForEach-Object { Write-Host $_ }

$skipped = @($output | Select-String 'skipped: the ACP endpoint is not available')
$result = ($output | Select-String 'test result:') | Select-Object -Last 1
$passed = if ($result -and $result -match '(\d+) passed; (\d+) failed') {
    [int]$Matches[1]
} else {
    0
}
$failed = if ($result -and $result -match '(\d+) passed; (\d+) failed') {
    [int]$Matches[2]
} else {
    1
}

$summary = @{
    version = '0.5.0'
    endpointAvailable = ($skipped.Count -eq 0)
    passed = $passed
    failed = $failed
    skipped = $skipped.Count
}
$summary | ConvertTo-Json -Depth 4 | Set-Content -Path $report -Encoding UTF8
Write-Host "wrote $report"

if ($skipped.Count -gt 0) {
    Write-Host "the endpoint was not available: $($skipped.Count) test(s) did not run"
    exit 1
}
if ($failed -gt 0) {
    Write-Host "$failed test(s) failed"
    exit 1
}
Write-Host "every test ran against the real endpoint and passed"
