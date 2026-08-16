// Shape Studio on the PACKAGED .exe — the Build sub-engine's creation flows, driven through the real
// UI where it matters (cards, draw canvas, combine buttons) and through the same commands the UI calls
// where the UI adds nothing (bulk spawns for the group portrait). Every stage is OS-captured so the
// result can be judged as pixels, not just asserted about.
//
// What this proves end-to-end: shape cards create real, coloured, selected solids · the drawn outline
// becomes a solid (extrude AND revolve) · exact CSG combine replaces two objects with one · the SDF
// meld melts two spheres into one blob · a parameter edit re-bakes in place · refusals are explained
// and change nothing · undo/redo treat each operation as one step.
//
// Capture: the PrintWindow/PW_RENDERFULLCONTENT script — verified multi-shot on this machine and the
// only path that works when the desktop is locked (CopyFromScreen needs an unlocked screen).

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-shapes");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

/** OS-composited capture, size-checked: a blank capture must fail loudly, never sit in the evidence
 *  folder looking like a pass. Two paths because neither alone is reliable across desktop states:
 *  PrintWindow reads the window's own surface (works while LOCKED, can go blank mid-session on this
 *  DirectComposition stack); CopyFromScreen needs an unlocked desktop but survives when PrintWindow
 *  degrades. Try both before failing. */
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
async function shot(label) {
  await browser.pause(900);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch {
      /* a failed capture path falls through to the next */
    }
    if (!good() && existsSync(out)) rmSync(out); // never leave a 158-byte blank as "evidence"
    return good();
  };
  if (
    !attempt(capture, ["-Out", out]) &&
    !attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out])
  ) {
    throw new Error(`capture ${label} came back blank on both paths`);
  }
  console.log(`[shapes] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
}

/** The scene's entity count, read through the DOM (the outliner is virtualized — `#count` is the
 *  stable signal) even while the Scene panel is the hidden tab. */
const entityCount = () =>
  browser.execute(() => {
    const el = document.querySelector("#count");
    const n = el ? parseInt(el.textContent, 10) : NaN;
    return Number.isFinite(n) ? n : -1;
  });

/** Wait until the count reaches an absolute value (callers compute it relatively from a baseline). */
const waitCount = async (expected, label) => {
  let last = -1;
  await browser
    .waitUntil(
      async () => {
        last = await entityCount();
        return last === expected;
      },
      { timeout: 30000, interval: 500, timeoutMsg: `expected ${expected} entities (${label})` },
    )
    .catch((e) => {
      throw new Error(`${e.message} — last saw ${last}`);
    });
};

/** Click a Shape Studio control only once it is genuinely enabled (the panel disables everything
 *  while a command is in flight — clicking a disabled button is a silent no-op). */
async function clickWhenEnabled(testid) {
  const el = await $(`[data-testid="${testid}"]`);
  await el.waitForExist({ timeout: 10000 });
  await browser.waitUntil(async () => (await el.getAttribute("disabled")) === null, {
    timeout: 10000,
    timeoutMsg: `${testid} never enabled`,
  });
  await el.click();
}

/** Centre the camera on the thing that was just created (it is the engine selection) — a
 *  frame_all in a crowded scene hides each new creation behind the biggest prior one. */
async function focusCreated() {
  const id = await invoke("gizmo_selected");
  if (typeof id === "string" && id.length > 0) {
    await invoke("focus_entity", { id });
    // Back the camera off: focus frames tightly, and a tight frame inside a crowded scene can put
    // the eye INSIDE neighbouring geometry (the all-black capture this replaced).
    await invoke("zoom", { delta: -5.0 });
    await browser.pause(500);
  }
}

describe("Shape Studio — the Build engine creates, draws, combines and melds", () => {
  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch {}
    });
    await invoke("new_project");
    await browser.pause(800);
  });

  /** The onboarding card only RENDERS once the scene is non-empty (`show={!sceneEmpty}`), and its
   *  dismissed-state initialiser ran at app mount — before any flag this suite writes. So it must be
   *  dismissed through its own Skip button AFTER the first creation, via a dispatched DOM click
   *  (physical clicks miss this overlay on a locked desktop). */
  async function dismissOnboarding() {
    await browser.waitUntil(
      async () =>
        browser.execute(() => {
          const skip = document.querySelector('[data-testid="onboardSkip"]');
          if (skip) skip.dispatchEvent(new MouseEvent("click", { bubbles: true }));
          return document.querySelector('[data-testid="onboarding"]') == null;
        }),
      { timeout: 10000, timeoutMsg: "the onboarding card never dismissed" },
    );
  }

  it("opens the Build engine and shows the shape catalog", async () => {
    const build = await $('[data-testid="engine-build"]');
    await build.waitForExist({ timeout: 20000 });
    await build.click();
    const grid = await $('[data-testid="shape-studio"]');
    await grid.waitForExist({ timeout: 10000 });
    // The catalog arrives from the Rust command — all eight kinds, as cards.
    for (const kind of ["box", "sphere", "cylinder", "cone", "torus", "capsule", "wedge", "prism", "tube"]) {
      const card = await $(`[data-testid="shape-card-${kind}"]`);
      await card.waitForExist({ timeout: 10000 });
    }
    await shot("build_panel_catalog");
  });

  it("clicking shape cards creates real selected solids (UI path)", async () => {
    const n0 = await entityCount();
    expect(n0).toBe(0);

    await clickWhenEnabled("shape-card-box");
    await waitCount(n0 + 1, "box landed");
    await dismissOnboarding();
    await clickWhenEnabled("shape-card-sphere");
    await waitCount(n0 + 2, "sphere landed");
    await clickWhenEnabled("shape-card-torus");
    await waitCount(n0 + 3, "torus landed");

    // The last created thing is the ENGINE selection too (the loop closes at the gesture).
    const sel = await invoke("gizmo_selected");
    expect(typeof sel === "string" && sel.length > 0).toBe(true);
  });

  it("spawns the rest of the family for the group portrait (same command the cards call)", async () => {
    const n0 = await entityCount();
    const spots = {
      cylinder: [3.2, 0, -2.2],
      cone: [-3.2, 0, -2.2],
      capsule: [3.2, 0, 2.2],
      wedge: [-3.2, 0, 2.2],
      prism: [0, 0, -3.4],
      tube: [0, 0, 3.4],
    };
    for (const [kind, pos] of Object.entries(spots)) {
      const reply = await invoke("shape_spawn", { kind, pos });
      expect(reply.reason).toBe(null);
      expect(typeof reply.created).toBe("string");
      expect(reply.triangles).toBeGreaterThan(7);
      console.log(`[shapes] ${kind}: ${reply.message} (${reply.ms.toFixed(1)} ms)`);
    }
    await waitCount(n0 + 6, "all nine shapes");
    await invoke("frame_all");
    await shot("nine_parametric_shapes");
  });

  it("raises a drawn star into a solid (UI draw path)", async () => {
    const n0 = await entityCount();
    await clickWhenEnabled("draw-preset-star");
    await clickWhenEnabled("draw-create");
    await waitCount(n0 + 1, "star extruded");
    await focusCreated();
    await shot("drawn_star_raised");
  });

  it("a freehand stroke becomes a solid (drag-to-draw, simplified to corners)", async () => {
    const n0 = await entityCount();
    // Draw a rough blob by dispatching real pointer events along a path — the same events a mouse
    // drag produces (physical WebDriver moves cannot reach this canvas on a locked desktop).
    const drawn = await browser.execute(() => {
      const svg = document.querySelector('[data-testid="draw-canvas"]');
      if (!svg) return -1;
      const r = svg.getBoundingClientRect();
      const px = (u, v) => [r.left + (u / 100) * r.width, r.top + (v / 70) * r.height];
      const fire = (type, [cx, cy]) =>
        svg.dispatchEvent(new PointerEvent(type, { clientX: cx, clientY: cy, bubbles: true, isPrimary: true, button: 0, pointerId: 7 }));
      // A wobbly closed-ish blob: 14 samples around an ellipse with radial noise.
      const pts = Array.from({ length: 14 }, (_, i) => {
        const a = (i / 14) * Math.PI * 2;
        const rad = 24 + 6 * Math.sin(i * 2.4);
        return [50 + rad * Math.cos(a), 35 + rad * 0.65 * Math.sin(a)];
      });
      fire("pointerdown", px(pts[0][0], pts[0][1]));
      for (const [u, v] of pts.slice(1)) fire("pointermove", px(u, v));
      fire("pointerup", px(pts[0][0], pts[0][1]));
      return true;
    });
    expect(drawn).toBe(true);
    // Continuous-event state commits on React's schedule, not synchronously with the dispatch —
    // poll the label instead of reading it in the same task.
    let strokePoints = 0;
    await browser.waitUntil(
      async () => {
        const label = await browser.execute(
          () => document.querySelector('[data-testid="draw-canvas"]')?.getAttribute("aria-label") ?? "",
        );
        const m = label.match(/(\d+) so far/);
        strokePoints = m ? parseInt(m[1], 10) : 0;
        return strokePoints >= 5;
      },
      { timeout: 5000, timeoutMsg: "the freehand stroke never appeared on the canvas" },
    );
    console.log(`[shapes] freehand stroke simplified to ${strokePoints} points`);
    await clickWhenEnabled("draw-create");
    await waitCount(n0 + 1, "freehand blob landed");
    await focusCreated();
    await shot("freehand_blob_raised");
  });

  it("tapers a gear preset into a crown (the new taper control, through the UI)", async () => {
    const n0 = await entityCount();
    await clickWhenEnabled("draw-mode-extrude");
    // Set taper through the real field: type + Enter commits (the NumericField contract).
    const taper = await $('[data-testid="draw-taper"]');
    await taper.click();
    await browser.keys(["Control", "a"]);
    await browser.keys("0.35");
    await browser.keys("Enter");
    await clickWhenEnabled("draw-preset-gear");
    // The precision readout tells the truth about what the engine will receive.
    const dims = await $('[data-testid="draw-dims"]');
    expect(await dims.getText()).toContain("36 points");
    await clickWhenEnabled("draw-create");
    await waitCount(n0 + 1, "gear crown landed");
    await focusCreated();
    await shot("gear_crown_tapered");
  });

  it("spins the same star into a revolved ring (UI draw path)", async () => {
    const n0 = await entityCount();
    await clickWhenEnabled("draw-mode-revolve");
    await clickWhenEnabled("draw-preset-star");
    await clickWhenEnabled("draw-create");
    await waitCount(n0 + 1, "star revolved");
    await focusCreated();
    await shot("drawn_star_spun");
  });

  it("undo removes exactly one creation; redo brings it back", async () => {
    const n0 = await entityCount();
    await invoke("undo");
    await waitCount(n0 - 1, "undo removed the revolve");
    await invoke("redo");
    await waitCount(n0, "redo restored it");
  });

  it("carves a sphere out of a box — two objects become one (exact CSG)", async () => {
    const n0 = await entityCount();
    const box = await invoke("shape_spawn", { kind: "box", pos: [12, 0, 0] });
    const ball = await invoke("shape_spawn", { kind: "sphere", pos: [12.5, 0.4, 0.3] });
    expect(box.created && ball.created).toBeTruthy();
    await waitCount(n0 + 2, "combine sources placed");

    const carved = await invoke("shape_combine", { a: box.created, b: ball.created, op: "carve" });
    expect(carved.reason).toBe(null);
    expect(carved.message).toContain("Carved");
    expect(carved.triangles).toBeGreaterThan(50);
    await waitCount(n0 + 1, "two sources replaced by one result");

    await invoke("focus_entity", { id: carved.created });
    await browser.pause(600);
    await shot("carved_box_closeup");
    await invoke("frame_all");
  });

  it("melds two spheres into one smooth blob (the SDF smooth-union)", async () => {
    const n0 = await entityCount();
    const a = await invoke("shape_spawn", { kind: "sphere", pos: [16, 0, 0] });
    const b = await invoke("shape_spawn", { kind: "sphere", pos: [16.6, 0, 0] });
    await waitCount(n0 + 2, "meld sources placed");

    const meld = await invoke("shape_meld", { a: a.created, b: b.created, k: 0.3 });
    expect(meld.reason).toBe(null);
    expect(meld.message).toContain("Melded");
    expect(meld.triangles).toBeGreaterThan(200);
    await waitCount(n0 + 1, "meld replaced its sources");

    await invoke("focus_entity", { id: meld.created });
    await browser.pause(600);
    await shot("meld_two_spheres_closeup");
  });

  it("a parameter edit re-bakes the same entity in place", async () => {
    const n0 = await entityCount();
    const ring = await invoke("shape_spawn", { kind: "torus", pos: [20, 0, 0] });
    await waitCount(n0 + 1, "ring placed");
    const before = ring.triangles;

    const updated = await invoke("shape_update", { id: ring.created, params: { thickness: 0.45, segments: 64 } });
    expect(updated.reason).toBe(null);
    expect(updated.created).toBe(ring.created);
    expect(updated.triangles).not.toBe(before);
    await waitCount(n0 + 1, "edit created nothing new");
  });

  it("refusals are explained in plain language and change nothing", async () => {
    const count = await entityCount();

    const same = await invoke("shape_combine", { a: "1_1", b: "1_1", op: "union" });
    expect(same.reason).toContain("two different objects");

    const bogus = await invoke("shape_spawn", { kind: "dodecahedron" });
    expect(bogus.reason).toContain("dodecahedron");

    // A meld of non-meldable kinds points at the alternative rather than a bare no.
    const w = await invoke("shape_spawn", { kind: "wedge", pos: [24, 0, 0] });
    const t = await invoke("shape_spawn", { kind: "torus", pos: [24.8, 0, 0] });
    const noMeld = await invoke("shape_meld", { a: w.created, b: t.created, k: 0.3 });
    expect(noMeld.reason).toContain("Combine");
    // The two probe shapes remain; nothing else changed.
    expect(await entityCount()).toBe(count + 2);

    // And the UI's own guard: an empty canvas cannot be created from, and says why. (Clear any
    // leftovers first — a failed earlier draw test must not cascade into this assertion.)
    const clear = await $('[data-testid="draw-clear"]');
    if ((await clear.getAttribute("disabled")) === null) {
      await clear.click();
      await browser.pause(200);
    }
    const create = await $('[data-testid="draw-create"]');
    expect(await create.getAttribute("disabled")).not.toBe(null);
    expect(await create.getAttribute("title")).toContain("three points");
  });

  it("the combine buttons work from a real two-object selection (UI path)", async () => {
    // Two fresh overlapping boxes, then select both through the real hierarchy rows.
    const a = await invoke("shape_spawn", { kind: "box", pos: [28, 0, 0] });
    const b = await invoke("shape_spawn", { kind: "box", pos: [28.6, 0, 0.4] });
    const before = await entityCount();

    // Selection through the engine + the store: click the Scene tab, find the two rows by data-id.
    await (await $('[data-testid="engine-scene"]')).click();
    await browser.pause(400);
    const rowA = await $(`[data-testid="hrow"][data-id="${a.created}"]`);
    await rowA.waitForExist({ timeout: 10000 });
    // WebDriver's physical clicks cannot reach the virtualized rows on a locked desktop (verified:
    // element clicks land nowhere while the Build panel's buttons receive them). Drive the rows'
    // OWN onClick handlers with real bubbling DOM events instead — plain click selects the first,
    // ctrl-click toggles the second in, exactly the documented gesture.
    const rowClick = (id, ctrl) =>
      browser.execute(
        (rid, withCtrl) => {
          const row = document.querySelector(`[data-testid="hrow"][data-id="${rid}"]`);
          row?.dispatchEvent(new MouseEvent("click", { bubbles: true, ctrlKey: withCtrl }));
          return row?.className ?? "(missing)";
        },
        id,
        ctrl,
      );
    const rowClass = (id) =>
      browser.execute(
        (rid) => document.querySelector(`[data-testid="hrow"][data-id="${rid}"]`)?.className ?? "",
        id,
      );
    await rowClick(a.created, false);
    await browser.waitUntil(async () => (await rowClass(a.created)).includes("is-selected"), {
      timeout: 5000,
      timeoutMsg: "row A never selected",
    });
    await rowClick(b.created, true);
    await browser.waitUntil(
      async () => /is-multi|is-selected/.test(await rowClass(b.created)),
      { timeout: 5000, timeoutMsg: "row B never joined the selection" },
    );

    await (await $('[data-testid="engine-build"]')).click();
    const union = await $('[data-testid="combine-union"]');
    await browser.waitUntil(async () => (await union.getAttribute("disabled")) === null, {
      timeout: 5000,
      timeoutMsg: "the Join button never enabled — the two-row selection did not reach the panel",
    });
    await union.click();
    await browser.waitUntil(async () => (await entityCount()) === before - 1, {
      timeout: 30000,
      interval: 500,
      timeoutMsg: "the UI Join did not replace the two boxes with one union",
    });
    await invoke("frame_all");
    await shot("final_scene_all_creations");
  });

  it("every capture in this run is real pixels, not a blank", async () => {
    // The size gate ran per-shot; this closes the loop on the whole folder.
    const files = readdirSync(shots);
    expect(files.length).toBeGreaterThanOrEqual(6);
    for (const f of files) {
      expect(statSync(path.join(shots, f)).size).toBeGreaterThan(20_000);
    }
    console.log(`[shapes] evidence: ${files.join(", ")}`);
  });
});
