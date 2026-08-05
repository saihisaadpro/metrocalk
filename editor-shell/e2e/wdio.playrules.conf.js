// LIVE Rules-in-Play + truth-state debugger campaign (M12.5 / ADR-049): drive the RUNNING Rules tier on the
// packaged .exe over the invoke bridge (multi_edit · author_rule · author_state_machine · play · stop ·
// fire_rule_event · rule_debug · rule_scrub) AND through the real React DOM (the RuleDebugPanel: Play -> click
// the sword -> see the live truth-state -> fire kills -> scrub the decision history). Verifies Rules execute
// as a PROJECTION (the authored doc is never mutated), the live truth-state is visible ("3 of 4"), the
// decision history time-travels, and Stop restores. Clean, empty seed (MTK_SCENE_N=0).
//
// Distinct from wdio.ruletest.conf.js (M12.1 — Rule *authoring*); this is M12.5 — Rule *running + debugging*.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver):
//   set MTK_OUT_DIR=<out dir>  &  node "node_modules\@wdio\cli\bin\wdio.js" run wdio.playrules.conf.js

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
  specs: ["./specs-playrules/**/*.e2e.js"],
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
