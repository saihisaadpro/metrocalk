// Reap the process tree an E2E run leaves behind.
//
// The harness spawned `tauri-driver`, which spawns `msedgedriver`, which spawns the packaged app, which
// spawns a family of WebView2 host processes. Teardown killed exactly one of those — `tauri-driver` — so
// any run that crashed or timed out orphaned the rest. They accumulate: a single afternoon of failed runs
// left fifteen live processes behind, and once enough of them are holding the automation port and the
// app's user-data directory, every SUBSEQUENT run fails too — each one slightly earlier than the last,
// which reads like a new bug every time rather than the same debris.
//
// So this runs on the way IN as well as on the way out. A run must not inherit the previous run's mess.
//
// Deliberately targeted rather than by-name-only: `msedgewebview2` is shared with any other WebView2
// application the user happens to be running, very much including the editor they may be reading this in.
// Only hosts whose command line points at OUR application are ours to kill.

import { execFileSync } from "node:child_process";
import { mkdtempSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";

// Written to a real .ps1 and run with -File. Passing this as a -Command string means collapsing it to one
// line, and PowerShell then needs every statement separated by `;` — a quoting problem that silently
// turns cleanup into a no-op, which is exactly the failure this module exists to prevent.
const SCRIPT = `
$ErrorActionPreference = 'SilentlyContinue'
$killed = 0
foreach ($name in @('metrocalk-editor-shell', 'tauri-driver', 'msedgedriver')) {
  foreach ($p in @(Get-Process -Name $name)) {
    try { Stop-Process -Id $p.Id -Force -ErrorAction Stop; $killed++ } catch { }
  }
}
$hosts = @(Get-CimInstance Win32_Process -Filter "Name = 'msedgewebview2.exe'" |
  Where-Object { $_.CommandLine -like '*metrocalk*' })
foreach ($p in $hosts) {
  try { Stop-Process -Id $p.ProcessId -Force -ErrorAction Stop; $killed++ } catch { }
}
Write-Output $killed
`;

/**
 * Kill this harness's leftovers.
 *
 * @param {string} [tag] label for the log line, so it is obvious which phase reaped what.
 * @returns {number} how many processes were killed.
 */
export function reapHarnessProcesses(tag = "reap") {
  let dir;
  try {
    dir = mkdtempSync(path.join(tmpdir(), "mtk-reap-"));
    const ps1 = path.join(dir, "reap.ps1");
    writeFileSync(ps1, SCRIPT, "utf8");
    const out = execFileSync(
      "powershell",
      ["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File", ps1],
      { encoding: "utf8", timeout: 30000 },
    );
    const killed = Number.parseInt(String(out).trim().split(/\s+/).pop() ?? "0", 10) || 0;
    if (killed > 0) console.log(`[${tag}] reaped ${killed} leftover harness process(es)`);
    return killed;
  } catch (e) {
    // Reaping is best-effort housekeeping: never fail a run because cleanup could not run.
    console.warn(`[${tag}] could not reap harness processes:`, e.message);
    return 0;
  } finally {
    if (dir) rmSync(dir, { recursive: true, force: true });
  }
}
