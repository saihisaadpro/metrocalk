// ADR-192 — "shoot from this view", proven on the packaged `.exe` by comparing CAMERAS.
//
// The claim this config exists to test cannot be passed by relabelling anything: after the gesture,
// the pose the cutscene runtime films from must be the pose the viewport was standing at — and it
// must differ, by metres, from the pose the shot's own card solves. Two numbers from
// `camera_probe`, taken through the same command the panel calls.
//
// Local-only, for the standing reason the rest of this directory is: a display, a WebView2-matched
// `msedgedriver`, and a GPU the wgpu surface can be created on.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.placedcamera.conf.js

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

// An empty scene: the only cutscene in the document is the one this spec authors, and the only
// objects in it are the two it spawns — so "the card would have filmed somewhere else" is a
// statement about a scene whose contents are known.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-placedcamera/placed-camera.e2e.js"],
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
