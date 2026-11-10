<#
.SYNOPSIS
    同时启动 DocuFlow 后端（lab stack）与 Workspace 前端。

.DESCRIPTION
    后端：docuflow-hermes-lab 的 dev:stack（mock Hermes + 真实 API 服务），监听 127.0.0.1:8787。
    前端：docuflow-workspace 的 Vite（浏览器形态）或 Tauri 桌面形态。

    桌面形态由 `npm run tauri dev` 自行拉起 Vite（tauri.conf.json 的 beforeDevCommand），
    所以本脚本不会重复启动前端 dev server。

    两个进程的输出会合并到同一个控制台，带 [api] / [ui] 前缀；Ctrl+C 会一并结束两者。

.PARAMETER Mode
    desktop = Tauri 桌面窗口（默认）；browser = 只起 Vite 并自动打开浏览器。

.PARAMETER NoBuild
    跳过 lab 的 TypeScript 编译，直接使用现有 dist。

.PARAMETER Force
    端口被占用时，先结束占用进程再启动。

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1

.EXAMPLE
    powershell -ExecutionPolicy Bypass -File .\scripts\dev.ps1 -Mode browser
#>
param(
    [ValidateSet("desktop", "browser")]
    [string] $Mode = "desktop",
    [switch] $NoBuild,
    [switch] $Force,
    [switch] $NoBrowser
)

$ErrorActionPreference = "Stop"

# PowerShell 5.1 writes redirected output in the console OEM code page, which turns
# the Chinese log lines into mojibake. Force UTF-8 so the log files stay readable.
try {
    [Console]::OutputEncoding = [System.Text.Encoding]::UTF8
    $OutputEncoding = [System.Text.Encoding]::UTF8
} catch {
    # Redirected streams cannot always take a new encoding; logs degrade, startup still works.
}

$Root = Split-Path -Parent $PSScriptRoot
$LabDir = Join-Path $Root "docuflow-hermes-lab"
$UiDir = Join-Path $Root "docuflow-workspace"
$LogDir = Join-Path $Root ".dev-logs"
$PidFile = Join-Path $LogDir "pids.txt"
$ApiPort = 8787
$WebPort = 1420

function Write-Step {
    param([string] $Message)
    Write-Host "==> $Message" -ForegroundColor Cyan
}

function Write-Note {
    param([string] $Message)
    Write-Host "    $Message" -ForegroundColor DarkGray
}

function Get-ListeningOwner {
    param([int] $Port)
    # Get-NetTCPConnection is the authoritative source here: Vite binds "localhost",
    # which resolves to ::1 (IPv6) on this machine, and an IPv4 TcpClient probe
    # cannot see that listener at all.
    try {
        $conn = Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction Stop |
            Select-Object -First 1
        if ($conn) { return $conn.OwningProcess }
    } catch {
        # Get-NetTCPConnection is unavailable on some editions; fall back to probing.
    }
    return $null
}

function Test-PortInUse {
    param([int] $Port)
    if (Get-ListeningOwner -Port $Port) { return $true }
    # Fallback for editions without Get-NetTCPConnection.
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $client.Connect("127.0.0.1", $Port)
        return $true
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

function Invoke-Taskkill {
    param([int] $TargetId)
    if (-not $TargetId) { return }
    # taskkill writes "not found" to stderr; with ErrorActionPreference=Stop that
    # would throw from inside the cleanup path and skip the rest of the teardown.
    $previous = $ErrorActionPreference
    $ErrorActionPreference = "SilentlyContinue"
    try {
        taskkill.exe /PID $TargetId /T /F 2>$null | Out-Null
    } finally {
        $ErrorActionPreference = $previous
    }
}

function Stop-Tree {
    param([int] $TargetId)
    Invoke-Taskkill -TargetId $TargetId
}

function Stop-PortOwner {
    param([int] $Port)
    # npm.cmd is a batch file: Windows spawns an extra cmd.exe between it and the
    # real node process, so killing the npm tree can orphan the dev server. The
    # listener itself is the reliable handle, so it gets killed directly too.
    $owner = Get-ListeningOwner -Port $Port
    if ($owner) {
        Invoke-Taskkill -TargetId $owner
        Start-Sleep -Milliseconds 300
    }
    if (Test-PortInUse $Port) {
        $owner = Get-ListeningOwner -Port $Port
        if ($owner) { Invoke-Taskkill -TargetId $owner }
    }
}

function Stop-All {
    param([object[]] $Handles, [int[]] $Ports)
    # Listeners first: they are the real node processes. Killing the npm.cmd
    # wrapper first would let the batch job print "Terminate batch job (Y/N)?"
    # and block the teardown on a console prompt.
    foreach ($port in $Ports) { Stop-PortOwner -Port $port }
    foreach ($handle in $Handles) {
        if ($handle -and $handle.Process) { Stop-Tree -TargetId $handle.Process.Id }
    }
    foreach ($handle in $Handles) {
        if ($handle -and $handle.Reader) { $handle.Reader.Dispose() }
        if ($handle -and $handle.Stream) { $handle.Stream.Dispose() }
    }
}

function Get-ExitCode {
    param($Process)
    # A Process from Start-Process -PassThru only fills ExitCode after the handle
    # has been cached and WaitForExit ran; without that it stays null.
    if (-not $Process) { return "unknown" }
    try {
        $null = $Process.Handle
        $Process.WaitForExit(2000) | Out-Null
        if ($null -eq $Process.ExitCode) { return "unknown" }
        return $Process.ExitCode
    } catch {
        return "unknown"
    }
}

function Assert-PortFree {
    param([int] $Port, [string] $What)

    if (-not (Test-PortInUse $Port)) { return }

    $owner = Get-ListeningOwner -Port $Port
    $who = if ($owner) { " (PID $owner)" } else { "" }
    if (-not $Force) {
        throw "端口 $Port 已被占用$who —— $What 需要该端口。确认要覆盖运行中的实例后，加 -Force 重试。"
    }
    Write-Note "端口 $Port 被占用$who，-Force 已启用，正在结束占用进程。"
    if ($owner) { Stop-Tree -TargetId $owner }
    Start-Sleep -Milliseconds 800
    if (Test-PortInUse $Port) {
        throw "端口 $Port 仍被占用，$What 无法启动。"
    }
}

function Start-Tailed {
    param(
        [string] $Tag,
        [string] $WorkingDirectory,
        [string] $Command,
        [string[]] $Arguments
    )

    $outLog = Join-Path $LogDir "$Tag.out.log"
    $errLog = Join-Path $LogDir "$Tag.err.log"
    $process = Start-Process -FilePath $Command -ArgumentList $Arguments `
        -WorkingDirectory $WorkingDirectory -PassThru -NoNewWindow `
        -RedirectStandardOutput $outLog -RedirectStandardError $errLog

    $stream = [System.IO.File]::Open($outLog, [System.IO.FileMode]::OpenOrCreate,
        [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
    $reader = New-Object System.IO.StreamReader($stream, [System.Text.Encoding]::UTF8)

    return [PSCustomObject]@{
        Tag = $Tag
        Process = $process
        Reader = $reader
        Stream = $stream
        OutLog = $outLog
        ErrLog = $errLog
    }
}

function Read-NewLines {
    param($Reader)
    $lines = New-Object System.Collections.ArrayList
    while ($true) {
        $line = $Reader.ReadLine()
        if ($null -eq $line) { break }
        [void] $lines.Add($line)
    }
    return $lines
}

function Save-Pids {
    param([object[]] $Handles)
    $lines = @()
    foreach ($handle in $Handles) {
        if ($handle -and $handle.Process) { $lines += "$($handle.Tag)|$($handle.Process.Id)" }
    }
    [System.IO.File]::WriteAllText($PidFile, ($lines -join "`n"))
}

# Ctrl+C can bypass the finally block, so the PIDs are also flushed to disk and
# cleaned up by the PowerShell.Exiting event. The path is captured here because
# $PSScriptRoot is not resolvable inside the event action's own runspace.
$script:PidFileForExit = $PidFile
Register-EngineEvent -SourceIdentifier "PowerShell.Exiting" -Action {
    $pidFile = $script:PidFileForExit
    if ($pidFile -and (Test-Path $pidFile)) {
        foreach ($line in [System.IO.File]::ReadAllLines($pidFile)) {
            if ($line -match '^(\w+)\|(\d+)$') {
                taskkill.exe /PID $Matches[2] /T /F 2>&1 | Out-Null
            }
        }
    }
    # The recorded PIDs are the npm wrappers; the real listeners can outlive them.
    foreach ($port in @(8787, 1420)) {
        try {
            $conn = Get-NetTCPConnection -LocalPort $port -State Listen -ErrorAction Stop |
                Select-Object -First 1
            if ($conn) { taskkill.exe /PID $conn.OwningProcess /T /F 2>&1 | Out-Null }
        } catch {
            # Nothing listening, or the cmdlet is unavailable.
        }
    }
} | Out-Null

$handles = @()

try {
    # ---------------------------------------------------------------- preflight
    Write-Step "检查运行环境"

    if (-not (Get-Command node -ErrorAction SilentlyContinue)) {
        throw "找不到 node，请先安装 Node.js。"
    }
    $nodeVersion = (node --version).Trim()
    Write-Note "node $nodeVersion"

    foreach ($dir in @($LabDir, $UiDir)) {
        if (-not (Test-Path $dir)) { throw "目录不存在：$dir" }
    }

    foreach ($pair in @(@($LabDir, "docuflow-hermes-lab"), @($UiDir, "docuflow-workspace"))) {
        $modules = Join-Path $pair[0] "node_modules"
        if (-not (Test-Path $modules)) {
            Write-Note "$($pair[1]) 缺少 node_modules，正在安装依赖…"
            Push-Location $pair[0]
            try { npm install | Out-Null } finally { Pop-Location }
        }
    }

    if (-not (Test-Path $LogDir)) { New-Item -ItemType Directory -Path $LogDir | Out-Null }

    # ------------------------------------------------------------------- build
    if (-not $NoBuild) {
        Write-Step "编译后端（docuflow-hermes-lab）"
        Push-Location $LabDir
        try {
            npm run build
            $buildExit = $LASTEXITCODE
        } finally {
            Pop-Location
        }
        if ($buildExit -ne 0) {
            $distEntry = Join-Path $LabDir "dist\runners\dev-stack.js"
            if (-not (Test-Path $distEntry)) {
                throw "后端编译失败且没有可用的 dist，请先修复 docuflow-hermes-lab 的类型错误。"
            }
            Write-Note "编译报错但 dist 已生成（tsc 仍会 emit），继续使用现有 dist。"
            Write-Note "如需强制干净构建，请先修复 lab 的类型错误后重试。"
        }
    }

    # -------------------------------------------------------------------- ports
    Write-Step "检查端口"
    Assert-PortFree -Port $ApiPort -What "后端 API"
    # 桌面模式下 Vite 由 tauri dev 拉起，但它同样监听 1420，所以两种模式都要先确认。
    Assert-PortFree -Port $WebPort -What "前端 Vite"

    # ------------------------------------------------------------------ backend
    Write-Step "启动后端（$ApiPort）"
    $api = Start-Tailed -Tag "api" -WorkingDirectory $LabDir `
        -Command "node" -Arguments @("dist\runners\dev-stack.js")
    $handles += $api

    $ready = $false
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 500
        if ($api.Process.HasExited) {
            throw "后端进程启动即退出（退出码 $(Get-ExitCode $api.Process)）。日志：$($api.OutLog)"
        }
        try {
            $probe = Invoke-WebRequest -Uri "http://127.0.0.1:$ApiPort/api/v1/runtimes" `
                -UseBasicParsing -TimeoutSec 2
            if ($probe.StatusCode -eq 200) { $ready = $true; break }
        } catch {
            # Not listening yet.
        }
    }
    if (-not $ready) { throw "后端在 30 秒内没有就绪，日志：$($api.OutLog)" }
    Write-Note "后端就绪：http://127.0.0.1:$ApiPort"

    # ------------------------------------------------------------------ frontend
    Write-Step "启动前端（$Mode）"
    if ($Mode -eq "desktop") {
        $ui = Start-Tailed -Tag "ui" -WorkingDirectory $UiDir `
            -Command "npm.cmd" -Arguments @("run", "tauri", "dev")
    } else {
        $ui = Start-Tailed -Tag "ui" -WorkingDirectory $UiDir `
            -Command "npm.cmd" -Arguments @("run", "dev")
    }
    $handles += $ui
    Save-Pids -Handles $handles

    # Both modes need Vite on 1420: in desktop mode `tauri dev` starts it as a
    # beforeDevCommand, in browser mode it is the dev server itself.
    $webReady = $false
    $webTimeout = if ($Mode -eq "desktop") { 300 } else { 60 }
    for ($i = 0; $i -lt $webTimeout; $i++) {
        Start-Sleep -Milliseconds 500
        if (Test-PortInUse $WebPort) { $webReady = $true; break }
        if ($ui.Process.HasExited) {
            throw "前端进程启动即退出（退出码 $(Get-ExitCode $ui.Process)）。日志：$($ui.OutLog)"
        }
    }
    if (-not $webReady) { throw "前端在 $([int]($webTimeout / 2)) 秒内没有监听 $WebPort，日志：$($ui.OutLog)" }
    Write-Note "前端就绪：http://localhost:$WebPort"
    if ($Mode -eq "browser" -and -not $NoBrowser) {
        Start-Process "http://localhost:$WebPort"
    }

    Write-Step "运行中 —— Ctrl+C 结束"
    Write-Note "日志文件：$LogDir"

    # -------------------------------------------------------------- output loop
    # Ctrl+C / Ctrl+Break surface as a pipeline stop. PowerShell routes Ctrl+Break
    # through its debugger, so no catch/trap can intercept it reliably; the
    # teardown in the finally block is what actually guarantees the children die.
    while ($true) {
        foreach ($handle in $handles) {
            foreach ($line in (Read-NewLines -Reader $handle.Reader)) {
                Write-Host "[$($handle.Tag)] $line"
            }
        }
        foreach ($handle in $handles) {
            if ($handle.Process.HasExited) {
                Write-Host "[$($handle.Tag)] 进程已退出，退出码 $(Get-ExitCode $handle.Process)" -ForegroundColor Yellow
                throw "进程 $($handle.Tag) 提前退出，日志：$($handle.OutLog)"
            }
        }
        Start-Sleep -Milliseconds 120
    }
} catch {
    Write-Host ""
    Write-Host "启动失败：$($_.Exception.Message)" -ForegroundColor Red
    exit 1
} finally {
    # Kill the listeners too: the npm.cmd -> cmd.exe -> node chain can orphan the
    # dev server when only the recorded PIDs are killed.
    Stop-All -Handles $handles -Ports @($ApiPort, $WebPort)
    # Empty the PID file rather than deleting it: a stale file would make the
    # exit handler kill unrelated processes on a later run.
    if (Test-Path $PidFile) { [System.IO.File]::WriteAllText($PidFile, "") }
}
