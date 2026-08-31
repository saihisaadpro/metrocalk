// THE STAGE ANSWERS WHAT THE CURSOR IS OVER — on the PACKAGED .exe, measured in real pixels.
//
// BEFORE: `viewport_peek` had named the object under the cursor since M3.3 and the aim badge had read
// the whole chain it hangs from since ADR-171 — `Box · 1 part` in `Assembly Hall · 7 parts`. Nothing on
// the PICTURE ever changed. On an imported production line that is a claim about which of 15,711
// identical grey parts a click is going to be about, and the only thing backing it was the label.
//
// SO THE ASSERTION CANNOT BE A LABEL. Every claim here is a count of pixels that changed inside the
// viewport rectangle between two OS captures of the real composite:
//
//   1. a NEGATIVE CONTROL first — two captures with nothing hovered differ by ~0 sampled pixels, so the
//      threshold is not reading its own noise, and every number below is a change this cue caused;
//   2. pointing at a part changes the picture, and the change is BLUER than it is red (the hover accent
//      is cyan, the far side of the wheel from the selection yellow — two brightnesses of one hue is a
//      reader guessing which is which);
//   3. THE LADDER, in pixels: hovering the rung that names the ASSEMBLY changes strictly MORE of the
//      picture than the rung that says `1 part`. That count was an abstraction until now — this is the
//      assertion that it describes something you can see;
//   4. and Escape puts the picture back to the one the aim started on, which is what makes it a cue and
//      not an edit.
//
// The diff is deliberately content-agnostic: it does not know what colour a hover is, so it cannot be
// satisfied by a tint chosen to match the assertion. The direction is reported separately.
//
// Empty scene (MTK_SCENE_N=0) so only what the test builds is under the cursor.

import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-stagehover");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const clientRect = path.resolve(dir, "../scripts/window-client-rect.ps1");
const diffRegion = path.resolve(dir, "../scripts/diff-region.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

/** Every measured number, to stdout AND to a file beside the captures — the reporter swallows a spec's
 *  own `console.log` at this level, and numbers in a scrolled terminal proved nothing. */
function note(line) {
  console.log(line);
  appendFileSync(path.join(shots, "measurements.txt"), `${line}\n`);
}

const ps = (script, args) =>
  execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], {
    stdio: "pipe",
  })
    .toString()
    .trim();

/** An OS capture of the real composite — the viewport is a transparent WebView2 over native wgpu, so a
 *  WebDriver screenshot is the React panels and a black hole where the 3D is. */
async function shot(label) {
  await browser.pause(700);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      ps(script, args);
    } catch {
      /* fall through to the other path */
    }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  let ok = false;
  for (let round = 0; round < 3 && !ok; round += 1) {
    if (round > 0) await browser.pause(1000);
    ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) note(`[hover] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
  else note(`[hover] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return ok ? out : null;
}

/** The part of the CAPTURE that is 3D and nothing else.
 *
 *  A capture of this app is the whole WINDOW. Its docks, its badge, its toasts and its stats read-out
 *  all change for reasons that have nothing to do with what the cursor is over, and a diff measured
 *  over them would report the badge repainting as "the stage lit up".
 *
 *  THE OVERLAYS ARE WHAT BITE, and both of them did, on the first run of the assertion they broke.
 *  `SubjectAimBadge` is a pale pill positioned INSIDE `#viewport` (it belongs to the stage — that is
 *  the whole point of it), and a band taken as a fixed fraction of the viewport clipped its top eleven
 *  pixels: "Escape puts the picture back" then measured the badge going away — 613 changed pixels at
 *  mean deltas R-62 G-69 B-72, a uniform DARKENING, which is a pale pill being removed and is not any
 *  hover. The Frames picker's popover opens UPWARD over the stage and did the same to the last case,
 *  inflating 379 sampled pixels to 1008 and pulling the mean toward white.
 *
 *  So the bottom edge is the highest overlay currently over the stage, measured off the DOM. Same
 *  class of mistake as the film gate's interdecile luma pinned near 233 by the white docks
 *  (`window-scope-image-metrics`): the metric was right and the RECTANGLE was wrong. */
async function stageRegion() {
  const geometry = JSON.parse(ps(clientRect, []));
  const box = await browser.execute(() => {
    const el = document.querySelector('[data-testid="viewport"]');
    if (!el) return null;
    const r = el.getBoundingClientRect();
    // Every overlay that can sit over the stage. The badge is inside the viewport; the picker's
    // popover is portalled to the body and opens upward when the dock leaves it no room below.
    const overlays = ['[data-testid="subjectAimBadge"]', '[data-testid="cutscene-subject-picker"]']
      .map((sel) => document.querySelector(sel))
      .filter(Boolean)
      .map((el) => el.getBoundingClientRect())
      .filter((box) => box.bottom > r.top && box.top < r.bottom && box.right > r.left && box.left < r.right)
      .map((box) => box.top);
    return {
      left: r.left,
      top: r.top,
      width: r.width,
      height: r.height,
      overlayTop: overlays.length ? Math.min(...overlays) : null,
      dpr: window.devicePixelRatio || 1,
    };
  });
  if (!box) return null;
  // The margins hold the view/projection pills at the top, the tool rail at the left and the stats
  // read-out; the bottom is the badge, or a generous fraction when there is no badge to measure.
  const left = box.left + box.width * 0.12;
  const right = box.left + box.width * 0.9;
  const top = box.top + box.height * 0.18;
  const bottom = (box.overlayTop ?? box.top + box.height * 0.82) - 10;
  if (bottom <= top || right <= left) return null;
  // The capture is framed by the WINDOW rect; the DOM is measured from the CLIENT origin. The offset
  // between them is the title bar and border.
  const offsetX = geometry.x - geometry.windowX;
  const offsetY = geometry.y - geometry.windowY;
  return {
    x: Math.round(offsetX + left * box.dpr),
    y: Math.round(offsetY + top * box.dpr),
    width: Math.round((right - left) * box.dpr),
    height: Math.round((bottom - top) * box.dpr),
    // The same band in CLIENT coordinates, so the point this test hovers is inside the rectangle it
    // then measures. Hovering something the diff cannot see would make every number below a zero.
    client: { left, top, width: right - left, height: bottom - top },
  };
}

/** Sampled pixels that changed inside `region`, and which way. */
function diff(before, after, region, label) {
  const out = JSON.parse(
    ps(diffRegion, [
      "-Before",
      before,
      "-After",
      after,
      "-X",
      String(region.x),
      "-Y",
      String(region.y),
      "-Width",
      String(region.width),
      "-Height",
      String(region.height),
    ]),
  );
  note(
    `[hover] ${label}: ${out.changed} of ${out.sampled} sampled pixels changed (${out.percent}%) ` +
      `over ${out.region}, mean delta R${out.meanDeltaR} G${out.meanDeltaG} B${out.meanDeltaB}`,
  );
  return out;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

const clickTabNamed = (containerSel, word) =>
  browser.execute(
    (sel, name) => {
      const root = document.querySelector(sel);
      if (!root) return false;
      const tab = [...root.querySelectorAll('[role="tab"]')].find((t) => t.textContent?.trim() === name);
      if (!tab) return false;
      tab.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return true;
    },
    containerSel,
    word,
  );

const selectRow = (id) =>
  browser.execute((key) => {
    const row = document.querySelector(`[data-testid="hrow"][data-id="${key}"]`);
    if (!row) return false;
    row.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, id);

const present = (selector) => browser.execute((sel) => !!document.querySelector(sel), selector);

/** Every rung the badge is offering, as the user reads it. */
const aimRungs = () =>
  browser.execute(() =>
    [...document.querySelectorAll('[data-testid^="subjectAimRung-"]')].map((el) => ({
      id: (el.getAttribute("data-testid") ?? "").replace("subjectAimRung-", ""),
      parts: Number(el.getAttribute("data-parts")),
      text: (el.textContent ?? "").replace(/\s+/g, " ").trim(),
    })),
  );

/** The RENDERER's selection — the number that says whether a hover moved it. */
const nativeSelection = () => invoke("gizmo_selected");

/** One pointer gesture on the stage surface itself, at real client coordinates. */
const stageEvent = (type, clientX, clientY) =>
  browser.execute(
    (t, x, y) => {
      const el = document.querySelector('[data-testid="viewport"]');
      if (!el) return false;
      const Ctor = t === "click" ? MouseEvent : window.PointerEvent || MouseEvent;
      el.dispatchEvent(new Ctor(t, { bubbles: true, cancelable: true, clientX: x, clientY: y }));
      return true;
    },
    type,
    clientX,
    clientY,
  );

/** Point at a badge rung the way a pointer does.
 *
 *  BOTH SPELLINGS, because React synthesises `onPointerEnter`/`onPointerLeave` from `pointerover` /
 *  `pointerout` at the root container — a bare `pointerenter` reaches a real DOM listener and can miss
 *  the React one entirely, which would make this spec pass or fail on a React implementation detail
 *  rather than on the feature. */
const rungPointer = (id, entering) =>
  browser.execute(
    (key, isEnter) => {
      const el = document.querySelector(`[data-testid="subjectAimRung-${key}"]`);
      if (!el) return false;
      const Ctor = window.PointerEvent || MouseEvent;
      const over = isEnter ? "pointerover" : "pointerout";
      const direct = isEnter ? "pointerenter" : "pointerleave";
      el.dispatchEvent(new Ctor(over, { bubbles: true, cancelable: true, relatedTarget: null }));
      el.dispatchEvent(new Ctor(direct, { bubbles: false, cancelable: true, relatedTarget: null }));
      return true;
    },
    id,
    entering,
  );

/** Walk a coarse grid over the MEASURED band and ask the engine what is behind each point.
 *
 *  Setup, not the thing under test: `viewport_peek` is a read that changes nothing, which is exactly
 *  why it can be used to look for a pixel with something behind it. Constrained to the band the diff
 *  measures — a point outside it would light something the measurement cannot see. */
const findStagePoint = async (band, accept) => {
  const win = await browser.execute(() => ({ w: window.innerWidth, h: window.innerHeight }));
  for (const fy of [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8]) {
    for (const fx of [0.5, 0.42, 0.58, 0.34, 0.66, 0.26, 0.74, 0.16, 0.84]) {
      const clientX = Math.round(band.left + band.width * fx);
      const clientY = Math.round(band.top + band.height * fy);
      const id = await invoke("viewport_peek", { x: clientX / win.w, y: clientY / win.h });
      if (id && (!accept || accept(id))) return { clientX, clientY, id };
    }
  }
  return null;
};

describe("The stage answers what the cursor is over", () => {
  let hall; // the assembly — six parts under one group node
  let part; // the object the cutscene hangs on
  let boxes = [];
  let hallName;
  let region;
  let point;
  let quiet; // the picture with the aim in flight and nothing hovered
  let selectionBefore;

  before(async () => {
    await browser.waitUntil(async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)), {
      timeout: 30000,
    });
    await click('[data-testid="stop"]');
    await invoke("new_project");
    await browser.pause(700);
    for (let round = 0; round < 6; round += 1) {
      const gone = await browser.execute(() => {
        const skip = document.querySelector('[data-testid="onboardSkip"]');
        if (skip) skip.dispatchEvent(new MouseEvent("click", { bubbles: true }));
        return !document.querySelector('[data-testid="onboardSkip"]');
      });
      if (gone) break;
      await browser.pause(500);
    }
  });

  it("builds an assembly and starts aiming a shot at it", async () => {
    const parts = [];
    part = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.4, 0] })).created;
    parts.push(part);
    boxes = [];
    for (const p of [
      [10, 0.5, 0],
      [-10, 0.5, 0],
      [6, 0.5, 5],
      [-6, 0.5, -5],
      [0, 0.5, 9],
    ]) {
      const id = (await invoke("shape_spawn", { kind: "box", pos: p })).created;
      boxes.push(id);
      parts.push(id);
    }
    hall = await invoke("group_entities", { ids: parts, name: "Assembly Hall" });
    const named = await invoke("cinema_subject_catalog", { id: part, index: null, query: "" });
    hallName = named.candidates.find((c) => c.id === hall)?.name ?? "Assembly Hall";
    note(`[hover] "${hallName}" holds ${parts.length} drawn parts`);

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(part)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="shot-establish"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: part })).shots === 1, {
      timeout: 15000,
      timeoutMsg: "the shot never landed",
    });

    if (!(await present("#bottom-workspaces-animation-tab"))) {
      expect(await click('[data-testid="bottom-dock-toggle"]')).toBe(true);
      await browser.pause(500);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });
    expect(await click('[data-testid="cutscene-clip"]')).toBe(true);
    await (await $('[data-testid="cutscene-shot-editor"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-subject"]')).toBe(true);
    await (await $('[data-testid="cutscene-subject-picker"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-subject-aim"]')).toBe(true);
    await (await $('[data-testid="subjectAimBadge"]')).waitForExist({ timeout: 10000 });

    await invoke("frame_all");
    await browser.pause(1200);
    selectionBefore = await nativeSelection();
    note(`[hover] the renderer's selection before any of this: ${JSON.stringify(selectionBefore)}`);
  });

  it("THE NEGATIVE CONTROL: two captures of the same untouched picture differ by nothing", async () => {
    region = await stageRegion();
    expect(region).toBeTruthy();
    note(`[hover] measuring the capture at ${region.x},${region.y} ${region.width}x${region.height}`);

    quiet = await shot("00_nothing_hovered");
    const again = await shot("01_still_nothing_hovered");
    expect(quiet).toBeTruthy();
    expect(again).toBeTruthy();

    // Without this, every number below could be the capture path's own noise, the grid shimmering, or
    // a repaint — and the whole spec would be measuring the instrument.
    const control = diff(quiet, again, region, "control (no hover -> no hover)");
    expect(control.changed).toBeLessThan(Math.max(20, control.sampled * 0.001));
  });

  it("pointing at a part changes the picture, and the change is cyan", async () => {
    point = await findStagePoint(region.client, (id) => id === part || boxes.includes(id));
    note(`[hover] the stage has ${point ? `"${point.id}"` : "nothing"} at ${point?.clientX},${point?.clientY}`);
    expect(point).toBeTruthy();

    expect(await stageEvent("pointermove", point.clientX, point.clientY)).toBe(true);
    await browser.waitUntil(async () => (await aimRungs()).length > 0, {
      timeout: 15000,
      timeoutMsg: "hovering the stage never named anything",
    });
    const rungs = await aimRungs();
    for (const rung of rungs) note(`[hover]   rung ${rung.id} — ${rung.text}`);
    expect(rungs[0].id).toBe(point.id);

    const lit = await shot("02_the_part_under_the_cursor");
    expect(lit).toBeTruthy();
    const leaf = diff(quiet, lit, region, "the leaf under the cursor");
    // 1. THE PICTURE CHANGED. Before this, the badge named the object and the stage did not move.
    expect(leaf.changed).toBeGreaterThan(50);
    // 2. AND IT CHANGED THE RIGHT WAY. The hover accent is cyan; the selection accent is yellow. Two
    //    brightnesses of one hue would be a reader guessing which of the two they are looking at.
    expect(leaf.meanDeltaB).toBeGreaterThan(leaf.meanDeltaR);
    expect(leaf.meanDeltaB).toBeGreaterThan(0);
    globalThis.__leafChanged = leaf.changed;
  });

  it("THE LADDER, IN PIXELS: the rung that names the assembly lights the assembly", async () => {
    const rungs = await aimRungs();
    const hallRung = rungs.find((r) => r.id === hall);
    expect(hallRung).toBeTruthy();
    note(`[hover] the rungs claim ${rungs[0].parts} part under the cursor and ${hallRung.parts} in "${hallName}"`);
    expect(hallRung.parts).toBeGreaterThan(rungs[0].parts);

    expect(await rungPointer(hall, true)).toBe(true);
    await browser.pause(900);
    const wide = await shot("03_the_assembly_the_rung_names");
    expect(wide).toBeTruthy();
    const assembly = diff(quiet, wide, region, "the whole assembly the rung names");

    // THE ASSERTION THIS SPEC EXISTS FOR. The assembly rung's part count was a number the author had to
    // take on trust — the picture was identical whether it said 7 or 700. It now lights strictly more of
    // the stage than the one part beside it, which is the difference between a label and a preview.
    note(`[hover] leaf lit ${globalThis.__leafChanged} sampled pixels; the assembly lit ${assembly.changed}`);
    expect(assembly.changed).toBeGreaterThan(globalThis.__leafChanged);
    expect(assembly.meanDeltaB).toBeGreaterThan(assembly.meanDeltaR);

    // AND IT IS STILL A HOVER. The whole reason the mode reads through a peek is that a pick here would
    // switch which cutscene is on screen and discard the shot being aimed; a highlight that moved the
    // selection would have reintroduced exactly that.
    const now = await nativeSelection();
    note(`[hover] the renderer's selection after lighting the whole assembly: ${JSON.stringify(now)}`);
    expect(now).toEqual(selectionBefore);
  });

  it("and Escape puts the picture back to the one the aim started on", async () => {
    await browser.execute(() => window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })));
    await browser.waitUntil(async () => !(await present('[data-testid="subjectAimBadge"]')), {
      timeout: 10000,
      timeoutMsg: "the badge never went away",
    });
    const after = await shot("04_the_cue_goes_out_with_the_mode");
    expect(after).toBeTruthy();

    // A cue that outlived its mode would be an edit. The measurement that says it did not is the same
    // one that said it arrived — compared against the picture from before anything was hovered.
    const back = diff(quiet, after, region, "back to the untouched picture");
    expect(back.changed).toBeLessThan(Math.max(20, back.sampled * 0.001));
  });

  it("and the picker's own rows point at the stage too — the third surface, same cue", async () => {
    // THE SURFACE THIS ONE IS FOR: the list is how you reach something you cannot see, and on a
    // 15,711-part import its rows repeat each other's names. "Which of these is the one by the door"
    // is a question about the picture, and until the row could light the stage there was no way to
    // ask it without committing the shot to a guess first.
    expect(await click('[data-testid="cutscene-subject"]')).toBe(true);
    await (await $('[data-testid="cutscene-subject-picker"]')).waitForExist({ timeout: 10000 });
    const row = `[data-testid="cutscene-subject-option-${hall}"]`;
    await (await $(row)).waitForExist({ timeout: 10000 });
    // RE-MEASURED with the popover on screen. It opens upward over the stage, so the band from the
    // earlier cases would have been measuring the popover's own white surface arriving.
    const above = await stageRegion();
    expect(above).toBeTruthy();
    note(`[hover] re-measuring above the open picker at ${above.x},${above.y} ${above.width}x${above.height}`);

    expect(
      await browser.execute((sel) => {
        const el = document.querySelector(sel);
        if (!el) return false;
        const Ctor = window.PointerEvent || MouseEvent;
        el.dispatchEvent(new Ctor("pointerover", { bubbles: true, cancelable: true, relatedTarget: null }));
        el.dispatchEvent(new Ctor("pointerenter", { bubbles: false, cancelable: true, relatedTarget: null }));
        return true;
      }, row),
    ).toBe(true);
    // Longer than the row's own 90 ms settle: a sweep down a list is one gesture, not one question
    // per row, and the delay is what stops twelve walks of every drawn instance.
    await browser.pause(1200);

    const listed = await shot("05_a_picker_row_points_at_the_stage");
    expect(listed).toBeTruthy();
    const fromList = diff(quiet, listed, above, "the assembly, lit from a list row");
    expect(fromList.changed).toBeGreaterThan(50);
    expect(fromList.meanDeltaB).toBeGreaterThan(fromList.meanDeltaR);

    // Still a hover, from here too.
    expect(await nativeSelection()).toEqual(selectionBefore);
  });
});
