// Production factory direction pass. The base native-drop config remains the owner of exact clean-state
// archival, executable/fixture hashing, driver startup, and post-run evidence preservation.
//
// PowerShell run example (the fixture is intentionally mandatory here):
//   $env:MTK_NATIVE_DROP_FIXTURE = 'X:\Work\Metrocalk\Games Projects\Unreal\Skid Weld Line A.1\Skid Weld Line A.1_(1).stp'
//   node "node_modules/@wdio/cli/bin/wdio.js" run wdio.factory-cinematic.conf.js

import { browser } from "@wdio/globals";

import { config as nativeCadDropConfig } from "./wdio.native-cad-drop.conf.js";
import { reapHarnessProcesses } from "./lib/reap.js";

if (!process.env.MTK_NATIVE_DROP_FIXTURE) {
  throw new Error(
    "MTK_NATIVE_DROP_FIXTURE must name the production Skid Weld Line STEP file; "
      + "the analytic fixture remains the default only for wdio.native-cad-drop.conf.js.",
  );
}

// The production film is intentionally rendered at the engine's highest directional-shadow tier.
// This is inherited by tauri-driver and the packaged application it launches.
process.env.MTK_SHADOW_QUALITY = "high";

export const config = {
  ...nativeCadDropConfig,
  specs: ["./specs-factory-cinematic/factory-cinematic.e2e.js"],
  logLevel: "warn",
  mochaOpts: { ui: "bdd", timeout: 3_600_000 },
  connectionRetryTimeout: 600_000,
  connectionRetryCount: 2,

  // WebDriver caps an `execute/sync` at 30 SECONDS by default, and every engine command in this spec
  // goes through one. On the production factory the heavy commands genuinely exceed that: the import is
  // already fire-and-forget because of it, and entering Play does a document snapshot, a physics
  // recording, a rules recording and a role-roster walk over 17,793 entities before it answers.
  //
  // A cap that fires does not cancel the command -- the engine finishes the work regardless -- it only
  // desynchronises the session, so the failure reads as "script timeout" no matter what the app did.
  // Raising it to five minutes lets a slow command be observed AS a slow command; the engine's own
  // `metrocalk-diagnostics.log` reports what it cost.
  before: async () => {
    await browser.setTimeout({ script: 300_000 });
  },
};
