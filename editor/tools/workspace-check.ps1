# The window-level check of the workspace: a project that has more than one
# folder is written down in a file of its own, and the file panel shows every
# one of those folders.
#
# The project is a folder of its own, outside the workspace, so the check does
# not touch the code of the editor.
#
# What the check proves, in the window and not in a unit test:
#
# 1. The file panel has a row for every folder of the workspace, and the files
#    of a folder are below its row.
# 2. The title of the section is the name of the workspace, not the name of the
#    section.
# 3. The right of the status bar names the workspace that is open.
# 4. Clicking a folder of the workspace closes it: the files under it go away.
#
# The places are worked out the way the editor works them out: the bar of the
# activities (48), the side bar (220), the title of a section (28), a row of
# the file panel (22), the indent of a level (14) and the room the mark of a
# row takes (15).
#
# The script is written without a byte order mark on purpose: it has no
# Chinese in it, and a script with Chinese in it has to have one, or Windows
# PowerShell reads it as something else.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$run = Join-Path $env:LOCALAPPDATA 'CodeWeaver\check'
$project = Join-Path $run 'workspace-project'
$workspaceFile = Join-Path $project 'proj.code-workspace'
$plainFile = Join-Path $project 'alpha\one.txt'
$shotWorkspace = Join-Path $logs 'workspace-on.png'
$shotPlain = Join-Path $logs 'workspace-off.png'
$shotClosed = Join-Path $logs 'workspace-closed.png'
$report = Join-Path $logs 'workspace-check.json'

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

# The places of the parts of the window, in the pixels of the window.
$ACTIVITY_BAR = 48.0
$SIDEBAR = 220.0

# The panel of the file panel: the title of the section, and then the rows.
$SECTION_TITLE = 28.0
$ROW = 22.0
# How much a level of the tree is indented, and the room the mark of a row
# takes before the name.
$INDENT = 14.0
$MARK = 15.0

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

# The files of the check: two folders, a file in each of them, and the
# workspace file that names the two folders.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path (Join-Path $project 'alpha') | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $project 'beta') | Out-Null
[System.IO.File]::WriteAllText(
    (Join-Path $project 'alpha\one.txt'),
    "one`n",
    (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText(
    (Join-Path $project 'beta\two.txt'),
    "two`n",
    (New-Object System.Text.UTF8Encoding($false)))
[System.IO.File]::WriteAllText(
    $workspaceFile,
    "{`n  `"folders`": [`n    { `"path`": `"alpha`" },`n    { `"path`": `"beta`" }`n  ]`n}`n",
    (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the workspace file: $workspaceFile"

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
        -RedirectStandardOutput (Join-Path $logs 'workspace-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'workspace-app-err.log') -PassThru
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

    # The places of the rows of the file panel.
    #
    # The first row is the first folder of the workspace, the second row is the
    # file under it, the third row is the second folder and the fourth row is
    # the file under that one.
    $panelLeft = $ACTIVITY_BAR
    $nameOfFolder = $panelLeft + 8.0 + $MARK
    $nameOfFile = $panelLeft + 8.0 + $INDENT + $MARK
    $firstFolder = $SECTION_TITLE
    $firstFile = $SECTION_TITLE + $ROW
    $secondFolder = $SECTION_TITLE + $ROW * 2
    $secondFile = $SECTION_TITLE + $ROW * 3

    # The editor is started with a plain file first: that is the state without
    # a workspace, and the title of the section is the name of the section.
    $plain = Start-Editor @($plainFile)
    $plainHandle = $plain.Handle
    try {
        Focus-Editor $plainHandle
        $shot = Save-Window $plainHandle $shotPlain
        Write-Host "saved $shotPlain"
        $width = [int]($shot.Width / $script:Factor)
        $height = [int]($shot.Height / $script:Factor)
        Write-Host "the window is $width by $height pixels"
    }
    finally {
        Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
        Start-Sleep -Milliseconds 500
    }

    # Now the editor is started with the workspace file: the file panel has a
    # row for every folder of the workspace.
    $editor = Start-Editor @($workspaceFile)
    $handle = $editor.Handle
    try {
        Write-Host "editor window: $handle, scale $($script:Factor)"
        Focus-Editor $handle

        $one = Save-Window $handle $shotWorkspace
        Write-Host "saved $shotWorkspace"
        $width = [int]($one.Width / $script:Factor)
        $height = [int]($one.Height / $script:Factor)
        Write-Host "the window is $width by $height pixels"

        # The title of the section, and the right of the status bar, where the
        # workspace is named.
        $titleOne = Region $one ($panelLeft + 8) 4 160 22
        $statusOne = Region $one ($width - 130) ($height - 20) 118 16

        # The names of the two folders of the workspace, and the files under
        # them.
        $folderOne = Region $one $nameOfFolder ($firstFolder + 2) 120 18
        $folderTwo = Region $one $nameOfFolder ($secondFolder + 2) 120 18
        $fileTwo = Region $one $nameOfFile ($secondFile + 2) 120 18

        $inkFolderOne = Count-Ink $one.Bitmap $folderOne.Left $folderOne.Top $folderOne.Width $folderOne.Height
        $inkFolderTwo = Count-Ink $one.Bitmap $folderTwo.Left $folderTwo.Top $folderTwo.Width $folderTwo.Height
        $inkFileTwo = Count-Ink $one.Bitmap $fileTwo.Left $fileTwo.Top $fileTwo.Width $fileTwo.Height
        Write-Host "the folders of the workspace: $inkFolderOne pixels of ink in the first one, $inkFolderTwo in the second one"
        Write-Host "the file under the second folder: $inkFileTwo pixels of ink"

        Check 'the file panel has a row for every folder of the workspace' `
            ($inkFolderOne -gt 5 -and $inkFolderTwo -gt 5) `
            "$inkFolderOne pixels of ink in the name of the first folder, $inkFolderTwo in the name of the second one"

        Check 'the files of a folder are below the row of that folder' `
            ($inkFileTwo -gt 5) `
            "$inkFileTwo pixels of ink in the name of the file under the second folder"

        # The title of the section and the right of the status bar are
        # compared with the state without a workspace.
        $plainShot = [System.Drawing.Bitmap]::FromFile($shotPlain)
        try {
            $plainTitle = Region @{ Left = $one.Left; Top = $one.Top } ($panelLeft + 8) 4 160 22
            $plainStatus = Region @{ Left = $one.Left; Top = $one.Top } ($width - 130) ($height - 20) 118 16
            $saidTitle = Count-Difference $plainShot $one.Bitmap $plainTitle.Left $plainTitle.Top $plainTitle.Width $plainTitle.Height
            $saidStatus = Count-Difference $plainShot $one.Bitmap $plainStatus.Left $plainStatus.Top $plainStatus.Width $plainStatus.Height
        }
        finally {
            $plainShot.Dispose()
        }
        Write-Host "the title of the section: $saidTitle pixels are different without a workspace"
        Write-Host "the right of the status bar: $saidStatus pixels are different without a workspace"

        Check 'the title of the section is the name of the workspace' `
            ($saidTitle -gt 0) `
            "$saidTitle pixels of the title of the section are different from the state without a workspace"

        Check 'the right of the status bar names the workspace' `
            ($saidStatus -gt 0) `
            "$saidStatus pixels of the right of the status bar are different from the state without a workspace"

        # Clicking the first folder of the workspace closes it: the files under
        # it go away, and the rows below move up.
        Click-At $one ($nameOfFolder + 10) ($firstFolder + 10)
        Focus-Editor $handle
        Start-Sleep -Seconds 2

        $two = Save-Window $handle $shotClosed
        Write-Host "saved $shotClosed"
        $fileTwoAfter = Region $two $nameOfFile ($secondFile + 2) 120 18
        $inkFileTwoAfter = Count-Ink $two.Bitmap $fileTwoAfter.Left $fileTwoAfter.Top $fileTwoAfter.Width $fileTwoAfter.Height
        Write-Host "the file under the second folder: $inkFileTwoAfter pixels of ink after the folder was closed"
        Check 'clicking a folder of the workspace closes it' `
            ($inkFileTwoAfter -lt $inkFileTwo) `
            "$inkFileTwo pixels of ink in the file under the second folder before the click, $inkFileTwoAfter after"

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
