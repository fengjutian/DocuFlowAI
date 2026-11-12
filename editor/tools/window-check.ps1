# The mouse messages are posted to the editor's window, so the check never
# takes the cursor away from whoever is using the machine. winit reads the
# position out of the message, so the editor sees the click where it was
# sent.

$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$exe = Join-Path $root 'target\debug\codeweaver-editor.exe'
$logs = Join-Path $root '.dev-logs'
$fixture = Join-Path $PSScriptRoot 'fixtures\sample.txt'
$sample = Join-Path $logs 'sample.txt'
$shot = Join-Path $logs 'window-check.png'

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
  [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
  [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
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

if (-not (Test-Path $exe)) {
    throw "the editor is not built; run 'cargo build -p codeweaver-editor' first"
}
New-Item -ItemType Directory -Force -Path $logs | Out-Null
# The fixture is read as UTF-8 on purpose: PowerShell reads a script without
# a byte order mark as ANSI, so the text must not travel through the script.
Copy-Item -Path $fixture -Destination $sample -Force

function Send-Mouse([IntPtr]$handle, [uint32]$message, [int]$x, [int]$y, [int]$extra) {
    $packed = [int64](($y -band 0xFFFF) -shl 16) -bor ($x -band 0xFFFF) -bor $extra
    [Win32]::PostMessage($handle, $message, [IntPtr]0, [IntPtr]$packed) | Out-Null
}

function Save-Window([IntPtr]$handle, [string]$path) {
    $rect = New-Object Win32+RECT
    [Win32]::GetClientRect($handle, [ref]$rect) | Out-Null
    $bitmap = New-Object System.Drawing.Bitmap $rect.Right, $rect.Bottom
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $hdc = $graphics.GetHdc()
    # PW_RENDERFULLCONTENT makes the window manager draw the window even
    # when it is behind another one.
    $ok = [Win32]::PrintWindow($handle, $hdc, 2)
    $graphics.ReleaseHdc($hdc)
    $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $graphics.Dispose()
    $bitmap.Dispose()
    return $ok
}

Get-Process -Name codeweaver-editor -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep -Milliseconds 500

$editor = Start-Process -FilePath $exe -ArgumentList $sample `
    -RedirectStandardOutput (Join-Path $logs 'app-out.log') `
    -RedirectStandardError (Join-Path $logs 'app-err.log') -PassThru
try {
    $handle = [IntPtr]::Zero
    for ($i = 0; $i -lt 60; $i++) {
        Start-Sleep -Milliseconds 250
        $editor.Refresh()
        if (-not $editor.HasExited) {
            $handle = [Win32]::Find([uint32]$editor.Id)
            if ($handle -ne [IntPtr]::Zero) { break }
        }
    }
    if ($handle -eq [IntPtr]::Zero) { throw 'the editor window never appeared' }
    # The first frame needs the GPU.
    Start-Sleep -Seconds 3

    # Windows restores the window to wherever it was the last time, which
    # can be a monitor that is not connected any more. An off-screen window
    # has no client area to capture, and the real cursor would land on it
    # and fight with the posted messages, so put it on the primary screen
    # first.
    [Win32]::SetWindowPos($handle, $TOPMOST, 40, 40, 1280, 800, $SWP_SHOWWINDOW) | Out-Null
    Start-Sleep -Seconds 2

    # The window's messages carry physical pixels, the editor works in
    # logical ones.
    $scale = [Win32]::GetDpiForWindow($handle) / 96.0
    Write-Output "editor window: $handle, scale $scale"

    # Drag over the last line of the document: the selection should show up.
    $x = [int]($BODY_X * $scale)
    $y = [int](($BODY_Y + $LINE_HEIGHT * 6) * $scale)
    Send-Mouse $handle $WM_MOUSEMOVE $x $y 0
    Start-Sleep -Milliseconds 200
    Send-Mouse $handle $WM_LBUTTONDOWN $x $y 0
    Start-Sleep -Milliseconds 200
    foreach ($dx in 40, 90, 140, 200) {
        Send-Mouse $handle $WM_MOUSEMOVE ($x + [int]($dx * $scale)) $y $MK_LBUTTON
        Start-Sleep -Milliseconds 150
    }
    Send-Mouse $handle $WM_LBUTTONUP ($x + [int](200 * $scale)) $y 0
    Start-Sleep -Milliseconds 800

    $ok = Save-Window $handle $shot
    Write-Output "print window: $ok"
    Write-Output "saved $shot"
}
finally {
    Stop-Process -Id $editor.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 400
    $errors = Get-Content (Join-Path $logs 'app-err.log') -Raw -ErrorAction SilentlyContinue
    if ($errors) { Write-Output '--- app stderr ---'; Write-Output $errors }
}
