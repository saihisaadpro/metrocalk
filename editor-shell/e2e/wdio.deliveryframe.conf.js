// The frame the camera is composed for — on the PACKAGED .exe, with OS-composited pixel captures.
//
// Two claims, one rectangle: opening a dock re-composes the picture for the stage that is left, and a
// cutscene delivered in scope is composed for scope and shows its bars. Empty scene (MTK_SCENE_N=0) so
// only what the test creates appears.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.deliveryframe.conf.js

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
// into today's assertions — and this spec asserts a delivery frame survives a reopen.
const exeDir = path.dirname(application);
for (const f of [
  "metrocalk-scene.jsonl",
  "metrocalk-wallet.json",
  "metrocalk-recents.json",
  "delivery-frame.mtk",
]) {
  const p = path.join(exeDir, f);
  if (existsSync(p)) rmSync(p);
}

// One run's evidence = one folder's worth.
const shots = path.resolve(dir, ".shots-deliveryframe");
if (existsSync(shots)) rmSync(shots, { recursive: true });

// A LEFTOVER EDITOR MAKES EVERY PIXEL ASSERTION UNANSWERABLE. `window-client-rect.ps1` refuses when
// two `metrocalk-editor-shell` processes exist — correctly, since it cannot know which window a
// fraction is a fraction of — so a crashed earlier run turns the capture half of this spec into an
// error about process counts. Reaped here rather than tolerated there.
const reapStrays = () =>
  spawnSync("taskkill", ["/F", "/IM", "metrocalk-editor-shell.exe"], { stdio: "ignore" });

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-deliveryframe/delivery-frame.e2e.js"],
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
