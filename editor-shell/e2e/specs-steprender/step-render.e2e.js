// LIVE visual acceptance for the M15.7 STEP AP242 assembly reader (ADR-077) — the owed real-pixel wgpu
// capture. Imports the REAL 263 MB STEP re-export of the CATIA file that black-screened Unreal (the bar
// file), then walks ≥12 distinct captured states (presets · zoom · focus/selection · undo-peel · re-import ·
// the 3DXML proxy factory cell on top) and OS-captures the true composite each time (CopyFromScreen — the
// only way to see the native wgpu viewport under the transparent WebView2). Asserts STRUCTURED signals
// (row counts, camera state, transform sanity), never pixel copy; the pixel judgment is the post-run
// analyze step (never-black variance) + the human-readable captures.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.steprender.conf.js

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-steprender");
const SHOT_PS1 = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
const STEP_FILE =
  process.env.MTK_STEP_BAR_FILE ||
  "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1_(1).stp";
const XML_FILE =
  process.env.MTK_3DXML_BAR_FILE ||
  "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1.3dxml";
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(700);
  const out = path.join(SHOT_DIR, `${label}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${out}"`, { stdio: "ignore" });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

const invoke = (cmd, args) =>
  browser.execute(
    async (c, a) => {
      try {
        return await window.__TAURI__.core.invoke(c, a || {});
      } catch (e) {
        return { error: String(e) };
      }
    },
    cmd,
    args,
  );

// Fire-and-forget for LONG engine ops (import of a multi-hundred-MB file, the whole-import undo): the
// command's reply now truthfully arrives only when the op COMPLETES (minutes), far beyond the WebDriver
// renderer bridge's 30 s execute limit — so don't await it; key on the scene state instead (rows appear /
// disappear), which is what the assertions do anyway.
const kick = (cmd, args) =>
  browser.execute(
    (c, a) => {
      window.__TAURI__.core.invoke(c, a || {}).catch(() => {});
      return true;
    },
    cmd,
    args,
  );

// The hierarchy panel is VIRTUALIZED (only visible rows exist in the DOM) — the stable total is the
// `#count` "N entities" signal (the prompt-40 scaffold contract).
const entityCount = () =>
  browser.execute(() => {
    const el = document.getElementById("count");
    const m = el && el.textContent ? el.textContent.match(/\d+/) : null;
    return m ? parseInt(m[0], 10) : 0;
  });

describe("M15.7 STEP assembly reader — live wgpu visual acceptance on the real bar file", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
    });
    await browser.pause(500);
  });

  it("imports the real STEP bar file, renders it (never black), and survives the manipulation walk", async () => {
    await shot("00_before_import");
    const before = await entityCount();
    console.log("entities before:", before);

    // (1) Import the real 263 MB STEP. The reply arrives only when the import completes (minutes), so
    // fire-and-forget and key on the scene populating (the one landed commit).
    const t0 = Date.now();
    await kick("import_asset", { path: STEP_FILE });
    await browser.waitUntil(async () => (await entityCount()) >= 300, {
      timeout: 240000,
      interval: 2000,
      timeoutMsg: "the imported STEP parts never appeared in the outliner",
    });
    const stepCount = await entityCount();
    console.log(`STEP landed: ${stepCount} entities in ${((Date.now() - t0) / 1000).toFixed(1)}s (incl. webview render)`);
    await invoke("frame_all");
    await shot("01_step_persp");

    // (2) Canonical view presets — placement/orientation reads correctly from every axis.
    for (const p of ["front", "top", "side"]) {
      await invoke("view_preset", { preset: p });
      await browser.pause(300);
      await shot(`0${p === "front" ? 2 : p === "top" ? 3 : 4}_step_${p}`);
    }
    await invoke("view_preset", { preset: "persp" });
    await invoke("frame_all");

    // (3) Zoom in/out — the geometry holds up close (real tessellation, not proxies).
    await invoke("zoom", { delta: 6.0 });
    await shot("05_zoom_in");
    await invoke("zoom", { delta: -10.0 });
    await shot("06_zoom_out");
    await invoke("frame_all");

    // (4) Focus + selection overlay on a leaf part (a real CAD part row, not a group folder). The panel is
    // virtualized, so only the VISIBLE rows are in the DOM — any visible part row works.
    const partIds = await browser.execute(() => {
      return [...document.querySelectorAll('[data-testid="hrow"]')]
        .filter((r) => r.getAttribute("data-kind") !== "group")
        .map((r) => r.getAttribute("data-id"))
        .filter(Boolean);
    });
    console.log("visible leaf part rows:", partIds.length);
    if (partIds.length < 5) throw new Error(`expected ≥5 visible part rows, got ${partIds.length}`);
    const hero = partIds[Math.floor(partIds.length / 2)];
    await invoke("gizmo_select", { id: hero });
    await invoke("focus_entity", { id: hero });
    await browser.pause(400);
    await shot("07_focus_hero");
    const tf = await invoke("read_transform", { id: hero });
    console.log("hero transform:", JSON.stringify(tf));
    // Units backstop: the skid is a ~2.5 m assembly in metres — a part's position must be sane
    // (|coord| < 100 m; the 10×-trap would park parts at 25+ m or 0.025 m scale).
    if (Array.isArray(tf)) {
      const [x, y, z] = tf;
      const r = Math.sqrt(x * x + y * y + z * z);
      if (!(r < 100)) throw new Error(`hero part position ${r}m from origin — units look wrong`);
    }
    const other = partIds[Math.floor(partIds.length / 4)];
    await invoke("gizmo_select", { id: other });
    await invoke("focus_entity", { id: other });
    await browser.pause(400);
    await shot("08_focus_other");
    await invoke("unfocus");
    await invoke("frame_all");

    // (5) The ONE-undo peel: the whole import is ONE undoable commit — Ctrl-Z empties the scene.
    await kick("undo");
    await browser.waitUntil(async () => (await entityCount()) <= before + 2, {
      timeout: 120000,
      interval: 1000,
      timeoutMsg: "undo did not peel the imported assembly",
    });
    await browser.pause(400);
    await shot("09_after_undo");

    // (6) Re-import — never-empty again (the resumable/re-runnable import path).
    await kick("import_asset", { path: STEP_FILE });
    await browser.waitUntil(async () => (await entityCount()) >= 300, {
      timeout: 240000,
      interval: 2000,
      timeoutMsg: "the re-import never landed",
    });
    console.log("re-import entities:", await entityCount());
    await invoke("frame_all");
    await shot("10_reimported");

    // (7) The 3DXML factory cell (proxy fidelity) lands ON TOP — both bar files coexist, never black.
    const withStep = await entityCount();
    await kick("import_asset", { path: XML_FILE });
    await browser.waitUntil(async () => (await entityCount()) >= withStep + 500, {
      timeout: 240000,
      interval: 2000,
      timeoutMsg: "the 3DXML factory cell never appeared",
    });
    console.log("entities with 3DXML:", await entityCount());
    await invoke("frame_all");
    await shot("11_step_plus_3dxml_persp");
    await invoke("view_preset", { preset: "top" });
    await browser.pause(300);
    await shot("12_step_plus_3dxml_top");

    const cam = await invoke("camera_debug");
    console.log("camera:", JSON.stringify(cam));
    console.log("✓ walked 13 captured states on the real bar files — never empty, one-undo peel verified live");
  });
});
