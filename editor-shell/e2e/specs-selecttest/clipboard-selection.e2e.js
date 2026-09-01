// **Copy, cut and paste over what was actually selected — on the packaged `.exe` and the real engine.**
// (ADR-198)
//
// The vitest suite proves the editor's half: the sentences, the clipboard label, and that a cut clears
// the selection while a paste takes it. It cannot prove any of what this change is actually about,
// because all of it lives in Rust behind the IPC boundary and the dev MockCore has no clipboard at all:
//
//   1. **N objects on the clipboard, not one.** `copy_selection` / `cut_selection` are new commands
//      over a new `Clipboard` model; a front end can call anything, and a shell that never registered
//      them answers with an error every caller swallows into a refusal.
//   2. **A pasted copy keeps its CAPABILITIES.** A `Composition` carries components and not capability
//      pairs, so every paste before this arrived with no `Requires`/`Provides` — the M3.1 reveal, the
//      product's headline gesture, had nothing to offer for it. `entity_details` is the engine's own
//      read of those pairs, and only the real document has them.
//   3. **A pasted copy keeps its NAME**, because `save_composition` stopped dropping the whole
//      `__meta__` component. A nameless paste and a named one look identical from the front end, which
//      substitutes the id when a name is missing.
//   4. **A CUT and a PASTE is a MOVE** — the object comes back under its own name, not as `… copy`.
//   5. **ONE transaction.** A single Ctrl-Z must take a whole paste back. N commits look exactly like
//      one until you press undo once.
//
// Every gesture is driven through the UI — the real chords on the real window. The only commands
// invoked directly are READS (`selection_ids`, `entity_details`): a read cannot fake a capability, and
// a spec that invoked `paste_clipboard` itself would prove only that the command exists.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.selecttest.conf.js --spec specs-selecttest/clipboard-selection.e2e.js

import { browser } from "@wdio/globals";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-clipboard-selection");
mkdirSync(SHOT_DIR, { recursive: true });

// A DOM capture, deliberately: everything under test here is the outliner, the toolbar and a status
// line, all of them ordinary DOM inside the packaged window. `<visual_acceptance>` 1's rule is about
// passing a DOM shot off as THE RENDER, and no claim here is about rendered pixels.
const shot = async (label) => {
  await browser.pause(400);
  const out = path.join(SHOT_DIR, `${label}.png`);
  await browser.saveScreenshot(out);
  console.log("  shot", out);
};

/** What the ENGINE says is selected — the authority, not the React store's mirror of it. */
const engineSelection = () => browser.execute(() => window.__TAURI__.core.invoke("selection_ids"));

/** The engine's own read of one entity: its name, and the capability pairs a paste used to drop. */
const details = (id) => browser.execute((i) => window.__TAURI__.core.invoke("entity_details", { id: i }), id);

/** How many objects the scene holds, as the outliner publishes it (the list is virtualized — the row
 *  count is what fits on screen, not what exists). */
const sceneCount = () =>
  browser.execute(() => Number(document.getElementById("count")?.getAttribute("data-entities") ?? -1));

const chord = (key, opts = {}) =>
  browser.execute(
    (k, o) =>
      window.dispatchEvent(
        new KeyboardEvent("keydown", { key: k, ctrlKey: true, bubbles: true, cancelable: true, ...o }),
      ),
    key,
    opts,
  );

const statusLine = () =>
  browser.execute(() => document.querySelector('[data-testid="status"]')?.textContent ?? null);

/** Select every object through the chord a user would press, and wait for the ENGINE to agree. */
async function selectAll(expected) {
  await chord("a");
  await browser.waitUntil(async () => (await engineSelection()).length === expected, {
    timeout: 15000,
    timeoutMsg: "Ctrl-A never reached the engine",
  });
  return engineSelection();
}

describe("the clipboard takes the whole selection, and gives it back intact", () => {
  before(async () => {
    await browser.waitUntil(
      async () => (await browser.execute(() => !!(window.__TAURI__ && window.__TAURI__.core))) === true,
      { timeout: 30000, timeoutMsg: "TAURI bridge never appeared" },
    );
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
    });
    await browser.refresh();
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelectorAll('[data-testid="hrow"]').length)) > 4,
      { timeout: 30000, timeoutMsg: "the seeded scene never reached the outliner" },
    );
  });

  it("Ctrl-C over a selection of many, then Ctrl-V, makes many — named, bindable, and one Ctrl-Z back", async () => {
    const before = await sceneCount();
    const sources = await selectAll(before);

    await chord("c");
    await browser.waitUntil(async () => /copied/i.test((await statusLine()) ?? ""), {
      timeout: 15000,
      timeoutMsg: "Ctrl-C said nothing — the chord, the command, or the status line never landed",
    });
    console.log("  copy said:", await statusLine());
    // A COPY IS A READ. Nothing may have changed in the document, and the selection it was made over
    // is still the selection.
    expect(await sceneCount()).toBe(before);
    expect((await engineSelection()).length).toBe(sources.length);

    await chord("v");
    await browser.waitUntil(async () => (await sceneCount()) > before, {
      timeout: 30000,
      timeoutMsg: "Ctrl-V produced nothing — the chord, the command, or the projection never landed",
    });
    const after = await sceneCount();
    console.log("  scene:", before, "->", after, "entities");
    console.log("  paste said:", await statusLine());
    // EVERY object that was selected, not the primary. This is the assertion the shipped verb could
    // not pass: the clipboard held ONE `Composition`.
    expect(after - before).toBe(sources.length);

    // The pasted objects are the selection, in the ENGINE and not only in the store.
    const pasted = await engineSelection();
    expect(pasted.length).toBeGreaterThan(0);
    expect(pasted.some((id) => sources.includes(id))).toBe(false);

    // NAMED — `save_composition` dropped the whole `__meta__` component before this, so a pasted
    // object came back with no name at all and the outliner drew its raw loro key.
    const copy = await details(pasted[0]);
    expect(copy).toBeTruthy();
    expect(copy.name).toBeTruthy();
    expect(copy.name).not.toBe(pasted[0]);
    console.log("  a pasted copy is called:", copy.name);

    // BINDABLE — the silent one. A `Composition` carries components and not pairs, so every pasted
    // object arrived with nothing to bind. At least one member of a seeded scene requires or provides
    // something; if none of the copies do, the pairs were dropped on the way through.
    const withCaps = [];
    for (const id of pasted.slice(0, 12)) {
      const d = await details(id);
      if (d && ((d.requires && d.requires.length) || (d.provides && d.provides.length))) withCaps.push(d);
    }
    console.log("  pasted objects carrying capabilities:", withCaps.length, "of", Math.min(12, pasted.length));
    expect(withCaps.length).toBeGreaterThan(0);
    await shot("clipboard-paste-many");

    // ONE transaction: one undo, everything back.
    await chord("z");
    await browser.waitUntil(async () => (await sceneCount()) === before, {
      timeout: 30000,
      timeoutMsg: `one Ctrl-Z did not take the whole paste back (still ${await sceneCount()} of ${before})`,
    });
  });

  it("pasting twice makes two sets, and the second is told apart from the first", async () => {
    const before = await sceneCount();
    const sources = await selectAll(before);
    await chord("c");
    await browser.waitUntil(async () => /copied/i.test((await statusLine()) ?? ""), {
      timeout: 15000,
      timeoutMsg: "Ctrl-C never landed",
    });

    await chord("v");
    await browser.waitUntil(async () => (await sceneCount()) === before + sources.length, {
      timeout: 30000,
      timeoutMsg: "the first paste never landed",
    });
    const first = await details((await engineSelection())[0]);

    await chord("v");
    await browser.waitUntil(async () => (await sceneCount()) === before + 2 * sources.length, {
      timeout: 30000,
      timeoutMsg: "the second paste added nothing — repeat-paste is what a clipboard is FOR",
    });
    const second = await details((await engineSelection())[0]);

    console.log("  two pastes:", first.name, "/", second.name);
    // Two objects, not two in one place. The offset is sized from the clipboard's own extent, so
    // without a step every paste of the same clipboard lands exactly on the last one; the names carry
    // the same fact into the outliner, where a person meets it.
    expect(second.name).not.toBe(first.name);
    await shot("clipboard-paste-twice");

    await chord("z");
    await chord("z");
    await browser.waitUntil(async () => (await sceneCount()) === before, {
      timeout: 30000,
      timeoutMsg: "two undos did not take two pastes back",
    });
  });

  it("Ctrl-X then Ctrl-V is a MOVE: the object comes back under its own name", async () => {
    const before = await sceneCount();
    const sources = await selectAll(before);
    const named = await details(sources[sources.length - 1]);

    await chord("x");
    await browser.waitUntil(async () => /^cut /i.test((await statusLine()) ?? ""), {
      timeout: 30000,
      timeoutMsg: "Ctrl-X said nothing",
    });
    console.log("  cut said:", await statusLine());
    // A cut CLEARS the selection, because what it was pointing at is gone from view.
    expect((await engineSelection()).length).toBe(0);
    await shot("clipboard-cut");

    await chord("v");
    await browser.waitUntil(async () => (await engineSelection()).length > 0, {
      timeout: 30000,
      timeoutMsg: "pasting the cut produced nothing",
    });
    const back = await engineSelection();
    const names = [];
    for (const id of back.slice(0, 12)) names.push((await details(id)).name);
    console.log("  pasted back as:", names.slice(0, 4).join(" · "));
    // THE MOVE. A cut source is deactivated, so nothing live is holding its name — the object comes
    // back as itself rather than as a copy of itself. This is one rule with the placement: a cut
    // pastes at step 0, exactly where it was.
    expect(names).toContain(named.name);
    expect(names.every((n) => /copy/i.test(n))).toBe(false);
    await shot("clipboard-cut-pasted-back");
  });
});
