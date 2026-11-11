<#
.SYNOPSIS
Starts, stops and inspects the pinned Hermes runtime used by the CodeWeaver lab.

.DESCRIPTION
The Hermes API server is a separate local process. This script is its lifecycle: start it in the
background, stop it, and report whether it is healthy. It exists so the pinned runtime can be run
without Docker and without a manual terminal dance.

Three facts drive the design:

  * The gateway forks. `gateway run` spawns a child that owns the listening socket, so the PID
    written at start is not the PID that answers on the port. The script records the parent PID and
    resolves the listener separately, because "the process I started" and "the process serving 8642"
    are different processes.
  * The profile holds the API key. `API_SERVER_KEY` in the profile `.env` is what the API server
    demands as a bearer token, so the script reads it from there rather than asking for it again.
  * A manually started Hermes must survive. `stop` refuses to kill a gateway this script did not
    start, unless `-Force` is passed, so stopping the lab never takes down a developer's own
    long-lived instance.

.PARAMETER Action
start | stop | status | restart. Defaults to status.

.PARAMETER InstallDir
Install root. Defaults to %LOCALAPPDATA%\hermes\hermes-agent-<version> from hermes.lock.json.

.PARAMETER ProfileDir
HERMES_HOME profile. Defaults to <InstallDir>\profile.

.PARAMETER Port
API server port. Defaults to 8642.

.PARAMETER Force
Allow `stop` to terminate a gateway this script did not start.

.PARAMETER TimeoutSeconds
How long `start` waits for the API server to answer. Defaults to 90.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts/hermes.ps1 status

.EXAMPLE
powershell -ExecutionPolicy Bypass -File scripts/hermes.ps1 start
#>
[CmdletBinding()]
param(
  [ValidateSet('start', 'stop', 'status', 'restart')]
  [string]$Action = 'status',
  [string]$InstallDir = '',
  [string]$ProfileDir = '',
  [int]$Port = 8642,
  [switch]$Force,
  [int]$TimeoutSeconds = 90
)

$ErrorActionPreference = 'Stop'
# The lock file lives in the lab root (this script's parent directory), not at the repository root.
$labRoot = Split-Path -Parent $PSScriptRoot
$lockPath = Join-Path $labRoot 'hermes.lock.json'
if (-not (Test-Path $lockPath)) { throw "hermes.lock.json not found at $lockPath" }
$lock = Get-Content $lockPath -Raw | ConvertFrom-Json

if (-not $InstallDir) { $InstallDir = Join-Path $env:LOCALAPPDATA "hermes\hermes-agent-$($lock.version)" }
if (-not $ProfileDir) { $ProfileDir = Join-Path $InstallDir 'profile' }

$venvPython = Join-Path $InstallDir 'venv\Scripts\python.exe'
$stateDir = Join-Path $InstallDir 'run'
$pidFile = Join-Path $stateDir 'gateway.pid'
$outLog = Join-Path $stateDir 'gateway.out.log'
$errLog = Join-Path $stateDir 'gateway.err.log'

function Write-Step([string]$message) { Write-Host "==> $message" }
function Write-Info([string]$message) { Write-Host "    $message" }
function Write-Warn([string]$message) { Write-Host "    WARNING: $message" }

function Get-ProfileEnv {
  # The profile .env is the API server's own configuration: it holds API_SERVER_KEY, which the
  # server demands as a bearer token. Reading it here keeps the key in one place instead of
  # asking the operator to supply it a second time.
  $envFile = Join-Path $ProfileDir '.env'
  $values = @{}
  if (-not (Test-Path $envFile)) { return $values }
  foreach ($line in Get-Content $envFile) {
    if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$') {
      $values[$matches[1]] = $matches[2].Trim()
    }
  }
  return $values
}

function Get-ListeningPid([int]$PortNumber) {
  $listener = Get-NetTCPConnection -LocalPort $PortNumber -State Listen -ErrorAction SilentlyContinue |
    Select-Object -First 1
  if ($listener) { return $listener.OwningProcess }
  return $null
}

function Get-RecordedPid {
  if (-not (Test-Path $pidFile)) { return $null }
  try {
    $raw = (Get-Content $pidFile -Raw).Trim()
    if (-not $raw) { return $null }
    $value = [int]$raw
    if ($value -le 0) { return $null }
    return $value
  } catch {
    return $null
  }
}

function Test-ProcessAlive([int]$ProcessId) {
  if (-not $ProcessId) { return $false }
  try {
    $proc = Get-Process -Id $ProcessId -ErrorAction Stop
    return -not $proc.HasExited
  } catch {
    return $false
  }
}

function Get-HermesChildPid([int]$ParentPid) {
  # `gateway run` forks: the parent supervises, the child owns the socket. The child is found by
  # parent PID rather than by command line, because the command line is identical for both.
  $child = Get-CimInstance Win32_Process -Filter "ParentProcessId=$ParentPid" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -match 'hermes_cli\.main' } |
    Select-Object -First 1
  if ($child) { return [int]$child.ProcessId }
  return $null
}

function Get-HermesGatewayPid {
  # Any running Hermes gateway process, whoever started it.
  $procs = Get-CimInstance Win32_Process -Filter "Name='python.exe'" -ErrorAction SilentlyContinue |
    Where-Object { $_.CommandLine -match 'hermes_cli\.main.*gateway' }
  if (-not $procs) { return $null }
  $listener = Get-ListeningPid $Port
  if ($listener) {
    $match = $procs | Where-Object { $_.ProcessId -eq $listener } | Select-Object -First 1
    if ($match) { return [int]$match.ProcessId }
  }
  return [int]($procs | Select-Object -First 1).ProcessId
}

function Invoke-Capabilities([string]$ApiKey) {
  $headers = @{}
  if ($ApiKey) { $headers['Authorization'] = "Bearer $ApiKey" }
  try {
    $response = Invoke-WebRequest -Uri "http://127.0.0.1:$Port/v1/capabilities" `
      -Headers $headers -TimeoutSec 5 -UseBasicParsing
    if ($response.StatusCode -ne 200) { return $null }
    return $response.Content | ConvertFrom-Json
  } catch {
    return $null
  }
}

function Test-HermesHealthy([string]$ApiKey) {
  return $null -ne (Invoke-Capabilities $ApiKey)
}

function Start-Hermes {
  if (-not (Test-Path $venvPython)) {
    throw "Hermes is not installed at $InstallDir (no venv\Scripts\python.exe). Run scripts/install-hermes.ps1 first."
  }

  $profileEnv = Get-ProfileEnv
  $apiKey = $profileEnv['API_SERVER_KEY']

  $existing = Get-HermesGatewayPid
  if ($existing -and (Test-HermesHealthy $apiKey)) {
    Write-Step 'Hermes is already running'
    Write-Info "pid: $existing"
    Write-Info "api: http://127.0.0.1:$Port"
    return
  }
  if ($existing) {
    # A process is there but it is not answering. Stop it rather than starting a second one that
    # would fight for the port.
    Write-Warn "a Hermes process ($existing) is running but not answering on port $Port; stopping it first"
    Stop-HermesGateway $existing
  }

  New-Item -ItemType Directory -Path $stateDir -Force | Out-Null

  Write-Step "Starting Hermes $($lock.version) on port $Port"
  $env:HERMES_HOME = $ProfileDir
  if ($profileEnv['API_SERVER_PORT']) { $env:API_SERVER_PORT = $profileEnv['API_SERVER_PORT'] }
  if ($profileEnv['API_SERVER_ENABLED']) { $env:API_SERVER_ENABLED = $profileEnv['API_SERVER_ENABLED'] }

  $process = Start-Process -FilePath $venvPython `
    -ArgumentList '-m', 'hermes_cli.main', 'gateway', 'run', '--accept-hooks' `
    -WorkingDirectory $InstallDir `
    -NoNewWindow -PassThru `
    -RedirectStandardOutput $outLog -RedirectStandardError $errLog

  [IO.File]::WriteAllText($pidFile, [string]$process.Id)

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    if ($process.HasExited) {
      Write-Warn "the gateway process exited with code $($process.ExitCode)"
      if (Test-Path $errLog) {
        Write-Host '    --- stderr tail ---'
        Get-Content $errLog -Tail 20 | ForEach-Object { Write-Host "    $_" }
      }
      throw 'Hermes did not stay up.'
    }
    if (Test-HermesHealthy $apiKey) {
      $listener = Get-ListeningPid $Port
      Write-Step 'Hermes is up'
      Write-Info "supervisor pid: $($process.Id)"
      if ($listener) { Write-Info "serving pid:    $listener" }
      Write-Info "api: http://127.0.0.1:$Port"
      Write-Info "logs: $outLog / $errLog"
      return
    }
  }
  throw "Hermes did not answer on port $Port within $TimeoutSeconds seconds. See $errLog"
}

function Stop-HermesGateway([int]$ProcessId) {
  if (-not $ProcessId) { return }
  # The supervisor's own children die with it; the child that owns the socket is a grandchild of
  # the shell but a child of the supervisor, so killing the supervisor's tree takes it along.
  try {
    $proc = Get-CimInstance Win32_Process -Filter "ProcessId=$ProcessId" -ErrorAction SilentlyContinue
    $children = @()
    if ($proc) {
      $children = Get-CimInstance Win32_Process -Filter "ParentProcessId=$ProcessId" -ErrorAction SilentlyContinue
    }
    foreach ($child in $children) {
      Stop-Process -Id $child.ProcessId -Force -ErrorAction SilentlyContinue
    }
    Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
  } catch {
    Write-Warn "failed to stop pid $ProcessId : $($_.Exception.Message)"
  }
}

function Stop-Hermes {
  $profileEnv = Get-ProfileEnv
  $apiKey = $profileEnv['API_SERVER_KEY']
  $recorded = Get-RecordedPid
  $gatewayPid = Get-HermesGatewayPid

  if (-not $gatewayPid) {
    Write-Step 'Hermes is not running'
    if (Test-Path $pidFile) { Remove-Item $pidFile -Force }
    return
  }

  if (-not $Force -and -not ($recorded -and ($recorded -eq $gatewayPid -or $gatewayPid -eq (Get-HermesChildPid $recorded)))) {
    # This script did not start it. A developer's own long-lived gateway must survive the lab
    # shutting down; `-Force` is the explicit override.
    Write-Step 'Hermes is running but was not started by this script'
    Write-Info "pid: $gatewayPid"
    Write-Warn 'refusing to stop it; pass -Force if you really mean to'
    return
  }

  Write-Step "Stopping Hermes (pid $gatewayPid)"
  if ($recorded -and (Test-ProcessAlive $recorded)) {
    Stop-HermesGateway $recorded
  }
  Stop-HermesGateway $gatewayPid
  Start-Sleep -Seconds 1
  if (Test-Path $pidFile) { Remove-Item $pidFile -Force }

  $listener = Get-ListeningPid $Port
  if ($listener) {
    Write-Warn "port $Port is still held by pid $listener"
  } else {
    Write-Info "port $Port is free"
  }
}

function Show-HermesStatus {
  $profileEnv = Get-ProfileEnv
  $apiKey = $profileEnv['API_SERVER_KEY']
  $gatewayPid = Get-HermesGatewayPid
  $listener = Get-ListeningPid $Port
  $capabilities = Invoke-Capabilities $apiKey

  Write-Step 'Hermes status'
  Write-Info "version:   $($lock.version)"
  Write-Info "install:   $InstallDir"
  Write-Info "profile:   $ProfileDir"
  Write-Info "pid:       $(if ($gatewayPid) { $gatewayPid } else { '(not running)' })"
  Write-Info "port:      $(if ($listener) { "$Port (pid $listener)" } else { "$Port (free)" })"
  Write-Info "healthy:   $(if ($capabilities) { 'yes' } else { 'no' })"

  if ($capabilities) {
    Write-Info "platform:  $($capabilities.platform)"
    Write-Info "model:     $($capabilities.model)"
    $features = $capabilities.features
    if ($features) {
      Write-Info 'features:'
      foreach ($name in ($features.PSObject.Properties.Name | Sort-Object)) {
        Write-Info ("  {0}: {1}" -f $name, $features.$name)
      }
    }
  }

  if (-not $capabilities) {
    if ($gatewayPid) {
      Write-Warn 'a Hermes process is running but the API server is not answering.'
      if (Test-Path $errLog) {
        Write-Host '    --- stderr tail ---'
        Get-Content $errLog -Tail 20 | ForEach-Object { Write-Host "    $_" }
      }
      return 1
    }
    return 1
  }
  return 0
}

switch ($Action) {
  'start' { Start-Hermes }
  'stop' { Stop-Hermes }
  'restart' { Stop-Hermes; Start-Hermes }
  # The exit code is the script's own signal: 0 when the API server answers, 1 when it does not.
  # `exit (Show-HermesStatus)` would return the function's whole output stream, so the code is
  # computed first and returned explicitly.
  'status' { $code = Show-HermesStatus; exit $code }
}
