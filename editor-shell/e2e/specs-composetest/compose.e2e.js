// LIVE M12.4 AI-compose verification (ADR-048). Drives the packaged .exe: a natural-language sentence becomes
// a reviewable Composition (propose_composition), which applies through the SAME validated commit pipeline a
// human uses (compose → one undoable transaction), a bad sentence + a deliberately-invalid composition are
// each rejected-as-UX with a plain-language reason (nothing applied), AND the ComposePanel renders in the real
// React DOM: selecting an entity, typing the sentence, Propose → review the patches → Apply authors the rule
// live. The data model + the seam are also covered headless (core + mcp + editor-shell tests); this is the
// live, on-.exe acceptance. Writes results.json. Clean, empty seed (MTK_SCENE_N=0).

import { browser } from "@wdio/globals";
import { writeFileSync, mkdirSync } from "node:fs";
import path from "node:path";

const OUT_DIR = process.env.MTK_OUT_DIR;
mkdirSync(OUT_DIR, { recursive: true });

const invoke = (cmd, args = {}) =>
  browser.execute(async (c, a) => window.__TAURI__.core.invoke(c, a), cmd, args);

const countEntities = async () => {
  try {
    const el = await browser.$("#count");
    const m = (await el.getText()).match(/(\d+)\s+entities/);
    return m ? Number(m[1]) : NaN;
  } catch {
    return NaN;
  }
};

const SENTENCE = "when an enemy dies and kills reach 4, set it on fire";
const results = [];

describe("LIVE AI compose tier (M12.4)", () => {
  before(async () => {
    await browser.waitUntil(async () => Number.isFinite(await countEntities()), {
      timeout: 30000,
      timeoutMsg: "editor never connected (#count empty)",
    });
    await browser.execute(() => {
      try {
        localStorage.setItem("mtk.onboarded.v1", "1");
      } catch (e) {
        void e;
      }
    });
    await browser.pause(300);
  });

  it("propose -> review -> apply (one undoable tx), and both bad paths are rejected-explained — bridge + DOM", async () => {
    // A real entity for the rule to act on (the "sword").
    const sword = await invoke("create_entity", { x: 0, y: 1, z: 0, name: "sword" });
    if (typeof sword !== "string") throw new Error(`create_entity returned ${JSON.stringify(sword)}`);

    // ── 1) PROPOSE: a sentence becomes a reviewable composition (nothing applied yet). ──
    const proposal = await invoke("propose_composition", { sentence: SENTENCE, target: sword });
    results.push({ step: "propose", result: proposal });
    if (!proposal.ok || !proposal.composition) throw new Error(`propose failed: ${JSON.stringify(proposal)}`);
    // The complete self-driving quest (M12.6): seeds (3) + the QuestState machine + tally + ignite + mirror.
    if (proposal.ops !== 7) throw new Error(`expected 7 ops (the full quest), got ${proposal.ops}`);
    if ((await invoke("list_rules")).length !== 0) throw new Error("proposing must NOT apply anything");

    // ── 2) APPLY: the reviewed composition commits as ONE undoable transaction. ──
    const applied = await invoke("compose", { composition: proposal.composition });
    results.push({ step: "compose", result: applied });
    if (!applied.ok || applied.applied !== 7 || applied.rules !== 3 || applied.stateMachines !== 1)
      throw new Error(`apply wrong: ${JSON.stringify(applied)}`);
    let rules = await invoke("list_rules");
    if (rules.length !== 3 || !rules.some((r) => r.rule.event === "EnemyDied"))
      throw new Error(`the AI-composed rules aren't listed: ${JSON.stringify(rules)}`);

    // ── 3) UNDO reverts the WHOLE composition (the AI is not a privileged path — one transaction). ──
    await invoke("undo");
    if ((await invoke("list_rules")).length !== 0) throw new Error("one undo should remove the composed rule");

    // ── 4) A bad SENTENCE is an honest, explained miss (rejected-as-UX, nothing applied). The explanation
    //    arrives either as a resolved `{ ok:false, error }` OR — defensively — as a rejection (both prove the
    //    every-"no" guarantee, the same convention as the Rules e2e). ──
    let badSentence;
    try {
      badSentence = await invoke("propose_composition", { sentence: "make the level shiny and purple", target: sword });
    } catch (e) {
      badSentence = { ok: false, error: String((e && e.message) || e) };
    }
    results.push({ step: "bad-sentence", result: badSentence });
    if (badSentence.ok || !badSentence.error || !/recognize|compose/i.test(badSentence.error))
      throw new Error(`an unrecognized sentence should be explained: ${JSON.stringify(badSentence)}`);

    // ── 5) A deliberately-INVALID composition is rejected by the pipeline + explained (the every-"no" gate). ──
    const badComposition = {
      ops: [
        {
          op: "authorRule",
          id: "r_bad",
          rule: {
            name: "bad",
            enabled: true,
            event: "Frobnicate", // not a registry event
            conditions: [],
            actions: [{ action: "SetField", entity: sword, component: "Flammable", field: "lit", value: { Bool: true } }],
          },
        },
      ],
    };
    let rejected;
    try {
      rejected = await invoke("compose", { composition: badComposition });
    } catch (e) {
      rejected = { ok: false, error: String((e && e.message) || e) };
    }
    results.push({ step: "bad-composition", result: rejected });
    if (rejected.ok || !rejected.error || !/event/i.test(rejected.error))
      throw new Error(`an invalid composition should be Blocked+explained: ${JSON.stringify(rejected)}`);
    if ((await invoke("list_rules")).length !== 0) throw new Error("a rejected composition must apply nothing");

    // ── 6) DOM: the ComposePanel renders + drives the live pipeline (select -> type -> Propose -> Apply). ──
    const panel = await browser.$("#compose");
    if (!(await panel.isExisting())) throw new Error("the Compose panel is not mounted");

    // Select the sword by clicking its hierarchy row (so the panel has a target).
    await browser.waitUntil(async () => (await browser.$('[data-testid="hrow"]')).isExisting(), {
      timeout: 5000,
      timeoutMsg: "the created entity never appeared in the hierarchy",
    });
    await (await browser.$('[data-testid="hrow"]')).click();

    const input = await browser.$('[data-testid="compose-sentence"]');
    await input.setValue(SENTENCE);
    await (await browser.$('[data-testid="compose-propose"]')).click();

    await browser.waitUntil(async () => (await browser.$('[data-testid="compose-proposal"]')).isExisting(), {
      timeout: 8000,
      timeoutMsg: "the DOM Propose did not surface a reviewable proposal",
    });
    const opcount = await (await browser.$('[data-testid="compose-opcount"]')).getText();
    results.push({ step: "dom-propose", opcount });
    if (opcount !== "7") throw new Error(`DOM proposal op count wrong: ${opcount}`);

    await (await browser.$('[data-testid="compose-apply"]')).click();
    await browser.waitUntil(async () => (await invoke("list_rules")).length === 3, {
      timeout: 8000,
      timeoutMsg: "the DOM Apply did not author the rules live",
    });
    results.push({ step: "dom-apply", rules: (await invoke("list_rules")).length });

    console.log("PASS: live propose -> review -> apply (one tx) + undo + bad-sentence + bad-composition + DOM panel.");
  });

  after(async () => {
    writeFileSync(path.join(OUT_DIR, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\nWROTE ${results.length} steps → ${path.join(OUT_DIR, "results.json")}`);
  });
});
