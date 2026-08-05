// Imported-animation clip instancing, audition and graph playback against the actual Windows Tauri
// executable. This is deliberately a visible, single-instance run: the evidence helper captures the
// final OS-composited window because WebDriver screenshots cannot see the native wgpu surface below the
// transparent WebView2 layer.
//
// Canonical release run:
//   node bootstrap.mjs
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.animationtest.conf.js
//
// Faster current-build iteration:
//   $env:MTK_EXE = (Resolve-Path "..\src-tauri\target\debug\metrocalk-editor-shell.exe")
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.animationtest.conf.js

import { spawn } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application =
  (process.env.MTK_EXE ? path.resolve(process.env.MTK_EXE) : null) ||
  path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const exeDir = path.dirname(application);
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// The fixture is imported by the test, so an empty authored seed makes both the outliner and native
// viewport deterministic and keeps the capture focused on the one moving object.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "0";

let tauriDriver;

function cleanSlate() {
  for (const file of [
    "metrocalk-recents.json",
    "metrocalk-scene.jsonl",
    "metrocalk-wallet.json",
    "metrocalk-animation-asset-identities.json",
    "metrocalk-window.json",
  ]) {
    rmSync(path.join(exeDir, file), { force: true });
  }
  for (const directory of ["metrocalk-assets", "metrocalk-cad-meshes"]) {
    rmSync(path.join(exeDir, directory), { recursive: true, force: true });
  }
}

const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

async function stopTauriDriver() {
  const driver = tauriDriver;
  tauriDriver = undefined;
  if (!driver || driver.exitCode !== null) return;
  await new Promise((resolve) => {
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timeout);
      resolve();
    };
    const timeout = setTimeout(finish, 2000);
    driver.once("exit", finish);
    if (!driver.kill()) finish();
  });
}

async function cleanSlateAfterSession() {
  let lastError;
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      cleanSlate();
      return;
    } catch (error) {
      lastError = error;
      await delay(250);
    }
  }
  throw lastError;
}

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-animation/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 300000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    if (!existsSync(application)) {
      throw new Error(`Current Tauri executable is missing: ${application}`);
    }
    if (!existsSync(nativeDriver)) {
      throw new Error(`Matched WebView2 driver is missing: ${nativeDriver}. Run node bootstrap.mjs.`);
    }
    if (!existsSync(tauriDriverBin)) {
      throw new Error(`tauri-driver is missing: ${tauriDriverBin}. Run node bootstrap.mjs.`);
    }
    cleanSlate();
  },

  beforeSession: () =>
    new Promise((resolve) => {
      cleanSlate();
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (error) => {
        console.error("tauri-driver failed to start:", error);
      });
      setTimeout(resolve, 2500);
    }),

  afterSession: async () => {
    await stopTauriDriver();
    await cleanSlateAfterSession();
  },
};
