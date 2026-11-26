# The window-level check of the size of the text: the text can be made bigger
# and smaller, the numbers of the lines stay on their lines, and the size is
# remembered when the editor starts again.
#
# The project is a folder of its own, outside the workspace, so the check does
# not touch the code of the editor. The settings of the editor are written in
# a folder of the check as well: the size of the text of the person who runs
# the check is none of the check's business.
#
# What the check proves, in the window and not in a unit test:
#
# 1. The rows of the text are further apart when the text is bigger.
# 2. Fewer lines fit on the screen when the text is bigger.
# 3. The numbers of the lines stay on their lines.
# 4. The size is written into the file of the settings.
# 5. The size is the same when the editor starts again.
# 6. The right of the status bar says the size.
#
# The size of the text is measured without counting colours of their own: the
# ground of the text is the most common colour of the area, and a row that has
# anything else on it is a row of text.
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'zoom-project'
$source = Join-Path $project 'main.rs'
$shotSmall = Join-Path $logs 'zoom-small.png'
$shotBig = Join-Path $logs 'zoom-big.png'
$shotAgain = Join-Path $logs 'zoom-again.png'
$report = Join-Path $logs 'zoom-check.json'

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
$VK_EQUALS = 0xBB

# The places of the parts of the window, in the pixels of the window.
#
# The line numbers are on the left of the text: the bar of activities (48), the
# side bar (220), the gap (16) and the numbers (56). The text starts under the
# strip of tabs (34) and the row that says where the file is (24).
$GUTTER_LEFT = 284.0
$GUTTER_WIDTH = 56.0
$TEXT_LEFT = 340.0
$TEXT_TOP = 58.0
$TEXT_WIDTH = 300.0
$TEXT_HEIGHT = 420.0

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

# The settings of the editor are written in a folder of the check, so that the
# size of the text of the person who runs the check is left alone.
$appData = Join-Path $run 'appdata'
$settingsFolder = Join-Path $appData 'CodeWeaver'
$settingsFile = Join-Path $settingsFolder 'settings.json'
if (Test-Path $settingsFolder) { Remove-Item -Recurse -Force $settingsFolder }
New-Item -ItemType Directory -Force -Path $settingsFolder | Out-Null
$oldAppData = $env:APPDATA
$env:APPDATA = $appData
Write-Output "the settings of the editor are in $settingsFile"

# The files of the check: a project of its own with a file of many lines, so
# that the screen is full of text.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'Cargo.toml'),
    "[workspace]`n",
    (New-Object System.Text.UTF8Encoding($false)))
$lines = @()
for ($i = 1; $i -le 40; $i++) {
    $lines += ("the line of the document number {0}" -f $i)
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
# ground in another colour tomorrow, and the check is about the size of the
# text, not about the colour of the ground.
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

# The rows of the text: every band of rows that has something on it that is
# not the ground.
function Get-Bands($picture, [int]$left, [int]$top, [int]$width, [int]$height, [int[]]$ground) {
    $bands = @()
    $open = $null
    for ($y = $top; $y -lt $top + $height; $y++) {
        $ink = 0
        for ($x = $left; $x -lt $left + $width; $x++) {
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - $ground[0]) -gt 6 -or
                [Math]::Abs($pixel.G - $ground[1]) -gt 6 -or
                [Math]::Abs($pixel.B - $ground[2]) -gt 6) {
                $ink++
                if ($ink -gt 3) { break }
            }
        }
        if ($ink -gt 3) {
            if ($null -eq $open) { $open = @{ Top = $y; Bottom = $y } }
            else { $open.Bottom = $y }
        }
        elseif ($null -ne $open) {
            $bands += $open
            $open = $null
        }
    }
    if ($null -ne $open) { $bands += $open }
    return $bands
}

# The rows of the text and the rows of the numbers, in one picture.
function Measure-Text($shot) {
    $textLeft = $shot.Left + (To-Screen $TEXT_LEFT)
    $textTop = $shot.Top + (To-Screen $TEXT_TOP)
    $textWidth = To-Screen $TEXT_WIDTH
    $textHeight = To-Screen $TEXT_HEIGHT
    $ground = Get-Ground $shot.Bitmap $textLeft $textTop $textWidth $textHeight

    $gutterLeft = $shot.Left + (To-Screen $GUTTER_LEFT)
    $gutterWidth = To-Screen $GUTTER_WIDTH

    return @{
        Ground = $ground
        Text = @(Get-Bands $shot.Bitmap $textLeft $textTop $textWidth $textHeight $ground)
        Gutter = @(Get-Bands $shot.Bitmap $gutterLeft $textTop $gutterWidth $textHeight $ground)
    }
}

# The distance between the tops of the first two rows of the text.
function Get-Row-Distance($bands) {
    if ($bands.Count -lt 2) { return 0 }
    return $bands[1].Top - $bands[0].Top
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'zoom-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'zoom-app-err.log') -PassThru
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

        $small = Save-Window $handle $shotSmall
        Write-Host "saved $shotSmall"
        $smallMeasure = Measure-Text $small
        $smallDistance = Get-Row-Distance $smallMeasure.Text
        $smallRows = $smallMeasure.Text.Count
        Write-Host "the rows of the text are $smallDistance apart, $smallRows rows on the screen"
        if ($smallRows -lt 3) {
            throw "the text of the document is not on the screen ($smallRows rows), so the check has nothing to measure"
        }

        # Ctrl+= makes the text bigger, three times: 130 percent.
        for ($i = 0; $i -lt 3; $i++) {
            Focus-Editor $handle
            Send-Chord @($VK_CONTROL) $VK_EQUALS
            Start-Sleep -Milliseconds 700
        }
        Start-Sleep -Seconds 2

        $big = Save-Window $handle $shotBig
        Write-Host "saved $shotBig"
        $bigMeasure = Measure-Text $big
        $bigDistance = Get-Row-Distance $bigMeasure.Text
        $bigRows = $bigMeasure.Text.Count
        Write-Host "the rows of the text are $bigDistance apart, $bigRows rows on the screen"

        Check 'the rows of the text are further apart when the text is bigger' `
            ($bigDistance -gt ($smallDistance * 1.15)) `
            "$smallDistance pixels apart at 100 percent, $bigDistance pixels apart at 130 percent"

        Check 'fewer lines fit on the screen when the text is bigger' `
            ($bigRows -lt $smallRows) `
            "$smallRows rows at 100 percent, $bigRows rows at 130 percent"

        # The numbers of the lines are on their lines: the second number is on
        # the second row of the text.
        $gutter = $bigMeasure.Gutter
        $text = $bigMeasure.Text
        $aligned = $false
        if ($gutter.Count -ge 2 -and $text.Count -ge 2) {
            $aligned = ($gutter[1].Top -le $text[1].Bottom) -and ($text[1].Top -le $gutter[1].Bottom)
        }
        Check 'the numbers of the lines stay on their lines' $aligned `
            "the second number is on the second row of the text"

        # The size is remembered in the file of the settings.
        $written = ''
        if (Test-Path $settingsFile) {
            $written = [System.IO.File]::ReadAllText($settingsFile)
        }
        Write-Host "the file of the settings: $written"
        Check 'the size is written into the file of the settings' `
            ($written -match '"fontSize"\s*:\s*130') `
            $written.Trim()

        # The right of the status bar says the size.
        $statusLeft = $big.Left + $big.Width - (To-Screen 260)
        $statusTop = $big.Top + $big.Height - (To-Screen 22)
        $statusWidth = To-Screen 252
        $statusHeight = To-Screen 20
        $said = Count-Difference $small.Bitmap $big.Bitmap $statusLeft $statusTop $statusWidth $statusHeight
        Write-Host "$said pixels of the right of the status bar changed"
        Check 'the right of the status bar says the size' ($said -gt 0) `
            "$said pixels of the right of the status bar changed"

        # The size is the same when the editor starts again.
        Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Seconds 1
        $editor = Start-Editor @($source)
        $handle = $editor.Handle
        Focus-Editor $handle
        $again = Save-Window $handle $shotAgain
        Write-Host "saved $shotAgain"
        $againMeasure = Measure-Text $again
        $againDistance = Get-Row-Distance $againMeasure.Text
        Write-Host "the rows of the text are $againDistance apart after the restart"
        Check 'the size is the same when the editor starts again' `
            ([Math]::Abs($againDistance - $bigDistance) -le 3) `
            "$bigDistance pixels apart at the end of the run, $againDistance pixels apart after the restart"

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
finally {
    $env:APPDATA = $oldAppData
}
