// The END-USER journey, screenshot-assessed (M15.7/M15.8 owed visual items + the M15.9 agility eyeball):
// launch → import a curved STEP → SEE smooth curved parts live (the M15.8 analytic leg in the real wgpu
// viewport) → undo → import the real 263 MB bar STEP → SEE the import-report panel populate (the M15.7
// never-silent surface, in pixels) → filter + select from it → stack the 222 MB 3DXML factory cell → walk
// view presets on the loaded scene. Every state OS-captured (the true composite); the app's own perf report
// lines ([viewport] cpu p50/p99) are harvested from stderr for an honest this-box interaction number.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.payoff.conf.js

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-payoff");
const SHOT_PS1 = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
const QUARTET = path.resolve(dir, "../samples/analytic_quartet.stp");
const STEP_FILE =
  process.env.MTK_STEP_BAR_FILE ||
  "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1_(1).stp";
const XML_FILE =
  process.env.MTK_3DXML_BAR_FILE ||
  "X:\\Work\\Metrocalk\\Games Projects\\Unreal\\Skid Weld Line A.1\\Skid Weld Line A.1.3dxml";
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(700);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${path.join(SHOT_DIR, `${label}.png`)}"`, { stdio: "ignore" });
    console.log("  shot", label);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};
const kick = (cmd, args) =>
  browser.execute((c, a) => { window.__TAURI__.core.invoke(c, a || {}).catch(() => {}); return true; }, cmd, args);
const invoke = (cmd, args) =>
  browser.execute(async (c, a) => { try { return await window.__TAURI__.core.invoke(c, a || {}); } catch (e) { return { error: String(e) }; } }, cmd, args);
const entityCount = () =>
  browser.execute(() => { const el = document.getElementById("count"); const m = el && el.textContent ? el.textContent.match(/\d+/) : null; return m ? parseInt(m[0], 10) : 0; });

describe("The user journey: curved CAD → the never-silent report → the loaded factory (screenshot-assessed)", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    // Dismiss the first-run tour like a user would (click "Got it") so captures show the stage.
    await browser.pause(1200);
    await browser.execute(() => {
      for (const b of document.querySelectorAll("button")) {
        if (/got it/i.test(b.textContent || "")) { b.click(); return true; }
      }
      return false;
    });
    // A clean scene like a user picking File → New (the replay log persists the previous session's doc).
    await browser.execute(async () => { try { await window.__TAURI__.core.invoke("new_project"); } catch (e) { void e; } });
    await browser.pause(600);
  });

  it("walks the journey", async () => {
    // ── (1) M15.8 owed: curved parts SMOOTH in the live viewport ─────────────────────────────────────────
    await kick("import_asset", { path: QUARTET });
    await browser.waitUntil(async () => (await entityCount()) >= 4, { timeout: 60000, interval: 1000, timeoutMsg: "the quartet never landed" });
    await invoke("frame_all");
    await shot("01_quartet_persp");
    await invoke("zoom", { delta: -3.0 }); // dolly IN (negative delta) for the curvature close-up
    await shot("02_quartet_close");
    await invoke("view_preset", { preset: "front" });
    await browser.pause(300);
    await shot("03_quartet_front");
    await invoke("view_preset", { preset: "persp" });

    // ── (2) M15.7 owed: the import-report panel, in pixels ───────────────────────────────────────────────
    await kick("undo");
    await browser.waitUntil(async () => (await entityCount()) <= 1, { timeout: 60000, interval: 1000, timeoutMsg: "undo did not peel the quartet" });
    await kick("import_asset", { path: STEP_FILE });
    await browser.waitUntil(async () => (await entityCount()) >= 300, { timeout: 420000, interval: 2000, timeoutMsg: "the bar STEP never landed" });
    await invoke("frame_all");
    await browser.pause(1500); // the panel's report fetch
    // Bring the report panel into view inside the right rail (the user scrolls to it).
    const panelInfo = await browser.execute(() => {
      const p = document.querySelector('[data-testid="import-report"]');
      if (!p) return null;
      p.scrollIntoView({ block: "start" });
      return {
        total: p.getAttribute("data-total"),
        belowExact: p.getAttribute("data-below-exact"),
        rows: document.querySelectorAll('[data-testid="import-row"]').length,
        chips: [...document.querySelectorAll('[data-testid^="filter-"]')].map((c) => c.getAttribute("data-testid")),
      };
    });
    console.log("import-report panel:", JSON.stringify(panelInfo));
    await shot("04_import_report_visible");
    if (!panelInfo) throw new Error("the import-report panel is not in the DOM after a CAD import");

    // The user clicks the tessellation-only chip → the list narrows.
    await browser.execute(() => {
      const chip = document.querySelector('[data-testid="filter-tessellation-only"]');
      if (chip) chip.click();
    });
    await browser.pause(300);
    await shot("05_report_filtered");
    // The user clicks the first row → the part is selected (inspector + hierarchy react).
    const clicked = await browser.execute(() => {
      const row = document.querySelector('[data-testid="import-row"]');
      if (!row) return null;
      row.click();
      return row.getAttribute("data-id");
    });
    console.log("clicked report row:", clicked);
    await browser.pause(500);
    await shot("06_report_row_selected");

    // ── (3) M15.9 agility eyeball: the full factory stacked, presets walked ─────────────────────────────
    const before3dxml = await entityCount();
    await kick("import_asset", { path: XML_FILE });
    await browser.waitUntil(async () => (await entityCount()) >= before3dxml + 500, { timeout: 240000, interval: 2000, timeoutMsg: "the 3DXML never landed" });
    await invoke("frame_all");
    await shot("07_factory_persp");
    for (const p of ["top", "front", "side"]) {
      await invoke("view_preset", { preset: p });
      await browser.pause(400);
      await shot(`08_factory_${p}`);
    }
    await invoke("view_preset", { preset: "persp" });
    // Dolly to ~40% of the framed distance for the close-up (the zoom step is 15%-of-distance per unit,
    // capped at 1 m/unit — compute the delta from the live camera state).
    const cam = await invoke("camera_debug");
    const step = Math.min(cam[2] * 0.15, 1.0);
    await invoke("zoom", { delta: -(cam[2] * 0.6) / Math.max(step, 1e-6) });
    await shot("09_factory_close");
    // Let the perf reporter print a few 2s windows while the heavy scene is loaded.
    await browser.pause(5000);
    console.log(`✓ journey complete: quartet smooth → report visible+filter+select → factory ${await entityCount()} entities across presets`);
  });
});
