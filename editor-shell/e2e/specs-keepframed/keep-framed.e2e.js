// ADR-195 — a placed camera that keeps its subject framed, and an engine that says when one is in a
// wall, on the packaged `.exe`.
//
// ADR-192 gave the author an absolute pose and the engine a promise never to reinterpret it. That
// promise had a cost nothing said out loud: a CARD shot re-solves around its subject sixty times a
// second, and a PLACED one does not — so the subject walks away and the shot films the empty floor it
// used to stand on. And because a placed camera is never negotiated against the scene, a lens parked
// inside a machine produced a solid-colour frame weeks later with nothing said at the time.
//
// THE ASSERTION THAT CANNOT BE PASSED BY RELABELLING IS THE CAMERA, and this file keeps ADR-192's
// discipline: every claim below is read from `camera_probe`, which reports where the renderer's own
// camera is, or from `cinema_list().problems`, which is the sentence the panel draws.
//
// The four questions, in the order a person would ask them:
//
//   1. Does switching the head on change anything while nothing has moved?  It must NOT. The offset
//      is the framing the author composed, so turning it on is bit-identical until something walks.
//   2. When the subject walks, does the aim follow and the EYE stay put?    Both, or it is not a
//      tripod with a head on it — it is a solver re-placing a camera the author placed.
//   3. Does a locked-off camera still ignore the subject entirely?          The negative control. It
//      is what makes question 2 a statement about tracking rather than about the subject moving.
//   4. Does the engine say when a placed camera is inside something?        In the author's words, in
//      the list the panel already draws, without moving anything.
//
// ...and then the fifth, which is the one every previous run of this lane had to add by hand:
// does any of it survive a reload?

import { existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
/** Where the real frames land — `viewport_capture` asks the RENDER thread for its next frame, so
 *  what is written is the wgpu composite and not a screenshot of the DOM. */
const evidence = path.resolve(dir, "../.shots-keepframed");
mkdirSync(evidence, { recursive: true });

const captureFrame = async (name) => {
  const reply = await invoke("viewport_capture", { path: path.join(evidence, name) });
  expect(reply.reason).toBeFalsy();
  return reply;
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

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const attr = (selector, name) =>
  browser.execute(
    (sel, a) => document.querySelector(sel)?.getAttribute(a) ?? null,
    selector,
    name,
  );

let opN = 0;
/** Move an object the way the editor does — one ordinary `setField` transaction. */
const moveTo = async (id, x, y, z) => {
  for (const [field, value] of [["x", x], ["y", y], ["z", z]]) {
    opN += 1;
    const tx = {
      clientOpId: `kf-op-${opN}`,
      label: `set Transform.${field}`,
      patches: [],
      intent: { kind: "setField", id, component: "Transform", field, value },
    };
    await browser.execute(async (t) => window.__TAURI__.core.invoke("submit_edit", { tx: t }), tx);
  }
  await browser.pause(250);
};

/** The pose the RUNTIME films this shot from, at its opening instant — and, optionally, the PICTURE
 *  it makes, captured WHILE the cutscene still holds the camera.
 *
 *  `cinema_preview` is the same solver Play runs, which is the only reason these numbers are evidence
 *  about the film. And the capture has to happen inside the preview: the first version of this file
 *  probed, tore the preview down, and captured afterwards — so the two "after the walk" frames were
 *  both pictures of the editor's own viewport and came back BYTE-IDENTICAL, which is the
 *  `window-scope-image-metrics` "wrong rectangle" trap arriving as a wrong MOMENT. */
const filmedPose = async (id, capture = null) => {
  await invoke("cinema_preview", { id, seconds: 0, active: true });
  await browser.pause(450);
  const probe = await invoke("camera_probe");
  expect(probe.cinematic).toBe(true);
  if (capture) await captureFrame(capture);
  await invoke("cinema_preview", { id, seconds: 0, active: false });
  await browser.pause(250);
  return probe;
};

const dist = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const note = (line) => console.log(line);
const xyz = (v) => v.map((n) => n.toFixed(2)).join(", ");

describe("ADR-195 · keep the subject framed", () => {
  let subject;
  /** Where the author was standing when they placed the camera. */
  let view;
  /** The subject's authored home, and where it walks to. */
  const HOME = [0, 1.6, 0];
  const WALKED = [5, 1.6, 0];

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await browser.pause(500);
    await browser.execute(() => {
      document.querySelector('[data-testid="onboardSkip"]')?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
  });

  it("places a camera by eye, through the panel's own button", async () => {
    subject = (await invoke("shape_spawn", { kind: "capsule", pos: HOME })).created;
    // A neighbour, far enough away that it is not in any frame below until the camera is put inside
    // it deliberately in the last test.
    await invoke("shape_spawn", { kind: "cylinder", pos: [-9, 0.6, 6] });
    await invoke("frame_all");
    expect(subject).toBeTruthy();

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="shot-hero"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 1, {
      timeout: 15000,
      timeoutMsg: "the shot never landed",
    });

    // The stage reaches its final shape before the first probe — a `viewport` delivery is composed
    // for whatever rectangle the docks have left, and every number below is a camera position.
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    await invoke("view_preset", { preset: "iso" });
    await browser.pause(300);
    await invoke("focus_entity", { id: subject });
    await browser.pause(400);
    view = (await invoke("camera_probe")).eye;
    note(`[view] the stage is standing at ${xyz(view)}`);

    expect(await click('[data-testid="cutscene-clip"]')).toBe(true);
    await (await $('[data-testid="cutscene-shoot-here"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-shoot-here"]')).toBe(true);
    await browser.waitUntil(
      async () => !!(await invoke("cinema_list", { id: subject })).rows[0].camera,
      { timeout: 15000, timeoutMsg: "the pose never reached the document" },
    );
    // Locked off as placed — the state every camera authored before this pass opens in.
    expect((await invoke("cinema_list", { id: subject })).rows[0].camera.track).toBe(null);
    expect(await attr('[data-testid="cutscene-keep-framed"]', "aria-pressed")).toBe("false");
    await filmedPose(subject, "1-placed-locked-off.png");
  });

  it("changes NOTHING when the head goes on while the subject has not moved", async () => {
    // The whole promise of storing the framing OFFSET rather than a bare flag. If turning tracking on
    // moved the frame, an author would have to re-compose every shot they asked to follow — and would
    // find out only when they played it back.
    const before = await filmedPose(subject);
    expect(await click('[data-testid="cutscene-keep-framed"]')).toBe(true);
    await browser.waitUntil(
      async () => !!(await invoke("cinema_list", { id: subject })).rows[0].camera.track,
      { timeout: 15000, timeoutMsg: "the head never went on" },
    );
    const after = await filmedPose(subject);
    note(`[at rest] ${xyz(before.eye)} -> ${xyz(after.eye)}, aim ${xyz(before.lookAt)} -> ${xyz(after.lookAt)}`);
    expect(dist(after.eye, before.eye)).toBeLessThan(0.001);
    expect(dist(after.lookAt, before.lookAt)).toBeLessThan(0.001);

    // ...and the panel says the state, in the two places a person reads it.
    expect(await attr('[data-testid="cutscene-keep-framed"]', "aria-pressed")).toBe("true");
    expect(await text('[data-testid="cutscene-shot-reads"]')).toMatch(/keeping it framed/);
    // `lookAt` is gone from the read-out: it is the framing the offset was taken from, not a place
    // the camera still looks, and printing it would be a number that stops being true when the
    // subject moves.
    expect(await text('[data-testid="cutscene-placed-pose"]')).toMatch(/aim follows/);
    expect(await text('[data-testid="cutscene-placed-pose"]')).not.toMatch(/looking at/);
  });

  it("turns the head and never the tripod when the subject walks", async () => {
    const placed = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    await moveTo(subject, ...WALKED);
    const followed = await filmedPose(subject, "3-following-after-the-walk.png");
    note(`[walked] eye ${xyz(followed.eye)}, aim ${xyz(followed.lookAt)}`);

    // THE TRIPOD IS BOLTED DOWN — the eye is the pose the author placed, to a millimetre.
    expect(dist(followed.eye, placed.eye)).toBeLessThan(0.01);
    // ...and the AIM moved by exactly what the subject did, so the framing the author composed is
    // carried rather than re-derived from a centroid.
    const expected = [
      placed.lookAt[0] + (WALKED[0] - HOME[0]),
      placed.lookAt[1] + (WALKED[1] - HOME[1]),
      placed.lookAt[2] + (WALKED[2] - HOME[2]),
    ];
    expect(dist(followed.lookAt, expected)).toBeLessThan(0.05);
  });

  it("THE NEGATIVE CONTROL: locked off, the same walk films the floor it used to stand on", async () => {
    // Without this, every line above is satisfied by an engine that re-solves ALL placed cameras
    // around their subject — which is the thing ADR-192 promises never to do.
    expect(await click('[data-testid="cutscene-keep-framed"]')).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).rows[0].camera.track === null,
      { timeout: 15000, timeoutMsg: "the head never came off" },
    );
    const placed = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    const locked = await filmedPose(subject, "2-locked-off-after-the-walk.png");
    note(`[locked off] eye ${xyz(locked.eye)}, aim ${xyz(locked.lookAt)} — subject is at ${xyz(WALKED)}`);
    expect(dist(locked.eye, placed.eye)).toBeLessThan(0.01);
    // The authored aim, unchanged, five metres from where the subject now is.
    expect(dist(locked.lookAt, placed.lookAt)).toBeLessThan(0.01);
    expect(dist(locked.lookAt, WALKED)).toBeGreaterThan(1.0);

    // Back on for the rest of the run, and for the reload.
    expect(await click('[data-testid="cutscene-keep-framed"]')).toBe(true);
    await browser.waitUntil(
      async () => !!(await invoke("cinema_list", { id: subject })).rows[0].camera.track,
      { timeout: 15000 },
    );
  });

  it("says when a placed camera is standing inside something, without moving it", async () => {
    // A box big enough to swallow the lens, placed so the eye is at its CENTRE. A shape rests on the
    // ground in its own local frame (`rest_on_ground`), so a 3 m block's origin has to sit 1.5 m
    // below the camera for the camera to be inside it — putting the origin AT the eye would leave the
    // lens on the bottom face, which is a different question. The engine never re-places a camera the
    // author placed, so the only thing it can do is say so.
    const wall = (await invoke("shape_spawn", { kind: "box", pos: [0, 0, 0] })).created;
    await invoke("shape_update", { id: wall, params: { width: 3, height: 3, depth: 3 } });
    const placed = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    await moveTo(wall, placed.eye[0], placed.eye[1] - 1.5, placed.eye[2]);
    await browser.pause(500);

    // ADR-200 made each entry `{ shot, message }`; the sentences are what this spec asserts.
    const said = (await invoke("cinema_list", { id: subject })).problems.map((p) => p.message);
    note(`[problems] ${JSON.stringify(said)}`);
    const buried = said.find((p) => /inside something/.test(p));
    expect(buried).toBeTruthy();
    // The author's words, naming the shot and what will happen. No mechanism named.
    expect(buried).toMatch(/^shot 1's/);
    expect(buried).toMatch(/solid colour/);
    expect(said.join(" ")).not.toMatch(/eye_inside|vantage|Vantage/);

    // A NOTE, NOT A CORRECTION. The pose in the document is untouched and the camera the runtime
    // films from is still the one the author placed — which is the whole reason the engine has to
    // speak at all.
    const after = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    expect(dist(after.eye, placed.eye)).toBeLessThan(0.001);
    const stillThere = await filmedPose(subject);
    expect(dist(stillThere.eye, placed.eye)).toBeLessThan(0.01);

    // ...and it goes quiet when the obstruction goes away, so the warning is about the geometry and
    // not about the shot having a placed camera at all.
    await invoke("remove_entity", { id: wall });
    await browser.pause(500);
    const cleared = (await invoke("cinema_list", { id: subject })).problems.map((p) => p.message);
    note(`[problems, cleared] ${JSON.stringify(cleared)}`);
    expect(cleared.some((p) => /inside something/.test(p))).toBe(false);
  });

  it("survives Save, New and Open — the head is in the document, not in this session", async () => {
    const before = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    expect(before.track).toBeTruthy();
    // Beside this spec's own evidence, NOT beside the .exe: `CARGO_TARGET_DIR` can put the binary
    // anywhere (the main checkout's warm target, when this runs from a worktree), and a save into a
    // directory that does not exist fails as `os error 3` five tests deep.
    const file = path.join(evidence, "keep-framed.mtk");
    if (existsSync(file)) rmSync(file, { force: true });
    await invoke("save_project", { path: file });
    await browser.waitUntil(async () => existsSync(file), {
      timeout: 20000,
      timeoutMsg: "no .mtk was written",
    });
    // NEW FIRST. Re-opening over a live document could pass on an engine that saved nothing at all.
    await invoke("new_project");
    await browser.pause(900);
    expect((await invoke("cinema_list", { id: subject })).shots).toBe(0);

    await invoke("open_project", { path: file });
    await browser.pause(1500);
    const reopened = await invoke("cinema_list", { id: subject });
    expect(reopened.shots).toBe(1);
    const after = reopened.rows[0].camera;
    expect(after).toBeTruthy();
    expect(after.track).toBeTruthy();
    note(`[reload] track ${JSON.stringify(before.track)} -> ${JSON.stringify(after.track)}`);
    expect(dist(after.track, before.track)).toBeLessThan(0.001);
    expect(dist(after.eye, before.eye)).toBeLessThan(0.001);
    // ...and the shot still FILMS the way it did, which is the claim the numbers are evidence for.
    expect(reopened.rows[0].reads).toMatch(/keeping it framed/);
  });
});
