// LIVE proof that the stage can select more than one object, against the packaged `.exe`.
//
// Everything here is driven through the UI — real pointer events on `#viewport` — and the only
// commands it invokes directly are READS (`selection_ids`, `entity_details`). A read cannot fake a
// capability, which is the whole point: a spec that invokes `select_entities` to prove selection
// works proves only that the command exists.
//
// `MTK_SCENE_N` seeds the perf fixture, which is what gives the marquee a crowd to take a subset of.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.selecttest.conf.js

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
// `MTK_APP` because the app crate's target dir is not always beside its source: a worktree that
// shares the main checkout's `CARGO_TARGET_DIR` (which is how it gets built at all on a box with 11 GB
// free and a 34 GB target tree) puts the binary somewhere this path cannot reach.
const application =
  process.env.MTK_APP || path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
const relDir = path.dirname(application);
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "24";

let tauriDriver;

export const config = {
  runner: "local",
  hostname: "127.0.0.1",
  port: 4444,
  path: "/",
  automationProtocol: "webdriver",
  specs: ["./specs-selecttest/**/*.e2e.js"],
  maxInstances: 1,
  capabilities: [{ maxInstances: 1, "tauri:options": { application } }],
  logLevel: "error",
  framework: "mocha",
  reporters: ["spec"],
  mochaOpts: { ui: "bdd", timeout: 120000 },
  connectionRetryTimeout: 120000,
  connectionRetryCount: 3,

  // FRESH SLATE, and this run is the reason the rule exists. Without it the first pass deleted 27
  // objects and undid them, the replay log kept the deletions, and the SECOND pass opened a scene with
  // nothing left to select — a test failing on the state its own predecessor left behind
  // (`<test_and_ci_discipline>` 4). It also found the real defect underneath, which is that the undo
  // and the log disagreed about how many transactions a batch delete was.
  onPrepare: () => {
    for (const f of ["metrocalk-recents.json", "metrocalk-scene.jsonl", "metrocalk-wallet.json"]) {
      try {
        rmSync(path.join(relDir, f), { force: true });
      } catch {
        /* nothing to clean */
      }
    }
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
