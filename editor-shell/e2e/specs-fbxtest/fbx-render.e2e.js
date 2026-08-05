// LIVE FBX render campaign — import each real .fbx into the .exe ISOLATED (demo character removed, onboarding
// dismissed), drive a battery of Transform edits, and capture an OS-level screenshot of the composited
// (wgpu-under-WebView2) window after each state. Reads the transform back after every edit so rendering can
// be checked against the EXACT authored values (precision). Writes results.json for the assessment pass.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";

const FBX_DIR = process.env.MTK_FBX_DIR;
const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const PROC = "metrocalk-editor-shell";

mkdirSync(SHOT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

let opN = 0;
const setField = async (id, field, value) => {
  opN += 1;
  const tx = {
    clientOpId: `fbx-op-${opN}`,
    label: `set Transform.${field}`,
    patches: [],
    intent: { kind: "setField", id, component: "Transform", field, value },
  };
  await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
};

const results = [];
let shotN = 0;
const shot = async (fbx, label, meta) => {
  await browser.pause(450); // let the rebuild + GPU upload land
  const t = await invoke("read_transform", { id: meta.id }).catch(() => null);
  const cam = await invoke("camera_debug").catch(() => null);
  const file = `${String(shotN).padStart(2, "0")}_${fbx}_${label}.png`;
  shotN += 1;
  const out = path.join(SHOT_DIR, file);
  try {
    // PrintWindow mode targets the editor HWND (captures the wgpu composite, can't grab other windows).
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" "${PROC}" "${out}" printwin`, { stdio: "ignore" });
  } catch (e) {
    console.error("shot failed", file, String(e));
  }
  results.push({ fbx, label, file, transform: t, camera: cam, ...meta });
  console.log(`  shot ${file}  transform=${JSON.stringify(t)}`);
};

const FILES = (process.env.MTK_FBX_FILES || "spider.fbx,jeep.fbx,samba_dancing.fbx").split(",");

const countEntities = async () => {
  try {
    const el = await browser.$("#count");
    const m = (await el.getText()).match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : NaN;
  } catch {
    return NaN;
  }
};

describe("LIVE FBX render campaign (M11.1 visual verification)", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    // Dismiss the first-run onboarding overlay (it blocks the lower viewport).
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
      const b = [...document.querySelectorAll("button")].find((x) => /skip/i.test(x.textContent || ""));
      if (b) b.click();
    });
    // Clear the demo character so each FBX renders alone on the grid.
    const dc = await invoke("demo_character").catch(() => null);
    if (Array.isArray(dc)) {
      const [root, parts] = dc;
      for (const pid of parts || []) await invoke("remove_entity", { id: pid }).catch(() => {});
      if (root) await invoke("remove_entity", { id: root }).catch(() => {});
    }
    await browser.pause(300);
  });

  for (const fbx of FILES) {
    it(`imports + edits ${fbx} (clean, 13 captured states)`, async () => {
      const p = path.join(FBX_DIR, fbx);
      const before = await countEntities();
      const id = await invoke("import_asset", { path: p });
      if (typeof id !== "string") throw new Error(`import_asset(${fbx}) returned ${JSON.stringify(id)}`);
      await browser.waitUntil(async () => (await countEntities()) > before, {
        timeout: 15000,
        timeoutMsg: `${fbx} did not place into the scene`,
      });
      await invoke("frame_all");
      const name = fbx.replace(/\.fbx$/i, "");

      const dist = async () => {
        const c = await invoke("camera_debug").catch(() => null);
        return Array.isArray(c) && c[2] ? c[2] : 12;
      };
      // The imported mesh is shown at an asset-NORMALIZED scale; the `scale` field overrides it with an
      // ABSOLUTE world-scale. Measure the normalized base so scale edits bracket it (mesh-independent):
      // framed distance ∝ shown-radius, so D0 (normalized) / D1 (scale=1.0) ≈ the normalized scale.
      const d0 = await dist();
      await setField(id, "scale", 1.0);
      await invoke("frame_all");
      const d1 = await dist();
      const base = Math.max(0.001, +(d0 / Math.max(0.001, d1)).toFixed(4)); // ≈ the asset-normalized scale
      await setField(id, "scale", base);
      await invoke("frame_all");
      const baseDist = await dist();
      const s = +(baseDist * 0.18).toFixed(3); // visible translate step at the baseline framing
      const t0 = await invoke("read_transform", { id });
      const baseY = Array.isArray(t0) ? t0[1] : 1;

      await shot(name, "00_import", { id, action: "import + frame_all", normalizedScale: base, step: s });

      const STATES = [
        // Translations at normalized size, camera fixed → the mesh visibly shifts against the grid.
        { label: "01_translate_xpos", e: async () => setField(id, "x", s) },
        { label: "02_translate_xneg", e: async () => setField(id, "x", -s) },
        { label: "03_translate_yup", e: async () => { await setField(id, "x", 0); await setField(id, "y", baseY + s); } },
        { label: "04_recenter_reframe", e: async () => { await setField(id, "y", baseY); await invoke("frame_all"); } },
        // Rotations at normalized size, camera fixed → orientation clearly visible.
        { label: "05_rotate_yaw90", e: async () => { await setField(id, "qw", 0.7071); await setField(id, "qy", 0.7071); } },
        { label: "06_rotate_yaw180", e: async () => { await setField(id, "qw", 0.0); await setField(id, "qy", 1.0); } },
        { label: "07_rotate_pitch90", e: async () => { await setField(id, "qy", 0.0); await setField(id, "qw", 0.7071); await setField(id, "qx", 0.7071); } },
        { label: "08_rotate_identity", e: async () => { await setField(id, "qx", 0.0); await setField(id, "qw", 1.0); await invoke("frame_all"); } },
        // Scale bracketed around the normalized base, camera fixed → smaller then bigger, mesh-independent.
        { label: "09_scale_half", e: async () => setField(id, "scale", +(base * 0.5).toFixed(4)) },
        { label: "10_scale_double", e: async () => setField(id, "scale", +(base * 2.0).toFixed(4)) },
        { label: "11_scale_reset_reframe", e: async () => { await setField(id, "scale", base); await invoke("frame_all"); } },
        // Camera presets → the same mesh from other angles.
        { label: "12_view_side", e: async () => invoke("view_preset", { preset: "side" }) },
        { label: "13_view_top", e: async () => { await invoke("view_preset", { preset: "persp" }); await invoke("frame_all"); } },
        // M11.2 PBR material assign — a metered AI-edit gives this entity a CHROME finish (metallic+smooth).
        { label: "14_material_chrome", e: async () => invoke("ai_edit", { id, material: "chrome" }) },
        { label: "15_material_gold", e: async () => invoke("ai_edit", { id, material: "gold" }) },
      ];

      for (const st of STATES) {
        await st.e();
        await shot(name, st.label, { id, action: st.label });
      }

      await invoke("remove_entity", { id }).catch(() => {});
      await browser.pause(300);
    });
  }

  after(async () => {
    writeFileSync(path.join(SHOT_DIR, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\nWROTE ${results.length} states → ${path.join(SHOT_DIR, "results.json")}`);
  });
});
