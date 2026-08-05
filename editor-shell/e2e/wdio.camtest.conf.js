// LIVE camera look-through (M11.4 visual verification): author a scene camera in the packaged .exe and look
// through it, capturing OS-level screenshots of the COMPOSITED window (native wgpu under the transparent
// WebView2). A small seeded scene (MTK_SCENE_N) gives visible content so the view change is unmistakable.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver):
//   set MTK_SHOT_DIR=<out dir>  &  set MTK_SHOT_PS1=<capture.ps1>
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.camtest.conf.js

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.resolve(dir, "../src-tauri/target/release");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// A small seeded scene so look-through has visible content (not the 5000-cube stress wall, not empty).
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "40";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-camtest/**/*.e2e.js"],
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
