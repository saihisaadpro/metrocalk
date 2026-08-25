# Hold the display awake for the lifetime of THIS process, and dismiss a screensaver that has already
# engaged.
#
# Why the film needs it: `captureComposited` uses PrintWindow, which reads the window's own presentation
# and works while the window is occluded or the screensaver is up -- that is why the still evidence keeps
# landing. The film does not: `ffmpeg -f gdigrab -i desktop` reads the DESKTOP, and while a screensaver
# owns it every frame fails with Windows error 5 (access denied):
#
#   [gdigrab] Failed to capture image (error 5)
#   [gdigrab] Could not find codec parameters for stream 0 ... unspecified size
#   Output file does not contain any stream
#
# which the harness reports as "neither hardware nor software H.264 capture passed preflight" -- a
# message about encoders, for a problem that is about the desktop. On this machine the culprit is ASUS's
# `OLED Care Screensaver.scr`, which engages on idle during an unattended run.
#
# `SetThreadExecutionState(ES_DISPLAY_REQUIRED | ES_CONTINUOUS)` is the documented way to say "something
# is presenting, do not blank the display" -- it is what a video player holds while playing. It changes
# NO user setting and is scoped to this process: when the script exits (or is killed) the request is
# dropped and the machine's own idle policy resumes untouched. It does not, and must not, unlock a locked
# workstation; if the session is genuinely locked this cannot help and the run should say so instead.
#
# Usage: start it before the run and stop the process afterwards.
#   $keeper = Start-Process powershell -PassThru -ArgumentList '-NoProfile','-File','scripts/keep-display-awake.ps1'
#   ...
#   Stop-Process -Id $keeper.Id

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Mtk -Name Power -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true)]
public static extern uint SetThreadExecutionState(uint esFlags);
'@

# Written as decimals: PowerShell 5.1 parses `0x80000000` as a negative Int32, and casting that to
# [uint32] throws "Value was either too large or too small".
$ES_CONTINUOUS       = [uint32]2147483648
$ES_DISPLAY_REQUIRED = [uint32]2
$ES_SYSTEM_REQUIRED  = [uint32]1

# A screensaver already up owns the desktop; ending it is the programmatic equivalent of the keypress a
# person at the machine would make. Only actual screensaver executables are touched.
$dismissed = @()
foreach ($process in Get-Process -ErrorAction SilentlyContinue) {
    $path = $null
    try { $path = $process.Path } catch { continue }
    if ($path -and $path.ToLowerInvariant().EndsWith('.scr')) {
        try {
            Stop-Process -Id $process.Id -Force -ErrorAction Stop
            $dismissed += "$($process.ProcessName) ($($process.Id))"
        } catch {
            Write-Output "could not dismiss $($process.ProcessName): $_"
        }
    }
}
if ($dismissed.Count -gt 0) { Write-Output "dismissed screensaver: $($dismissed -join ', ')" }

$state = [Mtk.Power]::SetThreadExecutionState($ES_CONTINUOUS -bor $ES_DISPLAY_REQUIRED -bor $ES_SYSTEM_REQUIRED)
if ($state -eq 0) {
    throw "SetThreadExecutionState was refused (last error $([System.Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
}
Write-Output "display kept awake by pid $PID -- stop this process to release it"

# Re-assert periodically. The flag is per-thread and continuous, so one call is enough in principle; the
# loop is what keeps the process (and therefore the request) alive, and it re-dismisses a screensaver
# that manages to start anyway.
while ($true) {
    Start-Sleep -Seconds 20
    [void][Mtk.Power]::SetThreadExecutionState($ES_CONTINUOUS -bor $ES_DISPLAY_REQUIRED -bor $ES_SYSTEM_REQUIRED)
    foreach ($process in Get-Process -ErrorAction SilentlyContinue) {
        $path = $null
        try { $path = $process.Path } catch { continue }
        if ($path -and $path.ToLowerInvariant().EndsWith('.scr')) {
            try { Stop-Process -Id $process.Id -Force -ErrorAction Stop } catch {}
        }
    }
}
