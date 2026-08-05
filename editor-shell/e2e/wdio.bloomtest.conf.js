// LIVE M11.4 bloom check: a bright chrome scene captured with MTK_BLOOM=off vs =on so the glow on bright
// highlights can be compared. MTK_BLOOM propagates from this process's env → tauri-driver → the app.
//   set MTK_BLOOM=on|off & set MTK_SHOT_DIR=<out> & set MTK_SHOT_PS1=<capture.ps1> & set MTK_FIX_DIR=<dir>
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.bloomtest.conf.js

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
  specs: ["./specs-bloomtest/**/*.e2e.js"],
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
