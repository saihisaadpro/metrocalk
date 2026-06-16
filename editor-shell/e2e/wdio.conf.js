// WebdriverIO config — drives the real Metrocalk editor .exe through tauri-driver (WebDriver over the
// WebView2). tauri-driver is started before the session and pointed at the bundled msedgedriver
// (matched to the WebView2 runtime, 149.x). The spec interacts with the editor's DOM — including the
// transparent viewport <div>, whose clicks fire the native pick — and asserts the resulting DOM.

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
// Built release binary (rebuild with: cargo build --release --manifest-path ../src-tauri/Cargo.toml).
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
// The shell writes its persistence log next to the exe; delete it so each E2E run starts on a clean,
// freshly-seeded scene (persistence-across-launch is covered by the headless tests).
const sceneLog = path.resolve(dir, "../src-tauri/target/release/metrocalk-scene.jsonl");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",

  specs: ["./specs/**/*.e2e.js"],
  maxInstances: 1,

  capabilities: [
    {
      maxInstances: 1,
      "tauri:options": { application },
    },
  ],

  logLevel: "warn",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 120000 },

  // tauri-driver is the WebDriver intermediary; it spawns the app + forwards to the native driver.
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  onPrepare: () => {
    try {
      rmSync(sceneLog, { force: true });
    } catch {
      /* no log yet — fine */
    }
  },

  beforeSession: () =>
    new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000); // give tauri-driver + msedgedriver a moment to bind their ports
    }),

  afterSession: () => {
    tauriDriver?.kill();
  },
};
