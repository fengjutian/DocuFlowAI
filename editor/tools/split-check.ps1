# The window-level check of the split: the window shows two files at the same
# time, side by side, and every column has its own tabs, its own text and its
# own keyboard.
#
# The project is a folder of its own, outside the workspace, so the check does
# not touch the code of the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+\ opens a second column: its strip of tabs is on the screen, to the
#    right of the first column.
# 2. The second column has its own text: it has its own line numbers.
# 3. The right of the status bar says which column has the keyboard.
# 4. A click in a column gives the keyboard to that column.
# 5. Closing the last tab of a column takes the column away.
#
# The places of the columns are worked out the same way the editor works them
# out: the bar of the activities (48), the side bar (220), the gap (16), the
# line between the columns (1), the line numbers (56) and the panel of the
# agent (34 percent of the width, between 280 and 520).
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'split-project'
$source = Join-Path $project 'main.rs'
$shotOne = Join-Path $logs 'split-one.png'
$shotTwo = Join-Path $logs 'split-two.png'
$shotClick = Join-Path $logs 'split-click.png'
$shotClosed = Join-Path $logs 'split-closed.png'
$report = Join-Path $logs 'split-check.json'

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
  [DllImport("user32.dll")] public static extern void mouse_event(uint flags, int dx, int dy, uint data, UIntPtr extra);
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
$VK_CONTROL = 0x11
$VK_W = 0x57
$VK_BACKSLASH = 0xDC

# The places of the parts of the window, in the pixels of the window.
#
# The bar of the activities is on the left (48), the side bar (220) and the
# gap (16) are before the text, and the line numbers (56) are between the
# gap and the text.
$ACTIVITY_BAR = 48.0
$SIDEBAR = 220.0
$GAP = 16.0
$GUTTER = 56.0
$SEPARATOR = 1.0

# The panel of the agent: a part of the width of the window, but never less
# than 280 and never more than 520.
$AGENT_SHARE = 0.34
$AGENT_MIN = 280.0
$AGENT_MAX = 520.0

# The height of the strip of tabs and the row that says where the file is.
$TAB_BAR = 34.0
$BREADCRUMBS = 24.0

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}

# The check drives the editor that is on disk, so that one has to be the one
# the sources describe.
$editorCrates = @('app', 'platform', 'render', 'text', 'syntax', 'lsp', 'dap', 'ui', 'workspace', 'workbench', 'editor-core', 'agent-acp', 'agent-ui', 'diff')
$newest = $editorCrates | ForEach-Object {
    $crate = Join-Path $root "crates\$_"
    if (Test-Path $crate) {
        Get-ChildItem -Path (Join-Path $crate 'src') -Recurse -Include *.rs -ErrorAction SilentlyContinue
    }
} | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($newest -and (Get-Item $exe).LastWriteTime -lt $newest.LastWriteTime) {
    throw "the editor is older than its sources; run 'cargo build -p codeweaver-editor' first"
}

# The editor runs from a folder of its own, outside the workspace.
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null

# The files of the check: a project of its own with a file of many short
# lines, so that the text of the left column does not reach the column on its
# right and the check can tell the two apart.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'Cargo.toml'),
    "[workspace]`n",
    (New-Object System.Text.UTF8Encoding($false)))
$lines = @()
for ($i = 1; $i -le 40; $i++) {
    $lines += ("line {0}" -f $i)
}
[System.IO.File]::WriteAllText(
    $source,
    (($lines -join "`n") + "`n"),
    (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the folder of the files: $project"

$script:Factor = 1.0
$script:Scale = 1.0

# The scale of the window: the editor draws in the pixels of the screen, and
# its own places are given in the pixels of the window.
function To-Screen([double]$logical) {
    return [int]($logical * $script:Scale)
}

# The pixels of the screen, when the script is not aware of the scale.
function To-Physical([double]$value) {
    if ($scriptIsAware) { return $value }
    return $value * $script:Factor
}

# Presses a key with modifiers.
#
# The keys are injected into the system, the way a keyboard would, because the
# window reads the state of the modifiers from the keyboard itself.
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
        Start-Sleep -Milliseconds 80
    }
}

function Save-Window([IntPtr]$handle, [string]$path) {
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
    $ok = [Win32]::PrintWindow($handle, $hdc, 2)
    $graphics.ReleaseHdc($hdc)
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    return @{
        Bitmap = $bitmap
        Ok = $ok
        Left = $origin.X - $window.Left
        Top = $origin.Y - $window.Top
        ScreenLeft = $origin.X
        ScreenTop = $origin.Y
        Width = [int](To-Physical $client.Right)
        Height = [int](To-Physical $client.Bottom)
    }
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

# The ground the text is drawn on: the most common colour of the area.
#
# The colour is not written down here on purpose: the editor may draw its
# ground in another colour tomorrow, and the check is about the columns, not
# about the colour of the ground.
function Get-Ground($picture, [int]$left, [int]$top, [int]$width, [int]$height) {
    $counts = @{}
    for ($y = $top; $y -lt $top + $height; $y += 2) {
        for ($x = $left; $x -lt $left + $width; $x += 2) {
            $pixel = $picture.GetPixel($x, $y)
            $key = "$($pixel.R),$($pixel.G),$($pixel.B)"
            if ($counts.ContainsKey($key)) { $counts[$key] += 1 } else { $counts[$key] = 1 }
        }
    }
    $best = $null
    $most = 0
    foreach ($key in $counts.Keys) {
        if ($counts[$key] -gt $most) { $most = $counts[$key]; $best = $key }
    }
    if (-not $best) { return @(0, 0, 0) }
    $parts = $best.Split(',')
    return @([int]$parts[0], [int]$parts[1], [int]$parts[2])
}

# The pixels of an area that are not the ground of the area: the ink of the
# text, of the numbers and of the tabs.
function Count-Ink($picture, [int]$left, [int]$top, [int]$width, [int]$height) {
    $ground = Get-Ground $picture $left $top $width $height
    $count = 0
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $picture.Height) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - $ground[0]) -gt 6 -or
                [Math]::Abs($pixel.G - $ground[1]) -gt 6 -or
                [Math]::Abs($pixel.B - $ground[2]) -gt 6) {
                $count++
            }
        }
    }
    return $count
}

# The place of an area of the window in the picture: the pixels of the window
# become the pixels of the picture.
function Region($shot, [double]$x, [double]$y, [double]$width, [double]$height) {
    return @{
        Left = [int]($shot.Left + (To-Screen $x))
        Top = [int]($shot.Top + (To-Screen $y))
        Width = [int](To-Screen $width)
        Height = [int](To-Screen $height)
    }
}

# Clicks in the window at a place of the client area, given in the pixels of
# the window.
function Click-At($shot, [double]$x, [double]$y) {
    $screenX = $shot.ScreenLeft + (To-Screen $x)
    $screenY = $shot.ScreenTop + (To-Screen $y)
    [Win32]::SetCursorPos([int]$screenX, [int]$screenY) | Out-Null
    Start-Sleep -Milliseconds 500
    [Win32]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 150
    [Win32]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 500
    # The real cursor must not stay over the editor: its own messages arrive
    # after the posted ones, and the editor would follow the real cursor
    # instead of the message.
    [Win32]::SetCursorPos(10, 10) | Out-Null
    Start-Sleep -Milliseconds 300
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'split-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'split-app-err.log') -PassThru
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
    Start-Sleep -Seconds 3
    $script:Factor = [Win32]::GetDpiForWindow($handle) / 96.0
    $script:Scale = $script:Factor
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
    return @{ Process = $process; Handle = $handle }
}

# Gives the editor the keyboard focus.
function Focus-Editor([IntPtr]$handle) {
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
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        Focus-Editor $handle

        # The size of the window in the pixels of the window: the editor lays
        # its parts out in these pixels.
        $client = New-Object Win32+RECT
        [Win32]::GetClientRect($handle, [ref]$client) | Out-Null
        $width = [Math]::Round($client.Right / $script:Factor)
        $height = [Math]::Round($client.Bottom / $script:Factor)
        Write-Host "the window is $width by $height pixels"

        # The places of the two columns, worked out the way the editor works
        # them out.
        $agent = [Math]::Min([Math]::Max($width * $AGENT_SHARE, $AGENT_MIN), $AGENT_MAX)
        $textRight = $width - $agent - $GAP
        $edge = $ACTIVITY_BAR + $SIDEBAR
        $columnWidth = ($textRight - $edge - $SEPARATOR) / 2.0
        $rightColumn = $edge + $columnWidth + $SEPARATOR
        $leftBody = $edge + $GAP + $GUTTER
        $rightGutter = $rightColumn + $GAP
        $textTop = $TAB_BAR + $BREADCRUMBS
        Write-Host "the second column starts at $rightColumn, its numbers at $rightGutter"

        # The strip of tabs of the second column.
        $one = Save-Window $handle $shotOne
        Write-Host "saved $shotOne"
        $tabsOne = Region $one $rightColumn 2 140 30
        $tabsBefore = Count-Ink $one.Bitmap $tabsOne.Left $tabsOne.Top $tabsOne.Width $tabsOne.Height

        # The numbers of the lines of the second column.
        $numbersOne = Region $one $rightGutter ($textTop + 2) 56 50
        $numbersBefore = Count-Ink $one.Bitmap $numbersOne.Left $numbersOne.Top $numbersOne.Width $numbersOne.Height

        # The right of the status bar, where the column that has the keyboard
        # is named.
        $statusOne = Region $one ($width - 56) ($height - 20) 46 16
        $statusBefore = Count-Ink $one.Bitmap $statusOne.Left $statusOne.Top $statusOne.Width $statusOne.Height

        # Ctrl+\ opens the second column.
        Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_BACKSLASH
        Start-Sleep -Seconds 2

        $two = Save-Window $handle $shotTwo
        Write-Host "saved $shotTwo"
        $tabsTwo = Region $two $rightColumn 2 140 30
        $tabsAfter = Count-Ink $two.Bitmap $tabsTwo.Left $tabsTwo.Top $tabsTwo.Width $tabsTwo.Height
        $numbersTwo = Region $two $rightGutter ($textTop + 2) 56 50
        $numbersAfter = Count-Ink $two.Bitmap $numbersTwo.Left $numbersTwo.Top $numbersTwo.Width $numbersTwo.Height
        $statusTwo = Region $two ($width - 56) ($height - 20) 46 16
        $statusAfter = Count-Ink $two.Bitmap $statusTwo.Left $statusTwo.Top $statusTwo.Width $statusTwo.Height
        Write-Host "the strip of the second column: $tabsBefore pixels before the split, $tabsAfter after"
        Write-Host "the numbers of the second column: $numbersBefore pixels before the split, $numbersAfter after"
        Write-Host "the right of the status bar: $statusBefore pixels before the split, $statusAfter after"

        Check 'the window shows a second column beside the first one' `
            ($tabsAfter -gt 20 -and $tabsBefore -lt 20) `
            "$tabsBefore pixels of ink in the strip of the second column before the split, $tabsAfter after"

        Check 'the second column has its own text and its own numbers' `
            ($numbersAfter -gt 3 -and $numbersBefore -lt 3) `
            "$numbersBefore pixels of ink in the numbers of the second column before the split, $numbersAfter after"

        Check 'the right of the status bar names the column that has the keyboard' `
            ($statusAfter -gt $statusBefore) `
            "$statusBefore pixels of ink in the right of the status bar before the split, $statusAfter after"

        # A click in the first column gives the keyboard to that column: the
        # right of the status bar says the other number.
        Click-At $two $leftBody 20
        Focus-Editor $handle
        Start-Sleep -Seconds 1
        $clicked = Save-Window $handle $shotClick
        Write-Host "saved $shotClick"
        $statusClick = Region $clicked ($width - 56) ($height - 20) 46 16
        $said = Count-Difference $two.Bitmap $clicked.Bitmap $statusClick.Left $statusClick.Top $statusClick.Width $statusClick.Height
        Write-Host "$said pixels of the right of the status bar changed after the click"
        Check 'a click in a column gives the keyboard to that column' `
            ($said -gt 0) `
            "$said pixels of the right of the status bar changed"

        # The last tab of the second column is closed: the column goes away.
        Click-At $clicked $rightColumn 20
        Focus-Editor $handle
        Start-Sleep -Seconds 1
        Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_W
        Start-Sleep -Seconds 2

        $closed = Save-Window $handle $shotClosed
        Write-Host "saved $shotClosed"
        $tabsClosed = Region $closed $rightColumn 2 140 30
        $tabsGone = Count-Ink $closed.Bitmap $tabsClosed.Left $tabsClosed.Top $tabsClosed.Width $tabsClosed.Height
        $numbersClosed = Region $closed $rightGutter ($textTop + 2) 56 50
        $numbersGone = Count-Ink $closed.Bitmap $numbersClosed.Left $numbersClosed.Top $numbersClosed.Width $numbersClosed.Height
        Write-Host "the strip of the second column: $tabsGone pixels after it was closed"
        Check 'the column goes away when its last tab is closed' `
            ($tabsGone -lt 20 -and $numbersGone -lt 3) `
            "$tabsGone pixels of ink in the strip of the second column and $numbersGone in its numbers after it was closed"

        $passed = @($script:checks | Where-Object { $_.ok }).Count
        $failed = @($script:checks | Where-Object { -not $_.ok }).Count
        @{ checks = $script:checks; passed = $passed; failed = $failed } |
            ConvertTo-Json -Depth 4 | Set-Content -Path $report -Encoding UTF8
        Write-Host "the report is in $report"
        Write-Host "passed: $passed, failed: $failed"
        if ($failed -gt 0) { exit 1 }
    }
    finally {
        Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    }
}
catch {
    Write-Host "the check failed: $_"
    exit 1
}
