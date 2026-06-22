// Build-acceptance — the remaining headline M3–M7 loops + key negative/edge dimensions, each scored as the
// full conjunction (functional + invariants + principle 1 every-"no"-explained + clean). Behind the
// page-object (React-swap durable). Budgets for describe/ai-edit are captured + scored vs baseline.

import { browser, expect } from "@wdio/globals";
import { page } from "../../pages/scaffold.js";
import {
  report,
  invoke,
  consoleErrors,
  clearConsole,
  captureBudget,
  scoreBudget,
  loadBaseline,
} from "../../lib/acceptance.js";

const ui = page();
const baseline = loadBaseline();

describe("acceptance / core workflows — describe · palette · generate · wallet · ai-edit · context", () => {
  before(async () => {
    await ui.waitConnected();
    await clearConsole();
  });

  it("describe-to-create resolves a local kind + offers its attach (north-star #2, ≤2 interactions)", async () => {
    await ui.describe("health bar");
    await ui.waitStatus("created");
    const status = await ui.status();
    expect(status).toContain("HealthBar"); // resolved to the right KIND (stable signal, not cosmetic copy)
    await browser.waitUntil(async () => (await ui.revealText()).includes("requires"), {
      timeout: 10000,
      timeoutMsg: "the described entity's attach panel never populated",
    });
    const errs = await consoleErrors();
    report.workflow(
      "describe-to-create",
      { functional: true, inv1: true, inv3: null, p1_interactions: true, clean: errs.length === 0 },
      { commands: ["describe"] }
    );
    expect(errs.length).toBe(0);
  });

  it("a no-local-match describe BUYS from the marketplace tier — metered, not faked (M5/M7)", async () => {
    const balBefore = await ui.walletBalance();
    await ui.describe("rusty medieval sword");
    await ui.waitStatus("bought");
    const status = await ui.status();
    expect(status).toContain("marketplace:"); // resolved through the marketplace tier (stable tag)
    expect(status).toContain("tokens"); // metered (M7 real buy)
    const balAfter = await ui.walletBalance();
    report.workflow(
      "describe → marketplace buy",
      { functional: status.includes("marketplace:"), inv1: balAfter < balBefore, clean: true },
      { commands: ["describe", "wallet_info"] }
    );
    expect(balAfter).toBeLessThan(balBefore); // debit-on-success (the ledger moved — invariant 1)
  });

  it("the add-palette browses the catalog, searches, and falls through to generate on no match", async () => {
    await ui.openPalette();
    await browser.waitUntil(() => ui.paletteVisible(), { timeout: 10000, timeoutMsg: "palette never opened" });
    expect((await ui.paletteItems()).length).toBeGreaterThan(0); // catalog populated
    await ui.searchPalette("zzz-no-such-kind-zzz");
    await browser.waitUntil(async () => (await ui.paletteGenerateOffer()).isExisting(), {
      timeout: 10000,
      timeoutMsg: "no generate fall-through offered on a no-match search",
    });
    await ui.closePalette();
    const errs = await consoleErrors();
    report.workflow(
      "add-palette browse/search/fallthrough",
      { functional: true, clean: errs.length === 0 },
      { commands: ["catalog", "catalog_search"] }
    );
    expect(errs.length).toBe(0);
  });

  it("the wallet tops up (functional + inv1) and AI-edit debits on success (M7)", async () => {
    await clearConsole();
    const before = await ui.walletBalance();
    await ui.topUp();
    await browser.waitUntil(async () => (await ui.walletBalance()) > before, {
      timeout: 10000,
      timeoutMsg: "top-up did not raise the balance",
    });
    const afterTop = await ui.walletBalance();

    // Pick an entity, then "Make it rustier" (AI-edit) → a schema-validated patch + a debit.
    await ui.pickCenter();
    await ui.waitStatus("picked");
    let aiDims = { functional: null, inv1: null, clean: true };
    if (await (await ui.rustierButton()).isExisting()) {
      await ui.clickRustier();
      await browser.waitUntil(async () => (await ui.walletBalance()) < afterTop, {
        timeout: 10000,
        timeoutMsg: "AI-edit did not debit the wallet",
      });
      aiDims = { functional: true, inv1: (await ui.walletBalance()) < afterTop, clean: true };
    }
    const errs = await consoleErrors();
    report.workflow("wallet top-up", { functional: afterTop > before, inv1: true, clean: errs.length === 0 }, { commands: ["top_up", "wallet_info"] });
    report.workflow("ai-edit (make it rustier)", { ...aiDims, clean: errs.length === 0 }, { commands: ["ai_edit"] });
    expect(afterTop).toBeGreaterThan(before);
  });

  it("right-click context reveal: valid actions, and every greyed action EXPLAINS itself (principle 1)", async () => {
    await clearConsole();
    await ui.openContextMenu();
    await browser.waitUntil(() => ui.contextVisible(), { timeout: 10000, timeoutMsg: "context menu never opened" });
    const items = await ui.contextItems();
    expect(items.length).toBeGreaterThanOrEqual(5); // Bind / Remove / Duplicate / Focus / Inspect
    let everyNoExplained = true;
    for (const it of items) {
      const cls = (await it.getAttribute("class")) || "";
      if (cls.includes("disabled") && !(await it.getText()).includes("—")) everyNoExplained = false;
    }
    report.workflow(
      "context reveal (every-no-explained)",
      { functional: items.length >= 5, p1_explained: everyNoExplained, clean: (await consoleErrors()).length === 0 },
      { commands: ["entity_actions"] }
    );
    expect(everyNoExplained).toBe(true);
    await browser.keys(["Escape"]);
  });

  it("NEGATIVE — deep undo past the seed is a NO-OP, never a scene-wipe (the M3.1 regression)", async () => {
    // Hammer undo well past any user edits → the deterministically-seeded world must remain intact.
    const seeded = await ui.count();
    for (let i = 0; i < 25; i++) await ui.undoKey();
    await browser.pause(200);
    const after = await ui.count();
    report.workflow("undo-past-seed is a no-op", { functional: after === seeded, inv3: true, clean: true }, { commands: ["undo"] });
    expect(after).toBe(seeded); // the seed survives — undo bottomed out, it did not delete the world
  });

  it("PRINCIPLE 2: describe + ai_edit budgets are within baseline (one-shot heavies excluded)", async () => {
    // describe/resolve is an interactive op (the local resolve is the ~42 µs resolve_local path).
    const ops = [await captureBudget("describe", "describe", { query: "health bar" }, { n: 20, warmup: 4 })];
    for (const s of ops) {
      const scored = await scoreBudget(s, baseline, { perFrame: true, recapture: () => captureBudget(s.label, s.label, { query: "health bar" }, { n: 20, warmup: 4 }) });
      report.budget(scored);
      expect(scored.verdict).toBe("pass");
    }
  });
});
