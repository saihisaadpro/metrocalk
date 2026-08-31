// THE PLAYHEAD ANSWERING WITH A PICTURE — on the PACKAGED .exe, driven through the UI.
//
// BEFORE: `solve_shot` was pure in `(recipe, subject, t)`, so the engine could produce the camera at
// ANY instant of any cutscene — and the only thing that ever asked was the Play loop. Composing a cut
// meant choosing five words, pressing Play, watching the whole sequence from its start to reach the
// one shot being edited, pressing Stop, and changing one word. Every framing decision cost a full
// playback, and the edit itself changed nothing the author could see.
//
// AFTER: turn Preview on and the viewport stands where the playhead is, solved by
// `present_cinematic_moment` — the SAME function Play runs every tick. Scrub and it follows. Change
// the angle and it re-poses at the same moment.
//
// EVERY CLAIM HERE IS A NUMBER OFF THE LIVE ENGINE. `camera_probe` reports the real eye, look-at and
// `cinematic` flag from the render state, so "the camera moved" is arithmetic rather than a
// screenshot that looks plausible. The only commands invoked directly are READS (`camera_probe`,
// `cinema_list`) and the scene setup; every step of the capability under test is a click on the
// control a user would click.

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-cinemapreview");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

/** An OS capture of the real composite. The viewport is a transparent WebView2 over the native wgpu
 *  surface, so a WebDriver screenshot is the React panels and a black hole where the 3D is. */
async function shot(label) {
  await browser.pause(500);
  const out = path.join(shots, `${String(shotIndex).padStart(2, "0")}_${label}.png`);
  shotIndex += 1;
  const good = () => existsSync(out) && statSync(out).size > 20_000;
  const attempt = (script, args) => {
    try {
      execFileSync("powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, ...args], { stdio: "pipe" });
    } catch { /* fall through */ }
    if (!good() && existsSync(out)) rmSync(out);
    return good();
  };
  let ok = false;
  for (let round = 0; round < 3 && !ok; round += 1) {
    if (round > 0) await browser.pause(1000);
    ok = attempt(capture, ["-Out", out]) || attempt(captureFg, ["-ProcName", "metrocalk-editor-shell", "-Out", out]);
  }
  if (!ok) console.log(`[prev] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
  else console.log(`[prev] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return ok ? out : null;
}

const click = (selector) =>
  browser.execute((sel) => {
    const el = document.querySelector(sel);
    if (!el) return false;
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    return true;
  }, selector);

/** Click a control by its VISIBLE WORD inside a container. The dock's tab strip has no test id — it
 *  is the shared `DockTabs` — and inventing one for a test would be a hook nothing else uses. */
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

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const exists = (selector) => browser.execute((sel) => !!document.querySelector(sel), selector);

const cam = () => invoke("camera_probe");
const dist3 = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const fmt = (v) => `[${v.map((n) => n.toFixed(2)).join(", ")}]`;

describe("Shot preview — the playhead poses the real camera, without Play", () => {
  let statue;
  let editorCam;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await invoke("new_project");
    await browser.pause(700);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("builds a set worth pointing a camera at", async () => {
    statue = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.6, 0] })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.2, 0] });
    for (const p of [[7, 0.5, 6], [-8, 0.5, 5], [6, 0.5, -7], [-6, 0.5, -6]]) {
      await invoke("shape_spawn", { kind: "box", pos: p });
    }
    await invoke("frame_all");
    expect(statue).toBeTruthy();
    await browser.pause(400);
    editorCam = await cam();
    console.log(`[prev] editor camera eye=${fmt(editorCam.eye)} cinematic=${editorCam.cinematic}`);
    // The premise every later measurement is taken against: nothing owns the camera yet.
    expect(editorCam.cinematic).toBe(false);
    await shot("00_editor_camera");
  });

  it("authors a three-shot cut through the cards, then opens it on the timeline", async () => {
    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(statue)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });

    for (const card of ["establish", "hero", "closeup"]) {
      expect(await click(`[data-testid="shot-${card}"]`)).toBe(true);
      await browser.pause(500);
    }
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: statue })).shots === 3, {
      timeout: 15000,
      timeoutMsg: "the three shots never landed",
    });

    // Open the timeline the way a user does: the bottom dock, its Animate workspace, its Cutscene tab.
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });
    const clips = await browser.execute(
      () => document.querySelectorAll('[data-testid="cutscene-clip"]').length,
    );
    console.log(`[prev] the timeline draws ${clips} clips`);
    expect(clips).toBe(3);
    await shot("01_timeline_no_preview");
  });

  it("BEFORE: the timeline names the shot, and the viewport is still the editor's", async () => {
    // The gap this session closed, stated as an assertion rather than as prose. The playhead knows
    // which shot is live; the picture in front of the author is unrelated to it.
    const readout = await text('[data-testid="cutscene-panel"] .mtk-toolbar');
    console.log(`[prev] the toolbar reads: ${readout?.replace(/\s+/g, " ").trim()}`);
    expect(readout).toContain("shot 1 of 3");
    const still = await cam();
    expect(still.cinematic).toBe(false);
    expect(dist3(still.eye, editorCam.eye)).toBeLessThan(0.01);
    // ...and the pose read-out does not exist yet, because nothing is standing anywhere.
    expect(await exists('[data-testid="cutscene-preview-pose"]')).toBe(false);
    expect(await exists('[data-testid="cinemaPreviewBadge"]')).toBe(false);
  });

  it("AFTER: one click on Preview stands the real camera on the playhead", async () => {
    expect(await click('[data-testid="cutscene-preview"]')).toBe(true);
    await browser.waitUntil(async () => (await cam()).cinematic === true, {
      timeout: 15000,
      timeoutMsg: "the preview never took the camera",
    });
    const posed = await cam();
    console.log(`[prev] posed  eye=${fmt(posed.eye)} lookAt=${fmt(posed.lookAt)} cinematic=${posed.cinematic}`);

    // 1. A DIFFERENT camera from the editor's — the whole claim, in one number.
    const moved = dist3(posed.eye, editorCam.eye);
    console.log(`[prev] the camera moved ${moved.toFixed(2)} units from the editor's view`);
    expect(moved).toBeGreaterThan(1.0);
    // 2. Aimed AT THE SUBJECT, which is what a solved shot means and an orbit view never guarantees.
    const aimErr = dist3(posed.lookAt, [0, 1.6, 0]);
    console.log(`[prev] aim error vs the statue: ${aimErr.toFixed(2)} units`);
    expect(aimErr).toBeLessThan(3.0);
    // 3. Play is NOT running. This is the distinction the whole feature exists for.
    expect(await exists('[data-testid="stop"]')).toBe(false);

    // 4. The surface says so where the author is looking, and the numbers agree with the engine.
    const pose = await text('[data-testid="cutscene-preview-pose"]');
    console.log(`[prev] the panel reads: ${pose?.replace(/\s+/g, " ").trim()}`);
    expect(pose).toBeTruthy();
    for (const n of posed.eye) expect(pose).toContain(n.toFixed(2));

    // 5. ...and the STAGE says so too, because a held camera is easy to miss.
    const badge = await text('[data-testid="cinemaPreviewBadgeShot"]');
    console.log(`[prev] the stage badge reads: ${badge}`);
    expect(badge).toContain("shot 1 of 3");
    await shot("02_preview_shot_1");
  });

  it("scrubbing to another clip moves the real camera to that shot", async () => {
    const first = await cam();
    const jumped = await browser.execute(() => {
      const clips = document.querySelectorAll('[data-testid="cutscene-clip"]');
      if (clips.length < 3) return false;
      clips[2].dispatchEvent(new MouseEvent("click", { bubbles: true }));
      return true;
    });
    expect(jumped).toBe(true);
    await browser.waitUntil(
      async () => dist3((await cam()).eye, first.eye) > 0.5,
      { timeout: 15000, timeoutMsg: "the camera never followed the playhead" },
    );
    const third = await cam();
    console.log(`[prev] shot 3 eye=${fmt(third.eye)} (moved ${dist3(third.eye, first.eye).toFixed(2)})`);
    // The third card is a CLOSE-UP and the first an establishing wide, so the close shot must stand
    // nearer to the subject. A preview that ignored `t` would have produced the same pose twice.
    console.log(`[prev] distance to subject: shot 1 ${first.distance.toFixed(2)} -> shot 3 ${third.distance.toFixed(2)}`);
    expect(third.distance).toBeLessThan(first.distance);
    expect(await text('[data-testid="cinemaPreviewBadgeShot"]')).toContain("shot 3 of 3");
    await shot("03_preview_shot_3_closeup");
  });

  it("THE LOOP: changing the angle re-poses at the same moment", async () => {
    const before = await cam();
    const changed = await browser.execute(() => {
      const select = document.querySelector('[data-testid="cutscene-angle"]');
      if (!select) return false;
      const setter = Object.getOwnPropertyDescriptor(window.HTMLSelectElement.prototype, "value").set;
      setter.call(select, "low");
      select.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    });
    expect(changed).toBe(true);
    // The edit lands AND the viewport is re-solved. Without the re-pose the author changes an angle
    // and the picture in front of them is the old one — an edit they have to imagine.
    await browser.waitUntil(
      async () => dist3((await cam()).eye, before.eye) > 0.2,
      { timeout: 15000, timeoutMsg: "the viewport never showed the new angle" },
    );
    const after = await cam();
    console.log(`[prev] re-framed from below: eye ${fmt(before.eye)} -> ${fmt(after.eye)}`);
    // "From below" means below: the new eye is lower than the old one.
    expect(after.eye[1]).toBeLessThan(before.eye[1]);
    expect((await invoke("cinema_list", { id: statue })).rows[2].angle).toBe("low");
    await shot("04_reframed_from_below");
  });

  it("Exit on the stage badge gives the author their camera back, exactly", async () => {
    expect(await click('[data-testid="stageExitPreview"]')).toBe(true);
    await browser.waitUntil(async () => (await cam()).cinematic === false, {
      timeout: 15000,
      timeoutMsg: "the preview never handed the camera back",
    });
    const back = await cam();
    console.log(`[prev] restored eye=${fmt(back.eye)} (editor was ${fmt(editorCam.eye)})`);
    // EXACTLY as it was found — the saved view is restored, not re-derived.
    expect(dist3(back.eye, editorCam.eye)).toBeLessThan(0.01);
    expect(await exists('[data-testid="cinemaPreviewBadge"]')).toBe(false);
    expect(await exists('[data-testid="cutscene-preview-pose"]')).toBe(false);
    await shot("05_camera_handed_back");
  });

  it("Play still owns the camera, and refuses the preview in plain language", async () => {
    await click('[data-testid="play"]');
    await browser.waitUntil(async () => exists('[data-testid="stop"]'), {
      timeout: 15000,
      timeoutMsg: "Play never engaged",
    });
    const toggle = await browser.execute(() => {
      const el = document.querySelector('[data-testid="cutscene-preview"]');
      return el ? { disabled: el.hasAttribute("disabled"), title: el.getAttribute("title") } : null;
    });
    console.log(`[prev] during Play the toggle is ${JSON.stringify(toggle)}`);
    expect(toggle).toBeTruthy();
    expect(toggle.disabled).toBe(true);
    expect((toggle.title ?? "").toLowerCase()).toContain("stop play");
    // ...and the stage does not claim a preview it is not showing.
    expect(await exists('[data-testid="cinemaPreviewBadge"]')).toBe(false);
    await click('[data-testid="stop"]');
    await browser.pause(800);
  });
});
