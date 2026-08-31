// ADR-193 — the delivery frame while the AUTHOR is framing, on the packaged `.exe`.
//
// ADR-192 gave the editor "Shoot from this view": the shot films from exactly the pose the viewport
// was standing at. What it could not give was the FRAME. A cutscene is delivered in a shape of its
// own — 2.39:1 scope, 9:16 vertical — and the author's stage is whatever shape the docks have left
// it, so the picture they composed and the picture the engine delivers were two different pictures
// at the same eye. The stage insets to the delivery frame only while something HOLDS the camera, and
// while the author is flying it nothing does.
//
// THE ASSERTION THAT CANNOT BE PASSED BY DRAWING BARS IS THE RECTANGLE. Every step reads
// `camera_probe`, which reports the frame the projection was actually sheared to, and compares:
//
//   S — the stage's own shape, whatever the docks have left (`stageAspect`);
//   F — the shape the picture on screen is composed for (`frameAspect`);
//   D — the shape the CUTSCENE is delivered in, from its own `delivery`.
//
// The claim is: with the guide off, F == S and F != D — the author is composing for the wrong frame.
// With it on, F == D, and the rectangle the shot is delivered in during preview is the SAME
// rectangle, to four decimal places. A feature that painted two black bars over the viewport and
// left the projection alone would pass a screenshot and fail every line below.

import { execFileSync } from "node:child_process";
import { appendFileSync, mkdirSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
/** Where the real frames land. Not a DOM screenshot — `viewport_capture` asks the RENDER thread for
 *  its next frame and crops it to the composed rectangle, so the FILE's own shape is the evidence. */
const evidence = path.resolve(dir, "../.shots-frameguide");
mkdirSync(evidence, { recursive: true });

/** Capture the stage, and report the file's pixel dimensions — the shape of what was composed. */
const captureFrame = async (name) => {
  const file = path.join(evidence, name);
  const reply = await invoke("viewport_capture", { path: file });
  expect(reply.reason).toBeFalsy();
  expect(statSync(file).size).toBeGreaterThan(2000);
  return reply;
};

/** THE WHOLE COMPOSITE, from the operating system - the only instrument that can see a frame GUIDE.
 *
 *  `viewport_capture` crops to the composed rectangle, which is exactly the region a guide leaves
 *  untouched, so the dim and the hairline are outside every frame it can return. That is a property
 *  worth having - a still is bit-identical with the guide on - and it means the guide's own pixels
 *  need the window.
 *
 *  `scripts/capture-composited-window.ps1`, not the `.uxtest` foreground helper: it uses
 *  `PrintWindow` with `PW_RENDERFULLCONTENT`, so it reads THIS window's presentation rather than
 *  whatever holds the foreground - which on an unattended box has silently produced five PNGs of
 *  somebody else's application while every functional assertion stayed green. It verifies its own
 *  output too (the dimensions match the window, and a uniform frame is rejected), and this reads the
 *  numbers back out so they land in the run's log rather than only in a file nobody opens.
 */
const captureWindow = (name) => {
  const file = path.join(evidence, name);
  const script = path.resolve(dir, "../scripts/capture-composited-window.ps1");
  const said = execFileSync(
    "powershell.exe",
    ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, "-Out", file, "-PreserveWindow"],
    { encoding: "utf8" },
  ).trim();
  const size = /CAPTURED (\d+)x(\d+)/.exec(said);
  expect(size).toBeTruthy();
  expect(Number(size[1])).toBeGreaterThan(900);
  expect(Number(size[2])).toBeGreaterThan(600);
  expect(statSync(file).size).toBeGreaterThan(20000);
  return { said };
};

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

const setSelect = (selector, value) =>
  browser.execute(
    (sel, v) => {
      const el = document.querySelector(sel);
      if (!el) return false;
      const setter = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value")?.set;
      setter?.call(el, v);
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    },
    selector,
    value,
  );

const attr = (selector, name) =>
  browser.execute((sel, a) => document.querySelector(sel)?.getAttribute(a) ?? null, selector, name);

const exists = (selector) => browser.execute((sel) => !!document.querySelector(sel), selector);

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const isDisabled = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    return el ? el.disabled === true || el.getAttribute("aria-disabled") === "true" : null;
  }, selector);

const SCOPE = 2.39;
const VERTICAL = 9 / 16;
/** Every measurement this file takes, on stdout AND in a file beside the captures.
 *
 *  `wdio`'s spec reporter does not surface `console.log` at `logLevel: "error"`, so a run's numbers
 *  were visible only while it was passing - which is the wrong way round, since a failing run is
 *  exactly when the numbers are wanted. */
const log = path.join(evidence, "measurements.txt");
writeFileSync(log, "", "utf8");
const note = (line) => {
  console.log(line);
  appendFileSync(log, `${line}
`, "utf8");
};

describe("ADR-193 · the frame guide", () => {
  let subject;
  /** The stage's own shape, with the bottom dock open. Measured once; nothing below changes it. */
  let stage;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    // THE PREFERENCE PERSISTS BY DESIGN, and the WebView2 profile outlives this process - so a run
    // that ended with the guide hidden would decide whether the next one draws anything at all.
    // Cleared BEFORE the reload rather than in `onPrepare`, which can only reach files beside the
    // executable. (`<test_and_ci_discipline>` 4: a test resets the persisted state it touches.)
    await browser.execute(() => {
      try {
        window.localStorage.removeItem("mtk.frameGuide");
      } catch {
        /* a locked-down WebView keeps nothing to clear */
      }
    });
    await browser.refresh();
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await browser.pause(800);
    await click('[data-testid="stop"]');
    await browser.pause(500);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    await browser.pause(300);
  });

  it("composes for the STAGE while the author frames, which is the defect", async () => {
    subject = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.6, 0] })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [6, 0.2, -4] });
    await invoke("frame_all");
    expect(subject).toBeTruthy();

    // THE FIRST-RUN CARD ONLY APPEARS ONCE THERE IS A SCENE (`show={!sceneEmpty && …}`), so the Skip
    // in `before()` above ran against a card that had not been rendered yet. It is dismissed HERE,
    // after the spawn, because it is the other occupant of the bottom of the stage and the frame
    // guide's badge deliberately yields to it - leaving it up would make every badge assertion below
    // a test of the yield rather than of the guide.
    await browser.waitUntil(
      async () => {
        await browser.execute(() => {
          document
            .querySelector('[data-testid="onboardSkip"]')
            ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
        });
        return !(await exists('[data-testid="onboardSkip"]'));
      },
      { timeout: 15000, timeoutMsg: "the first-run card never went away" },
    );

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="shot-hero"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 1, {
      timeout: 15000,
      timeoutMsg: "the shot never landed",
    });

    // The stage reaches its final shape before the first measurement — opening a dock changes the
    // visible rectangle, and every number below is about a rectangle (`placed-camera.e2e.js` records
    // what comparing two different ones costs).
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    const probe = await invoke("camera_probe");
    stage = probe.stageAspect;
    note(`[stage]  the author's viewport is ${stage.toFixed(3)}:1`);
    // THE DEFECT, stated as a measurement: nothing is composing for a delivery frame, so the picture
    // is composed for the window.
    expect(probe.frameGuide).toBe(null);
    expect(Math.abs(probe.frameAspect - stage)).toBeLessThan(1e-4);
    expect(probe.frame).toEqual(probe.visibleRect);
    // AND IT IS NOWHERE NEAR THE FRAME THIS RUN ENDS ON. Measured rather than assumed: this window's
    // stage is about 2.3:1 with the dock open, which is *nearly* scope - so scope alone would be a
    // 4% demonstration and a reader could not tell it from noise. 9:16 on a landscape stage is the
    // case that removes two thirds of the width, and it is the one the last two tests use.
    expect(Math.abs(stage - VERTICAL)).toBeGreaterThan(0.3);
    expect(Math.abs(stage - SCOPE)).toBeGreaterThan(2e-3);
  });

  it("draws nothing for a cut delivered to the stage's own shape, and says why", async () => {
    // NEGATIVE CONTROL FOR THE CONTROL ITSELF. "Match viewport" is a real delivery frame in the
    // picker and it is the one with no bars, so an implementation that guided to everything would
    // letterbox the author's own viewport here — with the reason for it being a control they never
    // touched.
    expect(await isDisabled('[data-testid="cutscene-frame-guide"]')).toBe(true);
    expect(await attr('[data-testid="cutscene-frame-guide"]', "title")).toMatch(/already is the stage/i);
    expect(await exists('[data-testid="frameGuideBadge"]')).toBe(false);
    note("[viewport] the guide control is dead, and says so, on a cut with no delivery frame");
  });

  it("composes the STAGE for the delivery frame the moment one is chosen", async () => {
    // ── the author picks scope, through the panel's own control ──────────────────────────────────
    expect(await setSelect('[data-testid="cutscene-delivery"]', "scope")).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).delivery === "scope",
      { timeout: 15000, timeoutMsg: "the delivery frame never reached the document" },
    );
    await browser.waitUntil(async () => (await invoke("camera_probe")).frameGuide === "scope", {
      timeout: 10000,
      timeoutMsg: "the stage never started guiding",
    });

    const probe = await invoke("camera_probe");
    note(`[guided] the stage now composes for ${probe.frameAspect.toFixed(3)}:1, inside a ${probe.stageAspect.toFixed(3)}:1 viewport`);
    // THE CLAIM: the picture is composed for the DELIVERY, with nothing holding the camera.
    expect(Math.abs(probe.frameAspect - SCOPE)).toBeLessThan(2e-3);
    // ...and it is a real inset of the stage, not a relabelled whole window.
    const [fx, fy, fw, fh] = probe.frame;
    const [vx, vy, vw, vh] = probe.visibleRect;
    expect(Math.abs(fw - vw)).toBeLessThan(1e-4);
    expect(Math.abs(fx - vx)).toBeLessThan(1e-4);
    expect(fh).toBeLessThan(vh - 1e-3);
    expect(fy).toBeGreaterThan(vy + 1e-4);
    // Equal bars, top and bottom — the difference between a frame and a crop.
    expect(Math.abs(fy - vy - (vy + vh - fy - fh))).toBeLessThan(2e-3);
    // As a fraction of THE STAGE, not of the surface. Both rectangles are surface fractions, and a
    // bar quoted against the whole window is a number about the docks rather than about the frame.
    note(`[bars]   ${(((fy - vy) / vh) * 100).toFixed(2)}% above, ${(((vy + vh - fy - fh) / vh) * 100).toFixed(2)}% below, of the stage`);

    // THE GUIDE'S OWN PIXELS. The composite, from the operating system: the delivered picture bright
    // inside its rectangle, the world above and below it DIMMED rather than cut, a hairline on the
    // boundary, and the FRAME GUIDE badge on the stage. This is the capture that shows a guide is a
    // gate and not a crop — no rectangle `viewport_capture` can return contains any of it.
    const guided = captureWindow("3-the-stage-while-framing-scope.png");
    note(`[window] ${guided.said}`);

    // The way OUT is on the stage, where the bars are — not only in a dock the author may close.
    expect(await exists('[data-testid="frameGuideBadge"]')).toBe(true);
    expect(await text('[data-testid="frameGuideBadgeFrame"]')).toMatch(/2\.39/);
    expect(await attr('[data-testid="cutscene-frame-guide"]', "aria-pressed")).toBe("true");
  });

  it("delivers the SHOT into the rectangle the author composed it in", async () => {
    // THE WHOLE PASS, IN ONE COMPARISON. The rectangle the author is framing inside, and the
    // rectangle the cutscene runtime composes the shot for, must be the same rectangle — otherwise
    // "shoot from this view" films a frame the author never saw.
    const framing = await invoke("camera_probe");

    // The pose is taken from the viewport, through the panel's own button (ADR-192).
    expect(await click('[data-testid="cutscene-clip"]')).toBe(true);
    await (await $('[data-testid="cutscene-shoot-here"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-shoot-here"]')).toBe(true);
    await browser.waitUntil(
      async () => !!(await invoke("cinema_list", { id: subject })).rows[0].camera,
      { timeout: 15000, timeoutMsg: "the pose never reached the document" },
    );

    // THE PREVIEW IS ALREADY RUNNING: "Shoot from this view" starts one at the shot's own opening
    // instant (ADR-192), which is what puts the delivery-framed result on the stage AT the gesture.
    // Driven through the panel and not by an `invoke`, because the panel owns the preview STORE as
    // well as the camera — an `invoke` moves the camera and leaves the panel believing it is still
    // previewing, which is a state no click can produce and which then suppresses the guide for the
    // rest of the file. (This spec learned that the expensive way: three later tests failed on it.)
    await browser.waitUntil(async () => (await invoke("camera_probe")).cinematic === true, {
      timeout: 15000,
      timeoutMsg: "shooting from this view did not put the shot on the stage",
    });
    await browser.pause(400);
    const delivered = await invoke("camera_probe");
    expect(delivered.cinematic).toBe(true);
    note(`[framed] composed in ${framing.frameAspect.toFixed(4)}:1 · [delivered] filmed in ${delivered.frameAspect.toFixed(4)}:1`);
    expect(Math.abs(delivered.frameAspect - framing.frameAspect)).toBeLessThan(1e-3);
    // The same rectangle, not merely the same ratio: a shot composed for the right shape in the
    // wrong PLACE would still be a picture the author did not frame.
    for (let i = 0; i < 4; i += 1) {
      expect(Math.abs(delivered.frame[i] - framing.frame[i])).toBeLessThan(2e-3);
    }
    // And the file is that rectangle. `viewport_capture` crops to the composed frame, so the PNG's
    // own dimensions are the strongest form of this claim: the deliverable is scope.
    const shot = await captureFrame("2-delivered-in-scope.png");
    note(`[file]   the delivered frame is ${shot.width}x${shot.height} (${(shot.width / shot.height).toFixed(3)}:1)`);
    expect(Math.abs(shot.width / shot.height - SCOPE)).toBeLessThan(0.02);

    // ...and out through the panel's own toggle, so the store and the camera agree on the way back.
    expect(await click('[data-testid="cutscene-preview"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("camera_probe")).cinematic === false, {
      timeout: 15000,
      timeoutMsg: "the preview never handed the camera back",
    });
    await browser.waitUntil(async () => (await invoke("camera_probe")).frameGuide === "scope", {
      timeout: 10000,
      timeoutMsg: "the guide never came back after the preview",
    });
  });

  it("follows the delivery frame, because the frame is the document's and the guide is not", async () => {
    // The guide has no shape of its own. Change what the cut DELIVERS and the stage follows, which
    // is also why the guide survives save and open: what persists is the cutscene's `delivery`.
    expect(await setSelect('[data-testid="cutscene-delivery"]', "vertical")).toBe(true);
    await browser.waitUntil(async () => (await invoke("camera_probe")).frameGuide === "vertical", {
      timeout: 15000,
      timeoutMsg: "the guide never followed the delivery frame",
    });
    const probe = await invoke("camera_probe");
    note(`[follow] delivering 9:16 composes the stage for ${probe.frameAspect.toFixed(3)}:1`);
    expect(Math.abs(probe.frameAspect - VERTICAL)).toBeLessThan(2e-3);
    // Vertical PILLARBOXES on a landscape stage — the other axis, so the inset rule is exercised
    // both ways rather than once.
    const [fx, fy, fw, fh] = probe.frame;
    const [vx, vy, vw, vh] = probe.visibleRect;
    expect(Math.abs(fh - vh)).toBeLessThan(1e-4);
    expect(Math.abs(fy - vy)).toBeLessThan(1e-4);
    expect(fw).toBeLessThan(vw - 1e-3);
    expect(fx).toBeGreaterThan(vx + 1e-4);
    note(`[bars]   ${(((fx - vx) / vw) * 100).toFixed(2)}% left, ${(((vx + vw - fx - fw) / vw) * 100).toFixed(2)}% right, of the stage`);

    // THE CAPTURE THAT SHOWS A GUIDE AT ALL. Scope on this window's 2.2:1 stage is a 1.5% bar, which
    // is a correct measurement and an invisible picture; 9:16 on the same stage keeps a quarter of
    // the width, so this is the frame where a reader can SEE that what is outside the rectangle is
    // dimmed rather than cut, with a hairline on the boundary.
    note(`[window] ${captureWindow("5-the-stage-while-framing-vertical.png").said}`);
  });

  it("hands the whole stage back from the stage itself, in one click", async () => {
    // The control that turned the guide on lives in the bottom dock. If that were the only way out,
    // an author who closed the dock would be looking at a letterboxed viewport with nothing on
    // screen saying why — the failure this badge makes unreachable rather than merely unlikely.
    expect(await click('[data-testid="stageHideFrameGuide"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("camera_probe")).frameGuide === null, {
      timeout: 10000,
      timeoutMsg: "the stage never stopped guiding",
    });
    const probe = await invoke("camera_probe");
    note(`[hidden] the stage is back to its own ${probe.frameAspect.toFixed(3)}:1`);
    expect(Math.abs(probe.frameAspect - stage)).toBeLessThan(1e-4);
    expect(probe.frame).toEqual(probe.visibleRect);
    expect(await exists('[data-testid="frameGuideBadge"]')).toBe(false);
    expect(await attr('[data-testid="cutscene-frame-guide"]', "aria-pressed")).toBe("false");

    // THE NEGATIVE CONTROL FOR THE WHOLE FILE, and it is the defect this pass exists to close: with
    // the guide off, the shot is still delivered in 9:16 and the author is still composing in a
    // landscape window. Same eye, two different pictures.
    const cut = await invoke("cinema_list", { id: subject });
    expect(cut.delivery).toBe("vertical");
    // The before, from the same instrument as `3-`: the same camera on the same stage with nothing
    // dimmed, nothing framed and no badge — the state every shot in this engine was composed in
    // until this pass.
    note(`[window] ${captureWindow("4-the-stage-with-no-guide.png").said}`);
    const before = await captureFrame("1-composed-without-a-guide.png");
    note(`[unguided] the author is composing a ${(before.width / before.height).toFixed(3)}:1 picture of a 0.563:1 film`);
    expect(Math.abs(before.width / before.height - stage)).toBeLessThan(0.02);
    expect(Math.abs(before.width / before.height - VERTICAL)).toBeGreaterThan(0.3);
  });

  it("turns back on from the panel, and the two controls agree about which frame", async () => {
    expect(await click('[data-testid="cutscene-frame-guide"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("camera_probe")).frameGuide === "vertical", {
      timeout: 10000,
      timeoutMsg: "the guide never came back",
    });
    expect(await text('[data-testid="frameGuideBadgeFrame"]')).toMatch(/9:16/);
    // The badge and the picker cannot disagree: both name the cutscene's own `delivery`, and the
    // badge's words are the ENGINE's, carried out of the framing catalogue the picker is built from.
    const value = await browser.execute(
      () => document.querySelector('[data-testid="cutscene-delivery"]')?.value ?? null,
    );
    expect(value).toBe("vertical");
  });
});
