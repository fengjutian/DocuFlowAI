# The mouse messages are posted to the editor's window, so the check never
# takes the cursor away from whoever is using the machine. winit reads the
# position out of the message, so the editor sees the click where it was
# sent.
#
# Two things fight that:
#
# - The real mouse. If it is over the editor, its own messages arrive after
#   the posted ones and the editor follows the real cursor instead of the
#   message. So the cursor is moved out of the way first, and put back
#   afterwards.
# - The scale. The editor works in logical pixels and the screen has
#   physical ones, and a script that is not aware of the scale gets
#   coordinates in a third space. So the script asks the system how aware
#   it is, and turns every measurement into physical pixels.
#
# The find bar is checked in a second run of the editor, started with the
# search already open: the editor reads the modifier keys from the real
# keyboard state, and neither a posted key message nor a key that was
# injected at the system level can be relied on to open the bar.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$fixture = Join-Path $PSScriptRoot 'fixtures\sample.txt'
$sample = Join-Path $logs 'sample.txt'
$rustFixture = Join-Path $PSScriptRoot 'fixtures\sample.rs'
$rustSample = Join-Path $logs 'sample.rs'
$shot = Join-Path $logs 'window-check.png'
$findShot = Join-Path $logs 'window-check-find.png'
$syntaxShot = Join-Path $logs 'window-check-syntax.png'
$lspShot = Join-Path $logs 'window-check-lsp.png'
$completionShot = Join-Path $logs 'window-check-completion.png'
# The language server of the check is not a real one: it says that the
# first line of the file is wrong, so that the underline can be looked for
# without waiting for a real server to think about the file.
$fakeServer = Join-Path $root 'target\debug\fake-lsp-server.exe'

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
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
  [DllImport("user32.dll")] public static extern bool SetProcessDpiAwarenessContext(IntPtr value);
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDpiAwarenessContext();

  public delegate bool EnumProc(IntPtr h, IntPtr extra);
  public struct RECT { public int Left, Top, Right, Bottom; }
  public struct POINT { public int X, Y; }

  // The editor's window: the visible window of the process that has the
  // editor's title and a client area.
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

# The editor is aware of the scale; the script tries to be as well. When it
# is not, the system hands out coordinates in the space of the system scale,
# and every measurement has to be turned into physical pixels.
[Win32]::SetProcessDpiAwarenessContext([IntPtr](-4)) | Out-Null
$awareness = [Win32]::GetThreadDpiAwarenessContext()
$scriptIsAware = ($awareness -eq [IntPtr](-4)) -or ($awareness -eq [IntPtr](-3))
Write-Output "the script is scale aware: $scriptIsAware"

$TOPMOST = [IntPtr](-1)
$SWP_SHOWWINDOW = 0x0040

$WM_MOUSEMOVE = 0x0200
$WM_LBUTTONDOWN = 0x0201
$WM_LBUTTONUP = 0x0202
$MK_LBUTTON = 0x0001

# Where the body sits inside the client area, and how tall a line is.
$BODY_X = 56
$BODY_Y = 52
$LINE_HEIGHT = 22

# The colours the editor draws its parts in, as the code writes them.
#
# The renderer draws in linear values and the surface turns them into sRGB,
# so the colour that shows up on the screen is the sRGB version of the one
# in the code. The check looks for those colours and not for "something
# blue", because the grey text of the status line is blue enough to pass
# such a test.
function To-Srgb([double]$value) {
    if ($value -le 0.0031308) { return $value * 12.92 }
    return 1.055 * [Math]::Pow($value, 1.0 / 2.4) - 0.055
}
function To-Screen([double[]]$linear) {
    return @($linear | ForEach-Object { [int][Math]::Round((To-Srgb $_) * 255) })
}
# The text is drawn in the colour the code writes: the text pass does not
# go through the conversion that the blocks of colour do.
function To-Bytes([double[]]$value) {
    return @($value | ForEach-Object { [int][Math]::Round($_ * 255) })
}
$SELECTION_RGB = To-Screen @(0.20, 0.33, 0.55)
$ACCENT_RGB = To-Screen @(0.30, 0.58, 0.96)
$MATCH_RGB = To-Screen @(0.58, 0.46, 0.11)
# The colours of the syntax: the keyword of a language and the text of a
# string.
$KEYWORD_RGB = To-Bytes @(0.76, 0.36, 0.90)
$STRING_RGB = To-Bytes @(0.68, 0.82, 0.42)
# The underline of a thing the language server found wrong.
$ERROR_RGB = To-Screen @(0.75, 0.10, 0.10)
# The row of the list of completions that is picked.
$COMPLETION_RGB = To-Screen @(0.18, 0.30, 0.52)

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}
if (-not (Test-Path $fakeServer)) {
    throw "the language server of the check is not built; run 'cargo build -p codeweaver-lsp --bin fake-lsp-server' first"
}
# The check drives the editor that is on disk, so that one has to be the one
# the sources describe. `cargo test` does not build it: it only builds the
# test harness next to it.
#
# Only the crates the editor is made of are looked at: the examples and the
# tests of a crate are not part of the editor, and a change in one of them
# says nothing about the editor being out of date.
$editorCrates = @('app', 'platform', 'render', 'text', 'syntax', 'lsp', 'ui', 'workspace', 'editor-core', 'agent-acp', 'agent-ui')
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
# The fixture is read as UTF-8 on purpose: PowerShell reads a script without
# a byte order mark as ANSI, so the text must not travel through the script.
Copy-Item -Path $fixture -Destination $sample -Force
Copy-Item -Path $rustFixture -Destination $rustSample -Force

# The scale of the monitor the editor is on, and the way from the
# coordinates of the script to the physical pixels of the screen.
$script:Factor = 1.0
function To-Physical([double]$value) {
    if ($scriptIsAware) { return $value }
    return $value * $script:Factor
}

function Send-Mouse([IntPtr]$handle, [uint32]$message, [int]$x, [int]$y, [int]$extra) {
    $packed = [int64](($y -band 0xFFFF) -shl 16) -bor ($x -band 0xFFFF) -bor $extra
    [Win32]::PostMessage($handle, $message, [IntPtr]0, [IntPtr]$packed) | Out-Null
}

function Save-Window([IntPtr]$handle, [string]$path) {
    # A minimised window has no client area to draw into, and a window that
    # was never shown does not redraw. Restore it and give it a new size, so
    # the editor paints the frame the check is about.
    #
    # Something else on the machine now and then minimises the window a
    # moment after it appears, so the restore is retried: a check that fails
    # because the window was not on screen says nothing about the editor.
    $client = New-Object Win32+RECT
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        [Win32]::ShowWindow($handle, 9) | Out-Null
        [Win32]::SetWindowPos($handle, $TOPMOST, 40, 40, 1201, 751, $SWP_SHOWWINDOW) | Out-Null
        Start-Sleep -Seconds 2
        [Win32]::GetClientRect($handle, [ref]$client) | Out-Null
        if ($client.Right -gt 0 -and $client.Bottom -gt 0) { break }
        Write-Output "the window has no client area (attempt $($attempt + 1)), restoring it again"
    }
    if ($client.Right -le 0 -or $client.Bottom -le 0) {
        throw 'the editor window has no client area to capture'
    }

    # The picture holds the whole window, title bar and all, so the client
    # area starts a little way inside it. The check needs to know where.
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
        # Where the client area starts inside the picture.
        Left = [int](To-Physical ($origin.X - $window.Left))
        Top = [int](To-Physical ($origin.Y - $window.Top))
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

# Starts the editor and waits for its window.
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
    # Windows restores the window to wherever it was the last time, which
    # can be a monitor that is not connected any more. An off-screen window
    # has no client area to capture, and the real cursor would land on it
    # and fight with the posted messages, so put it on the primary screen
    # first.
    [Win32]::SetWindowPos($handle, $TOPMOST, 40, 40, 1200, 750, $SWP_SHOWWINDOW) | Out-Null
    Start-Sleep -Seconds 2

    # The real cursor must not be over the editor: its own messages arrive
    # after the posted ones, and the editor would follow the real cursor
    # instead of the message. A check that depends on the order of two
    # things is a check that fails now and then.
    #
    # The cursor is put out of the way again here: whoever is using the
    # machine may have moved it while the editor was starting.
    $window = New-Object Win32+RECT
    [Win32]::GetWindowRect($handle, [ref]$window) | Out-Null
    for ($attempt = 0; $attempt -lt 5; $attempt++) {
        [Win32]::SetCursorPos(10, 10) | Out-Null
        Start-Sleep -Milliseconds 300
        $where = New-Object Win32+POINT
        [Win32]::GetCursorPos([ref]$where) | Out-Null
        if ($where.X -ge $window.Left -and $where.X -lt $window.Right -and
            $where.Y -ge $window.Top -and $where.Y -lt $window.Bottom) {
            Write-Output "the cursor is still over the editor window ($($where.X),$($where.Y)), moving it again"
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

try {
    # The real cursor is moved out of the way, so that its own messages do
    # not fight with the posted ones. It goes back where it was afterwards.
    $cursor = New-Object Win32+POINT
    [Win32]::GetCursorPos([ref]$cursor) | Out-Null
    [Win32]::SetCursorPos(10, 10) | Out-Null
    Start-Sleep -Milliseconds 300
    $moved = New-Object Win32+POINT
    [Win32]::GetCursorPos([ref]$moved) | Out-Null
    Write-Output "the cursor was moved to $($moved.X),$($moved.Y)"

    # The first run: the editor on its own.
    $editor = Start-Editor @($sample)
    $handle = $editor.Handle
    try {
        Write-Output "editor window: $handle, scale $($script:Factor)"

        # Drag over the third line of the document: the selection should show
        # up. The line is far enough from the find bar, which sits in the
        # top right corner, so the two do not cover each other.
        #
        # The real cursor is put out of the way first: it must not be over
        # the editor while the drag runs, or its own messages would arrive
        # after the posted ones.
        #
        # The drag is tried a few times: the messages are posted to the
        # window, and what the machine does with them while the editor is
        # starting up is not in the editor's hands. A check that fails now
        # and then says more about the machine than about the editor.
        $selected = 0
        for ($attempt = 1; $attempt -le 4; $attempt++) {
            [Win32]::SetCursorPos(10, 10) | Out-Null
            Start-Sleep -Milliseconds 300
            $dragLine = 2
            $x = [int](To-Physical $BODY_X)
            $y = [int](To-Physical ($BODY_Y + $LINE_HEIGHT * $dragLine))
            Send-Mouse $handle $WM_MOUSEMOVE $x $y 0
            Start-Sleep -Milliseconds 200
            Send-Mouse $handle $WM_LBUTTONDOWN $x $y 0
            Start-Sleep -Milliseconds 200
            foreach ($dx in 40, 90, 140, 200) {
                Send-Mouse $handle $WM_MOUSEMOVE ([int](To-Physical ($BODY_X + $dx))) $y $MK_LBUTTON
                Start-Sleep -Milliseconds 150
            }
            Send-Mouse $handle $WM_LBUTTONUP ([int](To-Physical ($BODY_X + 200))) $y 0
            Start-Sleep -Milliseconds 800

            $shot_result = Save-Window $handle $shot
            Write-Output "print window: $($shot_result.Ok)"
            Write-Output "saved $shot"
            $picture = $shot_result.Bitmap
            $left = $shot_result.Left
            $top = $shot_result.Top

            # The selection is drawn in the colour of the selection, and
            # nowhere else in the editor, so the whole client area is scanned.
            $selected = Count-Color $picture $left $top $shot_result.Width $shot_result.Height $SELECTION_RGB 14
            Write-Output "selected pixels: $selected"
            if ($selected -ge 25) { break }
            $picture.Dispose()
            Write-Output "the selection was not drawn (attempt $attempt), trying again"
        }
        if ($selected -lt 25) {
            throw 'the selection was not drawn'
        }
        Write-Output 'the selection is drawn'

        # The find bar sits in the top right corner of the body. Without it
        # open, that corner holds nothing but text.
        $findLeft = $left + [int](0.25 * $shot_result.Width)
        $findTop = $top + [int](To-Physical $BODY_Y)
        $findWidth = [int](0.65 * $shot_result.Width)
        $findHeight = [int](To-Physical 60)
        $accentBefore = Count-Color $picture $findLeft $findTop $findWidth $findHeight $ACCENT_RGB 14
        Write-Output "accent pixels where the find bar will be: $accentBefore"
        if ($accentBefore -gt 5) {
            throw 'the find bar region is not empty before the find bar is opened'
        }

        # Without a search, no match is highlighted.
        $matchedBefore = Count-Color $picture $left $top $shot_result.Width $shot_result.Height $MATCH_RGB 16
        Write-Output "highlighted match pixels: $matchedBefore"
        if ($matchedBefore -gt 5) {
            throw 'the body shows matches before the search was made'
        }
        $picture.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }

    # The second run: the editor with a search already open.
    $editor = Start-Editor @($sample, '--find', 'fox')
    $handle = $editor.Handle
    try {
        $find_result = Save-Window $handle $findShot
        Write-Output "print window: $($find_result.Ok)"
        Write-Output "saved $findShot"
        $findPicture = $find_result.Bitmap
        $left = $find_result.Left
        $top = $find_result.Top

        $findLeft = $left + [int](0.25 * $find_result.Width)
        $findTop = $top + [int](To-Physical $BODY_Y)
        $findWidth = [int](0.65 * $find_result.Width)
        $findHeight = [int](To-Physical 60)
        $accent = Count-Color $findPicture $findLeft $findTop $findWidth $findHeight $ACCENT_RGB 14
        Write-Output "accent pixels in the find bar: $accent"
        if ($accent -lt 40) {
            throw 'the find bar was not drawn'
        }
        Write-Output 'the find bar is drawn'

        # The match "fox" is on the first line of the document.
        $matched = Count-Color $findPicture $left $top $find_result.Width $find_result.Height $MATCH_RGB 16
        Write-Output "highlighted match pixels: $matched"
        if ($matched -lt 20) {
            throw 'the match was not highlighted'
        }
        Write-Output 'the match is highlighted'

        # Without a drag, no selection is drawn: the colour of the selection
        # belongs to the drag of the first run only.
        $selected = Count-Color $findPicture $left $top $find_result.Width $find_result.Height $SELECTION_RGB 14
        Write-Output "selected pixels: $selected"
        if ($selected -gt 5) {
            throw 'a selection is drawn without a drag'
        }
        $findPicture.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }

    # The third run: the editor on a file that has a language. The keywords
    # and the strings of the file are drawn in their own colours.
    $editor = Start-Editor @($rustSample)
    $handle = $editor.Handle
    try {
        $syntax_result = Save-Window $handle $syntaxShot
        Write-Output "print window: $($syntax_result.Ok)"
        Write-Output "saved $syntaxShot"
        $syntaxPicture = $syntax_result.Bitmap
        $left = $syntax_result.Left
        $top = $syntax_result.Top

        # The keywords of the file are drawn in the colour of a keyword, and
        # the text of a string in the colour of a string. Nothing else in the
        # window is drawn in those colours.
        $keywords = Count-Color $syntaxPicture $left $top $syntax_result.Width $syntax_result.Height $KEYWORD_RGB 22
        Write-Output "keyword pixels: $keywords"
        if ($keywords -lt 8) {
            throw 'the keywords were not highlighted'
        }
        Write-Output 'the keywords are highlighted'

        $strings = Count-Color $syntaxPicture $left $top $syntax_result.Width $syntax_result.Height $STRING_RGB 22
        Write-Output "string pixels: $strings"
        if ($strings -lt 8) {
            throw 'the strings were not highlighted'
        }
        Write-Output 'the strings are highlighted'
        $syntaxPicture.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }

    # The fourth run: the editor with a language server. The server says
    # that the first line of the file is wrong, and the thing it found wrong
    # is underlined.
    $editor = Start-Editor @($rustSample, '--lsp', ('"{0}"' -f $fakeServer))
    $handle = $editor.Handle
    try {
        # The server has to say hello, be told about the file and answer
        # before the underline is there, so the window is looked at a few
        # times before the check gives up.
        $underlined = 0
        for ($attempt = 1; $attempt -le 8; $attempt++) {
            Start-Sleep -Milliseconds 800
            $lspResult = Save-Window $handle $lspShot
            $underlined = Count-Color $lspResult.Bitmap $lspResult.Left $lspResult.Top $lspResult.Width $lspResult.Height $ERROR_RGB 22
            if ($underlined -ge 10) { break }
            $lspResult.Bitmap.Dispose()
        }
        Write-Output "error underline pixels: $underlined"
        if ($underlined -lt 10) {
            throw 'the thing the language server found wrong was not underlined'
        }
        Write-Output 'the diagnostics of the language server are shown'
        $lspResult.Bitmap.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }

    # The fifth run: the same server, but the editor asks it what could come
    # next, and the list of answers is shown under the caret.
    $editor = Start-Editor @($rustSample, '--lsp', ('"{0}"' -f $fakeServer), '--complete')
    $handle = $editor.Handle
    try {
        # The server has to answer before the list is there.
        $listed = 0
        for ($attempt = 1; $attempt -le 8; $attempt++) {
            Start-Sleep -Milliseconds 800
            $listResult = Save-Window $handle $completionShot
            $listed = Count-Color $listResult.Bitmap $listResult.Left $listResult.Top $listResult.Width $listResult.Height $COMPLETION_RGB 12
            if ($listed -ge 100) { break }
            $listResult.Bitmap.Dispose()
        }
        Write-Output "completion list pixels: $listed"
        if ($listed -lt 100) {
            throw 'the list of completions was not shown'
        }
        Write-Output 'the list of completions is shown'
        $listResult.Bitmap.Dispose()
    }
    finally {
        Stop-Process -Id $editor.Process.Id -Force -ErrorAction SilentlyContinue
        Start-Sleep -Milliseconds 400
    }
}
finally {
    Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Milliseconds 400
    # The cursor goes back where it was.
    [Win32]::SetCursorPos($cursor.X, $cursor.Y) | Out-Null
    $errors = Get-Content (Join-Path $logs 'app-err.log') -Raw -ErrorAction SilentlyContinue
    if ($errors) { Write-Output '--- app stderr ---'; Write-Output $errors }
}
