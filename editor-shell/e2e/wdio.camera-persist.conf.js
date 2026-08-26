// THE SECOND LAUNCH. Deliberately the same as `wdio.camtest.conf.js` MINUS its `onPrepare`, which deletes
// `metrocalk-scene.jsonl` for a fresh slate. Here the point is the opposite: the session log left behind by
// the previous run is the thing under test, so this conf must not touch it.
//
// Run it AFTER `wdio.camtest.conf.js --spec ./specs-camtest/camera-ui.e2e.js`, against the same binary:
//
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.camtest.conf.js --spec ./specs-camtest/camera-ui.e2e.js
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.camera-persist.conf.js
//
// Together those two invocations are a real close-and-reopen of the packaged app: the first authors a
// camera through the UI, the process exits, and the second starts a new one and asks whether the camera
// came back — including its aim, which is the field whose absence made a reopened project show every
// camera from a different distance at the same subject.

import { spawn } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const application = path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// The SAME seed as the first launch. The replay log is applied on top of a deterministic seed, so a
// different `MTK_SCENE_N` would allocate different ids and the record would replay onto nothing.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "40";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-camera-persist/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 900000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  // NO onPrepare. That is the whole configuration.

  beforeSession: () =>
    new Promise((resolve) => {
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2500);
    }),

  afterSession: () => {
    tauriDriver?.kill();
  },
};
