# The window-level check of the many carets: a column selection picks the same
# columns of several lines, and the word cursor picks the next place of the
# same word.
#
# The project is a folder of its own, outside the workspace, so the check does
# not touch the code of the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+D takes the word under the caret: it is selected on the screen.
# 2. Ctrl+D again takes the next place of that word: it is selected too.
# 3. Ctrl+Shift+Alt+Down opens a box, and Ctrl+Shift+Alt+Right makes it
#    wider: the same column of two lines is selected.
#
# The selections are blocks of colour, so the colour of the code goes through
# the conversion that turns the colour of the code into the colour of the
# screen. The text is not counted: the words are drawn over the blocks.
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'multicursor-project'
$source = Join-Path $project 'main.rs'
$shotWord = Join-Path $logs 'multicursor-word.png'
$shotBoth = Join-Path $logs 'multicursor-both.png'
$shotBox = Join-Path $logs 'multicursor-box.png'
$report = Join-Path $logs 'multicursor-check.json'

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

# The keys the check presses.
$VK_CONTROL = 0x11
$VK_SHIFT = 0x10
$VK_MENU = 0x12
$VK_D = 0x44
$VK_DOWN = 0x28
$VK_RIGHT = 0x27
$VK_ESCAPE = 0x1B

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The colour of a block of colour on the screen. A block goes through the
# conversion that turns the colour of the code into the colour of the screen.
function To-Pixel([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round((To-Srgb $_) * 255) })
}

# The colour of the selected text: a block of colour.
$SELECTION_RGB = To-Pixel @(0.20, 0.33, 0.55)

# The places of the parts of the window, in the pixels of the window.
#
# The bar of the activities (48), the side bar (220), the gap (16) and the
# line numbers (56) are before the text; the strip of tabs (34) and the row
# that says where the file is (24) are above it.
$TEXT_LEFT = 340.0
$TEXT_TOP = 58.0
$LINE_HEIGHT = 22.0
$LINE_WIDTH = 400.0

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

# The files of the check: the word "alpha" is in the first line and in the
# third one, so the word cursor has somewhere to go.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'Cargo.toml'),
    "[workspace]`n",
    (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText(
    $source,
    "alpha one`nbeta two`nalpha three`n",
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
    Start-Sleep -Milliseconds 400
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

# The pixels of an area that are the colour of the selection.
#
# The colour is looked for with a little room around it: the words are drawn
# over the blocks, so not every pixel of a block has the colour of the block.
function Count-Selected($picture, [int]$left, [int]$top, [int]$width, [int]$height) {
    $count = 0
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $picture.Height) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - $SELECTION_RGB[0]) -le 12 -and
                [Math]::Abs($pixel.G - $SELECTION_RGB[1]) -le 12 -and
                [Math]::Abs($pixel.B - $SELECTION_RGB[2]) -le 12) {
                $count++
            }
        }
    }
    return $count
}

# The place of the first pixel of the selection in the area, if there is one.
function First-Selected($picture, [int]$left, [int]$top, [int]$width, [int]$height) {
    for ($y = $top; $y -lt $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $picture.Height) { continue }
        for ($x = $left; $x -lt $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - $SELECTION_RGB[0]) -le 12 -and
                [Math]::Abs($pixel.G - $SELECTION_RGB[1]) -le 12 -and
                [Math]::Abs($pixel.B - $SELECTION_RGB[2]) -le 12) {
                return @{ X = $x; Y = $y }
            }
        }
    }
    return $null
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'multicursor-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'multicursor-app-err.log') -PassThru
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

        # The word cursor takes the word under the caret.
        Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_D
        Start-Sleep -Seconds 1

        $word = Save-Window $handle $shotWord
        Write-Host "saved $shotWord"
        $first = Region $word $TEXT_LEFT $TEXT_TOP $LINE_WIDTH $LINE_HEIGHT
        $third = Region $word $TEXT_LEFT ($TEXT_TOP + 2 * $LINE_HEIGHT) $LINE_WIDTH $LINE_HEIGHT
        $onFirst = Count-Selected $word.Bitmap $first.Left $first.Top $first.Width $first.Height
        $onThird = Count-Selected $word.Bitmap $third.Left $third.Top $third.Width $third.Height
        Write-Host "the selection on the first line: $onFirst pixels, on the third line: $onThird pixels"
        Check 'Ctrl+D takes the word under the caret' ($onFirst -gt 10) `
            "$onFirst pixels of the selection on the first line"

        # The word cursor takes the next place of the same word.
        Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_D
        Start-Sleep -Seconds 1

        $both = Save-Window $handle $shotBoth
        Write-Host "saved $shotBoth"
        $first = Region $both $TEXT_LEFT $TEXT_TOP $LINE_WIDTH $LINE_HEIGHT
        $third = Region $both $TEXT_LEFT ($TEXT_TOP + 2 * $LINE_HEIGHT) $LINE_WIDTH $LINE_HEIGHT
        $onFirst = Count-Selected $both.Bitmap $first.Left $first.Top $first.Width $first.Height
        $onThird = Count-Selected $both.Bitmap $third.Left $third.Top $third.Width $third.Height
        Write-Host "the selection on the first line: $onFirst pixels, on the third line: $onThird pixels"
        Check 'Ctrl+D again takes the next place of the same word' `
            ($onFirst -gt 10 -and $onThird -gt 10) `
            "$onFirst pixels of the selection on the first line, $onThird on the third line"

        # The escape key takes the selection away.
        Focus-Editor $handle
        Send-Chord @() $VK_ESCAPE
        Start-Sleep -Seconds 1

        # The column selection: a box over the same column of two lines.
        Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT, $VK_MENU) $VK_DOWN
        Start-Sleep -Milliseconds 700
        Send-Chord @($VK_CONTROL, $VK_SHIFT, $VK_MENU) $VK_RIGHT
        Start-Sleep -Seconds 1

        $box = Save-Window $handle $shotBox
        Write-Host "saved $shotBox"
        $one = Region $box $TEXT_LEFT $TEXT_TOP $LINE_WIDTH $LINE_HEIGHT
        $two = Region $box $TEXT_LEFT ($TEXT_TOP + $LINE_HEIGHT) $LINE_WIDTH $LINE_HEIGHT
        $onOne = Count-Selected $box.Bitmap $one.Left $one.Top $one.Width $one.Height
        $onTwo = Count-Selected $box.Bitmap $two.Left $two.Top $two.Width $two.Height
        $whereOne = First-Selected $box.Bitmap $one.Left $one.Top $one.Width $one.Height
        $whereTwo = First-Selected $box.Bitmap $two.Left $two.Top $two.Width $two.Height
        Write-Host "the selection on the first line: $onOne pixels, on the second line: $onTwo pixels"
        Check 'the column selection reaches two lines' `
            ($onOne -gt 3 -and $onTwo -gt 3) `
            "$onOne pixels of the selection on the first line, $onTwo on the second line"
        if ($whereOne -and $whereTwo) {
            $same = [Math]::Abs($whereOne.X - $whereTwo.X) -le 2
            Check 'the box is on the same column of the two lines' $same `
                "the selection starts at x=$($whereOne.X) on the first line and x=$($whereTwo.X) on the second one"
        }
        else {
            Check 'the box is on the same column of the two lines' $false `
                'the selection was not found on the two lines'
        }

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
