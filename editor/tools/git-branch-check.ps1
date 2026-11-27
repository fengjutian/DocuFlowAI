# The window-level check of the branches and the remotes: the editor asks git
# what branches there are and where the remotes are, shows them, goes to the
# branch that is picked, makes a new one, and asks before it pushes anything.
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
# 2. The letter B shows the branches and the remotes: the branch the program
#    is on is in green with a mark in front of it, the other branches are in
#    the colour of the text, the name of the remote is in orange and the
#    address of it is in the blue of an address.
# 3. The row that is picked has a block of its own behind it.
# 4. The letter N opens the box that asks for the name of the branch, and
#    what is written in it is a branch: git has the branch, and the program
#    is on it.
# 5. The letter P opens the box that asks whether the branch may be pushed.
#    The escape key leaves it alone: the remote has nothing new.
# 6. Ctrl+Alt+B opens the branches straight away, without the panel of the
#    changes first: the command of the key does what its name says.
# 7. The letter B goes back to the changed files, and the escape key closes
#    the panel.
#
# What is not proved here, but in a unit test: the state line at the bottom
# says which branch the program is on. The state line is one line of text in
# one colour, and the pixels of it cannot say which words are in it.
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
$project = Join-Path $run 'git-branch-project'
$remote = Join-Path $run 'git-branch-remote'
$source = Join-Path $project 'notes.txt'
$shotPanel = Join-Path $logs 'git-branch-panel.png'
$shotBranches = Join-Path $logs 'git-branch-branches.png'
$shotNew = Join-Path $logs 'git-branch-new.png'
$shotPush = Join-Path $logs 'git-branch-push.png'
$shotCommand = Join-Path $logs 'git-branch-command.png'
$report = Join-Path $logs 'git-branch-check.json'

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
$VK_ALT = 0x12
$VK_ESCAPE = 0x1B
$VK_RETURN = 0x0D
$VK_DOWN = 0x28
$VK_B = 0x42
$VK_E = 0x45
$VK_G = 0x47
$VK_H = 0x48
$VK_N = 0x4E
$VK_O = 0x4F
$VK_P = 0x50
$VK_T = 0x54
$VK_W = 0x57
$VK_MINUS = 0xBD

function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
# The colour of a block of colour on the screen. A block goes through the
# conversion that turns the colour of the code into the colour of the screen;
# the text does not.
function To-Pixel([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round((To-Srgb $_) * 255) })
}
# The ground of the panel and the ground of the box are blocks of colour. The
# names of the branches, of the remotes and the address of a remote are text,
# and the text is drawn in the colour the code writes.
$GROUND_RGB = To-Pixel @(0.055, 0.063, 0.082)
$BOX_RGB = To-Pixel @(0.10, 0.12, 0.16)
$SELECTED_RGB = To-Pixel @(0.18, 0.30, 0.52)
# The branch the program is on is in green, with a mark in front of it.
$CURRENT_RGB = To-Bytes @(0.42, 0.78, 0.56)
# The name of a remote is in orange.
$REMOTE_NAME_RGB = To-Bytes @(0.93, 0.74, 0.42)
# The address of a remote is in the blue of an address.
$REMOTE_ADDRESS_RGB = To-Bytes @(0.62, 0.70, 0.86)

# The places of the panel, in the pixels of the window.
#
# The list of the branches is on the left, and the box that asks for something
# floats over the text of the document.
$LIST_X = 16
$LIST_WIDTH = 360
$ROW_Y = 42
$ROW_HEIGHT = 22
# The column that says which remote it is, and the address beside it.
$REMOTE_NAME_WIDTH = 96
$ADDRESS_X = 116
$BOX_X = 244
$BOX_Y = 86
$BOX_WIDTH = 380
$BOX_HEIGHT = 40

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

# The repository the editor looks at, and the remote it has.
#
# The remote is a bare repository on this machine: the check pushes to it
# without asking anything of the network.
if (Test-Path $project) { Remove-Item -Recurse -Force $project }
if (Test-Path $remote) { Remove-Item -Recurse -Force $remote }
New-Item -ItemType Directory -Force -Path $project | Out-Null
New-Item -ItemType Directory -Force -Path $remote | Out-Null
git -C $remote init --quiet --bare
git -C $project init --quiet
git -C $project config user.email check@example.com
git -C $project config user.name check
[System.IO.File]::WriteAllText($source, "the first line`n", (New-Object System.Text.UTF8Encoding($false)))
git -C $project add notes.txt
git -C $project commit -m 'the first one' --quiet
git -C $project branch the-other
git -C $project remote add origin $remote
git -C $project push origin HEAD:refs/heads/main --quiet
Write-Output "the file that is opened: $source"
Write-Output "the branches before the check: $(git -C $project branch --list)"

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

# Writes a name, one key at a time.
function Send-Text([string]$text) {
    foreach ($letter in $text.ToCharArray()) {
        $code = [int][char]::ToUpperInvariant($letter)
        if ($letter -eq '-') { $code = $VK_MINUS }
        Send-Key $code
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
# The rectangle is given in the pixels of the window, the way the places of the
# panel are.
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
# The rectangle is given in the pixels of the screen, the way the places of the
# panel are.
function Count-In($shot, [int]$left, [int]$top, [int]$width, [int]$height, [int[]]$rgb, [int]$tolerance) {
    return Count-Color $shot.Bitmap ($shot.Left + $left) ($shot.Top + $top) $width $height $rgb $tolerance
}

function Start-Editor([string[]]$arguments) {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 500
    $process = Start-Process -FilePath $runExe -ArgumentList $arguments `
        -RedirectStandardOutput (Join-Path $logs 'git-branch-app-out.log') `
        -RedirectStandardError (Join-Path $logs 'git-branch-app-err.log') -PassThru
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
        $addressLeft = To-Screen $ADDRESS_X
        $boxLeft = To-Screen $BOX_X
        $boxTop = To-Screen $BOX_Y
        $boxWidth = To-Screen $BOX_WIDTH
        $boxHeight = To-Screen $BOX_HEIGHT

        $ground = Count-In $panel 0 0 $panel.Width $panel.Height $GROUND_RGB 4
        Write-Host "pixels of the ground of the panel: $ground"
        Check 'the panel covers the window' ($ground -gt 100000) "$ground pixels of the ground"

        # The letter B shows the branches and the remotes.
        $previous = Focus-Editor $handle
        Send-Key $VK_B
        Start-Sleep -Seconds 3

        $branches = Save-Window $handle $shotBranches
        Write-Host "saved $shotBranches"

        $ground = Count-In $branches 0 0 $branches.Width $branches.Height $GROUND_RGB 4
        Check 'the branches are on the screen' ($ground -gt 100000) "$ground pixels of the ground"

        # The branch the program is on is in green, with a mark in front of it.
        $current = Count-In $branches $listLeft $rowTop $listWidth $rowHeight $CURRENT_RGB 12
        Write-Host "pixels of the branch the program is on: $current"
        Check 'the panel says which branch the program is on' ($current -gt 20) "$current pixels of the branch"

        # The name of the remote is in orange, and the address of it is in the
        # blue of an address, in the column beside it.
        $name = Count-In $branches $listLeft $rowTop (To-Screen $REMOTE_NAME_WIDTH) ($rowHeight * 4) $REMOTE_NAME_RGB 10
        Write-Host "pixels of the name of the remote: $name"
        Check 'the panel names the remote' ($name -gt 20) "$name pixels of the name"

        $address = Count-In $branches $addressLeft $rowTop ($listWidth - (To-Screen $REMOTE_NAME_WIDTH)) ($rowHeight * 4) $REMOTE_ADDRESS_RGB 8
        Write-Host "pixels of the address of the remote: $address"
        Check 'the panel says where the remote is' ($address -gt 20) "$address pixels of the address"

        # The row that is picked has a block of its own behind it.
        $picked = Count-In $branches $listLeft $rowTop $listWidth $rowHeight $SELECTED_RGB 6
        Write-Host "pixels of the block behind the picked row: $picked"
        Check 'the row that is picked has a block behind it' ($picked -gt 200) "$picked pixels of the block"

        # The letter N opens the box that asks for the name of the branch.
        $previous = Focus-Editor $handle
        Send-Key $VK_N
        Start-Sleep -Seconds 2

        $asking = Save-Window $handle $shotNew
        Write-Host "saved $shotNew"

        $box = Count-In $asking $boxLeft $boxTop $boxWidth $boxHeight $BOX_RGB 4
        Write-Host "pixels of the box that asks: $box"
        Check 'the box that asks for the name of the branch is on the screen' ($box -gt 2000) "$box pixels of the box"

        # What is written in the box is the name of a branch.
        Send-Text 'the-new'
        Start-Sleep -Milliseconds 500
        Send-Key $VK_RETURN
        Start-Sleep -Seconds 3

        $made = Save-Window $handle $shotNew
        $branches = git -C $project branch --list
        Write-Host "the branches after the new one: $branches"
        Check 'the name in the box is a branch' ($branches -match 'the-new') "$branches"

        $current = Count-In $made $listLeft $rowTop $listWidth $rowHeight $CURRENT_RGB 12
        Write-Host "pixels of the branch the program is on: $current"
        Check 'the program is on the branch that was made' ($current -gt 20) "$current pixels of the branch"

        # The letter P asks whether the branch may be pushed.
        $previous = Focus-Editor $handle
        Send-Key $VK_P
        Start-Sleep -Seconds 2

        $pushing = Save-Window $handle $shotPush
        Write-Host "saved $shotPush"

        $box = Count-In $pushing $boxLeft $boxTop $boxWidth $boxHeight $BOX_RGB 4
        Write-Host "pixels of the box that asks about the push: $box"
        Check 'the push is asked for first' ($box -gt 2000) "$box pixels of the box"

        # The escape key leaves it alone: nothing is pushed.
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 2

        $there = git -C $remote branch --list
        Write-Host "the branches of the remote: $there"
        Check 'nothing is pushed without the answer' (-not ($there -match 'the-new')) "$there"

        # The letter B goes back to the changed files, and the escape key
        # closes the panel.
        Send-Key $VK_B
        Start-Sleep -Seconds 2
        Send-Key $VK_ESCAPE
        Start-Sleep -Seconds 2

        # Ctrl+Alt+B opens the branches straight away.
        $previous = Focus-Editor $handle
        Send-Chord @($VK_CONTROL, $VK_ALT) $VK_B
        Start-Sleep -Seconds 3

        $command = Save-Window $handle $shotCommand
        Write-Host "saved $shotCommand"

        $ground = Count-In $command 0 0 $command.Width $command.Height $GROUND_RGB 4
        Write-Host "pixels of the ground of the panel: $ground"
        Check 'the command of the key opens the branches' ($ground -gt 100000) "$ground pixels of the ground"

        $current = Count-In $command $listLeft $rowTop $listWidth $rowHeight $CURRENT_RGB 12
        Write-Host "pixels of the branch the program is on: $current"
        Check 'the branches are on the screen' ($current -gt 20) "$current pixels of the branch"
    }
    finally {
        Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
        if ($previous -ne [IntPtr]::Zero) { [Win32]::SetForegroundWindow($previous) | Out-Null }
    }

    $failed = @($script:checks | Where-Object { -not $_.ok })
    $summary = @{
        checks = $script:checks
        failed = $failed.Count
        total = $script:checks.Count
    }
    $summary | ConvertTo-Json -Depth 5 | Set-Content -Path $report -Encoding UTF8
    Write-Host "the report is in $report"
    if ($failed.Count -gt 0) {
        Write-Host "$($failed.Count) of $($script:checks.Count) checks failed"
        exit 1
    }
    Write-Host "all $($script:checks.Count) checks passed"
}
catch {
    Write-Host "the check did not finish: $_"
    exit 2
}
