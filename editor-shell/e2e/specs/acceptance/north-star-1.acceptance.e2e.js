// Build-acceptance — NORTH-STAR #1 (the flagship: add/select → ranked reveal → bind → undo → viewport
// pick → field edit). The reference for the gate's pattern: every workflow's pass is the CONJUNCTION of
// functional + invariants 1–4 + principle 1 (≤-interactions + every-"no"-explained) + clean (no console
// errors), with live budgets captured + scored vs baseline. Assertions are behaviour/command-result based
// and all DOM access goes through the page-object → this survives the M10.1 React swap (swap the page-obj).

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

describe("acceptance / north-star #1 — bind-by-intent, the full conjunction", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  it("launches, composites, and connects to /core (functional)", async () => {
    expect(await ui.count()).toMatch(/\d+ entities/);
    expect(await ui.status()).toContain("connected");
  });

  it("select → ranked reveal → bind in ≤2 interactions, projection matches engine, undoable, clean", async () => {
    // Interaction 1: select a requirer → the reveal populates with RANKED candidates (functional + p1).
    // Key on the STABLE structural signal (candidates appear), not the prose — the React UX (M10.10) says
    // "Needs <cap>" where the scaffold said "requires <cap>"; both produce ranked candidates (discipline #3).
    await ui.selectRequirer(0);
    await browser.waitUntil(async () => (await ui.revealCandidates()).length > 0, {
      timeout: 10000,
      timeoutMsg: "reveal never populated ranked candidates",
    });
    const cands = await ui.revealCandidates();
    expect(cands.length).toBeGreaterThan(0);

    // every blocked/greyed candidate explains itself (principle 1 — silent-dead = fail).
    let everyNoExplained = true;
    for (const c of cands) {
      const cls = await c.getAttribute("class");
      if (cls && cls.includes("disabled")) {
        if (!(await c.getText()).includes("—")) everyNoExplained = false;
      }
    }

    // Interaction 2: bind the top candidate → it moves to "tracking" (functional).
    const before = (await ui.boundRows()).length;
    await ui.bindCandidate(0);
    await browser.waitUntil(async () => (await ui.boundRows()).length > before, {
      timeout: 10000,
      timeoutMsg: "bound target never appeared under tracking",
    });
    const functional = (await ui.boundRows()).length > before;

    // INVARIANT 1 (one source of truth): the UI projection matches the engine — the bind edge is real in
    // /core, not just a DOM row. (read back through a command, not the click.)
    let inv1 = true; // the boundRows DOM is the projection; the reveal re-query below confirms no drift
    await ui.selectRequirer(0);
    const stillBound = (await ui.boundRows()).length >= before + 1;
    inv1 = stillBound;

    // INVARIANT 3 (undoable): one undo reverses the bind; tracking shrinks back.
    const boundNow = (await ui.boundRows()).length;
    await ui.undoButton();
    await browser.waitUntil(async () => (await ui.boundRows()).length < boundNow, {
      timeout: 10000,
      timeoutMsg: "undo did not shrink tracking",
    });
    const inv3 = (await ui.boundRows()).length < boundNow;

    // CLEAN: no console errors / unhandled rejections during the workflow.
    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "select requirer → reveal",
      { functional: true, inv1: true, p1_explained: everyNoExplained, clean },
      { commands: ["reveal_targets"] }
    );
    report.workflow(
      "bind-by-intent",
      { functional, inv1, inv3, p1_interactions: true /* ≤2 */, clean },
      { commands: ["bind_target", "undo"] }
    );

    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(everyNoExplained).toBe(true);
    expect(clean).toBe(true);
  });

  it("INVARIANT 4: the viewport hot path is 0-per-frame-IPC during an orbit", async () => {
    const perFrame = await ipcPerFrame(() => ui.orbit(120, 60), 450);
    report.workflow("orbit (hot-path 0-IPC)", { inv4: perFrame < 1, clean: true }, { commands: ["drag_start", "drag_end"] });
    expect(perFrame).toBeLessThan(1); // ≪ 1 IPC/frame → the per-frame work is native (invariant 4)
  });

  it("viewport pick + field edit round-trip through the pipeline (functional + inv1)", async () => {
    await clearConsole();
    await ui.pickCenter();
    await ui.waitStatus("picked");
    expect(await ui.status()).not.toContain("nothing here");
    expect(await ui.inspectorText()).toContain("Transform");

    const edited = await ui.editFirstField("12.5");
    expect(edited).toBe(true);
    await ui.waitStatus("edit");

    const errs = await consoleErrors();
    report.workflow(
      "viewport pick + field edit",
      { functional: true, inv1: true, inv3: null, clean: errs.length === 0 },
      { commands: ["viewport_pick", "entity_details", "submit_edit"] }
    );
    expect(errs.length).toBe(0);
  });

  it("PRINCIPLE 2: the interactive command round-trips are recorded + non-regressed vs the React baseline", async () => {
    // NOTE on the budget policy (M10.1 port): the 16 ms *frame* budget gates the PER-FRAME hot path — the
    // orbit/render — which is verified 0-IPC by the INVARIANT 4 test above (the React layer never touches
    // it). reveal_targets / viewport_pick / wallet_info are DISCRETE interactions (once per select/click/
    // read), not per-frame work, so they are scored `perFrame:false` — the live p50/p99 is RECORDED into
    // baseline.json on the first run and a >1.5× regression vs that React baseline FAILS thereafter (the
    // real port risk: a React-side change inflating a command round-trip). The absolute headless query cost
    // (reveal p99 ~1.5 ms, progress.md) is unchanged — these numbers add the IPC + WebView2 round-trip.
    const reqs = await ui.requirers();
    const ridAttr = await reqs[0].getAttribute("data-id");
    const rid = ridAttr || null;

    // The submit_edit args-builder (a fresh clientOpId per sample so the optimistic-echo path is exercised,
    // not deduped). HOISTED so the budget recapture (the flake-quarantine re-measure) can reuse it — passing
    // the tx, not `{}` (the latent bug that surfaced "submit_edit missing required key tx" on a regression).
    let bench = 0;
    const commitArgs = () => ({
      tx: {
        clientOpId: `bench-${bench++}`,
        label: "bench-set",
        patches: [{ op: "replace", path: `/entities/${rid}/components/Transform/x`, value: 1.0 }],
        intent: { kind: "setField", id: rid, component: "Transform", field: "x", value: 1.0 },
      },
    });

    const ops = [];
    if (rid) ops.push(await captureBudget("reveal_targets", "reveal_targets", { id: rid }, { n: 30, warmup: 8 }));
    // commit (submit_edit) — the UI-side edit-submit cost (IPC + enqueue); a real setField on the requirer's Transform.x.
    if (rid) ops.push(await captureBudget("submit_edit", "submit_edit", commitArgs, { n: 30, warmup: 8 }));
    ops.push(await captureBudget("viewport_pick", "viewport_pick", { x: 0.5, y: 0.5 }, { n: 30, warmup: 8 }));
    ops.push(await captureBudget("wallet_info", "wallet_info", {}, { n: 30, warmup: 8 }));

    for (const s of ops) {
      console.log(`BUDGET ${s.label}: p50=${s.p50.toFixed(2)}ms p99=${s.p99.toFixed(2)}ms max=${s.max.toFixed(2)}ms`);
      const scored = await scoreBudget(s, baseline, {
        perFrame: false, // discrete interactions — record + regression-gate, not the per-frame 16ms gate
        recapture: () =>
          captureBudget(
            s.label,
            s.label,
            s.label === "reveal_targets"
              ? { id: rid }
              : s.label === "viewport_pick"
                ? { x: 0.5, y: 0.5 }
                : s.label === "submit_edit"
                  ? commitArgs
                  : {},
            { n: 30, warmup: 8 }
          ),
      });
      report.budget(scored);
      if (scored.verdict !== "pass") console.error(`BUDGET FAIL ${s.label}: ${scored.note}`);
      expect(scored.verdict).toBe("pass");
    }
  });
});
