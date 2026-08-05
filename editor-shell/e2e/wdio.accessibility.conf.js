// Dedicated packaged-app accessibility gate. Keeping this separate from the full workbench journey lets a
// release run the WCAG evidence suite without paying for unrelated asset and CAD scenarios.
process.env.MTK_SCENE_N = process.env.MTK_SCENE_N || "40";

const { config: base } = await import("./wdio.conf.js");

export const config = {
  ...base,
  specs: ["./specs-workbench/accessibility.e2e.js"],
  mochaOpts: { ui: "bdd", timeout: 600000 },
};
