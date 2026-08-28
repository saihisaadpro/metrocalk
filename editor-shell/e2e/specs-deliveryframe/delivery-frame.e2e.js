// THE FRAME THE CAMERA IS COMPOSED FOR — on the PACKAGED .exe, driven through the UI.
//
// BEFORE: the wgpu surface is the whole window and the editor is composited over it, so the picture is
// a sub-rectangle. The renderer answered that by sliding the ORBIT TARGET sideways at framing time, by
// the hole's offset — an offset proportional to `distance`, expressed in the camera's own right/up
// basis. It was correct for exactly one frame. Opening a dock, orbiting or zooming each left a target
// solved for a rectangle that no longer existed, and the subject the author had just framed walked out
// of the picture with nobody touching the framing. ADR-162 measured it and tracked it: a wide shot
// previewed with the bottom dock open is right in every number and not on screen — and the control that
// starts a preview LIVES in the dock whose opening shortens the stage.
//
// AFTER: the visible rectangle shears the PROJECTION instead. Nothing is applied once, so nothing goes
// stale. And once the frame is a property of the projection rather than of the camera, a cutscene can
// be composed for a frame that is not the author's stage at all: a DELIVERY frame, with the bars drawn
// around it by the renderer that composed for it.
//
// EVERY CLAIM IS A NUMBER OFF THE LIVE ENGINE OR A PIXEL OFF THE REAL COMPOSITE. `camera_probe` now
// reports both rectangles — the one composed for and the one visible — so "the framing followed the
// dock" and "the bars are real" are arithmetic. `viewport_pick` casts the SAME ray a user's click makes
// through the SAME projection, so "the subject is still in the picture" is answered by the renderer
// rather than by a screenshot that looks plausible.

import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-deliveryframe");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
const clientRect = path.resolve(dir, "../scripts/window-client-rect.ps1");
const probe = path.resolve(dir, "../scripts/probe-pixels.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

/** Every measured number, to stdout AND to a file beside the captures.
 *
 *  The reporter swallows a spec's own `console.log` at this log level, and a run whose numbers exist
 *  only in a terminal that has scrolled is a run that proved nothing anybody can read back. */
function note(line) {
  console.log(line);
  appendFileSync(path.join(shots, "measurements.txt"), `${line}
`);
}

const ps = (script, args) =>
  execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], {
    stdio: "pipe",
  })
    .toString()
    .trim();

/** An OS capture of the real composite. The viewport is a transparent WebView2 over the native wgpu
 *  surface, so a WebDriver screenshot is the React panels and a black hole where the 3D is. */
async function shot(label) {
  await browser.pause(500);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      ps(script, args);
    } catch { /* fall through */ }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  let ok = false;
  for (let round = 0; round < 3 && !ok; round += 1) {
    if (round > 0) await browser.pause(1000);
    ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) note(`[frame] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
  else note(`[frame] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return ok ? out : null;
}

/** Where the SURFACE sits inside a `PrintWindow` capture. The capture is the whole window (border and
 *  all); the wgpu surface is the client area. A fraction measured on one is not a pixel on the other. */
function surfaceInCapture() {
  const geo = JSON.parse(ps(clientRect, []));
  return {
    x: geo.x - geo.windowX,
    y: geo.y - geo.windowY,
    width: geo.width,
    height: geo.height,
    occluded: geo.occluded,
  };
}

/** The mean colour of a patch, addressed in SURFACE fractions. */
function patch(file, geo, fx, fy, radius = 6) {
  const x = Math.round(geo.x + fx * geo.width);
  const y = Math.round(geo.y + fy * geo.height);
  const p = JSON.parse(ps(probe, ["-Path", file, "-X", String(x), "-Y", String(y), "-Radius", String(radius)]));
  return { ...p, luma: 0.2126 * p.r + 0.7152 * p.g + 0.0722 * p.b, x, y };
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

/** Pick a value on a <select> the way the DOM does, so React's onChange runs. */
const chooseOption = (selector, value) =>
  browser.execute(
    (sel, v) => {
      const el = document.querySelector(sel);
      if (!el) return false;
      const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set;
      setter?.call(el, v);
      el.dispatchEvent(new Event("change", { bubbles: true }));
      // A CONTROLLED SELECT SNAPS BACK. React re-renders on `change` with `value` still the old
      // one, so reading `el.value` here answers "has the reply landed yet", not "was the choice
      // made". The outcome is asserted against the ENGINE by the caller.
      return true;
    },
    selector,
    value,
  );

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const cam = () => invoke("camera_probe");

/** A probe taken once the LAYOUT HAS STOPPED MOVING.
 *
 *  The docks animate, and the cutscene panel grows a pose read-out when a preview starts — so the
 *  stage's height is still changing for a few hundred milliseconds after the click that changed it.
 *  A rectangle read mid-animation is a rectangle the engine has already replaced, and a fraction
 *  computed from it aims a hair away from where the picture is. Two equal readings, then measure. */
async function settled() {
  let last = null;
  for (let round = 0; round < 40; round += 1) {
    const probe = await cam();
    const key = probe.visibleRect.join(",") + "|" + probe.frame.join(",");
    if (key === last) return probe;
    last = key;
    await browser.pause(250);
  }
  throw new Error("the stage never stopped moving");
}
const dist3 = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const fmt = (v) => `[${v.map((n) => n.toFixed(3)).join(", ")}]`;
/** The centre of a `[x, y, w, h]` rectangle, in the same fractions. */
const centreOf = (r) => [r[0] + r[2] / 2, r[1] + r[3] / 2];

describe("The frame the camera is composed for", () => {
  let subject;
  let tallStage;
  let framedEye;
  let framedHit;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await invoke("new_project");
    await browser.pause(700);
    // The first-run card mounts after the project does, and it sits along the BOTTOM of the stage —
    // exactly where a letterbox bar is. One click before it exists dismisses nothing.
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

  it("frames a wide set against the FULL-height stage", async () => {
    // Wide and low, deliberately: a scene whose bounds are much wider than they are tall is the one a
    // short stage crops, and it is what every real assembly looks like from the front.
    subject = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.4, 0] })).created;
    for (const p of [[9, 0.5, 0], [-9, 0.5, 0], [5, 0.5, 4], [-5, 0.5, -4]]) {
      await invoke("shape_spawn", { kind: "box", pos: p });
    }
    // The bottom dock must be SHUT for the premise: this is the tall stage.
    if (await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab"))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(500);
    }
    await invoke("frame_all");
    await browser.pause(500);

    const probed = await settled();
    tallStage = probed.visibleRect;
    framedEye = probed.eye;
    note(`[frame] tall stage visible=${fmt(tallStage)} composed=${fmt(probed.frame)}`);
    // With no cutscene delivering a frame of its own, the two rectangles ARE the same rectangle.
    expect(dist3([...probed.frame.slice(0, 3)], [...tallStage.slice(0, 3)])).toBeLessThan(1e-4);
    expect(tallStage[3]).toBeGreaterThan(0.5); // the stage really is most of the window

    // The renderer's own answer to "is the framed scene in the picture": a ray through the middle of
    // the composed rectangle, cast by the same projection that drew the frame.
    const [cx, cy] = centreOf(probed.frame);
    framedHit = await invoke("viewport_pick", { x: cx, y: cy });
    note(`[frame] a click at the centre of the composed frame hits ${framedHit}`);
    expect(framedHit).toBeTruthy();
    await shot("00_framed_tall_stage");
  });

  it("THE GATE: opening the bottom dock keeps the framed subject in the picture", async () => {
    // The exact gesture ADR-162 tracked as unclosed. Nothing else happens: no re-frame, no camera
    // command, no click in the viewport.
    expect(await click('[data-testid="bottom-dock-toggle"]')).toBe(true);
    await browser.waitUntil(async () => (await cam()).visibleRect[3] < tallStage[3] * 0.8, {
      timeout: 15000,
      timeoutMsg: "the shell never reported a shorter stage",
    });
    const probed = await settled();
    const shortStage = probed.visibleRect;
    note(
      `[frame] short stage visible=${fmt(shortStage)} — height ${(tallStage[3] * 100).toFixed(1)}%` +
        ` -> ${(shortStage[3] * 100).toFixed(1)}% of the window`,
    );

    // 1. NOTHING RE-FRAMED. The camera is where the author left it, to the last decimal.
    expect(dist3(probed.eye, framedEye)).toBeLessThan(0.01);
    // 2. The composed rectangle followed the stage.
    expect(Math.abs(probed.frame[3] - shortStage[3])).toBeLessThan(1e-4);
    // 3. And the framed scene is STILL what a click in the middle of that stage hits — through the
    //    same projection, live. This is the assertion the tracked defect would fail.
    const [cx, cy] = centreOf(probed.frame);
    const hit = await invoke("viewport_pick", { x: cx, y: cy });
    note(`[frame] with the dock open, the centre of the stage hits ${hit} (was ${framedHit})`);
    expect(hit).toBe(framedHit);
    await shot("01_dock_open_still_framed");
  });

  it("authors a cut on the object and delivers it in scope", async () => {
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    for (const card of ["establish", "hero", "closeup"]) {
      expect(await click(`[data-testid="shot-${card}"]`)).toBe(true);
      await browser.pause(500);
    }
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 3, {
      timeout: 15000,
      timeoutMsg: "the three shots never landed",
    });

    // The engine rail can close the dock under us; re-open it the way a user would rather than
    // assuming the previous test left it open.
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(500);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    // BEFORE: the only frame available was the author's stage.
    expect((await invoke("cinema_list", { id: subject })).delivery).toBe("viewport");
    // The picker offers what the ENGINE publishes; a word invented here would be refused.
    const offered = await browser.execute(
      () => [...document.querySelectorAll('[data-testid="cutscene-delivery"] option')].map((o) => o.value),
    );
    note(`[frame] the picker offers ${offered.join(", ")}`);
    expect(offered).toContain("scope");

    expect(await chooseOption('[data-testid="cutscene-delivery"]', "scope")).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).delivery === "scope", {
      timeout: 15000,
      timeoutMsg: "the delivery frame never landed on the document",
    });
  });

  it("the preview composes for 2.39:1 and the renderer draws its bars", async () => {
    expect(await click('[data-testid="cutscene-preview"]')).toBe(true);
    await browser.waitUntil(async () => (await cam()).cinematic === true, {
      timeout: 15000,
      timeoutMsg: "the preview never took the camera",
    });
    const probed = await settled();
    const geo = surfaceInCapture();
    note(
      `[frame] composed=${fmt(probed.frame)} visible=${fmt(probed.visibleRect)}` +
        ` surface=${geo.width}x${geo.height}`,
    );

    // 1. The composed rectangle is the delivery frame, measured as an aspect ratio in real pixels.
    const composedAspect = (probed.frame[2] * geo.width) / (probed.frame[3] * geo.height);
    note(`[frame] the composed frame is ${composedAspect.toFixed(3)}:1`);
    expect(Math.abs(composedAspect - 2.39)).toBeLessThan(0.05);
    // 2. It is INSIDE the stage and CENTRED in it, which is what makes the difference two bars rather
    //    than a crop. Both edges, in one arithmetic statement.
    expect(probed.frame[3]).toBeLessThan(probed.visibleRect[3]);
    const barTop = probed.frame[1] - probed.visibleRect[1];
    const barBottom =
      probed.visibleRect[1] + probed.visibleRect[3] - (probed.frame[1] + probed.frame[3]);
    note(`[frame] bars: top ${(barTop * 800).toFixed(1)}px, bottom ${(barBottom * 800).toFixed(1)}px`);
    expect(barTop).toBeGreaterThan(0.005);
    expect(Math.abs(barTop - barBottom)).toBeLessThan(0.002);
    // 3. The panel says which frame the numbers were solved for.
    const pose = await text('[data-testid="cutscene-preview-pose"]');
    note(`[frame] the panel reads: ${pose?.replace(/\s+/g, " ").trim()}`);
    expect(pose).toMatch(/composed for/i);
    expect(pose).toMatch(/scope/i);

    // 4. AND THE PIXELS. A bar between the top of the stage and the top of the composed frame, against
    //    the middle of the picture. Measured on the real composite, not asserted from the numbers that
    //    produced it.
    const file = await shot("02_scope_preview");
    if (!file) {
      console.log("[frame] no capture — skipping the pixel half of this assertion");
      return;
    }
    // ONE COLUMN, TWO SAMPLES, and the column is deliberately NOT the middle one: the PREVIEW badge
    // is an overlay across the top-centre of the stage, so a sample there measures a white chip and
    // reports that the bars are missing. Found by READING the capture — the assertion said luma 222
    // where the picture plainly showed black bars.
    const columnX = probed.frame[0] + probed.frame[2] * 0.12;
    const barY = (probed.visibleRect[1] + probed.frame[1]) / 2;
    const bar = patch(file, geo, columnX, barY);
    const picture = patch(file, geo, columnX, probed.frame[1] + probed.frame[3] / 2, 16);
    note(
      `[frame] bar at (${bar.x}, ${bar.y}) luma ${bar.luma.toFixed(1)} spread ${bar.spread};` +
        ` picture at (${picture.x}, ${picture.y}) luma ${picture.luma.toFixed(1)}`,
    );
    expect(bar.luma).toBeLessThan(24);
    expect(bar.spread).toBeLessThan(12); // a FLAT black, not a dark part of the scene
    expect(picture.luma).toBeGreaterThan(bar.luma + 30);
  });

  it("hands the camera back, bars and all", async () => {
    expect(await click('[data-testid="cutscene-preview"]')).toBe(true);
    await browser.waitUntil(async () => (await cam()).cinematic === false, {
      timeout: 15000,
      timeoutMsg: "the preview never let go",
    });
    const probed = await settled();
    // The delivery frame belongs to the shot. The author's own viewport is not letterboxed.
    expect(Math.abs(probed.frame[3] - probed.visibleRect[3])).toBeLessThan(1e-4);
    expect(dist3(probed.eye, framedEye)).toBeLessThan(0.01);
    await shot("03_camera_handed_back");
  });

  it("the delivery frame survives Save, New and Open", async () => {
    const file = path.resolve(
      path.dirname(path.resolve(dir, "../../src-tauri/target/release/metrocalk-editor-shell.exe")),
      "delivery-frame.mtk",
    );
    await invoke("save_project", { path: file });
    await browser.waitUntil(async () => existsSync(file), { timeout: 20000, timeoutMsg: "no .mtk was written" });
    await invoke("new_project");
    await browser.pause(800);
    expect((await invoke("cinema_list", { id: subject })).shots).toBe(0);

    await invoke("open_project", { path: file });
    await browser.pause(1500);
    const reopened = await invoke("cinema_list", { id: subject });
    note(`[frame] after reopen: ${reopened.shots} shots, delivered in ${reopened.delivery}`);
    expect(reopened.shots).toBe(3);
    expect(reopened.delivery).toBe("scope");
  });
});
