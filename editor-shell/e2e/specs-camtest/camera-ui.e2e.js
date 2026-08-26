// THE SAME JOURNEY AS `lookthrough.e2e.js`, PERFORMED BY A USER (M11.4, ADR-154).
//
// `lookthrough.e2e.js` drives `add_camera` / `look_through_camera` / `scene_camera_debug` directly and its
// own header says why: "the same commands a future React 'Cameras' panel will feed". That panel did not
// exist — those three commands, plus `set_look_dev_camera`, had ZERO references anywhere in `editor/src`,
// so the only way to place or look through a camera was to drive the app from a script. Which is exactly
// what that spec is.
//
// So this one invokes NOTHING. Every step is a click on a control the shipped editor draws: the View menu
// at the viewport, the row it grows, the badge on the stage, the Inspector's camera section. `invoke` is
// used only to READ (`scene_camera_debug`), never to act — a read cannot fake a capability. If a control
// is missing or does nothing, this spec fails, and that is the whole point of it existing beside the other.
//
// The viewport is native wgpu under a transparent WebView2, so the evidence is an OS-level PrintWindow of
// the COMPOSITED window after each state, exactly as the sibling spec captures it.
//
// Run (LOCAL — needs the GUI + a WebView2-matched msedgedriver):
//   set MTK_SHOT_DIR=<out dir>  &  set MTK_SHOT_PS1=<capture.ps1>
//   node "node_modules\@wdio\cli\bin\wdio.js" run wdio.camtest.conf.js

import { browser } from "@wdio/globals";
import { execSync } from "node:child_process";
import { mkdirSync } from "node:fs";
import path from "node:path";

const SHOT_DIR = process.env.MTK_SHOT_DIR;
const SHOT_PS1 = process.env.MTK_SHOT_PS1;
const PROC = "metrocalk-editor-shell";
mkdirSync(SHOT_DIR, { recursive: true });

/** READ-ONLY. This spec never invokes a command that changes anything — that is its contract. */
const read = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const shot = async (label) => {
  await browser.pause(600);
  const out = path.join(SHOT_DIR, `${label}.png`);
  try {
    execSync(`powershell -ExecutionPolicy Bypass -File "${SHOT_PS1}" -Out "${out}" -ProcName "${PROC}"`, {
      stdio: "ignore",
    });
    console.log("  shot", out);
  } catch (e) {
    console.error("shot failed", label, String(e));
  }
};

/** Click a control by test id, failing loudly if it is not there — a missing control is the finding. */
async function press(testId) {
  const el = await browser.$(`[data-testid="${testId}"]`);
  await el.waitForExist({ timeout: 10000, timeoutMsg: `no control [data-testid="${testId}"]` });
  await el.click();
  await browser.pause(250);
}

const exists = async (testId) => (await browser.$$(`[data-testid="${testId}"]`)).length > 0;

/** Where a control actually IS, in window coordinates, or `null` — `exists` answers a question about the
 *  DOM and the captures answer a question about the screen, and those came apart the first time this ran:
 *  every assertion passed while the stage badge and the Inspector were outside the captured window. */
const boxOf = async (testId) =>
  browser.execute((id) => {
    const el = document.querySelector(`[data-testid="${id}"]`);
    if (!el) return null;
    const r = el.getBoundingClientRect();
    const visible = r.width > 0 && r.height > 0 && r.right > 0 && r.bottom > 0
      && r.left < window.innerWidth && r.top < window.innerHeight;
    return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height),
             visible, win: [window.innerWidth, window.innerHeight] };
  }, testId);

// READ `textContent`, NOT `getText()`. WebDriver's getText returns VISIBLE text, and below 980px both
// docks collapse to icon rails (ADR-125), so a small window makes the connection check answer "" — which
// is indistinguishable from "the editor never connected". The count is a state signal here, not a
// legibility claim, so the DOM is the right place to read it.
const countEntities = async () =>
  browser.execute(() => {
    const el = document.querySelector("#count");
    const m = (el?.textContent || "").match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : Number.NaN;
  });

describe("scene cameras, through the editor (ADR-154)", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
      const b = [...document.querySelectorAll("button")].find((x) => /skip/i.test(x.textContent || ""));
      if (b) b.click();
    });
    await browser.pause(300);
  });

  it("saves the current view as a camera, from the View menu, with no command invoked", async () => {
    await shot("ui-00-before-any-camera");

    // The View menu says, in words, that this scene has no cameras yet — rather than an empty heading.
    await press("vpView");
    expect(await exists("vpSaveCamera")).toBe(true);
    expect(await exists("vpNoCameras")).toBe(true);
    await shot("ui-01-view-menu-no-cameras");

    const before = await countEntities();
    await press("vpSaveCamera");
    await browser.pause(600);

    // ONE authored entity, and the engine agrees there is now an active camera.
    const after = await countEntities();
    const [count, activePresent, fov] = await read("scene_camera_debug");
    console.log(`  entities ${before} -> ${after}; cameras=${count} active=${activePresent} fov=${fov}`);
    expect(after).toBe(before + 1);
    expect(count).toBeGreaterThanOrEqual(1);
    expect(activePresent).toBe(true);

    // The loop closed where the gesture happened: the new camera is SELECTED, so its Inspector section
    // is already open on it. A "created" toast with nothing selected would be the gutter-message defect.
    await browser.waitUntil(async () => exists("camera-section"), {
      timeout: 10000,
      timeoutMsg: "saving a view did not select the camera it made",
    });
    await shot("ui-02-camera-saved-and-selected");
  });

  it("the saved camera carries the aim it was saved with, not the editor's orbit point", async () => {
    // The defect ADR-154 exists to end. A camera with no aim renders whatever the editor is orbiting, and
    // the Inspector says so in words — so the ABSENCE of that warning is the assertion.
    const section = await boxOf("camera-section");
    console.log("  camera section at", JSON.stringify(section));
    expect(section?.visible).toBe(true);
    expect(await exists("cameraUnaimed")).toBe(false);

    const pose = await (await browser.$('[data-testid="cameraPose"]')).getText();
    console.log("  pose read-out:", pose.replace(/\s+/g, " "));
    expect(/Aimed at/.test(pose)).toBe(true);
    // A camera that reports an aim of exactly the world origin on every axis is the unaimed fallback
    // wearing the fix's clothes. The seeded scene is not centred on (0,0,0) at the framing this uses.
    expect(/0\.0,\s*0\.0,\s*0\.0/.test(pose.split("Aimed at")[1] || "")).toBe(false);
  });

  it("looks through it from the Inspector, and the stage says which camera you are inside", async () => {
    await press("cameraLookThrough");
    await browser.pause(700);

    // `<ux_quality>` 5 — a mode change is unmistakable ON THE STAGE, not only in a panel.
    await browser.waitUntil(async () => exists("lookThroughBadge"), {
      timeout: 10000,
      timeoutMsg: "looking through a camera left no marker on the stage",
    });
    const badge = await (await browser.$('[data-testid="lookThroughBadge"]')).getText();
    const where = await boxOf("lookThroughBadge");
    console.log("  stage badge:", badge.replace(/\s+/g, " "), "at", JSON.stringify(where));
    expect(/Looking through/i.test(badge)).toBe(true);
    // ON the stage, and ON the screen. A badge in the DOM and off the window is not a mode indicator.
    expect(where?.visible).toBe(true);

    // And the control that would re-aim this camera at its own picture is OFF, with the reason on screen.
    expect(await exists("cameraRecaptureWhy")).toBe(true);
    await shot("ui-03-looking-through-camera");
  });

  it("changes the lens through the slider and the picture changes with it", async () => {
    const slider = await browser.$('[data-testid="cameraFov"]');
    await slider.waitForExist({ timeout: 10000 });
    // Drive the real range input the way a keyboard user does, then blur to commit — the same path the
    // component commits on, not a synthetic value write.
    for (let i = 0; i < 20; i += 1) await browser.keys("ArrowLeft");
    await slider.click();
    for (let i = 0; i < 20; i += 1) await browser.keys("ArrowLeft");
    await browser.pause(700);
    await shot("ui-04-narrower-lens");

    const [, , fov] = await read("scene_camera_debug");
    console.log("  lens after 20 steps down:", fov);
    expect(fov).toBeLessThan(55);
  });

  it("returns to free look from the badge on the stage", async () => {
    await press("stageFreeLook");
    await browser.pause(700);
    expect(await exists("lookThroughBadge")).toBe(false);
    await shot("ui-05-back-to-free-look");
  });

  it("keeps the camera in the View menu, where it can be returned to", async () => {
    await press("vpView");
    expect(await exists("vpCamera")).toBe(true);
    expect(await exists("vpNoCameras")).toBe(false);
    const row = await (await browser.$('[data-testid="vpCamera"]')).getText();
    console.log("  menu row:", row.replace(/\s+/g, " "));
    expect(/mm/.test(row)).toBe(true); // the lens, in the vocabulary of someone composing a shot
    await shot("ui-06-view-menu-with-camera");

    await press("vpCamera");
    await browser.pause(700);
    expect(await exists("lookThroughBadge")).toBe(true);
    await shot("ui-07-returned-to-the-camera");
    await press("stageFreeLook");
  });
});
