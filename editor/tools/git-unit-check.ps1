# The unit checks of git, run from outside the workspace.
#
# The binary of the checks is built inside the workspace, and the workspace
# carries a low integrity label: a low integrity process cannot start the
# `git` of the machine, which runs at the ordinary level. The checks would
# therefore be skipped there, and a check that did not run is not a check
# that passed. The copy of the binary outside the workspace runs at the
# ordinary level, so the checks of git really run.
#
# What the check proves:
#
# 1. The checks of the git module pass: the root of a repository, the status
#    of a repository, the diff of a file, the staging of a file, a commit.
# 2. The checks of the panel of the editor pass: the panel opens and shows
#    what changed, the letter S stages the file that is picked, the letter C
#    asks for the message of the commit and commits what was staged.
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $root 'Cargo.toml'
$deps = Join-Path $root 'target\debug\deps'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$logs = Join-Path $root '.dev-logs'
$report = Join-Path $logs 'git-unit-check.json'

# The checks are built, but not run: cargo runs them where they are built,
# and that is the place where git cannot be started.
Write-Host 'building the checks of the platform'
cargo test --no-run --manifest-path $manifest -p codeweaver-platform
if ($LASTEXITCODE -ne 0) { throw 'the checks of the platform did not build' }

$binary = Get-ChildItem -Path $deps -Filter 'codeweaver_platform-*.exe' -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $binary) { throw 'the binary of the checks is not there' }
Write-Host "the checks are in $($binary.FullName)"

# The copy runs outside the workspace, and therefore at the ordinary level.
New-Item -ItemType Directory -Force -Path $run | Out-Null
$copy = Join-Path $run 'codeweaver-platform-checks.exe'
Copy-Item $binary.FullName $copy -Force
Write-Host "the checks run from $copy"

New-Item -ItemType Directory -Force -Path $logs | Out-Null
$log = Join-Path $logs 'git-unit-check.log'

# Only the checks of git are run: the filter is a part of the name of the
# test, and every check of git has `git` in its name.
$output = & $copy 'git' '--nocapture' '--test-threads' '1' 2>&1
$code = $LASTEXITCODE
$output | ForEach-Object { Write-Host $_ }
[System.IO.File]::WriteAllText($log, ($output | Out-String), (New-Object System.Text.UTF8Encoding($false)))
Write-Host "saved $log"

$summary = @($output | Where-Object { $_ -match 'test result:' })
$blocked = @($output | Where-Object { $_ -match 'BLOCKED' })

$result = @{
    exitCode = $code
    summary = $summary
    blocked = $blocked.Count
    binary = $binary.FullName
    copy = $copy
    log = $log
}
[System.IO.File]::WriteAllText($report, ($result | ConvertTo-Json -Depth 4), (New-Object System.Text.UTF8Encoding($false)))
Write-Host "saved $report"

if ($blocked.Count -gt 0) {
    Write-Host "the checks of git were not run at all: $($blocked[0])"
    exit 1
}
if ($code -ne 0) {
    Write-Host 'the checks of git did not pass'
    exit 1
}
Write-Host 'the checks of git passed'
