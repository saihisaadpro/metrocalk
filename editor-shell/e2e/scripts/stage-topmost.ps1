# Claim the desktop stage for the packaged editor, and prove the claim by looking at the screen.
#
# WHY THIS EXISTS
# ---------------
# `gdigrab` records the DESKTOP at a rectangle, not a window, so the delivered film is whatever pixels
# are on screen there. `window-client-rect.ps1` refuses to start a recording while something covers the
# editor, and that guard is correct - but it is a guard against a state, not a defence against entering
# it. FOUR production runs reached the app, imported 15,711 parts, authored the tracks, directed the
# shots and then threw at exactly that guard because an unrelated window had come to the foreground in
# the fifteen minutes it took to get there. Each of those runs cost a quarter of an hour.
#
# A TOPMOST window sits in a Z-order band above all non-topmost windows whether or not it has focus, so
# an ordinary window taking the foreground no longer paints over it. That removes the common cause
# without weakening the guard: the occlusion check still runs, and still refuses.
#
# WHY IT NO LONGER TRUSTS SetWindowPos
# ------------------------------------
# The first version of this script asked for topmost and then verified by reading WS_EX_TOPMOST back out
# of GWL_EXSTYLE. On run 8 it reported `{"hwnd":657636,"topmost":true}` and the very next call -
# WindowFromPoint at the same window's client centre, one second later - found a maximised Chrome window
# on top. The style bit had been set and the window was not on screen: the check was a measurement
# ALMOST of the thing it named, which is the exact defect class this whole lane has been chasing.
#
# So the contract here is the SCREEN, not the flag. `Test-StageClear` asks the same question the recorder
# will ask - "which window is actually at this point?" - and every rung below is judged by it:
#
#   1. pin              SetWindowPos(HWND_TOPMOST, SWP_NOACTIVATE)
#   2. attached-pin     the same call with this thread attached to the foreground thread's input queue,
#                       which lifts the foreground-lock that can silently downgrade a Z-order change
#   3. name-occluder    say what is actually on top, by handle, and leave it alone (see rung 3 below)
#
# It escalates only on evidence, retries across a few seconds because a transient window (a toast, a
# console another tool spawned) will leave on its own, and reports WHICH rung worked so a future failure
# is diagnosable instead of mysterious. If none of them works it returns clear=false rather than throwing
# - the caller's own guard is what decides whether to record.
#
# This does NOT steal focus. SWP_NOACTIVATE is set on every call deliberately: tauri-driver owns the
# input focus and the WebDriver session breaks if it is taken away mid-run.
param(
  [Parameter(Mandatory = $true)][ValidateSet("on", "off")][string]$Topmost,
  [string]$ProcName = "metrocalk-editor-shell",
  [int]$Attempts = 6,
  [int]$DelayMs = 1500
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class MetrocalkStageTopmost {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }
  [DllImport("user32.dll", SetLastError=true)] public static extern bool SetWindowPos(
    IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int n);
  [DllImport("user32.dll")] public static extern int GetWindowLong(IntPtr h, int index);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool GetClientRect(IntPtr h, out RECT r);
  [DllImport("user32.dll", SetLastError=true)] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
  [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
  [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint from, uint to, bool attach);
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowText(IntPtr h, StringBuilder s, int m);
}
"@

. (Join-Path $PSScriptRoot "lib/app-window.ps1")

$HWND_TOPMOST = [IntPtr](-1)
$HWND_NOTOPMOST = [IntPtr](-2)
# SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE - Z-order band only. Never move, size or focus the host: the
# capture rectangle was measured from its client area and the driver owns its focus.
$ZORDER_ONLY = 0x0001 -bor 0x0002 -bor 0x0010

function Get-WindowTitle([IntPtr]$handle) {
  $buffer = New-Object System.Text.StringBuilder 256
  [void][MetrocalkStageTopmost]::GetWindowText($handle, $buffer, $buffer.Capacity)
  return $buffer.ToString()
}

function Get-WindowOwnerName([IntPtr]$handle) {
  [uint32]$owner = 0
  [void][MetrocalkStageTopmost]::GetWindowThreadProcessId($handle, [ref]$owner)
  $process = Get-Process -Id $owner -ErrorAction SilentlyContinue
  if ($null -eq $process) { return "" }
  return $process.ProcessName
}

<#
  .SYNOPSIS
    Which top-level window is actually on screen at the host's client centre?
  .DESCRIPTION
    Deliberately the same question, at the same point, that window-client-rect.ps1 asks before it lets
    a recording start. Verifying the claim with a different measure than the one that will judge it is
    how run 8 reported a pinned stage and filmed a covered one.
#>
function Get-StageOwner([IntPtr]$handle) {
  $client = New-Object MetrocalkStageTopmost+RECT
  if (-not [MetrocalkStageTopmost]::GetClientRect($handle, [ref]$client)) {
    throw "GetClientRect failed for '$ProcName' with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
  }
  $point = New-Object MetrocalkStageTopmost+POINT
  $point.X = 0
  $point.Y = 0
  if (-not [MetrocalkStageTopmost]::ClientToScreen($handle, [ref]$point)) {
    throw "ClientToScreen failed for '$ProcName' with Win32 error $([Runtime.InteropServices.Marshal]::GetLastWin32Error())."
  }
  $centre = New-Object MetrocalkStageTopmost+POINT
  $centre.X = $point.X + [int](($client.Right - $client.Left) / 2)
  $centre.Y = $point.Y + [int](($client.Bottom - $client.Top) / 2)
  return [MetrocalkStageTopmost]::GetAncestor([MetrocalkStageTopmost]::WindowFromPoint($centre), 2)
}

function Test-TopmostStyle([IntPtr]$handle) {
  return ([MetrocalkStageTopmost]::GetWindowLong($handle, -20) -band 0x00000008) -ne 0
}

# ---------------------------------------------------------------------------------------------------
# Release. Tolerant on purpose: this runs in cleanup, after a run that may already have closed the app.
# ---------------------------------------------------------------------------------------------------
if ($Topmost -eq "off") {
  $released = $false
  $handle = [IntPtr]::Zero
  try {
    $handle = Get-MetrocalkAppWindow -ProcName $ProcName -AllowIconic
    [void][MetrocalkStageTopmost]::SetWindowPos($handle, $HWND_NOTOPMOST, 0, 0, 0, 0, $ZORDER_ONLY)
    $released = -not (Test-TopmostStyle $handle)
  } catch {
    # The app is gone, which is the state this call wanted to reach anyway.
    $released = $true
  }
  # Nothing to lift back: this script no longer moves any window but the editor's own, so release is
  # release and not a restoration. (A -Raise parameter existed here to undo rung 3's demotions; with
  # rung 3 reduced to a report it could never be exercised, and a parameter nothing can reach is the
  # same defect as a retry that cannot retry.)
  [ordered]@{
    hwnd = [long]$handle
    topmost = $false
    released = $released
  } | ConvertTo-Json -Compress
  return
}

# ---------------------------------------------------------------------------------------------------
# Claim.
# ---------------------------------------------------------------------------------------------------
# -AllowIconic: a minimised host reports the 160x28 icon rect and would be rejected by the size gate
# before this script got the chance to restore it, which turns a recoverable minimise into a dead run.
$handle = Get-MetrocalkAppWindow -ProcName $ProcName -AllowIconic
if ([MetrocalkStageTopmost]::IsIconic($handle)) {
  [void][MetrocalkStageTopmost]::ShowWindow($handle, 9)   # SW_RESTORE
  Start-Sleep -Milliseconds 400
}

$wouldDemote = @()
$demotedHandles = @{}
$ladder = @()
$clear = $false
$rung = "none"

for ($attempt = 1; $attempt -le $Attempts -and -not $clear; $attempt++) {
  # Rung 1 - the ordinary pin.
  [void][MetrocalkStageTopmost]::SetWindowPos($handle, $HWND_TOPMOST, 0, 0, 0, 0, $ZORDER_ONLY)
  Start-Sleep -Milliseconds 200
  if ((Get-StageOwner $handle) -eq $handle) { $clear = $true; $rung = "pin"; break }
  $ladder += "attempt${attempt}:pin-insufficient"

  # Rung 2 - the same call, with the foreground thread's input queue attached. A Z-order change asked
  # for by a process that does not own the foreground can be downgraded; attaching lifts that.
  [uint32]$foregroundPid = 0
  $foreground = [MetrocalkStageTopmost]::GetForegroundWindow()
  $foregroundThread = [MetrocalkStageTopmost]::GetWindowThreadProcessId($foreground, [ref]$foregroundPid)
  $self = [MetrocalkStageTopmost]::GetCurrentThreadId()
  if ($foregroundThread -ne 0 -and $foregroundThread -ne $self) {
    $attached = [MetrocalkStageTopmost]::AttachThreadInput($foregroundThread, $self, $true)
    [void][MetrocalkStageTopmost]::SetWindowPos($handle, $HWND_TOPMOST, 0, 0, 0, 0, $ZORDER_ONLY)
    if ($attached) { [void][MetrocalkStageTopmost]::AttachThreadInput($foregroundThread, $self, $false) }
    Start-Sleep -Milliseconds 200
    if ((Get-StageOwner $handle) -eq $handle) { $clear = $true; $rung = "attached-pin"; break }
    $ladder += "attempt${attempt}:attached-pin-insufficient"
  }

  # Rung 3 - NAME whatever is genuinely on top. Deliberately does not move it.
  #
  # This rung used to push the occluder to HWND_BOTTOM by handle. It worked, and it is the wrong thing
  # for this harness to do: the windows that have covered the editor in production were a person's
  # browser on a live page, twice, on a machine in interactive use. Rearranging somebody's application
  # so a test can film the desktop is not a trade a test gets to make on its own.
  #
  # It costs nothing to give up, because the desktop recording is no longer what the run depends on:
  # the legibility verdict is measured from the window itself via PrintWindow, which reads the app's own
  # presentation whether or not anything covers it. The pin is now an optimisation for a secondary
  # artifact, so its last rung is a report rather than an act.
  $owner = Get-StageOwner $handle
  if ($owner -ne [IntPtr]::Zero -and $owner -ne $handle) {
    $title = Get-WindowTitle $owner
    $ownerName = Get-WindowOwnerName $owner
    if (-not $demotedHandles.ContainsKey([long]$owner)) {
      $demotedHandles[[long]$owner] = $true
      $wouldDemote += [ordered]@{ hwnd = [long]$owner; title = $title; process = $ownerName }
    }
    $ladder += "attempt${attempt}:left '$title' ($ownerName) where it is; not the harness's window to move"
  }

  # Something transient may simply leave. Wait before escalating again.
  if ($attempt -lt $Attempts) { Start-Sleep -Milliseconds $DelayMs }
}

$finalOwner = Get-StageOwner $handle
$occludedBy = ""
if ($finalOwner -ne $handle) {
  $occludedBy = "$(Get-WindowTitle $finalOwner) [$(Get-WindowOwnerName $finalOwner)]"
}

[ordered]@{
  hwnd = [long]$handle
  # Kept, but no longer the verdict: it is the flag that lied on run 8, reported beside the truth.
  topmostStyle = (Test-TopmostStyle $handle)
  clear = $clear
  rung = $rung
  attempts = $attempt - 1
  # Named, never moved: what was on top when the pin could not clear it (see rung 3).
  wouldHaveDemoted = $wouldDemote
  ladder = $ladder
  occludedBy = $occludedBy
} | ConvertTo-Json -Compress -Depth 5
