// LIVE M12.2 state-machine verification (ADR-046). Drives the packaged .exe: authoring a state machine as
// structured data (states + transitions, each transition an M12.1 Rule whose action enters `to`), a DANGLING
// transition Blocked+explained (ADR-016), the reachability warning, server-stamped STABLE transition ids,
// undo-removes-a-transition (one undoable tx), AND that the visual state-graph (the reused M2.5 React Flow
// layer) renders states-as-nodes / transitions-as-edges keyed off STABLE node/edge ids (never label copy).
// Writes results.json. The persist/reload + merge data model are also covered headless (core + editor-shell
// tests); this is the live, on-.exe acceptance.

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

// The canonical "enter `to`" action (the typo-proof Then) + a transition built from its parts — the shape
// the registry-fed builder produces.
const enterAction = (entity, to) => ({
  action: "SetField",
  entity,
  component: "QuestState",
  field: "state",
  value: { Str: to },
});
const trans = (entity, from, to, event, conditions = []) => ({
  id: "", // blank → the shell stamps a stable, peer-namespaced edge id on commit
  from,
  to,
  rule: { name: `${from} -> ${to}`, enabled: true, event, conditions, actions: [enterAction(entity, to)] },
});
const machine = (entity, transitions) => ({
  name: "quest",
  entity,
  component: "QuestState",
  field: "state",
  states: ["Hunting", "ReadyForBoss", "FacingBoss"],
  initial: "Hunting",
  transitions,
});

const results = [];

describe("LIVE state machines (M12.2)", () => {
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

  it("registry-fed transitions, dangling Blocked-explained, reachability, stable ids, undo-removes-a-transition", async () => {
    // A real entity to carry the QuestState.
    const q = await invoke("create_entity", { x: 0, y: 1, z: 0, name: "quest" });
    if (typeof q !== "string") throw new Error(`create_entity returned ${JSON.stringify(q)}`);

    // 1) Author the QuestState machine: Hunting --EnemyDied--> ReadyForBoss --ZoneEntered--> FacingBoss.
    const v1 = machine(q, [
      trans(q, "Hunting", "ReadyForBoss", "EnemyDied", [
        { entity: q, component: "KillCounter", field: "count", op: "ge", value: { Integer: 4 } },
      ]),
      trans(q, "ReadyForBoss", "FacingBoss", "ZoneEntered"),
    ]);
    const authored = await invoke("author_state_machine", { sm: v1, id: null });
    results.push({ step: "author_state_machine", result: authored });
    if (typeof authored.id !== "string") throw new Error(`author failed: ${JSON.stringify(authored)}`);
    if (authored.error) throw new Error(`unexpected error: ${authored.error}`);
    const smId = authored.id;
    // Every state is reachable from Hunting → no unreachable warning.
    if ((authored.unreachable || []).length !== 0)
      throw new Error(`unexpected unreachable: ${JSON.stringify(authored.unreachable)}`);

    // 2) It shows in the list, with the transitions carrying STABLE, server-stamped ids (the e2e/graph key).
    let list = await invoke("state_machines");
    const found = list.find((m) => m.id === smId);
    if (!found || found.machine.states.length !== 3) throw new Error(`state_machines wrong: ${JSON.stringify(list)}`);
    if (found.machine.transitions.length !== 2) throw new Error(`expected 2 transitions, got ${found.machine.transitions.length}`);
    const stampedIds = found.machine.transitions.map((t) => t.id);
    if (stampedIds.some((id) => !id)) throw new Error(`a transition has no stable id: ${JSON.stringify(stampedIds)}`);
    // The current-state read is the M12.5 seam — it defaults to the initial state.
    if (found.current !== "Hunting") throw new Error(`current should default to initial, got ${found.current}`);
    results.push({ step: "list", transitionIds: stampedIds, current: found.current });

    // 3) A DANGLING transition (to a state the machine doesn't declare) is Blocked + explained (ADR-016).
    let bad;
    try {
      bad = await invoke("author_state_machine", {
        sm: machine(q, [trans(q, "Hunting", "Nowhere", "EnemyDied")]),
        id: null,
      });
    } catch (e) {
      bad = { id: null, error: String((e && e.message) || e) };
    }
    results.push({ step: "blocked-dangling", result: bad });
    if (bad.id || !bad.error || !/state/i.test(bad.error))
      throw new Error(`a dangling transition should be Blocked+explained: ${JSON.stringify(bad)}`);

    // 4) A machine with an island state surfaces the reachability WARNING (explained, not rejected).
    const islandSm = {
      ...machine(q, [trans(q, "Hunting", "ReadyForBoss", "EnemyDied")]),
      states: ["Hunting", "ReadyForBoss", "FacingBoss"], // FacingBoss has no incoming transition → unreachable
    };
    const island = await invoke("author_state_machine", { sm: islandSm, id: null });
    results.push({ step: "reachability", result: island });
    if (island.error) throw new Error(`island machine should validate (warn, not reject): ${island.error}`);
    if (!(island.unreachable || []).includes("FacingBoss"))
      throw new Error(`FacingBoss should be reported unreachable: ${JSON.stringify(island.unreachable)}`);

    // 5) Edit the first machine to ADD a transition (one undoable tx), then undo → the transition is removed.
    const withExtra = {
      ...found.machine,
      transitions: [...found.machine.transitions, trans(q, "FacingBoss", "Hunting", "EnemyDied")],
    };
    const edited = await invoke("author_state_machine", { sm: withExtra, id: smId });
    if (edited.error) throw new Error(`edit failed: ${edited.error}`);
    let after = (await invoke("state_machines")).find((m) => m.id === smId);
    if (after.machine.transitions.length !== 3) throw new Error(`edit should yield 3 transitions, got ${after.machine.transitions.length}`);
    await invoke("undo");
    after = (await invoke("state_machines")).find((m) => m.id === smId);
    results.push({ step: "undo", transitions: after.machine.transitions.length });
    if (after.machine.transitions.length !== 2) throw new Error(`undo should remove the transition (back to 2), got ${after.machine.transitions.length}`);

    // 6) The visual state-graph renders in the REAL React DOM, reusing the React Flow layer. Driving the
    //    panel UI ("+ New machine" authors a default 3-state machine) → the graph draws NODES keyed by the
    //    STABLE state names (never label copy).
    const panel = await browser.$("#stategraph");
    if (!(await panel.isExisting())) throw new Error("the state-graph panel is not mounted");
    await (await browser.$('[data-testid="sm-new"]')).click();
    await browser.waitUntil(async () => (await browser.$('[data-testid="state-graph"]')).isExisting(), {
      timeout: 5000,
      timeoutMsg: "the state-graph did not render",
    });
    const nodeIds = await browser.execute(() =>
      [...document.querySelectorAll(".react-flow__node")].map((n) => n.getAttribute("data-id")),
    );
    results.push({ step: "dom-graph", nodeIds });
    if (!nodeIds.includes("Hunting") || !nodeIds.includes("FacingBoss"))
      throw new Error(`the React Flow graph did not render the state nodes (by stable id): ${JSON.stringify(nodeIds)}`);

    // 7) Draw a transition through the PANEL → the graph gains an EDGE → capture the rendered state-graph
    //    for visual acceptance (the panel is regular React/SVG DOM, so a WebDriver screenshot IS the render).
    await (await browser.$('[data-testid="sm-add-transition"]')).click();
    await browser.waitUntil(
      async () => (await browser.execute(() => document.querySelectorAll(".react-flow__edge").length)) >= 1,
      { timeout: 5000, timeoutMsg: "the transition edge did not render in the graph" },
    );
    const edgeIds = await browser.execute(() =>
      [...document.querySelectorAll(".react-flow__edge")].map((e) => e.getAttribute("data-id")),
    );
    results.push({ step: "dom-graph-edges", edgeIds });
    await browser.pause(250); // let React Flow settle the layout before the capture
    await (await browser.$("#stategraph")).saveScreenshot(path.join(OUT_DIR, "state-graph.png"));
    console.log(`WROTE state-graph.png (${edgeIds.length} edge(s)) → ${OUT_DIR}`);

    console.log("PASS: live state-machine authoring + Blocked-dangling + reachability + stable ids + undo + React Flow graph.");
  });

  after(async () => {
    writeFileSync(path.join(OUT_DIR, "results.json"), JSON.stringify(results, null, 2));
    console.log(`\nWROTE ${results.length} steps → ${path.join(OUT_DIR, "results.json")}`);
  });
});
