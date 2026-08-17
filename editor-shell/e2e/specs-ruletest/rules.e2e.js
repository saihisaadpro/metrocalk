// LIVE M12.1 Rules verification (ADR-045). Drives the packaged .exe: the registry-fed builder vocabulary,
// authoring a multi-part conditional (typo-proof), the Blocked+explained refusal of an unknown event, the
// proactively-offered mirror "cleanup" rule, the Rule list, undo, AND that the builder renders in the real
// React DOM fed by the live registry. Writes results.json. The persist/reload + undo data model are also
// covered headless (core + editor-shell tests); this is the live, on-.exe acceptance.

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

const fv = { Integer: (n) => ({ Integer: n }), Str: (s) => ({ Str: s }), Bool: (b) => ({ Bool: b }) };
const results = [];

describe("LIVE Rules layer (M12.1)", () => {
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

  it("registry-fed vocabulary, typo-proof authoring, Blocked-explained, the mirror offer, list + undo", async () => {
    // 1) The builder's vocabulary is fed by the registry (typo-proof).
    const reg = await invoke("rule_registry");
    results.push({ step: "rule_registry", events: reg.events.map((e) => e.name), actions: reg.actions.map((a) => a.name) });
    const eventNames = reg.events.map((e) => e.name);
    const compNames = reg.components.map((c) => c.name);
    const actionNames = reg.actions.map((a) => a.name);
    if (!eventNames.includes("EnemyDied") || !eventNames.includes("StateEntered"))
      throw new Error(`events missing: ${eventNames}`);
    if (!compNames.includes("KillCounter") || !compNames.includes("Flammable"))
      throw new Error(`components missing: ${compNames}`);
    if (!actionNames.includes("SetField")) throw new Error(`actions missing: ${actionNames}`);

    // A real entity to target (the "sword").
    const sword = await invoke("create_entity", { x: 0, y: 1, z: 0, name: "sword" });
    if (typeof sword !== "string") throw new Error(`create_entity returned ${JSON.stringify(sword)}`);

    // 2) Author the conditional by structured data (what the builder produces): "when entering FacingBoss,
    //    if QuestState.state == FacingBoss, ignite the sword" — an enter+set-bool rule, so a mirror is offered.
    const onRule = {
      name: "flame on",
      enabled: true,
      event: "StateEntered",
      conditions: [{ entity: sword, component: "QuestState", field: "state", op: "eq", value: fv.Str("FacingBoss") }],
      actions: [{ action: "SetField", entity: sword, component: "Flammable", field: "lit", value: fv.Bool(true) }],
    };
    const authored = await invoke("author_rule", { rule: onRule, id: null });
    results.push({ step: "author_rule", result: authored });
    if (typeof authored.id !== "string") throw new Error(`author failed: ${JSON.stringify(authored)}`);
    if (authored.error) throw new Error(`unexpected error: ${authored.error}`);
    if (!authored.mirror || authored.mirror.event !== "StateExited")
      throw new Error(`no mirror offered: ${JSON.stringify(authored.mirror)}`);
    if (authored.mirror.actions[0].value.Bool !== false)
      throw new Error("the mirror should turn the effect OFF");

    // 3) It shows in the Rule list.
    let rules = await invoke("list_rules");
    if (rules.length !== 1 || rules[0].rule.event !== "StateEntered") throw new Error(`list_rules wrong: ${JSON.stringify(rules)}`);

    // 4) An UNKNOWN event is Blocked + explained (ADR-016) — refused, not committed. (Resolve with an
    //    `error`, or — defensively — surface the explanation as a rejection; both prove Blocked+explained.)
    let bad;
    try {
      bad = await invoke("author_rule", { rule: { ...onRule, event: "Frobnicate" }, id: null });
    } catch (e) {
      bad = { id: null, error: String((e && e.message) || e) };
    }
    results.push({ step: "blocked", result: bad });
    if (bad.id || !bad.error || !/event/i.test(bad.error))
      throw new Error(`an unknown event should be Blocked+explained: ${JSON.stringify(bad)}`);
    if ((await invoke("list_rules")).length !== 1) throw new Error("the blocked rule must NOT have been committed");

    // 5) Accept the offered mirror → both rules exist.
    const mirrorRes = await invoke("author_rule", { rule: authored.mirror, id: null });
    if (typeof mirrorRes.id !== "string") throw new Error(`mirror author failed: ${JSON.stringify(mirrorRes)}`);
    if ((await invoke("list_rules")).length !== 2) throw new Error("both the rule and its mirror should be listed");

    // 6) Undo removes the last authored rule (one undoable transaction).
    await invoke("undo");
    rules = await invoke("list_rules");
    if (rules.length !== 1) throw new Error(`undo should leave 1 rule, got ${rules.length}`);

    // 7) The registry-fed builder renders in the REAL React DOM (WebView2-visible).
    const panel = await browser.$("#rules");
    if (!(await panel.isExisting())) throw new Error("the Rules panel is not mounted");
    await (await browser.$('[data-testid="rule-new"]')).click();
    await browser.waitUntil(async () => (await browser.$('[data-testid="rule-builder"]')).isExisting(), {
      timeout: 5000,
      timeoutMsg: "the registry-fed builder did not open",
    });
    const eventOpts = await browser.execute(() => {
      const sel = document.querySelector('[data-testid="rule-event"]');
      return sel ? [...sel.options].map((o) => o.value) : [];
    });
    results.push({ step: "dom-builder", eventOptions: eventOpts });
    if (!eventOpts.includes("EnemyDied")) throw new Error(`the live registry did not feed the DOM dropdown: ${eventOpts}`);

    console.log("PASS: live registry-fed authoring + Blocked-explained + mirror + list + undo + DOM builder.");
  });

  after(async () => {
    writeFileSync(path.join(OUT_DIR, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\nWROTE ${results.length} steps → ${path.join(OUT_DIR, "results.json")}`);
  });
});
