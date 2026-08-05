// LIVE M11.1 LOD — a large sphere captured near (frame_all → LOD 0, smooth) and zoomed out (LOD engages,
// coarser). Run under MTK_LOD=on vs =off to compare the far silhouette. Both runs must render the sphere at
// every distance (the LOD swap must not break the draw); the coarsening is subtle by design.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const FIX = process.env.MTK_FIX_DIR;
const L = process.env.MTK_LOD || "on";
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

let opN = 0;
const setField = async (id, field, value) => {
  opN += 1;
  const tx = {
    clientOpId: `lod-op-${opN}`,
    label: `set Transform.${field}`,
    patches: [],
    intent: { kind: "setField", id, component: "Transform", field, value },
  };
  await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
};

const shot = async (label) => {
  await browser.pause(600);
  const out = path.join(SHOT_DIR, `${label}_lod${L}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -Out "${out}" -ProcName "${PROC}"`, {
      stdio: "ignore",
    });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

const countEntities = async () => {
  try {
    const el = await browser.$("#count");
    const m = (await el.getText()).match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : NaN;
  } catch {
    return NaN;
  }
};

describe("LIVE M11.1 — LOD distance selection", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected",
    });
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
      const b = [...document.querySelectorAll("button")].find((x) => /skip/i.test(x.textContent || ""));
      if (b) b.click();
    });
    const dc = await invoke("demo_character").catch(() => null);
    if (Array.isArray(dc)) {
      const [root, parts] = dc;
      for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    await browser.pause(1500);
  });

  it(`renders a sphere near (LOD 0) and far (coarser) without breaking (MTK_LOD=${L})`, async () => {
    const id = await invoke("import_asset", { path: path.join(FIX, "dense_sphere.glb") });
    if (typeof id === "string") await setField(id, "scale", 6); // big, so faceting is visible even when far
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");
    await shot("00_near"); // distance ~7 → LOD 0 (smooth)
    await invoke("zoom", { delta: 18 }); // → distance ~25 → LOD engages
    await shot("01_far");
    await invoke("zoom", { delta: 22 }); // → distance ~47 → coarsest LOD
    await shot("02_farther");
    console.log("  done, count:", await countEntities());
  });
});
