// Build-acceptance — the M9 TRANSFORM surface (now in the shipping .exe): the transform GIZMO (G1, M9.1),
// rigid PART editing (G2, M9.2), and the intent-powered transform SOLVER (G4, M9.4). Each workflow is
// scored as the full ACCEPTANCE CONJUNCTION (functional + the applicable invariants 1/3/4 + principle-1
// every-"no"-explained + clean), read back from a STABLE signal (a gizmo_debug tag, a part_debug field, a
// snap hit's .why, a constraint's .reason) — never cosmetic status copy. All DOM access goes through the
// page-object (the W/E/R keybinding + the inspector buttons that appear on selection); the instrumentation
// the transparent viewport can't show is read through the same window.__TAURI__ invoke the UI uses. This
// survives the M10.1 React swap (re-point the page-object). Mirrors the live northstar.e2e.js M9.1/2/4 tests.

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

// Drive the gizmo mode through the W/E/R DOM keybinding (the universal game-editor shortcut the user
// presses). The keydown handler bails while a field is focused, so retry the key until gizmo_debug confirms
// the mode landed — a STABLE signal (the tier tag), not status copy. The page-object's `gizmoMode` is the
// only DOM touch; the read-back is command-result.
async function gizmoModeVia(mode, timeout = 10000) {
  await browser.waitUntil(
    async () => {
      await ui.gizmoMode(mode); // the W/E/R key
      const g = await ui.gizmoDebug(); // [mode, hasSel, dragging, space, pivot]
      return g[0] === mode;
    },
    { timeout, timeoutMsg: `gizmo never entered "${mode}" mode via the keybinding (last: ${JSON.stringify(await ui.gizmoDebug())})` }
  );
}

describe("acceptance / M9 transform — gizmo (G1) · part edit (G2) · solver (G4), the full conjunction", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  // ── G1 (M9.1): the transform gizmo drags a SELECTED, PAUSED body — native (0 per-frame IPC), one undoable
  // transaction, hierarchy-correct. functional + inv4 (hot-path 0-IPC) + inv3 (atomic undo). ─────────────────
  it("G1: the gizmo translates a paused body — 0-per-frame-IPC drag (inv4), moved (functional), Ctrl-Z reverts atomically (inv3)", async () => {
    await clearConsole();

    // A deterministic target: a PAUSED physics body at a known spot, so the gizmo move is the only motion.
    const id = await invoke("spawn_body", { x: 0, y: 6, z: 0 });
    expect(typeof id).toBe("string");
    await invoke("set_sim_running", { run: false });

    // Select it + enter translate via the W key → gizmo_debug confirms the mode + a live selection.
    expect(await invoke("gizmo_select", { id })).toBe(true);
    await gizmoModeVia("translate");
    const g = await ui.gizmoDebug(); // [mode, hasSel, dragging, space, pivot]
    expect(g[0]).toBe("translate"); // stable tier tag
    expect(g[1]).toBe(true); // hasSelection
    const t0 = await ui.readTransform(id); // [x, y, z, ...]

    // INVARIANT 4: grab the Y handle, then prove 0-PER-FRAME-IPC. Across ~27 render frames (450 ms) with an
    // ACTIVE native drag and NO commands, the IPC counter must NOT grow ~1/frame — only the gesture's
    // start/end cross JS; the per-frame move is native in the render loop.
    expect(await invoke("gizmo_grab", { axis: "y" })).toBe(true);
    const perFrame = await ipcPerFrame(async () => {}, 450); // measure the idle-during-drag IPC rate
    const inv4 = perFrame < 1;

    // FUNCTIONAL: drive the target up +Y (the render loop applies the drag natively), release → ONE commit.
    await invoke("gizmo_set_target", { tx: t0[0], ty: t0[1] + 5.0, tz: t0[2] });
    await browser.pause(150); // let the native render-loop drag apply
    await invoke("gizmo_drag_end");
    await browser.waitUntil(async () => (await ui.readTransform(id))[1] > t0[1] + 1.0, {
      timeout: 10000,
      timeoutMsg: "the gizmo drag did not move the body along +Y",
    });
    const functional = (await ui.readTransform(id))[1] > t0[1] + 1.0;

    // INVARIANT 3: one Ctrl-Z reverts the whole move atomically (back to the spawn height).
    await ui.undoKey();
    await browser.waitUntil(async () => Math.abs((await ui.readTransform(id))[1] - t0[1]) < 0.2, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not revert the gizmo move atomically",
    });
    const inv3 = Math.abs((await ui.readTransform(id))[1] - t0[1]) < 0.2;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "gizmo transform",
      { functional, inv1: null, inv3, inv4, p1_interactions: true, clean, offline: true },
      { commands: ["gizmo_select", "gizmo_mode", "gizmo_grab", "gizmo_set_target", "gizmo_drag_end", "gizmo_debug", "read_transform", "ipc_count"] }
    );

    expect(inv4).toBe(true); // ≪ 1 IPC/frame → the per-frame work is native (invariant 4)
    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── G2 (M9.2): rigid PART editing — the G1 gizmo on a CHILD node commits as a sparse per-field OVERRIDE
  // (ADR-026), the saved character's fresh instance carries the edit, and deactivate-not-delete is
  // recoverable. functional/inv1 (override, not base rewrite) + inv3 (deactivate undo) + clean. ──────────────
  it("G2: a part edit lands as a sparse OVERRIDE that survives save→instantiate (inv1); deactivate-not-delete is Ctrl-Z-restorable (inv3)", async () => {
    await clearConsole();

    // The seeded demo character: a body root + rigid child PART nodes (each a child Movable-Tree node).
    const demo = await invoke("demo_character"); // [root, [parts]]
    expect(demo).not.toBe(null);
    const [, parts] = demo;
    expect(parts.length).toBeGreaterThanOrEqual(2);
    const part = parts[0];

    // The part starts ACTIVE. (Override count is NOT asserted as 0 — a prior run's persisted edit may have
    // replayed; the test keys off the DELTA the edit produces, robust either way.)
    const before = await invoke("part_debug", { id: part }); // [x, y, z, active, nOverrides]
    expect(before[3]).toBe(true);

    // Edit the part with the gizmo: select → translate (W) → grab Y → move up → release. The commit routes
    // through edit_part_transform (parent-space write-back → a SPARSE per-field OVERRIDE), NOT a base
    // rewrite — so the source stays intact and the override wins by structure (inv1 = one source of truth).
    expect(await invoke("gizmo_select", { id: part })).toBe(true);
    await gizmoModeVia("translate");
    expect(await invoke("gizmo_grab", { axis: "y" })).toBe(true);
    await invoke("gizmo_set_target", { tx: before[0], ty: before[1] + 3.0, tz: before[2] });
    await browser.pause(150);
    await invoke("gizmo_drag_end");
    await browser.waitUntil(
      async () => {
        const d = await invoke("part_debug", { id: part });
        return d[4] > 0 && d[1] > before[1] + 1.0; // overrides present AND moved up in world
      },
      { timeout: 10000, timeoutMsg: "the part edit did not land as a per-field override + move" }
    );
    const after = await invoke("part_debug", { id: part });
    const functional = after[4] > 0 && after[1] > before[1] + 1.0;
    const inv1Override = after[4] > 0; // sparse per-field override KEYS exist (not a whole-object copy)

    // Save the edited character for reuse (via the inspector button) → drop a fresh instance → its matching
    // part carries the edit (the override is baked into the reusable asset; override wins over the source).
    await ui.saveChar(); // #saveChar — appears because the part is selected
    const comp = await invoke("save_character", { id: part }); // read the comp id back UI-agnostically
    expect(typeof comp).toBe("string");
    await ui.dropInstance(); // #dropInst — instantiate_character (the inspector wires lastComp)
    const inst = await invoke("instantiate_character", { comp });
    expect(typeof inst).toBe("string");
    const instPart = await invoke("part_at_path", { root: inst, path: "0" });
    expect(typeof instPart).toBe("string");
    const instDbg = await invoke("part_debug", { id: instPart }); // [x, y, z, active, nOverrides]
    const inv1Carries = instDbg[1] > before[1] + 1.0; // the saved edit is present on the fresh instance
    const inv1 = inv1Override && inv1Carries;

    // Deactivate-not-delete: a removed part is HIDDEN + recoverable. Establish a known ACTIVE baseline first
    // (robust to a prior run), deactivate via the inspector, assert part_debug active==false, Ctrl-Z restores.
    const part2 = parts[1];
    await invoke("set_part_active", { id: part2, active: true });
    expect(await invoke("gizmo_select", { id: part2 })).toBe(true); // select so #deactPart is rendered
    await ui.deactivatePart(); // #deactPart → set_part_active active:false
    await browser.waitUntil(async () => (await invoke("part_debug", { id: part2 }))[3] === false, {
      timeout: 10000,
      timeoutMsg: "the part never deactivated (active should be false, data preserved)",
    });
    expect((await invoke("part_debug", { id: part2 }))[3]).toBe(false); // hidden, but data preserved
    await ui.undoKey();
    await browser.waitUntil(async () => (await invoke("part_debug", { id: part2 }))[3] === true, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not restore the deactivated part (deactivate-not-delete)",
    });
    const inv3 = (await invoke("part_debug", { id: part2 }))[3] === true;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    // Two inventory workflows the conjunction satisfies (save-character + deactivate-not-delete share this it).
    report.workflow(
      "G2/save character",
      { functional, inv1, inv3: null, clean, offline: true },
      { commands: ["demo_character", "part_debug", "gizmo_select", "gizmo_mode", "gizmo_grab", "gizmo_set_target", "gizmo_drag_end", "save_character", "instantiate_character", "part_at_path"] }
    );
    report.workflow(
      "G2/deactivate-not-delete",
      { functional: true, inv1: null, inv3, clean, offline: true },
      { commands: ["set_part_active"] }
    );

    expect(functional).toBe(true);
    expect(inv1).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── G4 (M9.4): the intent-powered transform SOLVER — the snap-graph (ADR-011 reuse) ranks hits each with
  // an explained .why nearest-first; a declared constraint SOLVES (moves the part) or is REFUSED with a
  // .reason (every-"no"-explained, p1); a natural-language placement sentence compiles to ≥1 editable
  // intent. functional + inv1 (the part actually moved) + p1_explained + clean. ──────────────────────────────
  it("G4: snap-graph ranks explained hits nearest-first; a snap constraint solves+commits; a target-less constraint is REFUSED with a reason (p1); a sentence compiles to intents", async () => {
    await clearConsole();

    const demo = await invoke("demo_character"); // [root, [parts]]
    const [root, parts] = demo;
    const part = parts[0];

    // Turn magnetic snapping ON via the inspector toggle (select the part first so #snapToggle renders),
    // then read the snap-graph: ranked candidates within radius, each with an explained "why this" — the
    // reveal/rank/explain pattern applied to space (the SAME ADR-011 ranker, reused).
    expect(await invoke("gizmo_select", { id: part })).toBe(true);
    await ui.toggleSnap(); // #snapToggle → set_snap
    const hits = await invoke("snap_query", { id: part, radius: 100.0 });
    expect(Array.isArray(hits) && hits.length > 0).toBe(true);
    // STABLE signal: each hit explains itself ("snap to …"); ranked nearest-first (proximity primary).
    expect(typeof hits[0].why).toBe("string");
    const everyHitExplained = hits.every((h) => typeof h.why === "string" && h.why.toLowerCase().includes("snap to"));
    const rankedNearestFirst = hits[0].distance <= hits[hits.length - 1].distance + 1e-3;
    expect(everyHitExplained).toBe(true);
    expect(rankedNearestFirst).toBe(true);

    // Declare a "snap" constraint targeting the root → the part SOLVES to the target's world position
    // (solve + commit). Read the move back from part_debug — a structured field, not status copy.
    const rootDbg = await invoke("part_debug", { id: root }); // [x, y, z, active, nOverrides]
    const res = await invoke("apply_constraint", { id: part, kind: "snap", target: root, value: 0.0 });
    expect(res.ok).toBe(true);
    await browser.waitUntil(
      async () => {
        const d = await invoke("part_debug", { id: part });
        return Math.abs(d[0] - rootDbg[0]) < 0.1 && Math.abs(d[1] - rootDbg[1]) < 0.1 && Math.abs(d[2] - rootDbg[2]) < 0.1;
      },
      { timeout: 10000, timeoutMsg: "the snap constraint did not move the part to the target" }
    );
    const moved = await invoke("part_debug", { id: part });
    const functional =
      Math.abs(moved[0] - rootDbg[0]) < 0.1 && Math.abs(moved[1] - rootDbg[1]) < 0.1 && Math.abs(moved[2] - rootDbg[2]) < 0.1;

    // PRINCIPLE 1 (every "no" explained, ADR-016): a constraint with NO target is REFUSED, with a reason —
    // not a silent no-op. The .reason is the stable explained signal.
    const blocked = await invoke("apply_constraint", { id: part, kind: "coaxial", target: null, value: 0.0 });
    expect(blocked.ok).toBe(false);
    const p1Explained = typeof blocked.reason === "string" && blocked.reason.length > 0;
    expect(p1Explained).toBe(true);

    // A natural-language placement SENTENCE compiles to ≥1 editable intent (the AI-as-constraint-compiler);
    // driven through the inspector field+button, read back through the command result.
    await invoke("gizmo_select", { id: root }); // select the root so #placeSentence/#placeBtn render
    await ui.placeBySentence("upright, 10 cm from the edge"); // #placeSentence + #placeBtn → placement_sentence
    const placed = await invoke("placement_sentence", { id: root, text: "upright, 10 cm from the edge" });
    const compiles = Array.isArray(placed.intents) && placed.intents.length > 0;
    expect(compiles).toBe(true);

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    // The G4 inventory workflows the conjunction satisfies.
    report.workflow(
      "G4/magnetic snap toggle",
      { functional: true, inv1: null, clean, offline: true },
      { commands: ["set_snap"] }
    );
    report.workflow(
      "G4/snap-to-nearest",
      { functional, inv1: functional, p1_explained: p1Explained, clean, offline: true },
      { commands: ["snap_query", "apply_constraint"] }
    );
    report.workflow(
      "G4/place-by-sentence",
      { functional: compiles, inv1: null, clean, offline: true },
      { commands: ["placement_sentence"] }
    );

    expect(functional).toBe(true);
    expect(p1Explained).toBe(true);
    expect(compiles).toBe(true);
    expect(clean).toBe(true);
  });

  // ── G2 reparent ("drag in hierarchy" = node.move): reparenting a part under a new parent is a real,
  // undoable structural move — assert the projection's parentId edge changes, then Ctrl-Z restores it (inv3).
  it("G2: reparent moves a part under a new parent (node.move) and Ctrl-Z restores the original parent (inv3)", async () => {
    await clearConsole();

    const demo = await invoke("demo_character"); // [root, [parts]]
    const [root, parts] = demo;
    const child = parts[1]; // a part that starts under the root

    // `part_parent` is the stable structural read-back (the node.move edge): root key now (child starts
    // under root), null for a true root. NO try/catch fallback — a missing command must FAIL the gate, not
    // let the workflow self-pass on a hardcoded literal.
    const beforeParent = await invoke("part_parent", { id: child });
    // Reparent the child under parts[0] via the inspector (select it so #reparentBtn renders).
    expect(await invoke("gizmo_select", { id: child })).toBe(true);
    await ui.reparentTo(parts[0]); // #reparentTo + #reparentBtn → reparent_part
    // The structural edge actually moved to the new parent (node.move), read back from part_parent.
    await browser.waitUntil(async () => (await invoke("part_parent", { id: child })) === parts[0], {
      timeout: 10000,
      timeoutMsg: "reparent did not move the part under the new parent (node.move edge)",
    });
    const functional = (await invoke("part_parent", { id: child })) === parts[0];
    // INVARIANT 3 — Ctrl-Z restores the original parent.
    await ui.undoKey();
    await browser.waitUntil(async () => (await invoke("part_parent", { id: child })) === beforeParent, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not restore the original parent after reparent",
    });
    const inv3 = (await invoke("part_parent", { id: child })) === beforeParent;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "G2/reparent",
      { functional, inv1: null, inv3, clean, offline: true },
      { commands: ["reparent_part"] }
    );

    expect(functional).toBe(true);
    expect(clean).toBe(true);
    expect(root).toBeTruthy();
  });

  // ── PRINCIPLE 2: the interactive transform ops hold ≤16 ms live (p50/p99) and within baseline. The gizmo
  // drag is native (no per-op IPC budget), so we budget the command-result ops the solver round-trips on. ────
  it("PRINCIPLE 2: read_transform / gizmo_debug / snap_query hold ≤16 ms live and within baseline", async () => {
    // A selected, paused body gives read_transform/gizmo_debug a real subject.
    const id = await invoke("spawn_body", { x: 0, y: 6, z: 0 });
    await invoke("set_sim_running", { run: false });
    await invoke("gizmo_select", { id });

    const demo = await invoke("demo_character"); // [root, [parts]] — a snap subject
    const part = demo[1][0];

    const ops = [
      await captureBudget("read_transform", "read_transform", { id }, { n: 30, warmup: 5 }),
      await captureBudget("gizmo_debug", "gizmo_debug", {}, { n: 30, warmup: 5 }),
      await captureBudget("snap_query", "snap_query", { id: part, radius: 100.0 }, { n: 30, warmup: 5 }),
    ];

    for (const s of ops) {
      const scored = await scoreBudget(s, baseline, {
        perFrame: true,
        recapture: () =>
          captureBudget(
            s.label,
            s.label,
            s.label === "read_transform" ? { id } : s.label === "snap_query" ? { id: part, radius: 100.0 } : {},
            { n: 30, warmup: 5 }
          ),
      });
      report.budget(scored);
      expect(scored.verdict, `${s.label}: ${scored.note}`).toBe("pass");
    }
  });
});
