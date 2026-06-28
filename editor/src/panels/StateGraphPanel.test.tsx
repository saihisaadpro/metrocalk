//! M12.2 (ADR-046) — the state-graph panel, verified headless: the transition builder is **registry-fed +
//! states-fed** (typo-proof — only real events + the machine's own states are offerable); drawing a
//! transition submits a structured `StateMachine` whose transition **is an M12.1 Rule** (the auto "enter
//! `to`" set-state action — never hand-typed); a **Blocked** machine shows its explained reason inline
//! (ADR-016); **unreachable** states surface as an explained warning; the machine list renders + deletes;
//! and the visual state-graph (the reused React Flow layer) renders.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { StateGraphPanel } from "./StateGraphPanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import type { RuleRegistryInfo, StateMachine, StateMachineInfo } from "../transport/protocol";

const REGISTRY: RuleRegistryInfo = {
  events: [
    { name: "EnemyDied", description: "an enemy was defeated" },
    { name: "ZoneEntered", description: "entered a zone" },
  ],
  actions: [{ name: "SetField", description: "set a component field" }],
  components: [
    { name: "QuestState", fields: [{ name: "state", ty: "string" }] },
    { name: "KillCounter", fields: [{ name: "count", ty: "integer" }] },
  ],
};

afterEach(() => {
  projectionStore.getState().reset();
  toastStore.getState().reset();
});

async function openNew(client = fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY) })) {
  render(<StateGraphPanel client={client} />);
  await waitFor(() => expect((screen.getByTestId("sm-new") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByTestId("sm-new"));
  await screen.findByTestId("sm-name"); // the builder is up (draft set)
}

test("a new machine starts with states (the graph nodes) and the visual graph renders", async () => {
  await openNew();
  // The default QuestState machine's states become the graph nodes (+ the state-name editors).
  expect(screen.getByTestId("state-graph")).toBeTruthy();
  const stateRows = screen.getAllByTestId("sm-state");
  expect(stateRows.length).toBe(3); // Hunting / ReadyForBoss / FacingBoss
});

test("the transition builder is registry-fed + states-fed (typo-proof — no free text)", async () => {
  await openNew();
  fireEvent.click(screen.getByTestId("sm-add-transition"));
  // When dropdown = exactly the registry events.
  const event = (await screen.findByTestId("sm-trans-event")) as HTMLSelectElement;
  expect([...event.options].map((o) => o.value)).toEqual(["EnemyDied", "ZoneEntered"]);
  // from/to dropdowns = exactly the machine's own states (never a free-typed state name).
  const from = screen.getByTestId("sm-trans-from") as HTMLSelectElement;
  expect([...from.options].map((o) => o.value)).toEqual(["Hunting", "ReadyForBoss", "FacingBoss"]);
});

test("drawing a transition submits a structured machine whose transition IS an M12.1 Rule", async () => {
  const authorStateMachine = vi.fn((_sm: StateMachine) =>
    Promise.resolve({ id: "sm-1", error: null, unreachable: [] }),
  );
  await openNew(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorStateMachine }));

  fireEvent.click(screen.getByTestId("sm-add-transition"));
  // Find the commit that carried the new transition (the initial new-machine save has zero transitions).
  await waitFor(() => expect(authorStateMachine.mock.calls.some((c) => c[0].transitions.length === 1)).toBe(true));
  const sm = authorStateMachine.mock.calls.find((c) => c[0].transitions.length === 1)![0];
  const t = sm.transitions[0];
  expect(t.from).toBe("Hunting");
  expect(t.to).toBe("ReadyForBoss");
  expect(t.rule.event).toBe("EnemyDied"); // from the registry, not typed
  // The transition IS a Rule, and its Then is the AUTO "enter `to`" set-state action (typo-proof).
  expect(t.rule.actions).toHaveLength(1);
  expect(t.rule.actions[0].action).toBe("SetField");
  expect(t.rule.actions[0].component).toBe("QuestState");
  expect(t.rule.actions[0].field).toBe("state");
  expect(t.rule.actions[0].value).toEqual({ Str: "ReadyForBoss" });
});

test("a Blocked machine shows its explained reason inline (ADR-016)", async () => {
  const authorStateMachine = vi.fn(() =>
    Promise.resolve({ id: null, error: "a transition points to 'Nowhere', which isn't one of this machine's states", unreachable: [] }),
  );
  await openNew(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorStateMachine }));
  fireEvent.click(screen.getByTestId("sm-add-transition"));
  const err = await screen.findByTestId("sm-error");
  expect(err.textContent).toMatch(/isn't one of this machine's states/);
});

test("unreachable states surface as an explained warning (not a rejection)", async () => {
  const authorStateMachine = vi.fn(() =>
    Promise.resolve({ id: "sm-1", error: null, unreachable: ["FacingBoss"] }),
  );
  await openNew(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorStateMachine }));
  const warn = await screen.findByTestId("sm-unreachable");
  expect(warn.textContent).toMatch(/Unreachable/);
  expect(warn.textContent).toMatch(/FacingBoss/);
});

test("the machine list renders an authored machine and deletes the open one", async () => {
  const machine: StateMachine = {
    name: "quest",
    entity: "1_0",
    component: "QuestState",
    field: "state",
    states: ["Hunting", "ReadyForBoss"],
    initial: "Hunting",
    transitions: [],
  };
  const info: StateMachineInfo = { id: "sm-7", current: "Hunting", machine };
  const deleteStateMachine = vi.fn(() => Promise.resolve(true));
  const client = fakeClient({
    ruleRegistry: () => Promise.resolve(REGISTRY),
    stateMachines: () => Promise.resolve([info]),
    deleteStateMachine,
  });
  render(<StateGraphPanel client={client} />);

  const row = await screen.findByTestId("sm-row");
  expect(row.textContent).toMatch(/quest/);
  fireEvent.click(row); // load it → the delete control appears
  fireEvent.click(await screen.findByTestId("sm-delete"));
  await waitFor(() => expect(deleteStateMachine).toHaveBeenCalledWith("sm-7"));
});
