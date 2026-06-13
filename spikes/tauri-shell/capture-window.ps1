# Capture the on-screen pixels of a window by title → PNG. This captures the FINAL composited
# result (whatever is actually visible), which is exactly the 1b compositing ground truth:
# if the native wgpu triangle shows through the transparent webview, it appears here.
param(
  [string]$Title = "metrocalk m2.1 spike",
  [string]$Out = "evidence\1b-composite.png"
)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class WinCap {
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindow(string c, string n);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  public struct RECT { public int Left, Top, Right, Bottom; }
}
"@
$h = [WinCap]::FindWindow($null, $Title)
if ($h -eq [IntPtr]::Zero) { Write-Output "WINDOW NOT FOUND: $Title"; exit 2 }
[WinCap]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 600
$r = New-Object WinCap+RECT
[WinCap]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left; $ht = $r.Bottom - $r.Top
if ($w -le 0 -or $ht -le 0) { Write-Output "BAD RECT $w x $ht"; exit 3 }
Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap $w, $ht
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($r.Left, $r.Top, 0, 0, $bmp.Size)
$dir = Split-Path $Out -Parent
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force $dir | Out-Null }
$bmp.Save((Resolve-Path -LiteralPath . ).Path + "\" + $Out, [System.Drawing.Imaging.ImageFormat]::Png)
$g.Dispose(); $bmp.Dispose()
Write-Output "CAPTURED $w x $ht -> $Out (window rect $($r.Left),$($r.Top))"
