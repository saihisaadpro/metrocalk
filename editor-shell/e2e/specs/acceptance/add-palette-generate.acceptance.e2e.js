// Build-acceptance — the ADD-PALETTE + GENERATE workflows (M3.4 browse/search/pick + M6 opt-in
// generation), each scored as the full ACCEPTANCE CONJUNCTION (functional read-back + invariants + the
// ≤-interactions principle + clean). Same shape as north-star-1: all DOM access goes through the
// page-object (React-swap durable), all state changes are read back through a command / the projection /
// a stable status tag (never cosmetic copy), and the result is recorded into the `report` singleton.

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

// The live entity count as a NUMBER (the projection store's size, surfaced into #count as "N entities").
// The stable structural signal that an add/generate placed an entity and that an undo removed it again.
const entityCount = async () => {
  const m = (await ui.count()).match(/(\d+)\s+entities/);
  return m ? Number(m[1]) : NaN;
};

describe("acceptance / add-palette + generate — the create-from-catalog + last-resort generation loops", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  // ── ADD-PALETTE: open → catalog populated → pick an installed kind → it lands (count grows + status
  // names it) → ONE undo removes it (inv 3). commands: catalog, add_item. ─────────────────────────────
  it("opens the palette, the catalog is populated, picking an item places an installed kind (functional + inv3)", async () => {
    await clearConsole();

    // Open the palette → it becomes visible and the catalog (invoke `catalog`) populates the body.
    await ui.openPalette();
    await browser.waitUntil(() => ui.paletteVisible(), {
      timeout: 10000,
      timeoutMsg: "the + Add palette never became visible",
    });
    await browser.waitUntil(async () => (await ui.paletteItems()).length > 0, {
      timeout: 10000,
      timeoutMsg: "the catalog never populated (catalog returned no installed kinds)",
    });
    const catalogCount = (await ui.paletteItems()).length;
    expect(catalogCount).toBeGreaterThan(0);

    // Pick the first installed kind → add_item instantiates it (one undoable tx). Read the STATE CHANGE
    // back as a STRUCTURAL signal: the projection store grows by one entity (count++), AND the status
    // names the add ("added …"/"bought …") — both stable, not cosmetic.
    const before = await entityCount();
    await ui.pickPaletteItem(0);
    await browser.waitUntil(async () => (await entityCount()) > before, {
      timeout: 10000,
      timeoutMsg: `the palette pick never grew the entity count (was ${before})`,
    });
    const afterAdd = await entityCount();
    const addStatus = await ui.status();
    const functional = afterAdd > before && /\b(added|bought)\b/i.test(addStatus);
    expect(functional).toBe(true);

    // INVARIANT 3 (undoable): ONE undo removes the just-added entity → the count returns to `before`.
    await ui.undoKey();
    await browser.waitUntil(async () => (await entityCount()) <= before, {
      timeout: 10000,
      timeoutMsg: `undo did not remove the added entity (count stayed ${await entityCount()}, want ≤ ${before})`,
    });
    const inv3 = (await entityCount()) <= before;
    expect(inv3).toBe(true);

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "add-palette/open",
      { functional: catalogCount > 0, inv3: null, clean },
      { commands: ["catalog"] }
    );
    report.workflow(
      "add-palette/pick",
      // inv1 is null (not claimed): the count read-back IS the functional proof; a distinct
      // determinism/idempotence inv1 assertion isn't performed here, so don't overclaim it.
      { functional, inv1: null, inv3, p1_interactions: true /* open + pick = 2 */, clean },
      { commands: ["add_item", "undo"] }
    );
    expect(clean).toBe(true);
  });

  // ── ADD-PALETTE SEARCH: a query filters the catalog (catalog_search), and a NO-MATCH query offers the
  // generate fall-through (#palGen). commands: catalog_search. ─────────────────────────────────────────
  it("the search bar filters the catalog and a no-match query offers the generate fall-through (functional)", async () => {
    await clearConsole();

    // The palette is still open from the previous workflow; ensure it is (re-open if a stray Escape closed it).
    if (!(await ui.paletteVisible())) {
      await ui.openPalette();
      await browser.waitUntil(() => ui.paletteVisible(), {
        timeout: 10000,
        timeoutMsg: "the palette never re-opened for the search workflow",
      });
    }

    // A REAL term filters the catalog down to matching items (catalog_search reuses the tiered resolver).
    await ui.searchPalette("health");
    await browser.waitUntil(async () => (await ui.paletteItems()).length > 0, {
      timeout: 10000,
      timeoutMsg: "a real catalog search ('health') returned no items",
    });
    const filtered = (await ui.paletteItems()).length;
    expect(filtered).toBeGreaterThan(0);

    // A NO-MATCH query falls through to the generate seam: the #palGen offer appears (the opt-in last resort).
    await ui.searchPalette("zzz-no-such-kind-zzz-9000");
    await browser.waitUntil(async () => (await ui.paletteGenerateOffer()).isExisting(), {
      timeout: 10000,
      timeoutMsg: "no generate fall-through (#palGen) offered on a no-match catalog search",
    });
    const offered = await (await ui.paletteGenerateOffer()).isExisting();
    expect(offered).toBe(true);

    await ui.closePalette();
    await browser.waitUntil(async () => !(await ui.paletteVisible()), {
      timeout: 10000,
      timeoutMsg: "the palette did not close on esc",
    });

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "add-palette/search",
      { functional: filtered > 0 && offered, inv3: null, clean },
      { commands: ["catalog_search"] }
    );
    report.workflow(
      "add-palette/generate-fallthrough",
      { functional: offered, inv3: null, clean },
      { commands: ["catalog_search"] }
    );
    expect(clean).toBe(true);
  });

  // ── GENERATE (M6, opt-in, METERED): a no-local-match describe surfaces #genBtn → click Generate → a grey
  // placeholder lands instantly (count++) and the metered cost is reserved (wallet drops); the generated
  // mesh streams in over the projection (the placeholder gains a MeshRenderer.mesh handle, ADR-017 patch).
  // ONE undo removes the placeholder (inv 3). commands: generate. ─────────────────────────────────────
  it("a no-local-match describe → Generate places a metered placeholder, the mesh streams in, undo removes it", async () => {
    await clearConsole();

    // Generation is gated on the wallet (it RESERVES ≈10 tokens before any placeholder drops). Top up first
    // so the balance can't refuse the generate (and so the post-generate debit is unambiguous).
    const balPre = await ui.walletBalance();
    if (balPre < 50) {
      await ui.topUp();
      await browser.waitUntil(async () => (await ui.walletBalance()) > balPre, {
        timeout: 10000,
        timeoutMsg: "top-up did not raise the balance before generate",
      });
    }
    const balBefore = await ui.walletBalance();

    // A describe with no local + no marketplace match surfaces the OPT-IN Generate button (#genBtn). The
    // stable signal is the status: "no local or marketplace match …" AND #genBtn becoming visible.
    await ui.describe("a glowing crystal totem");
    await browser.waitUntil(() => ui.generateVisible(), {
      timeout: 15000,
      timeoutMsg: "a no-local-match describe never surfaced the Generate button (#genBtn hidden)",
    });
    expect(await ui.generateVisible()).toBe(true);
    expect(await ui.status()).toContain("Generate"); // the opt-in offer (stable copy: the affordance name)

    // Click Generate → a grey placeholder drops in INSTANTLY (count grows by one) and the cost is RESERVED
    // (wallet drops). Capture the placeholder id from the structural delta (the new entity).
    const countBefore = await entityCount();
    await ui.clickGenerate();

    // (1) the grey placeholder landed → count grew by one (the structural read-back, not the copy).
    await browser.waitUntil(async () => (await entityCount()) > countBefore, {
      timeout: 15000,
      timeoutMsg: `Generate never placed a placeholder entity (count stayed ${countBefore})`,
    });
    const countAfter = await entityCount();
    const placed = countAfter > countBefore;
    expect(placed).toBe(true);

    // (2) METERED: the wallet balance dropped (the generation reserved its ≈10 tokens up front, M7).
    await browser.waitUntil(async () => (await ui.walletBalance()) < balBefore, {
      timeout: 15000,
      timeoutMsg: `generate did not meter the wallet (balance stayed ${balBefore})`,
    });
    const metered = (await ui.walletBalance()) < balBefore;
    expect(metered).toBe(true);

    // (3) the status reaches a stable "generating" signal (the streaming affordance — distinct from the offer).
    await ui.waitStatus("generating", 15000);
    expect(await ui.status()).toContain("generating");

    // (4) the generated mesh STREAMS IN over the projection (ADR-017 validated patch): the just-placed
    //     entity gains a MeshRenderer mesh handle. doGenerate AUTO-SELECTS the created placeholder, so the
    //     inspector renders the generated entity's components — read the streamed-in MeshRenderer slot back
    //     from that projection-backed view (a STABLE structural field, not cosmetic copy). First confirm the
    //     auto-select landed (gizmo reports a selection), then assert the MeshRenderer is present.
    await browser.waitUntil(
      async () => {
        const g = await invoke("gizmo_debug").catch(() => null); // [mode, hasSel, dragging, space, pivot]
        return Array.isArray(g) && g[1] === true; // the generated placeholder is selected
      },
      { timeout: 15000, timeoutMsg: "the generated entity was never auto-selected after Generate" }
    );
    await browser.waitUntil(async () => (await ui.inspectorText()).includes("MeshRenderer"), {
      timeout: 15000,
      timeoutMsg: "the generated entity never exposed a MeshRenderer (the streamed-in mesh never landed)",
    });
    const streamedIn = (await ui.inspectorText()).includes("MeshRenderer");
    expect(streamedIn).toBe(true);

    const functional = placed && metered && streamedIn;

    // INVARIANT 3 (undoable): ONE undo removes the generated placeholder → the count returns to `countBefore`.
    await ui.undoKey();
    await browser.waitUntil(async () => (await entityCount()) <= countBefore, {
      timeout: 10000,
      timeoutMsg: `undo did not remove the generated placeholder (count ${await entityCount()}, want ≤ ${countBefore})`,
    });
    const inv3 = (await entityCount()) <= countBefore;
    expect(inv3).toBe(true);

    const errs = await consoleErrors();
    const clean = errs.length === 0;
    if (!clean) report.consoleErrorCount += errs.length;

    report.workflow(
      "generate (opt-in)",
      { functional, inv1: metered, inv3, p1_interactions: true /* describe + Generate = 2 */, clean },
      { commands: ["generate", "describe", "undo"] }
    );

    expect(functional).toBe(true);
    expect(inv3).toBe(true);
    expect(clean).toBe(true);
  });

  // ── PRINCIPLE 2: the interactive catalog ops hold ≤16 ms live (p50/p99) and within baseline. The
  // one-shot generate itself is a heavy async stream-in (excluded from the per-frame budget). ──────────
  it("PRINCIPLE 2: catalog + catalog_search round-trips are ≤16 ms live and within baseline", async () => {
    const ops = [
      await captureBudget("catalog", "catalog", {}, { n: 20, warmup: 4 }),
      await captureBudget("catalog_search", "catalog_search", { query: "health" }, { n: 20, warmup: 4 }),
    ];
    for (const s of ops) {
      const scored = await scoreBudget(s, baseline, {
        perFrame: true,
        recapture: () =>
          captureBudget(s.label, s.label, s.label === "catalog_search" ? { query: "health" } : {}, {
            n: 20,
            warmup: 4,
          }),
      });
      report.budget(scored);
      expect(scored.verdict, `${s.label}: ${scored.note}`).toBe("pass");
    }
  });
});
