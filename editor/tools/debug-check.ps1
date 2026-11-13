# The window-level check of the debugger: the editor puts a breakpoint on the
# line of the caret, starts a debug session with the adapter, and shows the
# variables of the frame the program is stopped in.
#
# The adapter is the fake one of the repository: it is a program that answers
# the handshake, keeps the breakpoints it is told about, and stops at the
# first one. It is started outside the workspace as well, because the
# workspace carries a low integrity label and a low integrity process cannot
# start the git of the machine. The adapter is started by the editor, and
# the editor is started from the copy outside the workspace.
#
# What the check proves, in the window and not in a unit test:
#
# 1. F9 puts a breakpoint on the line of the caret, and the breakpoint is
#    drawn in the margin of the text.
# 2. Ctrl+F9 gives the breakpoint a condition, and a breakpoint with a
#    condition is drawn in another colour.
# 3. A line that is put above the breakpoint moves the breakpoint with it: the
#    breakpoint follows the text it is on, and the undo takes it back.
# 4. F5 starts the debug session, and the panel of the debugger is drawn at
#    the bottom of the window.
# 5. The variables of the frame the program is stopped in are in the panel:
#    the name, the type and the value of every variable.
# 6. F10 walks one line, and the program stops again with its variables.
# 7. Ctrl+Shift+E asks for an expression, and what the adapter says about
#    it is in the panel.
# 8. The stack of the program is in the panel, and a click on a frame of it
#    picks that frame: the mark of the frame that is picked moves to the
#    frame that was clicked on.
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
$adapter = Join-Path $root 'target\debug\fake-dap-adapter.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'debug-project'
$source = Join-Path $project 'main.rs'
$shotBreakpoint = Join-Path $logs 'debug-breakpoint.png'
$shotConditional = Join-Path $logs 'debug-conditional.png'
$shotMoved = Join-Path $logs 'debug-moved.png'
$shotUndone = Join-Path $logs 'debug-undone.png'
$shotStopped = Join-Path $logs 'debug-stopped.png'
$shotPicked = Join-Path $logs 'debug-picked.png'
$shotStepped = Join-Path $logs 'debug-stepped.png'
$shotEvaluated = Join-Path $logs 'debug-evaluated.png'
$report = Join-Path $logs 'debug-check.json'

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
$VK_SHIFT = 0x10
$VK_CONTROL = 0x11
$VK_ESCAPE = 0x1B
$VK_RETURN = 0x0D
$VK_Z = 0x5A
$VK_F5 = 0x74
$VK_F9 = 0x78
$VK_F10 = 0x79
$VK_E = 0x45

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The ground of the panel of the debugger, the colour of the breakpoint in
# the margin, the colour of a breakpoint with a condition, the colour of the
# text of a variable, the colour of what the adapter said, the colour of the
# text of a frame of the stack, and the colour of the mark of the frame that
# is picked.
$GROUND_RGB = To-Bytes @(0.055, 0.063, 0.082)
$BREAKPOINT_RGB = To-Bytes @(0.90, 0.35, 0.35)
$CONDITIONAL_RGB = To-Bytes @(0.95, 0.68, 0.25)
$VARIABLE_RGB = To-Bytes @(0.82, 0.86, 0.92)
$OUTPUT_RGB = To-Bytes @(0.62, 0.67, 0.76)
$FRAME_RGB = To-Bytes @(0.74, 0.80, 0.70)
$MARKER_RGB = To-Bytes @(0.30, 0.58, 0.96)

# The places of the panel, in the pixels of the window.
#
# The panel is at the bottom of the window, under the text of the document,
# and the breakpoint is in the margin of the text, on the left.
#
# The panel is 220 pixels high and the window is 750: the panel starts at
# 530. Its title is at 538, the headings of the two columns are at 562, and
# the first row of them is at 580. The column of the stack is on the left:
# the mark of the frame that is picked is at 12, and the text of the frames
# is at 24. The column of the variables is at 328.
#
# The adapter answers with three frames and two variables, so what the
# adapter said starts under the taller of the two columns, at 634.
$PANEL_HEIGHT = 220
$BREAKPOINT_X = 224
$BREAKPOINT_Y = 84
$MARKER_X = 12
$FRAME_X = 24
$FRAME_Y = 580
$VARIABLE_X = 328
$VARIABLE_Y = 580
$OUTPUT_Y = 634

# The flags of the mouse of Windows.
$MOUSEEVENTF_LEFTDOWN = 0x0002
$MOUSEEVENTF_LEFTUP = 0x0004

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}
if (-not (Test-Path $adapter)) {
    throw "the fake debug adapter is not built; run 'cargo build -p codeweaver-dap' first"
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

# The file that is opened.
#
# It is a program of its own for the sake of the adapter: the adapter is
# told that this is what is being debugged.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
[System.IO.File]::WriteAllText(
    $source,
    "fn main() {`n    let count = 3;`n    let name = `"greeting`";`n    println!(`"{name} {count}`");`n}`n",
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
    Start-Sleep -Milliseconds 60
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
}

# The key of one character.
function Get-VirtualKey([char]$character) {
    if ($character -eq ' ') { return 0x20 }
    $code = [int][char]$character
    if ($code -ge 97 -and $code -le 122) { return 0x41 + ($code - 97) }
    if ($code -ge 48 -and $code -le 57) { return 0x30 + ($code - 48) }
    throw "the check cannot press the key $character"
}

# Types a text.
function Send-Text([string]$text) {
    foreach ($character in $text.ToCharArray()) {
        if ($character -eq '>') {
            # The greater-than sign is the period with the shift held.
            Send-Chord @($VK_SHIFT) 0xBE
            continue
        }
        Send-Key (Get-VirtualKey $character)
    }
}

# Clicks on a place of the window.
#
# The place is in the pixels of the window, the way the editor counts them.
# The cursor is put there, the button goes down, and the button comes up.
# The cursor goes away from the editor again afterwards: the editor follows
# the real cursor, and a cursor that is left over the window would be a race
# for the checks that come after.
function Click-At([IntPtr]$handle, [double]$x, [double]$y) {
    $where = New-Object Win32+POINT
    $where.X = To-Screen $x
    $where.Y = To-Screen $y
    [Win32]::ClientToScreen($handle, [ref]$where) | Out-Null
    [Win32]::SetCursorPos($where.X, $where.Y) | Out-Null
    Start-Sleep -Milliseconds 300
    [Win32]::mouse_event($MOUSEEVENTF_LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 120
    [Win32]::mouse_event($MOUSEEVENTF_LEFTUP, 0, 0, 0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 600
    [Win32]::SetCursorPos(10, 10) | Out-Null
    Start-Sleep -Milliseconds 300
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

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'debug-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'debug-app-err.log') -PassThru
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

    $editor = Start-Editor @($source, '--dap', $adapter)
    $handle = $editor.Handle
    $previous = [IntPtr]::Zero
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        $previous = Focus-Editor $handle
        Start-Sleep -Seconds 2

        # F9 puts a breakpoint on the line of the caret.
        $previous = Focus-Editor $handle
        Send-Key $VK_F9
        Start-Sleep -Seconds 2

        $breakpointShot = Save-Window $handle $shotBreakpoint
        Write-Host "saved $shotBreakpoint"
        $dot = Count-In $breakpointShot (To-Screen $BREAKPOINT_X) (To-Screen $BREAKPOINT_Y) (To-Screen 20) (To-Screen 20) $BREAKPOINT_RGB 14
        Write-Host "pixels of the breakpoint in the margin: $dot"
        Check 'the breakpoint is drawn in the margin' ($dot -gt 10) "$dot pixels of the breakpoint"

        # Ctrl+F9 gives the breakpoint a condition, and a breakpoint with a
        # condition is drawn in another colour: it stops the program only
        # sometimes.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_F9
        Start-Sleep -Seconds 2
        Send-Text 'x>3'
        Start-Sleep -Seconds 2
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 2

        $conditionalShot = Save-Window $handle $shotConditional
        Write-Host "saved $shotConditional"
        $amber = Count-In $conditionalShot (To-Screen $BREAKPOINT_X) (To-Screen $BREAKPOINT_Y) (To-Screen 20) (To-Screen 20) $CONDITIONAL_RGB 14
        $red = Count-In $conditionalShot (To-Screen $BREAKPOINT_X) (To-Screen $BREAKPOINT_Y) (To-Screen 20) (To-Screen 20) $BREAKPOINT_RGB 14
        Write-Host "pixels of the breakpoint with a condition: $amber of the other colour, $red of the plain one"
        Check 'a breakpoint with a condition is drawn in another colour' ($amber -gt 10 -and $red -lt 10) `
            "$amber pixels of the conditional breakpoint, $red pixels of the plain one"

        # A line that is put above the breakpoint moves the breakpoint with it.
        $previous = Focus-Editor $handle
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 2

        $movedShot = Save-Window $handle $shotMoved
        Write-Host "saved $shotMoved"
        $below = Count-In $movedShot (To-Screen $BREAKPOINT_X) (To-Screen ($BREAKPOINT_Y + 22)) (To-Screen 20) (To-Screen 20) $CONDITIONAL_RGB 14
        $above = Count-In $movedShot (To-Screen $BREAKPOINT_X) (To-Screen $BREAKPOINT_Y) (To-Screen 20) (To-Screen 20) $CONDITIONAL_RGB 14
        Write-Host "pixels of the breakpoint: $below on the line below, $above on the line above"
        Check 'the breakpoint follows the text above it' ($below -gt 10 -and $above -lt 10) "$below pixels below, $above pixels above"

        # The undo takes the line away, and the breakpoint goes back with it.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL) $VK_Z
        Start-Sleep -Seconds 2

        $undoneShot = Save-Window $handle $shotUndone
        Write-Host "saved $shotUndone"
        $back = Count-In $undoneShot (To-Screen $BREAKPOINT_X) (To-Screen $BREAKPOINT_Y) (To-Screen 20) (To-Screen 20) $CONDITIONAL_RGB 14
        Write-Host "pixels of the breakpoint after the undo: $back"
        Check 'the breakpoint goes back with the undo' ($back -gt 10) "$back pixels of the breakpoint"

        # F5 starts the debug session.
        $previous = Focus-Editor $handle
        Send-Key $VK_F5

        $stoppedShot = $null
        $variables = 0
        $said = 0
        for ($attempt = 1; $attempt -le 6; $attempt++) {
            Start-Sleep -Seconds 3
            $shot = Save-Window $handle $shotStopped
            $variables = Count-In $shot (To-Screen $VARIABLE_X) (To-Screen $VARIABLE_Y) (To-Screen 500) (To-Screen 20) $VARIABLE_RGB 14
            $said = Count-In $shot (To-Screen 12) (To-Screen $OUTPUT_Y) (To-Screen 500) (To-Screen 100) $OUTPUT_RGB 14
            Write-Host "attempt ${attempt}: $variables pixels of variables, $said pixels of what the adapter said"
            if ($variables -gt 20) {
                $stoppedShot = $shot
                break
            }
            $shot.Bitmap.Dispose()
        }
        Check 'the variables of the frame are in the panel' ($variables -gt 20) "$variables pixels of the variables"

        $ground = Count-In $stoppedShot 0 ([int]($stoppedShot.Height - (To-Screen $PANEL_HEIGHT))) $stoppedShot.Width (To-Screen $PANEL_HEIGHT) $GROUND_RGB 4
        Write-Host "pixels of the ground of the panel: $ground"
        Check 'the panel of the debugger is at the bottom' ($ground -gt 5000) "$ground pixels of the ground"

        Check 'what the adapter said is in the panel' ($said -gt 20) "$said pixels of what the adapter said"

        # The stack of the program is in the panel, and the frame that is
        # picked is marked.
        $stack = Count-In $stoppedShot (To-Screen $FRAME_X) (To-Screen $FRAME_Y) (To-Screen 280) (To-Screen 40) $FRAME_RGB 14
        Write-Host "pixels of the frames of the stack: $stack"
        Check 'the stack of the program is in the panel' ($stack -gt 20) "$stack pixels of the frames"

        $marked = Count-In $stoppedShot (To-Screen $MARKER_X) (To-Screen $FRAME_Y) (To-Screen 6) (To-Screen 18) $MARKER_RGB 14
        Write-Host "pixels of the mark of the frame that is picked: $marked"
        Check 'the frame that is picked is marked' ($marked -gt 5) "$marked pixels of the mark"

        # A click on the second frame of the stack picks that frame: the
        # mark moves to the frame that was clicked on.
        $previous = Focus-Editor $handle
        Click-At $handle 40 ($FRAME_Y + 18)
        Start-Sleep -Seconds 2

        $pickedShot = Save-Window $handle $shotPicked
        Write-Host "saved $shotPicked"
        $markedSecond = Count-In $pickedShot (To-Screen $MARKER_X) (To-Screen ($FRAME_Y + 18)) (To-Screen 6) (To-Screen 18) $MARKER_RGB 14
        $markedFirst = Count-In $pickedShot (To-Screen $MARKER_X) (To-Screen $FRAME_Y) (To-Screen 6) (To-Screen 18) $MARKER_RGB 14
        Write-Host "pixels of the mark: $markedFirst on the first frame, $markedSecond on the second one"
        Check 'a click picks the frame that was clicked on' ($markedSecond -gt 5 -and $markedFirst -lt 5) `
            "$markedSecond pixels on the second frame, $markedFirst on the first one"

        # The frame that is picked is the top one again: the program stops
        # at the same place, and the stack is read again.
        $previous = Focus-Editor $handle
        Send-Key $VK_F10
        Start-Sleep -Seconds 3

        $steppedShot = Save-Window $handle $shotStepped
        Write-Host "saved $shotStepped"
        $stepped = Count-In $steppedShot (To-Screen $VARIABLE_X) (To-Screen $VARIABLE_Y) (To-Screen 500) (To-Screen 20) $VARIABLE_RGB 14
        Write-Host "pixels of the variables after the step: $stepped"
        Check 'the program stops again after a step' ($stepped -gt 20) "$stepped pixels of the variables"

        # Ctrl+Shift+E asks for an expression to be worked out, and what the
        # adapter says about it is in the panel.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_E
        Start-Sleep -Seconds 2
        Send-Text 'count'
        Start-Sleep -Seconds 2
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 3

        $evaluatedShot = Save-Window $handle $shotEvaluated
        Write-Host "saved $shotEvaluated"
        $evaluated = Count-In $evaluatedShot (To-Screen 12) (To-Screen $OUTPUT_Y) (To-Screen 500) (To-Screen 100) $OUTPUT_RGB 14
        Write-Host "pixels of what the panel says: $said before, $evaluated after"
        Check 'the worth of the expression is in the panel' ($evaluated -gt $said) `
            "$evaluated pixels after, $said before"

        $result = @{
            checks = $script:checks
            project = $project
            pixels = @{
                breakpoint = $dot
                conditional = $amber
                conditionalPlain = $red
                breakpointBelow = $below
                breakpointAbove = $above
                breakpointAfterUndo = $back
                ground = $ground
                variables = $variables
                output = $said
                stack = $stack
                marker = $marked
                markerAfterClick = $markedSecond
                markerBeforeClick = $markedFirst
                variablesAfterStep = $stepped
                outputAfterEvaluation = $evaluated
            }
            shots = @($shotBreakpoint, $shotConditional, $shotMoved, $shotUndone, $shotStopped, $shotPicked, $shotStepped, $shotEvaluated)
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
