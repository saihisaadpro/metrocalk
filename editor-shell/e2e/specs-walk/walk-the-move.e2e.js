// ADR-201 — walking a shot's camera path from the stage, on the packaged `.exe`.
//
// A shot is a PATH. `plan_shot` has judged five instants along it since ADR-176, because a push-in
// that starts in clear air and ends inside a machine is what produced the solid-red frames in the
// first factory film — and it kept one verdict. ADR-200 could stand the author at the instant that
// verdict came from; the other four fifths of the path were reachable only through Preview, which
// HOLDS the viewport: the camera is taken, any drag is overwritten on the next tick, and neither
// "orbit from here" nor "shoot from this view" is available. So the frames an author most needs to
// judge were the ones they could look at and not stand in.
//
// WHAT MAKES THIS EVIDENCE RATHER THAN A REPLY READ BACK TO ITSELF. Every camera claim is checked
// against `camera_probe`, which reports where the RENDERER'S camera actually is, and `cinematic`
// distinguishes the author's own orbit camera from a held cutscene one. A reply that said `moved:
// true` about a camera that had not moved would pass a spec written any other way.
//
// The five questions, in the order a person would ask them:
//
//   1. A locked-off shot: is there a walk?                 There must NOT be. The negative control,
//      first, because a scrub over a shot that sweeps nothing moves and changes nothing.
//   2. Give it a move through the panel: does one appear?  And with the engine's own five instants
//      drawn on it, not a number this side invented.
//   3. Drag the slider: does the camera go?                To BOTH ends of the move, to two different
//      places, and staying the author's throughout.
//   4. Does the track describe the walk?                   Standing at each drawn instant has to
//      reproduce the verdict drawn there, or the picture is decoration.
//   5. Does what the walk produces persist?                "Shoot from this view" from where the walk
//      stopped, through Save / New / Open.
//
// Local-only, for the standing reason the rest of this directory is: a display, a WebView2-matched
// `msedgedriver`, and a GPU the wgpu surface can be created on.

import { existsSync, mkdirSync, rmSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const evidence = path.resolve(dir, "../.shots-walk");
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

/** Drive a real control the way a pointer does — React listens on `input`/`change`, and a bare
 *  `el.value = x` sets the DOM property without telling it. */
const setValue = (testid, value) =>
  browser.execute(
    (tid, v) => {
      const el = document.querySelector(`[data-testid="${tid}"]`);
      if (!el) return false;
      const proto =
        el.tagName === "SELECT" ? window.HTMLSelectElement.prototype : window.HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(proto, "value").set.call(el, String(v));
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      el.dispatchEvent(new PointerEvent("pointerup", { bubbles: true }));
      return true;
    },
    testid,
    value,
  );

const present = (testid) =>
  browser.execute((tid) => !!document.querySelector(`[data-testid="${tid}"]`), testid);

/** The verdicts the track actually DRAWS, read off the dots rather than off the reply that fed them. */
const drawnMarks = () =>
  browser.execute(() =>
    [...document.querySelectorAll('[data-testid="cutscene-walk-mark"]')].map((n) => ({
      progress: Number(n.getAttribute("data-progress")),
      clear: n.getAttribute("data-clear") === "yes",
    })),
  );

const readOut = () =>
  browser.execute(
    () => document.querySelector('[data-testid="cutscene-walk-reads"]')?.textContent ?? "",
  );

const finite = (v) => v.every((n) => Number.isFinite(n));
const dist = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const note = (line) => console.log(line);
const xyz = (v) => v.map((n) => n.toFixed(2)).join(", ");

describe("ADR-201 · walking a shot's move", () => {
  let subject;
  const HOME = [0, 1.6, 0];

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
    await click('[data-testid="stop"]');
    await browser.pause(500);
    await browser.execute(() => {
      document
        .querySelector('[data-testid="onboardSkip"]')
        ?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    subject = (await invoke("shape_spawn", { kind: "capsule", pos: HOME })).created;
    expect(subject).toBeTruthy();
    await invoke("frame_all");

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    // Through the panel's own catalogue, not through a command: this is the gesture an author makes.
    expect(await click('[data-testid="shot-hero"]')).toBe(true);
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 1, {
      timeout: 15000,
      timeoutMsg: "the shot never landed",
    });

    // The panel that draws the walk lives in the bottom dock's Animation workspace.
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });
    expect(await click('[data-testid="cutscene-clip"]')).toBe(true);
    await (await $('[data-testid="cutscene-shot-editor"]')).waitForExist({ timeout: 10000 });
  });

  it("offers no walk for a shot that sweeps nothing", async () => {
    // THE NEGATIVE CONTROL, and it is what makes the rest of this file mean anything: `moving` has to
    // be caused by a camera that travels, not by a shot existing. Set through the panel's own Camera
    // move control, which is the gesture that produces this state.
    expect(await setValue("cutscene-motion", "hold")).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).rows[0].motion === "hold",
      { timeout: 15000, timeoutMsg: "the shot never became a locked-off one" },
    );

    const still = await invoke("cinema_stand_at_shot", { id: subject, index: 0 });
    note(`[hold] ${JSON.stringify({ moving: still.moving, travel: still.travel, path: still.path.length })}`);
    expect(still.reason).toBeFalsy();
    expect(still.moved).toBe(true);
    expect(still.moving).toBe(false);
    expect(still.travel).toBeLessThan(0.001);
    // ...and its read-out does not talk about a move it does not have.
    expect(still.message).not.toMatch(/move/);

    expect(await click('[data-testid="cutscene-stand-here"]')).toBe(true);
    await browser.pause(900);
    expect(await present("cutscene-walk")).toBe(false);
    await captureFrame("1-nothing-to-walk.png");
  });

  it("appears once the shot has a move and the author is standing on it", async () => {
    // Both edits through the panel, both one ordinary undoable command, exactly as an author makes
    // them: a move, and a strength for it.
    expect(await setValue("cutscene-motion", "push_in")).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).rows[0].motion === "push_in",
      { timeout: 15000, timeoutMsg: "the move never landed" },
    );
    expect(await setValue("cutscene-amount", "0.6")).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).rows[0].amount > 0.5,
      { timeout: 15000, timeoutMsg: "the move strength never landed" },
    );

    // THE PANEL'S OWN BUTTON, not the command underneath it. The strip is the second half of this
    // gesture and cannot exist before it.
    expect(await present("cutscene-walk")).toBe(false);
    expect(await click('[data-testid="cutscene-stand-here"]')).toBe(true);
    await (await $('[data-testid="cutscene-walk"]')).waitForExist({ timeout: 15000 });

    // FIVE dots, because the planner scores five. A track drawn at twenty points would be claiming a
    // resolution the placement search does not have.
    const marks = await drawnMarks();
    note(`[marks] ${JSON.stringify(marks)}`);
    expect(marks.length).toBe(5);
    expect(marks.map((m) => m.progress)).toEqual([0, 0.25, 0.5, 0.75, 1]);
    expect(await readOut()).toMatch(/its move/);
    await captureFrame("2-walk-appears.png");
  });

  it("takes the camera to both ends of the move when the slider is dragged", async () => {
    // THE CAPABILITY. Each stop is read back off `camera_probe` — the renderer's own camera — because
    // a reply claiming `moved: true` about a camera that had not moved would pass any spec that only
    // read the reply.
    expect(await setValue("cutscene-walk-slider", "0")).toBe(true);
    await browser.pause(1200);
    const opening = await invoke("camera_probe");
    note(`[0%] eye ${xyz(opening.eye)} cinematic=${opening.cinematic}`);
    expect(finite(opening.eye)).toBe(true);
    // THE AUTHOR'S OWN CAMERA, which is the whole difference from a preview: they can orbit from
    // here, and the next tick will not take it back.
    expect(opening.cinematic).toBe(false);
    expect(await readOut()).toMatch(/at the opening of its move/);
    await captureFrame("3-walk-opening.png");

    expect(await setValue("cutscene-walk-slider", "1")).toBe(true);
    await browser.pause(1200);
    const end = await invoke("camera_probe");
    note(`[100%] eye ${xyz(end.eye)} cinematic=${end.cinematic}`);
    expect(end.cinematic).toBe(false);
    expect(await readOut()).toMatch(/at the end of its move/);
    await captureFrame("4-walk-end.png");

    // TWO DIFFERENT PLACES, and the distance between them is the travel the engine reported — the
    // number the slider's own tooltip quotes.
    const travelled = dist(opening.eye, end.eye);
    const reply = await invoke("cinema_stand_at_shot", { id: subject, index: 0, progress: 1 });
    note(`[travel] walked ${travelled.toFixed(3)} m, reported ${reply.travel.toFixed(3)} m`);
    expect(travelled).toBeGreaterThan(0.05);
    expect(Math.abs(travelled - reply.travel)).toBeLessThan(Math.max(0.05, reply.travel * 0.05));
    // A push-in ENDS CLOSER to what it films than it starts.
    expect(dist(end.eye, HOME)).toBeLessThan(dist(opening.eye, HOME));
  });

  it("draws a track that describes the walk it is a track of", async () => {
    // A picture whose dots disagreed with what standing there actually finds is decoration. Every
    // drawn instant is visited through the same command the slider sends, and its verdict compared
    // with the one the dot beside it claims.
    const marks = await drawnMarks();
    expect(marks.length).toBe(5);
    for (const mark of marks) {
      const at = await invoke("cinema_stand_at_shot", { id: subject, index: 0, progress: mark.progress });
      expect(at.reason).toBeFalsy();
      expect(Math.abs(at.progress - mark.progress)).toBeLessThan(1e-4);
      expect(at.acceptable).toBe(mark.clear);
    }

    // ...AND THE WORST ONE IS ONE OF THEM, and asking for no instant at all lands on it — which is
    // what a warning's own "Take me there" still means.
    const unnamed = await invoke("cinema_stand_at_shot", { id: subject, index: 0 });
    note(`[worst] ${unnamed.worst} progress=${unnamed.progress}`);
    expect(Math.abs(unnamed.progress - unnamed.worst)).toBeLessThan(1e-4);
    expect(marks.some((m) => Math.abs(m.progress - unnamed.worst) < 1e-4)).toBe(true);

    // Past the end is the end, not a refusal: a slider's own arithmetic is what produces the number.
    const past = await invoke("cinema_stand_at_shot", { id: subject, index: 0, progress: 4.2 });
    expect(past.reason).toBeFalsy();
    expect(Math.abs(past.progress - 1)).toBeLessThan(1e-4);
  });

  it("refuses to walk while Play is driving the camera, and says why", async () => {
    // The one state in which a walk would move nothing: Play owns the viewport, and a step that was
    // overwritten on the next tick would look exactly like a control that had stopped working.
    await click('[data-testid="play"]');
    await browser.pause(900);
    const refused = await invoke("cinema_stand_at_shot", { id: subject, index: 0, progress: 0.5 });
    note(`[during play] ${refused.reason}`);
    expect(refused.moved).toBe(false);
    expect(refused.reason).toMatch(/Play/);
    await click('[data-testid="stop"]');
    await browser.pause(900);
  });

  it("stores the frame the walk stopped at, and it is still there after Save, New and Open", async () => {
    // WHAT THE WALK PRODUCES HAS TO PERSIST, even though the walk itself does not. A camera op is
    // render state — not undoable, not saved, and deliberately so, because a control that wrote a
    // viewport pose into the document would make orbiting an edit. The author's decision is the next
    // click, and THAT is an ordinary `ShotCamera` on the shot.
    expect(await setValue("cutscene-walk-slider", "0.5")).toBe(true);
    await browser.pause(1200);
    const middle = await invoke("camera_probe");
    note(`[50%] eye ${xyz(middle.eye)}`);
    expect(middle.cinematic).toBe(false);

    expect(await click('[data-testid="cutscene-shoot-here"]')).toBe(true);
    await browser.waitUntil(
      async () => !!(await invoke("cinema_list", { id: subject })).rows[0].camera,
      { timeout: 15000, timeoutMsg: "the view was never stored" },
    );
    const stored = (await invoke("cinema_list", { id: subject })).rows[0].camera;
    note(`[stored] eye ${xyz(stored.eye)}`);
    expect(dist(stored.eye, middle.eye)).toBeLessThan(0.01);
    await captureFrame("5-shot-from-the-walk.png");

    // Beside this spec's own evidence, NOT beside the .exe: `CARGO_TARGET_DIR` can put the binary
    // anywhere, and a save into a directory that does not exist fails as `os error 3`.
    const file = path.join(evidence, "walk-the-move.mtk");
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

    const reopened = (await invoke("cinema_list", { id: subject })).rows[0];
    note(`[reopened] eye ${xyz(reopened.camera.eye)}`);
    expect(dist(reopened.camera.eye, middle.eye)).toBeLessThan(0.01);
    // AND THE MOVE CAME BACK WITH IT — a placed camera keeps all six verbs, so the reopened shot is
    // still walkable. That is the row a `camera.is_none()` reading of "does this move" gets wrong.
    expect(reopened.motion).toBe("push_in");
    const again = await invoke("cinema_stand_at_shot", { id: subject, index: 0 });
    note(`[reopened walk] placed=${again.placed} moving=${again.moving} travel=${again.travel}`);
    expect(again.placed).toBe(true);
    expect(again.moving).toBe(true);
    expect(again.travel).toBeGreaterThan(0.001);
    await captureFrame("6-after-reopen.png");
  });
});
