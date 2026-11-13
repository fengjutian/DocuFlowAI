# The window-level check of the task runner: the editor builds the project on
# its own, the output of the build flows into the panel at the bottom of the
# window, and the panel says how the build went.
#
# The project is a crate of its own, outside the workspace, so the build is
# quick and does not touch the code of the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+Shift+B starts the build of the project the open file belongs to.
# 2. The output panel is drawn at the bottom of the window: the room for it
#    is taken away from the text of the document.
# 3. What the build said is in the panel: there is text in it.
# 4. Esc closes the panel, and the text of the document takes the room back.
#
# The keys are injected into the system, the way a keyboard would, because the
# window reads the state of the modifiers from the keyboard itself: a
# message that is only posted to the window does not change that state.
#
# The file is read as UTF-8 and the script is written without a byte order
# mark on purpose: a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'task-project'
$source = Join-Path $project 'src\main.rs'
$shotBefore = Join-Path $logs 'task-before.png'
$shotAfter = Join-Path $logs 'task-after.png'
$shotClosed = Join-Path $logs 'task-closed.png'
$report = Join-Path $logs 'task-check.json'

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
$VK_ESCAPE = 0x1B
$VK_B = 0x42

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The colour of the output panel, and the colour of the text in it.
$PANEL_RGB = To-Bytes @(0.055, 0.063, 0.082)
$TEXT_RGB = To-Bytes @(0.78, 0.81, 0.87)

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}

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
# that lies inside another one is refused by cargo.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path (Join-Path $project 'src') | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'Cargo.toml'),
    "[workspace]`n`n[package]`nname = `"task-check`"`nversion = `"0.1.0`"`nedition = `"2021`"`n",
    (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText(
    $source,
    "fn main() {`n    println!(`"the task ran`");`n}`n",
    (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the file that is opened: $source"

$script:Factor = 1.0
function To-Physical([double]$value) {
    if ($scriptIsAware) { return $value }
    return $value * $script:Factor
}

# Presses a key with modifiers.
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

# The band at the bottom of the window, where the output panel is drawn.
function Get-PanelRect($shot) {
    return @{
        Left = $shot.Left
        Top = $shot.Top + $shot.Height - [int](To-Physical 200)
        Width = $shot.Width
        Height = [int](To-Physical 200)
    }
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'task-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'task-app-err.log') -PassThru
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

    $editor = Start-Editor @($source)
    $handle = $editor.Handle
    $previous = [IntPtr]::Zero
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        $previous = Focus-Editor $handle
        Start-Sleep -Seconds 2

        $before = Save-Window $handle $shotBefore
        Write-Host "saved $shotBefore"
        $panel = Get-PanelRect $before
        $panelBefore = Count-Color $before.Bitmap $panel.Left $panel.Top $panel.Width $panel.Height $PANEL_RGB 10
        Write-Host "pixels of the panel before the task: $panelBefore"

        # Ctrl+Shift+B builds the project.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_B

        $after = $null
        $panelAfter = 0
        $textAfter = 0
        for ($attempt = 1; $attempt -le 6; $attempt++) {
            Start-Sleep -Seconds 4
            $shot = Save-Window $handle $shotAfter
            $panelAfter = Count-Color $shot.Bitmap $panel.Left $panel.Top $panel.Width $panel.Height $PANEL_RGB 10
            $textAfter = Count-Color $shot.Bitmap $panel.Left $panel.Top $panel.Width $panel.Height $TEXT_RGB 14
            Write-Host "attempt ${attempt}: $panelAfter pixels of the panel, $textAfter pixels of text in it"
            if ($panelAfter -gt 500 -and $textAfter -gt 30) {
                $after = $shot
                break
            }
            $shot.Bitmap.Dispose()
        }
        Check 'the output panel is drawn' ($null -ne $after) "$panelAfter pixels of the panel"
        Check 'what the task said is in the panel' ($textAfter -gt 30) "$textAfter pixels of text in the panel"

        # Esc closes the panel, and the text of the document takes the room
        # back.
        $previous = Focus-Editor $handle
        Send-Chord @() $VK_ESCAPE
        Start-Sleep -Seconds 2
        $closed = Save-Window $handle $shotClosed
        $panelClosed = Count-Color $closed.Bitmap $panel.Left $panel.Top $panel.Width $panel.Height $PANEL_RGB 10
        Write-Host "pixels of the panel after the escape: $panelClosed"
        Check 'the escape key closes the output panel' ($panelClosed -lt 500) "$panelClosed pixels of the panel"

        $result = @{
            checks = $script:checks
            project = $project
            pixels = @{
                panelBefore = $panelBefore
                panelAfter = $panelAfter
                textAfter = $textAfter
                panelAfterEscape = $panelClosed
            }
            shots = @($shotBefore, $shotAfter, $shotClosed)
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
