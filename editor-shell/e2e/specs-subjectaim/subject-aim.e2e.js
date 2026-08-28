// AIMING A SHOT BY POINTING AT THE THING — on the PACKAGED .exe, driven through the UI.
//
// BEFORE: three capabilities had crossed the boundary and never met. `viewport_peek` names the entity
// under the cursor WITHOUT touching the selection — written for M3.3 hover and, until this change,
// referenced by nothing in `editor/src`. `cinema_subject_catalog` ranks the scene's own hierarchy with
// a DRAWN-PART count on every row. `cinema_set_shot_subject` re-aims a shot as one undoable edit. What
// a user could reach was a search box: in a 15,711-part import, "film THAT one" meant knowing its name.
//
// AND THE OBVIOUS GESTURE CANNOT WORK. "Select the object, then frame the selection" is impossible
// here by construction: the Cutscene panel is bound to the editor selection, so selecting the thing
// you want to film switches which cutscene is on screen and throws the shot away. That is why the aim
// reads through the non-mutating peek, and why the assertion this spec exists for is a NEGATIVE one —
// after aiming a shot by clicking an object, the selection must be exactly where it was.
//
// THE PROOF IS NOT THAT A LABEL CHANGED. It is (1) that the native selection model is untouched, read
// back off the renderer's own state, and (2) that the CAMERA moved — the same solver Play runs is
// asked where it stands for each shot, and a wide of a six-part assembly stands further back and looks
// somewhere else than a close-up of one part inside it.
//
// Empty scene (MTK_SCENE_N=0) so only what the test builds is in the hierarchy the badge reads.

import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-subjectaim");
const capture = path.resolve(dir, "../scripts/capture-composited-window.ps1");
const captureFg = path.resolve(dir, "../../../.uxtest/audit/exe/capture-window-fg.ps1");
mkdirSync(shots, { recursive: true });

let shotIndex = 0;

/** Every measured number, to stdout AND to a file beside the captures — the reporter swallows a
 *  spec's own `console.log` at this level, and numbers in a scrolled terminal proved nothing. */
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

/** An OS capture of the real composite — the viewport is a transparent WebView2 over native wgpu, so
 *  a WebDriver screenshot is the React panels and a black hole where the 3D is. */
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
  if (!ok) note(`[aim] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
  else note(`[aim] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
  return ok ? out : null;
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

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const present = (selector) => browser.execute((sel) => !!document.querySelector(sel), selector);

/** Every rung the badge is offering, as the user reads it — off the DOM, because the claim under test
 *  is what the BADGE shows. A stage that fetched the right chain and drew one rung of it would pass
 *  an assertion made against the reply. */
const aimRungs = () =>
  browser.execute(() =>
    [...document.querySelectorAll('[data-testid^="subjectAimRung-"]')].map((el) => ({
      id: (el.getAttribute("data-testid") ?? "").replace("subjectAimRung-", ""),
      parts: Number(el.getAttribute("data-parts")),
      text: (el.textContent ?? "").replace(/\s+/g, " ").trim(),
    })),
  );

/** The RENDERER's selection — `st.selected` projected off the same selection model `viewport_pick`
 *  mutates. This is the number that says whether the aim picked or peeked. */
const nativeSelection = () => invoke("gizmo_selected");

/** The EDITOR's selection, which is what decides whose cutscene the panel is showing. */
const panelSelection = () =>
  browser.execute(
    () => document.querySelector('[data-testid="hrow"][aria-selected="true"]')?.getAttribute("data-id") ?? null,
  );

/** Dispatch one pointer gesture on the stage surface itself, at real client coordinates.
 *
 *  ON THE ELEMENT, not on a child: `stageInput.ts` decides ownership with
 *  `event.target === event.currentTarget`, which is the browser's own hit test, and a gesture that
 *  began on a control floating over the stage did not begin on the stage. */
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

/** Walk a coarse grid over the stage and ask the engine what is behind each point.
 *
 *  FINDING A PIXEL WITH SOMETHING BEHIND IT IS SETUP, not the thing under test: the gesture asserted
 *  below is the click, and it goes through the UI at whatever point this returns. `viewport_peek` is
 *  a read that changes nothing, which is exactly why it can be used to look for one. */
const findStagePoint = async (accept) => {
  const rect = await browser.execute(() => {
    const el = document.querySelector('[data-testid="viewport"]');
    if (!el) return null;
    const r = el.getBoundingClientRect();
    return { left: r.left, top: r.top, width: r.width, height: r.height, w: window.innerWidth, h: window.innerHeight };
  });
  if (!rect) return null;
  for (const fy of [0.5, 0.44, 0.56, 0.36, 0.64, 0.28, 0.72]) {
    for (const fx of [0.5, 0.44, 0.56, 0.36, 0.64, 0.28, 0.72, 0.2, 0.8]) {
      const clientX = Math.round(rect.left + rect.width * fx);
      const clientY = Math.round(rect.top + rect.height * fy);
      const id = await invoke("viewport_peek", { x: clientX / rect.w, y: clientY / rect.h });
      if (id && (!accept || accept(id))) return { clientX, clientY, id };
    }
  }
  return null;
};

const dist3 = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const fmt = (v) => `[${v.map((n) => n.toFixed(2)).join(", ")}]`;
const poseAt = (id, seconds) => invoke("cinema_preview", { id, seconds, active: true });

describe("Aiming a shot by pointing at the thing", () => {
  let hall; // the assembly — six parts under one group node
  let part; // the object the cutscene hangs on
  let boxes = [];
  let hallName;
  let partName;

  before(async () => {
    await browser.waitUntil(
      async () => browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core)),
      { timeout: 30000 },
    );
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

  it("builds an assembly and authors two shots — both, as always, of the object the cutscene hangs on", async () => {
    const parts = [];
    part = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.4, 0] })).created;
    parts.push(part);
    boxes = [];
    for (const p of [[10, 0.5, 0], [-10, 0.5, 0], [6, 0.5, 5], [-6, 0.5, -5], [0, 0.5, 9]]) {
      const id = (await invoke("shape_spawn", { kind: "box", pos: p })).created;
      boxes.push(id);
      parts.push(id);
    }
    hall = await invoke("group_entities", { ids: parts, name: "Assembly Hall" });
    const named = await invoke("cinema_subject_catalog", { id: part, index: null, query: "" });
    partName = named.ownerName;
    hallName = named.candidates.find((c) => c.id === hall)?.name ?? "Assembly Hall";
    note(`[aim] the cutscene hangs on "${partName}" inside "${hallName}" (${parts.length} parts)`);

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(part)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    for (const card of ["establish", "closeup"]) {
      expect(await click(`[data-testid="shot-${card}"]`)).toBe(true);
      await browser.pause(500);
    }
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: part })).shots === 2, {
      timeout: 15000,
      timeoutMsg: "the two shots never landed",
    });
    const before = await invoke("cinema_list", { id: part });
    expect(before.rows.every((r) => r.subject === part)).toBe(true);
    note(`[aim] BEFORE — both shots frame ${before.rows[0].subjectName}`);
  });

  it("the picker offers pointing at it BEFORE naming it, and the badge takes the stage", async () => {
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

    // The first row of the picker, above the search box.
    const aimRow = await text('[data-testid="cutscene-subject-aim"]');
    note(`[aim] the picker's first row reads: ${JSON.stringify(aimRow)}`);
    expect(aimRow).toContain("Click it in the viewport");

    expect(await click('[data-testid="cutscene-subject-aim"]')).toBe(true);
    await (await $('[data-testid="subjectAimBadge"]')).waitForExist({ timeout: 10000 });
    // The picker gets out of the way — the next click belongs to the stage, and a popover over the
    // stage would eat it.
    expect(await present('[data-testid="cutscene-subject-picker"]')).toBe(false);
    const shotLine = await text('[data-testid="subjectAimShot"]');
    note(`[aim] the badge names: ${shotLine}`);
    expect(shotLine).toBe("shot 1 of 2");
    await shot("00_aiming_shot_one");
  });

  it("the badge names what the cursor is over, and the assembly it is part of, with the engine's counts", async () => {
    await invoke("frame_all");
    await browser.pause(900);
    // Constrained to the assembly's own parts: the scene also draws a ground receiver, and a point
    // that happened to land on it would make this a test about the floor.
    const point = await findStagePoint((id) => id === part || boxes.includes(id));
    note(`[aim] the stage has ${point ? `"${point.id}"` : "nothing"} at ${point?.clientX},${point?.clientY}`);
    expect(point).toBeTruthy();

    expect(await stageEvent("pointermove", point.clientX, point.clientY)).toBe(true);
    await browser.waitUntil(async () => (await aimRungs()).length > 0, {
      timeout: 15000,
      timeoutMsg: "hovering the stage never named anything",
    });
    const rungs = await aimRungs();
    for (const rung of rungs) note(`[aim]   rung ${rung.id} — ${rung.text}`);

    // 1. The first rung IS the thing under the cursor — what a click on the stage itself would take.
    expect(rungs[0].id).toBe(point.id);
    // 2. THE LADDER. A pick is a hit test against drawn triangles, so it lands on a LEAF; the shot an
    //    author means is usually the assembly that leaf belongs to, and it is one click, not a search.
    const hallRung = rungs.find((r) => r.id === hall);
    expect(hallRung).toBeTruthy();
    // 3. AND THE NUMBER, which is what tells the two apart when their names do not. It is a question
    //    about the RENDER list and cannot be computed in the editor.
    note(`[aim] "${hallName}" holds ${hallRung.parts} drawn parts; the rung under the cursor holds ${rungs[0].parts}`);
    expect(rungs[0].parts).toBe(1);
    expect(hallRung.parts).toBeGreaterThan(rungs[0].parts);
    expect(hallRung.text).toContain(hallName);
    await shot("01_the_ladder_under_the_cursor");
  });

  it("THE GESTURE: clicking the assembly rung aims shot 1 at it — and the selection never moved", async () => {
    const nativeBefore = await nativeSelection();
    const panelBefore = await panelSelection();
    note(`[aim] before the aim — renderer selection ${nativeBefore}, outliner selection ${panelBefore}`);
    expect(panelBefore).toBe(part);

    expect(await click(`[data-testid="subjectAimRung-${hall}"]`)).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[0].subject === hall,
      { timeout: 15000, timeoutMsg: "the re-aimed subject never reached the document" },
    );

    const after = await invoke("cinema_list", { id: part });
    note(`[aim] AFTER — shot 1: ${after.rows[0].reads}`);
    note(`[aim] AFTER — shot 2: ${after.rows[1].reads}`);
    expect(after.rows[0].subjectName).toBe(hallName);
    expect(after.rows[1].subject).toBe(part);
    expect(after.shots).toBe(2);

    // THE ASSERTION THIS WHOLE MODE EXISTS FOR. `viewport_pick` would have moved both of these, and
    // the Cutscene panel is bound to the second — so a pick here would have switched which cutscene
    // is on screen mid-gesture and silently discarded the shot being aimed.
    const nativeAfter = await nativeSelection();
    const panelAfter = await panelSelection();
    note(`[aim] after the aim — renderer selection ${nativeAfter}, outliner selection ${panelAfter}`);
    expect(nativeAfter).toBe(nativeBefore);
    expect(panelAfter).toBe(panelBefore);

    // The mode ends at the choice, and the panel reads back what it committed.
    expect(await present('[data-testid="subjectAimBadge"]')).toBe(false);
    await browser.waitUntil(async () => (await text('[data-testid="cutscene-subject-name"]')) === hallName, {
      timeout: 10000,
      timeoutMsg: "the Frames control never read back the object the stage had chosen",
    });
    await shot("02_shot_one_frames_the_assembly");
  });

  it("and clicking the STAGE takes what the cursor is over — shot 2 films the part that was clicked", async () => {
    expect(await click('[data-testid="cutscene-clip"]:nth-of-type(2)')).toBe(true);
    await (await $('[data-testid="cutscene-shot-editor"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-subject"]')).toBe(true);
    await (await $('[data-testid="cutscene-subject-picker"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-subject-aim"]')).toBe(true);
    await (await $('[data-testid="subjectAimBadge"]')).waitForExist({ timeout: 10000 });

    // Somewhere with a DIFFERENT part behind it — one of the boxes, not the capsule the cutscene
    // hangs on, so "it changed" cannot be confused with "it was already that".
    const point = await findStagePoint((id) => boxes.includes(id));
    note(`[aim] clicking the stage at ${point?.clientX},${point?.clientY}, which is "${point?.id}"`);
    expect(point).toBeTruthy();

    const nativeBefore = await nativeSelection();
    const panelBefore = await panelSelection();
    expect(await stageEvent("click", point.clientX, point.clientY)).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[1].subject === point.id,
      { timeout: 15000, timeoutMsg: "the click on the stage never aimed shot 2" },
    );
    const after = await invoke("cinema_list", { id: part });
    note(`[aim] shot 2 now reads: ${after.rows[1].reads}`);
    expect(after.shots).toBe(2);
    expect(after.rows[0].subject).toBe(hall);
    // Again: a click that aimed a shot must not also have selected the thing it aimed at.
    expect(await nativeSelection()).toBe(nativeBefore);
    expect(await panelSelection()).toBe(panelBefore);
    await shot("03_shot_two_films_what_was_clicked");
  });

  it("and the CAMERA moved — the wide stands back from the assembly, the close-up sits on the part", async () => {
    const cut = await invoke("cinema_list", { id: part });
    const wide = await poseAt(part, cut.rows[0].openSeconds);
    const close = await poseAt(part, cut.rows[1].openSeconds);
    await invoke("cinema_preview", { id: part, seconds: 0, active: false });
    note(`[aim] shot 1 (${wide.subjectName}): eye ${fmt(wide.eye)} looking at ${fmt(wide.lookAt)}`);
    note(`[aim] shot 2 (${close.subjectName}): eye ${fmt(close.eye)} looking at ${fmt(close.lookAt)}`);

    const wideRange = dist3(wide.eye, wide.lookAt);
    const closeRange = dist3(close.eye, close.lookAt);
    note(`[aim] the wide stands ${wideRange.toFixed(2)} units off; the close-up ${closeRange.toFixed(2)}`);
    // THE ASSERTION NO RELABELLING CAN PASS. `solve_shot` fits the camera to the SUBJECT's bounds, and
    // the assembly's bounds are the union of six parts spread over twenty metres.
    expect(wideRange).toBeGreaterThan(closeRange * 2);
    const aimApart = dist3(wide.lookAt, close.lookAt);
    note(`[aim] the two shots are aimed ${aimApart.toFixed(2)} units apart`);
    expect(aimApart).toBeGreaterThan(0.5);
  });

  it("Escape cancels an aim, and the shot is left framing what it did", async () => {
    const beforeSubject = (await invoke("cinema_list", { id: part })).rows[1].subject;
    expect(await click('[data-testid="cutscene-subject"]')).toBe(true);
    await (await $('[data-testid="cutscene-subject-picker"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-subject-aim"]')).toBe(true);
    await (await $('[data-testid="subjectAimBadge"]')).waitForExist({ timeout: 10000 });

    await browser.execute(() =>
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true })),
    );
    await browser.waitUntil(async () => !(await present('[data-testid="subjectAimBadge"]')), {
      timeout: 10000,
      timeoutMsg: "Escape never took the badge off the stage",
    });
    expect((await invoke("cinema_list", { id: part })).rows[1].subject).toBe(beforeSubject);
    note("[aim] Escape cancelled the aim and the shot kept its subject");
  });

  it("one Ctrl-Z puts each shot back, one step at a time", async () => {
    await invoke("undo");
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[1].subject === part,
      { timeout: 15000, timeoutMsg: "undo never restored shot 2's subject" },
    );
    const once = await invoke("cinema_list", { id: part });
    // ONE step: the click on the stage was one transaction, exactly as the list's own row is.
    expect(once.rows[0].subject).toBe(hall);
    expect(once.shots).toBe(2);

    await invoke("undo");
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[0].subject === part,
      { timeout: 15000, timeoutMsg: "a second undo never restored shot 1's subject" },
    );
    note("[aim] two undos, two shots back on their own object — one edit each");
  });

  it("Play takes the stage back, and an aim in flight goes with it", async () => {
    expect(await click('[data-testid="cutscene-subject"]')).toBe(true);
    await (await $('[data-testid="cutscene-subject-picker"]')).waitForExist({ timeout: 10000 });
    expect(await click('[data-testid="cutscene-subject-aim"]')).toBe(true);
    await (await $('[data-testid="subjectAimBadge"]')).waitForExist({ timeout: 10000 });

    expect(await click('[data-testid="play"]')).toBe(true);
    await browser.waitUntil(async () => !(await present('[data-testid="subjectAimBadge"]')), {
      timeout: 15000,
      timeoutMsg: "Play left the stage intercepting clicks for an edit the engine refuses",
    });
    note("[aim] pressing Play cancelled the aim in flight");
    await click('[data-testid="stop"]');
    await browser.pause(800);
  });
});
