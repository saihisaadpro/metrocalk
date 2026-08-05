// Viewport render-quality matrix. Exercises deliberately different assets through the real packaged wgpu
// surface, then records matched cinematic/CAD views. `MTK_CAPTURE_BASELINE=1` captures the current default
// once per asset so shader/frame-graph changes can be compared against the same camera and content.

import { browser } from "@wdio/globals";
import { execFileSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const FIX = process.env.MTK_FIX_DIR;
const BASELINE = process.env.MTK_CAPTURE_BASELINE === "1";
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);
const kick = (cmd, args = {}) =>
  browser.execute((c, a) => {
    window.__TAURI__.core.invoke(c, a).catch(() => {});
    return true;
  }, cmd, args);
const entityCount = () => browser.execute(() => {
  const match = document.getElementById("count")?.textContent?.match(/\d+/);
  return match ? Number(match[0]) : 0;
});

const shot = async (label) => {
  await browser.pause(650);
  const out = path.join(SHOT_DIR, `${label}.png`);
  execFileSync("powershell", [
    "-ExecutionPolicy", "Bypass", "-File", SHOT_PS1,
    "-Out", out, "-ProcName", PROC,
  ], { stdio: "ignore" });
  console.log("  shot", out);
};

const fixtureAssets = [
  { key: "hard_surface", file: "cube.glb", material: "chrome" },
  { key: "curvature", file: "dense_sphere.glb", material: "brushed" },
  { key: "normal_map", file: "normal_mapped_quad.glb" },
  { key: "multi_material", file: "multi_material_quad.glb" },
  { key: "micro_relief", file: "ripple_quad_wide.glb" },
];
const cadAssets = [
  process.env.MTK_CAD_STEP && { key: "industrial_step", source: process.env.MTK_CAD_STEP, assembly: true },
  process.env.MTK_CAD_3DXML && { key: "industrial_3dxml", source: process.env.MTK_CAD_3DXML, assembly: true },
].filter(Boolean);
const assets = process.env.MTK_ONLY_CAD === "1" ? cadAssets : [...fixtureAssets, ...cadAssets];

describe("viewport render-quality matrix", () => {
  before(async () => {
    await browser.waitUntil(async () => {
      try {
        return Array.isArray(await invoke("camera_debug"));
      } catch {
        return false;
      }
    }, {
      timeout: 30000,
      timeoutMsg: "editor never connected",
    });
    await browser.execute(() => {
      localStorage.setItem("mtk.onboarded.v1", "1");
      const button = [...document.querySelectorAll("button")]
        .find((item) => /skip/i.test(item.textContent || ""));
      button?.click();
    });
    const demo = await invoke("demo_character").catch(() => null);
    if (Array.isArray(demo)) {
      const [root, parts] = demo;
      for (const id of parts || []) await invoke("remove_entity", { id }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    await browser.pause(900);
  });

  for (const asset of assets) {
    it(`renders ${asset.key} in the quality profiles`, async () => {
      const source = asset.source || path.join(FIX, asset.file);
      let id = null;
      if (asset.assembly) {
        const before = await entityCount();
        await kick("import_asset", { path: source });
        await browser.waitUntil(async () => (await entityCount()) >= before + 300, {
          timeout: 240000,
          interval: 2000,
          timeoutMsg: `industrial assembly never populated: ${source}`,
        });
      } else {
        id = await invoke("import_asset", { path: source });
        if (typeof id !== "string") throw new Error(`failed to import ${source}`);
        if (asset.material) await invoke("ai_edit", { id, material: asset.material });
      }
      await invoke("view_preset", { preset: "persp" });
      if (asset.assembly) await invoke("frame_all");
      else await invoke("focus_entity", { id });

      if (BASELINE) {
        await shot(`${asset.key}_baseline`);
      } else {
        const cinematic = await invoke("set_render_profile", { profile: "cinematic" });
        if (cinematic !== "cinematic") throw new Error(`cinematic profile rejected: ${cinematic}`);
        await shot(`${asset.key}_cinematic`);
        const cad = await invoke("set_render_profile", { profile: "cad" });
        if (cad !== "cad") throw new Error(`CAD profile rejected: ${cad}`);
        await shot(`${asset.key}_cad`);

        // A whole factory line proves scale/framing, but it cannot prove close-range surface stability.
        // Focus a real visible CAD leaf and capture the same part in both profiles as a second scale gate.
        if (asset.assembly) {
          const partIds = await browser.execute(() => [...document.querySelectorAll('[data-testid="hrow"]')]
            .filter((row) => row.getAttribute("data-kind") !== "group")
            .map((row) => row.getAttribute("data-id"))
            .filter(Boolean));
          if (partIds.length === 0) throw new Error("industrial assembly exposed no focusable CAD leaf");
          const hero = partIds[Math.floor(partIds.length / 2)];
          await invoke("gizmo_select", { id: hero });
          await invoke("focus_entity", { id: hero });
          await invoke("set_render_profile", { profile: "cinematic" });
          await shot(`${asset.key}_hero_cinematic`);
          await invoke("set_render_profile", { profile: "cad" });
          await shot(`${asset.key}_hero_cad`);
          await invoke("unfocus");
          await invoke("frame_all");
        }
      }

      if (!asset.assembly) {
        await invoke("unfocus");
        await invoke("remove_entity", { id });
      }
    });
  }
});
