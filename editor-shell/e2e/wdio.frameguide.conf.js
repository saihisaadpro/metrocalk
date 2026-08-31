// ADR-193 — the frame guide, proven on the packaged `.exe` by comparing RECTANGLES.
//
// The claim cannot be passed by drawing bars: the rectangle the author composes in and the rectangle
// the shot is delivered in must be the SAME rectangle, and without the guide they must differ. Both
// numbers come from `camera_probe`, which reports the frame the projection was actually sheared to.
//
// Local-only, for the standing reason the rest of this directory is: a display, a WebView2-matched
// `msedgedriver`, and a GPU the wgpu surface can be created on.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.frameguide.conf.js

import { spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application =
  process.env.MTK_EXE || path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const exeDir = path.dirname(application);
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// An empty scene: the only cutscene in the document is the one this spec authors.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-frameguide/frame-guide.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 300000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    // A stale replay log would open yesterday's cutscene into today's assertions.
    for (const f of ["metrocalk-scene.jsonl", "metrocalk-wallet.json", "metrocalk-recents.json"]) {
      const p = path.join(exeDir, f);
      if (existsSync(p)) rmSync(p, { force: true });
    }
    return new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2500);
    });
  },

  onComplete: () => {
    try {
      tauriDriver?.kill();
    } catch {
      /* already gone */
    }
  },
};
