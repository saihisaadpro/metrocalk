// WebdriverIO config for the M15.11 HDR-pipeline visual verification (see wdio.viewportstress.conf.js for
// the framing audit it is modelled on): drive the packaged .exe and capture OS-composited pixels across
// every camera mode, exposure, presentation profile, overlay state and a real native resize.
//
// The renderer reads MSAA / SSAO / bloom / sky from the environment ONCE at device creation, so a routing
// combination cannot be switched inside a session. This config passes the current environment straight
// through to the app, and `scripts/hdr-capture-all.ps1` runs it once per combination with a different
// MTK_* set and MTK_SHOT_DIR.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver; bootstrap first):
//   node bootstrap.mjs
//   powershell -File scripts/hdr-capture-all.ps1              # every routing combination
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.hdr.conf.js   # just the current environment

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const sceneLog = path.resolve(dir, "../src-tauri/target/release/metrocalk-scene.jsonl");
const walletFile = path.resolve(dir, "../src-tauri/target/release/metrocalk-wallet.json");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",

  // `MTK_HDR_SPEC` selects the thumbnail check instead of the capture sweep; both drive the same app.
  specs: [process.env.MTK_HDR_SPEC || "./specs/hdr-pipeline.e2e.js"],
  maxInstances: 1, // serial — pixel capture must run on an otherwise-idle session

  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],

  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 240000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    // Clean, deterministically-seeded slate: drop the scene log + reset the wallet to the full free grant.
    try {
      rmSync(sceneLog, { force: true });
      rmSync(walletFile, { force: true });
    } catch {
      /* nothing to clean — fine */
    }
    // …and clear THIS configuration's shot directory. Frames are numbered by capture order, so a run that
    // stopped early leaves a differently-numbered set behind and the two interleave into a folder that
    // looks complete and is not. The audit set has to be exactly one run's worth.
    try {
      rmSync(path.resolve(dir, `.shots-hdr/${process.env.MTK_SHOT_DIR || "default"}`), {
        recursive: true,
        force: true,
      });
    } catch {
      /* nothing to clean — fine */
    }
  },

  beforeSession: () =>
    new Promise((resolve) => {
      // tauri-driver spawns the app as a child, so the MTK_* configuration in THIS process's environment
      // is what the renderer reads. That is the whole mechanism by which a routing combination is selected.
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
        env: process.env,
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000);
    }),

  afterSession: () => {
    tauriDriver?.kill();
  },
};
