// Verification config for the C10 first-run fix: launches the DEBUG .exe with NO MTK_SCENE_N override, so
// it exercises the shell's NEW DEFAULT seed (the small sample) — proving a fresh user no longer faces the
// 5,000-entity stress wall. Runs the smoke spec (reads #count + captures).
import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
// MTK_EXE overrides the binary (e.g. point at the release build); defaults to the debug build.
const application = process.env.MTK_EXE || path.resolve(dir, "../src-tauri/target/debug/metrocalk-editor-shell.exe");
const exeDir = path.dirname(application);
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// CRITICAL: ensure NO scene-size override leaks in — we want the shell's real default first-run.
delete process.env.MTK_SCENE_N;

function cleanSlate() {
  for (const f of ["metrocalk-scene.jsonl", "metrocalk-wallet.json", "metrocalk-recents.json"]) {
    try { rmSync(path.join(exeDir, f), { force: true }); } catch { /* fine */ }
  }
  try { rmSync(path.join(exeDir, "metrocalk-assets"), { recursive: true, force: true }); } catch { /* fine */ }
}

let tauriDriver;
export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-smoke/smoke.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 120000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  onPrepare: () => cleanSlate(),
  beforeSession: () =>
    new Promise((resolve) => {
      cleanSlate();
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], { stdio: [null, process.stdout, process.stderr] });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000);
    }),
  afterSession: () => { tauriDriver?.kill(); },
};
