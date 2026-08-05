// Focused packaged-app acceptance for the redesigned editor workbench and direct viewport asset authoring.
// It reuses the proven tauri-driver lifecycle/clean-slate logic while keeping this high-value gate fast.
//
//   node bootstrap.mjs
//   npm run test:workbench

// The ESM dynamic import is intentional: set the lightweight deterministic fixture before wdio.conf.js
// applies its 5k stress default. Scale/performance remains covered by the full acceptance suite.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "40";

const { config: base } = await import("./wdio.conf.js");

export const config = {
  ...base,
  // Keep axe in the dedicated accessibility configuration. The practical workbench gate must remain runnable
  // when a pinned axe install is unavailable, while the certification preflight still fails closed.
  specs: [
    "./specs-workbench/workbench.e2e.js",
    "./specs-workbench/pipe-forge.e2e.js",
    "./specs-workbench/asset-lab.e2e.js",
  ],
  mochaOpts: { ui: "bdd", timeout: 600000 },
};
