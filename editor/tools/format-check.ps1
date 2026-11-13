# The window-level check of the formatting: the editor asks the language
# server to format the file, the file is laid out the way the server wants
# it, and one undo takes the whole thing back.
#
# The server is a real one: the `rust-analyzer` of a toolchain on this
# machine, pointed at the editor with `--lsp`. The file that is opened is
# badly laid out on purpose, so the formatting has something to do.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Alt+Shift+F makes the server lay the file out: the text on screen
#    changes.
# 2. The file is marked as changed afterwards.
# 3. One undo takes the whole formatting back: the text on screen is the
#    text that was there before, and the file is not marked as changed any
#    more.
#
# The keys are injected into the system, the way a keyboard would, because
# the window reads the state of the modifiers from the keyboard itself: a
# message that is only posted to the window does not change that state.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'format-project'
$source = Join-Path $project 'src\main.rs'
$shotBefore = Join-Path $logs 'format-before.png'
$shotAfter = Join-Path $logs 'format-after.png'
$shotUndone = Join-Path $logs 'format-undone.png'
$report = Join-Path $logs 'format-check.json'

$sig = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr extra);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll", EntryPoint = "GetWindowThreadProcessId")] public static extern uint ThreadOfWindow(IntPtr h, IntPtr pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder text, int max);
  [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();

  public delegate bool EnumProc(IntPtr h, IntPtr extra);
  public struct RECT { public int Left, Top, Right, Bottom; }
  public struct POINT { public int X, Y; }

  public static IntPtr Find(uint wanted) {
    IntPtr found = IntPtr.Zero;
    EnumWindows(delegate(IntPtr h, IntPtr extra) {
      uint pid;
      GetWindowThreadProcessId(h, out pid);
      if (pid != wanted) return true;
      if (!IsWindowVisible(h)) return true;
      var text = new StringBuilder(256);
      GetWindowText(h, text, 256);
      if (text.ToString() != "CodeWeaver") return true;
      RECT r;
      if (!GetClientRect(h, out r) || r.Right <= 0 || r.Bottom <= 0) return true;
      found = h;
      return false;
    }, IntPtr.Zero);
    return found;
  }
}
'@
Add-Type -TypeDefinition $sig
Add-Type -AssemblyName System.Drawing

[Win32]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
$awareness = [Win32]::GetThreadDpiAwarenessContext()
$scriptIsAware = ($awareness -eq [IntPtr](-4)) -or ($awareness -eq [IntPtr](-3))
Write-Output "the script is scale aware: $scriptIsAware"

$TOPMOST = [IntPtr](-1)
$SWP_SHOWWINDOW = 0x0040

# The keys the check presses.
$VK_SHIFT = 0x10
$VK_CONTROL = 0x11
$VK_MENU = 0x12
$VK_F = 0x46
$VK_Z = 0x5A

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The colour of the dirty mark on a tab: text, so the colour is the one the
# code writes.
$DIRTY_RGB = To-Bytes @(0.30, 0.58, 0.96)

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}

# The language server of this machine.
#
# The `rust-analyzer` on the path is the rustup proxy, and the proxy is of no
# use when the component is not installed in the toolchain it points at. The
# real binary is in a toolchain of its own, so it is looked for there.
function Find-Server {
    $toolchains = Join-Path $env:USERPROFILE '.rustup\toolchains'
    if (Test-Path $toolchains) {
        $found = Get-ChildItem -Path $toolchains -Recurse -Filter 'rust-analyzer.exe' -ErrorAction SilentlyContinue |
            Sort-Object FullName -Descending |
            Select-Object -First 1
        if ($found) { return $found.FullName }
    }
    $onPath = Get-Command rust-analyzer -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}
$server = Find-Server
if (-not $server) {
    throw 'there is no rust-analyzer on this machine'
}
Write-Output "the language server is $server"

# The check drives the editor that is on disk, so that one has to be the one
# the sources describe. `cargo test` does not build it.
$editorCrates = @('app', 'platform', 'render', 'text', 'syntax', 'lsp', 'ui', 'workspace', 'workbench', 'editor-core', 'agent-acp', 'agent-ui', 'diff')
$newest = $editorCrates | ForEach-Object {
    $crate = Join-Path $root "crates\$_"
    if (Test-Path $crate) {
        Get-ChildItem -Path (Join-Path $crate 'src') -Recurse -Include *.rs -ErrorAction SilentlyContinue
    }
} | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($newest -and (Get-Item $exe).LastWriteTime -lt $newest.LastWriteTime) {
    throw "the editor is older than its sources; run 'cargo build -p codeweaver-editor' first"
}

# The editor is started from a folder of its own, outside the workspace.
#
# The workspace carries a low integrity label, so a program built there runs
# with fewer rights than the programs it starts. The copy runs with the rights
# a program normally has.
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null

# The project the file belongs to.
#
# It is outside the workspace, and it has a workspace of its own: a project
# that lies inside another one is refused by cargo, and the server would have
# no project to load.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path (Join-Path $project 'src') | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'Cargo.toml'),
    "[workspace]`n`n[package]`nname = `"format-check`"`nversion = `"0.1.0`"`nedition = `"2021`"`n",
    (New-Object System.Text.UTF8Encoding($false)))
# The file is laid out badly on purpose: the formatting has something to do.
[System.IO.File]::WriteAllText(
    $source,
    "fn main( ){let x   =  1;let y=2;   println!( `"{} {}`",x,y );}`n",
    (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the file that is not laid out: $source"

$script:Factor = 1.0
function To-Physical([double]$value) {
    if ($scriptIsAware) { return $value }
    return $value * $script:Factor
}

# Presses a key with modifiers.
#
# The keys are injected into the system, the way a keyboard would, because the
# window reads the state of the modifiers from the keyboard itself: a message
# that is only posted to the window does not change that state.
# Injected keys go to the window that has the focus, so the editor is brought
# to the front first.
function Send-Chord([int[]]$modifiers, [int]$virtualKey) {
    $sequence = @()
    foreach ($modifier in $modifiers) {
        $sequence += ,@($modifier, $false)
    }
    $sequence += ,@($virtualKey, $false)
    $sequence += ,@($virtualKey, $true)
    for ($i = $modifiers.Length - 1; $i -ge 0; $i--) {
        $sequence += ,@($modifiers[$i], $true)
    }
    foreach ($step in $sequence) {
        $flags = if ($step[1]) { 2 } else { 0 }
        [Win32]::keybd_event([byte]$step[0], 0, [uint32]$flags, [UIntPtr]::Zero)
        Start-Sleep -Milliseconds 60
    }
}

function Save-Window([IntPtr]$handle, [string]$path) {
    # A minimised window has no client area to draw into, and a window that
    # was never shown does not redraw. The restore is retried: something else
    # on the machine minimises the window now and then.
    $client = New-Object Win32+RECT
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        [Win32]::ShowWindow($handle, 9) | Out-Null
        [Win32]::SetWindowPos($handle, $TOPMOST, 40, 40, 1201, 751, $SWP_SHOWWINDOW) | Out-Null
        Start-Sleep -Seconds 2
        [Win32]::GetClientRect($handle, [ref]$client) | Out-Null
        if ($client.Right -gt 0 -and $client.Bottom -gt 0) { break }
        Write-Host "the window has no client area (attempt $($attempt + 1)), restoring it again"
    }
    if ($client.Right -le 0 -or $client.Bottom -le 0) {
        throw 'the editor window has no client area to capture'
    }

    $window = New-Object Win32+RECT
    [Win32]::GetWindowRect($handle, [ref]$window) | Out-Null
    $origin = New-Object Win32+POINT
    [Win32]::ClientToScreen($handle, [ref]$origin) | Out-Null

    $width = [int](To-Physical ($window.Right - $window.Left))
    $height = [int](To-Physical ($window.Bottom - $window.Top))
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    # PW_RENDERFULLCONTENT makes the window manager draw the window even
    # when it is behind another one.
    $ok = [Win32]::PrintWindow($handle, $hdc, 2)
    $graphics.ReleaseHdc($hdc)
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    return @{
        Bitmap = $bitmap
        Ok = $ok
        Left = $origin.X - $window.Left
        Top = $origin.Y - $window.Top
        Width = [int](To-Physical $client.Right)
        Height = [int](To-Physical $client.Bottom)
    }
}

# Counts the pixels of one colour in a rectangle of the picture.
function Count-Color($picture, [int]$left, [int]$top, [int]$width, [int]$height, [int[]]$rgb, [int]$tolerance) {
    $count = 0
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $picture.Height) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - $rgb[0]) -le $tolerance -and
                [Math]::Abs($pixel.G - $rgb[1]) -le $tolerance -and
                [Math]::Abs($pixel.B - $rgb[2]) -le $tolerance) {
                $count++
            }
        }
    }
    return $count
}

# The number of pixels that are different in two pictures, in one area.
function Count-Difference($first, $second, [int]$left, [int]$top, [int]$width, [int]$height) {
    $count = 0
    $maxX = [Math]::Min($first.Width, $second.Width)
    $maxY = [Math]::Min($first.Height, $second.Height)
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $maxY) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $maxX) { continue }
            $a = $first.GetPixel($x, $y)
            $b = $second.GetPixel($x, $y)
            if ($a.R -ne $b.R -or $a.G -ne $b.G -or $a.B -ne $b.B) { $count++ }
        }
    }
    return $count
}

# The area the text of the document is drawn in, in the picture.
function Get-BodyRect($shot) {
    $logical = $shot.Width / $script:Factor
    $panelLogical = [Math]::Min([Math]::Max($logical * 0.34, 280), 520)
    $left = $shot.Left + [int](To-Physical 236)
    $right = $shot.Left + $shot.Width - [int](To-Physical $panelLogical) - [int](To-Physical 16)
    return @{
        Left = $left
        Top = $shot.Top + [int](To-Physical 78)
        Width = [Math]::Max($right - $left, 1)
        Height = [int](To-Physical 200)
    }
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'format-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'format-app-err.log') -PassThru
    $handle = [IntPtr]::Zero
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 250
        $process.Refresh()
        if (-not $process.HasExited) {
            $handle = [Win32]::Find([uint32]$process.Id)
            if ($handle -ne [IntPtr]::Zero) { break }
        }
    }
    if ($handle -eq [IntPtr]::Zero) { throw 'the editor window never appeared' }
    # The first frame needs the GPU.
    Start-Sleep -Seconds 3
    $script:Factor = [Win32]::GetDpiForWindow($handle) / 96.0
    [Win32]::SetWindowPos($handle, $TOPMOST, 40, 40, 1200, 750, $SWP_SHOWWINDOW) | Out-Null
    Start-Sleep -Seconds 2

    # The real cursor must not be over the editor: its own messages arrive
    # after the posted ones, and the editor would follow the real cursor
    # instead of the message.
    $window = New-Object Win32+RECT
    [Win32]::GetWindowRect($handle, [ref]$window) | Out-Null
    $where = New-Object Win32+POINT
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        [Win32]::SetCursorPos(10, 10) | Out-Null
        Start-Sleep -Milliseconds 300
        [Win32]::GetCursorPos([ref]$where) | Out-Null
        if ($where.X -ge $window.Left -and $where.X -lt $window.Right -and
            $where.Y -ge $window.Top -and $where.Y -lt $window.Bottom) {
            Write-Host "the cursor is still over the editor window ($($where.X),$($where.Y)), moving it again"
            continue
        }
        break
    }
    if ($where.X -ge $window.Left -and $where.X -lt $window.Right -and
        $where.Y -ge $window.Top -and $where.Y -lt $window.Bottom) {
        throw "the cursor is over the editor window ($($where.X),$($where.Y)), so the check would be a race"
    }
    return @{ Process = $process; Handle = $handle }
}

# Gives the editor the keyboard focus.
function Focus-Editor([IntPtr]$handle) {
    $before = [Win32]::GetForegroundWindow()
    $foreground = [Win32]::GetForegroundWindow()
    $other = [Win32]::ThreadOfWindow($foreground, [IntPtr]::Zero)
    $mine = [Win32]::GetCurrentThreadId()
    [Win32]::AttachThreadInput($mine, $other, $true) | Out-Null
    [Win32]::BringWindowToTop($handle) | Out-Null
    [Win32]::SetForegroundWindow($handle) | Out-Null
    [Win32]::AttachThreadInput($mine, $other, $false) | Out-Null
    Start-Sleep -Milliseconds 400
    $now = [Win32]::GetForegroundWindow()
    Write-Host "the editor has the focus: $($now -eq $handle)"
    return $before
}

$script:checks = @()
function Check([string]$name, [bool]$ok, [string]$detail) {
    $script:checks += @{ name = $name; ok = $ok; detail = $detail }
    Write-Host ("{0}: {1} ({2})" -f $name, $(if ($ok) { 'passed' } else { 'FAILED' }), $detail)
}

try {
    $cursor = New-Object Win32+POINT
    [Win32]::GetCursorPos([ref]$cursor) | Out-Null
    [Win32]::SetCursorPos(10, 10) | Out-Null
    Start-Sleep -Milliseconds 300

    $editor = Start-Editor @('--lsp', $server, $source)
    $handle = $editor.Handle
    $previous = [IntPtr]::Zero
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        $previous = Focus-Editor $handle

        # The server needs a moment to load the project before it can lay a
        # file out.
        Start-Sleep -Seconds 8

        $before = Save-Window $handle $shotBefore
        Write-Host "saved $shotBefore"
        $body = Get-BodyRect $before

        # Alt+Shift+F asks the server to format the file. The server may
        # still be loading the project, so the key is pressed again if
        # nothing happened.
        $formatted = $null
        $changed = 0
        for ($attempt = 1; $attempt -le 4; $attempt++) {
            # The keys are injected into the system and go to the window that
            # has the focus. Something else on the machine takes the focus
            # now and then, and a window that was restored does not get it
            # back, so it is taken again before every key.
            $previous = Focus-Editor $handle
            Send-Chord @($VK_MENU, $VK_SHIFT) $VK_F
            Start-Sleep -Seconds 5
            $shot = Save-Window $handle $shotAfter
            $changed = Count-Difference $before.Bitmap $shot.Bitmap $body.Left $body.Top $body.Width $body.Height
            Write-Host "attempt ${attempt}: $changed pixels of the body changed"
            if ($changed -ge 100) {
                $formatted = $shot
                break
            }
            $shot.Bitmap.Dispose()
            Start-Sleep -Seconds 5
        }
        Check 'the server lays the file out' ($null -ne $formatted) "$changed pixels of the body changed"
        if (-not $formatted) {
            throw 'the file was not laid out, so the rest of the check has nothing to measure'
        }

        # The file is marked as changed.
        $dotTop = $formatted.Top + [int](To-Physical 5)
        $dotHeight = [int](To-Physical 26)
        $dotWidth = [int](To-Physical 220)
        $dot = Count-Color $formatted.Bitmap $formatted.Left $dotTop $dotWidth $dotHeight $DIRTY_RGB 18
        Write-Host "pixels of the dirty mark: $dot"
        Check 'the formatted file is marked in the strip' ($dot -ge 3) "$dot pixels of the dirty mark"

        # One undo takes the whole formatting back.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_Z
        $undone = $null
        $back = 0
        for ($attempt = 1; $attempt -le 4; $attempt++) {
            Start-Sleep -Seconds 2
            $shot = Save-Window $handle $shotUndone
            $back = Count-Difference $before.Bitmap $shot.Bitmap $body.Left $body.Top $body.Width $body.Height
            Write-Host "attempt ${attempt}: $back pixels of the body differ from the text before the formatting"
            if ($back -le 20) {
                $undone = $shot
                break
            }
            $shot.Bitmap.Dispose()
        }
        Check 'one undo takes the formatting back' ($null -ne $undone) "$back pixels of the body differ from the text before the formatting"

        $dotAfterUndo = 0
        if ($undone) {
            $dotAfterUndo = Count-Color $undone.Bitmap $undone.Left $dotTop $dotWidth $dotHeight $DIRTY_RGB 18
            Write-Host "pixels of the dirty mark after the undo: $dotAfterUndo"
            Check 'the file is not marked as changed after the undo' ($dotAfterUndo -lt 3) "$dotAfterUndo pixels of the dirty mark"
        }

        $result = @{
            checks = $script:checks
            server = $server
            project = $project
            pixels = @{
                changedByFormatting = $changed
                differentAfterUndo = $back
                dirtyMark = $dot
                dirtyMarkAfterUndo = $dotAfterUndo
            }
            shots = @($shotBefore, $shotAfter, $shotUndone)
        }
        [System.IO.File]::WriteAllText($report, ($result | ConvertTo-Json -Depth 6), (New-Object System.Text.UTF8Encoding($false)))
        Write-Host "saved $report"

        $failed = @($script:checks | Where-Object { -not $_.ok })
        if ($failed.Count -gt 0) {
            Write-Host "the check failed: $($failed.Count) of $($script:checks.Count) checks did not pass"
            exit 1
        }
        Write-Host "the check passed: $($script:checks.Count) of $($script:checks.Count)"
    }
    finally {
        if ($previous -ne [IntPtr]::Zero) {
            [Win32]::SetForegroundWindow($previous) | Out-Null
        }
        Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    }
}
catch {
    Write-Host "the check failed to run: $_"
    exit 1
}
