# The window-level check of the path above the text: the row is drawn, the
# names of the folders and of the file are written in it, and clicking a folder
# of the path makes the tree of files look at that folder.
#
# The project is a folder of its own, outside the workspace, so the check does
# not touch the code of the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. The row that says where the file is is drawn between the tabs and the
#    text.
# 2. The name of the file is written in the row.
# 3. The name of the folder above it is written in the row as well.
# 4. Clicking the folder of the project in the path makes the tree of files
#    look at that folder: the side bar shows a directory of its own afterwards.
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
$project = Join-Path $run 'breadcrumbs-project'
$folder = Join-Path $project 'src'
$source = Join-Path $folder 'main.rs'
$shotStart = Join-Path $logs 'breadcrumbs-start.png'
$shotAfter = Join-Path $logs 'breadcrumbs-after.png'
$report = Join-Path $logs 'breadcrumbs-check.json'

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

# The flags of the mouse of Windows.
$MOUSEEVENTF_LEFTDOWN = 0x0002
$MOUSEEVENTF_LEFTUP = 0x0004

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

# The colours the check looks for.
#
# The ground of the row of the path is a block of colour, so it goes through the
# conversion of the screen. The names in it are text, and the text is drawn in
# the colour the code writes.
$ROW_GROUND_RGB = To-Pixel @(0.10, 0.12, 0.16)
$FILE_RGB = To-Bytes @(0.82, 0.86, 0.93)
$FOLDER_RGB = To-Bytes @(0.52, 0.56, 0.64)
# The name of a directory in the tree of files is text as well.
$DIRECTORY_RGB = To-Bytes @(0.76, 0.80, 0.88)

# The places of the parts of the window, in the pixels of the window.
$ACTIVITY_BAR_WIDTH = 48.0
$SIDEBAR_WIDTH = 220.0
$TAB_BAR_HEIGHT = 34.0
$BREADCRUMB_HEIGHT = 24.0
$PATH_PADDING = 12.0
$SIDEBAR_TITLE_HEIGHT = 28.0
$STATUS_BAR_HEIGHT = 24.0

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
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null

# The project of the check. It has a manifest of its own, so that the editor
# says which project the file belongs to: the path of the file is shortened
# with it.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $folder | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'Cargo.toml'),
    "[workspace]`n",
    (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText(
    $source,
    ("one`ntwo`nthree`n"),
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

# Clicks on a place of the window.
#
# The place is in the pixels of the window, the way the editor counts them.
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
    Start-Sleep -Milliseconds 800
    [Win32]::SetCursorPos(10, 10) | Out-Null
    Start-Sleep -Milliseconds 300
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

# Where the row of the path is, in the pixels of the window.
function Get-Places($shot) {
    $logicalWidth = $shot.Width / $script:Scale
    $logicalHeight = $shot.Height / $script:Scale
    $agentWidth = [Math]::Min(520.0, [Math]::Max(280.0, $logicalWidth * 0.34))
    $right = $logicalWidth - $agentWidth - 16.0
    $rowLeft = $ACTIVITY_BAR_WIDTH + $SIDEBAR_WIDTH
    return @{
        LogicalWidth = $logicalWidth
        LogicalHeight = $logicalHeight
        RowLeft = $rowLeft
        RowTop = $TAB_BAR_HEIGHT
        RowHeight = $BREADCRUMB_HEIGHT
        RowRight = $right
        # The first piece of the path is written at the left of the row.
        FirstTextLeft = $rowLeft + $PATH_PADDING
        SidebarLeft = $ACTIVITY_BAR_WIDTH
        SidebarTop = $SIDEBAR_TITLE_HEIGHT
        SidebarWidth = $SIDEBAR_WIDTH
        SidebarHeight = $logicalHeight - $SIDEBAR_TITLE_HEIGHT - $STATUS_BAR_HEIGHT
    }
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'breadcrumbs-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'breadcrumbs-app-err.log') -PassThru
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
        Start-Sleep -Seconds 2

        $startShot = Save-Window $handle $shotStart
        Write-Host "saved $shotStart"
        $places = Get-Places $startShot
        $rowLeft = (To-Screen $places.RowLeft)
        $rowTop = (To-Screen $places.RowTop)
        $rowWidth = (To-Screen ($places.RowRight - $places.RowLeft))
        $rowHeight = (To-Screen $places.RowHeight)

        # 1. The row of the path is drawn between the tabs and the text.
        $ground = Count-In $startShot $rowLeft $rowTop $rowWidth $rowHeight $ROW_GROUND_RGB 6
        $area = $rowWidth * $rowHeight
        $share = $ground / $area
        Write-Host "pixels of the ground of the row: $ground of $area"
        Check 'the row of the path is drawn' ($share -gt 0.80) ("{0:N2} of the row is the ground of the path" -f $share)

        # 2. The name of the file is written in the row.
        $file = Count-In $startShot $rowLeft $rowTop $rowWidth $rowHeight $FILE_RGB 8
        Write-Host "pixels of the name of the file: $file"
        Check 'the name of the file is written in the row' ($file -gt 10) "$file pixels of the name of the file"

        # 3. The name of the folder above it is written in the row as well.
        $folderName = Count-In $startShot $rowLeft $rowTop $rowWidth $rowHeight $FOLDER_RGB 8
        Write-Host "pixels of the names of the folders: $folderName"
        Check 'the names of the folders are written in the row' ($folderName -gt 10) "$folderName pixels of the names of the folders"

        # 4. Clicking the folder of the project in the path makes the tree of
        #    files look at that folder: the side bar shows a directory then.
        $directoriesBefore = Count-In $startShot `
            (To-Screen $places.SidebarLeft) (To-Screen $places.SidebarTop) `
            (To-Screen $places.SidebarWidth) (To-Screen $places.SidebarHeight) `
            $DIRECTORY_RGB 8
        Write-Host "pixels of the names of directories in the side bar before the click: $directoriesBefore"

        Focus-Editor $handle
        Click-At $handle ($places.FirstTextLeft + 4.0) ($places.RowTop + 12.0)
        $afterShot = Save-Window $handle $shotAfter
        Write-Host "saved $shotAfter"
        $directoriesAfter = Count-In $afterShot `
            (To-Screen $places.SidebarLeft) (To-Screen $places.SidebarTop) `
            (To-Screen $places.SidebarWidth) (To-Screen $places.SidebarHeight) `
            $DIRECTORY_RGB 8
        Write-Host "pixels of the names of directories in the side bar after the click: $directoriesAfter"
        Check 'clicking a folder of the path makes the tree of files look at it' `
            ($directoriesAfter -gt $directoriesBefore) `
            "the side bar went from $directoriesBefore pixels of directory names to $directoriesAfter pixels"

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
