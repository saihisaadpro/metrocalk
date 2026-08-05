// LIVE provenance campaign (M11.5 / ADR-044): import textured assets into the packaged .exe and read back
// the `asset_provenance` projection over the invoke bridge — verifying that the import path RECORDS a
// provenance record (kind/source/content-hash/perceptual-hash) and that a perceptual near-duplicate (same
// texture, different bytes) is HINTED. No screenshots: this verifies the backend wiring, not pixels.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver):
//   set MTK_ASSET_DIR=<dir of demo .glb>  &  set MTK_OUT_DIR=<out dir>
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.provtest.conf.js

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.resolve(dir, "../src-tauri/target/release");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// A clean, empty seed so the imported assets are the only meshes (the stress scene would bury them).
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-provtest/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    // Fresh slate: no last-project reopen, no stale scene-replay, no leftover imported blobs (so the
    // near-duplicate hint reflects only what THIS run imports, not a previous session's assets).
    for (const f of ["metrocalk-recents.json", "metrocalk-scene.jsonl", "metrocalk-wallet.json"]) {
      try {
        rmSync(path.join(relDir, f), { force: true });
      } catch {
        /* nothing to clean */
      }
    }
    try {
      rmSync(path.join(relDir, "metrocalk-assets"), { recursive: true, force: true });
    } catch {
      /* no persisted blobs yet */
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
