// AIMING A SHOT BY POINTING AT THE THING — on the PACKAGED .exe, driven through the UI.
//
// The engine could name what is under the cursor without touching the selection (`viewport_peek`),
// answer with the chain that object hangs from, and re-aim a shot as one undoable edit. None of the
// three had a caller. This spec performs the gesture those three make possible — point at an object
// on the stage, take it or the assembly it belongs to — and proves the selection did not move, which
// is the whole reason the mode reads through a peek rather than a pick.
//
// Empty scene (MTK_SCENE_N=0) so only what the test builds appears in the chain the badge reads.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.subjectaim.conf.js

import { spawn, spawnSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

// Clean slate beside the exe: a stale replay log or saved project would replay yesterday's cutscene
// into today's assertions.
const exeDir = path.dirname(application);
for (const f of ["metrocalk-scene.jsonl", "metrocalk-wallet.json", "metrocalk-recents.json"]) {
  const p = path.join(exeDir, f);
  if (existsSync(p)) rmSync(p);
}

const shots = path.resolve(dir, ".shots-subjectaim");
if (existsSync(shots)) rmSync(shots, { recursive: true });

// A leftover editor makes every window measurement unanswerable, and a crashed earlier run leaves one.
const reapStrays = () =>
  spawnSync("taskkill", ["/F", "/IM", "metrocalk-editor-shell.exe"], { stdio: "ignore" });

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-subjectaim/subject-aim.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: reapStrays,

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
    reapStrays();
  },
};
