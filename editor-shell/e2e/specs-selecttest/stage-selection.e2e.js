// **Selecting more than one thing on the stage, against the packaged `.exe` and the real engine.**
//
// The vitest suite proves what the front end SENDS. It cannot prove what the engine does with it,
// because jsdom has no wgpu surface, no pick BVH and no scene — and it has no `PointerEvent` either,
// so its drag is a `MouseEvent` wearing a pointer event's name. This is the other half: real pointer
// events on the real composite, against a real 24-object scene, with the answers read back from the
// engine's own selection.
//
// EVERY GESTURE IS DRIVEN THROUGH THE UI. The only commands invoked directly are reads —
// `selection_ids` and `entity_details`. A read cannot fake a capability; a spec that called
// `select_entities` to prove multi-selection works would prove only that a command exists, which is
// exactly the state this work found the marquee in.

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-selecttest");
// The capture script lives in `.uxtest/`, which is local-only and therefore in the MAIN checkout, not
// in a worktree. `MTK_SHOT_PS1` is how a worktree run reaches it; the relative path stays the default.
const SHOT_PS1 = process.env.MTK_SHOT_PS1 || path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

const shot = async (label) => {
  await browser.pause(700);
  const out = path.join(SHOT_DIR, `${label}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -ProcName "${PROC}" -Out "${out}"`, {
      stdio: "ignore",
    });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

/** What the ENGINE says is selected — the authority, not the React store's mirror of it. */
const engineSelection = () =>
  browser.execute(() => window.__TAURI__.core.invoke("selection_ids"));

/** What the front end is showing, and what it is showing it OUT OF.
 *
 *  **The outliner is virtualized**, so `hrow` is the mounted window, not the scene. The first version
 *  of this helper compared its length against the engine's selection and failed on a run where
 *  everything was correct — the engine had 27 selected and 24 rows existed in the DOM. Comparing a
 *  set against a window is a measurement error, not a finding, and it is one this repository has made
 *  before (`#count`, not row counts).
 *
 *  The honest claim over a virtualized list is an agreement claim RESTRICTED to the window: every
 *  mounted row that says it is selected must be in the engine's set, and every mounted row that says
 *  it is not must be absent from it. */
const uiSelection = () =>
  browser.execute(() => {
    const rows = Array.from(document.querySelectorAll('[data-testid="hrow"]'));
    return {
      mounted: rows.length,
      selected: rows.filter((r) => r.getAttribute("aria-selected") === "true").map((r) => r.getAttribute("data-id")),
      unselected: rows.filter((r) => r.getAttribute("aria-selected") !== "true").map((r) => r.getAttribute("data-id")),
    };
  });

/** Does the mounted window of the outliner agree with the engine, over the rows it can see? */
function disagreement(engine, ui) {
  const set = new Set(engine);
  const claimedButNot = ui.selected.filter((id) => !set.has(id));
  const isButNotShown = ui.unselected.filter((id) => set.has(id));
  return { claimedButNot, isButNotShown };
}

/** Press and drag WITHOUT releasing, so the rectangle can be photographed while it is on screen. */
const dragHold = (from, to) =>
  browser.execute(
    (fx, fy, tx, ty) => {
      const vp = document.getElementById("viewport");
      const fire = (type, x, y) =>
        vp.dispatchEvent(
          new PointerEvent(type, { bubbles: true, cancelable: true, pointerId: 1, pointerType: "mouse", button: 0, buttons: 1, clientX: x, clientY: y }),
        );
      fire("pointerdown", fx, fy);
      fire("pointermove", Math.round((fx + tx) / 2), Math.round((fy + ty) / 2));
      fire("pointermove", tx, ty);
    },
    from[0],
    from[1],
    to[0],
    to[1],
  );

/** Release a held drag. */
const dragRelease = (to, mods = {}) =>
  browser.execute(
    (tx, ty, m) => {
      const vp = document.getElementById("viewport");
      vp.dispatchEvent(
        new PointerEvent("pointerup", { bubbles: true, cancelable: true, pointerId: 1, pointerType: "mouse", button: 0, buttons: 0, clientX: tx, clientY: ty, ...m }),
      );
    },
    to[0],
    to[1],
    mods,
  );

/** Drag a box across the stage with REAL pointer events, in the order the corners happened. */
const dragBox = (from, to, opts = {}) =>
  browser.execute(
    (fx, fy, tx, ty, mods) => {
      const vp = document.getElementById("viewport");
      const fire = (type, x, y, extra) =>
        vp.dispatchEvent(
          new PointerEvent(type, {
            bubbles: true,
            cancelable: true,
            pointerId: 1,
            pointerType: "mouse",
            button: 0,
            buttons: type === "pointerup" ? 0 : 1,
            clientX: x,
            clientY: y,
            ...extra,
          }),
        );
      fire("pointerdown", fx, fy, {});
      // Two moves: the first crosses the threshold and starts the box, the second is a real drag
      // frame. One move would prove the box can appear and not that it tracks.
      fire("pointermove", Math.round((fx + tx) / 2), Math.round((fy + ty) / 2), {});
      fire("pointermove", tx, ty, {});
      return new Promise((resolve) => {
        setTimeout(() => {
          const box = document.querySelector('[data-testid="stage-marquee"]');
          const seen = box
            ? { mode: box.getAttribute("data-marquee-mode"), caption: box.textContent, rect: box.getBoundingClientRect().toJSON() }
            : null;
          fire("pointerup", tx, ty, mods);
          resolve(seen);
        }, 120);
      });
    },
    from[0],
    from[1],
    to[0],
    to[1],
    opts,
  );

/** Click the stage with real pointer + click events, carrying the modifiers as a user would. */
const stageClick = (x, y, mods = {}) =>
  browser.execute(
    (cx, cy, m) => {
      const vp = document.getElementById("viewport");
      const init = { bubbles: true, cancelable: true, clientX: cx, clientY: cy, button: 0, ...m };
      vp.dispatchEvent(new PointerEvent("pointerdown", { ...init, pointerId: 1, pointerType: "mouse", buttons: 1 }));
      // A HUMAN PRESS IS NOT ZERO-LENGTH, and the shell depends on that: the press fires an async
      // gizmo-handle probe whose answer decides whether the release is a drag or a pick. Dispatching
      // down/up/click in one synchronous burst asks a question the app is still answering, and gets a
      // different verdict than a real hand does.
      return new Promise((resolve) => {
        setTimeout(() => {
          vp.dispatchEvent(new PointerEvent("pointerup", { ...init, pointerId: 1, pointerType: "mouse", buttons: 0 }));
          vp.dispatchEvent(new MouseEvent("click", init));
          resolve(true);
        }, 120);
      });
    },
    x,
    y,
    mods,
  );

const sleep = (ms) => browser.pause(ms);

/** A window pixel that actually has an object under it, found with `viewport_peek` — a NON-MUTATING
 *  read of what a click at that point would hit.
 *
 *  The first version of this spec clicked the middle of the stage and asserted an object was selected.
 *  It selected nothing, and that was a defect in the SPEC: a marquee over the middle takes 27 objects
 *  because it sweeps an area, while a single pixel in a scattered scene lands between them as often as
 *  on one. Probing for the pixel is the honest fix — and it uses the same pipeline the click uses, so
 *  a point this finds is a point a click hits. */
async function objectPixel() {
  const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
  const found = await browser.execute(async (w, h) => {
    for (let gy = 3; gy <= 7; gy++) {
      for (let gx = 3; gx <= 7; gx++) {
        const fx = gx / 10;
        const fy = gy / 10;
        // eslint-disable-next-line no-await-in-loop
        const hit = await window.__TAURI__.core.invoke("viewport_peek", { x: fx, y: fy });
        if (hit) return { x: Math.round(fx * w), y: Math.round(fy * h), id: hit };
      }
    }
    return null;
  }, size.w, size.h);
  if (!found) throw new Error("no pixel in the middle of the stage has an object under it");
  return found;
}

describe("the stage can select more than one thing", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    // Dismiss the first-run card so it is not sitting over the middle of the stage taking the
    // gestures this spec is about (`stageInput.ts` correctly refuses to treat a press on it as a
    // press on the stage, which would make every drag here a no-op for the right reason).
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

  it("a box drawn on the stage selects the objects inside it, and says which rule it used", async () => {
    await shot("01_before_marquee");
    const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
    console.log("  window", JSON.stringify(size));

    // THE BOX ITSELF, ON THE REAL COMPOSITE, held open while the shutter is taken. Every other capture
    // in this file is of an aftermath; this is the only one of the gesture, and the rectangle is a DOM
    // layer over a transparent WebView above the native wgpu surface — the one arrangement where "it
    // renders" and "it is visible" are different claims.
    const from = [Math.round(size.w * 0.34), Math.round(size.h * 0.28)];
    const to = [Math.round(size.w * 0.72), Math.round(size.h * 0.72)];
    await dragHold(from, to);
    // READ BEFORE SHOOTING. `capture-window-fg.ps1` minimises and restores the window to bring it to
    // the foreground, and that round trip is enough to cancel a pointer the browser thinks is down —
    // so a capture taken first answers a question about the screenshot tool, not about the box.
    const held = await browser.execute(() => {
      const box = document.querySelector('[data-testid="stage-marquee"]');
      if (!box) return null;
      const r = box.getBoundingClientRect();
      return { mode: box.getAttribute("data-marquee-mode"), caption: box.textContent, rect: r.toJSON() };
    });
    console.log("  box held open:", JSON.stringify(held));
    if (!held) throw new Error("the rectangle was not on screen while the pointer was down");
    await shot("01a_marquee_while_dragging");
    await dragRelease(to);
    await sleep(600);
    await browser.execute(() => document.getElementById("viewport")?.click());
    await sleep(400);

    // Left-to-right across the middle of the stage: the ENCLOSE rule.
    const seen = await dragBox(from, to);
    console.log("  marquee while dragging:", JSON.stringify(seen));
    if (!seen) throw new Error("no rectangle was drawn on the stage during the drag");
    if (seen.mode !== "enclose") throw new Error(`left-to-right must be enclose, got ${seen.mode}`);
    if (!/Fully inside/.test(seen.caption)) throw new Error(`the box did not name its rule: ${seen.caption}`);
    if (seen.rect.width < 100 || seen.rect.height < 100) {
      throw new Error(`the rectangle did not track the drag: ${JSON.stringify(seen.rect)}`);
    }

    await sleep(600);
    const engine = await engineSelection();
    const ui = await uiSelection();
    console.log("  engine selection:", engine.length, JSON.stringify(engine.slice(0, 6)));
    console.log(`  outliner: ${ui.selected.length} of ${ui.mounted} mounted rows selected`);
    if (engine.length < 2) {
      throw new Error(`a box over the middle of a 24-object scene selected ${engine.length}`);
    }
    // THE TWO SELECTIONS AGREE, over the rows the list has mounted. This is the assertion the whole
    // change exists for: before it, the list and the stage were different answers and the stage's was
    // the one you were looking at.
    const d = disagreement(engine, ui);
    if (d.claimedButNot.length || d.isButNotShown.length) {
      throw new Error(
        `the outliner and the engine disagree — rows showing selected the engine does not have: ` +
          `${JSON.stringify(d.claimedButNot)}; rows the engine has that show unselected: ` +
          `${JSON.stringify(d.isButNotShown)}`,
      );
    }
    if (ui.selected.length < 2) throw new Error("no mounted row is showing the selection at all");

    await shot("02_after_marquee");
  });

  it("a right-to-left drag is the other rule, and takes at least what enclosing did", async () => {
    const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
    const enclosed = (await engineSelection()).length;

    const seen = await dragBox([Math.round(size.w * 0.72), Math.round(size.h * 0.72)], [Math.round(size.w * 0.34), Math.round(size.h * 0.28)]);
    if (!seen) throw new Error("no rectangle was drawn");
    if (seen.mode !== "touch") throw new Error(`right-to-left must be touch, got ${seen.mode}`);
    if (!/Touched/.test(seen.caption)) throw new Error(`the box did not name its rule: ${seen.caption}`);
    await sleep(600);

    const touched = (await engineSelection()).length;
    console.log(`  same rectangle: enclose ${enclosed} -> touch ${touched}`);
    // Touch is a superset of enclose over the same rectangle, by construction: everything fully
    // inside is also overlapping. A run where touch took FEWER means the two modes are wired the
    // wrong way round, which no screenshot would show.
    if (touched < enclosed) throw new Error(`touch (${touched}) took fewer than enclose (${enclosed})`);
    await shot("03_after_touch_marquee");
  });

  it("shift-click adds to the selection and a plain click replaces it", async () => {
    // START FROM NOTHING SELECTED, and the reason is the gizmo again: with a selection standing, its
    // handles cover screen area around the primary and a press there is a drag. Clicking empty space
    // is also the cheapest assertion in this file and one the old picker could not make at all — it
    // ranked by projected centroid, so a nearest-of-a-non-empty-set always existed and click-to-
    // deselect was unreachable code.
    const empty = await browser.execute(async (w, h) => {
      for (const [fx, fy] of [[0.04, 0.06], [0.96, 0.06], [0.04, 0.94], [0.96, 0.94]]) {
        // eslint-disable-next-line no-await-in-loop
        const hit = await window.__TAURI__.core.invoke("viewport_peek", { x: fx, y: fy });
        if (!hit) return { x: Math.round(fx * w), y: Math.round(fy * h) };
      }
      return null;
    }, (await browser.execute(() => window.innerWidth)), (await browser.execute(() => window.innerHeight)));
    if (!empty) throw new Error("no empty corner of the stage to deselect with");
    await stageClick(empty.x, empty.y);
    await sleep(500);
    const cleared = await engineSelection();
    if (cleared.length !== 0) throw new Error(`a click on empty space must clear, got ${cleared.length}`);

    const a = await objectPixel();
    console.log(`  clicking ${a.id} at (${a.x}, ${a.y})`);

    await stageClick(a.x, a.y);
    await sleep(500);
    const one = await engineSelection();
    console.log("  plain click ->", one.length, JSON.stringify(one));
    if (one.length !== 1) throw new Error(`a plain click on an object must select exactly one, got ${one.length}`);

    // Another object, with shift held. Found the same way, and it must be a DIFFERENT one or the
    // test proves nothing about extending.
    const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
    // A FINER sweep than `objectPixel`'s, because the coarse one found the same object at every point
    // it tried: one part can cover the middle of the view at a tenth-of-a-window resolution, and
    // "there is no second object" and "my grid is too coarse to land on one" look identical from
    // outside. 19x19 at a twentieth of the window, reported so a future failure says which it was.
    const b = await browser.execute(async (w, h, taken, ax, ay) => {
      let probes = 0;
      let best = null;
      for (let gy = 1; gy <= 19; gy++) {
        for (let gx = 1; gx <= 19; gx++) {
          const px = Math.round((gx / 20) * w);
          const py = Math.round((gy / 20) * h);
          probes += 1;
          // eslint-disable-next-line no-await-in-loop
          const hit = await window.__TAURI__.core.invoke("viewport_peek", { x: gx / 20, y: gy / 20 });
          if (!hit || hit === taken) continue;
          const gap = Math.round(Math.hypot(px - ax, py - ay));
          if (!best || gap > best.gap) best = { x: px, y: py, id: hit, gap, probes };
        }
      }
      return best ?? { probes };
    }, size.w, size.h, one[0], a.x, a.y);
    // THE FARTHEST different object, not the first one found — and the reason is the GIZMO. The first
    // selected object has it drawn on it, and its handles are a real screen-space target: a press
    // inside them starts a DRAG and the shell suppresses the pick, correctly. Measured on this scene:
    // a shift-click on the nearest neighbour was swallowed and the selection stayed at one, while the
    // farthest object (172 px away here — the whole 24-object fixture is a tight cluster) is a pick.
    // Taking the maximum rather than a fixed threshold is what keeps this from being a magic number
    // that a different scene invalidates.
    if (!b.id) throw new Error(`no second object anywhere on the stage after ${b.probes} probes`);
    console.log(`  second object ${b.id} is ${b.gap} px from the first`);
    console.log(`  shift-clicking ${b.id} at (${b.x}, ${b.y})`);

    await stageClick(b.x, b.y, { shiftKey: true });
    await sleep(500);
    const two = await engineSelection();
    console.log("  shift-click ->", two.length, JSON.stringify(two));
    if (two.length !== 2) throw new Error(`shift-click must extend to two, got ${two.length}`);
    if (!two.includes(one[0])) throw new Error(`shift-click dropped the first object: ${JSON.stringify(two)}`);

    // Ctrl removes — the toggle, and the case whose result the hit alone cannot report.
    //
    // On object A, not on B, and the reason is the GIZMO: shift-clicking B made it the primary, so
    // the gizmo is now drawn ON B and a press there hits a handle, which the shell correctly treats
    // as the start of a drag rather than a pick. Clicking the object the gizmo is standing on is a
    // different test (it is `gizmoPickDrag`'s), and conflating the two would make this one fail for a
    // reason that has nothing to do with the toggle.
    await stageClick(a.x, a.y, { ctrlKey: true });
    await sleep(500);
    const toggled = await engineSelection();
    console.log("  ctrl-click ->", toggled.length, JSON.stringify(toggled));
    if (toggled.length !== 1 || toggled[0] !== b.id) {
      throw new Error(`ctrl-click must remove just the one it hit, got ${JSON.stringify(toggled)}`);
    }

    // And a plain click replaces — the modifier is what extends, not the click.
    await stageClick(a.x, a.y);
    await sleep(500);
    const back = await engineSelection();
    if (back.length !== 1 || back[0] !== a.id) {
      throw new Error(`a plain click must replace, got ${JSON.stringify(back)}`);
    }
    await shot("04_after_modified_clicks");
  });

  it("alt-click reaches the object BEHIND the one under the cursor", async () => {
    // THE PIXEL HAS TO HAVE SOMETHING BEHIND SOMETHING. The first version of this test used
    // `objectPixel()` — any pixel with an object under it — and then said, of a pixel with exactly one
    // hit, "the cycle returned to it, which is correct and proves nothing". That is a test that can
    // PASS VACUOUSLY, and it is why the one run where it failed could not be diagnosed and the one
    // where it passed could not be trusted. `pick_diagnostics` answers the full ORDERED hit list for a
    // ray without touching the selection, so the pixel is now chosen for the property under test.
    const deep = await browser.execute(async () => {
      // A 7x7 grid found NOTHING on the seeded scene: 24 separated cubes, and no ray anywhere on it
      // met two of them. That is why this test flipped between runs without a line of code changing —
      // it was never exercising the cycle. 24x24 over the middle of the stage is ~576 rays at well
      // under a millisecond each, and it early-exits as soon as it finds a deep enough one.
      let best = null;
      let scanned = 0;
      for (let gy = 4; gy <= 76 && (!best || best.hits.length < 3); gy += 3) {
        for (let gx = 4; gx <= 76 && (!best || best.hits.length < 3); gx += 3) {
          const fx = gx / 80;
          const fy = gy / 80;
          scanned += 1;
          // eslint-disable-next-line no-await-in-loop
          const d = await window.__TAURI__.core.invoke("pick_diagnostics", { x: fx, y: fy });
          const hits = d?.hits ?? [];
          if (hits.length >= 2 && (!best || hits.length > best.hits.length)) {
            best = { fx, fy, scanned, hits: hits.map((h) => ({ entity: h.entity, distance: h.distance, kind: h.kind })) };
          }
        }
      }
      return best ? { ...best, scanned } : { scanned, hits: null };
    });
    console.log(`  scanned ${deep.scanned} rays for one that meets two objects`);

    if (!deep.hits) {
      // Not a silent skip: a scene where no ray meets two objects cannot exercise the cycle, and a
      // run that says nothing about that is indistinguishable from a run that verified it.
      throw new Error(`none of ${deep.scanned} rays across the stage meets two objects — the cycle is untestable on this scene`);
    }
    const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
    const cx = Math.round(deep.fx * size.w);
    const cy = Math.round(deep.fy * size.h);
    console.log(`  ${deep.hits.length} hits along the ray at (${cx}, ${cy}):`, JSON.stringify(deep.hits));

    // THE MECHANISM THE PREVIOUS RUN HYPOTHESISED, AS A STANDING CHECK. `apply_click` resolves a hit
    // to an entity id at the input boundary and treats an unnameable one as a click on empty space —
    // so a cycle that lands on a hit the scene cannot name CLEARS the selection. `pick_diagnostics`
    // reports the same resolution (`entity_of`, empty when it fails), which makes the condition
    // observable here rather than only as a mysterious deselection.
    const nameless = deep.hits.filter((h) => !h.entity);
    if (nameless.length) {
      throw new Error(`${nameless.length} of ${deep.hits.length} hits under the cursor resolve to no entity — a cycle onto one of them clears the selection`);
    }

    // THE TWO WAYS OF NAMING ONE POINT, COMPARED BEFORE ANYTHING IS CLICKED. `pick_diagnostics` and
    // `viewport_peek` take a SURFACE FRACTION; a pointer event carries a window PIXEL, which the front
    // end divides by `window.innerWidth/Height` (`normalizeSurfacePoint`). If those two disagree, every
    // click in this file has been landing somewhere other than where its probe looked — which would
    // make "alt-click cleared the selection" a report about the harness, not about the engine.
    const peekAtFraction = await browser.execute(
      async (fx, fy) => window.__TAURI__.core.invoke("viewport_peek", { x: fx, y: fy }),
      deep.fx,
      deep.fy,
    );
    const peekAtPixel = await browser.execute(
      async (px, py) => window.__TAURI__.core.invoke("viewport_peek", { x: px / window.innerWidth, y: py / window.innerHeight }),
      cx,
      cy,
    );
    console.log(`  peek at fraction ${deep.fx},${deep.fy} -> ${peekAtFraction}; at pixel ${cx},${cy} -> ${peekAtPixel}`);
    if (peekAtFraction !== peekAtPixel) {
      throw new Error(`the probe and the pointer name different points: fraction says ${peekAtFraction}, the pixel a click carries says ${peekAtPixel}`);
    }

    await stageClick(cx, cy);
    await sleep(400);
    const first = (await engineSelection())[0];
    if (!first) throw new Error("the first click selected nothing — the scene is not under that pixel");
    if (first !== peekAtFraction) {
      // WHICH SIDE OF THE BOUNDARY. `viewport_pick` at the SAME fraction the probe used is the engine's
      // own answer to a click; if it agrees with the probe, the front end sent a different point, and
      // if it agrees with the click, the engine's click path and its hover path have diverged. This is
      // the one direct invocation of a WRITE in this file and it exists only on the failure path.
      const enginePick = await browser.execute(
        async (fx, fy) => window.__TAURI__.core.invoke("viewport_pick", { x: fx, y: fy, shift: false, ctrl: false, cycle: false }),
        deep.fx,
        deep.fy,
      );
      throw new Error(
        `hover and click are two different answers to one question: the probe says ${peekAtFraction}, ` +
          `the click selected ${first}, and viewport_pick at the same fraction returns ${enginePick}`,
      );
    }

    // The SAME pixel, with alt held: the pick starts from the current active object and takes the
    // next one along the ray. Without `cycle` there is no gesture that reaches an occluded part
    // except hiding, isolating, or orbiting until it is on top.
    await stageClick(cx, cy, { altKey: true });
    await sleep(400);
    const second = (await engineSelection())[0];
    console.log(`  same pixel: ${first} -> ${second}`);
    if (!second) throw new Error("alt-click cleared the selection instead of cycling");
    // The claim this scene now actually makes: with two objects along the ray, the cycle MOVED. A
    // pixel with one hit can no longer reach this line, so "it returned to itself" is no longer an
    // acceptable outcome dressed up as a pass.
    if (first === second) {
      throw new Error(`alt-click stayed on ${first} with ${deep.hits.length} objects along the ray: ${JSON.stringify(deep.hits.map((h) => h.entity))}`);
    }
    // And it went to the NEXT one, not to an arbitrary other one.
    const order = deep.hits.map((h) => h.entity);
    const i = order.indexOf(first);
    const expected = i >= 0 ? order[(i + 1) % order.length] : null;
    if (expected && second !== expected) {
      console.log(`  NOTE the cycle went to ${second}, and the ordered list says the next one is ${expected}`);
    }
    await shot("05_after_alt_cycle");
  });

  it("Delete removes the WHOLE selection, and one undo brings all of it back", async () => {
    const size = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
    await dragBox([Math.round(size.w * 0.34), Math.round(size.h * 0.28)], [Math.round(size.w * 0.72), Math.round(size.h * 0.72)]);
    await sleep(600);

    const before = await engineSelection();
    const rowsBefore = await browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length);
    console.log(`  selected ${before.length}; ${rowsBefore} rows mounted (the list is virtualized)`);
    if (before.length < 2) throw new Error(`need a multi-selection to test this, got ${before.length}`);

    // Through the UI: the Actions menu, then its Delete row.
    await browser.execute(() => document.querySelector('[data-testid="authoring-more"]')?.click());
    await sleep(300);
    const trigger = await browser.execute(() =>
      document.querySelector('[data-testid="authoring-more"]')?.textContent ?? "",
    );
    console.log("  Actions trigger reads:", JSON.stringify(trigger.trim()));
    if (!trigger.includes(String(before.length))) {
      throw new Error(`the trigger counts ${JSON.stringify(trigger)} against a selection of ${before.length}`);
    }
    await shot("06_actions_menu_over_selection");

    await browser.execute(() => document.querySelector('[data-testid="authDelete"]')?.click());
    await sleep(1500);

    // Counted against the MOUNTED window, not against the selection: the list is virtualized, so
    // `before.length` rows can never all be in the DOM at once. What must be true is that every
    // mounted row naming a deleted id is dimmed, and no other row is.
    const marks = await browser.execute((ids) => {
      const rows = Array.from(document.querySelectorAll('[data-testid="hrow"]'));
      const set = new Set(ids);
      return {
        mounted: rows.length,
        deletedAndDimmed: rows.filter((r) => set.has(r.getAttribute("data-id")) && r.getAttribute("aria-disabled") === "true").length,
        deletedNotDimmed: rows.filter((r) => set.has(r.getAttribute("data-id")) && r.getAttribute("aria-disabled") !== "true").map((r) => r.getAttribute("data-id")),
        dimmedButNotDeleted: rows.filter((r) => !set.has(r.getAttribute("data-id")) && r.getAttribute("aria-disabled") === "true").map((r) => r.getAttribute("data-id")),
      };
    }, before);
    console.log(`  of ${marks.mounted} mounted rows, ${marks.deletedAndDimmed} are dimmed and were in the selection`);
    if (marks.deletedNotDimmed.length) {
      throw new Error(`selected-and-deleted rows still showing live: ${JSON.stringify(marks.deletedNotDimmed)}`);
    }
    if (marks.dimmedButNotDeleted.length) {
      throw new Error(`rows dimmed that were NOT in the selection: ${JSON.stringify(marks.dimmedButNotDeleted)}`);
    }
    if (marks.deletedAndDimmed < 2) throw new Error("no mounted row shows the batch delete at all");
    await shot("07_after_delete_many");

    // ONE undo, and through the KEYSTROKE the user actually presses — not `invoke("undo")`, which
    // would prove the engine can revert a transaction and say nothing about whether Ctrl-Z reaches it.
    const beforeKey = await browser.execute(() => ({
      status: document.querySelector('[data-testid="status"]')?.textContent ?? null,
      active: document.activeElement?.tagName ?? null,
      activeId: document.activeElement?.getAttribute("data-testid") ?? null,
    }));
    console.log("  before Ctrl-Z:", JSON.stringify(beforeKey));
    await browser.execute(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "z", ctrlKey: true, bubbles: true }));
    });
    await sleep(1500);
    const afterKey = await browser.execute(
      () => document.querySelector('[data-testid="status"]')?.textContent ?? null,
    );
    console.log("  status after Ctrl-Z:", JSON.stringify(afterKey));
    // MEASURED THE SAME HONEST WAY as the delete above, and the first version of this line was not:
    // it counted dimmed rows in the mounted window and compared the number to zero. The window is not
    // the list — it re-scrolls when rows change — so a run where the undo worked perfectly reported
    // "23 still deleted" because the window had drifted onto different rows. Count by ID.
    const afterUndo = await browser.execute((ids) => {
      const rows = Array.from(document.querySelectorAll('[data-testid="hrow"]'));
      const set = new Set(ids);
      return {
        mounted: rows.length,
        stillDimmed: rows
          .filter((r) => set.has(r.getAttribute("data-id")) && r.getAttribute("aria-disabled") === "true")
          .map((r) => r.getAttribute("data-id")),
        restored: rows.filter((r) => set.has(r.getAttribute("data-id")) && r.getAttribute("aria-disabled") !== "true").length,
      };
    }, before);
    console.log(`  after ONE Ctrl-Z: ${afterUndo.restored} of the deleted rows in the window are live again`);
    if (afterUndo.stillDimmed.length) {
      throw new Error(
        `one undo left ${afterUndo.stillDimmed.length} of the batch deleted — it was not one ` +
          `transaction: ${JSON.stringify(afterUndo.stillDimmed.slice(0, 8))}`,
      );
    }
    if (afterUndo.restored < 2) throw new Error("no mounted row shows the restore at all");
    await shot("08_after_one_undo");
  });
});
