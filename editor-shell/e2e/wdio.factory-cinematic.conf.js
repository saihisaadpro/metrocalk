// Production factory direction pass. The base native-drop config remains the owner of exact clean-state
// archival, executable/fixture hashing, driver startup, and post-run evidence preservation.
//
// PowerShell run example (the fixture is intentionally mandatory here):
//   $env:MTK_NATIVE_DROP_FIXTURE = 'X:\Work\Metrocalk\Games Projects\Unreal\Skid Weld Line A.1\Skid Weld Line A.1_(1).stp'
//   node "node_modules/@wdio/cli/bin/wdio.js" run wdio.factory-cinematic.conf.js

import { config as nativeCadDropConfig } from "./wdio.native-cad-drop.conf.js";

if (!process.env.MTK_NATIVE_DROP_FIXTURE) {
  throw new Error(
    "MTK_NATIVE_DROP_FIXTURE must name the production Skid Weld Line STEP file; "
      + "the analytic fixture remains the default only for wdio.native-cad-drop.conf.js.",
  );
}

export const config = {
  ...nativeCadDropConfig,
  specs: ["./specs-factory-cinematic/factory-cinematic.e2e.js"],
  logLevel: "warn",
  mochaOpts: { ui: "bdd", timeout: 3_600_000 },
  connectionRetryTimeout: 600_000,
  connectionRetryCount: 2,
};
