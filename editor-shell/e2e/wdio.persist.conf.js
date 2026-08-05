// R-NEXT-2 cross-reload verification. Runs two specs SEQUENTIALLY against the same persisted log:
//   p1 deactivates an entity → p2 RELAUNCHES (log NOT cleaned between sessions) and asserts it's still hidden.
// onPrepare cleans once for a fresh baseline; beforeSession deliberately does NOT clean, so the log survives.
import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = process.env.MTK_EXE || path.resolve(dir, "../src-tauri/target/debug/metrocalk-editor-shell.exe");
const exeDir = path.dirname(application);
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

delete process.env.MTK_SCENE_N; // exercise the real default first-run (small scene), deterministic ids

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
  // Order matters: deactivate, then relaunch + verify. maxInstances 1 → sequential, each its own launch.
  specs: ["./specs-persist/p1-deactivate.e2e.js", "./specs-persist/p2-verify.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 120000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  onPrepare: () => cleanSlate(), // fresh baseline ONCE
  beforeSession: () =>
    new Promise((resolve) => {
      // NB: NO cleanSlate here — the log must persist from p1's launch into p2's launch.
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], { stdio: [null, process.stdout, process.stderr] });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000);
    }),
  beforeTest: async () => {
    try { const skip = await browser.$("#onboardSkip"); if (await skip.isExisting()) await skip.click(); } catch { /* fine */ }
  },
  afterSession: () => { tauriDriver?.kill(); },
};
