//! M12.5 (ADR-049) — the live truth-state debugger panel, verified headless (test #5 boxes 3–4):
//! - it is a **Play-time** surface (hidden when Stopped);
//! - click an entity → its rules' truth is **visible** (✅/❌ per condition + the machine's current state) with
//!   the `explain_rule` narration — debug by *looking*, asserted off the STABLE `data-satisfied` flag, never
//!   the overlay copy (`<test_and_ci_discipline>` rule 3);
//! - firing a gameplay event drives the When-channel (`fireRuleEvent`);
//! - the decision-history **scrub** calls `ruleScrub` (time-travel);
//! - a determinism-flagged rule (non-deterministic plugin) is surfaced, never silent.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { RuleDebugPanel } from "./RuleDebugPanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { playStore } from "../store/play";
import type { RuleDebugInfo } from "../transport/protocol";

/** The canonical test-#5 truth-state after 3 kills: ✅ FacingBoss, ❌ KillCounter 3 of 4 (does not burn). */
function threeKills(): RuleDebugInfo {
  return {
    playing: true,
    frame: 3,
    head: 3,
    truth: {
      entity: "1_0",
      rules: [
        {
          rule: "r_ignite",
          name: "rusty sword ignites",
          event: "EnemyDied",
          fires: false,
          conditions: [
            { satisfied: false, entity: "1_0", component: "KillCounter", field: "count", actual: { Integer: 3 }, expected: { Integer: 4 }, display: "KillCounter = 3 of 4" },
            { satisfied: true, entity: "1_0", component: "Zone", field: "current", actual: { Str: "BossArena" }, expected: { Str: "BossArena" }, display: "Zone.current = BossArena (want to be exactly BossArena)" },
          ],
        },
      ],
      machines: [{ machine: "sm_quest", name: "quest", field: "state", current: "FacingBoss", display: "state = FacingBoss" }],
    },
    explanations: [{ rule: "r_ignite", text: "'rusty sword ignites' is blocked: KillCounter.count is 3, but the rule needs to be at least 4 (waiting on EnemyDied)" }],
    decisions: [
      { frame: 0, kind: "counterChanged", entity: "1_0", component: "KillCounter", field: "count", from: { Integer: 0 }, to: { Integer: 1 } },
      { frame: 2, kind: "counterChanged", entity: "1_0", component: "KillCounter", field: "count", from: { Integer: 2 }, to: { Integer: 3 } },
    ],
    flagged: [],
  };
}

afterEach(() => {
  projectionStore.getState().reset();
  playStore.getState().reset();
});

test("the debugger is hidden when not playing (it is a Play-time surface)", () => {
  projectionStore.getState().select("1_0");
  const { container } = render(<RuleDebugPanel client={fakeClient()} />);
  expect(container.querySelector("#ruleDebug")).toBeNull();
});

test("click the sword → the live truth-state is VISIBLE: ✅ FacingBoss, ❌ KillCounter 3 of 4", async () => {
  playStore.getState().refresh({ playing: true, paused: false });
  projectionStore.getState().select("1_0");
  const ruleDebug = vi.fn(() => Promise.resolve(threeKills()));
  render(<RuleDebugPanel client={fakeClient({ ruleDebug })} />);

  // The read was for the clicked entity.
  await waitFor(() => expect(ruleDebug).toHaveBeenCalledWith("1_0"));

  // The machine's current state is shown.
  const machine = await screen.findByTestId("truthMachine-sm_quest");
  expect(machine.textContent).toMatch(/state = FacingBoss/);

  // The blocking condition is made visible — asserted off the STABLE data-satisfied flag, not the copy.
  const counter = screen.getByTestId("truthCond-r_ignite-0");
  expect(counter.getAttribute("data-satisfied")).toBe("false");
  const zone = screen.getByTestId("truthCond-r_ignite-1");
  expect(zone.getAttribute("data-satisfied")).toBe("true");
  // The why is shown, not logged (explain_rule narration present).
  expect(screen.getByTestId("explain-r_ignite").textContent).toMatch(/needs to be at least 4/);
});

test("firing an event drives the When-channel for the selected entity", async () => {
  playStore.getState().refresh({ playing: true, paused: false });
  projectionStore.getState().select("1_0");
  const fireRuleEvent = vi.fn(() => Promise.resolve(threeKills()));
  render(<RuleDebugPanel client={fakeClient({ ruleDebug: () => Promise.resolve(threeKills()), fireRuleEvent })} />);

  fireEvent.click(await screen.findByTestId("fireEnemyDied"));
  await waitFor(() => expect(fireRuleEvent).toHaveBeenCalledWith("EnemyDied", null, "1_0"));
});

test("scrubbing the decision history calls ruleScrub (time-travel)", async () => {
  playStore.getState().refresh({ playing: true, paused: false });
  projectionStore.getState().select("1_0");
  const ruleScrub = vi.fn(() => Promise.resolve(threeKills()));
  render(<RuleDebugPanel client={fakeClient({ ruleDebug: () => Promise.resolve(threeKills()), ruleScrub })} />);

  const slider = await screen.findByTestId("ruleScrub");
  fireEvent.change(slider, { target: { value: "1" } });
  await waitFor(() => expect(ruleScrub).toHaveBeenCalledWith(1, "1_0"));
  // The history is shown as a readable story.
  expect(screen.getAllByTestId("decisionRow").length).toBeGreaterThan(0);
});

test("a non-deterministic-plugin rule is surfaced (flagged out of the lockstep path), never silent", async () => {
  playStore.getState().refresh({ playing: true, paused: false });
  projectionStore.getState().select("1_0");
  const flaggedInfo: RuleDebugInfo = { ...threeKills(), flagged: [{ rule: "r_chaos", reason: "'chaos' isn't a known-deterministic plugin, so this rule is held out of the deterministic Play/replay path" }] };
  render(<RuleDebugPanel client={fakeClient({ ruleDebug: () => Promise.resolve(flaggedInfo) })} />);

  const flagged = await screen.findByTestId("ruleFlagged");
  expect(flagged.textContent).toMatch(/chaos.*deterministic/);
});
