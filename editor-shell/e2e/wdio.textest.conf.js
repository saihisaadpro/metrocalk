// LIVE M11.2 texture visual (MR + normal maps): import a full-PBR demo tile (base + metallic-roughness +
// normal map) into the packaged .exe and capture OS-level screenshots of the COMPOSITED window (native wgpu
// under the transparent WebView2). Empty seed (MTK_SCENE_N=0) so the tile is the only thing in view.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver):
//   set MTK_SHOT_DIR=<out dir>  &  set MTK_SHOT_PS1=<capture.ps1>  &  set MTK_TEX_GLB=<normal_mapped_quad.glb>
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.textest.conf.js

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.resolve(dir, "../src-tauri/target/release");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// Empty seed so the imported tile is the only mesh in view.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-textest/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
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
    tauriDriver?.kill();
  },
};
