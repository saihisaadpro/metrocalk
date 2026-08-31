// ADR-175 — rendering a cutscene to files, on the PACKAGED .exe. The claims are the PNGs themselves:
// how many were written, what shape they are, and whether consecutive frames differ.
//
// Empty scene (MTK_SCENE_N=0) so only what the test creates appears in front of the camera.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.render.conf.js

import { spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

// Clean slate beside the exe: a stale replay log would replay yesterday's shapes into today's frames.
const exeDir = path.dirname(application);
for (const f of ["metrocalk-scene.jsonl", "metrocalk-wallet.json", "metrocalk-recents.json"]) {
  const p = path.join(exeDir, f);
  if (existsSync(p)) rmSync(p);
}

// One run's evidence = one folder's worth. The rendered sequences land here too, so a previous run's
// frames can never be counted as this one's — which is the whole assertion in two of these cases.
const shots = path.resolve(dir, ".shots-render");
if (existsSync(shots)) rmSync(shots, { recursive: true });

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-render/cinema-render.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

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
