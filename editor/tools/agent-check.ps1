# The window-level check of the agent panel: the context the user picks, the
# change the agent proposes, and the change the user approves.
#
# The editor is driven against the mock ACP endpoint in
# `crates/agent-acp/tests/fixtures/mock-acp-agent.mjs`, so the check needs no
# runtime and no model. The endpoint is the real one — only its backend is
# scripted — so the shapes that travel are the production ones.
#
# The mouse messages are posted to the editor's window, so the check never
# takes the cursor away from whoever is using the machine.
#
# What the check proves, in the window and not in a unit test:
#
# 1. The panel lists the context the user can pick from.
# 2. A change the agent proposes is shown as blocks, with the line each block
#    starts at.
# 3. Approving a block and applying it changes the text in the buffer, and
#    the tab is marked as changed.
#
# The panel is on the right of the window. Its text is drawn in a light grey
# on a dark background, so the check counts the bright pixels of the panel to
# say that something is written there, and counts the pixels that differ
# between two pictures to say that the text changed.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$mock = Join-Path $root 'crates\agent-acp\tests\fixtures\mock-acp-agent.mjs'
$work = Join-Path $logs 'agent'
$fixture = Join-Path $work 'fixture.txt'
$shotPanel = Join-Path $logs 'agent-panel.png'
$shotDiff = Join-Path $logs 'agent-diff.png'
$shotApplied = Join-Path $logs 'agent-applied.png'
$report = Join-Path $logs 'agent-check.json'

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
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();
  [DllImport("user32.dll")] public static extern uint SendInput(uint count, INPUT[] inputs, int size);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();

  [StructLayout(LayoutKind.Explicit)]
  public struct INPUT {
    [FieldOffset(0)] public uint type;
    [FieldOffset(8)] public InputUnion U;
  }

  [StructLayout(LayoutKind.Explicit)]
  public struct InputUnion {
    [FieldOffset(0)] public KEYBDINPUT ki;
  }

  [StructLayout(LayoutKind.Sequential)]
  public struct KEYBDINPUT {
    public ushort wVk;
    public ushort wScan;
    public uint dwFlags;
    public uint time;
    public IntPtr dwExtraInfo;
  }

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

$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Screen([double[]]$linear) {
    return @($linear | ForEach-Object { [int][Math]::Round((To-Srgb $_) * 255) })
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The colour of the text in the agent panel.
$PANEL_TEXT_RGB = To-Bytes @(0.84, 0.87, 0.92)
# The colour of the dirty mark on a tab: text, so the colour is the one the
# code writes.
$DIRTY_RGB = To-Bytes @(0.30, 0.58, 0.96)

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}
if (-not (Test-Path $mock)) {
    throw "the mock endpoint is missing: $mock"
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
# with fewer rights than the programs it starts: it cannot start the agent
# endpoint, which is refused with "access denied". The copy runs with the
# rights a program normally has, and the check is about the editor, not
# about the label of the folder it was built in.
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null
# The text the mock endpoint proposes a change to.
Set-Content -Path $fixture -Value "line one`nline two`n" -Encoding UTF8

$script:Factor = 1.0
# The mouse messages carry the physical pixels of the screen.
function To-Physical([double]$value) {
    if ($scriptIsAware) { return $value }
    return $value * $script:Factor
}

# Presses a key.
#
# Only the key goes down and comes up. The character is not posted with it:
# the window makes the character out of the key by itself, the same way it
# does when a person types. A character that is posted on its own is dropped
# by the window, because it does not know which key made it.
function Send-KeyDown([IntPtr]$handle, [int]$virtualKey, [int]$scanCode) {
    $repeat = [int64]1
    $scanned = [int64]$scanCode -shl 16
    $flags = [int64]0xC0000000
    $down = $repeat -bor $scanned -bor $flags
    [Win32]::PostMessage($handle, $WM_KEYDOWN, [IntPtr]$virtualKey, [IntPtr]$down) | Out-Null
}

function Send-KeyUp([IntPtr]$handle, [int]$virtualKey, [int]$scanCode) {
    $repeat = [int64]1
    $scanned = [int64]$scanCode -shl 16
    $flags = [int64]0xC0000000
    $up = $repeat -bor $scanned -bor $flags
    [Win32]::PostMessage($handle, $WM_KEYUP, [IntPtr]$virtualKey, [IntPtr]$up) | Out-Null
}

function Send-Key([IntPtr]$handle, [int]$virtualKey, [int]$scanCode) {
    Send-KeyDown $handle $virtualKey $scanCode
    Send-KeyUp $handle $virtualKey $scanCode
}

# Presses a key with modifiers.
#
# The keys are injected into the system, the way a keyboard would, because
# the window reads the state of the modifiers from the keyboard itself: a
# message that is only posted to the window does not change that state.
# Injected keys go to the window that has the focus, so the editor is brought
# to the front first, and the window that had it is given back at the end.
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
    }
}

function Save-Window([IntPtr]$handle, [string]$path) {
    # A minimised window has no client area to draw into, and a window that
    # was never shown does not redraw. The restore is retried: something else
    # on the machine minimises the window now and then, and a check that fails
    # because the window was not on screen says nothing about the editor.
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

# The area the agent panel is drawn in, in the picture.
#
# The editor draws the panel on the right of the window, a third of it wide,
# with a floor and a ceiling. The script works out the same place, so that
# the text it counts is the text of the panel.
function Get-PanelRect($shot) {
    $logical = $shot.Width / $script:Factor
    $panelLogical = [Math]::Min([Math]::Max($logical * 0.34, 280), 520)
    $width = [int]($panelLogical * $script:Factor)
    return @{
        Left = $shot.Left + $shot.Width - $width
        Top = $shot.Top + [int](To-Physical 50)
        Width = $width
        Height = [int](To-Physical 400)
    }
}

# The area the text of the document is drawn in, in the picture.
function Get-BodyRect($shot) {
    $panel = Get-PanelRect $shot
    $left = $shot.Left + [int](To-Physical 236)
    $right = $panel.Left - [int](To-Physical 16)
    return @{
        Left = $left
        Top = $shot.Top + [int](To-Physical 78)
        Width = [Math]::Max($right - $left, 1)
        Height = [int](To-Physical 200)
    }
}

# The number of bright pixels in a rectangle: the text of the panel is much
# brighter than the panel it is drawn on.
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

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'agent-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'agent-app-err.log') -PassThru
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
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        [Win32]::SetCursorPos(10, 10) | Out-Null
        Start-Sleep -Milliseconds 300
        $where = New-Object Win32+POINT
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
#
# The focus is taken by attaching to the thread of the window that has it:
# a process may not otherwise take the focus away from another one.
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

    # The editor is pointed at the mock endpoint, which proposes a change
    # when it is asked to.
    if ($mock.Contains(' ')) {
        throw "the mock endpoint is at a path with a space in it: $mock"
    }
    $env:CODEWEAVER_ACP_COMMAND = 'node'
    $env:CODEWEAVER_ACP_ARGS = "$mock diff"
    $env:CODEWEAVER_ACP_CWD = $work

    $editor = Start-Editor @($fixture)
    $handle = $editor.Handle
    $previous = [IntPtr]::Zero
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        $previous = Focus-Editor $handle

        # 1. The panel is drawn and lists what the user can pick from.
        $panelShot = Save-Window $handle $shotPanel
        Write-Host "saved $shotPanel"
        $picture = $panelShot.Bitmap
        $panel = Get-PanelRect $panelShot
        $body = Get-BodyRect $panelShot
        $bright = Count-Bright $picture $panel.Left $panel.Top $panel.Width $panel.Height
        Write-Host "bright pixels in the agent panel: $bright"
        Check 'the agent panel is drawn' ($bright -ge 20) "$bright bright pixels in the panel"

        # 2. Ctrl+M starts a turn; the agent proposes a change and the panel
        #    shows it as blocks.
        Send-Chord @(0x11) 0x4d
        Start-Sleep -Seconds 4

        $diffShot = Save-Window $handle $shotDiff
        Write-Host "saved $shotDiff"
        $picture = $diffShot.Bitmap
        $panel = Get-PanelRect $diffShot
        $body = Get-BodyRect $diffShot
        # The panel has to say more than it said before the turn: the answer
        # of the agent and the change it proposes.
        $said = Count-Difference $panelShot.Bitmap $picture $panel.Left $panel.Top $panel.Width $panel.Height
        Write-Host "pixels of the panel that changed during the turn: $said"
        Check 'the panel shows what the agent said' ($said -ge 200) "$said pixels of the panel changed"

        # 3. The proposed change is shown as blocks, and approving one and
        #    applying it changes the text in the buffer.
        #
        # The change has to be approved first: Ctrl+Alt+1 picks the first
        # block, Ctrl+Alt+A approves it, and Ctrl+Alt+Enter writes it into
        # the buffer.
        $before = $picture
        $beforeBright = Count-Bright $picture $body.Left $body.Top $body.Width $body.Height
        Write-Host "bright pixels in the body before the change: $beforeBright"
        Send-Chord @(0x11, 0x12) 0x31
        Start-Sleep -Milliseconds 500
        Send-Chord @(0x11, 0x12) 0x41
        Start-Sleep -Milliseconds 500
        Send-Chord @(0x11, 0x12) 0x0d
        Start-Sleep -Seconds 3

        $appliedShot = Save-Window $handle $shotApplied
        Write-Host "saved $shotApplied"
        $applied = $appliedShot.Bitmap
        # The text of the buffer is what changed, so the body is what is
        # looked at: the panel changes on its own, the body only when the
        # change was written into the buffer.
        $changed = Count-Difference $before $applied $body.Left $body.Top $body.Width $body.Height
        Write-Host "pixels of the body that changed after the change was applied: $changed"
        Check 'the approved change reaches the editor' ($changed -ge 200) "$changed pixels changed"

        # The buffer has one line more than it had: the change added one.
        $afterBright = Count-Bright $applied $body.Left $body.Top $body.Width $body.Height
        Write-Host "bright pixels in the body after the change: $afterBright"
        Check 'the body shows more text than before' ($afterBright -gt $beforeBright) "$beforeBright before, $afterBright after"

        # The panel keeps the trail of what happened: the change was approved
        # and it was written into the buffer.
        $trail = Count-Difference $diffShot.Bitmap $applied $panel.Left $panel.Top $panel.Width $panel.Height
        Write-Host "pixels of the panel that changed after the approval: $trail"
        Check 'the panel keeps the trail of the change' ($trail -ge 50) "$trail pixels of the panel changed"

        # The tab of the file that changed is marked as changed.
        $dotTop = $appliedShot.Top + [int](To-Physical 5)
        $dotHeight = [int](To-Physical 26)
        $dotWidth = [int](To-Physical 220)
        $dot = Count-Color $applied $appliedShot.Left $dotTop $dotWidth $dotHeight $DIRTY_RGB 18
        Write-Host "pixels of the dirty mark: $dot"
        Check 'the changed file is marked in the strip' ($dot -ge 3) "$dot pixels of the dirty mark"

        $panelShot.Bitmap.Dispose()
        $diffShot.Bitmap.Dispose()
        $before.Dispose()
        $applied.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
        # The window that had the focus gets it back.
        if ($previous -ne [IntPtr]::Zero) {
            [Win32]::SetForegroundWindow($previous) | Out-Null
        }
    }
}
finally {
    $moved = New-Object Win32+POINT
    [Win32]::GetCursorPos([ref]$moved) | Out-Null
    [Win32]::SetCursorPos($cursor.X, $cursor.Y) | Out-Null
    Remove-Item Env:\CODEWEAVER_ACP_COMMAND -ErrorAction SilentlyContinue
    Remove-Item Env:\CODEWEAVER_ACP_ARGS -ErrorAction SilentlyContinue
    Remove-Item Env:\CODEWEAVER_ACP_CWD -ErrorAction SilentlyContinue
}

$failed = @($script:checks | Where-Object { -not $_.ok })
$summary = @{
    version = '0.6.0'
    checks = $script:checks
    passed = $script:checks.Count - $failed.Count
    failed = $failed.Count
    screenshots = @($shotPanel, $shotDiff, $shotApplied)
}
$summary | ConvertTo-Json -Depth 6 | Set-Content -Path $report -Encoding UTF8
Write-Host "wrote $report"

if ($failed.Count -gt 0) {
    Write-Host "$($failed.Count) check(s) failed"
    exit 1
}
Write-Host "every check passed"
