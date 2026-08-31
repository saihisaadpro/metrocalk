// Presentation-environment look-dev lab.
//
// The factory film costs ~20 minutes an iteration, almost all of it re-importing 15,711 STEP parts that
// have not changed. This config opens the .mtk that a completed film run already saved, so a change to
// the presentation set can be SEEN in about a minute. It is a look-dev loop, not a gate: it proves
// nothing about the film and is not wired into CI. The film remains the acceptance run.
//
// PowerShell run example:
//   $env:MTK_HALL_LAB_PROJECT = 'X:\...\run12-honest-metric\skid-weld-line-24-track-30-shot-cinematic.mtk'
//   node "node_modules/@wdio/cli/bin/wdio.js" run wdio.hall-lab.conf.js

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { assertExeMatchesTree } from "./lib/build-freshness.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application =
  process.env.MTK_EXE || path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");

// Refuse to look at a picture produced by code that is not in the tree. This lab exists to judge a
// presentation change, so a stale binary would attribute the OLD room's picture to the NEW room's source.
assertExeMatchesTree(application, path.resolve(dir, "../.."));
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

if (!process.env.MTK_HALL_LAB_PROJECT) {
  throw new Error("MTK_HALL_LAB_PROJECT must name the .mtk a completed film run saved.");
}
if (!existsSync(process.env.MTK_HALL_LAB_PROJECT)) {
  throw new Error(`MTK_HALL_LAB_PROJECT does not exist: ${process.env.MTK_HALL_LAB_PROJECT}`);
}

// The film renders at the highest directional-shadow tier; the lab must judge the same picture.
process.env.MTK_SHADOW_QUALITY = process.env.MTK_SHADOW_QUALITY || "high";
// An empty seeded scene, so the only thing in the viewport is what the project carries.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

// One lab run = one folder. Named by the label so a before/after pair survives side by side.
const label = process.env.MTK_HALL_LAB_LABEL || "lab";
const shots = path.resolve(dir, "../evidence/presentation-hall", label);
if (existsSync(shots)) rmSync(shots, { recursive: true, force: true });
mkdirSync(shots, { recursive: true });
process.env.MTK_HALL_LAB_SHOTS = shots;

// Recents drive startup (ADR-033 open-last-else-seeded-sample). Left in place, the lab would boot the
// previous lab's project before the spec has said which one it wants, and the first capture would be of
// whatever ran last rather than of the project under test.
const exeDir = path.dirname(application);
for (const f of ["metrocalk-scene.jsonl", "metrocalk-recents.json"]) {
  const p = path.join(exeDir, f);
  if (existsSync(p)) rmSync(p, { force: true });
}

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-hall-lab/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 1_800_000 },
  connectionRetryTimeout: 300_000,
  connectionRetryCount: 2,

  // Opening a 14 MB project with 17,793 entities is well past WebDriver's 30 s script cap, and a cap
  // that fires does not cancel the command — it only desynchronises the session.
  before: async () => {
    const { browser } = await import("@wdio/globals");
    await browser.setTimeout({ script: 300_000 });
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
    try {
      tauriDriver?.kill();
    } catch {
      /* already gone */
    }
  },
};
