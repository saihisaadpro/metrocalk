import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
// `MTK_APP` because a worktree cannot afford its own 35 GB `target/`: the release build is pointed at
// the MAIN checkout's target dir, so the binary lands outside this tree and the relative path below
// cannot reach it. Same reason, same variable, as the other worktree-driven confs.
const application = process.env.MTK_APP || path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

process.env.MTK_SCENE_N = "0";
let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-importsearch/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  // A 275 MB STEP assembly is minutes of import before the first assertion.
  mochaOpts: { ui: "bdd", timeout: 1200000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,
  beforeSession: () =>
    new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (error) => console.error("tauri-driver failed:", error));
      setTimeout(resolve, 2500);
    }),
  afterSession: () => {
    try {
      tauriDriver?.kill();
    } catch {
      // Already stopped.
    }
  },
};
