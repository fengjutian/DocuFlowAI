# The check of the debugger against an adapter that is real: debugpy.
#
# The check is an ordinary test of the platform crate, but it cannot run
# inside the workspace: the workspace carries a low integrity label, and a
# low integrity process cannot start the Python of the machine, which runs at
# the ordinary level. The copy of the test binary outside the workspace runs
# at the ordinary level, so the real adapter really runs.
#
# The check needs a Python that has debugpy in it. By default the script
# looks for the one in the check folder of the local data of the machine;
# another one can be named with -Python.
#
# The script does not touch the desktop: it opens no window, takes no
# keyboard and moves no pointer.
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

param(
    [string]$Python = ''
)

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root 'Cargo.toml'
$deps = Join-Path $root 'target\debug\deps'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$logs = Join-Path $root '.dev-logs'

if (-not $Python) {
    $Python = Join-Path $run 'debugpy-venv\Scripts\python.exe'
}
if (-not (Test-Path $Python)) {
    throw "there is no python at $Python; name one with -Python"
}

# The adapter has to be in that python.
$version = & $Python -c 'import debugpy; print(debugpy.__version__)'
if ($LASTEXITCODE -ne 0) {
    throw "debugpy is not in $Python"
}
Write-Host "debugpy $version in $Python"

# The check is built, but not run: cargo runs the checks where they are
# built, and that is the place where the Python of the machine cannot be
# started.
cargo test --no-run --manifest-path $manifest -p codeweaver-platform --test real_adapter
if ($LASTEXITCODE -ne 0) { throw 'the check did not build' }

$binary = Get-ChildItem -Path $deps -Filter 'real_adapter-*.exe' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $binary) { throw 'the binary of the check is not there' }
Write-Host "the check is in $($binary.FullName)"

# The copy runs outside the workspace, and therefore at the ordinary level.
New-Item -ItemType Directory -Force -Path $run | Out-Null
$copy = Join-Path $run 'real-adapter-check.exe'
Copy-Item $binary.FullName $copy -Force
Write-Host "the check runs from $copy"

New-Item -ItemType Directory -Force -Path $logs | Out-Null
$log = Join-Path $logs 'dap-real-check.log'
$outFile = Join-Path $logs 'dap-real-check.out.txt'
$errFile = Join-Path $logs 'dap-real-check.err.txt'

# The output of the check goes into files and not into the stream of the
# script: a check that fails says why on the error stream, and Windows
# PowerShell reads that as the script itself failing.
$env:CODEWEAVER_DEBUGPY_PYTHON = $Python
& $copy '--nocapture' '--test-threads' '1' 1>$outFile 2>$errFile
$code = $LASTEXITCODE
$output = @(Get-Content $outFile -ErrorAction SilentlyContinue) +
    @(Get-Content $errFile -ErrorAction SilentlyContinue)
$output | ForEach-Object { Write-Host $_ }
[System.IO.File]::WriteAllText($log, ($output | Out-String), (New-Object System.Text.UTF8Encoding($false)))
Write-Host "saved $log"

$blocked = @($output | Where-Object { $_ -match 'BLOCKED' })

$report = @{
    exitCode = $code
    python = $Python
    debugpy = $version
    blocked = $blocked.Count
    binary = $binary.FullName
    copy = $copy
    log = $log
}
$reportFile = Join-Path $logs 'dap-real-check.json'
[System.IO.File]::WriteAllText($reportFile, ($report | ConvertTo-Json -Depth 4), (New-Object System.Text.UTF8Encoding($false)))
Write-Host "saved $reportFile"

if ($blocked.Count -gt 0) {
    Write-Host "the check was not run at all: $($blocked[0])"
    exit 1
}
if ($code -ne 0) {
    Write-Host 'the check of the real adapter did not pass'
    exit 1
}
Write-Host 'the check of the real adapter passed'
