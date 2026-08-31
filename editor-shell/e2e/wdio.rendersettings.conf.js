// ADR-190 — the render settings survive a RESTART, proven by restarting.
//
// Two specs run SEQUENTIALLY against the same persisted replay log, each in its own launch of the
// packaged `.exe`:
//   r1 authors the five answers through the Render dialog's own controls;
//   r2 RELAUNCHES and asserts the dialog opens on them.
//
// `onPrepare` cleans once, for a baseline nothing earlier can have written. `beforeSession`
// deliberately does NOT clean — the log surviving from r1's process into r2's IS the claim. Same shape
// as `wdio.persist.conf.js`, which proved the same thing about a deactivated entity.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.rendersettings.conf.js

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

// Empty scene, so the only cutscene in the document is the one r1 authors.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-rendersettings/r1-author.e2e.js", "./specs-rendersettings/r2-after-restart.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 300000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  // ONCE, and only here. A stale log would replay yesterday's cutscene into today's assertions, and a
  // stale handoff file would let r2 pass against an object r1 never created this run.
  onPrepare: () => {
    for (const f of ["metrocalk-scene.jsonl", "metrocalk-wallet.json", "metrocalk-recents.json"]) {
      const p = path.join(exeDir, f);
      if (existsSync(p)) rmSync(p, { force: true });
    }
    const handoff = path.resolve(dir, ".shots-rendersettings");
    if (existsSync(handoff)) rmSync(handoff, { recursive: true, force: true });
    return new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2500);
    });
  },

  // ONE DRIVER FOR BOTH SESSIONS, spawned here rather than per-session.
  //
  // The sibling configs spawn and kill a `tauri-driver` around every session, which is fine when a
  // config runs one spec. Two specs in a row is a second `spawn` onto port 4444 two and a half
  // seconds after the first was killed, and the second session came back
  // `WebDriverError: UND_ERR_SOCKET` — the port was still held. The driver itself is happy to serve
  // sequential sessions at `maxInstances: 1`, and each session still launches its own `.exe`, which
  // is the restart this config exists to perform.
  onComplete: () => {
    try {
      tauriDriver?.kill();
    } catch {
      /* already gone */
    }
  },
};
