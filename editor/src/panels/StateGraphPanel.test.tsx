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
  await screen.findAllByTestId("sm-state"); // the builder is up (draft set)
}

/** Open a collapsed section by its heading, the way a user reaches it. */
function openSection(name: RegExp) {
  fireEvent.click(screen.getByRole("button", { name }));
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

test("machine, state, transition, and condition controls expose row-context names", async () => {
  await openNew();

  // Where the state is STORED is set once and then read from the panel subtitle, so its fields sit in a
  // collapsed section. Collapsed means `hidden` + `inert`, so the controls are genuinely out of reach
  // until it is opened — which is what this asserts by opening it the way a user would.
  expect(screen.queryByRole("textbox", { name: "State machine name" })).toBeNull();
  openSection(/Where the state is stored/);
  expect(screen.getByRole("textbox", { name: "State machine name" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "State machine target entity" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "State machine target component" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "State machine state field" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "State 1 name" })).toBeTruthy();
  expect(screen.getByRole("radio", { name: "Set Hunting as initial state" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Delete state Hunting" })).toBeTruthy();

  fireEvent.click(screen.getByTestId("sm-add-transition"));
  await screen.findByRole("combobox", { name: "Transition 1 trigger event" });
  expect(screen.getByRole("combobox", { name: "Transition 1 source state" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Transition 1 destination state" })).toBeTruthy();
  expect(
    screen.getByRole("button", { name: "Delete transition 1 from Hunting to ReadyForBoss" }),
  ).toBeTruthy();

  fireEvent.click(screen.getByTestId("sm-add-cond"));
  fireEvent.click(screen.getByTestId("sm-add-cond"));
  expect(screen.getByRole("combobox", { name: "Transition 1 condition 1 entity" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Transition 1 condition 1 component" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Transition 1 condition 1 field" })).toBeTruthy();
  expect(
    screen.getByRole("combobox", { name: "Transition 1 condition 1 comparison operator" }),
  ).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Transition 1 condition 1 value" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Transition 1 condition 2 entity" })).toBeTruthy();
  expect(screen.getByRole("textbox", { name: "Transition 1 condition 2 value" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove transition 1 condition 1" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove transition 1 condition 2" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "×" })).toBeNull();
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
  // The state the engine named, and the start state it cannot be reached from — the two facts. NOT the
  // sentence around them, which is copy and drifts (`<test_and_ci_discipline>` 3).
  expect(warn.textContent).toMatch(/FacingBoss/);
  expect(warn.textContent).toMatch(/Hunting/);
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

  // The machine list is the shared `NavRail` every multi-document workspace uses: one tab per machine,
  // one stop in the tab order, arrow keys within it.
  const row = await screen.findByRole("tab", { name: /quest/ });
  expect(screen.getByTestId("sm-list").getAttribute("role")).toBe("tablist");
  fireEvent.click(row); // load it → the delete control appears
  fireEvent.click(await screen.findByTestId("sm-delete"));
  await waitFor(() => expect(deleteStateMachine).toHaveBeenCalledWith("sm-7"));
});

test("a rename the machine cannot accept SAYS SO, and commits nothing", async () => {
  const authorStateMachine = vi.fn(() => Promise.resolve({ id: "sm-1", error: null, unreachable: [] }));
  await openNew(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorStateMachine }));
  const commitsBefore = authorStateMachine.mock.calls.length;

  // "ReadyForBoss" is already this machine's second state. Before this the field simply snapped back on
  // blur with no message anywhere — a silent refusal, `<ux_quality>` 6.
  const first = screen.getByRole("textbox", { name: "State 1 name" });
  fireEvent.change(first, { target: { value: "ReadyForBoss" } });
  fireEvent.blur(first);

  const refusal = await screen.findByTestId("sm-rename-refusal");
  expect(refusal.textContent).toMatch(/ReadyForBoss/);
  expect(authorStateMachine.mock.calls.length).toBe(commitsBefore);
  // And the state is still called what it was called.
  expect((screen.getByRole("textbox", { name: "State 1 name" }) as HTMLInputElement).value).toBe("Hunting");
});

test("the last state cannot be deleted, and the control says why", async () => {
  const machine: StateMachine = {
    name: "door",
    entity: "1_0",
    component: "QuestState",
    field: "state",
    states: ["Closed"],
    initial: "Closed",
    transitions: [],
  };
  const info: StateMachineInfo = { id: "sm-9", current: machine.initial, machine };
  render(
    <StateGraphPanel
      client={fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), stateMachines: () => Promise.resolve([info]) })}
    />,
  );
  fireEvent.click(await screen.findByRole("tab", { name: /door/ }));

  // It used to be an enabled button whose handler returned early — an inert control, which photographs
  // and reads exactly like one that works.
  const remove = await screen.findByRole("button", { name: "Delete state Closed" });
  expect((remove as HTMLButtonElement).disabled).toBe(true);
  expect(remove.getAttribute("title")).toBe("A machine needs at least one state.");
});

test("a guard offers only the operators that can MEAN something about its field", async () => {
  await openNew();
  fireEvent.click(screen.getByTestId("sm-add-transition"));
  fireEvent.click(await screen.findByTestId("sm-add-cond"));

  // The clause opens on `QuestState.state`, a string. Ordering two names compares them alphabetically,
  // which is not a thing an author means — so the Rules builder had already removed `< ≤ > ≥` there and
  // this editor, its own copy of the same builder, had not. One component now, so one answer.
  const op = (await screen.findByRole("combobox", { name: "Transition 1 condition 1 comparison operator" })) as HTMLSelectElement;
  expect([...op.options].map((o) => o.value)).toEqual(["eq", "ne"]);
  expect(op.getAttribute("title")).toMatch(/alphabetically/);

  // A numeric field gets all six back.
  const component = screen.getByRole("combobox", { name: "Transition 1 condition 1 component" });
  fireEvent.change(component, { target: { value: "KillCounter" } });
  const numeric = (await screen.findByRole("combobox", { name: "Transition 1 condition 1 comparison operator" })) as HTMLSelectElement;
  expect([...numeric.options].map((o) => o.value)).toEqual(["eq", "ne", "lt", "le", "gt", "ge"]);
});

test("an empty scene offers a CONTROL, not a sentence about one", async () => {
  render(<StateGraphPanel client={fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY) })} />);
  const empty = await screen.findByTestId("sm-empty");
  expect(empty.textContent).toMatch(/No state machines yet/);
  // The decisive step is a button inside the empty state, not a line of prose ending in a question mark
  // somewhere else on the panel (`<ux_quality>` 1).
  expect(empty.contains(screen.getByTestId("sm-new"))).toBe(true);
});
