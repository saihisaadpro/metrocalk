// WebdriverIO config — drives the real Metrocalk editor .exe through tauri-driver (WebDriver over the
// WebView2). tauri-driver is started before the session and pointed at the bundled msedgedriver
// (matched to the WebView2 runtime, 149.x). The spec interacts with the editor's DOM — including the
// transparent viewport <div>, whose clicks fire the native pick — and asserts the resulting DOM.

import { spawn } from "node:child_process";
import { rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
// Built binary. Release by default (the canonical gate target; rebuild with:
// cargo build --release --manifest-path ../src-tauri/Cargo.toml). MTK_EXE overrides the path so the
// journey can iterate against a faster debug build, then run the standing gate against release.
const application =
  process.env.MTK_EXE ||
  path.resolve(dir, "../src-tauri/target/release/metrocalk-editor-shell.exe");
// The shell writes its persistence log + wallet next to the exe; delete them so each E2E run starts on a
// clean, freshly-seeded scene + the full free token grant (persistence-across-launch is covered by the
// headless tests; the marketplace-buy test DEBITS the wallet, so it must not carry over between runs and
// eventually flake "insufficient balance"). Derived from the ACTUAL exe path so an MTK_EXE override (debug)
// cleans beside the debug binary, not the release one.
const exeDir = path.dirname(application);
const sceneLog = path.join(exeDir, "metrocalk-scene.jsonl");
const walletFile = path.join(exeDir, "metrocalk-wallet.json");
// The recents list (ADR-033 startup = open-last-else-seeded-sample). The first-session journey SAVES a
// `.mtk`, which pushes that path into recents — so without clearing it, every LATER spec boots the
// journey's sample instead of the freshly-seeded scene (and its bind/edit/requirer assertions fail on the
// wrong scene). Clear it so each run starts from the known-good seeded scene.
const recentsFile = path.join(exeDir, "metrocalk-recents.json");
// The M11.1 content-addressed asset-blob dir (persisted generated/imported bytes). A generate test writes
// a blob here; without clearing it, a LATER spec's boot would re-import that stale asset into its store
// (an orphan — harmless, but a clean slate must be deterministic).
const blobDir = path.join(exeDir, "metrocalk-assets");

// A clean slate beside the exe: a freshly-seeded scene, the full free token grant (the marketplace-buy
// test debits the wallet), and NO recents (so startup boots the seeded scene, not a journey-saved sample).
function cleanSlate() {
  for (const f of [sceneLog, walletFile, recentsFile]) {
    try {
      rmSync(f, { force: true });
    } catch {
      /* not present yet — fine */
    }
  }
  try {
    rmSync(blobDir, { recursive: true, force: true });
  } catch {
    /* not present yet — fine */
  }
}
const nativeDriver = path.resolve(dir, ".driver/msedgedriver.exe");
const tauriDriverBin = path.resolve(process.env.USERPROFILE, ".cargo/bin/tauri-driver.exe");

// The shell's default first-run is now the SMALL C10 sample (not the 5k stress wall). These functional
// specs were written against the at-scale seed, so pin the stress fixture here to keep them unchanged.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "5000";

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

  onPrepare: () => cleanSlate(),

  beforeSession: () =>
    new Promise((resolve) => {
      // Each spec FILE relaunches the exe, so re-clean before every session — otherwise the first-session
      // journey's saved `.mtk` (pushed into recents mid-run) makes a later spec boot the sample via
      // open-last instead of the seeded scene, and its scene/wallet mutations leak forward too.
      cleanSlate();
      tauriDriver = spawn(tauriDriverBin, ["--native-driver", nativeDriver], {
        stdio: [null, process.stdout, process.stderr],
      });
      tauriDriver.on("error", (e) => console.error("tauri-driver failed to start:", e));
      setTimeout(resolve, 2000); // give tauri-driver + msedgedriver a moment to bind their ports
    }),

  // Dismiss the first-run onboarding card before every test so it never overlaps an acceptance interaction
  // (the onboarding spec re-summons it explicitly inside its own test, after this hook has run). A no-op
  // when the card isn't present (already dismissed / not the React build).
  beforeTest: async () => {
    try {
      const skip = await browser.$("#onboardSkip");
      if (await skip.isExisting()) await skip.click();
    } catch {
      /* card absent — fine */
    }
  },

  afterSession: () => {
    tauriDriver?.kill();
  },
};
