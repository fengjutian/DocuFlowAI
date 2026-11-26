# The window-level check of the panel of the commands: Ctrl+Shift+P opens it,
# the commands of the editor are listed, writing narrows the list down, the
# escape key closes it, and the enter key runs the command that is picked.
#
# The project is a folder of its own, outside the workspace, so the check does
# not touch the code of the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+Shift+P opens the panel of the commands: the panel covers the whole
#    window.
# 2. The commands of the editor are listed.
# 3. Writing in the box narrows the list down to the command that is wanted.
# 4. The escape key closes the panel, and the document is back.
# 5. The enter key runs the command that is picked: the panel of the settings
#    is there, so the command ran.
# 6. The escape key closes the panel of the settings, and the document is back.
#
# The keys are injected into the system, the way a keyboard would, because the
# window reads the state of the modifiers from the keyboard itself.
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'palette-project'
$source = Join-Path $project 'notes.txt'
$shotOpen = Join-Path $logs 'command-palette-open.png'
$shotNarrow = Join-Path $logs 'command-palette-narrow.png'
$shotClosed = Join-Path $logs 'command-palette-closed.png'
$shotRan = Join-Path $logs 'command-palette-ran.png'
$report = Join-Path $logs 'command-palette-check.json'

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
$VK_RETURN = 0x0D
$VK_ESCAPE = 0x1B
$VK_P = 0x50

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

# The ground of the panel of the commands, the ground of the box in it, the
# line of colour on the top of it, the colour of the names of the commands,
# and the ground of the panel of the settings.
#
# The ground of a panel and the ground of the settings are blocks of colour.
# The names of the commands are text, and the text is drawn in the colour the
# code writes.
$PALETTE_GROUND_RGB = To-Pixel @(0.055, 0.063, 0.082)
$PALETTE_BOX_RGB = To-Pixel @(0.10, 0.11, 0.15)
$PALETTE_LINE_RGB = To-Pixel @(0.30, 0.58, 0.96)
$COMMAND_RGB = To-Bytes @(0.88, 0.90, 0.95)
$SETTINGS_GROUND_RGB = To-Pixel @(0.02, 0.03, 0.05)

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

# The editor is started from a folder of its own, outside the workspace.
#
# The workspace carries a low integrity label, so a program built there runs
# with fewer rights than the programs it starts. The copy runs with the
# rights a program normally has.
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null

# The file that is opened. The check only needs a document with text in it.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
[System.IO.File]::WriteAllText(
    $source,
    "the notes of the check`n`nthe second line of the notes`n",
    (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the file that is opened: $source"

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

# Presses a key that has no modifier.
function Send-Key([int]$virtualKey) {
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 40
}

# The key of one character.
function Get-VirtualKey([char]$character) {
    if ($character -eq ' ') { return 0x20 }
    if ($character -eq '/') { return 0xBF }
    if ($character -eq '.') { return 0xBE }
    if ($character -eq '-') { return 0xBD }
    $code = [int][char]$character
    if ($code -ge 97 -and $code -le 122) { return 0x41 + ($code - 97) }
    if ($code -ge 48 -and $code -le 57) { return 0x30 + ($code - 48) }
    throw "the check cannot press the key $character"
}

# Types a text.
function Send-Text([string]$text) {
    foreach ($character in $text.ToCharArray()) {
        Send-Key (Get-VirtualKey $character)
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

# Counts the pixels of one colour in a rectangle of the client area.
function Count-In($shot, [int]$left, [int]$top, [int]$width, [int]$height, [int[]]$rgb, [int]$tolerance) {
    return Count-Color $shot.Bitmap ($shot.Left + $left) ($shot.Top + $top) $width $height $rgb $tolerance
}

# Counts the bright pixels in a rectangle of the client area: the text of the
# document is much brighter than the ground it is drawn on.
function Count-Bright-In($shot, [int]$left, [int]$top, [int]$width, [int]$height) {
    $count = 0
    for ($y = $shot.Top + $top; $y -lt $shot.Top + $top + $height; $y++) {
        if ($y -lt 0 -or $y -ge $shot.Bitmap.Height) { continue }
        for ($x = $shot.Left + $left; $x -lt $shot.Left + $left + $width; $x++) {
            if ($x -lt 0 -or $x -ge $shot.Bitmap.Width) { continue }
            $pixel = $shot.Bitmap.GetPixel($x, $y)
            if ($pixel.R -ge 120 -and $pixel.G -ge 120) { $count++ }
        }
    }
    return $count
}

# The place of the panel of the commands, in the pixels of the window.
#
# The panel is in the middle of the window, and its width is fixed, so where
# it is depends on the size of the window. It is worked out from the size
# that was measured, the way the code works it out.
function Get-PaletteRect($shot) {
    $logicalWidth = $shot.Width / $script:Scale
    $width = [Math]::Min(640.0, [Math]::Max(200.0, $logicalWidth - 40.0))
    return @{
        Left = ($logicalWidth - $width) / 2.0
        Top = 32.0
        Width = $width
        # The list of the commands starts under the line the text is written in.
        ListTop = 32.0 + 8.0 + 36.0
        RowHeight = 24.0
        Rows = 10
    }
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'palette-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'palette-app-err.log') -PassThru
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

        # Ctrl+Shift+P opens the panel of the commands.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_P
        Start-Sleep -Seconds 2

        $openShot = Save-Window $handle $shotOpen
        Write-Host "saved $shotOpen"
        $palette = Get-PaletteRect $openShot
        $area = $openShot.Width * $openShot.Height
        $ground = Count-In $openShot 0 0 $openShot.Width $openShot.Height $PALETTE_GROUND_RGB 6
        $share = $ground / $area
        Write-Host "pixels of the ground of the panel: $ground of $area"
        Check 'Ctrl+Shift+P opens the panel of the commands' ($share -gt 0.30) ("{0:N2} of the window is the ground of the panel" -f $share)

        # The commands of the editor are listed.
        $listLeft = $palette.Left + 12.0
        $listWidth = $palette.Width - 130.0
        $listTop = $palette.ListTop
        $listHeight = $palette.RowHeight * $palette.Rows
        $listed = Count-In $openShot (To-Screen $listLeft) (To-Screen $listTop) (To-Screen $listWidth) (To-Screen $listHeight) $COMMAND_RGB 12
        Write-Host "pixels of the names of the commands: $listed"
        Check 'the commands of the editor are listed' ($listed -gt 100) "$listed pixels of the names of the commands"

        # The line of colour on the top of the panel is there.
        $line = Count-In $openShot (To-Screen $palette.Left) (To-Screen $palette.Top) (To-Screen $palette.Width) (To-Screen 2.0) $PALETTE_LINE_RGB 8
        Write-Host "pixels of the line on the top of the panel: $line"
        Check 'the panel has its line of colour' ($line -gt 200) "$line pixels of the line"

        # Writing in the box narrows the list down.
        $previous = Focus-Editor $handle
        Send-Text 'settings'
        Start-Sleep -Seconds 2

        $narrowShot = Save-Window $handle $shotNarrow
        Write-Host "saved $shotNarrow"
        $narrowed = Count-In $narrowShot (To-Screen $listLeft) (To-Screen $listTop) (To-Screen $listWidth) (To-Screen $listHeight) $COMMAND_RGB 12
        Write-Host "pixels of the names of the commands: $listed before, $narrowed after"
        Check 'writing narrows the list down' ($narrowed -lt ($listed / 3)) "the names of the commands went from $listed pixels to $narrowed pixels"

        # The escape key closes the panel, and the document is back.
        $previous = Focus-Editor $handle
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 2

        $closedShot = Save-Window $handle $shotClosed
        Write-Host "saved $shotClosed"
        $left = Count-In $closedShot 0 0 $closedShot.Width $closedShot.Height $PALETTE_GROUND_RGB 6
        $body = Count-Bright-In $closedShot (To-Screen 340) (To-Screen 58) (To-Screen 400) (To-Screen 200)
        Write-Host "pixels of the ground of the panel: $left, bright pixels of the document: $body"
        Check 'the escape key closes the panel' ($left -lt ($area / 10)) "$left pixels of the ground of the panel are left"
        Check 'the document is back' ($body -gt 20) "$body bright pixels of the document"

        # The enter key runs the command that is picked: the panel of the
        # settings is there, so the command ran.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_P
        Start-Sleep -Seconds 2
        $previous = Focus-Editor $handle
        Send-Text 'settings'
        Start-Sleep -Seconds 2
        $previous = Focus-Editor $handle
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 3

        $ranShot = Save-Window $handle $shotRan
        Write-Host "saved $shotRan"
        $settings = Count-In $ranShot 0 0 $ranShot.Width $ranShot.Height $SETTINGS_GROUND_RGB 6
        $share = $settings / ($ranShot.Width * $ranShot.Height)
        Write-Host "pixels of the ground of the settings: $settings"
        Check 'the enter key runs the command that is picked' ($share -gt 0.30) ("{0:N2} of the window is the ground of the settings" -f $share)

        # The escape key closes the panel of the settings.
        $previous = Focus-Editor $handle
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 2

        $afterShot = Save-Window $handle (Join-Path $logs 'command-palette-after.png')
        Write-Host "saved $(Join-Path $logs 'command-palette-after.png')"
        $left = Count-In $afterShot 0 0 $afterShot.Width $afterShot.Height $SETTINGS_GROUND_RGB 6
        $body = Count-Bright-In $afterShot (To-Screen 340) (To-Screen 58) (To-Screen 400) (To-Screen 200)
        Write-Host "pixels of the ground of the settings: $left, bright pixels of the document: $body"
        Check 'the escape key closes the panel of the settings' ($left -lt ($area / 10)) "$left pixels of the ground of the settings are left"
        Check 'the document is back after the command ran' ($body -gt 20) "$body bright pixels of the document"

        $result = @{
            checks = $script:checks
            project = $project
            pixels = @{
                ground = $ground
                listed = $listed
                line = $line
                narrowed = $narrowed
                settings = $settings
            }
            shots = @($shotOpen, $shotNarrow, $shotClosed, $shotRan)
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
