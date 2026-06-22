// Build-acceptance — the NEGATIVE / EDGE surface: the failure modes the gate must catch, each scored as
// the full ACCEPTANCE CONJUNCTION (functional = the failure was handled correctly + invariant 1 = the
// projected state stayed consistent + clean = no console errors). Same shape as north-star-1: every DOM
// touch goes through the page-object (React-swap durable); every state change is read back through a
// command / the projection / a STABLE signal (a reject token, a refusal token, a structural count —
// never cosmetic copy that drifts); results are recorded into the `report` singleton.
//
// What "good" looks like here is the OPPOSITE of the happy paths: a bad edit must be REJECTED (not
// silently coerced) and leave the engine untouched (rejection-as-UX, ADR-010); a paid action with no
// budget must be REFUSED with an explained seam (not a crash, not a silent fake); and a rapid double-fire
// of the same mutation must NOT double-apply (the projection grows by exactly one). undo-past-seed is
// already covered in core-workflows — not duplicated here.

import { browser, expect } from "@wdio/globals";
import { page } from "../../pages/scaffold.js";
import { report, invoke, consoleErrors, clearConsole } from "../../lib/acceptance.js";

const ui = page();

// The cheapest paid action is the AI-edit (Action::Edit = 2 tokens); the free grant is 30 tokens, so a
// drain via repeated AI-edits bottoms out in ~15 iterations — comfortably inside the ≤60 bound below.
const EDIT_COST = 2;
const DRAIN_BOUND = 60;

describe("acceptance / negative + edge — rejection-as-UX, refuse-when-broke, idempotency", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  // ── REJECT-INVALID-EDIT (rejection-as-UX, ADR-010): an invalid field edit through the SAME commit
  // pipeline is REJECTED and surfaced, and the projected state is UNCHANGED. We pick a real entity (so the
  // entity is valid — the rejection is about the VALUE, not a bad id), read its Transform.x back through
  // `read_transform` (the engine-authoritative translation, a stable numeric field), then submit a
  // setField whose value is non-scalar garbage ({} / []). The bridge's json_to_field can't map a
  // non-scalar to a /core FieldValue → the commit is rejected with "unsupported value for Transform.x"
  // and the connected channel delivers a `rejects` delta → the UI paints #reject (ui.reject()). Crucially
  // the engine never committed, so read_transform reads back the SAME x (invariant 1). ───────────────────
  it("an invalid field edit is REJECTED + surfaced, and the projected state is UNCHANGED (functional + inv1 + clean)", async () => {
    await clearConsole();

    // Select a real entity from the viewport so we have a valid id carrying a Transform.
    await ui.pickCenter();
    await ui.waitStatus("picked");
    expect(await ui.status()).not.toContain("nothing here");
    expect(await ui.inspectorText()).toContain("Transform");

    // The pick landed on an entity (the gizmo reports a live selection).
    const sel = await invoke("gizmo_debug"); // [mode, hasSel, dragging, space, pivot]
    expect(sel[1]).toBe(true);

    // Resolve the picked entity's id directly: viewport_pick at the screen centre returns the id of the
    // entity under the cursor (the same normalized [0,1] pick the click issues), which we then target in
    // the submit_edit intent and read back through read_transform.
    const id = await invoke("viewport_pick", { x: 0.5, y: 0.5 });
    expect(typeof id).toBe("string");
    expect(id.length).toBeGreaterThan(0);

    // The engine-authoritative Transform.x BEFORE the bad edit (the value the projection must keep).
    const beforeT = await invoke("read_transform", { id }); // [x, y, z, ...]
    const beforeX = beforeT[0];

    // Submit an INVALID edit through the real pipeline: Transform.x ← a non-scalar garbage value. This is
    // the exact tx shape the inspector's editField builds (clientOpId/label/patches/intent), but the value
    // can't be mapped to a /core scalar → the bridge rejects it ("unsupported value for Transform.x"),
    // all-or-nothing, nothing applied.
    const clientOpId = "e2e-bad-edit-" + Date.now();
    await invoke("submit_edit", {
      tx: {
        clientOpId,
        label: "set Transform.x (garbage)",
        patches: [],
        intent: { kind: "setField", id, component: "Transform", field: "x", value: { garbage: true } },
      },
    });

    // FUNCTIONAL — the rejection happened and is SURFACED: #reject is populated (ui.reject()) OR the status
    // names a rejection. #reject auto-hides after a few seconds, so poll inside that window for the token.
    let rejected = false;
    await browser.waitUntil(
      async () => {
        const rj = (await ui.reject()) || "";
        const st = (await ui.status()) || "";
        rejected = /reject/i.test(rj) || /reject/i.test(st);
        return rejected;
      },
      { timeout: 10000, timeoutMsg: "the invalid edit was not surfaced as a rejection (#reject / status)" }
    );
    expect(rejected).toBe(true);

    // INVARIANT 1 — the projected state is UNCHANGED: the engine never committed the bad value, so the
    // authoritative Transform.x reads back EXACTLY what it was (read it through the command, not the DOM).
    const afterT = await invoke("read_transform", { id });
    const unchanged = Math.abs(afterT[0] - beforeX) < 1e-9;
    expect(unchanged).toBe(true);
    // And the field was NOT silently coerced to the garbage (defensive: it's still a finite number).
    expect(Number.isFinite(afterT[0])).toBe(true);

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "reject-invalid-edit (rejection-as-UX)",
      { functional: rejected, inv1: unchanged, inv3: null, clean },
      { commands: ["submit_edit", "read_transform"] }
    );

    expect(rejected).toBe(true);
    expect(unchanged).toBe(true);
    expect(clean).toBe(true);
  });

  // ── REFUSE-WHEN-BROKE (wallet): drain the wallet to near-zero with the cheapest paid action (AI-edit,
  // 2 tokens), BOUNDED to ≤60 iterations, stopping once the balance can't cover another edit. Then the
  // NEXT paid action must be REFUSED with an EXPLAINED seam — a stable refusal token ("insufficient" /
  // "balance" / "need" / "top up"), never a crash or a silent fake. The wallet is restored (top_up) at the
  // end so later specs aren't starved. If the drain can't reach broke within the bound, we record
  // functional:null and note it rather than fake the assertion. ────────────────────────────────────────
  it("a paid action with no budget is REFUSED with an explained seam, not a crash or a silent fake (functional + clean)", async () => {
    await clearConsole();

    // A valid, selected entity to aim the paid AI-edit at (the edit applies MeshRenderer.material).
    const id = await invoke("viewport_pick", { x: 0.5, y: 0.5 });
    expect(typeof id).toBe("string");

    // Drain: loop the cheapest paid action until the spendable balance can't cover another edit. ai_edit
    // returns { ok, balance, cost, message }; each success debits EDIT_COST. BOUNDED — never unbounded.
    let bal = await ui.walletBalance();
    let iters = 0;
    let drained = false;
    while (iters < DRAIN_BOUND) {
      if (bal < EDIT_COST) {
        drained = true;
        break;
      }
      const r = await invoke("ai_edit", { id }); // a metered paid action (debit-on-success)
      iters++;
      // The engine reports the post-action balance; mirror it into our local view (and the DOM updates too).
      bal = typeof r?.balance === "number" ? r.balance : await ui.walletBalance();
      if (r && r.ok === false && /insufficient|balance|broke|need|top up/i.test(r.message || "")) {
        // We hit the refusal seam mid-loop (balance fell below the price between checks) — that IS the drain.
        drained = true;
        break;
      }
    }

    let functional = null;
    let clean;
    if (!drained) {
      // Could not reach broke within the bound — DO NOT fake it. Record null + note, assert nothing on the
      // refusal seam, and still verify cleanliness.
      const errs = await consoleErrors();
      clean = errs.length === 0;
      if (!clean) report.consoleErrorCount += errs.length;
      report.workflow(
        "refuse-when-broke (wallet)",
        { functional: null, inv1: null, inv3: null, clean },
        { commands: ["ai_edit", "wallet_info", "top_up"] }
      );
      console.log(`[negative-edge] drain did not reach broke within ${DRAIN_BOUND} iters (bal=${bal}) — functional:null`);
      expect(clean).toBe(true);
      return;
    }

    // Broke (or already refusing). The NEXT paid action must be REFUSED with an EXPLAINED token — never a
    // throw, never a silent ok:true. Try the cheapest paid action again.
    const refusal = await invoke("ai_edit", { id });
    expect(refusal).not.toBe(null);
    expect(refusal.ok).toBe(false); // not a silent fake-success
    const msg = (refusal.message || "").toLowerCase();
    const explained = /insufficient|balance|broke|need|top up/.test(msg);
    expect(explained).toBe(true); // the seam EXPLAINS the "no" (principle 1) with a stable refusal token
    functional = refusal.ok === false && explained;

    // The engine is still alive after the refusal (wallet_info responds — no crash from the broke path).
    const w = await invoke("wallet_info");
    expect(typeof w?.balance).toBe("number");

    const errs = await consoleErrors();
    clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "refuse-when-broke (wallet)",
      { functional, inv1: null, inv3: null, p1_explained: explained, clean },
      { commands: ["ai_edit", "wallet_info", "top_up"] }
    );

    // Restore the wallet for the rest of the run (top-up RAISES the balance — leaves later specs solvent).
    const broke = await ui.walletBalance();
    await ui.topUp();
    await browser.waitUntil(async () => (await ui.walletBalance()) > broke, {
      timeout: 10000,
      timeoutMsg: "top-up did not restore the balance after the drain",
    });

    expect(functional).toBe(true);
    expect(clean).toBe(true);
  });

  // ── IDEMPOTENCY (rapid / double-click): fire the SAME mutating action twice in immediate succession and
  // assert it did NOT produce a DUPLICATE op. We bind the same (requirer → target) edge twice back-to-back;
  // the projection's authoritative bound set (reveal_targets(id).bound) must grow by EXACTLY 1, not 2 — the
  // binding is keyed by (from|rel|to) in /core, so a re-fire overwrites the same key rather than appending
  // a second edge (the rapid-double-click guard). ─────────────────────────────────────────────────────
  it("a rapid double-fire of the same bind does NOT duplicate — the projection grows by exactly one (functional + inv1 + inv3)", async () => {
    await clearConsole();

    // Pick a requirer (a HealthBar) and reveal its ranked compatible targets through the command.
    await ui.selectRequirer(0);
    await browser.waitUntil(async () => (await ui.revealCandidates()).length > 0, {
      timeout: 10000,
      timeoutMsg: "the requirer's reveal never populated",
    });
    const reqs = await ui.requirers();
    const rid = await reqs[0].getAttribute("data-id");
    expect(typeof rid).toBe("string");

    // Resolve a concrete compatible target id from the reveal (the same id the .cand[data-to] carries).
    const reveal = await invoke("reveal_targets", { id: rid });
    expect(Array.isArray(reveal.compatible)).toBe(true);
    expect(reveal.compatible.length).toBeGreaterThan(0);
    const tid = reveal.compatible[0].id;
    expect(typeof tid).toBe("string");

    // The authoritative bound count BEFORE (the projection's tracking set for this requirer).
    const boundBefore = (reveal.bound || []).length;

    // Fire the SAME bind TWICE in immediate succession (the rapid double-click). Both calls reference the
    // identical (from, to) pair — a duplicate must collapse, not stack.
    await Promise.all([
      invoke("bind_target", { from: rid, to: tid }),
      invoke("bind_target", { from: rid, to: tid }),
    ]);

    // FUNCTIONAL + INVARIANT 1 — read the bound set back from the projection and assert it grew by EXACTLY
    // one (the single edge), never two. Poll until the bind echo lands, then assert the count is +1.
    let boundAfter = boundBefore;
    await browser.waitUntil(
      async () => {
        const r = await invoke("reveal_targets", { id: rid });
        boundAfter = (r.bound || []).length;
        return boundAfter > boundBefore; // the bind landed
      },
      { timeout: 10000, timeoutMsg: `the double-bind never appeared under tracking (was ${boundBefore})` }
    );
    // Settle briefly so a (hypothetical) second duplicate echo would have arrived, then re-read.
    await browser.pause(200);
    boundAfter = ((await invoke("reveal_targets", { id: rid })).bound || []).length;
    const grewByExactlyOne = boundAfter === boundBefore + 1;
    expect(grewByExactlyOne).toBe(true); // +1, NOT +2 → the duplicate was idempotent

    // INVARIANT 3 — the single bound edge is reversed by ONE undo (one transaction, not two stacked).
    await ui.undoKey();
    await browser.waitUntil(
      async () => ((await invoke("reveal_targets", { id: rid })).bound || []).length <= boundBefore,
      { timeout: 10000, timeoutMsg: "one undo did not remove the (single) bound edge" }
    );
    const inv3 = ((await invoke("reveal_targets", { id: rid })).bound || []).length <= boundBefore;

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "idempotency (rapid double-bind)",
      { functional: grewByExactlyOne, inv1: grewByExactlyOne, inv3, clean },
      { commands: ["bind_target", "reveal_targets", "undo"] }
    );

    expect(grewByExactlyOne).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });
});
