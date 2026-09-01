// ADR-201 — "walk the move", proven on the packaged `.exe`.
//
// A shot is a PATH. `plan_shot` has always judged five instants along it and reported one verdict, and
// until this ADR the other four instants were reachable only through a preview, which HOLDS the
// viewport — so the frames an author most needs to judge were the ones they could look at and not
// stand in. This spec walks that path through the command the slider sends, and reads the RENDERER'S
// OWN CAMERA back after every step (`camera_probe`), which is the only thing that can distinguish a
// camera that moved from a reply that says it did.
//
// Local-only, for the standing reason the rest of this directory is: a display, a WebView2-matched
// `msedgedriver`, and a GPU the wgpu surface can be created on.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.walk.conf.js

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

// An empty scene: every pose this spec measures is a statement about a world whose entire contents it
// spawned itself.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-walk/walk-the-move.e2e.js"],
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
    // its own `.mtk` before writing it, beside its evidence rather than beside the .exe —
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
