<#
.SYNOPSIS
  Perform a REAL OS-level drag-and-drop of one or more files onto a window.

.DESCRIPTION
  Not a simulated application event. This builds a genuine CF_HDROP IDataObject and runs
  ole32!DoDragDrop -- the exact mechanism Explorer uses when a user drags a file out of a
  folder. Windows routes DragEnter/DragOver/Drop to whichever IDropTarget is registered on
  the window under the cursor; for this app that is the WebView2/wry drop target, which
  raises tauri's WindowEvent::DragDrop.

  THE INPUT PUMP IS THE WHOLE TRICK. DoDragDrop runs a modal loop that only advances when
  it sees real mouse input, and it only asks the drop source whether to continue on those
  same events. A first attempt that merely called SetCursorPos (and mouse_event with a
  zero-delta relative move) produced NO messages, so the loop sat idle forever and the
  script had to be killed. So we now do what a user does, with SendInput:

      left button DOWN at a source point  ->  a run of relative MOVEs across to the
      target  ->  left button UP over the target

  and the drop source uses the standard rule (button released => drop). The same reasoning
  applies AFTER the button-up: if OLE's loop does not observe that release it will never be
  woken again, so the gesture thread keeps injecting tiny moves until the drag returns.
  A wall-clock deadline in the source form's QueryContinueDrag handler is the backstop that
  guarantees the modal loop always terminates.

  NOTE ON TIMING: current builds enqueue the import and return from DoDragDrop as soon as
  the OS drop is accepted. A successful script exit therefore proves the end-user OLE
  gesture reached the application; callers must independently wait for the application's
  import lifecycle to report success.

.PARAMETER Files              One or more absolute paths to drop.
.PARAMETER ProcName           Exact executable process name that owns the target top-level window.
.PARAMETER WindowTitleLike    Diagnostic/fallback substring of the target top-level window title.
.PARAMETER DropX / DropY      Fractional point in the target's client rect (0..1).
.PARAMETER StartX / StartY    Fractional point the drag STARTS from, so there is real
                              travel across the window (a user never drops without moving).
.PARAMETER TimeoutSeconds     Hard deadline for the drag loop before it force-drops.
.PARAMETER HoverReadyPath     Optional new file written after the cursor is genuinely held
                              over the target. Use it as the screenshot-ready barrier.
.PARAMETER ReleasePath        Optional file the caller creates to release a held drag. Must
                              be supplied together with HoverReadyPath.
.PARAMETER HoverTimeoutSeconds
                              Hard deadline while waiting for ReleasePath.
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string[]] $Files,
  [string] $ProcName = "metrocalk-editor-shell",
  [string] $WindowTitleLike = "metrocalk",
  [double] $DropX = 0.55,
  [double] $DropY = 0.55,
  [double] $StartX = 0.92,
  [double] $StartY = 0.12,
  [int]    $Steps = 26,
  [int]    $TimeoutSeconds = 20,
  [string] $HoverReadyPath,
  [string] $ReleasePath,
  [int]    $HoverTimeoutSeconds = 30,
  [switch] $KeepCursor
)

$ErrorActionPreference = "Stop"

foreach ($f in $Files) {
  if (-not (Test-Path -LiteralPath $f)) { throw "No such file: $f" }
  if ((Get-Item -LiteralPath $f).PSIsContainer) { throw "Only files can be dropped: $f" }
}
if ([string]::IsNullOrWhiteSpace($HoverReadyPath) -xor [string]::IsNullOrWhiteSpace($ReleasePath)) {
  throw "HoverReadyPath and ReleasePath must be supplied together."
}
if (-not [string]::IsNullOrWhiteSpace($HoverReadyPath)) {
  $HoverReadyPath = [IO.Path]::GetFullPath($HoverReadyPath)
  $ReleasePath = [IO.Path]::GetFullPath($ReleasePath)
  if (Test-Path -LiteralPath $HoverReadyPath) { throw "HoverReadyPath already exists: $HoverReadyPath" }
  if (Test-Path -LiteralPath $ReleasePath) { throw "ReleasePath already exists: $ReleasePath" }
  $barrierDirectory = Split-Path -Path $HoverReadyPath -Parent
  if ($barrierDirectory -and -not (Test-Path -LiteralPath $barrierDirectory -PathType Container)) {
    throw "Hover barrier directory does not exist: $barrierDirectory"
  }
}

Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing

$src = @'
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using ComTypes = System.Runtime.InteropServices.ComTypes;

namespace MtkDrop
{
    [ComImport, Guid("00000121-0000-0000-C000-000000000046"),
     InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    public interface IDropSource
    {
        [PreserveSig] int QueryContinueDrag(int fEscapePressed, int grfKeyState);
        [PreserveSig] int GiveFeedback(int dwEffect);
    }

    // NOTE: the drag runs through WinForms' Control.DoDragDrop, which supplies its own IDropSource.
    // A hand-written IDropSource with a tick ceiling and deadline used to live here, but nothing ever
    // instantiated it and ole32!DoDragDrop was never called, so it bounded nothing while reading as if
    // it did. The real bound is the QueryContinueDrag handler attached to the source form below.

    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X, Y; }

    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT
    {
        public int dx; public int dy; public uint mouseData;
        public uint dwFlags; public uint time; public IntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT { public uint type; public MOUSEINPUT mi; }

    public static class Native
    {
        public const uint INPUT_MOUSE = 0;
        public const uint MOUSEEVENTF_MOVE = 0x0001;
        public const uint MOUSEEVENTF_LEFTDOWN = 0x0002;
        public const uint MOUSEEVENTF_LEFTUP = 0x0004;
        public const uint MOUSEEVENTF_ABSOLUTE = 0x8000;
        public const uint MOUSEEVENTF_VIRTUALDESK = 0x4000;

        [DllImport("ole32.dll")] public static extern int OleInitialize(IntPtr r);
        [DllImport("ole32.dll", PreserveSig = true)]
        public static extern int DoDragDrop(ComTypes.IDataObject d, IDropSource s, int ok, out int eff);

        [DllImport("user32.dll", SetLastError = true)]
        public static extern uint SendInput(uint n, INPUT[] inputs, int cb);
        [DllImport("user32.dll")] public static extern int GetSystemMetrics(int i);
        [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
        [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT p);
        [DllImport("user32.dll")] public static extern IntPtr WindowFromPoint(POINT p);
        [DllImport("user32.dll")] public static extern IntPtr GetAncestor(IntPtr h, uint flags);
        [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr h, out RECT r);
        [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
        [DllImport("user32.dll")] public static extern bool ClientToScreen(IntPtr h, ref POINT p);
        [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
        [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr h);
        [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int command);
        [DllImport("user32.dll")] public static extern bool IsChild(IntPtr parent, IntPtr child);
        [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
        [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
        [DllImport("user32.dll")] public static extern bool BringWindowToTop(IntPtr h);
        [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
        [DllImport("user32.dll")] public static extern bool PostMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
        [DllImport("user32.dll")] public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
        [DllImport("kernel32.dll")] public static extern IntPtr GetConsoleWindow();
        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int max);
        [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc cb, IntPtr l);
        public delegate bool EnumWindowsProc(IntPtr h, IntPtr l);

        private static Thread _gestureThread;
        private static Thread _pressThread;
        private static volatile string _gestureStatus = "idle";
        private static volatile string _pressStatus = "idle";
        private static Thread _watchdogThread;
        private static volatile bool _watchdogArmed;
        private static volatile string _watchdogStatus = "idle";
        // Set by the STA once Control.DoDragDrop returns, so the gesture thread can stop feeding the
        // modal loop the moment it is no longer modal.
        private static volatile bool _dragReturned = false;
        // How long to keep waking OLE's loop after the button-up before giving up and letting the
        // QueryContinueDrag deadline end the drag.
        public const int PostReleaseNudgeMs = 6000;
        public static string GestureStatus { get { return _gestureStatus; } }
        public static string PressStatus { get { return _pressStatus; } }
        public static string WatchdogStatus { get { return _watchdogStatus; } }
        public static void MarkDragReturned() { _dragReturned = true; }
        public static void ResetDragReturned() { _dragReturned = false; }

        // DoDragDrop owns the calling STA until the gesture completes. Drive the physical
        // mouse sequence from a plain CLR thread: a nested PowerShell runspace can stall in
        // Add-Type startup while the caller is inside OLE's modal loop, leaving the button
        // transition invisible to OLE forever.
        public static void StartGesture(
            int sx, int sy, int tx, int ty, int steps, IntPtr expectedTargetRoot,
            string hoverReadyPath, string releasePath, int hoverTimeoutMs)
        {
            if (_gestureThread != null && _gestureThread.IsAlive)
                throw new InvalidOperationException("A drag gesture is already running.");

            _gestureStatus = "starting";
            _gestureThread = new Thread(() =>
            {
                try
                {
                    Thread.Sleep(300); // let DoDragDrop enter its modal message loop first
                    _gestureStatus = "moving";
                    for (int i = 1; i <= steps; i++)
                    {
                        double u = i / (double)steps;
                        double e = u < 0.5 ? 2.0 * u * u : 1.0 - Math.Pow(-2.0 * u + 2.0, 2.0) / 2.0;
                        MoveAbs((int)Math.Round(sx + (tx - sx) * e),
                                (int)Math.Round(sy + (ty - sy) * e));
                        Thread.Sleep(35);
                    }
                    _gestureStatus = "hovering";
                    for (int i = 0; i < 6; i++)
                    {
                        MoveAbs(tx + (i % 3) - 1, ty + ((i + 1) % 3) - 1);
                        Thread.Sleep(45);
                    }
                    MoveAbs(tx, ty);
                    Thread.Sleep(120);
                    var targetPoint = new POINT { X = tx, Y = ty };
                    IntPtr under = WindowFromPoint(targetPoint);
                    IntPtr root = GetAncestor(under, 2); // GA_ROOT
                    if (root != expectedTargetRoot)
                    {
                        _gestureStatus = "target-hit-test-failed: under=" + under + " root=" + root;
                        try { PressEscape(); } catch { }
                        Button(false);
                        return;
                    }

                    if (!String.IsNullOrWhiteSpace(hoverReadyPath))
                    {
                        File.WriteAllText(hoverReadyPath,
                            "hovered=" + DateTimeOffset.UtcNow.ToString("O") + Environment.NewLine +
                            "target=" + expectedTargetRoot + Environment.NewLine +
                            "point=" + tx + "," + ty + Environment.NewLine +
                            "press=" + PressStatus + Environment.NewLine);
                        _gestureStatus = "held-over-target";
                        var releaseClock = Stopwatch.StartNew();
                        while (!File.Exists(releasePath))
                        {
                            if (releaseClock.ElapsedMilliseconds >= hoverTimeoutMs)
                            {
                                _gestureStatus = "hover-release-timeout";
                                try { PressEscape(); } catch { }
                                Button(false);
                                return;
                            }
                            // Keep generating tiny real pointer messages while held. This preserves
                            // the target's DragOver state and keeps OLE's modal loop responsive.
                            MoveAbs(tx + ((int)(releaseClock.ElapsedMilliseconds / 125) % 2), ty);
                            Thread.Sleep(125);
                        }
                        MoveAbs(tx, ty);
                        _gestureStatus = "release-authorized";
                        File.AppendAllText(hoverReadyPath,
                            "releaseObserved=" + DateTimeOffset.UtcNow.ToString("O") + Environment.NewLine);
                    }
                    Button(false);
                    _gestureStatus = "released";
                    if (!String.IsNullOrWhiteSpace(hoverReadyPath))
                        File.AppendAllText(hoverReadyPath,
                            "buttonReleased=" + DateTimeOffset.UtcNow.ToString("O") + Environment.NewLine);

                    // The hold loop above nudges the pointer precisely because OLE's modal loop only
                    // re-reads the button state when it dequeues an input message. Stopping those nudges
                    // at the release is what hung this script: if the loop does not observe THAT button-up
                    // (WebView2/DirectComposition targets coalesce and re-route pointer input), nothing
                    // further ever arrives to wake it, and DoDragDrop waits forever. Keep supplying
                    // messages until the drag actually returns.
                    var settleClock = Stopwatch.StartNew();
                    while (!_dragReturned && settleClock.ElapsedMilliseconds < PostReleaseNudgeMs)
                    {
                        MoveAbs(tx + ((int)(settleClock.ElapsedMilliseconds / 100) % 2), ty);
                        Thread.Sleep(100);
                    }
                    _gestureStatus = _dragReturned ? "drag-returned" : "release-nudge-exhausted";
                }
                catch (Exception ex)
                {
                    try { Button(false); } catch { }
                    _gestureStatus = "failed: " + ex.GetType().Name + ": " + ex.Message;
                }
            });
            _gestureThread.IsBackground = true;
            _gestureThread.Name = "MtkDrop.SendInputGesture";
            _gestureThread.Start();
        }

        public static bool WaitGesture(int milliseconds)
        {
            return _gestureThread == null || _gestureThread.Join(milliseconds);
        }

        public static void StartSourcePress(IntPtr sourceHwnd, int sx, int sy)
        {
            _pressStatus = "starting";
            _pressThread = new Thread(() =>
            {
                try
                {
                    // Let the Shown callback return so the source UI thread is actively
                    // dispatching before the synthetic button message arrives.
                    Thread.Sleep(350);
                    // WebDriver can reclaim foreground z-order after the Form.Shown callback. Reassert
                    // visibility/topmost immediately before the physical press so this HWND—not the
                    // destination WebView child—owns mouse capture when Control.DoDragDrop begins.
                    ShowWindow(sourceHwnd, 5); // SW_SHOW
                    SetWindowPos(sourceHwnd, new IntPtr(-1), 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0040);
                    BringWindowToTop(sourceHwnd);
                    SetForegroundWindow(sourceHwnd);
                    Thread.Sleep(120);
                    MoveAbs(sx, sy);
                    Thread.Sleep(180);
                    var point = new POINT { X = sx, Y = sy };
                    IntPtr under = WindowFromPoint(point);
                    RECT sourceRect;
                    bool hasSourceRect = GetWindowRect(sourceHwnd, out sourceRect);
                    string sourceState = " visible=" + IsWindowVisible(sourceHwnd) +
                        " rectOk=" + hasSourceRect + " rect=" + sourceRect.Left + "," + sourceRect.Top +
                        ".." + sourceRect.Right + "," + sourceRect.Bottom + " point=" + sx + "," + sy;
                    if (under != sourceHwnd && !IsChild(sourceHwnd, under))
                    {
                        // Node launches this helper with CREATE_NO_WINDOW so no console flashes in the
                        // captured film. Windows can apply that startup show-state to the first WinForms
                        // HWND too. Keep a real globally-held mouse button, but deliver the initiating
                        // WM_LBUTTONDOWN only to our own drag-source form. The destination still receives
                        // genuine OLE DragEnter/DragOver/Drop from Control.DoDragDrop; no target event is
                        // posted or simulated.
                        Button(true);
                        IntPtr clientPoint = new IntPtr((1 << 16) | 1); // MAKELPARAM(1, 1)
                        if (!PostMessageW(sourceHwnd, 0x0201, new IntPtr(1), clientPoint))
                        {
                            Button(false);
                            _pressStatus = "source-post-failed: under=" + under;
                            PostMessageW(sourceHwnd, 0x0010, IntPtr.Zero, IntPtr.Zero);
                            return;
                        }
                        _pressStatus = "pressed-source-post: under=" + under + sourceState;
                        return;
                    }
                    Button(true);
                    _pressStatus = "pressed" + sourceState;
                }
                catch (Exception ex)
                {
                    _pressStatus = "failed: " + ex.GetType().Name + ": " + ex.Message;
                    PostMessageW(sourceHwnd, 0x0010, IntPtr.Zero, IntPtr.Zero);
                }
            });
            _pressThread.IsBackground = true;
            _pressThread.Name = "MtkDrop.SourcePress";
            _pressThread.Start();
        }

        public static void PressEscape()
        {
            keybd_event(0x1B, 0, 0, UIntPtr.Zero);
            Thread.Sleep(30);
            keybd_event(0x1B, 0, 0x0002, UIntPtr.Zero);
        }

        public static void StartWatchdog(IntPtr sourceHwnd, int timeoutMs)
        {
            _watchdogArmed = true;
            _watchdogStatus = "armed";
            _watchdogThread = new Thread(() =>
            {
                Thread.Sleep(timeoutMs);
                if (!_watchdogArmed) return;
                _watchdogStatus = "fired";
                try { Button(false); } catch { }
                try { PressEscape(); } catch { }
                PostMessageW(sourceHwnd, 0x001F, IntPtr.Zero, IntPtr.Zero); // WM_CANCELMODE
                PostMessageW(sourceHwnd, 0x0010, IntPtr.Zero, IntPtr.Zero); // WM_CLOSE
            });
            _watchdogThread.IsBackground = true;
            _watchdogThread.Name = "MtkDrop.Watchdog";
            _watchdogThread.Start();
        }

        public static void DisarmWatchdog()
        {
            _watchdogArmed = false;
            if (_watchdogStatus == "armed") _watchdogStatus = "disarmed";
        }

        // SendInput's absolute coordinates are normalised to 0..65535 over the virtual desktop.
        public static void MoveAbs(int x, int y)
        {
            int vx = GetSystemMetrics(76), vy = GetSystemMetrics(77);   // SM_XVIRTUALSCREEN/Y
            int vw = GetSystemMetrics(78), vh = GetSystemMetrics(79);   // SM_CXVIRTUALSCREEN/CY
            if (vw <= 0) vw = 1; if (vh <= 0) vh = 1;
            int nx = (int)Math.Round((double)(x - vx) * 65535.0 / (double)(vw - 1));
            int ny = (int)Math.Round((double)(y - vy) * 65535.0 / (double)(vh - 1));
            var inp = new INPUT[1];
            inp[0].type = INPUT_MOUSE;
            inp[0].mi.dx = nx; inp[0].mi.dy = ny;
            inp[0].mi.dwFlags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
            if (SendInput(1, inp, Marshal.SizeOf(typeof(INPUT))) != 1)
                throw new InvalidOperationException("SendInput(move) failed: " + Marshal.GetLastWin32Error());
        }

        public static void Button(bool down)
        {
            var inp = new INPUT[1];
            inp[0].type = INPUT_MOUSE;
            inp[0].mi.dwFlags = down ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_LEFTUP;
            if (SendInput(1, inp, Marshal.SizeOf(typeof(INPUT))) != 1)
                throw new InvalidOperationException("SendInput(button) failed: " + Marshal.GetLastWin32Error());
        }

        public static IntPtr[] FindTopLevels(string titleLike)
        {
            var found = new List<IntPtr>();
            EnumWindows((h, l) =>
            {
                if (!IsWindowVisible(h)) return true;
                var sb = new StringBuilder(512);
                GetWindowTextW(h, sb, sb.Capacity);
                var t = sb.ToString();
                if (t.Length > 0 && t.IndexOf(titleLike, StringComparison.OrdinalIgnoreCase) >= 0)
                {
                    found.Add(h);
                }
                return true;
            }, IntPtr.Zero);
            return found.ToArray();
        }

        public static string TitleOf(IntPtr h)
        { var sb = new StringBuilder(512); GetWindowTextW(h, sb, sb.Capacity); return sb.ToString(); }
    }

}
'@

# Let Add-Type use the runtime's complete default BCL reference set. WinForms stays in
# PowerShell below, where its already-loaded runtime assembly can be used directly.
Add-Type -TypeDefinition $src | Out-Null

# Keep the interactive WinForms drag source visible, while moving PowerShell's console far outside the
# virtual desktop before any evidence capture. Hiding the console suppresses the first form on some
# Windows builds and breaks real mouse capture.
$consoleWindow = [MtkDrop.Native]::GetConsoleWindow()
if ($consoleWindow -ne [IntPtr]::Zero) {
  [void][MtkDrop.Native]::SetWindowPos($consoleWindow, [IntPtr](-2), -32000, -32000, 0, 0, 0x0001)
}

# -- locate + raise the target window -----------------------------------------
# Chromium/WebView automation can expose a visible, titled zero-client helper HWND, and tao publishes a
# visible unowned 16x16 'Tao Thread Event Target'. `MainWindowHandle` matches that event target as readily
# as the host, and aiming a drag at a 16x16 rect drops the file nowhere. Select the host by class instead.
. (Join-Path $PSScriptRoot "lib/app-window.ps1")
try {
  $hwnd = Get-MetrocalkAppWindow -ProcName $ProcName
}
catch {
  $fallbackWindows = @([MtkDrop.Native]::FindTopLevels($WindowTitleLike))
  Write-Output "DROP_RESULT: FAIL expected-one-host-window procName='$ProcName' error='$($_.Exception.Message)' titleFallbackCount=$($fallbackWindows.Count)"
  exit 2
}
Write-Output "DROP_TARGET: hwnd=$hwnd title='$([MtkDrop.Native]::TitleOf($hwnd))'"

$SWP_NOMOVE = 0x0002; $SWP_NOSIZE = 0x0001; $SWP_SHOWWINDOW = 0x0040
if ([MtkDrop.Native]::IsIconic($hwnd)) {
  [void][MtkDrop.Native]::ShowWindow($hwnd, 9) # SW_RESTORE
  Start-Sleep -Milliseconds 600
}

# PREFLIGHT. A drag can only land on a window that is visible when the button comes up. If another
# application owns a full-screen foreground, Windows re-minimises the editor mid-gesture, the cursor
# ends up over the desktop, and ole32's modal loop spins with no drop target until the harness kills
# it 45 seconds later - reported as "OLE timed out", which names neither the cause nor the culprit.
# Refuse in one second with the actual reason instead.
$foregroundConflict = Get-MetrocalkForegroundConflict
if ($foregroundConflict) {
  Write-Output "DROP_RESULT: FAIL foreground-conflict $foregroundConflict"
  exit 2
}
if ([MtkDrop.Native]::IsIconic($hwnd)) {
  Write-Output "DROP_RESULT: FAIL target-minimised the editor window re-minimised immediately after SW_RESTORE."
  exit 2
}
[void][MtkDrop.Native]::SetWindowPos($hwnd, [IntPtr](-1), 0, 0, 0, 0, ($SWP_NOMOVE -bor $SWP_NOSIZE -bor $SWP_SHOWWINDOW))
[void][MtkDrop.Native]::BringWindowToTop($hwnd)
[void][MtkDrop.Native]::SetForegroundWindow($hwnd)
Start-Sleep -Milliseconds 500
# The foreground raise above makes the destination visible, but leaving it HWND_TOPMOST prevents the
# later topmost WinForms drag source from reliably owning the initial mouse-down on WebView2 systems.
[void][MtkDrop.Native]::SetWindowPos($hwnd, [IntPtr](-2), 0, 0, 0, 0, ($SWP_NOMOVE -bor $SWP_NOSIZE -bor $SWP_SHOWWINDOW))

$cr = New-Object MtkDrop.RECT
[void][MtkDrop.Native]::GetClientRect($hwnd, [ref]$cr)
$o = New-Object MtkDrop.POINT; $o.X = 0; $o.Y = 0
[void][MtkDrop.Native]::ClientToScreen($hwnd, [ref]$o)
$w = $cr.Right - $cr.Left; $h = $cr.Bottom - $cr.Top
if ($w -le 0 -or $h -le 0) {
  Write-Output "DROP_RESULT: FAIL invalid-target-client ${w}x${h}"
  exit 2
}

$sx = [int]($o.X + $w * $StartX); $sy = [int]($o.Y + $h * $StartY)
$tx = [int]($o.X + $w * $DropX);  $ty = [int]($o.Y + $h * $DropY)
Write-Output "DROP_GEOM: client=${w}x${h} start=($sx,$sy) drop=($tx,$ty)"

# -- a genuine CF_HDROP payload -----------------------------------------------
$resolvedFiles = [Collections.Generic.List[string]]::new()
foreach ($f in $Files) {
  $full = (Resolve-Path -LiteralPath $f).ProviderPath
  $resolvedFiles.Add($full)
  Write-Output "DROP_FILE: $full"
}

$before = New-Object MtkDrop.POINT
[void][MtkDrop.Native]::GetCursorPos([ref]$before)

$under = New-Object MtkDrop.POINT; $under.X = $tx; $under.Y = $ty
$targetUnder = [MtkDrop.Native]::WindowFromPoint($under)
$targetRoot = [MtkDrop.Native]::GetAncestor($targetUnder, 2)
Write-Output "DROP_UNDER_CURSOR: hwnd=$targetUnder root=$targetRoot expectedRoot=$hwnd"
if ($targetRoot -ne $hwnd) {
  [void][MtkDrop.Native]::SetWindowPos($hwnd, [IntPtr](-2), 0, 0, 0, 0, ($SWP_NOMOVE -bor $SWP_NOSIZE))
  Write-Output "DROP_RESULT: FAIL target-hit-test root=$targetRoot expected=$hwnd"
  exit 2
}

# OLE routes mouse messages through the thread that owns the drag source's capture.
# A headless PowerShell caller has no HWND to own that capture, so a raw DoDragDrop can
# remain modal forever even when SendInput physically releases the button. Use a tiny
# real WinForms source window and Control.DoDragDrop, the same windowed OLE path used by
# Explorer and other desktop applications.
if ([Threading.Thread]::CurrentThread.ApartmentState -ne [Threading.ApartmentState]::STA) {
  throw "OLE file drag requires an STA thread; current apartment is $([Threading.Thread]::CurrentThread.ApartmentState)."
}

$fileList = New-Object System.Collections.Specialized.StringCollection
foreach ($full in $resolvedFiles) { [void]$fileList.Add($full) }
$dataObj = New-Object System.Windows.Forms.DataObject
$dataObj.SetFileDropList($fileList)

$state = [hashtable]::Synchronized(@{
  Started = $false
  Status = "not-started"
  Effect = [System.Windows.Forms.DragDropEffects]::None
  Error = $null
  SourceX = $null
  SourceY = $null
  Ending = "(never queried)"
})
# The drag runs through WinForms' Control.DoDragDrop, which supplies its OWN IDropSource. A hand-written
# IDropSource carrying a tick ceiling and a wall-clock deadline used to sit in the C# block above, but
# nothing ever instantiated it and ole32!DoDragDrop was never called - so that safety net was dead code
# and the gesture had no upper bound at all. Six consecutive runs hung here and were SIGTERMed by the
# harness. Enforce the documented -TimeoutSeconds on the path that actually executes.
$dragClock = New-Object System.Diagnostics.Stopwatch
$dragDeadlineMs = ($HoverTimeoutSeconds + $TimeoutSeconds) * 1000
$allowed = [System.Windows.Forms.DragDropEffects]::Copy -bor [System.Windows.Forms.DragDropEffects]::Link

# A real visible HWND receives the physical WM_LBUTTONDOWN. Its own handler then
# enters Control.DoDragDrop on the same STA, matching Explorer's input ownership.
$sourceForm = New-Object System.Windows.Forms.Form
$sourceForm.Text = "CAD file drag source - $([IO.Path]::GetFileName($resolvedFiles[0]))"
$sourceForm.FormBorderStyle = [System.Windows.Forms.FormBorderStyle]::FixedToolWindow
$sourceForm.ShowInTaskbar = $false
$sourceForm.TopMost = $true
$sourceForm.StartPosition = [System.Windows.Forms.FormStartPosition]::Manual
$sourceForm.ClientSize = New-Object System.Drawing.Size(220, 70)

# Prefer genuine desktop space beside the destination, like dragging from Explorer. Starting the source
# on top of the WebView makes its automation z-order compete with the helper form on some Windows builds.
$virtualScreen = [System.Windows.Forms.SystemInformation]::VirtualScreen
$sourceCenterX = $sx
$sourceCenterY = $sy
$targetRight = $o.X + $w
$targetLeft = $o.X
if (($virtualScreen.Right - $targetRight) -ge 250) {
  $sourceCenterX = $targetRight + 125
  $sourceCenterY = [Math]::Max($virtualScreen.Top + 55, [Math]::Min($virtualScreen.Bottom - 55, $o.Y + 110))
}
elseif (($targetLeft - $virtualScreen.Left) -ge 250) {
  $sourceCenterX = $targetLeft - 125
  $sourceCenterY = [Math]::Max($virtualScreen.Top + 55, [Math]::Min($virtualScreen.Bottom - 55, $o.Y + 110))
}
$sourceForm.Location = New-Object System.Drawing.Point(($sourceCenterX - 110), ($sourceCenterY - 35))

# WinForms raises this on the drag SOURCE for every input event OLE's modal loop dequeues. Its default
# already ends the drag on button-release; this only adds the missing upper bound, and only overrides
# the action when the deadline has genuinely expired.
$sourceForm.add_QueryContinueDrag({
  param($sender, $eventArgs)
  if ($eventArgs.EscapePressed) {
    $state.Ending = "escape"
    $eventArgs.Action = [System.Windows.Forms.DragAction]::Cancel
    return
  }
  if ($eventArgs.Action -ne [System.Windows.Forms.DragAction]::Continue) {
    $state.Ending = "button-released"
    return
  }
  if ($dragClock.IsRunning -and $dragClock.ElapsedMilliseconds -gt $dragDeadlineMs) {
    $state.Ending = "deadline"
    $eventArgs.Action = [System.Windows.Forms.DragAction]::Drop
  }
}.GetNewClosure())

$sourceForm.add_MouseDown({
  param($sender, $eventArgs)
  if ($eventArgs.Button -ne [System.Windows.Forms.MouseButtons]::Left) { return }
  if ($state.Started) { return }
  $state.Started = $true
  $state.Status = "ole-active"
    $sender.Text = "CAD file drag active"
    try {
      [MtkDrop.Native]::StartGesture(
      ([int]$state.SourceX), ([int]$state.SourceY), $tx, $ty, $Steps, $hwnd,
      $HoverReadyPath, $ReleasePath, ($HoverTimeoutSeconds * 1000))
    [MtkDrop.Native]::ResetDragReturned()
    $dragClock.Restart()
    $state.Effect = $sender.DoDragDrop($dataObj, $allowed)
    $state.Status = "ole-returned"
  }
  catch {
    $state.Error = $_.Exception.ToString()
    $state.Status = "ole-exception"
  }
  finally {
    $dragClock.Stop()
    # Stops the post-release nudges immediately; without this they would run their full budget.
    [MtkDrop.Native]::MarkDragReturned()
    [MtkDrop.Native]::DisarmWatchdog()
    [MtkDrop.Native]::Button($false)
    $sender.Close()
  }
}.GetNewClosure())

$sourceForm.add_Shown({
  param($sender, $eventArgs)
  $SWP_NOMOVE = 0x0002; $SWP_NOSIZE = 0x0001; $SWP_SHOWWINDOW = 0x0040
  [void][MtkDrop.Native]::SetWindowPos(
    $sender.Handle, [IntPtr](-1), 0, 0, 0, 0,
    ($SWP_NOMOVE -bor $SWP_NOSIZE -bor $SWP_SHOWWINDOW))
  [void]$sender.Activate()
  $sender.BringToFront()
  [void][MtkDrop.Native]::SetForegroundWindow($sender.Handle)
  $sender.Text = "CAD file drag source ready"

  # WinForms can reposition or DPI-scale a manually placed tool window. Measure the real client centre
  # after it is shown instead of assuming the requested Location survived unchanged.
  $sourceClientCenter = [System.Drawing.Point]::new(
    [int]($sender.ClientSize.Width / 2),
    [int]($sender.ClientSize.Height / 2)
  )
  $sourcePoint = $sender.PointToScreen($sourceClientCenter)
  $state.SourceX = $sourcePoint.X
  $state.SourceY = $sourcePoint.Y

  [MtkDrop.Native]::StartWatchdog($sender.Handle, ($TimeoutSeconds * 1000))
  [MtkDrop.Native]::StartSourcePress($sender.Handle, $state.SourceX, $state.SourceY)
}.GetNewClosure())

$sw = [Diagnostics.Stopwatch]::StartNew()
try {
  [System.Windows.Forms.Application]::Run($sourceForm)
}
catch {
  Write-Output "DROP_RESULT: FAIL exception=$($_.Exception.Message)"
  [void][MtkDrop.Native]::WaitGesture(2000)
  Write-Output "DROP_GESTURE: $([MtkDrop.Native]::GestureStatus)"
  [MtkDrop.Native]::Button($false)
  exit 3
}
finally {
  $sw.Stop()
  [MtkDrop.Native]::DisarmWatchdog()
  [void][MtkDrop.Native]::WaitGesture(2000)
  # Never leave a button stuck down, whatever happened above.
  [MtkDrop.Native]::Button($false)
  $sourceForm.Close()
  $sourceForm.Dispose()
  [void][MtkDrop.Native]::SetWindowPos($hwnd, [IntPtr](-2), 0, 0, 0, 0, ($SWP_NOMOVE -bor $SWP_NOSIZE))
  if (-not $KeepCursor) { [void][MtkDrop.Native]::SetCursorPos($before.X, $before.Y) }
}

Write-Output "DROP_GESTURE: $([MtkDrop.Native]::GestureStatus)"
Write-Output "DROP_PRESS: $([MtkDrop.Native]::PressStatus)"
Write-Output "DROP_WATCHDOG: $([MtkDrop.Native]::WatchdogStatus)"
Write-Output "DROP_SOURCE_GEOM: measured=($($state.SourceX),$($state.SourceY)) requested=($sx,$sy)"
Write-Output "DROP_SOURCE: started=$($state.Started) status=$($state.Status)"
Write-Output "DROP_EFFECT: $($state.Effect) elapsedMs=$($sw.ElapsedMilliseconds) ending=$($state.Ending)"
Write-Output "DROP_GESTURE: $([MtkDrop.Native]::GestureStatus)"
if ($null -ne $state.Error) {
  Write-Output "DROP_RESULT: FAIL source=$($state.Error)"
  exit 3
}
if (-not $state.Started) {
  Write-Output "DROP_RESULT: FAIL source-never-started status=$($state.Status)"
  exit 3
}
if ($state.Effect -eq [System.Windows.Forms.DragDropEffects]::None) {
  Write-Output "DROP_RESULT: CANCELLED"
  exit 4
}
Write-Output "DROP_RESULT: OK os-accepted effect=$($state.Effect); await application import lifecycle for engine success"
exit 0
