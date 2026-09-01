// **Duplicating what was actually selected — on the packaged `.exe` and the real engine.** (ADR-196)
//
// The vitest suite proves the editor's half: the sentence, and that the copies become the selection.
// It cannot prove any of the three things this change is actually about, because all three live in
// Rust on the other side of the IPC boundary and the dev MockCore duplicates nothing:
//
//   1. **N copies, not one.** `duplicate_selection` is a new command; a front end can call it and a
//      shell that never registered it would answer with an error the caller swallows into a refusal.
//   2. **A duplicated GROUP carries its contents.** The old verb cloned one entity and stopped, and
//      the empty node it produced looked exactly like a full one from the outside — same name, same
//      parent, a fresh id, a success toast. Only the real document knows what is inside.
//   3. **ONE transaction.** A single Ctrl-Z must take the whole duplicate back. N commits look
//      identical to one until you press undo once.
//
// Every gesture is driven through the UI — the real chords on the real window. The only commands
// invoked directly are READS (`selection_ids`, `entity_details`): a read cannot fake a capability,
// and a spec that invoked `duplicate_selection` itself would prove only that the command exists.
//
// Run: node "node_modules\@wdio\cli\bin\wdio.js" run wdio.selecttest.conf.js --spec specs-selecttest/duplicate-selection.e2e.js

import { browser } from "@wdio/globals";
import { mkdirSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dir = path.dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = process.env.MTK_SHOT_DIR || path.resolve(dir, "../.shots-duplicate-selection");
mkdirSync(SHOT_DIR, { recursive: true });

// A DOM capture, deliberately: everything under test here is the outliner and a toast, both of them
// ordinary DOM inside the packaged window. `<visual_acceptance>` 1's rule is about passing a DOM shot
// off as THE RENDER, and no claim here is about rendered pixels.
const shot = async (label) => {
  await browser.pause(400);
  const out = path.join(SHOT_DIR, `${label}.png`);
  await browser.saveScreenshot(out);
  console.log("  shot", out);
};

/** What the ENGINE says is selected — the authority, not the React store's mirror of it. */
const engineSelection = () => browser.execute(() => window.__TAURI__.core.invoke("selection_ids"));

/** The engine's own read of one entity: its name is what proves a copy is distinguishable. */
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

describe("duplicate acts on the whole selection, and carries what is inside it", () => {
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

  it("Ctrl-D over a selection of many makes many, selects the copies, and one Ctrl-Z takes them all back", async () => {
    const before = await sceneCount();
    // Select all, through the chord a user would press — not through `select_entities`.
    await chord("a");
    await browser.waitUntil(async () => (await engineSelection()).length === before, {
      timeout: 15000,
      timeoutMsg: "Ctrl-A never reached the engine",
    });
    const sources = await engineSelection();
    expect(sources.length).toBe(before);

    await chord("d");
    await browser.waitUntil(async () => (await sceneCount()) > before, {
      timeout: 30000,
      timeoutMsg: "Ctrl-D produced nothing — the chord, the command, or the projection never landed",
    });

    const after = await sceneCount();
    console.log("  scene:", before, "→", after, "entities");
    // EVERY selected object, not the primary. This is the assertion the shipped verb could not pass.
    expect(after - before).toBe(sources.length);

    // The copies are the selection, in the ENGINE and not only in the store — so the drag that
    // usually follows a duplicate moves what was just made. The count is the number of TOP-MOST
    // objects, which equals the selection only in a flat scene: a selection holding a parent and its
    // child copies the parent once, and the child comes with it rather than twice.
    const selected = await engineSelection();
    expect(selected.length).toBeGreaterThan(0);
    expect(selected.length).toBeLessThanOrEqual(sources.length);
    expect(selected.some((id) => sources.includes(id))).toBe(false);

    // And they are distinguishable: a copy is named, and named differently from its source.
    const copy = await details(selected[0]);
    expect(copy).toBeTruthy();
    expect(copy.name).toMatch(/copy/i);
    console.log("  a copy is called:", copy.name);
    const status = await statusLine();
    console.log("  status:", status);
    await shot("duplicate-selection-many");

    // ONE transaction: one undo, everything back.
    await chord("z");
    await browser.waitUntil(async () => (await sceneCount()) === before, {
      timeout: 30000,
      timeoutMsg: `one Ctrl-Z did not take the whole duplicate back (still ${await sceneCount()} of ${before})`,
    });
  });

  it("a duplicated GROUP comes back with its contents, not as an empty node", async () => {
    const before = await sceneCount();
    // Build a group through the UI: select everything, then the toolbar's Group row.
    await chord("a");
    await browser.waitUntil(async () => (await engineSelection()).length === before, {
      timeout: 15000,
      timeoutMsg: "Ctrl-A never reached the engine",
    });
    const members = (await engineSelection()).length;

    await browser.execute(() => document.querySelector('[data-testid="authoring-more"]').click());
    await browser.pause(300);
    await browser.execute(() => document.querySelector('[data-testid="authGroup"]').click());
    await browser.waitUntil(async () => (await engineSelection()).length === 1, {
      timeout: 30000,
      timeoutMsg: "Group never landed",
    });
    const [group] = await engineSelection();
    const withGroup = await sceneCount();
    expect(withGroup).toBe(before + 1);

    await chord("d");
    await browser.waitUntil(async () => (await sceneCount()) > withGroup, {
      timeout: 30000,
      timeoutMsg: "duplicating the group produced nothing",
    });
    const after = await sceneCount();
    console.log("  group of", members, "duplicated:", withGroup, "→", after, "entities");

    // THE WHOLE POINT. The old verb added exactly ONE entity here — the empty node — and said
    // "duplicated". The copy has to cost what the original cost.
    expect(after - withGroup).toBe(members + 1);

    const [clone] = await engineSelection();
    expect(clone).not.toBe(group);
    const copy = await details(clone);
    expect(copy.name).toMatch(/copy/i);
    await shot("duplicate-selection-group");

    await chord("z");
    await browser.waitUntil(async () => (await sceneCount()) === withGroup, {
      timeout: 30000,
      timeoutMsg: "one Ctrl-Z did not take the duplicated subtree back",
    });
  });
});
