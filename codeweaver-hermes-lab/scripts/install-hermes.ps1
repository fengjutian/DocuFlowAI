<#
.SYNOPSIS
Installs the pinned Hermes release used by the Phase 0 lab and starts its API server.

.DESCRIPTION
The lab is only meaningful against the version hermes.lock.json pins, so this script installs
that exact release into an isolated profile and proves the runtime identifies itself correctly.

Why it is not just "pip install hermes-agent":
  * PyPI only publishes up to 0.19.0; the pinned 0.23.0 exists as a GitHub tag, so the source
    tarball is the only installable artifact for the pinned version.
  * The release tarball ships version = "0.0.0" in pyproject.toml. Upstream's own identity
    mechanism is install-stamp.json (stamp -> live git -> unknown), so the script writes that
    stamp with the locked commit.
  * Every runtime dependency is declared as python_version >= '3.14'. On an older interpreter pip
    installs the package and none of its dependencies, which fails later at import time.
  * The API server needs aiohttp, which is not a core dependency.
  * The Anthropic SDK is an optional extra that Hermes resolves lazily on the first agent
    turn. Left missing, every turn blocks on a runtime package download, so it is installed
    up front.

The install lives in %LOCALAPPDATA%\hermes\hermes-agent-<version> and uses its own virtual
environment, so an existing Hermes installation is never touched. Configuration goes to a separate
HERMES_HOME profile directory next to it.

.PARAMETER Version
Release version to install. Must match hermes.lock.json.

.PARAMETER InstallDir
Install root. Defaults to %LOCALAPPDATA%\hermes\hermes-agent-<version>.

.PARAMETER ProfileDir
HERMES_HOME profile. Defaults to <InstallDir>\profile.

.PARAMETER StartGateway
Start the API server in the background after installing.

.PARAMETER ApiServerPort
Port for the Hermes API server. Defaults to 8642.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts/install-hermes.ps1

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts/install-hermes.ps1 -StartGateway
#>
[CmdletBinding()]
param(
  [string]$Version = "",
  [string]$InstallDir = "",
  [string]$ProfileDir = "",
  [switch]$StartGateway,
  [int]$ApiServerPort = 8642
)

$ErrorActionPreference = 'Stop'
# The lock file lives next to this script's parent directory (the lab root), not at the repository
# root: the lab was renamed from docuflow-hermes-lab, and this path was left pointing one level
# too high, so the script could not find the lock it refuses to install without.
$labRoot = Split-Path -Parent $PSScriptRoot
$lockPath = Join-Path $labRoot 'hermes.lock.json'
if (-not (Test-Path $lockPath)) { throw "hermes.lock.json not found at $lockPath" }
$lock = Get-Content $lockPath -Raw | ConvertFrom-Json

if (-not $Version) { $Version = $lock.version }
if ($Version -ne $lock.version) {
  throw "hermes.lock.json pins $($lock.version) ($($lock.tag)); refusing to install $Version"
}
if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA "hermes\hermes-agent-$Version" }
if (-not $ProfileDir) { $ProfileDir = Join-Path $InstallDir 'profile' }

$pythonVersion = '3.14'
$venvDir = Join-Path $InstallDir 'venv'
$venvPython = Join-Path $venvDir 'Scripts\python.exe'
$work = Join-Path $env:TEMP "hermes-install-$Version"
$tarball = Join-Path $work "hermes-agent-$Version.tar.gz"

function Write-Step([string]$message) { Write-Host "==> $message" }

function Resolve-TarballUrl {
  # The commit-pinned URL is what makes the artifact identity checkable; the tag URL is the
  # same bytes but does not name the commit.
  param([string]$Commit)
  $api = "https://api.github.com/repos/NousResearch/hermes-agent/tarball/$Commit"
  try {
    $response = Invoke-WebRequest -Uri $api -Method Head -TimeoutSec 20 -UseBasicParsing
    if ($response.StatusCode -lt 400) { return $api }
  } catch { }
  return "https://github.com/NousResearch/hermes-agent/archive/refs/tags/v$Version.tar.gz"
}

function Get-RemoteSha {
  param([string]$Tag)
  try {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/NousResearch/hermes-agent/releases/tags/$Tag" `
      -Headers @{ 'User-Agent' = 'codeweaver-hermes-lab' } -TimeoutSec 25
    return $release.target_commitish
  } catch { return $null }
}

function Invoke-Download {
  <#
    Downloads with parallel byte ranges when the host supports them. A single stream from
    codeload measured ~1 MB/min on this network; ranged parallel chunks measured ~1.6 MB/s.
    Falls back to a plain download when the server ignores Range.
  #>
  param([string]$Url, [string]$Destination, [int]$Parts = 8)

  $chunkDir = Join-Path $work 'chunks'
  if (Test-Path $chunkDir) { Remove-Item $chunkDir -Recurse -Force }
  New-Item -ItemType Directory -Path $chunkDir | Out-Null

  $head = Invoke-WebRequest -Uri $Url -Method Head -TimeoutSec 25 -UseBasicParsing
  $total = [int]$head.Headers['Content-Length']
  if ($total -le 0) { throw "Cannot determine the artifact size from $Url" }

  $rangeProbe = & curl.exe -sS -L -r 0-1023 -o NUL -w '%{http_code}' --connect-timeout 15 --max-time 40 $Url 2>$null
  if ($rangeProbe -notmatch '^206') {
    Write-Step "Server ignores Range; falling back to a single download"
    & curl.exe -sS -L --retry 5 --retry-delay 3 --retry-all-errors -o $Destination $Url
    if ($LASTEXITCODE -ne 0) { throw "Download failed: $Url" }
    return
  }

  $size = [math]::Ceiling($total / $Parts)
  $procs = @()
  for ($i = 0; $i -lt $Parts; $i++) {
    $start = $i * $size
    $end = [math]::Min($total - 1, ($i + 1) * $size - 1)
    $chunk = Join-Path $chunkDir ("part{0:D2}.bin" -f $i)
    $procs += Start-Process -FilePath 'curl.exe' `
      -ArgumentList '-sS','-L','--retry','8','--retry-delay','2','--retry-all-errors',
        '--speed-time','10','--speed-limit','15000','--max-time','300',
        '-r', "$start-$end", '-o', $chunk, $Url `
      -NoNewWindow -PassThru
  }
  $procs | ForEach-Object { $_.WaitForExit(320000) | Out-Null }

  for ($i = 0; $i -lt $Parts; $i++) {
    $start = $i * $size
    $end = [math]::Min($total - 1, ($i + 1) * $size - 1)
    $chunk = Join-Path $chunkDir ("part{0:D2}.bin" -f $i)
    $have = if (Test-Path $chunk) { (Get-Item $chunk).Length } else { 0 }
    if ($have -ne ($end - $start + 1)) { throw "Chunk $i is incomplete ($have of $($end - $start + 1) bytes)" }
  }

  $stream = [IO.File]::Create($Destination)
  try {
    for ($i = 0; $i -lt $Parts; $i++) {
      $bytes = [IO.File]::ReadAllBytes((Join-Path $chunkDir ("part{0:D2}.bin" -f $i)))
      $stream.Write($bytes, 0, $bytes.Length)
    }
  } finally { $stream.Close() }
  if ((Get-Item $Destination).Length -ne $total) { throw "Assembled artifact size mismatch" }
}

function New-VenvWithPython {
  param([string]$Path, [string]$Target)
  $uv = Join-Path $env:LOCALAPPDATA 'hermes\bin\uv.exe'
  if (Test-Path $uv) {
    & $uv venv --python $Target $Path
    if ($LASTEXITCODE -eq 0) { return }
  }
  $launcher = Get-Command py -ErrorAction SilentlyContinue
  if ($launcher) { & py "-$Target" -m venv $Path } else { throw "No way to create a $Target virtual environment" }
}

Write-Step "Pinned release: $($lock.tag) ($($lock.commit))"
$releaseCommit = Get-RemoteSha -Tag $lock.tag
if ($releaseCommit -and $releaseCommit -ne $lock.commit) {
  Write-Warning "Tag $($lock.tag) currently points at $releaseCommit, but hermes.lock.json pins $($lock.commit)."
}

if (Test-Path $InstallDir) { Remove-Item $InstallDir -Recurse -Force }
New-Item -ItemType Directory -Path $InstallDir | Out-Null

Write-Step 'Downloading the pinned source tarball'
if (Test-Path $work) { Remove-Item $work -Recurse -Force }
New-Item -ItemType Directory -Path $work | Out-Null
$url = Resolve-TarballUrl -Commit $lock.commit
Invoke-Download -Url $url -Destination $tarball
Write-Step ("Downloaded {0:N1} MB" -f ((Get-Item $tarball).Length / 1MB))

Write-Step 'Extracting'
$extract = Join-Path $work 'extract'
New-Item -ItemType Directory -Path $extract | Out-Null
& tar.exe -xzf $tarball -C $extract
if ($LASTEXITCODE -ne 0) { throw 'Extraction failed' }
$inner = Get-ChildItem $extract -Directory | Select-Object -First 1
Move-Item $inner.FullName $InstallDir

Write-Step "Creating a Python $pythonVersion virtual environment"
New-VenvWithPython -Path $venvDir -Target $pythonVersion
& $venvPython --version

Write-Step 'Installing Hermes and its dependencies'
$uv = Join-Path $env:LOCALAPPDATA 'hermes\bin\uv.exe'
Push-Location $InstallDir
try {
  if (Test-Path $uv) {
    & $uv pip install --python $venvPython -e . --quiet
    if ($LASTEXITCODE -ne 0) { throw 'uv pip install failed' }
    # The API server adapter needs aiohttp, which is not a core dependency.
    & $uv pip install --python $venvPython aiohttp --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Installing aiohttp failed' }
    # The Anthropic SDK is an optional extra. Hermes resolves it lazily on the first
    # agent turn (pm.ensure_import), so a missing SDK makes every turn stall in a
    # runtime package download instead of failing fast. Pre-install it.
    & $uv pip install --python $venvPython 'anthropic==0.125.0' --quiet
    if ($LASTEXITCODE -ne 0) { throw 'Installing the anthropic SDK failed' }
  } else {
    & $venvPython -m pip install -e . --quiet
    if ($LASTEXITCODE -ne 0) { throw 'pip install failed' }
    & $venvPython -m pip install aiohttp --quiet
    & $venvPython -m pip install 'anthropic==0.125.0' --quiet
  }
} finally { Pop-Location }

Write-Step 'Writing the install stamp (the release tarball ships version 0.0.0)'
& $venvPython (Join-Path $InstallDir 'scripts\write_install_stamp.py') `
  --output (Join-Path $InstallDir 'install-stamp.json') `
  --commit $lock.commit --base-version $lock.version `
  --update-mechanism self --source local
if ($LASTEXITCODE -ne 0) { throw 'Failed to write install-stamp.json' }

Write-Step 'Verifying the installed runtime'
$versionLine = & $venvPython -m hermes_cli.main --version 2>&1 | Select-Object -First 1
Write-Host "    $versionLine"
if ($versionLine -notmatch [regex]::Escape($lock.version)) {
  throw "The installed runtime does not report $($lock.version): $versionLine"
}

Write-Step "Preparing the isolated profile at $ProfileDir"
New-Item -ItemType Directory -Path $ProfileDir -Force | Out-Null
$profileEnv = Join-Path $ProfileDir '.env'
if (-not (Test-Path $profileEnv)) {
  $apiKey = [guid]::NewGuid().ToString('N').Substring(0, 32)
  $lines = @(
    '# Isolated Hermes profile for the CodeWeaver Phase 0 lab.',
    '# Hermes reads provider credentials and API-server settings from this file.',
    'API_SERVER_ENABLED=true',
    "API_SERVER_KEY=$apiKey",
    "API_SERVER_PORT=$ApiServerPort",
    '# MiniMax China platform key (https://platform.minimaxi.com). Anthropic Messages wire.',
    'MINIMAX_CN_API_KEY='
  )
  [IO.File]::WriteAllText($profileEnv, ($lines -join "`n"), (New-Object Text.UTF8Encoding($false)))
  Write-Host "    wrote $profileEnv (add MINIMAX_CN_API_KEY to enable model-backed runs)"
} else {
  Write-Host "    kept the existing $profileEnv"
}

if ($StartGateway) {
  Write-Step 'Starting the Hermes API server'
  $env:HERMES_HOME = $ProfileDir
  $log = Join-Path $env:TEMP 'hermes-gateway.log'
  $process = Start-Process -FilePath $venvPython `
    -ArgumentList '-m', 'hermes_cli.main', 'gateway', 'run', '--accept-hooks' `
    -NoNewWindow -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"
  Start-Sleep -Seconds 25
  $listener = Get-NetTCPConnection -LocalPort $ApiServerPort -State Listen -ErrorAction SilentlyContinue
  if ($listener) {
    Write-Host "    API server listening on 127.0.0.1:$ApiServerPort (pid $($listener.OwningProcess))"
  } else {
    Write-Warning "The API server is not listening on port $ApiServerPort yet. Check $log.err"
  }
}

Write-Step 'Done'
Write-Host "    interpreter : $venvPython"
Write-Host "    profile     : $ProfileDir"
Write-Host "    verify with : npm run check:hermes (set HERMES_PYTHON to the interpreter above)"
