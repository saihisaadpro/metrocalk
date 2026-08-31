// THE STAGE ANSWERS WHAT THE CURSOR IS OVER — on the PACKAGED .exe, measured in real pixels.
//
// The badge has named the object under the cursor, and the whole assembly chain above it, since
// ADR-171. The picture never changed — so on an imported line, `Assembly Hall · 378 parts` was a claim
// about which of 15,711 identical grey parts a click was going to be about, and the only thing backing
// it was the label. This spec asserts the claim in PIXELS: an OS capture of the real composite before
// and after, diffed inside the viewport rectangle, with a negative control first so the numbers are
// not the instrument's own noise.
//
// Empty scene (MTK_SCENE_N=0) so only what the test builds is under the cursor.
//
// Run: node "node_modules/@wdio/cli/bin/wdio.js" run wdio.stagehover.conf.js

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

const shots = path.resolve(dir, ".shots-stagehover");
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
  specs: ["./specs-stagehover/stage-hover.e2e.js"],
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
