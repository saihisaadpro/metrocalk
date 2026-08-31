// **What is under the pointer, against the packaged `.exe` and the real engine** (ADR-191).
//
// Three commands could answer "what is under this point" and two of them had no caller in
// `editor/src` at all: `viewport_peek` (the hover read) and `pick_diagnostics`. `HoverTooltip`, a
// finished panel that names an entity and lists its capability contract, was mounted by nothing but
// its own test. So the only way to learn what an object was, was to SELECT it — and the only way to
// reach the object behind the one you can see was a blind alt-click.
//
// **Every claim about the capability is driven through the UI.** The commands invoked directly are
// reads (`pick_candidates`, `pick_diagnostics`, `selection_ids`) plus ONE fixture op, `view_preset`,
// which arranges the camera. That is not the capability under test and it is not asserted: the seeded
// scene is a sparse scatter where 98% of rays hit nothing and, at the default camera, a 625-ray scan
// finds ZERO pixels with two objects along them — measured. A gate for click-through on a scene with
// no occlusion in it is a gate that cannot fail, which is exactly the state the previous run of this
// lane found the alt-click assertion in.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-selecttest");
const SHOT_PS1 = process.env.MTK_SHOT_PS1 || path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
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

const sleep = (ms) => browser.pause(ms);
const engineSelection = () => browser.execute(() => window.__TAURI__.core.invoke("selection_ids"));

/** A real press-pause-release, because the shell's press fires an async gizmo probe whose answer
 *  decides whether the release is a drag or a pick. A synchronous down/up/click burst asks a question
 *  the app is still answering and gets a different verdict than a hand does. */
const stageClick = (x, y, mods = {}) =>
  browser.execute(
    (cx, cy, m) => {
      const vp = document.getElementById("viewport");
      const init = { bubbles: true, cancelable: true, clientX: cx, clientY: cy, button: 0, ...m };
      vp.dispatchEvent(new PointerEvent("pointerdown", { ...init, pointerId: 1, pointerType: "mouse", buttons: 1 }));
      return new Promise((resolve) => {
        setTimeout(() => {
          vp.dispatchEvent(new PointerEvent("pointerup", { ...init, pointerId: 1, pointerType: "mouse", buttons: 0 }));
          vp.dispatchEvent(new MouseEvent("click", init));
          resolve(true);
        }, 120);
      });
    },
    x, y, mods,
  );

const rightClick = (x, y) =>
  browser.execute(
    (cx, cy) => {
      const vp = document.getElementById("viewport");
      const init = { bubbles: true, cancelable: true, clientX: cx, clientY: cy, button: 2 };
      vp.dispatchEvent(new PointerEvent("pointerdown", { ...init, pointerId: 1, pointerType: "mouse", buttons: 2 }));
      vp.dispatchEvent(new PointerEvent("pointerup", { ...init, pointerId: 1, pointerType: "mouse", buttons: 0 }));
      vp.dispatchEvent(new MouseEvent("contextmenu", init));
      return true;
    },
    x, y,
  );

/** Move the pointer and leave it there. The probe fires on SETTLE, so the pause is the gesture. */
const hoverStage = async (x, y) => {
  await browser.execute(
    (cx, cy) => {
      const vp = document.getElementById("viewport");
      vp.dispatchEvent(
        new PointerEvent("pointermove", { bubbles: true, cancelable: true, pointerId: 1, pointerType: "mouse", clientX: cx, clientY: cy, buttons: 0 }),
      );
    },
    x, y,
  );
  await sleep(700);
};

/** A pixel with `want` or more objects along its ray, hunted across the camera presets.
 *
 *  Reported in numbers, and a hard failure when nothing is found — a run that says nothing about its
 *  own reach is indistinguishable from a run that verified something. */
async function deepPixel(want) {
  const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
  for (const preset of ["persp", "front", "side"]) {
    // eslint-disable-next-line no-await-in-loop
    await browser.execute(async (p) => window.__TAURI__.core.invoke("view_preset", { preset: p }), preset);
    // eslint-disable-next-line no-await-in-loop
    await sleep(400);
    // eslint-disable-next-line no-await-in-loop
    const found = await browser.execute(async (n) => {
      let scanned = 0;
      let best = null;
      for (let gy = 2; gy <= 22; gy++) {
        for (let gx = 2; gx <= 22; gx++) {
          const fx = gx / 24;
          const fy = gy / 24;
          scanned += 1;
          // eslint-disable-next-line no-await-in-loop
          const d = await window.__TAURI__.core.invoke("pick_diagnostics", { x: fx, y: fy });
          if ((d.hits || []).length >= n) {
            best = { fx, fy, hits: d.hits.map((h) => h.entity) };
            return { scanned, best };
          }
        }
      }
      return { scanned, best };
    }, want);
    console.log(`  [${preset}] scanned ${found.scanned} rays for one meeting ${want} objects — ${found.best ? "found" : "none"}`);
    if (found.best) {
      return { ...found.best, preset, x: Math.round(found.best.fx * size.w), y: Math.round(found.best.fy * size.h) };
    }
  }
  throw new Error(`no ray across three camera presets meets ${want} objects — click-through is untestable on this scene`);
}

describe("the pointer is a question the engine answers", () => {
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
    await browser.refresh();
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length)) > 4,
      { timeout: 30000, timeoutMsg: "the seeded scene never reached the outliner" },
    );
    await sleep(800);
  });

  it("pick_candidates and pick_diagnostics are ONE answer, not two", async () => {
    const deep = await deepPixel(2);
    console.log(`  [${deep.preset}] (${deep.fx.toFixed(4)}, ${deep.fy.toFixed(4)}) meets ${deep.hits.length}: ${JSON.stringify(deep.hits)}`);

    const listed = await browser.execute(
      async (fx, fy) => window.__TAURI__.core.invoke("pick_candidates", { x: fx, y: fy }),
      deep.fx,
      deep.fy,
    );
    console.log(`  pick_candidates -> ${JSON.stringify(listed.map((c) => `${c.id}@${c.distance.toFixed(2)}`))}`);

    // SAME PIPELINE, BY CONSTRUCTION. `pick_candidates` runs the same `pick_all` a click runs and
    // resolves at the same boundary, so a row in the menu and the click that would take it cannot
    // answer differently. Asserting the two agree is what keeps that true after an edit to either.
    if (JSON.stringify(listed.map((c) => c.id)) !== JSON.stringify(deep.hits)) {
      throw new Error(`the menu's list and the diagnostic disagree: ${JSON.stringify(listed.map((c) => c.id))} vs ${JSON.stringify(deep.hits)}`);
    }
    // Nearest first — the order alt-click walks, so the gesture and the list mean the same thing by
    // "the next one".
    for (let i = 1; i < listed.length; i += 1) {
      if (listed[i].distance < listed[i - 1].distance) {
        throw new Error(`the list is not in depth order: ${JSON.stringify(listed.map((c) => c.distance))}`);
      }
    }
  });

  it("right-click on an object you have NOT selected opens the menu and lists what is behind it", async () => {
    const deep = await deepPixel(2);

    // Nothing selected: the state in which this used to open NO MENU AT ALL, which is the state every
    // other 3D tool treats as the primary way in. Cleared through the UI, by clicking empty sky.
    await stageClick(4, 4);
    await sleep(400);
    const before = await engineSelection();
    if (before.length) throw new Error(`the corner of the stage selected ${JSON.stringify(before)} — it was supposed to be empty`);

    await rightClick(deep.x, deep.y);
    await sleep(600);
    const rows = await browser.execute(() =>
      [...document.querySelectorAll('[data-testid="ctxcandidate"]')].map((r) => ({
        id: r.getAttribute("data-id"),
        text: (r.textContent || "").replace(/\s+/g, " ").trim(),
      })),
    );
    console.log(`  the menu lists ${rows.length}: ${JSON.stringify(rows)}`);
    if (rows.length < 2) throw new Error(`the menu listed ${rows.length} objects at a pixel with ${deep.hits.length} along its ray`);
    if (rows.map((r) => r.id).join(",") !== deep.hits.slice(0, rows.length).join(",")) {
      throw new Error(`the menu lists ${JSON.stringify(rows.map((r) => r.id))}, the ray meets ${JSON.stringify(deep.hits)}`);
    }
    await shot("06_under_the_pointer");

    // AND THE ROW SELECTS THE ONE BEHIND — the gesture this whole surface exists to replace.
    await browser.execute(() => document.querySelectorAll('[data-testid="ctxcandidate"]')[1].click());
    await sleep(600);
    const after = await engineSelection();
    console.log(`  chose row 2 -> engine selection ${JSON.stringify(after)}`);
    if (after.length !== 1 || after[0] !== deep.hits[1]) {
      throw new Error(`choosing the second row selected ${JSON.stringify(after)}, not ${deep.hits[1]}`);
    }
    await shot("07_selected_the_one_behind");
  });

  it("hovering NAMES the object, and changes nothing", async () => {
    const deep = await deepPixel(1);
    await stageClick(4, 4); // clear, through the UI
    await sleep(400);

    await hoverStage(deep.x, deep.y);
    const tip = await browser.execute(() => {
      const el = document.querySelector('[data-testid="tooltip-name"]');
      const host = document.querySelector('[data-testid="stage-hover"]');
      return el ? { name: el.textContent, rect: host?.getBoundingClientRect().toJSON() ?? null } : null;
    });
    console.log(`  hover at (${deep.x}, ${deep.y}) -> ${JSON.stringify(tip)}`);
    if (!tip) throw new Error("hovering an object named nothing — the tooltip never appeared");
    // HOVER MUST BE INERT. It is a read for exactly this reason: learning what something is must not
    // change what is selected, bump the document revision, or land in the undo stack.
    const selection = await engineSelection();
    if (selection.length) throw new Error(`hovering selected ${JSON.stringify(selection)} — the probe is supposed to be a read`);
    // On screen, not merely in the DOM.
    const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
    const r = tip.rect;
    if (!r || r.width < 40 || r.height < 20 || r.left < 0 || r.top < 0 || r.right > size.w || r.bottom > size.h) {
      throw new Error(`the tooltip is ${JSON.stringify(r)} in a ${size.w}x${size.h} window — off screen or collapsed`);
    }
    await shot("08_hover_names_it");
  });
});
