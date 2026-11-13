# The window-level check of the panel of the changes: the editor asks git what
# has changed, shows the files and their difference, stages what the user
# picks and commits what was staged.
#
# The project is a repository of its own, outside the workspace, so the check
# does not touch the code of the editor. It is also outside the workspace on
# purpose: the workspace carries a low integrity label, and a low integrity
# process cannot start the `git` of the machine.
#
# What the check proves, in the window and not in a unit test:
#
# 1. Ctrl+Shift+G opens the panel of the changes, and the panel covers the
#    window.
# 2. The panel lists the file that changed, says what happened to it, and
#    shows the difference of the file.
# 3. The letter S stages the file that is picked, and the panel says so at
#    once: the file is in a colour of its own, and git agrees.
# 4. The letter U takes the file out of the staging area again, and git
#    agrees: the file is a change that is not staged any more.
# 5. The letter C opens the box that asks for the message of the commit, and
#    what is written in it is committed: git has the commit, and the panel
#    does not list the file any more. The message has a space in it: the
#    space bar has to reach the box.
# 6. The letter L shows the commits of the repository: the number that names
#    the commit and its message, and what the commit changed.
# 7. The arrow keys pick another commit, and the escape key goes back to the
#    changed files without closing the panel.
# 8. The escape key closes the panel, and the editor is there again.
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
$project = Join-Path $run 'git-project'
$source = Join-Path $project 'notes.txt'
$shotPanel = Join-Path $logs 'git-panel.png'
$shotStaged = Join-Path $logs 'git-staged.png'
$shotUnstaged = Join-Path $logs 'git-unstaged.png'
$shotBox = Join-Path $logs 'git-box.png'
$shotCommitted = Join-Path $logs 'git-committed.png'
$shotCommits = Join-Path $logs 'git-commits.png'
$shotOlder = Join-Path $logs 'git-older-commit.png'
$shotFiles = Join-Path $logs 'git-files-again.png'
$shotClosed = Join-Path $logs 'git-closed.png'
$report = Join-Path $logs 'git-check.json'

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
$VK_SPACE = 0x20
$VK_DOWN = 0x28
$VK_S = 0x53
$VK_C = 0x43
$VK_D = 0x44
$VK_E = 0x45
$VK_G = 0x47
$VK_H = 0x48
$VK_K = 0x4B
$VK_L = 0x4C
$VK_N = 0x4E
$VK_O = 0x4F
$VK_T = 0x54
$VK_U = 0x55

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The ground of the panel, the colour of the path of a file, the colour of
# the state of a file that is not staged and of one that is, the colour of a
# line that was put in, and the colour of the box that asks for the message
# of the commit.
$GROUND_RGB = To-Bytes @(0.055, 0.063, 0.082)
$PATH_RGB = To-Bytes @(0.86, 0.89, 0.94)
$NEW_RGB = To-Bytes @(0.93, 0.74, 0.42)
$STAGED_RGB = To-Bytes @(0.42, 0.78, 0.56)
$ADDED_RGB = To-Bytes @(0.52, 0.82, 0.58)
$BOX_RGB = To-Bytes @(0.10, 0.12, 0.16)
# The number that names a commit is in a colour of its own, the same green
# as the state of a staged file.
$COMMIT_ID_RGB = To-Bytes @(0.42, 0.78, 0.56)

# The places of the panel, in the pixels of the window.
#
# The list of the files is on the left, the difference on the right, and the
# box that asks for the message of the commit floats over the text of the
# document.
$LIST_X = 16
$LIST_WIDTH = 360
$ROW_Y = 42
$ROW_HEIGHT = 22
$LABEL_WIDTH = 92
$DIFF_X = 392
$BOX_X = 244
$BOX_Y = 86
$BOX_WIDTH = 380
$BOX_HEIGHT = 40
# The column that says which commit it is, and the message beside it.
$COMMIT_ID_WIDTH = 72
$MESSAGE_X = 92
$MESSAGE_WIDTH = 280
# The second row of the list.
$SECOND_ROW_Y = 64

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}

# The check drives the editor that is on disk, so that one has to be the one
# the sources describe.
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
# with fewer rights than the programs it starts, and it cannot start the git
# of the machine at all. The copy runs with the rights a program normally
# has.
New-Item -ItemType Directory -Force -Path $run | Out-Null
$runExe = Join-Path $run 'codeweaver-editor.exe'
Copy-Item $exe $runExe -Force
Write-Output "the editor runs from $runExe"

New-Item -ItemType Directory -Force -Path $logs | Out-Null

# The repository the editor looks at.
#
# One file is in it, and the file is changed after it was committed: that is
# the state the panel has to show.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
New-Item -ItemType Directory -Force -Path $project | Out-Null
git -C $project init --quiet
git -C $project config user.email check@example.com
git -C $project config user.name check
[System.IO.File]::WriteAllText($source, "the first line`n", (New-Object System.Text.UTF8Encoding($false)))
git -C $project add notes.txt
git -C $project commit -m 'the first one' --quiet
[System.IO.File]::WriteAllText($source, "the first line`nthe second line`n", (New-Object System.Text.UTF8Encoding($false)))
Write-Output "the file that is opened: $source"
Write-Output "git says before the check: $(git -C $project status --porcelain)"

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
        -RedirectStandardOutput (Join-Path $logs 'git-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'git-app-err.log') -PassThru
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

        # Ctrl+Shift+G opens the panel of the changes.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_SHIFT) $VK_G
        Start-Sleep -Seconds 3

        $panel = Save-Window $handle $shotPanel
        Write-Host "saved $shotPanel"

        # The places of the panel, in the pixels of the screen.
        $listLeft = To-Screen $LIST_X
        $rowTop = To-Screen $ROW_Y
        $rowHeight = To-Screen $ROW_HEIGHT
        $listWidth = To-Screen $LIST_WIDTH
        $labelWidth = To-Screen $LABEL_WIDTH
        $diffLeft = To-Screen $DIFF_X
        $boxLeft = To-Screen $BOX_X
        $boxTop = To-Screen $BOX_Y
        $boxWidth = To-Screen $BOX_WIDTH
        $boxHeight = To-Screen $BOX_HEIGHT
        $commitIdWidth = To-Screen $COMMIT_ID_WIDTH
        $messageLeft = To-Screen $MESSAGE_X
        $messageWidth = To-Screen $MESSAGE_WIDTH
        $secondRowTop = To-Screen $SECOND_ROW_Y

        $ground = Count-In $panel 0 0 $panel.Width $panel.Height $GROUND_RGB 4
        Write-Host "pixels of the ground of the panel: $ground"
        Check 'the panel covers the window' ($ground -gt 100000) "$ground pixels of the ground"

        $path = Count-In $panel $listLeft $rowTop $listWidth $rowHeight $PATH_RGB 14
        Write-Host "pixels of the path of the file: $path"
        Check 'the panel lists the file that changed' ($path -gt 20) "$path pixels of the path"

        $new = Count-In $panel $listLeft $rowTop $labelWidth $rowHeight $NEW_RGB 14
        Write-Host "pixels of the state of the file: $new"
        Check 'the panel says what happened to the file' ($new -gt 20) "$new pixels of the state"

        $added = Count-In $panel $diffLeft $rowTop ($panel.Width - $diffLeft) (To-Screen 120) $ADDED_RGB 14
        Write-Host "pixels of the line that was put in: $added"
        Check 'the panel shows the difference of the file' ($added -gt 20) "$added pixels of the difference"

        # The letter S stages the file that is picked.
        $previous = Focus-Editor $handle
        Send-Key $VK_S
        Start-Sleep -Seconds 3

        $stagedShot = Save-Window $handle $shotStaged
        Write-Host "saved $shotStaged"
        $staged = Count-In $stagedShot $listLeft $rowTop $labelWidth $rowHeight $STAGED_RGB 14
        Write-Host "pixels of the state of a staged file: $staged"
        Check 'the letter S stages the file' ($staged -gt 20) "$staged pixels of the state"

        $said = git -C $project status --porcelain
        Write-Host "git says: $said"
        Check 'git knows that the file is staged' ($said -match '^M') "git says: $said"

        # The letter U takes the file out of the staging area again.
        $previous = Focus-Editor $handle
        Send-Key $VK_U
        Start-Sleep -Seconds 3

        $unstagedShot = Save-Window $handle $shotUnstaged
        Write-Host "saved $shotUnstaged"
        $unstaged = Count-In $unstagedShot $listLeft $rowTop $labelWidth $rowHeight $NEW_RGB 14
        Write-Host "pixels of the state of a file that is not staged: $unstaged"
        Check 'the letter U takes the file out of the staging area' ($unstaged -gt 20) "$unstaged pixels of the state"

        $said = git -C $project status --porcelain
        Write-Host "git says: $said"
        Check 'git knows that the file is not staged any more' ($said -match '^ M') "git says: $said"

        # The file is staged again, so that the commit can be made.
        $previous = Focus-Editor $handle
        Send-Key $VK_S
        Start-Sleep -Seconds 3

        $said = git -C $project status --porcelain
        Write-Host "git says: $said"
        Check 'the letter S stages the file again' ($said -match '^M') "git says: $said"

        # The letter C opens the box that asks for the message of the commit.
        $previous = Focus-Editor $handle
        Send-Key $VK_C
        Start-Sleep -Seconds 2

        $boxShot = Save-Window $handle $shotBox
        Write-Host "saved $shotBox"
        $box = Count-In $boxShot $boxLeft $boxTop $boxWidth $boxHeight $BOX_RGB 6
        Write-Host "pixels of the box that asks for the message: $box"
        Check 'the letter C asks for the message of the commit' ($box -gt 500) "$box pixels of the box"

        # The message is written and the commit is made.
        #
        # The message has a space in it: the space bar has to reach the box
        # that asks for the message.
        $previous = Focus-Editor $handle
        Send-Key $VK_T
        Send-Key $VK_H
        Send-Key $VK_E
        Send-Key $VK_SPACE
        Send-Key $VK_S
        Send-Key $VK_E
        Send-Key $VK_C
        Send-Key $VK_O
        Send-Key $VK_N
        Send-Key $VK_D
        Send-Key $VK_SPACE
        Send-Key $VK_O
        Send-Key $VK_N
        Send-Key $VK_E
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 3

        $committedShot = Save-Window $handle $shotCommitted
        Write-Host "saved $shotCommitted"
        $left = Count-In $committedShot $listLeft $rowTop $listWidth $rowHeight $PATH_RGB 14
        Write-Host "pixels of the path of the file after the commit: $left"
        Check 'the panel does not list the file any more' ($left -lt 10) "$left pixels of the path"

        $log = git -C $project log --oneline
        Write-Host "git log: $log"
        Check 'the commit is really made' (($log -split "`n").Count -eq 2 -and ($log -match 'second')) "git log: $log"

        # The letter L shows the commits of the repository.
        $previous = Focus-Editor $handle
        Send-Key $VK_L
        Start-Sleep -Seconds 3

        $commitsShot = Save-Window $handle $shotCommits
        Write-Host "saved $shotCommits"
        $number = Count-In $commitsShot $listLeft $rowTop $commitIdWidth $rowHeight $COMMIT_ID_RGB 14
        Write-Host "pixels of the number of the commit: $number"
        Check 'the panel shows the number of the commit' ($number -gt 20) "$number pixels of the number"

        $message = Count-In $commitsShot $messageLeft $rowTop $messageWidth $rowHeight $PATH_RGB 14
        Write-Host "pixels of the message of the commit: $message"
        Check 'the panel shows the message of the commit' ($message -gt 20) "$message pixels of the message"

        $shown = Count-In $commitsShot $diffLeft $rowTop ($commitsShot.Width - $diffLeft) (To-Screen 400) $ADDED_RGB 14
        Write-Host "pixels of what the commit changed: $shown"
        Check 'the panel shows what the commit changed' ($shown -gt 20) "$shown pixels of the difference"

        # The arrow keys pick another commit.
        $previous = Focus-Editor $handle
        Send-Key $VK_DOWN
        Start-Sleep -Seconds 3

        $olderShot = Save-Window $handle $shotOlder
        Write-Host "saved $shotOlder"
        $older = Count-In $olderShot $messageLeft $secondRowTop $messageWidth $rowHeight $PATH_RGB 14
        Write-Host "pixels of the message of the older commit: $older"
        Check 'the arrow keys pick another commit' ($older -gt 20) "$older pixels of the message"

        # The escape key goes back to the changed files, and the panel stays
        # open: the commits are a page of the panel, not the panel itself.
        $previous = Focus-Editor $handle
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 3

        $filesShot = Save-Window $handle $shotFiles
        Write-Host "saved $shotFiles"
        $stillGround = Count-In $filesShot 0 0 $filesShot.Width $filesShot.Height $GROUND_RGB 4
        Write-Host "pixels of the ground after the escape in the commits: $stillGround"
        Check 'the escape key in the commits keeps the panel open' ($stillGround -gt 100000) "$stillGround pixels of the ground"

        $gone = Count-In $filesShot $listLeft $rowTop $commitIdWidth $rowHeight $COMMIT_ID_RGB 14
        Write-Host "pixels of the number of the commit after the escape: $gone"
        Check 'the escape key goes back to the changed files' ($gone -lt 10) "$gone pixels of the number"

        # The escape key closes the panel.
        $previous = Focus-Editor $handle
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 2

        $closed = Save-Window $handle $shotClosed
        Write-Host "saved $shotClosed"
        $closedGround = Count-In $closed 0 0 $closed.Width $closed.Height $GROUND_RGB 4
        Write-Host "pixels of the ground after the escape: $closedGround"
        Check 'the escape key closes the panel' ($closedGround -lt 10000) "$closedGround pixels of the ground"

        $result = @{
            checks = $script:checks
            project = $project
            pixels = @{
                ground = $ground
                path = $path
                state = $new
                difference = $added
                staged = $staged
                unstaged = $unstaged
                box = $box
                pathAfterCommit = $left
                commitNumber = $number
                commitMessage = $message
                commitDifference = $shown
                olderCommitMessage = $older
                groundAfterEscapeInCommits = $stillGround
                commitNumberAfterEscape = $gone
                groundAfterEscape = $closedGround
            }
            shots = @($shotPanel, $shotStaged, $shotUnstaged, $shotBox, $shotCommitted, $shotCommits, $shotOlder, $shotFiles, $shotClosed)
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
