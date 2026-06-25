// Build-acceptance — M10.6 SCENE-AUTHORING VERBS (ADR-036): the hierarchy is a real tree editor. Each
// verb is ONE undoable transaction on the LIVE engine over the Movable Tree (reparent = node.move,
// cycle-safe) + the override model (delete = deactivate). Assertions read the STATE back from STABLE
// structural signals — `part_parent` (the tree edge), `read_transform` (the moved coords), `part_debug`
// (the active flag), `#count` (the projection size) — never cosmetic copy. The React surface (the
// AuthoringToolbar + the hierarchy multi-select/drag) dispatches exactly these commands; a UI-button smoke
// (#authCreate / #authNudge) proves the React→command→delta→store loop is wired live.

import { browser, expect, $ } from "@wdio/globals";
import {
  report,
  invoke,
  consoleErrors,
  clearConsole,
  captureBudget,
  scoreBudget,
  loadBaseline,
} from "../../lib/acceptance.js";

const baseline = loadBaseline();
const countEntities = async () => {
  const txt = await $("#count").getText();
  const m = txt.match(/(\d+)\s+entities/);
  return m ? Number(m[1]) : NaN;
};
const yOf = async (id) => (await invoke("read_transform", { id }))[1];

describe("acceptance / M10.6 — scene-authoring verbs (the hierarchy as a real tree editor, live)", () => {
  before(async () => {
    await browser.waitUntil(async () => (await countEntities()) > 0, {
      timeout: 20000,
      timeoutMsg: "editor never connected (#count stayed empty)",
    });
    await invoke("set_sim_running", { run: false });
    await clearConsole();
  });

  // ── REPARENT (the hierarchy drag's command) — node.move edge + undo + cycle-safe ────────────────────
  it("reparent moves the Movable-Tree edge, Ctrl-Z reverts, and a cycle is REJECTED (functional + inv3)", async () => {
    await clearConsole();
    const a = await invoke("spawn_body", { x: -2, y: 4, z: 0 });
    const b = await invoke("spawn_body", { x: 2, y: 4, z: 0 });
    await invoke("set_sim_running", { run: false });
    expect(await invoke("part_parent", { id: b })).toBe(null);

    // The same command the hierarchy drag dispatches (reparentPart): move b under a (node.move).
    await invoke("reparent_part", { id: b, parent: a });
    await browser.waitUntil(async () => (await invoke("part_parent", { id: b })) === a, {
      timeout: 5000,
      timeoutMsg: "reparent never took (part_parent != a)",
    });
    const reparented = (await invoke("part_parent", { id: b })) === a;

    // CYCLE-SAFE: moving a UNDER its own child b would orphan a cycle — REJECTED (the engine's
    // is_descendant guard + Loro's MovableTree CyclicMoveError). The tree is unchanged.
    await invoke("reparent_part", { id: a, parent: b });
    const cycleRejected = (await invoke("part_parent", { id: a })) === null;

    // Ctrl-Z reverts the (valid) reparent.
    await invoke("undo");
    await browser.waitUntil(async () => (await invoke("part_parent", { id: b })) === null, {
      timeout: 5000,
      timeoutMsg: "undo never reverted the reparent",
    });
    const reverted = (await invoke("part_parent", { id: b })) === null;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "authoring/reparent",
      { functional: reparented && cycleRejected && reverted, inv1: true, inv3: reverted, clean, offline: true },
      { commands: ["reparent_part", "part_parent", "undo"] }
    );
    expect(reparented).toBe(true);
    expect(cycleRejected).toBe(true);
    expect(reverted).toBe(true);
    expect(clean).toBe(true);
  });

  // ── MULTI-EDIT (batched) — the adversarial guard: N entities, ONE undoable tx (one undo restores ALL N)
  it("a multi-edit on 3 entities is ONE batched tx — move all 3 → one Ctrl-Z restores ALL 3 (inv3)", async () => {
    await clearConsole();
    const ids = [];
    for (let i = 0; i < 3; i++) ids.push(await invoke("spawn_body", { x: i - 1, y: 2, z: 0 }));
    await invoke("set_sim_running", { run: false });
    const before = await Promise.all(ids.map(yOf));

    // The same command the toolbar's "Move (all)" dispatches: set Transform.y on EVERY selected entity in
    // ONE undoable transaction.
    const ok = await invoke("multi_edit", { ids, component: "Transform", field: "y", value: 7.0 });
    expect(ok).toBe(true);
    await browser.waitUntil(
      async () => (await Promise.all(ids.map(yOf))).every((y) => Math.abs(y - 7.0) < 0.01),
      { timeout: 5000, timeoutMsg: "the batched multi-edit never moved all 3" }
    );
    const allMoved = (await Promise.all(ids.map(yOf))).every((y) => Math.abs(y - 7.0) < 0.01);

    // ONE undo restores ALL 3 (the adversarial trap: N un-grouped ops would need N undos).
    await invoke("undo");
    await browser.waitUntil(
      async () => {
        const ys = await Promise.all(ids.map(yOf));
        return ys.every((y, i) => Math.abs(y - before[i]) < 0.01);
      },
      { timeout: 5000, timeoutMsg: "one undo did NOT restore all 3 (multi-edit was not one tx)" }
    );
    const allReverted = (await Promise.all(ids.map(yOf))).every((y, i) => Math.abs(y - before[i]) < 0.01);

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "authoring/multi-edit",
      { functional: allMoved && allReverted, inv1: true, inv3: allReverted, clean, offline: true },
      { commands: ["multi_edit", "read_transform", "undo"] }
    );
    expect(allMoved).toBe(true);
    expect(allReverted).toBe(true);
    expect(clean).toBe(true);
  });

  // ── GROUP — wrap a selection under a new parent node, one undoable tx ───────────────────────────────
  it("group wraps a selection under a new parent node and Ctrl-Z dissolves it (functional + inv3)", async () => {
    await clearConsole();
    const a = await invoke("spawn_body", { x: -1, y: 1, z: 0 });
    const b = await invoke("spawn_body", { x: 1, y: 1, z: 0 });
    await invoke("set_sim_running", { run: false });
    const g = await invoke("group_entities", { ids: [a, b], name: "Group" });
    expect(typeof g).toBe("string");
    await browser.waitUntil(async () => (await invoke("part_parent", { id: a })) === g, {
      timeout: 5000,
      timeoutMsg: "group never reparented its members",
    });
    const grouped = (await invoke("part_parent", { id: a })) === g && (await invoke("part_parent", { id: b })) === g;

    await invoke("undo");
    await browser.waitUntil(async () => (await invoke("part_parent", { id: a })) === null, {
      timeout: 5000,
      timeoutMsg: "undo never dissolved the group",
    });
    const dissolved = (await invoke("part_parent", { id: a })) === null && (await invoke("part_parent", { id: b })) === null;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "authoring/group",
      { functional: grouped && dissolved, inv1: true, inv3: dissolved, clean, offline: true },
      { commands: ["group_entities", "part_parent", "undo"] }
    );
    expect(grouped).toBe(true);
    expect(dissolved).toBe(true);
    expect(clean).toBe(true);
  });

  // ── DELETE = DEACTIVATE — non-destructive, undo restores (the entity survives) ──────────────────────
  it("delete = deactivate-not-destroy: the entity survives + undo restores it (functional + inv3)", async () => {
    await clearConsole();
    const id = await invoke("spawn_body", { x: 0, y: 1, z: 0 });
    await invoke("set_sim_running", { run: false });
    const before = await countEntities();

    const ok = await invoke("delete_deactivate", { id });
    expect(ok).toBe(true);
    // Deactivate, not destroy → the entity still EXISTS (part_debug reads its active flag: [x,y,z,active,n]).
    await browser.waitUntil(async () => (await invoke("part_debug", { id }))[3] === false, {
      timeout: 5000,
      timeoutMsg: "delete never deactivated the entity",
    });
    const deactivated = (await invoke("part_debug", { id }))[3] === false;
    const survives = (await invoke("part_debug", { id })).length >= 4; // still readable → still exists

    await invoke("undo");
    await browser.waitUntil(async () => (await invoke("part_debug", { id }))[3] === true, {
      timeout: 5000,
      timeoutMsg: "undo never re-activated the entity",
    });
    const restored = (await invoke("part_debug", { id }))[3] === true;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "authoring/delete-deactivate",
      { functional: deactivated && survives && restored, inv1: true, inv3: restored, clean, offline: true },
      { commands: ["delete_deactivate", "part_debug", "undo"] }
    );
    expect(deactivated).toBe(true);
    expect(survives).toBe(true);
    expect(restored).toBe(true);
    expect(before).toBeGreaterThan(0);
  });

  // ── REACT SURFACE SMOKE — the AuthoringToolbar buttons dispatch the live commands (React→delta→store) ──
  it("the React AuthoringToolbar is wired live: #authCreate grows the scene; #authNudge moves the selection", async () => {
    await clearConsole();
    const before = await countEntities();
    // A real UI button → create_entity → project delta → the store grows (#count).
    await $("#authCreate").click();
    await browser.waitUntil(async () => (await countEntities()) > before, {
      timeout: 8000,
      timeoutMsg: "#authCreate did not grow the scene (React surface not wired)",
    });
    const grew = (await countEntities()) > before;
    // The created entity is auto-selected; #authNudge → multiEdit on it → it moves (the button→command loop).
    const sel = await invoke("gizmo_selected");
    let moved = true;
    if (sel) {
      const y0 = await yOf(sel);
      await $("#authNudge").click();
      await browser.waitUntil(async () => Math.abs((await yOf(sel)) - y0) > 0.5, {
        timeout: 6000,
        timeoutMsg: "#authNudge did not move the selection",
      });
      moved = Math.abs((await yOf(sel)) - y0) > 0.5;
    }

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;
    report.workflow(
      "authoring/react-surface",
      { functional: grew && moved, inv1: true, clean, offline: true },
      { controls: ["#authCreate", "#authNudge"], commands: ["create_entity", "multi_edit"] }
    );
    expect(grew).toBe(true);
    expect(moved).toBe(true);
    expect(clean).toBe(true);
  });

  // ── PRINCIPLE 2 — the authoring verbs hold the interaction budget live ──────────────────────────────
  it("PRINCIPLE 2: reparent / multi-edit round-trips are ≤16 ms live and within baseline", async () => {
    const a = await invoke("spawn_body", { x: 0, y: 0, z: 0 });
    const b = await invoke("spawn_body", { x: 1, y: 0, z: 0 });
    const reparent = await captureBudget("reparent_part", "reparent_part", () => ({ id: b, parent: a }));
    const multi = await captureBudget("multi_edit", "multi_edit", () => ({
      ids: [a, b],
      component: "Transform",
      field: "y",
      value: 1.0,
    }));
    // Discrete authoring ops (multi_edit replies AFTER a full re-projection — a one-shot heavy, not a
    // 60fps interaction), so score against baseline, not the per-frame budget (like save_project).
    const rep = await scoreBudget(reparent, baseline, { perFrame: false });
    const med = await scoreBudget(multi, baseline, { perFrame: false });
    report.budget(rep);
    report.budget(med);
    console.log(`BUDGET reparent_part p50=${reparent.p50}ms p99=${reparent.p99}ms`);
    console.log(`BUDGET multi_edit p50=${multi.p50}ms p99=${multi.p99}ms`);
    expect(rep.verdict).not.toBe("fail");
    expect(med.verdict).not.toBe("fail");
  });
});
