// WHAT A SHOT FRAMES — on the PACKAGED .exe, driven through the UI.
//
// BEFORE: `ShotRecipe.subject` has existed since cutscenes shipped. The runtime resolves it as the
// union of every rendered instance in that object's HIERARCHY SUBTREE, `cinema_add_shot` has always
// taken a subject, and every row the shot list sends back carries its own `subjectName`. The editor
// sent no subject and had no command to change one — so in practice every shot filmed the object its
// cutscene hung on, and the most ordinary cinematic sequence there is, "hold on the whole assembly,
// then cut in to the one machine", could not be authored at all.
//
// AFTER: the shot inspector's Frames control opens the scene's own hierarchy, ranked by the engine —
// this object, what it is part of, what it is made of, what stands beside it — each row carrying how
// many DRAWN parts sit under it, which is the number that tells an assembly apart from the bracket
// inside it. `cinema_set_shot_subject` commits the choice as one undoable edit.
//
// THE PROOF IS NOT THAT A LABEL CHANGED. It is that the CAMERA moved: the same preview that Play
// films is asked where it stands for each shot, and a wide of a six-part assembly must stand further
// back and look at a different point than a close-up of one part inside it. Those are numbers off the
// live solver, and no amount of relabelling produces them.

import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, mkdirSync, rmSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const shots = path.resolve(dir, "../.shots-subjectpicker");
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
  if (!ok) note(`[subject] CAPTURE UNAVAILABLE for ${label} — the desktop refused both paths`);
  else note(`[subject] captured ${path.basename(out)} (${statSync(out).size} bytes)`);
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

/** Every row the open picker is drawing, as the user reads it: heading, name and the parts count.
 *
 *  Read off the DOM rather than off a second `cinema_subject_catalog` call, because the claim under
 *  test is what the PANEL shows. A picker that fetched the right list and drew four rows of it would
 *  pass an assertion made against the reply. */
const pickerRows = () =>
  browser.execute(() =>
    [...document.querySelectorAll(".cutscene-subject-option")].map((el) => ({
      testid: el.getAttribute("data-testid") ?? "",
      parts: Number(el.getAttribute("data-parts")),
      checked: el.getAttribute("aria-checked") === "true",
      text: (el.textContent ?? "").replace(/\s+/g, " ").trim(),
    })),
  );

/** The headings the list is grouped under, in the order they are drawn. */
const pickerGroups = () =>
  browser.execute(() =>
    [...document.querySelectorAll('[data-testid="cutscene-subject-list"] .mtk-popup-menu__group-label')].map(
      (el) => (el.textContent ?? "").trim(),
    ),
  );

/** Type into the picker's search box the way a person does, so React's onChange runs. */
const typeSearch = (value) =>
  browser.execute((v) => {
    const el = document.querySelector('[data-testid="cutscene-subject-search"]');
    if (!el) return false;
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
    setter?.call(el, v);
    el.dispatchEvent(new Event("input", { bubbles: true }));
    return true;
  }, value);

const dist3 = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
const fmt = (v) => `[${v.map((n) => n.toFixed(2)).join(", ")}]`;

/** Stand the preview at one moment and report the pose the SOLVER produced there. The same call the
 *  panel's Preview toggle makes, and the same function Play runs each tick. */
const poseAt = (id, seconds) => invoke("cinema_preview", { id, seconds, active: true });

describe("What a shot frames", () => {
  let hall; // the assembly — six parts under one group node
  let part; // one machine inside it, and the object the cutscene hangs on
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
    // The first-run card mounts AFTER the project does; one click before it exists dismisses nothing.
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

  it("builds an assembly — a group of parts, the shape every imported CAD scene has", async () => {
    // Spread WIDE and low, like a production line: an assembly whose bounds are much larger than any
    // one part is what makes "a wide of the whole thing" a different camera from "a close of one".
    const parts = [];
    part = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.4, 0] })).created;
    parts.push(part);
    for (const p of [[10, 0.5, 0], [-10, 0.5, 0], [6, 0.5, 5], [-6, 0.5, -5], [0, 0.5, 9]]) {
      parts.push((await invoke("shape_spawn", { kind: "box", pos: p })).created);
    }
    hall = await invoke("group_entities", { ids: parts, name: "Assembly Hall" });
    note(`[subject] grouped ${parts.length} parts under ${hall}`);
    expect(hall).toBeTruthy();

    const named = await invoke("cinema_subject_catalog", { id: part, index: null, query: "" });
    partName = named.ownerName;
    hallName = named.candidates.find((c) => c.id === hall)?.name ?? "Assembly Hall";
    note(`[subject] the cutscene will hang on "${partName}" inside "${hallName}"`);
  });

  it("authors two shots — both, as always, of the object the cutscene hangs on", async () => {
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

    // THE STATE THIS FEATURE EXISTS TO CHANGE. Both shots film the part, because until now there was
    // no other answer an editor could give.
    const before = await invoke("cinema_list", { id: part });
    note(`[subject] BEFORE — shot 1 frames ${before.rows[0].subjectName}, shot 2 frames ${before.rows[1].subjectName}`);
    expect(before.rows.every((r) => r.subject === part)).toBe(true);
  });

  it("opens the picker and finds the scene's own hierarchy in it, with the parts count that decides", async () => {
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      expect(await click('[data-testid="bottom-dock-toggle"]')).toBe(true);
      await browser.pause(500);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    // Open shot 1 — the establishing wide, and the one that should be of the whole assembly.
    expect(await click('[data-testid="cutscene-clip"]')).toBe(true);
    await (await $('[data-testid="cutscene-shot-editor"]')).waitForExist({ timeout: 10000 });
    expect(await text('[data-testid="cutscene-subject-name"]')).toBe(partName);
    await shot("00_shot_open_framing_its_owner");

    expect(await click('[data-testid="cutscene-subject"]')).toBe(true);
    await (await $('[data-testid="cutscene-subject-picker"]')).waitForExist({ timeout: 10000 });
    await browser.waitUntil(async () => (await pickerRows()).length > 1, {
      timeout: 15000,
      timeoutMsg: "the picker never listed the scene",
    });

    const groups = await pickerGroups();
    const rows = await pickerRows();
    note(`[subject] the picker is grouped under: ${groups.join(" · ")}`);
    for (const row of rows) note(`[subject]   ${row.testid} — ${row.text}`);

    // 1. The ranking is the scene's own hierarchy, not an alphabetical dump of it.
    expect(groups).toContain("This object");
    expect(groups).toContain("What it is part of");
    // 2. The assembly is on it, one row from a shot of the part inside it.
    const hallRow = rows.find((r) => r.testid.endsWith(hall));
    expect(hallRow).toBeTruthy();
    // 3. AND THE NUMBER. The assembly has every part under it; the part has one. This is the fact
    //    that cannot be computed in the editor — it is a question about the RENDER list — and it is
    //    what tells two similarly-named objects apart in a 15,711-part import.
    const partRow = rows.find((r) => r.testid.endsWith(part));
    note(`[subject] "${hallName}" holds ${hallRow.parts} drawn parts; "${partName}" holds ${partRow.parts}`);
    expect(hallRow.parts).toBeGreaterThan(partRow.parts);
    expect(partRow.parts).toBe(1);
    // 4. What the shot films right now is ticked, so the list says where you are before you move.
    expect(partRow.checked).toBe(true);
    await shot("01_picker_open_on_the_hierarchy");
  });

  it("searches the whole scene by name when the ranked list is not enough", async () => {
    expect(await typeSearch(hallName)).toBe(true);
    await browser.waitUntil(
      async () => {
        const groups = await pickerGroups();
        return groups.length > 0 && groups.every((g) => g === "Matches");
      },
      { timeout: 15000, timeoutMsg: "the search never replaced the ranked list" },
    );
    const hits = await pickerRows();
    note(`[subject] searching "${hallName}" returns ${hits.length}: ${hits.map((h) => h.text).join(" | ")}`);
    expect(hits.some((h) => h.testid.endsWith(hall))).toBe(true);
    await shot("02_search_by_name");

    // Back to the ranked list, so the click that follows is the one a person would make.
    expect(await typeSearch("")).toBe(true);
    await browser.waitUntil(async () => (await pickerGroups()).includes("This object"), {
      timeout: 15000,
      timeoutMsg: "clearing the search never restored the ranked list",
    });
  });

  it("THE SEQUENCE THAT COULD NOT BE AUTHORED: shot 1 becomes a wide of the whole assembly", async () => {
    expect(await click(`[data-testid="cutscene-subject-option-${hall}"]`)).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[0].subject === hall,
      { timeout: 15000, timeoutMsg: "the re-aimed subject never reached the document" },
    );

    const after = await invoke("cinema_list", { id: part });
    note(`[subject] AFTER — shot 1: ${after.rows[0].reads}`);
    note(`[subject] AFTER — shot 2: ${after.rows[1].reads}`);
    // The cut is otherwise untouched: shot 2 still films the part, and neither shot lost its length,
    // its place or its framing.
    expect(after.rows[0].subjectName).toBe(hallName);
    expect(after.rows[0].reads).toContain(hallName);
    expect(after.rows[1].subject).toBe(part);
    expect(after.shots).toBe(2);
    // ...and the closed control, and the lane, now say what shot 1 films.
    await browser.waitUntil(async () => (await text('[data-testid="cutscene-subject-name"]')) === hallName, {
      timeout: 10000,
      timeoutMsg: "the picker never read back the object it had just chosen",
    });
    const lane = await text('[data-testid="cutscene-timeline"]');
    expect(lane).toContain(hallName);
    await shot("03_shot_one_frames_the_assembly");
  });

  it("and the CAMERA moved — the wide stands back from the assembly, the close-up sits on the part", async () => {
    const cut = await invoke("cinema_list", { id: part });
    const wide = await poseAt(part, cut.rows[0].openSeconds);
    note(`[subject] shot 1 (${wide.subjectName}): eye ${fmt(wide.eye)} looking at ${fmt(wide.lookAt)}`);
    const close = await poseAt(part, cut.rows[1].openSeconds);
    note(`[subject] shot 2 (${close.subjectName}): eye ${fmt(close.eye)} looking at ${fmt(close.lookAt)}`);
    await invoke("cinema_preview", { id: part, seconds: 0, active: false });

    // The preview reports which object each frame is OF, resolved per shot.
    expect(wide.subjectName).toBe(hallName);
    expect(close.subjectName).toBe(partName);

    // THE ASSERTION NO RELABELLING CAN PASS. `solve_shot` fits the camera to the SUBJECT's bounds,
    // and the assembly's bounds are the union of six parts spread over twenty metres. A wide of it
    // must stand much further from what it is aimed at than a close-up of the one capsule at the
    // origin — and it must be aimed somewhere else, because the assembly's centre is not the part's.
    const wideRange = dist3(wide.eye, wide.lookAt);
    const closeRange = dist3(close.eye, close.lookAt);
    note(`[subject] the wide stands ${wideRange.toFixed(2)} units off; the close-up ${closeRange.toFixed(2)}`);
    expect(wideRange).toBeGreaterThan(closeRange * 2);
    const aimApart = dist3(wide.lookAt, close.lookAt);
    note(`[subject] the two shots are aimed ${aimApart.toFixed(2)} units apart`);
    expect(aimApart).toBeGreaterThan(0.5);
  });

  it("one Ctrl-Z puts the shot back on its own object, and one redo takes it away again", async () => {
    await invoke("undo");
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[0].subject === part,
      { timeout: 15000, timeoutMsg: "undo never restored the original subject" },
    );
    const undone = await invoke("cinema_list", { id: part });
    note(`[subject] after undo — shot 1 frames ${undone.rows[0].subjectName}`);
    // ONE undo step, not two: re-aiming must not also have rewritten the length or the framing.
    expect(undone.shots).toBe(2);
    expect(undone.rows[0].seconds).toBeCloseTo(2.5, 1);

    await invoke("redo");
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: part })).rows[0].subject === hall,
      { timeout: 15000, timeoutMsg: "redo never re-applied the subject" },
    );
    note("[subject] redo re-aims it at the assembly again");
  });

  it("refuses to re-aim during Play, and says so where the control is", async () => {
    await click('[data-testid="play"]');
    await browser.pause(1200);
    const state = await browser.execute(() => {
      const el = document.querySelector('[data-testid="cutscene-subject"]');
      return el ? { disabled: el.hasAttribute("disabled"), title: el.getAttribute("title") } : null;
    });
    note(`[subject] during Play the control reads: ${JSON.stringify(state)}`);
    expect(state?.disabled).toBe(true);
    expect(state?.title).toMatch(/Stop Play first/i);
    // ...and the engine refuses it too, so the disabled control is a courtesy and not the guard.
    const refused = await invoke("cinema_set_shot_subject", { id: part, index: 1, subject: hall });
    note(`[subject] the engine refuses with: ${refused.reason}`);
    expect(refused.reason).toMatch(/stop Play first/i);
    await click('[data-testid="stop"]');
    await browser.pause(800);
    expect((await invoke("cinema_list", { id: part })).rows[1].subject).toBe(part);
  });

  it("the re-aimed shot survives Save, New and Open", async () => {
    const file = path.resolve(
      path.dirname(path.resolve(dir, "../../src-tauri/target/release/metrocalk-editor-shell.exe")),
      "subject-picker.mtk",
    );
    await invoke("save_project", { path: file });
    await browser.waitUntil(async () => existsSync(file), {
      timeout: 20000,
      timeoutMsg: "no .mtk was written",
    });
    await invoke("new_project");
    await browser.pause(800);
    expect((await invoke("cinema_list", { id: part })).shots).toBe(0);

    await invoke("open_project", { path: file });
    await browser.pause(1500);
    const reopened = await invoke("cinema_list", { id: part });
    note(
      `[subject] after reopen: ${reopened.shots} shots — 1 frames ${reopened.rows[0].subjectName},` +
        ` 2 frames ${reopened.rows[1].subjectName}`,
    );
    // THE FAILURE THIS CLOSES. Before the log carried a re-aim, a reopened project replayed every
    // shot onto its owner: the shot count was unchanged, the sentences were plausible, and the
    // establishing wide of the assembly had quietly become a second shot of the one part.
    expect(reopened.shots).toBe(2);
    expect(reopened.rows[0].subject).toBe(hall);
    expect(reopened.rows[1].subject).toBe(part);
    await shot("04_reopened_still_frames_the_assembly");
  });
});
