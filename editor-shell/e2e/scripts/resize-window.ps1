# Resize the packaged editor's NATIVE window.
#
# Why this exists: `browser.setWindowSize` does not resize a Tauri window through tauri-driver — the
# viewport-stress audit measured that and recorded three identical "resize" frames as a known gap. The
# render loop polls `window.inner_size()` every frame and rebuilds every size-dependent target when it
# changes, so a resize is exactly the path that can leave a bind group pointing at a stale texture view.
# It has to be exercised for real, which means going to the window manager directly.
param(
  [Parameter(Mandatory = $true)][int]$Width,
  [Parameter(Mandatory = $true)][int]$Height,
  [string]$ProcName = "metrocalk-editor-shell"
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class MetrocalkWindowResize {
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowPos(
    IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@

# Select by window class; `MainWindowHandle` can resolve to tao's 16x16 event target, and resizing THAT
# leaves the host untouched while reporting success.
. (Join-Path $PSScriptRoot "lib/app-window.ps1")
$handle = Get-MetrocalkAppWindow -ProcName $ProcName
if ([MetrocalkWindowResize]::IsIconic($handle)) {
  [MetrocalkWindowResize]::ShowWindow($handle, 9) | Out-Null
  Start-Sleep -Milliseconds 300
}

# SWP_NOZORDER | SWP_NOACTIVATE — move/size only, do not steal focus from the driver.
$flags = 0x0004 -bor 0x0010
if (-not [MetrocalkWindowResize]::SetWindowPos($handle, [IntPtr]::Zero, 40, 40, $Width, $Height, $flags)) {
  throw "SetWindowPos failed with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
}
# Give the render loop several frames to notice, reconfigure the surface and rebuild its targets.
Start-Sleep -Milliseconds 900

$rect = New-Object MetrocalkWindowResize+RECT
[MetrocalkWindowResize]::GetWindowRect($handle, [ref]$rect) | Out-Null
$actual = "$($rect.Right - $rect.Left)x$($rect.Bottom - $rect.Top)"
Write-Output "RESIZED -> $actual (asked ${Width}x${Height})"
