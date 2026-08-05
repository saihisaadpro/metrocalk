// M14.3 SCRUB-MOVES-THE-STAGE visual convergence (ADR-059, prompt 66 / 40): the one owed Accepted-tier
// gate — prove a NumericField *scrub* in the inspector VISIBLY transforms the selected entity on the native
// wgpu stage (a real-pixel before/after, NOT a DOM screenshot — the viewport composites under the
// transparent WebView2). Drives the inspector's real scrub commit path (`submit_edit`) against the REAL
// /core and captures OS-level CopyFromScreen frames of the composited window.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver):
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.scrubtest.conf.js
//
// Env overrides (all self-resolve in the spec if unset): MTK_SCENE_N, MTK_SHOT_DIR, MTK_SHOT_PS1, MTK_PNGDIFF_PS1.

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.resolve(dir, "../src-tauri/target/release");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// A small seeded scene → frame_all gives a wide view so a ~2-unit scrub is a clear in-frame shift (not a
// 5000-cube stress wall, not empty). The forced HealthBar at i=0 is a guaranteed visible cube.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "12";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-scrubtest/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    // Fresh slate: no last-project reopen, no stale scene-replay, full wallet.
    for (const f of ["metrocalk-recents.json", "metrocalk-scene.jsonl", "metrocalk-wallet.json"]) {
      try {
        rmSync(path.join(relDir, f), { force: true });
      } catch {
        /* nothing to clean */
      }
    }
  },

  beforeSession: () =>
    new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2500);
    }),

  afterSession: () => {
    tauriDriver?.kill();
  },
};
