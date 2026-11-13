# The window-level check of the settings panel: the sizes at which the
# highlighter gives up, and the fact that changing them takes effect at once.
#
# The check opens a file that is bigger than the default limit, so the editor
# starts by refusing to colour it. The panel is then opened, the limit is
# raised by one step, and the file is coloured again.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+, opens the panel: the window is dimmed under it, and the panel
#    itself is drawn with its settings on it.
# 2. Raising the limit by one step brings the colours back: the text of the
#    document has coloured pixels in it that it did not have before.
# 3. Enter closes the panel: the window is not dimmed any more.
#
# The keys are injected into the system, the way a keyboard would, because the
# window reads the state of the modifiers from the keyboard itself: a message
# that is only posted to the window does not change that state.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$work = Join-Path $logs 'settings'
$big = Join-Path $work 'big.rs'
$shotBefore = Join-Path $logs 'settings-before.png'
$shotPanel = Join-Path $logs 'settings-panel.png'
$shotAfter = Join-Path $logs 'settings-after.png'
$report = Join-Path $logs 'settings-check.json'

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
$VK_CONTROL = 0x11
$VK_COMMA = 0xBC
$VK_RIGHT = 0x27
$VK_RETURN = 0x0D

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
# a program normally has, and the check is about the editor, not about the
# label of the folder it was built in.
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null

# A file that is bigger than the default limit of one megabyte, and that has
# fewer lines than the default limit of fifty thousand.
$lines = New-Object System.Collections.Generic.List[string]
$bytes = 0
$index = 0
while ($bytes -lt 1500000) {
    $line = "fn f$index() { let x = $index; let y = $index * 2; }`n"
    $lines.Add($line)
    $bytes += $line.Length
    $index++
}
[System.IO.File]::WriteAllText($big, ($lines -join ''), (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the big file is $bytes bytes and $($lines.Count) lines"

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
        Start-Sleep -Milliseconds 40
    }
}

# Presses a key that has no modifier.
function Send-Key([int]$virtualKey) {
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
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

# Counts the bright pixels in a rectangle: the text of the editor is much
# brighter than the background it is drawn on.
function Count-Bright($picture, [int]$left, [int]$top, [int]$width, [int]$height) {
    $count = 0
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $picture.Height) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            if ($pixel.R -ge 120 -and $pixel.G -ge 120) { $count++ }
        }
    }
    return $count
}

# Counts the coloured pixels in a rectangle: a pixel whose channels do not
# agree is a pixel of coloured text, and coloured text is syntax highlighting.
function Count-Colored($picture, [int]$left, [int]$top, [int]$width, [int]$height) {
    $count = 0
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $picture.Height) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            $highest = [Math]::Max($pixel.R, [Math]::Max($pixel.G, $pixel.B))
            $lowest = [Math]::Min($pixel.R, [Math]::Min($pixel.G, $pixel.B))
            if (($highest - $lowest) -ge 30) { $count++ }
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

# The part of the document that is above the settings panel.
#
# The panel is a box in the middle of the window, so the text above it is
# the text of the document alone.
function Get-AboveRect($shot, $panel) {
    $body = Get-BodyRect $shot
    return @{
        Left = $body.Left
        Top = $body.Top
        Width = [int](To-Physical 100)
        Height = [Math]::Max($panel.Top - $body.Top - [int](To-Physical 10), 1)
    }
}

# The part of the document that holds nothing but the text.
#
# The blue bar on the left of the body and the caret in the first line are
# not text, and both of them have a colour of their own.
function Get-TextRect($shot) {
    $body = Get-BodyRect $shot
    return @{
        Left = $body.Left + [int](To-Physical 24)
        Top = $body.Top + [int](To-Physical 30)
        Width = [Math]::Max($body.Width - [int](To-Physical 40), 1)
        Height = [Math]::Max($body.Height - [int](To-Physical 40), 1)
    }
}

# Finds the settings panel in the picture, by the colour of its background.
#
# The panel is a box in the middle of the window. Where exactly it is depends
# on the size of the window, so it is looked for in the picture instead of
# being worked out from it.
function Find-SettingsBox($picture) {
    $minX = 9999
    $maxX = -1
    $minY = 9999
    $maxY = -1
    $from = [int]($picture.Height * 0.15)
    $to = [int]($picture.Height * 0.85)
    for ($y = $from; $y -lt $to; $y++) {
        for ($x = $from; $x -lt $to; $x++) {
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - 101) -le 4 -and
                [Math]::Abs($pixel.G - 108) -le 4 -and
                [Math]::Abs($pixel.B - 124) -le 4) {
                if ($x -lt $minX) { $minX = $x }
                if ($x -gt $maxX) { $maxX = $x }
                if ($y -lt $minY) { $minY = $y }
                if ($y -gt $maxY) { $maxY = $y }
            }
        }
    }
    if ($maxX -lt 0) { return $null }
    return @{
        Left = $minX
        Top = $minY
        Width = ($maxX - $minX + 1)
        Height = ($maxY - $minY + 1)
    }
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'settings-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'settings-app-err.log') -PassThru
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
#
# Injected keys go to the window that has the focus, so the editor has to
# have it while the check drives it. The window that had the focus is given
# back when the check is over.
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

    $editor = Start-Editor @($big)
    $handle = $editor.Handle
    $previous = [IntPtr]::Zero
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        $previous = Focus-Editor $handle

        # Before the panel is open, the file is too big to be coloured: the
        # text of the document is there, but none of it has a colour.
        $before = Save-Window $handle $shotBefore
        Write-Host "saved $shotBefore"
        $text = Get-TextRect $before
        $coloredBefore = Count-Colored $before.Bitmap $text.Left $text.Top $text.Width $text.Height
        Write-Host "coloured pixels in the text: $coloredBefore"
        Check 'the file over the limit is not coloured' ($coloredBefore -le 20) "$coloredBefore coloured pixels before the limit is raised"

        # Ctrl+, opens the settings panel: the window is dimmed under it and
        # the panel is drawn with its settings on it.
        Send-Chord @($VK_CONTROL) $VK_COMMA
        Start-Sleep -Seconds 2
        $panelShot = Save-Window $handle $shotPanel
        Write-Host "saved $shotPanel"
        $panel = Find-SettingsBox $panelShot.Bitmap
        $brightBefore = 0
        $brightDimmed = 0
        if (-not $panel) {
            Check 'the settings panel is drawn' $false 'the panel is not in the picture'
        }
        else {
            $brightPanel = Count-Bright $panelShot.Bitmap $panel.Left $panel.Top $panel.Width $panel.Height
            Write-Host "the panel is at $($panel.Left),$($panel.Top), and holds $brightPanel bright pixels"
            Check 'the settings panel is drawn' ($brightPanel -ge 20) "$brightPanel bright pixels in the panel"

            $aboveBefore = Get-AboveRect $before $panel
            $brightBefore = Count-Bright $before.Bitmap $aboveBefore.Left $aboveBefore.Top $aboveBefore.Width $aboveBefore.Height
            $abovePanel = Get-AboveRect $panelShot $panel
            $brightDimmed = Count-Bright $panelShot.Bitmap $abovePanel.Left $abovePanel.Top $abovePanel.Width $abovePanel.Height
            Write-Host "bright pixels above the panel: $brightBefore before, $brightDimmed under the panel"
            Check 'the panel is drawn over the window' ($brightDimmed -le 20) "the text above the panel went from $brightBefore bright pixels to $brightDimmed"
        }

        # The limit is raised by one step: 1 MB becomes 2 MB, and the file of
        # one and a half megabytes is small enough to be coloured again.
        Send-Key $VK_RIGHT
        Start-Sleep -Seconds 3
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 2

        $after = Save-Window $handle $shotAfter
        Write-Host "saved $shotAfter"
        $textAfter = Get-TextRect $after
        $coloredAfter = Count-Colored $after.Bitmap $textAfter.Left $textAfter.Top $textAfter.Width $textAfter.Height
        Write-Host "coloured pixels in the text: $coloredAfter"
        Check 'raising the limit brings the colours back' ($coloredAfter -gt ($coloredBefore + 50)) "coloured pixels went from $coloredBefore to $coloredAfter"
        if ($panel) {
            $aboveAfter = Get-AboveRect $after $panel
            $brightAfter = Count-Bright $after.Bitmap $aboveAfter.Left $aboveAfter.Top $aboveAfter.Width $aboveAfter.Height
            Check 'the panel is closed again' ($brightAfter -gt 20) "the text above the panel came back, $brightAfter bright pixels"
        }

        $result = @{
            checks = $script:checks
            bigFile = @{ path = $big; bytes = $bytes; lines = $lines.Count }
            pixels = @{
                brightBefore = $brightBefore
                coloredBefore = $coloredBefore
                brightDimmed = $brightDimmed
                brightPanel = $brightPanel
                brightAfter = $brightAfter
                coloredAfter = $coloredAfter
            }
            shots = @($shotBefore, $shotPanel, $shotAfter)
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
