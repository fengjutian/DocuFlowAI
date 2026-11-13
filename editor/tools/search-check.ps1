# The window-level check of the search in the files: the editor finds the text
# in the files of the project, says where it is, and opens the place that is
# picked.
#
# The project is outside the workspace, so the check does not touch the code of
# the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+Shift+F opens the panel of the search, and the panel covers the
#    window.
# 2. The word that is typed goes into the box of the panel, not into the
#    document behind it.
# 3. The places where the word was found are listed, and the build products of
#    the project are not searched at all.
# 4. The enter key opens the file of the place that is picked: the panel goes
#    away, the document is there again, and the editor says what it opened.
# 5. The escape key closes the panel.
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
$project = Join-Path $run 'search-project'
$source = Join-Path $project 'notes.txt'
$shotPanel = Join-Path $logs 'search-panel.png'
$shotFound = Join-Path $logs 'search-found.png'
$shotOpened = Join-Path $logs 'search-opened.png'
$shotEmpty = Join-Path $logs 'search-empty.png'
$report = Join-Path $logs 'search-check.json'

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
$VK_RETURN = 0x0D
$VK_BACK = 0x08
$VK_F = 0x46

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The ground of the panel, the colour of the text in the box, the colour of
# the place of a result, the colour of the text of the document and the colour
# of the status line.
$GROUND_RGB = To-Bytes @(0.055, 0.063, 0.082)
$TYPED_RGB = To-Bytes @(0.86, 0.89, 0.94)
$PLACE_RGB = To-Bytes @(0.62, 0.70, 0.86)
$BODY_RGB = To-Bytes @(0.075, 0.086, 0.11)
$STATUS_RGB = To-Bytes @(0.55, 0.60, 0.70)

# The places of the panel and of the window, in the pixels of the window.
#
# The box the word is written in is at the top; the list of the places is
# under it; the document is in the middle of the window; the status line is
# under the heading.
$BOX_X = 16
$BOX_Y = 16
$BOX_WIDTH = 640
$BOX_HEIGHT = 40
$LIST_X = 16
$LIST_Y = 90
$LIST_WIDTH = 1120
$LIST_HEIGHT = 560
$BODY_X = 236
$BODY_Y = 78
$BODY_WIDTH = 540
$BODY_HEIGHT = 560
$STATUS_X = 236
$STATUS_Y = 60
$STATUS_WIDTH = 700
$STATUS_HEIGHT = 22

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}

# The check drives the editor that is on disk, so that one has to be the one
# the sources describe.
$editorCrates = @('app', 'platform', 'render', 'text', 'syntax', 'lsp', 'ui', 'workspace', 'workbench', 'editor-core', 'agent-acp', 'agent-ui', 'diff', 'dap')
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

# The project the editor looks at.
#
# The word is in two files that are searched, and in one file that is in the
# build products: the search must not find that one.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $project 'other') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $project 'target') | Out-Null
$utf8 = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($source, "the first line`nthe needle is here`n", $utf8)
[System.IO.File]::WriteAllText((Join-Path $project 'other\notes2.txt'), "the needle is over here`n", $utf8)
[System.IO.File]::WriteAllText((Join-Path $project 'target\built.txt'), "the hiddenonly is here`n", $utf8)
# The manifest of the project pins where the search starts: without it the
# editor would walk up the folders looking for the project of the file, and
# the search would cover the whole home folder of the machine.
[System.IO.File]::WriteAllText((Join-Path $project 'package.json'), "{ `"name`": `"search-project`" }`n", $utf8)
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

# Presses a key that has no modifier.
function Send-Key([int]$virtualKey) {
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]0, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
    [Win32]::keybd_event([byte]$virtualKey, 0, [uint32]2, [UIntPtr]::Zero)
    Start-Sleep -Milliseconds 60
}

# Types a word, one letter at a time.
function Send-Word([string]$word) {
    foreach ($letter in $word.ToCharArray()) {
        Send-Key ([int][char]$letter)
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
#
# The rectangle is given in the pixels of the window, the way the places of
# the panel are.
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

# The scale of the window: the editor draws in the pixels of the screen, and
# its own places are given in the pixels of the window.
$script:Scale = 1.0
function To-Screen([double]$logical) {
    return [int]($logical * $script:Scale)
}

# Counts the pixels of one colour in a rectangle of the client area.
#
# The rectangle is given in the pixels of the screen, the way the places of
# the panel are.
function Count-In($shot, [int]$left, [int]$top, [int]$width, [int]$height, [int[]]$rgb, [int]$tolerance) {
    return Count-Color $shot.Bitmap ($shot.Left + $left) ($shot.Top + $top) $width $height $rgb $tolerance
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'search-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'search-app-err.log') -PassThru
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

        # Ctrl+Shift+F opens the panel of the search.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_F
        Start-Sleep -Seconds 3

        $panel = Save-Window $handle $shotPanel
        Write-Host "saved $shotPanel"

        # The places of the panel, in the pixels of the screen.
        $boxLeft = To-Screen $BOX_X
        $boxTop = To-Screen $BOX_Y
        $boxWidth = To-Screen $BOX_WIDTH
        $boxHeight = To-Screen $BOX_HEIGHT
        $listLeft = To-Screen $LIST_X
        $listTop = To-Screen $LIST_Y
        $listWidth = To-Screen $LIST_WIDTH
        $listHeight = To-Screen $LIST_HEIGHT
        $bodyLeft = To-Screen $BODY_X
        $bodyTop = To-Screen $BODY_Y
        $bodyWidth = To-Screen $BODY_WIDTH
        $bodyHeight = To-Screen $BODY_HEIGHT
        $statusLeft = To-Screen $STATUS_X
        $statusTop = To-Screen $STATUS_Y
        $statusWidth = To-Screen $STATUS_WIDTH
        $statusHeight = To-Screen $STATUS_HEIGHT

        $ground = Count-In $panel 0 0 $panel.Width $panel.Height $GROUND_RGB 3
        Write-Host "pixels of the ground of the panel: $ground"
        Check 'the panel covers the window' ($ground -gt 100000) "$ground pixels of the ground"

        # The word is typed into the box of the panel.
        $previous = Focus-Editor $handle
        Send-Word 'needle'
        Start-Sleep -Seconds 2

        $typedShot = Save-Window $handle $shotPanel
        Write-Host "saved $shotPanel"
        $typed = Count-In $typedShot $boxLeft $boxTop $boxWidth $boxHeight $TYPED_RGB 10
        Write-Host "pixels of the word in the box: $typed"
        Check 'the word goes into the box of the panel' ($typed -gt 15) "$typed pixels of the word"

        # The enter key starts the search.
        $previous = Focus-Editor $handle
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 4

        $found = Save-Window $handle $shotFound
        Write-Host "saved $shotFound"
        $places = Count-In $found $listLeft $listTop $listWidth $listHeight $PLACE_RGB 14
        Write-Host "pixels of the places that were found: $places"
        Check 'the places of the word are listed' ($places -gt 40) "$places pixels of the places"

        # The enter key opens the place that is picked.
        $previous = Focus-Editor $handle
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 3

        $opened = Save-Window $handle $shotOpened
        Write-Host "saved $shotOpened"
        $openedGround = Count-In $opened 0 0 $opened.Width $opened.Height $GROUND_RGB 3
        Write-Host "pixels of the ground after the panel is closed: $openedGround"
        Check 'the panel goes away when the place is opened' ($openedGround -lt 10000) "$openedGround pixels of the ground"

        $document = Count-In $opened $bodyLeft $bodyTop $bodyWidth $bodyHeight $BODY_RGB 3
        Write-Host "pixels of the document: $document"
        Check 'the document is there again' ($document -gt 10000) "$document pixels of the document"

        $said = Count-In $opened $statusLeft $statusTop $statusWidth $statusHeight $STATUS_RGB 10
        Write-Host "pixels of the status line: $said"
        Check 'the editor says what it opened' ($said -gt 15) "$said pixels of the status line"

        # The panel is opened again, and the word that is only in the build
        # products is looked for: the search must not find it.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_F
        Start-Sleep -Seconds 3
        $previous = Focus-Editor $handle
        for ($i = 0; $i -lt 12; $i++) { Send-Key $VK_BACK }
        Send-Word 'hiddenonly'
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 4

        $empty = Save-Window $handle $shotEmpty
        Write-Host "saved $shotEmpty"
        $none = Count-In $empty $listLeft $listTop $listWidth $listHeight $PLACE_RGB 14
        Write-Host "pixels of the places of the word that is only in the build products: $none"
        Check 'the build products are not searched' ($none -lt 15) "$none pixels of the places"

        # The escape key closes the panel.
        $previous = Focus-Editor $handle
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 2

        $closed = Save-Window $handle $shotEmpty
        Write-Host "saved $shotEmpty"
        $closedGround = Count-In $closed 0 0 $closed.Width $closed.Height $GROUND_RGB 3
        Write-Host "pixels of the ground after the escape: $closedGround"
        Check 'the escape key closes the panel' ($closedGround -lt 10000) "$closedGround pixels of the ground"

        $result = @{
            checks = $script:checks
            project = $project
            pixels = @{
                ground = $ground
                word = $typed
                places = $places
                groundAfterOpen = $openedGround
                document = $document
                status = $said
                placesOfTheHiddenWord = $none
                groundAfterEscape = $closedGround
            }
            shots = @($shotPanel, $shotFound, $shotOpened, $shotEmpty)
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
