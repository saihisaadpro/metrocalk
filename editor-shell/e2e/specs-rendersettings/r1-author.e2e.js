// ADR-190 step 1 — AUTHOR THE FIVE ANSWERS THROUGH THE DIALOG'S OWN CONTROLS, on the packaged `.exe`.
//
// This is the half of ADR-182's owed item 1 that a unit test can reach: the controls exist, they
// commit, and the engine agrees. Step 2 (`r2-after-restart`) is the half it cannot — a SECOND launch
// of the same executable, against the replay log this one left behind.
//
// EVERY ANSWER HERE IS SET BY A GESTURE. The three pickers are `<select>` changes, the name is typed
// and blurred, and each one is asserted by asking the ENGINE what it now holds — never by reading back
// the control that was just set, which would assert that React remembers its own state.
//
// ONE DOCUMENTED EXCEPTION, the same one `specs-render` declares: the destination folder arrives from a
// NATIVE picker, and a WebDriver session cannot dismiss an OS dialog. So the folder is sent through
// `cinema_set_render` — the command the "Choose…" button's own reply lands in — and everything else is
// clicked. What step 2 proves about it is identical either way: the document kept it across a restart.

import { mkdirSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { invoke } from "../pages/scaffold.js";

const dir = path.dirname(fileURLToPath(import.meta.url));
const handoff = path.resolve(dir, "../.shots-rendersettings");
mkdirSync(handoff, { recursive: true });

/** NOT ONE OF THE DEFAULTS, in all five. A test that authored `movie / 24 / 1080 / ""` would pass on an
 *  engine that had thrown the settings away and re-derived them — which is precisely the engine this
 *  ADR replaced. */
export const AUTHORED = {
  format: "sequence",
  fps: 60,
  height: 1440,
  name: "weld-line-master",
  folder: path.join(handoff, "delivery"),
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

/** Change a `<select>` the way the browser does — through the value setter React's onChange listens to. */
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

/** Type into a text field and LEAVE IT — the name commits on blur, not on every keystroke, because
 *  fourteen undoable entries behind one nine-letter name is not an authoring history anybody wants. */
const typeAndBlur = (selector, value) =>
  browser.execute(
    (sel, v) => {
      const el = document.querySelector(sel);
      if (!el) return false;
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(el, v);
      el.dispatchEvent(new Event("input", { bubbles: true }));
      // `focus()` THEN `blur()`, NOT A SYNTHETIC `blur` EVENT. React has delegated `onBlur` to the
      // bubbling `focusout` since 17, and a hand-dispatched `blur` does not bubble — so the first
      // version of this helper set the field, fired nothing React was listening for, and reported
      // "the name never reached the document" about a commit path that works. Asking the element to
      // take and drop focus makes the browser fire the real pair.
      el.focus();
      el.blur();
      return true;
    },
    selector,
    value,
  );

const text = (selector) =>
  browser.execute((sel) => document.querySelector(sel)?.textContent ?? null, selector);

const exists = (selector) => browser.execute((sel) => !!document.querySelector(sel), selector);

const note = (line) => console.log(line);

describe("ADR-190 · what a cut delivers is authored on the cut", () => {
  let subject;

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

  it("authors a cutscene from the shot cards", async () => {
    subject = (await invoke("shape_spawn", { kind: "capsule", pos: [0, 1.6, 0] })).created;
    await invoke("shape_spawn", { kind: "cylinder", pos: [0, 0.2, 0] });
    await invoke("frame_all");
    expect(subject).toBeTruthy();

    await click('[data-testid="engine-gameplay"]');
    await browser.pause(500);
    expect(await selectRow(subject)).toBe(true);
    await (await $('[data-testid="cinema-section"]')).waitForExist({ timeout: 10000 });
    for (const card of ["establish", "hero"]) {
      expect(await click(`[data-testid="shot-${card}"]`)).toBe(true);
      await browser.pause(500);
    }
    await browser.waitUntil(async () => (await invoke("cinema_list", { id: subject })).shots === 2, {
      timeout: 15000,
      timeoutMsg: "the two shots never landed",
    });

    // THE DEFAULTS ARE THE DOCUMENT'S, and a fresh cut already answers all five. Read from the engine
    // BEFORE anything is changed, so the "not the defaults" claim below has a measured baseline rather
    // than an assumed one.
    const fresh = (await invoke("cinema_list", { id: subject })).render;
    note(`[settings] a fresh cut delivers: ${fresh.format} · ${fresh.fps} fps · ${fresh.height} · name ${JSON.stringify(fresh.name)}`);
    expect(fresh.format).toBe("movie");
    expect(fresh.fps).toBe(24);
    expect(fresh.height).toBe(1080);
    expect(fresh.name).toBe("");
    expect(fresh.folder).toBe("");
  });

  it("the Render dialog opens on the document's answers, and every control writes back to it", async () => {
    if (!(await browser.execute(() => !!document.querySelector("#bottom-workspaces-animation-tab")))) {
      await click('[data-testid="bottom-dock-toggle"]');
      await browser.pause(400);
    }
    expect(await click("#bottom-workspaces-animation-tab")).toBe(true);
    await browser.pause(600);
    expect(await clickTabNamed("#bottom-workspaces-animation-panel", "Cutscene")).toBe(true);
    await (await $('[data-testid="cutscene-timeline"]')).waitForExist({ timeout: 15000 });

    expect(await click('[data-testid="cutscene-render"]')).toBe(true);
    await (await $('[data-testid="render-dialog"]')).waitForExist({ timeout: 10000 });

    // ── format ───────────────────────────────────────────────────────────────────────────────────
    expect(await setSelect('[data-testid="render-format"]', AUTHORED.format)).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).render.format === AUTHORED.format,
      { timeout: 10000, timeoutMsg: "the format never reached the document" },
    );

    // ── rate ─────────────────────────────────────────────────────────────────────────────────────
    expect(await setSelect('[data-testid="render-fps"]', String(AUTHORED.fps))).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).render.fps === AUTHORED.fps,
      { timeout: 10000, timeoutMsg: "the rate never reached the document" },
    );

    // ── size ─────────────────────────────────────────────────────────────────────────────────────
    expect(await setSelect('[data-testid="render-size"]', String(AUTHORED.height))).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).render.height === AUTHORED.height,
      { timeout: 10000, timeoutMsg: "the size never reached the document" },
    );

    // ── name ─────────────────────────────────────────────────────────────────────────────────────
    expect(await typeAndBlur('[data-testid="render-stem"]', AUTHORED.name)).toBe(true);
    await browser.waitUntil(
      async () => (await invoke("cinema_list", { id: subject })).render.name === AUTHORED.name,
      { timeout: 10000, timeoutMsg: "the name never reached the document" },
    );

    // ── where (the documented OS-picker exception) ───────────────────────────────────────────────
    const stored = await invoke("cinema_set_render", {
      id: subject,
      format: AUTHORED.format,
      fps: AUTHORED.fps,
      height: AUTHORED.height,
      name: AUTHORED.name,
      folder: AUTHORED.folder,
    });
    expect(stored.reason).toBe(null);

    const held = (await invoke("cinema_list", { id: subject })).render;
    note(`[settings] the cut now delivers: ${held.format} · ${held.fps} fps · ${held.height} · name ${JSON.stringify(held.name)} · into ${held.folder}`);
    expect(held).toEqual(AUTHORED);

    // AND THE DIALOG SAYS WHERE, before the click that writes. The one thing this dialog never stated.
    //
    // THE ROW READS "WILL ASK" HERE, AND THAT IS CORRECT RATHER THAN A MISS. The folder above was
    // stored by the command the "Choose..." button calls, not by the button — WebDriver cannot dismiss
    // a native picker — so the panel that owns `cut` never saw the reply, and this tree has no
    // cross-panel document-changed signal to tell it (see the report's owed list). Through the button
    // the reply comes back to `onSettingsSaved` and the row updates; the assertion that the STORED
    // path reaches this row is made in step 2, where the dialog opens on it from a cold document.
    const where = await text('[data-testid="render-folder"]');
    note(`[settings] the dialog's "Where" row reads: ${where}`);
    expect(where).toContain("asked when you render");
  });

  it("and one Ctrl-Z takes the last answer back, because these are ordinary document edits", async () => {
    // THE PROOF THAT THIS IS NOT A PREFERENCE. A settings store would have no undo; these are commits,
    // so the engine's own undo reaches them — which is also why the panel raises "Ctrl-Z to undo"
    // when a picker changes.
    //
    // IN THIS SESSION AND NOT AFTER THE RESTART. An undo stack does not survive a relaunch: the
    // document in step 2 is rebuilt by REPLAY, with nothing behind it to step back into, so an undo
    // assertion over there would be asserting the wrong thing about the right feature.
    const before = (await invoke("cinema_list", { id: subject })).render;
    await invoke("undo");
    await browser.pause(400);
    const after = (await invoke("cinema_list", { id: subject })).render;
    note(`[settings] undo moved the folder from ${JSON.stringify(before.folder)} to ${JSON.stringify(after.folder)}`);
    expect(after).not.toEqual(before);
    await invoke("redo");
    await browser.pause(400);
    expect((await invoke("cinema_list", { id: subject })).render).toEqual(before);
  });

  it("refuses the one pair it cannot store, and leaves the document as it was", async () => {
    // A MOVIE HAS ONE SIZE FOR ITS WHOLE LENGTH. The engine refuses `(movie, "as on screen")` at the
    // point it would be STORED, not at the point it would be rendered — a setting that can only ever
    // produce a refusal is one the author finds again tomorrow, still broken, with nothing said.
    const refused = await invoke("cinema_set_render", {
      id: subject,
      format: "movie",
      fps: AUTHORED.fps,
      height: null,
      name: AUTHORED.name,
      folder: AUTHORED.folder,
    });
    note(`[settings] movie + as-on-screen: ${refused.reason}`);
    expect(refused.reason).toContain("one size for its whole length");
    expect((await invoke("cinema_list", { id: subject })).render).toEqual(AUTHORED);

    // ...and a rate nobody offers, said WITH the rates that are offered.
    const badRate = await invoke("cinema_set_render", {
      id: subject,
      format: AUTHORED.format,
      fps: 100,
      height: AUTHORED.height,
      name: AUTHORED.name,
      folder: AUTHORED.folder,
    });
    note(`[settings] 100 fps: ${badRate.reason}`);
    expect(badRate.reason).toContain("24, 25, 30, 60");
    expect((await invoke("cinema_list", { id: subject })).render).toEqual(AUTHORED);

    expect(await click('[data-testid="render-cancel"]')).toBe(true);
    await browser.pause(300);
    expect(await exists('[data-testid="render-dialog"]')).toBe(false);
  });

  it("hands the next launch the object it must ask about", async () => {
    writeFileSync(
      path.join(handoff, "authored.json"),
      JSON.stringify({ subject, authored: AUTHORED }, null, 2),
      "utf8",
    );
    note(`[settings] step 2 will reopen ${subject} and expect ${JSON.stringify(AUTHORED)}`);
    expect(subject).toBeTruthy();
  });
});
