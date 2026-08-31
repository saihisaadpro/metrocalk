param(
  [string]$ProcName = "metrocalk-editor-shell"
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class MetrocalkWindowGeometry {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, System.Text.StringBuilder s, int m);
}
"@

# Select by window class. `MainWindowHandle` can return tao's visible 16x16 event target instead of the
# host, which here would silently crop the recorded film to a 16x16 corner of the screen.
. (Join-Path $PSScriptRoot "lib/app-window.ps1")
$handle = Get-MetrocalkAppWindow -ProcName $ProcName
if ([MetrocalkWindowGeometry]::IsIconic($handle)) {
  throw "Cannot measure the minimised '$ProcName' window."
}
$client = New-Object MetrocalkWindowGeometry+RECT
if (-not [MetrocalkWindowGeometry]::GetClientRect($handle, [ref]$client)) {
  throw "GetClientRect failed for '$ProcName' with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
}
$origin = New-Object MetrocalkWindowGeometry+POINT
$origin.X = 0
$origin.Y = 0
if (-not [MetrocalkWindowGeometry]::ClientToScreen($handle, [ref]$origin)) {
  throw "ClientToScreen failed for '$ProcName' with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
}
$width = $client.Right - $client.Left
$height = $client.Bottom - $client.Top
if ($width -le 0 -or $height -le 0) {
  throw "Invalid client rectangle ${width}x${height} for '$ProcName'."
}

# Is the host actually the window on screen at its own client centre, or is something covering it?
# gdigrab records the DESKTOP at a rectangle, so an occluded editor silently yields a film of whatever
# is on top - and a luminance/motion quality gate cannot tell that apart from a good capture. Report
# occlusion here so the caller can refuse to record rather than record the wrong window.
$centre = New-Object MetrocalkWindowGeometry+POINT
$centre.X = $origin.X + [int]($width / 2)
$centre.Y = $origin.Y + [int]($height / 2)
$topWindow = [MetrocalkWindowGeometry]::WindowFromPoint($centre)
$topRoot = [MetrocalkWindowGeometry]::GetAncestor($topWindow, 2) # GA_ROOT
$occluded = ($topRoot -ne $handle)
$occludingTitle = ""
if ($occluded) {
  $sb = New-Object System.Text.StringBuilder 256
  [void][MetrocalkWindowGeometry]::GetWindowText($topRoot, $sb, $sb.Capacity)
  $occludingTitle = $sb.ToString()
}

# The WINDOW rect as well as the client rect, because two different captures of this app are framed by
# two different rectangles and a number measured on one is not a number about the other.
#
# `gdigrab` films a screen rectangle derived from the CLIENT area; `PrintWindow` returns the whole
# WINDOW, title bar and border included. A window capture measured as if it were a film frame includes
# the editor's white docks and header, which pin YHIGH near 233 no matter what the 3D viewport shows: a
# frame whose viewport is entirely one dark surface -- the camera inside a machine, the exact failure
# this lane exists to detect -- measures an interdecile range of 166 that way and 0 when cropped to the
# viewport. Reporting the window origin is what lets the caller crop one into the other.
$window = New-Object MetrocalkWindowGeometry+RECT
if (-not [MetrocalkWindowGeometry]::GetWindowRect($handle, [ref]$window)) {
  throw "GetWindowRect failed for '$ProcName' with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
}

[ordered]@{
  processId = (Get-Process -Name $ProcName).Id
  hwnd = [long]$handle
  windowX = $window.Left
  windowY = $window.Top
  windowWidth = $window.Right - $window.Left
  windowHeight = $window.Bottom - $window.Top
  occluded = $occluded
  occludedBy = $occludingTitle
  x = $origin.X
  y = $origin.Y
  width = $width
  height = $height
} | ConvertTo-Json -Compress
