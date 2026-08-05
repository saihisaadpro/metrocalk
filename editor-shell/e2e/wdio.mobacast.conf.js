// WebdriverIO config for the GP-13 REACHABLE ABILITY run (see wdio.moba.conf.js for the pipeline run): drive a real match in the packaged .exe and capture the
// OS-composited pixels at each beat. Modelled on the acceptance conf (same tauri-driver harness) — the exhaustive, re-runnable gate over
// every user-facing control of the packaged .exe. Extends the base harness (tauri-driver → WebView2 DOM +
// the transparent viewport div → native pick) with: the acceptance specs under ./specs/acceptance, the
// console-error guard installed per session, and an `after` hook that writes the machine-readable report +
// the generated COVERAGE.md once the run completes.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver; bootstrap first):
//   node bootstrap.mjs
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.acceptance.conf.js
//   set MTK_PROFILE=min-spec & node "...wdio.js" run wdio.acceptance.conf.js   # the min-spec profile

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

// Deliberately NOT the acceptance gate's 5k stress fixture. This run captures pixels a human has to be
// able to read, and it authors a match into whatever scene is loaded — five thousand unrelated objects
// would bury the lane in the frame and prove nothing about the match. The small default sample stands.

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",

  specs: ["./specs/moba-cast.e2e.js"],
  maxInstances: 1, // serial — the budget capture must run on an otherwise-idle session

  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],

  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  // ≥ 30 min of continuous testing across the full control surface.
  mochaOpts: { ui: "bdd", timeout: 180000 },
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
  },

  beforeSession: () =>
    new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000);
    }),


  // Worker-side: runs after all this worker's specs (maxInstances 1 → the whole run) → write artifacts once.

  afterSession: () => {
    tauriDriver?.kill();
  },
};
