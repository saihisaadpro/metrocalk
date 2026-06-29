// WebdriverIO config for the RELOAD regression — the live half of the Prompt 22 fix. Where the main
// `wdio.conf.js` deletes the scene log for a clean seed, this config does the opposite: it WRITES a
// known-good log (a HealthBar tracking two providers, at the shell's deterministic SCENE_N=5000 id
// space — ids verified live) BEFORE launch, so the app starts as if reopened after a prior session's
// binds. The spec then asserts the real .exe *surfaces* that restored state on load (the bug that was
// fixed): the requirer shows a "tracking" badge, auto-focus selects it, and its "tracking" list is
// populated — none of which held on the pre-fix frontend.
//
// (A literal kill-and-relaunch within one tauri-driver session isn't expressible — tauri-driver owns a
// single app launch. So the loop is split: the headless `reload_surfacing.rs` proves write→replay
// restores the data; this proves a restored log surfaces as visible tracking in the live shell.)

import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const sceneLog = path.resolve(dir, "../src-tauri/target/release/metrocalk-scene.jsonl");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// Reload regression pre-seeds a known-good log against the at-scale fingerprint; the shell default is now
// the small C10 sample, so pin the stress fixture here to preserve this spec's seed/replay namespace.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "5000";

// A known-good log in the deterministic SCENE_N=5000 id space. The header must match
// `capscene::fingerprint(5000)` byte-for-byte or replay discards it; 1_1129 is a seeded HealthBar and
// 1_aac / 1_6ec are compatible Health providers (all verified live: `restored N (0 skipped)`).
const SEED_LOG = [
  "#mtk mtkscene1 seed=0x4d4554524f434131 n=5000",
  '{"kind":"bind","from":"1_1129","to":"1_aac"}',
  '{"kind":"bind","from":"1_1129","to":"1_6ec"}',
  "",
].join("\n");

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",

  specs: ["./specs-reload/**/*.e2e.js"],
  maxInstances: 1,

  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],

  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 120000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  // Seed the log with prior-session binds, so launch is a true "reopen after binding".
  onPrepare: () => {
    writeFileSync(sceneLog, SEED_LOG);
  },

  beforeSession: () =>
    new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000);
    }),

  afterSession: () => {
    tauriDriver?.kill();
  },
};
