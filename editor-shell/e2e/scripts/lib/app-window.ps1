# Resolve the packaged editor's real, DWM-composited host window.
#
# WHY THIS EXISTS
# ---------------
# Every harness script used to select the window with:
#
#   Get-Process -Name $ProcName | Where-Object { $_.MainWindowHandle -ne 0 }
#
# `Process.MainWindowHandle` returns the FIRST top-level window of the process that is visible and
# unowned, walking windows in Z-order. The packaged app has TWO windows matching that description:
#
#   class 'Tauri Window'            1296x839   <- the real host, what we always meant
#   class 'Tao Thread Event Target'    16x16   <- tao's message-loop event target
#
# Z-order is not stable, so `MainWindowHandle` non-deterministically returns the 16x16 event target.
# When it did, the screenshot gate wrote a 16x16 PNG (158 bytes) and threw "unexpectedly small"; the
# OLE drop aimed its cursor at a 16x16 rect; and the video crop geometry would have framed 256 pixels
# of a 3-minute film. All three read as unrelated flakes. They were one wrong window.
#
# Selecting on the window CLASS is deterministic: tao names its event target, and Tauri names its host.
# A size sanity-check is kept as a second gate so a future class rename fails loudly instead of
# silently reintroducing a tiny-window capture.

if (-not ("MetrocalkAppWindow" -as [type])) {
  Add-Type @"
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class MetrocalkAppWindow {
  public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc callback, IntPtr lparam);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
  [DllImport("user32.dll")] public static extern IntPtr GetWindow(IntPtr hwnd, uint command);
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder buffer, int max);
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassName(IntPtr hwnd, StringBuilder buffer, int max);
  public struct RECT { public int Left, Top, Right, Bottom; }

  public class Found {
    public IntPtr Handle;
    public string Class;
    public int Width;
    public int Height;
    public override string ToString() {
      return string.Format("hwnd=0x{0:X} class='{1}' {2}x{3}", (long)Handle, Class, Width, Height);
    }
  }

  /// Every visible, unowned, top-level window belonging to the process — the exact candidate set
  /// `MainWindowHandle` picks from, but returned in full so the caller can choose deliberately.
  public static List<Found> TopLevelWindows(uint processId) {
    var found = new List<Found>();
    EnumWindows((hwnd, lparam) => {
      uint owner;
      GetWindowThreadProcessId(hwnd, out owner);
      if (owner != processId) return true;
      if (!IsWindowVisible(hwnd)) return true;
      if (GetWindow(hwnd, 4 /* GW_OWNER */) != IntPtr.Zero) return true;
      RECT rect;
      if (!GetWindowRect(hwnd, out rect)) return true;
      var name = new StringBuilder(256);
      GetClassName(hwnd, name, name.Capacity);
      found.Add(new Found {
        Handle = hwnd,
        Class = name.ToString(),
        Width = rect.Right - rect.Left,
        Height = rect.Bottom - rect.Top,
      });
      return true;
    }, IntPtr.Zero);
    return found;
  }
}
"@
}

# The host window's class. Tao's 16x16 message-only-ish event target is 'Tao Thread Event Target'.
$script:MetrocalkHostWindowClass = "Tauri Window"
# Below this, the window cannot be the composited host; it is an event target or a helper.
$script:MetrocalkMinimumHostEdge = 200

# Screen bounds are needed to recognise a foreign full-screen foreground window.
Add-Type -AssemblyName System.Windows.Forms

function Get-MetrocalkAppWindow {
  <#
    .SYNOPSIS
      The one true host window handle for the packaged editor, or a throwing diagnostic.
    .OUTPUTS
      IntPtr — the HWND of the 'Tauri Window' host.
  #>
  param(
    [Parameter(Mandatory = $true)][string]$ProcName,
    # Return a minimised host instead of rejecting it, so a caller that intends to un-minimise the
    # window can do so. Without this, the size gate below fires first and the caller never gets the
    # chance - which turns a recoverable minimise into a failed capture.
    [switch]$AllowIconic
  )

  $processes = @(Get-Process -Name $ProcName -ErrorAction SilentlyContinue)
  if ($processes.Count -ne 1) {
    $ids = ($processes | ForEach-Object { $_.Id }) -join ", "
    throw "Expected exactly one '$ProcName' process; found $($processes.Count) (PIDs: $ids)."
  }
  $process = $processes[0]

  $candidates = @([MetrocalkAppWindow]::TopLevelWindows([uint32]$process.Id))
  $hosts = @($candidates | Where-Object { $_.Class -eq $script:MetrocalkHostWindowClass })

  if ($hosts.Count -ne 1) {
    $seen = ($candidates | ForEach-Object { $_.ToString() }) -join "; "
    throw ("Expected exactly one '$($script:MetrocalkHostWindowClass)' window for '$ProcName'; " +
      "found $($hosts.Count). Visible unowned top-level windows: $seen")
  }

  $hostWindow = $hosts[0]
  if ($AllowIconic -and [MetrocalkAppWindow]::IsIconic($hostWindow.Handle)) {
    return $hostWindow.Handle
  }
  if ($hostWindow.Width -lt $script:MetrocalkMinimumHostEdge -or
      $hostWindow.Height -lt $script:MetrocalkMinimumHostEdge) {
    # A minimised window reports the icon rect (-32000,-32000 160x28), so say WHY it is small.
    $why = if ([MetrocalkAppWindow]::IsIconic($hostWindow.Handle)) {
      " The window is MINIMISED. " + (Get-MetrocalkForegroundConflict)
    } else { "" }
    throw ("The '$($script:MetrocalkHostWindowClass)' window is implausibly small for a host " +
      "($($hostWindow.Width)x$($hostWindow.Height)); refusing to capture or target it.$why")
  }

  return $hostWindow.Handle
}

<#
  .SYNOPSIS
    Describes a foreign window that currently owns the foreground and covers the screen, or "".

  .DESCRIPTION
    GUI automation on this harness needs the editor visible and un-minimised. A full-screen
    application belonging to another process (a game, a media player, a remote-desktop client) takes
    the foreground and Windows minimises everything else - including the editor, mid-gesture. The
    symptoms are wildly misleading: an OLE drag hangs until the harness SIGTERMs it, a PrintWindow
    capture returns the 16x28 icon rect, and the run reports "OLE timed out" or "capture unexpectedly
    small" with no hint that the cause is another application entirely. Name it instead of guessing.
#>
function Get-MetrocalkForegroundConflict {
  $foreground = [MetrocalkAppWindow]::GetForegroundWindow()
  if ($foreground -eq [IntPtr]::Zero) { return "" }

  $ourProcess = @(Get-Process -Name "metrocalk-editor-shell" -ErrorAction SilentlyContinue)
  [uint32]$foregroundPid = 0
  [void][MetrocalkAppWindow]::GetWindowThreadProcessId($foreground, [ref]$foregroundPid)
  if ($ourProcess.Count -eq 1 -and $foregroundPid -eq $ourProcess[0].Id) { return "" }

  $rect = New-Object MetrocalkAppWindow+RECT
  if (-not [MetrocalkAppWindow]::GetWindowRect($foreground, [ref]$rect)) { return "" }
  $width = $rect.Right - $rect.Left
  $height = $rect.Bottom - $rect.Top

  $title = New-Object System.Text.StringBuilder 256
  [void][MetrocalkAppWindow]::GetWindowText($foreground, $title, $title.Capacity)
  $owner = Get-Process -Id $foregroundPid -ErrorAction SilentlyContinue

  $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
  $coversScreen = ($width -ge $screen.Width) -and ($height -ge $screen.Height)
  if (-not $coversScreen) { return "" }

  return ("A full-screen window owned by another application currently holds the foreground and is " +
    "minimising the editor: '$($title.ToString())' (process '$($owner.ProcessName)', PID $foregroundPid, " +
    "${width}x${height}). Close or windowed-mode that application before running the GUI harness.")
}
