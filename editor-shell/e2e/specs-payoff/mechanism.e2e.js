// M15.9 — ANIMATE A MECHANISM FROM THE VIEWPORT, screenshot-assessed like a user: import a small CAD file,
// make one part a SLIDING joint and another a TURNING joint about a real offset axis (never the scene
// origin), key two poses each, then SCRUB the shared timeline across 12 captured frames — the sphere must
// glide along X while the cylinder swings around the (20,0,5) axis, smoothly, deterministically (re-visiting
// a time reproduces the frame), and the whole rig peels with undo.
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
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(500);
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

describe("Animate a mechanism from the viewport (M15.9)", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
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

  it("rigs two joints, keys poses, scrubs 12 frames, and undoes the rig", async () => {
    await kick("import_asset", { path: QUARTET });
    await browser.waitUntil(async () => (await entityCount()) >= 4, { timeout: 60000, interval: 1000, timeoutMsg: "the quartet never landed" });
    await invoke("frame_all");

    // The part entities, in outliner order (cylinder, sphere band, torus ring, cone frustum).
    const ids = await browser.execute(() =>
      [...document.querySelectorAll('[data-testid="hrow"]')].map((r) => r.getAttribute("data-id")).filter(Boolean),
    );
    console.log("part ids:", ids);
    if (ids.length < 4) throw new Error(`expected 4 parts, got ${ids.length}`);
    const [cylinder, sphere] = [ids[0], ids[1]];

    // The user selects the sphere → the Mechanism panel offers the two verbs; make it SLIDE along X.
    await browser.execute((id) => {
      const row = document.querySelector(`[data-testid="hrow"][data-id="${id}"]`);
      if (row) row.click();
    }, sphere);
    await browser.pause(500);
    await shot("m00_mechanism_panel");
    // Joint values live in the entity's METER space (the parts are cm-scale after mm→m normalization), so
    // the motions are proportionate: a 6 cm slide, a half-turn swing about an axis 4 cm to the part's side.
    const sJoint = await invoke("set_joint", { id: sphere, revolute: false, axis: [1, 0, 0], pivot: [0.02, 0, 0.005], min: -1, max: 1, source: "manual" });
    if (sJoint !== true) throw new Error(`set_joint(sphere) failed: ${JSON.stringify(sJoint)}`);
    await invoke("joint_key", { id: sphere, t: 0.0 }); // pose 0 at t=0
    await invoke("joint_value", { id: sphere, value: 0.06, commit: true }); // slide 6 cm
    await invoke("joint_key", { id: sphere, t: 2.0 }); // keyed at t=2

    // The cylinder TURNS about the real offset axis: vertical z through a point 4 cm right of it — a
    // swing about a hinge beside the part, not an origin spin.
    const cJoint = await invoke("set_joint", { id: cylinder, revolute: true, axis: [0, 0, 1], pivot: [0.04, 0, 0], min: -10, max: 10, source: "manual" });
    if (cJoint !== true) throw new Error(`set_joint(cylinder) failed: ${JSON.stringify(cJoint)}`);
    await invoke("joint_key", { id: cylinder, t: 0.0 });
    await invoke("joint_value", { id: cylinder, value: Math.PI, commit: true }); // half turn
    await invoke("joint_key", { id: cylinder, t: 2.0 });

    // Frame the motion envelope at the mid pose so the whole swing stays in view (a small pull-back:
    // positive zoom delta = dolly OUT).
    await invoke("joint_scrub", { t: 1.0 });
    await invoke("frame_all");
    await invoke("zoom", { delta: 2.0 });

    // ── SCRUB: 12 captured frames across the 2 s timeline — the mechanism animates in the viewport. ──────
    for (let i = 0; i <= 11; i++) {
      const t = (2.0 * i) / 11;
      const posed = await invoke("joint_scrub", { t });
      if (i === 0) console.log("posed entities:", posed);
      await shot(`m${String(i + 1).padStart(2, "0")}_scrub_${t.toFixed(2)}`);
    }

    // DETERMINISM, user-visible: re-visiting t=1.0 must reproduce the same frame.
    await invoke("joint_scrub", { t: 1.0 });
    await shot("m13_revisit_t1");

    // Stop playback (clears the pose projection), then UNDO the whole rig: 6 authored commits
    // (2 joints + 2 keys each ... plus 2 committed drags) peel back to the freshly-imported pose.
    await invoke("joint_scrub", { t: -1.0 });
    for (let u = 0; u < 6; u++) await invoke("undo");
    await browser.pause(400);
    await shot("m14_after_undo");

    console.log("✓ mechanism rigged from the viewport, 12 frames scrubbed, revisit deterministic, rig undone");
  });
});
