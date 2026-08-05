// LIVE CAD import campaign (M15.7 / ADR-077) — import the REAL CATIA 3DXML (and the STEP AP242) into the
// packaged .exe through the never-empty/never-silent pipeline (File→Import routes CAD files to `land_cad`),
// frame the assembly, and OS-capture the COMPOSITED window (native wgpu under the transparent WebView2 — a
// WebDriver screenshot only sees the transparent DOM). Clean, empty scene via MTK_SCENE_N=0.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver; the .exe must be built):
//   set MTK_CAD_FILE=<path to .3dxml/.stp>  &  set MTK_SHOT_DIR=<out dir>
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.cadtest.conf.js

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.resolve(dir, "../src-tauri/target/release");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-cadtest/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 1800000 }, // 30 min — a 262 MB STEP parse + a 2,678-entity land is slow
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
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
    try {
      tauriDriver?.kill();
    } catch {
      /* already gone */
    }
  },
};
