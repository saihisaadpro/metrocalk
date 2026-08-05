// Matched packaged-app viewport quality captures. Required env:
// MTK_SHOT_DIR, MTK_SHOT_PS1 and MTK_FIX_DIR. Set MTK_CAPTURE_BASELINE=1 for one pre-change view per asset.

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.resolve(dir, "../src-tauri/target/release");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

process.env.MTK_SCENE_N = "0";
let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-renderquality/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    for (const file of ["metrocalk-recents.json", "metrocalk-scene.jsonl", "metrocalk-wallet.json"]) {
      rmSync(path.join(relDir, file), { force: true });
    }
  },
  beforeSession: () => new Promise((resolve) => {
    tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
      stdio: [null, process.stdout, process.stderr],
    });
    tauriDriver.on("error", (error) => console.error("tauri-driver failed to start:", error));
    setTimeout(resolve, 2500);
  }),
  afterSession: () => tauriDriver?.kill(),
};
