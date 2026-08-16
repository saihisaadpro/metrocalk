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
  [int]$Y = 40
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

$visibleProcesses = @(
  Get-Process -Name $ProcName -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 }
)
if ($visibleProcesses.Count -ne 1) {
  $visibleIds = ($visibleProcesses | ForEach-Object { $_.Id }) -join ", "
  throw "Expected exactly one visible window for process '$ProcName'; found $($visibleProcesses.Count) (PIDs: $visibleIds)."
}
$process = $visibleProcesses[0]

$handle = $process.MainWindowHandle
$topmost = [IntPtr](-1)
$notTopmost = [IntPtr](-2)
$swpNoSize = 0x0001
$swpShowWindow = 0x0040

try {
  # Only un-minimise. The old unconditional minimise/restore/topmost dance existed to force DWM to
  # recomposite for the desktop-DC read; PrintWindow does not need it, and doing it ~50 times in a run is
  # what made the window handle transiently invalid.
  if ([MetrocalkWindowCapture]::IsIconic($handle)) {
    [MetrocalkWindowCapture]::ShowWindow($handle, 9) | Out-Null
    Start-Sleep -Milliseconds 400
  }
  [MetrocalkWindowCapture]::SetWindowPos($handle, $topmost, $X, $Y, 0, 0, ($swpNoSize -bor $swpShowWindow)) | Out-Null
  [MetrocalkWindowCapture]::BringWindowToTop($handle) | Out-Null
  Start-Sleep -Milliseconds 350

  $rect = New-Object MetrocalkWindowCapture+RECT
  if (-not [MetrocalkWindowCapture]::GetWindowRect($handle, [ref]$rect)) {
    throw "GetWindowRect failed for '$ProcName'."
  }
  $width = $rect.Right - $rect.Left
  $height = $rect.Bottom - $rect.Top
  if ($width -le 0 -or $height -le 0) { throw "Invalid window rectangle ${width}x${height}." }

  Add-Type -AssemblyName System.Drawing

  # ── path 1: PrintWindow ───────────────────────────────────────────────────────────────────────────
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
      if ([MetrocalkWindowCapture]::PrintWindow($handle, $printMemory, 2)) {
        $image = [System.Drawing.Image]::FromHbitmap($printBitmap)
        try {
          $image.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
          $printed = $true
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
  # NOTE: the window is left TOPMOST on purpose. Releasing it here was tried and made EVERY capture blank:
  # PrintWindow against this DirectComposition surface needs the window unoccluded, and the console this
  # script runs from will otherwise be over it. A blank result is written as a ~158-byte uniform PNG rather
  # than failing, so callers must check the output size.
  if ($printed) { return }

  # ── path 2: the original desktop-DC BitBlt ────────────────────────────────────────────────────────
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
finally {
  [MetrocalkWindowCapture]::SetWindowPos($handle, $notTopmost, $X, $Y, 0, 0, $swpNoSize) | Out-Null
}

$captured = Get-Item -LiteralPath $Out
if ($captured.Length -le 1024) { throw "Capture '$Out' is unexpectedly small ($($captured.Length) bytes)." }
Write-Output "CAPTURED ${width}x${height} -> $($captured.FullName)"
