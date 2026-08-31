// LIVE M12.5 Rules-in-Play + truth-state debugger verification (ADR-049). Drives the packaged .exe: authored
// Rules + a state machine EXECUTE in Play as a projection (the authored doc is never mutated), the live
// truth-state is VISIBLE on click ("❌ KillCounter = 3 of 4", "✅ state = FacingBoss" — debug by looking, not
// Debug.Log), the 4th kill ignites the sword, the decision history SCRUBS backward (watch when the counter
// incremented), and STOP restores. AND the RuleDebugPanel renders + drives this live in the real React DOM.
// The runtime + determinism + scrub-replay are also covered headless (core + editor-shell tests); this is the
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

const fv = { Integer: (n) => ({ Integer: n }), Str: (s) => ({ Str: s }), Bool: (b) => ({ Bool: b }) };
const results = [];

/** Find the rule's truth in a rule_debug payload. */
const ruleTruth = (info, id) => (info.truth ? info.truth.rules.find((r) => r.rule === id) : undefined);

describe("LIVE Rules in Play + truth-state debugger (M12.5)", () => {
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

  it("Rules run in Play as a projection; the live truth-state is visible; history scrubs; Stop restores", async () => {
    // ── Author the scene: a sword carrying the test-#5 components + the kill-tally rule + the ignite rule. ──
    const sword = await invoke("create_entity", { x: 0, y: 1, z: 0, name: "sword" });
    if (typeof sword !== "string") throw new Error(`create_entity returned ${JSON.stringify(sword)}`);

    // Seed its components (multi_edit = one undoable SetField per call).
    //
    // THREE OF THESE FOUR COULD NOT HAVE WORKED BEFORE ADR-169. `multi_edit`'s `value` parameter was
    // declared `f64`, so a string or a boolean argument failed to deserialise and the invoke rejected;
    // the results were never checked, so the seeding silently did nothing and the rules below read
    // whatever the defaults were. Widening the parameter to any scalar is what makes them real, and
    // each one is now checked — an unchecked seed is how this went unnoticed.
    for (const [component, field, value] of [
      ["KillCounter", "count", 0],
      ["Zone", "current", "BossArena"],
      ["Flammable", "lit", false],
      ["QuestState", "state", "Hunting"],
    ]) {
      const seeded = await invoke("multi_edit", { ids: [sword], component, field, value });
      if (!seeded || seeded.ok !== true)
        throw new Error(`seeding ${component}.${field} failed: ${JSON.stringify(seeded)}`);
    }

    // r_count: When EnemyDied -> AdjustCounter count += 1.
    const count = {
      name: "tally kills",
      enabled: true,
      event: "EnemyDied",
      conditions: [],
      actions: [{ action: "AdjustCounter", entity: sword, component: "KillCounter", field: "count", value: fv.Integer(1) }],
    };
    if (typeof (await invoke("author_rule", { rule: count, id: "r_count" })).id !== "string")
      throw new Error("count rule failed to author");

    // r_ignite: When EnemyDied, If count>=4 AND Zone==BossArena, Then Flammable.lit = true.
    const ignite = {
      name: "rusty sword ignites",
      enabled: true,
      event: "EnemyDied",
      conditions: [
        { entity: sword, component: "KillCounter", field: "count", op: "ge", value: fv.Integer(4) },
        { entity: sword, component: "Zone", field: "current", op: "eq", value: fv.Str("BossArena") },
      ],
      actions: [{ action: "SetField", entity: sword, component: "Flammable", field: "lit", value: fv.Bool(true) }],
    };
    if (typeof (await invoke("author_rule", { rule: ignite, id: "r_ignite" })).id !== "string")
      throw new Error("ignite rule failed to author");

    // The QuestState machine: Hunting --(ZoneEntered)--> FacingBoss (walking to the arena advances the quest).
    const machine = {
      name: "quest",
      entity: sword,
      component: "QuestState",
      field: "state",
      states: ["Hunting", "FacingBoss"],
      initial: "Hunting",
      transitions: [
        {
          id: "t_arena",
          from: "Hunting",
          to: "FacingBoss",
          rule: {
            name: "enter arena",
            enabled: true,
            event: "ZoneEntered",
            conditions: [],
            actions: [{ action: "SetField", entity: sword, component: "QuestState", field: "state", value: fv.Str("FacingBoss") }],
          },
        },
      ],
    };
    const sm = await invoke("author_state_machine", { sm: machine, id: "sm_quest" });
    if (typeof sm.id !== "string") throw new Error(`machine failed to author: ${JSON.stringify(sm)}`);

    // ── PLAY: enter the runtime. ──
    const playInfo = await invoke("play");
    results.push({ step: "play", result: playInfo });
    if (!playInfo.playing) throw new Error("Play did not enter play mode");

    // ── Run the quest: walk to the arena (the machine advances), then kill 3 enemies. ──
    await invoke("fire_rule_event", { event: "ZoneEntered", subject: sword, selected: sword });
    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    let dbg = await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    results.push({ step: "after-3-kills", frame: dbg.frame, head: dbg.head });

    // ── The live truth-state on click: ✅ state = FacingBoss, ❌ KillCounter 3 of 4 (the sword does NOT burn). ──
    const t = ruleTruth(dbg, "r_ignite");
    if (!t) throw new Error(`the ignite rule isn't in the truth-state: ${JSON.stringify(dbg.truth)}`);
    if (t.fires) throw new Error("the sword must NOT burn after only 3 kills");
    const counterCond = t.conditions[0];
    if (counterCond.satisfied) throw new Error("the kill threshold must be unmet at 3");
    if (!counterCond.actual || counterCond.actual.Integer !== 3) throw new Error(`expected count 3, got ${JSON.stringify(counterCond.actual)}`);
    if (counterCond.expected.Integer !== 4) throw new Error(`expected threshold 4, got ${JSON.stringify(counterCond.expected)}`);
    const fb = dbg.truth.machines.find((m) => m.machine === "sm_quest");
    if (!fb || fb.current !== "FacingBoss") throw new Error(`the machine should be at FacingBoss: ${JSON.stringify(fb)}`);
    results.push({ step: "truth-state-3of4", display: counterCond.display, machine: fb.current });

    // ── Running Rules are a PROJECTION: the AUTHORED doc is untouched — list_rules still has both, and a Stop
    //    will restore the unlit sword. (The bit-exact doc guard is proven headless; here we confirm the run
    //    didn't corrupt the authoring.) ──
    if ((await invoke("list_rules")).length !== 2) throw new Error("running rules must not change the authored rule set");

    // ── The 4th kill IGNITES the sword (the projection sees the fire). ──
    dbg = await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    const t4 = ruleTruth(dbg, "r_ignite");
    if (!t4.fires) throw new Error("the 4th kill should ignite the sword (count reaches 4)");
    results.push({ step: "ignite-on-4th", fires: t4.fires, frame: dbg.frame });
    // The decision history records the ignite (a fieldSet on Flammable.lit).
    if (!dbg.decisions.some((d) => d.kind === "fieldSet" && d.component === "Flammable" && d.field === "lit"))
      throw new Error("the ignite must be in the decision history");

    // ── TIME-TRAVEL: scrub backward to watch exactly when the counter was 2 (box 4). The events are
    //    ZoneEntered@0, EnemyDied@1,2,3,4; seeking to frame 3 replays frames 0..2 (two kills) -> count 2,
    //    before the sword ignited. ──
    const head = dbg.head;
    const scrubbed = await invoke("rule_scrub", { frame: 3, selected: sword });
    results.push({ step: "scrub-to-2-kills", frame: scrubbed.frame, count: ruleTruth(scrubbed, "r_ignite")?.conditions[0]?.actual });
    const scrubTruth = ruleTruth(scrubbed, "r_ignite");
    const scrubCount = scrubTruth.conditions[0].actual;
    if (!scrubCount || scrubCount.Integer !== 2) throw new Error(`scrub to frame 3 should show count 2, got ${JSON.stringify(scrubCount)}`);
    if (scrubTruth.fires) throw new Error("scrubbed back before the ignite — the sword must not be on fire there");
    // Resume to the head — deterministic-by-rebuild (the runtime returns to ignited).
    const resumed = await invoke("rule_scrub", { frame: head, selected: sword });
    if (!ruleTruth(resumed, "r_ignite").fires) throw new Error("resuming to the head should be ignited again");

    // ── STOP: leave Play — the runtime session is dropped (a projection), the authored doc restored. ──
    const stopInfo = await invoke("stop");
    results.push({ step: "stop", result: stopInfo });
    if (stopInfo.playing) throw new Error("Stop did not exit play mode");
    const afterStop = await invoke("rule_debug", { id: sword });
    if (afterStop.playing) throw new Error("after Stop the Rules session must be gone (not playing)");
    if ((await invoke("list_rules")).length !== 2) throw new Error("Stop must leave the authored rules intact (non-destructive)");

    // ── DOM: the RuleDebugPanel renders + drives the live debugger (Play -> select -> fire -> scrub). ──
    await invoke("play");
    await browser.waitUntil(async () => (await browser.$("#ruleDebug")).isExisting(), {
      timeout: 8000,
      timeoutMsg: "the RuleDebugPanel did not mount in Play",
    });
    // Select the sword via its hierarchy row so the panel has a target.
    await browser.waitUntil(async () => (await browser.$('[data-testid="hrow"]')).isExisting(), {
      timeout: 5000,
      timeoutMsg: "the sword never appeared in the hierarchy",
    });
    await (await browser.$('[data-testid="hrow"]')).click();
    // Fire a kill from the DOM button — the truth-state updates live.
    await (await browser.$('[data-testid="fireEnemyDied"]')).click();
    await browser.waitUntil(async () => (await browser.$('[data-testid="truthRule-r_ignite"]')).isExisting(), {
      timeout: 8000,
      timeoutMsg: "the rule truth-state never rendered in the DOM",
    });
    const condEl = await browser.$('[data-testid="truthCond-r_ignite-0"]');
    const satisfied = await condEl.getAttribute("data-satisfied");
    results.push({ step: "dom-truth", satisfied });
    // After 1 DOM kill the threshold is unmet — the ❌ is visible (asserted off the STABLE flag, not copy).
    if (satisfied !== "false") throw new Error(`the DOM truth-state should show the unmet threshold: ${satisfied}`);
    // The scrubber is present (time-travel control).
    if (!(await (await browser.$('[data-testid="ruleScrub"]')).isExisting())) throw new Error("the decision-history scrubber is not mounted");
    await invoke("stop");

    console.log("PASS: live Rules-in-Play projection + truth-state (3 of 4) + ignite + history scrub + Stop + DOM panel.");
  });

  after(async () => {
    writeFileSync(path.join(OUT_DIR, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\nWROTE ${results.length} steps → ${path.join(OUT_DIR, "results.json")}`);
  });
});
