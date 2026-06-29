// M12.6 — THE CAPSTONE ACCEPTANCE (north-star #6, audio-free subset) — the single integrated `.exe` run.
//
// Mirrors the M10.5 first-session pattern: the WHOLE slice is driven as ONE flow on the packaged React `.exe`
// (`MTK_UI=react`), so an INTEGRATED break — a freshly-composed Rule that won't fire in Play, a piece that
// isn't an ordinary entity, a chain Ctrl-Z can't unwind, an offline path that needs a socket — fails it, not
// just a step in isolation. Every assertion is off a STABLE state signal (entity counts, the compose result
// counts, the rule_debug truth-state fields, the decision history) — NEVER UI prose (test_and_ci rule 3).
//
// The flow: New -> assemble the knight + the rusty sword (made a dynamic physics body, the #3 pickup leg) ->
// ONE SENTENCE -> compose the flame quest (the #5 leg: KillCounter + QuestState machine + tally + ignite +
// the offered mirror, ALL via the validated pipeline) -> inspect each piece -> Play -> 4 kills in the boss
// arena -> the sword ignites -> the live truth-state shows WHY ("debug by looking") -> scrub the decision
// history -> Stop (the authored doc is intact) -> Ctrl-Z peels the whole chain back -> the offline leg.
//
// The audio leg ("footsteps that echo") and the browser-funnel box are NAMED SEAMS (M13+ audio / ADR-006) —
// accepted-or-owed honestly, never faked green.

import { browser, expect } from "@wdio/globals";
import { page } from "../../pages/scaffold.js";
import {
  report,
  invoke,
  consoleErrors,
  clearConsole,
  installConsoleGuard,
  captureBudget,
  scoreBudget,
  loadBaseline,
  ipcPerFrame,
} from "../../lib/acceptance.js";

const ui = page();
const baseline = loadBaseline();

// The one sentence the whole slice is composed from (the gates-doc test #6 scenario, audio-free).
const SENTENCE =
  "a knight picks up a rusty sword that bursts into flame after 4 kills in the boss arena";

const num = (s) => Number(String(s).match(/(\d+)/)?.[1] ?? NaN);
const ruleTruth = (info, id) => (info && info.truth ? info.truth.rules.find((r) => r.rule === id) : undefined);
const cond = (rt, component) => (rt ? rt.conditions.find((c) => c.component === component) : undefined);
const intOf = (v) => (v && typeof v === "object" ? v.Integer : undefined);

describe("acceptance / M12.6 — the capstone: one sentence -> a coherent, inspectable, undoable, playable slice", () => {
  before(async () => {
    await ui.waitConnected();
    await installConsoleGuard();
    await clearConsole();
  });

  it("THE CAPSTONE — one sentence becomes a playable multi-system slice, driven as ONE integrated run", async () => {
    // ── STEP 1: New project -> an empty scene ───────────────────────────────────────────────────────────
    await ui.newProject();
    await browser.waitUntil(async () => num(await ui.count()) === 0, {
      timeout: 10000,
      timeoutMsg: "New project did not yield an empty scene",
    });
    report.workflow("capstone/new-project", { functional: true, clean: true }, { commands: ["new_project"] });

    // ── STEP 2: Assemble the assets — a knight + a rusty sword (ordinary entities) ───────────────────────
    const knight = await invoke("create_entity", { x: -1, y: 0, z: 0, name: "Knight" });
    const sword = await invoke("create_entity", { x: 1, y: 0, z: 0, name: "Rusty Sword" });
    expect(knight).toBeTruthy();
    expect(sword).toBeTruthy();
    await browser.waitUntil(async () => num(await ui.count()) === 2, {
      timeout: 10000,
      timeoutMsg: "the knight + sword did not appear as two ordinary entities",
    });
    report.workflow(
      "capstone/assemble-assets",
      { functional: true, inv1: true, clean: true },
      { commands: ["create_entity"], controls: ["#hierarchy"] }
    );

    // ── STEP 3: Physics pickup (#3, shipped) — the sword becomes a dynamic body ──────────────────────────
    const madeDynamic = await invoke("make_dynamic", { id: sword });
    expect(madeDynamic).toBe(true);
    const [bodies] = await invoke("physics_debug");
    expect(bodies).toBeGreaterThan(0);
    report.workflow(
      "capstone/physics-pickup",
      { functional: bodies > 0, clean: true },
      { commands: ["make_dynamic", "physics_debug"] }
    );

    // ── STEP 4: ONE SENTENCE -> compose the flame quest (the #5 leg, validated pipeline) ─────────────────
    const proposal = await invoke("propose_composition", { sentence: SENTENCE, target: sword });
    expect(proposal.ok).toBe(true);
    expect(proposal.ops).toBe(7); // seeds (3) + the QuestState machine + tally + ignite + the offered mirror
    // Proposing must NOT apply anything (review-then-apply).
    expect((await invoke("list_rules")).length).toBe(0);

    const applied = await invoke("compose", { composition: proposal.composition });
    expect(applied.ok).toBe(true);
    expect(applied.applied).toBe(7);
    expect(applied.rules).toBe(3); // tally + ignite + the offered mirror "off switch"
    expect(applied.stateMachines).toBe(1); // the QuestState machine
    report.workflow(
      "capstone/compose-quest",
      { functional: true, inv1: true, inv3: true, p1_interactions: true, clean: true },
      { commands: ["propose_composition", "compose"], evidence: `${applied.applied} ops, ${applied.rules} rules` }
    );

    // ── STEP 5: Inspect — each piece is listed + an ordinary entity/component/rule (no god-object) ───────
    const rules = await invoke("list_rules");
    expect(rules.length).toBe(3);
    expect(rules.every((r) => r.event === "EnemyDied" || r.event === "ZoneExited")).toBe(true);
    const details = await invoke("entity_details", { id: sword });
    expect(details).toBeTruthy();
    // The quest's data is ordinary registry components on the sword (no privileged container).
    for (const c of ["KillCounter", "QuestState", "Flammable"]) {
      expect(details.components.includes(c)).toBe(true);
    }
    report.workflow(
      "capstone/inspect-ordinary",
      { functional: true, p1_explained: true, clean: true },
      { commands: ["list_rules", "entity_details"] }
    );

    // ── STEP 6: Play -> 4 kills in the boss arena -> the sword ignites ───────────────────────────────────
    const playInfo = await invoke("play");
    expect(playInfo.playing).toBe(true);

    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword }); // kill 1
    await invoke("fire_rule_event", { event: "ZoneEntered", subject: null, selected: sword }); // reach arena
    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword }); // kill 2
    const dbg3 = await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword }); // kill 3

    // ── STEP 7: the live truth-state shows WHY (debug by looking) — off STABLE fields, never prose ───────
    const ignite3 = ruleTruth(dbg3, "r_ai_ignite");
    expect(ignite3).toBeTruthy();
    expect(ignite3.fires).toBe(false); // not lit after only 3 kills
    const arena = cond(ignite3, "QuestState");
    expect(arena.satisfied).toBe(true); // reached the boss arena (state = FacingBoss)
    const kills = cond(ignite3, "KillCounter");
    expect(kills.satisfied).toBe(false); // the kill threshold is unmet
    expect(intOf(kills.actual)).toBe(3); // KillCounter = 3 of 4
    expect(intOf(kills.expected)).toBe(4);
    const machine3 = dbg3.truth.machines.find((m) => m.machine === "sm_ai_quest");
    expect(machine3.current).toBe("FacingBoss");
    report.workflow(
      "capstone/truth-state",
      { functional: true, clean: true },
      { commands: ["fire_rule_event", "rule_debug"], evidence: `${kills.display} | ${machine3.display}` }
    );

    // The 4th kill ignites the sword (the cascade: count reaches 4 AND ignites in the same tick).
    const dbg4 = await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword }); // kill 4
    const ignite4 = ruleTruth(dbg4, "r_ai_ignite");
    expect(ignite4.fires).toBe(true);
    expect(
      dbg4.decisions.some((d) => d.kind === "fieldSet" && d.component === "Flammable" && d.field === "lit")
    ).toBe(true);
    report.workflow(
      "capstone/ignite-after-4-kills",
      { functional: true, clean: true },
      { commands: ["fire_rule_event"], evidence: "the rusty sword bursts into flame on the 4th kill" }
    );

    // ── STEP 8: time-travel the decision history (the M8.4 channel) ──────────────────────────────────────
    const head = dbg4.head;
    const scrubbed = await invoke("rule_scrub", { frame: 4, selected: sword }); // back before the 4th kill
    expect(ruleTruth(scrubbed, "r_ai_ignite").fires).toBe(false);
    const resumed = await invoke("rule_scrub", { frame: head, selected: sword }); // resume to the ignite
    expect(ruleTruth(resumed, "r_ai_ignite").fires).toBe(true);
    report.workflow(
      "capstone/time-travel",
      { functional: true, clean: true },
      { commands: ["rule_scrub"] }
    );

    // ── STEP 9: Stop — the authored doc is intact (the ignite was a projection, never the doc) ───────────
    const stopInfo = await invoke("stop");
    expect(stopInfo.playing).toBe(false);
    // The authored rules + the scene are unchanged by running (non-destructive, ADR-021/034).
    expect((await invoke("list_rules")).length).toBe(3);
    await browser.waitUntil(async () => num(await ui.count()) === 2, {
      timeout: 10000,
      timeoutMsg: "Stop did not restore the authored scene",
    });
    // After Stop the Rules session is gone (not playing) — the runtime state was a projection, now dropped.
    expect((await invoke("rule_debug", { id: sword })).playing).toBe(false);
    report.workflow(
      "capstone/stop-restores",
      { functional: true, inv3: true, clean: true },
      { commands: ["stop", "rule_debug"] }
    );

    // ── STEP 10: Ctrl-Z peels the whole chain back as ordinary transactions ─────────────────────────────
    // Stop is a NON-DESTRUCTIVE snapshot-restore (a fresh engine + merge, ADR-034) — it deliberately resets
    // the in-memory undo stack (Stop is not itself an undoable edit). So the chain-unwind is demonstrated on a
    // live EDIT-state chain: re-assemble the slice, then Ctrl-Z peels every leg back as an ordinary tx.
    await ui.newProject();
    await browser.waitUntil(async () => num(await ui.count()) === 0, { timeout: 10000, timeoutMsg: "not empty for re-assemble" });
    const knight2 = await invoke("create_entity", { x: -1, y: 0, z: 0, name: "Knight" });
    void knight2;
    const sword2 = await invoke("create_entity", { x: 1, y: 0, z: 0, name: "Rusty Sword" });
    await invoke("make_dynamic", { id: sword2 });
    const p2 = await invoke("propose_composition", { sentence: SENTENCE, target: sword2 });
    const a2 = await invoke("compose", { composition: p2.composition });
    expect(a2.rules).toBe(3);
    await browser.waitUntil(async () => num(await ui.count()) === 2, { timeout: 10000, timeoutMsg: "re-assemble did not produce the slice" });

    // Ctrl-Z #1 — the compose (one `ai-compose` tx): the whole quest unwinds at once (genuine Ctrl-Z keydown).
    await ui.undoKey();
    await browser.waitUntil(async () => (await invoke("list_rules")).length === 0, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not peel back the composed quest",
    });
    expect((await invoke("state_machines")).length).toBe(0);
    // Ctrl-Z #2-4 — make-dynamic, then the sword, then the knight -> the empty scene (one Ctrl-Z per tx).
    for (let i = 0; i < 3; i++) {
      await ui.undoKey();
      await browser.pause(150);
    }
    await browser.waitUntil(async () => num(await ui.count()) === 0, {
      timeout: 10000,
      timeoutMsg: "Ctrl-Z did not peel the whole chain back to an empty scene",
    });
    report.workflow(
      "capstone/ctrl-z-peels-chain",
      { functional: true, inv3: true, clean: true },
      { commands: ["undo", "list_rules", "state_machines"] }
    );

    // ── CLEAN: no console errors across the WHOLE integrated run ─────────────────────────────────────────
    expect(await consoleErrors()).toEqual([]);
  });

  it("OFFLINE LEG — the slice composes + plays with no network; the paid tiers degrade to honest seams", async () => {
    // The build has NO real network providers — the compose is the OFFLINE demo composer, the marketplace is a
    // checked-in catalog, and a real LLM is a documented seam. Assemble + compose + Play with nothing but
    // local `/core`, and assert the paid tier is an explained seam (never a crash or a silent fake).
    await clearConsole();
    await ui.newProject();
    await browser.waitUntil(async () => num(await ui.count()) === 0, { timeout: 10000, timeoutMsg: "not empty" });

    const sword = await invoke("create_entity", { x: 0, y: 1, z: 0, name: "Rusty Sword" });
    await invoke("make_dynamic", { id: sword });
    // ONE SENTENCE -> the quest, composed OFFLINE (the demo composer, no model/socket).
    const proposal = await invoke("propose_composition", { sentence: SENTENCE, target: sword });
    expect(proposal.ok).toBe(true);
    const applied = await invoke("compose", { composition: proposal.composition });
    expect(applied.ok).toBe(true);
    // Play it offline — the quest ignites with no network.
    await invoke("play");
    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    await invoke("fire_rule_event", { event: "ZoneEntered", subject: null, selected: sword });
    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    const dbg = await invoke("fire_rule_event", { event: "EnemyDied", subject: null, selected: sword });
    const litOffline = ruleTruth(dbg, "r_ai_ignite")?.fires === true;
    await invoke("stop");

    // The paid asset tier (a no-local-match describe) degrades to an honest, explained seam — not a crash.
    const desc = await invoke("describe", { query: "an ornate dragon-bone greatsword" }).catch((e) => ({
      seam: String((e && e.message) || e),
    }));
    const honestSeam =
      !!desc &&
      (typeof desc.seam === "string" ||
        desc.source === "marketplace" ||
        desc.source === "generate" ||
        desc.created != null);
    report.workflow(
      "capstone/offline-leg",
      { functional: litOffline, inv1: true, clean: (await consoleErrors()).length === 0, offline: litOffline && honestSeam },
      { commands: ["propose_composition", "compose", "fire_rule_event", "describe"] }
    );
    expect(litOffline).toBe(true);
    expect(honestSeam).toBe(true);
    report.offline = { localPathsWork: true, paidSeamHonest: honestSeam };
  });

  it("INVARIANT 4 — composing + playing the slice never crosses JS per-frame (0 hot-path IPC)", async () => {
    const perFrame = await ipcPerFrame(() => ui.orbit(120, 60), 600);
    report.workflow(
      "capstone/inv4-hot-path",
      { functional: perFrame < 1, inv4: perFrame < 1, clean: true },
      { commands: ["drag_start", "drag_end"], evidence: `${perFrame.toFixed(3)} IPC/frame` }
    );
    expect(perFrame).toBeLessThan(1);
  });

  it("PRINCIPLE 2 — composing the quest holds the interaction budget (discrete op, recorded)", async () => {
    // Build the composition once on a real sword, then re-apply it N times (the same ids overwrite — a clean
    // authoring tx each time) to measure the compose round-trip. Discrete (not per-frame): assert <=16 ms AND
    // within baseline tolerance if a baseline entry exists.
    await ui.newProject();
    const sword = await invoke("create_entity", { x: 0, y: 1, z: 0, name: "Rusty Sword" });
    const p = await invoke("propose_composition", { sentence: SENTENCE, target: sword });
    expect(p.ok).toBe(true);
    const s = await captureBudget("compose", "compose", { composition: p.composition }, { n: 12, warmup: 3 });
    console.log("BUDGET compose p50=", s.p50.toFixed(2), "p99=", s.p99.toFixed(2), "max=", s.max.toFixed(2));
    const scored = await scoreBudget(s, baseline, {
      perFrame: false,
      recapture: () => captureBudget("compose", "compose", { composition: p.composition }, { n: 12, warmup: 3 }),
    });
    report.budget(scored);
    expect(scored.verdict).toBe("pass");
  });

  after(() => {
    try {
      report.writeArtifacts(ui.inventory ? ui.inventory() : [], {
        reportName: "acceptance-report.json",
        coverageName: "COVERAGE.md",
      });
    } catch {
      /* artifacts are a reporting nicety */
    }
  });
});
