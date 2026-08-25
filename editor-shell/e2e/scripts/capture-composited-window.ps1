# Capture the final composited pixels of the packaged editor, including the native wgpu viewport that
# sits beneath the transparent WebView2. A WebDriver screenshot cannot: it shows the React panels and a
# black hole where the 3D is.
#
# TWO paths, in this order:
#
#  1. `PrintWindow` with PW_RENDERFULLCONTENT. Verified on this app to return BOTH the WebView2 UI and the
#     wgpu surface. It reads the window's own presentation, so it needs no minimise/restore, no topmost,
#     and no foreground steal - it is faster, far less disruptive, and works while the window is occluded.
#  2. The original desktop-DC BitBlt, kept as a fallback.
#
# The order was reversed after the desktop DC began failing every BitBlt with ERROR_INVALID_HANDLE (6) on
# this machine - reproducibly, for a 64x64 copy, outside this harness entirely. Whatever makes the desktop
# DC unavailable to a session, a capture path that never touches it is simply the more robust one.
param(
  [Parameter(Mandatory = $true)]
  [string]$Out,
  [string]$ProcName = "metrocalk-editor-shell",
  [int]$X = 40,
  [int]$Y = 40,
  # Capture the current presentation without moving, raising, restoring, or changing
  # z-order. Required while an OLE drag is held over the viewport.
  [switch]$PreserveWindow
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class MetrocalkWindowCapture {
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, IntPtr pid);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint a, uint b, bool attach);
  [DllImport("user32.dll", SetLastError=true)] public static extern IntPtr GetDC(IntPtr h);
  [DllImport("user32.dll", SetLastError=true)] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
  [DllImport("gdi32.dll", SetLastError=true)] public static extern IntPtr CreateCompatibleDC(IntPtr dc);
  [DllImport("gdi32.dll", SetLastError=true)] public static extern bool DeleteDC(IntPtr dc);
  [DllImport("gdi32.dll", SetLastError=true)] public static extern IntPtr CreateCompatibleBitmap(IntPtr dc, int width, int height);
  [DllImport("gdi32.dll", SetLastError=true)] public static extern IntPtr SelectObject(IntPtr dc, IntPtr value);
  [DllImport("gdi32.dll", SetLastError=true)] public static extern bool DeleteObject(IntPtr value);
  [DllImport("gdi32.dll", SetLastError=true)] public static extern bool BitBlt(
    IntPtr destination, int destinationX, int destinationY, int width, int height,
    IntPtr source, int sourceX, int sourceY, int rasterOperation);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

# Resolve the composited host by window CLASS. `MainWindowHandle` also matches tao's visible, unowned,
# top-level 16x16 'Tao Thread Event Target', and returning that one is what produced 158-byte captures.
. (Join-Path $PSScriptRoot "lib/app-window.ps1")
# Resolve permissively first: a minimised host is recoverable here (see the SW_SHOWNOACTIVATE below),
# and rejecting it on size before restoring it would turn that into a failed capture. -PreserveWindow
# cannot restore, so it keeps the strict resolve and fails loudly.
$handle = Get-MetrocalkAppWindow -ProcName $ProcName -AllowIconic:(-not $PreserveWindow)
$topmost = [IntPtr](-1)
$notTopmost = [IntPtr](-2)
$swpNoSize = 0x0001
$swpShowWindow = 0x0040
$preparedWindow = $false

try {
  # Only un-minimise. The old unconditional minimise/restore/topmost dance existed to force DWM to
  # recomposite for the desktop-DC read; PrintWindow does not need it, and doing it ~50 times in a run is
  # what made the window handle transiently invalid.
  if ($PreserveWindow -and [MetrocalkWindowCapture]::IsIconic($handle)) {
    throw "Cannot preserve and capture a minimised '$ProcName' window."
  }
  if (-not $PreserveWindow -and [MetrocalkWindowCapture]::IsIconic($handle)) {
    # SW_SHOWNOACTIVATE (4), not SW_RESTORE (9). Restoring with activation takes the foreground, and if
    # a full-screen application owns it (a game, a player, an RDP client) that application immediately
    # takes it back and re-minimises this window - mid-run, between two captures. PrintWindow reads the
    # window's own presentation and does not need the foreground, so never ask for it.
    [MetrocalkWindowCapture]::ShowWindow($handle, 4) | Out-Null
    Start-Sleep -Milliseconds 400
    # Re-validate strictly: if it is STILL minimised, something is putting it back (see
    # Get-MetrocalkForegroundConflict) and the capture would be of the 16x28 icon rect.
    $handle = Get-MetrocalkAppWindow -ProcName $ProcName
  }
  # Deliberately NO z-order or activation change here. PrintWindow captures an occluded window
  # correctly (verified against a full-screen foreground application), so raising the window would
  # only disturb whatever the user has in front - and start a foreground fight this script loses.
  # The BitBlt fallback below reads the desktop and DOES need visible pixels; it raises the window
  # itself, at the point where it is actually needed.

  $rect = New-Object MetrocalkWindowCapture+RECT
  if (-not [MetrocalkWindowCapture]::GetWindowRect($handle, [ref]$rect)) {
    throw "GetWindowRect failed for '$ProcName'."
  }
  $width = $rect.Right - $rect.Left
  $height = $rect.Bottom - $rect.Top
  if ($width -le 0 -or $height -le 0) { throw "Invalid window rectangle ${width}x${height}." }

  Add-Type -AssemblyName System.Drawing

  # PrintWindow returns TRUE even when it hands back an unpainted (uniform) bitmap - typically when the
  # target's UI thread is busy and cannot service WM_PRINT in time. Accepting that TRUE at face value set
  # $printed and made the BitBlt fallback below unreachable, so a recoverable blank frame became a hard
  # failure. Grade the pixels first; only a frame with actual contrast counts as a capture.
  function Test-CaptureHasContrast {
    param([System.Drawing.Bitmap]$Image)
    $low = 255.0
    $high = 0.0
    for ($gy = 0; $gy -lt 16; $gy++) {
      $sampleY = [Math]::Min($Image.Height - 1, [int](($gy + 0.5) * $Image.Height / 16.0))
      for ($gx = 0; $gx -lt 16; $gx++) {
        $sampleX = [Math]::Min($Image.Width - 1, [int](($gx + 0.5) * $Image.Width / 16.0))
        $pixel = $Image.GetPixel($sampleX, $sampleY)
        $luma = 0.2126 * $pixel.R + 0.7152 * $pixel.G + 0.0722 * $pixel.B
        $low = [Math]::Min($low, $luma)
        $high = [Math]::Max($high, $luma)
      }
    }
    return ($high - $low) -ge 3.0
  }

  # -- path 1: PrintWindow ---------------------------------------------------------------------------
  $directory = Split-Path -Path $Out -Parent
  if ($directory -and -not (Test-Path -LiteralPath $directory)) {
    New-Item -ItemType Directory -Force -Path $directory | Out-Null
  }
  $printed = $false
  $windowDc = [MetrocalkWindowCapture]::GetDC($handle)
  if ($windowDc -ne [IntPtr]::Zero) {
    $printMemory = [MetrocalkWindowCapture]::CreateCompatibleDC($windowDc)
    $printBitmap = [MetrocalkWindowCapture]::CreateCompatibleBitmap($windowDc, $width, $height)
    if ($printMemory -ne [IntPtr]::Zero -and $printBitmap -ne [IntPtr]::Zero) {
      $printPrevious = [MetrocalkWindowCapture]::SelectObject($printMemory, $printBitmap)
      # 2 = PW_RENDERFULLCONTENT, the flag that makes DirectComposition/WebView2 content render.
      # Retry briefly: a busy UI thread that misses one WM_PRINT usually services the next.
      for ($printAttempt = 1; $printAttempt -le 3 -and -not $printed; $printAttempt++) {
        if (-not [MetrocalkWindowCapture]::PrintWindow($handle, $printMemory, 2)) {
          Start-Sleep -Milliseconds 400
          continue
        }
        $image = [System.Drawing.Image]::FromHbitmap($printBitmap)
        try {
          if (Test-CaptureHasContrast -Image $image) {
            $image.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
            $printed = $true
          }
          else {
            Start-Sleep -Milliseconds 400
          }
        }
        finally { $image.Dispose() }
      }
      if ($printPrevious -ne [IntPtr]::Zero -and $printPrevious -ne [IntPtr](-1)) {
        [MetrocalkWindowCapture]::SelectObject($printMemory, $printPrevious) | Out-Null
      }
    }
    if ($printBitmap -ne [IntPtr]::Zero) { [MetrocalkWindowCapture]::DeleteObject($printBitmap) | Out-Null }
    if ($printMemory -ne [IntPtr]::Zero) { [MetrocalkWindowCapture]::DeleteDC($printMemory) | Out-Null }
    [MetrocalkWindowCapture]::ReleaseDC($handle, $windowDc) | Out-Null
  }
  if (-not $printed) {
    # -- path 2: the original desktop-DC BitBlt ------------------------------------------------------
    # This one reads the DESKTOP, so it can only see pixels that are actually on screen. Raising is
    # required here and only here; the finally block restores the z-order afterwards.
    if (-not $PreserveWindow) {
      [MetrocalkWindowCapture]::SetWindowPos($handle, $topmost, $X, $Y, 0, 0, ($swpNoSize -bor $swpShowWindow)) | Out-Null
      [MetrocalkWindowCapture]::BringWindowToTop($handle) | Out-Null
      $preparedWindow = $true
      Start-Sleep -Milliseconds 350
      # The rect can move when the window is repositioned above; re-read it before copying.
      [MetrocalkWindowCapture]::GetWindowRect($handle, [ref]$rect) | Out-Null
      $width = $rect.Right - $rect.Left
      $height = $rect.Bottom - $rect.Top
    }
    # A WebView2 restore can transiently invalidate the desktop DC for one frame. Recreate every GDI
    # object for each bounded retry; never accept a stale/partial capture. Graphics.CopyFromScreen wraps
    # this operation but can throw "The handle is invalid" before exposing which handle failed.
    $captured = $false
    for ($attempt = 1; $attempt -le 3 -and -not $captured; $attempt++) {
    $screenDc = [MetrocalkWindowCapture]::GetDC([IntPtr]::Zero)
    $memoryDc = [IntPtr]::Zero
    $nativeBitmap = [IntPtr]::Zero
    $previousBitmap = [IntPtr]::Zero
    $bitmap = $null
    try {
      if ($screenDc -eq [IntPtr]::Zero) {
        throw "GetDC(desktop) failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
      }
      $memoryDc = [MetrocalkWindowCapture]::CreateCompatibleDC($screenDc)
      if ($memoryDc -eq [IntPtr]::Zero) {
        throw "CreateCompatibleDC failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
      }
      $nativeBitmap = [MetrocalkWindowCapture]::CreateCompatibleBitmap($screenDc, $width, $height)
      if ($nativeBitmap -eq [IntPtr]::Zero) {
        throw "CreateCompatibleBitmap failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
      }
      $previousBitmap = [MetrocalkWindowCapture]::SelectObject($memoryDc, $nativeBitmap)
      if ($previousBitmap -eq [IntPtr]::Zero -or $previousBitmap -eq [IntPtr](-1)) {
        throw "SelectObject failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
      }
      # The desktop DC is already DWM-composited. SRCCOPY captures those visible pixels directly.
      # CAPTUREBLT is intentionally omitted: current WebView2/DWM builds may reject that layered-window
      # flag with ERROR_ACCESS_DENIED even though an ordinary composited desktop copy is permitted.
      if (-not [MetrocalkWindowCapture]::BitBlt(
        $memoryDc, 0, 0, $width, $height,
        $screenDc, $rect.Left, $rect.Top, 0x00CC0020
      )) {
        throw "BitBlt failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
      }
      $bitmap = [System.Drawing.Image]::FromHbitmap($nativeBitmap)
      # Windows PowerShell 5.1 treats `-LiteralPath` + `-Parent` as an ambiguous parameter set.
      $directory = Split-Path -Path $Out -Parent
      if ($directory -and -not (Test-Path -LiteralPath $directory)) {
        New-Item -ItemType Directory -Force -Path $directory | Out-Null
      }
      $bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
      $captured = $true
    }
    catch {
      if ($attempt -ge 3) { throw }
      Start-Sleep -Milliseconds 500
    }
    finally {
      if ($null -ne $bitmap) { $bitmap.Dispose() }
      if ($previousBitmap -ne [IntPtr]::Zero -and $previousBitmap -ne [IntPtr](-1)) {
        [MetrocalkWindowCapture]::SelectObject($memoryDc, $previousBitmap) | Out-Null
      }
      if ($nativeBitmap -ne [IntPtr]::Zero) {
        [MetrocalkWindowCapture]::DeleteObject($nativeBitmap) | Out-Null
      }
      if ($memoryDc -ne [IntPtr]::Zero) {
        [MetrocalkWindowCapture]::DeleteDC($memoryDc) | Out-Null
      }
      if ($screenDc -ne [IntPtr]::Zero) {
        [MetrocalkWindowCapture]::ReleaseDC([IntPtr]::Zero, $screenDc) | Out-Null
      }
    }
    }
  }
}
finally {
  if ($preparedWindow) {
    [MetrocalkWindowCapture]::SetWindowPos($handle, $notTopmost, $X, $Y, 0, 0, $swpNoSize) | Out-Null
  }
}

$captured = Get-Item -LiteralPath $Out
if ($captured.Length -le 1024) { throw "Capture '$Out' is unexpectedly small ($($captured.Length) bytes)." }
$verificationImage = [System.Drawing.Bitmap]::FromFile($captured.FullName)
try {
  if ($verificationImage.Width -ne $width -or $verificationImage.Height -ne $height) {
    throw "Capture dimensions $($verificationImage.Width)x$($verificationImage.Height) do not match the window ${width}x${height}."
  }
  # Reject the uniform black/white/transparent frames that PrintWindow can return while
  # still claiming success. Sampling a regular grid is cheap and sufficient for this gate.
  $minLuma = 255.0
  $maxLuma = 0.0
  for ($gy = 0; $gy -lt 16; $gy++) {
    $sampleY = [Math]::Min($height - 1, [int](($gy + 0.5) * $height / 16.0))
    for ($gx = 0; $gx -lt 16; $gx++) {
      $sampleX = [Math]::Min($width - 1, [int](($gx + 0.5) * $width / 16.0))
      $pixel = $verificationImage.GetPixel($sampleX, $sampleY)
      $luma = 0.2126 * $pixel.R + 0.7152 * $pixel.G + 0.0722 * $pixel.B
      $minLuma = [Math]::Min($minLuma, $luma)
      $maxLuma = [Math]::Max($maxLuma, $luma)
    }
  }
  if (($maxLuma - $minLuma) -lt 3.0) {
    throw "Capture '$Out' is visually uniform (sampled luma range $([Math]::Round($minLuma, 2))..$([Math]::Round($maxLuma, 2)))."
  }
}
finally {
  $verificationImage.Dispose()
}
Write-Output "CAPTURED ${width}x${height} luma=$([Math]::Round($minLuma, 1))..$([Math]::Round($maxLuma, 1)) -> $($captured.FullName)"
