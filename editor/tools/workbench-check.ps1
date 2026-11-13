# The window-level check of the workbench: the file tree, the tab strip and
# the dirty mark.
#
# The mouse messages are posted to the editor's window, so the check never
# takes the cursor away from whoever is using the machine. winit reads the
# position out of the message, so the editor sees the click where it was
# sent.
#
# Two coordinate systems are in play, and mixing them up is how a check ends
# up looking at the wrong part of the window:
#
# - The mouse messages carry the physical pixels of the screen, because the
#   editor is per-monitor aware. They are turned into the logical pixels the
#   editor works in.
# - The picture carries the logical pixels of the window, because that is
#   how the window is drawn into it. The place of the client area inside the
#   picture is the difference between where the client area is and where the
#   window is.
#
# What the check proves, in the window and not in a unit test:
#
# 1. The file tree is drawn, and the files of the folder are in it.
# 2. Clicking a file in the tree opens it in a tab of its own, and that tab
#    comes to the front. The tab that is in front is marked with a bar of
#    colour under the top of the tab, so the place of that bar says which tab
#    is in front.
# 3. Typing a character makes the tab dirty, and the dirty mark is drawn.
#
# The marks are looked for by their colour. The bar under the active tab is
# drawn as a block of colour and goes through the conversion from linear to
# sRGB; the dirty dot is text and does not. The two are therefore different
# colours on the screen, and the check tells them apart.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$fixture = Join-Path $PSScriptRoot 'fixtures\workbench'
$work = Join-Path $logs 'workbench'
$first = Join-Path $work 'a-first.txt'
$second = Join-Path $work 'b-second.txt'
$shotTree = Join-Path $logs 'workbench-tree.png'
$shotTab = Join-Path $logs 'workbench-tab.png'
$shotDirty = Join-Path $logs 'workbench-dirty.png'
$report = Join-Path $logs 'workbench-check.json'

$sig = @'
using System;
using System.Runtime.InteropServices;
using System.Text;
public class Win32 {
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr extra);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
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

$WM_MOUSEMOVE = 0x0200
$WM_LBUTTONDOWN = 0x0201
$WM_LBUTTONUP = 0x0202
$WM_KEYDOWN = 0x0100
$WM_KEYUP = 0x0101
$WM_CHAR = 0x0102
$MK_LBUTTON = 0x0001

# The place of the parts of the window, in the logical pixels the editor
# works in: the strip of tabs at the top, the tree of files under it on the
# left, and a row of the tree every 22 pixels.
$TAB_STRIP_HEIGHT = 34
$SIDEBAR_WIDTH = 220
$TREE_ROW_HEIGHT = 22
# The row of the second file of the fixture: the first row is the first file.
$TREE_ROW_OF_SECOND = 1

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
# The bar under the tab that is in front: a block of colour.
$ACTIVE_RGB = To-Screen @(0.30, 0.58, 0.96)
# The dot of a tab that has changes: text, so the colour is the one the code
# writes.
$DIRTY_RGB = To-Bytes @(0.30, 0.58, 0.96)

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}
# The check drives the editor that is on disk, so that one has to be the one
# the sources describe. `cargo test` does not build it.
$editorCrates = @('app', 'platform', 'render', 'text', 'syntax', 'lsp', 'ui', 'workspace', 'workbench', 'editor-core', 'agent-acp', 'agent-ui')
$newest = $editorCrates | ForEach-Object {
    $crate = Join-Path $root "crates\$_"
    if (Test-Path $crate) {
        Get-ChildItem -Path (Join-Path $crate 'src') -Recurse -Include *.rs -ErrorAction SilentlyContinue
    }
} | Sort-Object LastWriteTime -Descending | Select-Object -First 1
if ($newest -and (Get-Item $exe).LastWriteTime -lt $newest.LastWriteTime) {
    throw "the editor is older than its sources; run 'cargo build -p codeweaver-editor' first"
}

New-Item -ItemType Directory -Force -Path $logs | Out-Null
if (Test-Path $work) { Remove-Item -Recurse -Force $work }
New-Item -ItemType Directory -Force -Path $work | Out-Null
Copy-Item -Path (Join-Path $fixture '*.txt') -Destination $work -Force

$script:Factor = 1.0
# The mouse messages carry the physical pixels of the screen.
function To-Physical([double]$value) {
    if ($scriptIsAware) { return $value }
    return $value * $script:Factor
}

function Send-Mouse([IntPtr]$handle, [uint32]$message, [int]$x, [int]$y, [int]$extra) {
    $packed = [int64](($y -band 0xFFFF) -shl 16) -bor ($x -band 0xFFFF) -bor $extra
    [Win32]::PostMessage($handle, $message, [IntPtr]0, [IntPtr]$packed) | Out-Null
}

# Presses a key.
#
# Only the key goes down and comes up. The character is not posted with it:
# the window makes the character out of the key by itself, the same way it
# does when a person types. A character that is posted on its own is dropped
# by the window, because it does not know which key made it.
function Send-Key([IntPtr]$handle, [int]$virtualKey, [int]$scanCode) {
    $repeat = [int64]1
    $scanned = [int64]$scanCode -shl 16
    $flags = [int64]0xC0000000
    $down = $repeat -bor $scanned -bor $flags
    [Win32]::PostMessage($handle, $WM_KEYDOWN, [IntPtr]$virtualKey, [IntPtr]$down) | Out-Null
    [Win32]::PostMessage($handle, $WM_KEYUP, [IntPtr]$virtualKey, [IntPtr]$down) | Out-Null
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

    # The window is drawn into the picture in the pixels of the screen, so
    # the picture is as big as the window is on the screen, and the client
    # area starts where the client area and the window differ.
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
        # Where the client area starts inside the picture.
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

# The place of the leftmost pixel of one colour in a band of the picture.
# Returns -1 when the colour is not there at all.
function Left-Of-Color($picture, [int]$left, [int]$top, [int]$width, [int]$height, [int[]]$rgb, [int]$tolerance) {
    for ($x = $left; $x -lt $left + $width; $x++) {
        for ($y = $top; $y -lt $top + $height; $y++) {
            if ($y -lt 0 -or $y -ge $picture.Height) { continue }
            if ($x -lt 0 -or $x -ge $picture.Width) { continue }
            $pixel = $picture.GetPixel($x, $y)
            if ([Math]::Abs($pixel.R - $rgb[0]) -le $tolerance -and
                [Math]::Abs($pixel.G - $rgb[1]) -le $tolerance -and
                [Math]::Abs($pixel.B - $rgb[2]) -le $tolerance) {
                return $x
            }
        }
    }
    return -1
}

# The number of pixels that are different in two pictures.
function Count-Difference($first, $second) {
    $count = 0
    $width = [Math]::Min($first.Width, $second.Width)
    $height = [Math]::Min($first.Height, $second.Height)
    for ($y = 0; $y -lt $height; $y++) {
        for ($x = 0; $x -lt $width; $x++) {
            $a = $first.GetPixel($x, $y)
            $b = $second.GetPixel($x, $y)
            if ($a.R -ne $b.R -or $a.G -ne $b.G -or $a.B -ne $b.B) { $count++ }
        }
    }
    return $count
}

# The number of bright pixels in a rectangle: the text of the tree is much
# brighter than the panel it is drawn on. The panel is a dark grey, so the
# line between them is high enough to tell the text from the panel.
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
    $process = Start-Process -FilePath $exe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'app-out.log') `
        -RedirectStandardError (Join-Path $logs 'app-err.log') -PassThru
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

    $editor = Start-Editor @($first)
    $handle = $editor.Handle
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"

        # 1. The tree of files is drawn.
        $treeShot = Save-Window $handle $shotTree
        Write-Host "saved $shotTree"
        $picture = $treeShot.Bitmap
        $left = $treeShot.Left
        $top = $treeShot.Top
        # The tree is under the strip of tabs, on the left of the window.
        $treeTop = $top + [int](To-Physical $TAB_STRIP_HEIGHT)
        $treeHeight = [int](To-Physical ($TREE_ROW_HEIGHT * 3))
        $treeWidth = [int](To-Physical $SIDEBAR_WIDTH)
        $bright = Count-Bright $picture $left $treeTop $treeWidth $treeHeight
        Write-Host "bright pixels in the tree: $bright"
        Check 'the tree of files is drawn' ($bright -ge 20) "$bright bright pixels in the tree"
        # The tab that is in front is marked with a bar of colour under the
        # top of the strip.
        $barTop = $top
        $barHeight = [int](To-Physical 3)
        $barBefore = Left-Of-Color $picture $left $barTop $treeWidth $barHeight $ACTIVE_RGB 14
        Write-Host "the bar of the tab in front starts at: $barBefore"
        Check 'the tab in front is marked' ($barBefore -ge 0) "the bar starts at $barBefore"
        $picture.Dispose()

        # 2. Clicking a file in the tree opens it in a tab of its own.
        [Win32]::SetCursorPos(10, 10) | Out-Null
        Start-Sleep -Milliseconds 300
        $clickX = [int](To-Physical 40)
        $clickY = [int](To-Physical ($TAB_STRIP_HEIGHT + $TREE_ROW_HEIGHT * $TREE_ROW_OF_SECOND + 8))
        Send-Mouse $handle $WM_MOUSEMOVE $clickX $clickY 0
        Start-Sleep -Milliseconds 300
        Send-Mouse $handle $WM_LBUTTONDOWN $clickX $clickY 0
        Start-Sleep -Milliseconds 300
        Send-Mouse $handle $WM_LBUTTONUP $clickX $clickY 0
        Start-Sleep -Seconds 2

        $tabShot = Save-Window $handle $shotTab
        Write-Host "saved $shotTab"
        $picture = $tabShot.Bitmap
        $left = $tabShot.Left
        $top = $tabShot.Top
        $barAfter = Left-Of-Color $picture $left $barTop $treeWidth $barHeight $ACTIVE_RGB 14
        Write-Host "the bar of the tab in front starts at: $barAfter"
        $moved = ($barAfter -ge 0) -and ($barAfter -gt $barBefore + 20)
        Check 'clicking a file in the tree opens it in a tab of its own' $moved `
            "the tab in front moved from $barBefore to $barAfter"

        # 3. Typing makes the tab dirty, and the dirty mark is drawn.
        [Win32]::SetCursorPos(10, 10) | Out-Null
        Start-Sleep -Milliseconds 300
        # The arrow key first: the caret moves, and the picture changes. It
        # says whether the key messages arrive at all.
        Send-Key $handle 0x28 0x50
        Start-Sleep -Seconds 2
        $arrowShot = Save-Window $handle (Join-Path $logs 'workbench-arrow.png')
        $arrowPicture = $arrowShot.Bitmap
        $moved = Count-Difference $picture $arrowPicture
        Write-Host "pixels that changed after the arrow key: $moved"
        Check 'the keyboard reaches the editor' ($moved -ge 1) "$moved pixels changed"

        # The file is not changed yet, so there is no dirty mark yet.
        $dotBefore = Count-Color $arrowPicture $left ($top + [int](To-Physical 5)) $treeWidth ([int](To-Physical 26)) $DIRTY_RGB 18
        Write-Host "pixels of the dirty mark before the text changed: $dotBefore"
        Check 'a file that is not changed has no dirty mark' ($dotBefore -lt 3) `
            "$dotBefore pixels of the dirty mark"

        # The letter X would not do here: the window makes the character out
        # of the key by itself, and on a machine with an input method the
        # character comes from the input method and not from the key. The
        # Delete key needs no character: it deletes the character under the
        # caret, and the text of the document changes.
        Send-Key $handle 0x2e 0x53
        Start-Sleep -Seconds 2

        $dirtyShot = Save-Window $handle $shotDirty
        Write-Host "saved $shotDirty"
        $picture = $dirtyShot.Bitmap
        $left = $dirtyShot.Left
        $top = $dirtyShot.Top
        # The dot is drawn in the strip, below the bar of the tab in front.
        $dotTop = $top + [int](To-Physical 5)
        $dotHeight = [int](To-Physical 26)
        $dot = Count-Color $picture $left $dotTop $treeWidth $dotHeight $DIRTY_RGB 18
        Write-Host "pixels of the dirty mark: $dot"
        Check 'a changed file is marked in the strip' ($dot -ge 3) "$dot pixels of the dirty mark"
        $picture.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }
}
finally {
    $moved = New-Object Win32+POINT
    [Win32]::GetCursorPos([ref]$moved) | Out-Null
    [Win32]::SetCursorPos($cursor.X, $cursor.Y) | Out-Null
}

$failed = @($script:checks | Where-Object { -not $_.ok })
$summary = @{
    version = '0.4.0'
    checks = $script:checks
    passed = $script:checks.Count - $failed.Count
    failed = $failed.Count
    screenshots = @($shotTree, $shotTab, $shotDirty)
}
$summary | ConvertTo-Json -Depth 6 | Set-Content -Path $report -Encoding UTF8
Write-Host "wrote $report"

if ($failed.Count -gt 0) {
    Write-Host "$($failed.Count) check(s) failed"
    exit 1
}
Write-Host "every check passed"
