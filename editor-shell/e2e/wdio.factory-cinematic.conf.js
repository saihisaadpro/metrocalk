// Production factory direction pass. The base native-drop config remains the owner of exact clean-state
// archival, executable/fixture hashing, driver startup, and post-run evidence preservation.
//
// PowerShell run example (the fixture is intentionally mandatory here):
//   $env:MTK_NATIVE_DROP_FIXTURE = 'X:\Work\Metrocalk\Games Projects\Unreal\Skid Weld Line A.1\Skid Weld Line A.1_(1).stp'
//   node "node_modules/@wdio/cli/bin/wdio.js" run wdio.factory-cinematic.conf.js

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { browser } from "@wdio/globals";

import { config as nativeCadDropConfig } from "./wdio.native-cad-drop.conf.js";
import { assertExeMatchesTree } from "./lib/build-freshness.js";
import { reapHarnessProcesses } from "./lib/reap.js";

if (!process.env.MTK_NATIVE_DROP_FIXTURE) {
  throw new Error(
    "MTK_NATIVE_DROP_FIXTURE must name the production Skid Weld Line STEP file; "
      + "the analytic fixture remains the default only for wdio.native-cad-drop.conf.js.",
  );
}

// The production film is intentionally rendered at the engine's highest directional-shadow tier.
// This is inherited by tauri-driver and the packaged application it launches.
process.env.MTK_SHADOW_QUALITY = "high";

const confDir = path.dirname(fileURLToPath(import.meta.url));

// A production film costs about twenty minutes and is the only measurement this lane trusts. Check
// BEFORE any of it that the binary about to be filmed contains the tree it will be credited to - a
// stale .exe does not announce itself, it just quietly reproduces the previous build's numbers.
assertExeMatchesTree(
  process.env.MTK_EXE || path.resolve(confDir, "../src-tauri/target/release/metrocalk-editor-shell.exe"),
  path.resolve(confDir, "../.."),
);

const keepDisplayAwakeScript = path.resolve(confDir, "scripts/keep-display-awake.ps1");
let displayKeeper = null;

/**
 * Hold the display awake for the whole run.
 *
 * `scripts/keep-display-awake.ps1` was written for exactly this film and then documented as something
 * an operator starts by hand before the run -- which is to say, wired to nothing. An unattended run is
 * precisely when this machine's OLED-care screensaver engages, and while a screensaver owns the desktop
 * every `gdigrab` frame fails with error 5. The harness reports that as "neither hardware nor software
 * H.264 capture passed preflight": a message about encoders, for a problem about the desktop.
 *
 * A guard that depends on somebody remembering it is not a guard. This is the film's own config, so the
 * film's own prerequisite belongs here rather than in the shared native-drop base.
 */
function startDisplayKeeper() {
  const child = spawn("powershell.exe", [
    "-NoLogo",
    "-NoProfile",
    "-NonInteractive",
    "-ExecutionPolicy",
    "Bypass",
    "-File",
    keepDisplayAwakeScript,
  ], { stdio: ["ignore", "pipe", "pipe"], windowsHide: true });
  child.stdout?.setEncoding("utf8");
  child.stderr?.setEncoding("utf8");
  child.stdout?.on("data", (chunk) => process.stdout.write(`[display-keeper] ${chunk}`));
  child.stderr?.on("data", (chunk) => process.stderr.write(`[display-keeper] ${chunk}`));
  // It only exits on its own if it FAILED -- the success path is an endless re-assert loop. Say so,
  // rather than letting a silently dead keeper look the same as a working one.
  child.on("exit", (code, signal) => {
    if (displayKeeper === child) {
      process.stderr.write(`[display-keeper] exited early (code ${code}, signal ${signal}); `
        + "the display is no longer being held awake for this run.\n");
    }
  });
  return child;
}

export const config = {
  ...nativeCadDropConfig,
  specs: ["./specs-factory-cinematic/factory-cinematic.e2e.js"],
  logLevel: "warn",
  mochaOpts: { ui: "bdd", timeout: 3_600_000 },
  connectionRetryTimeout: 600_000,
  connectionRetryCount: 2,

  // WebDriver caps an `execute/sync` at 30 SECONDS by default, and every engine command in this spec
  // goes through one. On the production factory the heavy commands genuinely exceed that: the import is
  // already fire-and-forget because of it, and entering Play does a document snapshot, a physics
  // recording, a rules recording and a role-roster walk over 17,793 entities before it answers.
  //
  // A cap that fires does not cancel the command -- the engine finishes the work regardless -- it only
  // desynchronises the session, so the failure reads as "script timeout" no matter what the app did.
  // Raising it to five minutes lets a slow command be observed AS a slow command; the engine's own
  // `metrocalk-diagnostics.log` reports what it cost.
  before: async () => {
    await browser.setTimeout({ script: 300_000 });
  },

  onPrepare: async (...args) => {
    displayKeeper = startDisplayKeeper();
    await nativeCadDropConfig.onPrepare?.(...args);
  },

  onComplete: async (...args) => {
    if (displayKeeper) {
      const keeper = displayKeeper;
      displayKeeper = null;   // so the exit handler does not report the deliberate stop as an early death
      try { keeper.kill(); } catch { /* already gone */ }
    }
    await nativeCadDropConfig.onComplete?.(...args);
  },
};
