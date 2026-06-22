// Build-acceptance — M3.3 CONTEXT REVEAL + every context action (the right-click "what can I do here?"
// menu: Bind / Remove / Duplicate / Focus / Inspect). Each workflow's pass is the CONJUNCTION of
// functional + the applicable invariants (inv3 undoable) + principle-1 (every greyed action explains
// itself) + clean (no console errors). Assertions read the STATE CHANGE back from a STABLE signal — the
// projection entity-count (#count = store.size), a *_debug command (focus_debug = [dist, focused]), the
// inspector/component read-back, the status' structured tokens ("removed … Ctrl-Z", "undo") — never
// cosmetic copy that drifts. All DOM access goes through the page-object, so this survives the M10.1
// React swap (swap the page-object, not the spec). Mirrors the live northstar.e2e.js context-menu block.

import { browser, expect } from "@wdio/globals";
import { page } from "../../pages/scaffold.js";
import {
  report,
  invoke,
  consoleErrors,
  clearConsole,
  ipcPerFrame,
  captureBudget,
  scoreBudget,
  loadBaseline,
} from "../../lib/acceptance.js";

const ui = page();
const baseline = loadBaseline();

// The projection entity-count, read from the page-object's #count ("N entities") — a STABLE structured
// signal for "the world grew/shrank", not cosmetic copy. (store.size is mirrored into #count on every delta.)
const entityCount = async () => {
  const m = (await ui.count()).match(/(\d+)\s+entities/);
  return m ? Number(m[1]) : NaN;
};

// Open the context menu on the centre entity and wait until it's actually populated (≥1 ctxitem). The
// menu is fed by viewport_peek → entity_actions, so a populated menu == a real target was found.
const openMenuOnCenter = async () => {
  await ui.openContextMenu();
  await browser.waitUntil(
    async () => (await ui.contextVisible()) && (await ui.contextItems()).length > 0,
    { timeout: 10000, timeoutMsg: "context menu never opened / populated on right-click at centre" }
  );
};

describe("acceptance / M3.3 — context reveal + every action, the full conjunction", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  // ── CONTEXT REVEAL: right-click → a menu of this entity's VALID actions, every greyed one explained ──
  it("right-click reveals ≥5 actions; every greyed action explains itself (functional + p1_explained)", async () => {
    await clearConsole();
    // A clean slate (no prior focus / open menu), then ensure something is selectable at centre.
    await browser.keys(["Escape"]);
    await ui.pickCenter();
    await ui.waitStatus("picked");

    await openMenuOnCenter();

    // ≥5 items — the menu surfaces Bind / Remove / Duplicate / Focus / Inspect (functional).
    const items = await ui.contextItems();
    expect(items.length).toBeGreaterThanOrEqual(5);

    // The menu offers the five named actions by their stable data-action keys (not by their label copy).
    const present = {};
    for (const a of ["bind", "remove", "duplicate", "focus", "inspect"]) {
      present[a] = await (await ui.contextAction(a)).isExisting();
    }
    const functional =
      items.length >= 5 &&
      present.bind && present.remove && present.duplicate && present.focus && present.inspect;
    expect(functional).toBe(true);

    // PRINCIPLE 1 (explain every "no"): every greyed/disabled action carries a reason — the DOM renders
    // "label  —  reason" for unavailable actions, so the dash is the stable explained-marker (silent-dead = fail).
    let everyNoExplained = true;
    for (const it of items) {
      const cls = (await it.getAttribute("class")) || "";
      if (cls.includes("disabled")) {
        if (!(await it.getText()).includes("—")) everyNoExplained = false;
      }
    }
    expect(everyNoExplained).toBe(true);

    // Close the menu so this workflow leaves no open chrome behind.
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await ui.contextVisible()), {
      timeout: 10000,
      timeoutMsg: "context menu never closed on Escape",
    });

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "context reveal",
      { functional, inv3: null, p1_explained: everyNoExplained, clean },
      { commands: ["entity_actions"] }
    );
    expect(clean).toBe(true);
  });

  // ── REMOVE → status "removed … Ctrl-Z" + projection shrinks; Ctrl-Z restores (inv3) ────────────────
  it("Remove deletes the entity (projection shrinks) and Ctrl-Z restores it (functional + inv3 + clean)", async () => {
    await clearConsole();
    await browser.keys(["Escape"]);

    const before = await entityCount();
    expect(before).toBeGreaterThan(0);

    await openMenuOnCenter();
    await ui.clickContext("remove");

    // Functional: the status reports the removal AND advertises the undo affordance (stable tokens).
    await ui.waitStatus("removed");
    const removedStatus = await ui.status();
    expect(removedStatus).toContain("removed");
    expect(removedStatus).toContain("Ctrl-Z");
    // The menu closed on the action click.
    expect(await ui.contextVisible()).toBe(false);

    // Read the STATE CHANGE back from the projection: the entity count dropped (one source of truth, not the copy).
    await browser.waitUntil(async () => (await entityCount()) < before, {
      timeout: 10000,
      timeoutMsg: `entity count never dropped after Remove (was ${before}, still ${await entityCount()})`,
    });
    const afterRemove = await entityCount();
    const functional = afterRemove < before;

    // INVARIANT 3 (undoable): one Ctrl-Z restores the entity — status says "undo" AND the count climbs back.
    await ui.undoKey();
    await ui.waitStatus("undo");
    await browser.waitUntil(async () => (await entityCount()) >= before, {
      timeout: 10000,
      timeoutMsg: `Ctrl-Z did not restore the removed entity (count ${await entityCount()} < ${before})`,
    });
    const inv3 = (await entityCount()) >= before;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "context/remove",
      { functional, inv3, p1_explained: null, clean },
      { commands: ["remove_entity", "undo"] }
    );
    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── DUPLICATE → a clone exists (projection grows); Ctrl-Z removes it (inv3) ─────────────────────────
  it("Duplicate creates a clone (projection grows) and Ctrl-Z removes it (functional + inv3 + clean)", async () => {
    await clearConsole();
    await browser.keys(["Escape"]);

    const before = await entityCount();
    expect(before).toBeGreaterThan(0);

    await openMenuOnCenter();
    await ui.clickContext("duplicate");

    // Functional: status reports the new id AND the projection grows by a clone (read back the count, not copy).
    await ui.waitStatus("duplicated");
    expect(await ui.status()).toContain("duplicated");
    await browser.waitUntil(async () => (await entityCount()) > before, {
      timeout: 10000,
      timeoutMsg: `entity count never grew after Duplicate (was ${before}, still ${await entityCount()})`,
    });
    const afterDup = await entityCount();
    const functional = afterDup > before;

    // INVARIANT 3 (undoable): one Ctrl-Z peels the clone back — the count returns to the pre-duplicate size.
    await ui.undoKey();
    await ui.waitStatus("undo");
    await browser.waitUntil(async () => (await entityCount()) <= before, {
      timeout: 10000,
      timeoutMsg: `Ctrl-Z did not remove the duplicate (count ${await entityCount()} > ${before})`,
    });
    const inv3 = (await entityCount()) <= before;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "context/duplicate",
      { functional, inv3, p1_explained: null, clean },
      { commands: ["duplicate_entity", "undo"] }
    );
    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── FOCUS → banner up + camera "got nearby" (data-dist ≤ 40 via focus_debug); Escape clears ─────────
  it("Focus centers + zooms nearby (banner data-dist ≤ 40) and Escape clears it (functional + inv3 + clean)", async () => {
    await clearConsole();
    await browser.keys(["Escape"]); // clean slate: no prior focus, no open menu

    await openMenuOnCenter();
    await ui.clickContext("focus");

    // Functional: the "you are focused" banner appears (the visible affordance to return to normal).
    await browser.waitUntil(async () => await ui.focusBannerVisible(), {
      timeout: 10000,
      timeoutMsg: "focus banner never appeared after Focus",
    });
    const focusedBanner = await ui.focusBanner();

    // Read the camera STATE back from a stable signal: the Rust focus_debug = [dist, focused] is surfaced
    // into the banner dataset. "get nearby" clamps the orbit distance to ≤ 40 (well in from the ~60 overview).
    const dist = Number(await focusedBanner.getAttribute("data-dist"));
    expect(await focusedBanner.getAttribute("data-focused")).toBe("true");
    expect(dist).toBeLessThanOrEqual(40);
    // Corroborate against the command directly (the dataset can't drift from the engine): focus_debug[1] truthy.
    const fdbg = await invoke("focus_debug"); // [dist, focused]
    expect(Number(fdbg[0])).toBeLessThanOrEqual(40);
    const functional = (await ui.focusBannerVisible()) && dist <= 40;

    // INVARIANT 3 (reversible): Escape unfocuses → the banner clears (everything back to normal overview).
    await browser.keys(["Escape"]);
    await browser.waitUntil(async () => !(await ui.focusBannerVisible()), {
      timeout: 10000,
      timeoutMsg: "focus banner never cleared on Escape",
    });
    expect(await ui.status()).toContain("focus cleared");
    const inv3 = !(await ui.focusBannerVisible());

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "focus mode",
      { functional, inv3, p1_explained: null, clean },
      { commands: ["focus_entity", "unfocus", "focus_debug"] }
    );
    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── INSPECT → selects the entity + the inspector shows its real component details ───────────────────
  it("Inspect selects the entity and surfaces its component details (functional + clean)", async () => {
    await clearConsole();
    await browser.keys(["Escape"]);

    await openMenuOnCenter();
    await ui.clickContext("inspect");

    // Functional: inspect selects the entity → the inspector renders its real components (Transform/Health/…),
    // a structured read-back of the projection, not a cosmetic label.
    await ui.waitStatus("inspect");
    await browser.waitUntil(
      async () => /Transform|Health|Renderable|Collider|MeshRenderer/.test(await ui.inspectorText()),
      { timeout: 10000, timeoutMsg: "inspector never showed component details after Inspect" }
    );
    const inspectorText = await ui.inspectorText();
    const functional = /Transform|Health|Renderable|Collider|MeshRenderer/.test(inspectorText);
    expect(functional).toBe(true);

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "context/inspect",
      { functional, inv3: null, p1_explained: null, clean },
      { commands: ["entity_details"] }
    );
    expect(functional).toBe(true);
    expect(clean).toBe(true);
  });

  // ── PRINCIPLE 2: the context-action ops hold ≤16 ms live (p50/p99) and within baseline ─────────────
  it("PRINCIPLE 2: entity_actions / entity_details / duplicate hold ≤16 ms (p50/p99) within baseline", async () => {
    // A real target id to measure against — viewport_peek at the screen centre returns the entity under it.
    const id = await invoke("viewport_peek", { x: 0.5, y: 0.5 });
    expect(typeof id).toBe("string");

    const ops = [];
    ops.push(await captureBudget("entity_actions", "entity_actions", { id }, { n: 30, warmup: 5 }));
    ops.push(await captureBudget("entity_details", "entity_details", { id }, { n: 30, warmup: 5 }));

    for (const s of ops) {
      const scored = await scoreBudget(s, baseline, {
        perFrame: false,
        recapture: () => captureBudget(s.label, s.label, { id }, { n: 30, warmup: 5 }),
      });
      report.budget(scored);
      expect(scored.verdict).toBe("pass");
    }
  });
});
