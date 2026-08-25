// Import-free capture-integrity probe. Deliberately reuses the base config (same exe, same clean slate,
// same tauri-driver startup) and only swaps the spec, so it costs a shell launch rather than a 275 MB
// STEP import — which is the whole point: the question it answers is about the CAPTURE, not the scene.
//
// PowerShell:
//   node "node_modules/@wdio/cli/bin/wdio.js" run wdio.capture-integrity.conf.js

import { config as baseConfig } from "./wdio.conf.js";

export const config = {
  ...baseConfig,
  specs: ["./specs-capture-integrity/*.e2e.js"],
  logLevel: "warn",
  mochaOpts: { ui: "bdd", timeout: 300_000 },
};
