// ADR-197 — "there is nowhere to film this from", proven on the packaged `.exe` by enclosing a
// subject and reading what the engine says about it.
//
// The claim cannot be passed by relabelling anything. `plan_shot` walks fifty-four placements against
// the real scene BVH, and the spec's own negative controls are what make its positive assertion mean
// something: the same shot in an open world says nothing, and removing the one obstruction makes the
// sentence go away. Both run in this file, against the same subject, in the same session.
//
// Local-only, for the standing reason the rest of this directory is: a display, a WebView2-matched
// `msedgedriver`, and a GPU the wgpu surface can be created on.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.nowhere.conf.js

import { spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application =
  process.env.MTK_EXE || path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const exeDir = path.dirname(application);
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// An empty scene. Every obstruction claim below is a statement about a world whose entire contents
// this spec spawned, which is the only way "there is nowhere to film this from" can be attributed.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-nowhere/nowhere-to-film.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 300000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    // A stale replay log would open yesterday's cutscene into today's assertions. (The spec deletes
    // its own `keep-framed.mtk` before writing it, beside its evidence rather than beside the .exe —
    // `CARGO_TARGET_DIR` can put the binary somewhere this directory does not exist.)
    for (const f of [
      "metrocalk-scene.jsonl",
      "metrocalk-wallet.json",
      "metrocalk-recents.json",
    ]) {
      const p = path.join(exeDir, f);
      if (existsSync(p)) rmSync(p, { force: true });
    }
    return new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2500);
    });
  },

  onComplete: () => {
    try {
      tauriDriver?.kill();
    } catch {
      /* already gone */
    }
  },
};
