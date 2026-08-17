//! M12.1 (ADR-045) — the Rules panel, verified headless: the builder's dropdowns are fed by the registry
//! (typo-proof — only real events/components/fields/actions are offerable); building + Create submits a
//! structured `RuleData` (never free text); a registry-Blocked rule shows its explained reason inline; the
//! engine's offered mirror "cleanup" rule is surfaced with an explicit accept control (offered, never
//! forced); and the Rule list renders + deletes.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { RulesPanel } from "./RulesPanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import type { RuleCondition, RuleData, RuleRegistryInfo, RuleSummary } from "../transport/protocol";

const REGISTRY: RuleRegistryInfo = {
  events: [
    { name: "EnemyDied", description: "an enemy was defeated" },
    { name: "StateEntered", description: "entered a state" },
  ],
  actions: [
    { name: "SetField", description: "set a component field" },
    { name: "AdjustCounter", description: "add to a counter" },
  ],
  components: [
    { name: "KillCounter", fields: [{ name: "count", ty: "integer" }] },
    { name: "Flammable", fields: [{ name: "lit", ty: "boolean" }] },
    { name: "QuestState", fields: [{ name: "state", ty: "string" }] },
  ],
};

const clause = (component: string, field: string): RuleCondition => ({
  entity: "1_1",
  component,
  field,
  op: "ge",
  value: { Integer: 1 },
});
/** A Rule-list row. It carries a REAL rule, not a set of counts — which is the point: the row's "1 if ·
 *  any of 2" is now derived from the rule it describes, so a fixture cannot assert a count the rule
 *  contradicts. */
const listed = (id: string, rule: Partial<RuleData> & { name: string }): RuleSummary => ({
  id,
  rule: {
    enabled: true,
    event: "EnemyDied",
    conditions: [clause("KillCounter", "count")],
    actions: [{ action: "SetField", entity: "1_1", component: "Flammable", field: "lit", value: { Bool: true } }],
    ...rule,
  },
});

afterEach(() => {
  projectionStore.getState().reset();
  toastStore.getState().reset();
});

async function openBuilder(client = fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY) })) {
  render(<RulesPanel client={client} />);
  // The "+ New rule" button enables once the registry loads.
  await waitFor(() => expect((screen.getByTestId("rule-new") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByTestId("rule-new"));
  await screen.findByTestId("rule-builder");
}

test("the builder's dropdowns are fed by the registry (typo-proof — no free text)", async () => {
  await openBuilder();
  // The When dropdown offers exactly the registry events.
  const event = screen.getByTestId("rule-event") as HTMLSelectElement;
  expect([...event.options].map((o) => o.value)).toEqual(["EnemyDied", "StateEntered"]);
  // Adding a condition surfaces registry component + field pickers.
  fireEvent.click(screen.getByText("+ condition"));
  const comp = screen.getByTestId("rule-component") as HTMLSelectElement;
  expect([...comp.options].map((o) => o.value)).toEqual(["KillCounter", "Flammable", "QuestState"]);
  // Adding an action surfaces the closed action vocabulary.
  fireEvent.click(screen.getByText("+ action"));
  const action = screen.getByTestId("rule-action") as HTMLSelectElement;
  expect([...action.options].map((o) => o.value)).toEqual(["SetField", "AdjustCounter"]);
});

test("repeated builder controls expose unambiguous row-context names", async () => {
  await openBuilder();

  expect(screen.getByRole("textbox", { name: "Rule name" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Rule trigger event" })).toBeTruthy();

  fireEvent.click(screen.getByText("+ condition"));
  fireEvent.click(screen.getByText("+ condition"));
  fireEvent.click(screen.getByText("+ action"));

  expect(screen.getByRole("combobox", { name: "Condition 1 entity" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Condition 1 component" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Condition 1 field" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Condition 1 comparison operator" })).toBeTruthy();
  expect(screen.getByRole("spinbutton", { name: "Condition 1 value" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Condition 2 entity" })).toBeTruthy();
  expect(screen.getByRole("spinbutton", { name: "Condition 2 value" })).toBeTruthy();

  expect(screen.getByRole("combobox", { name: "Action 1 type" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Action 1 entity" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Action 1 component" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Action 1 field" })).toBeTruthy();
  expect(screen.getByRole("spinbutton", { name: "Action 1 value" })).toBeTruthy();

  expect(screen.getByRole("button", { name: "Remove condition 1" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove condition 2" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Remove action 1" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: "×" })).toBeNull();
});

test("Create submits a structured RuleData assembled from registry clicks", async () => {
  const authorRule = vi.fn((_rule: RuleData) => Promise.resolve({ id: "r1", error: null, mirror: null }));
  await openBuilder(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorRule }));

  fireEvent.change(screen.getByTestId("rule-name"), { target: { value: "ignite" } });
  fireEvent.click(screen.getByText("+ condition"));
  fireEvent.click(screen.getByText("+ action"));
  fireEvent.click(screen.getByTestId("rule-create"));

  await waitFor(() => expect(authorRule).toHaveBeenCalled());
  const rule = authorRule.mock.calls[0][0];
  expect(rule.name).toBe("ignite");
  expect(rule.event).toBe("EnemyDied"); // from the registry, not typed
  expect(rule.conditions).toHaveLength(1);
  expect(rule.conditions[0].component).toBe("KillCounter");
  expect(rule.conditions[0].field).toBe("count");
  expect(rule.actions).toHaveLength(1);
  expect(rule.actions[0].action).toBe("SetField");
});

test("an alternative is authored into the OR group — never as one more AND condition", async () => {
  const authorRule = vi.fn((_rule: RuleData) => Promise.resolve({ id: "r1", error: null, mirror: null }));
  await openBuilder(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorRule }));

  fireEvent.click(screen.getByText("+ condition"));
  fireEvent.click(screen.getByText("+ alternative"));
  fireEvent.click(screen.getByText("+ alternative"));
  // Retarget the second alternative, so the two lists cannot be told apart by luck: a wrong wiring that
  // appended alternatives to `conditions` would put a Flammable clause in the AND list.
  fireEvent.change(screen.getByRole("combobox", { name: "Alternative 2 component" }), {
    target: { value: "Flammable" },
  });

  fireEvent.click(screen.getByTestId("rule-create"));
  await waitFor(() => expect(authorRule).toHaveBeenCalled());
  const rule = authorRule.mock.calls[0][0];

  // The AND list is untouched; the OR group carries both alternatives — snake_case, because
  // `metrocalk_core::rules::RuleData` is plain serde (this key IS the wire key).
  expect(rule.conditions).toHaveLength(1);
  expect(rule.conditions[0].component).toBe("KillCounter");
  expect(rule.any_of).toHaveLength(2);
  expect(rule.any_of?.[0].component).toBe("KillCounter");
  // The alternative stayed typo-proof: picking a component moved its field AND the value's kind with it.
  expect(rule.any_of?.[1].component).toBe("Flammable");
  expect(rule.any_of?.[1].field).toBe("lit");
  expect(rule.any_of?.[1].value).toEqual({ Bool: false });
});

test("a rule with no alternatives still sends an empty OR group, and shows no OR affordance", async () => {
  const authorRule = vi.fn((_rule: RuleData) => Promise.resolve({ id: "r1", error: null, mirror: null }));
  await openBuilder(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorRule }));

  // The group only appears once it has a member — an empty "either" would be a claim about nothing.
  expect(screen.queryByTestId("rule-anyof")).toBeNull();
  fireEvent.click(screen.getByTestId("rule-create"));
  await waitFor(() => expect(authorRule).toHaveBeenCalled());
  expect(authorRule.mock.calls[0][0].any_of).toEqual([]);
});

test("conditions and alternatives are independent — removing one never renumbers the other", async () => {
  await openBuilder();
  fireEvent.click(screen.getByText("+ condition"));
  fireEvent.click(screen.getByText("+ alternative"));

  // Each repeated control says which row it belongs to (the same rule the condition rows are held to).
  expect(screen.getByRole("combobox", { name: "Condition 1 entity" })).toBeTruthy();
  expect(screen.getByRole("combobox", { name: "Alternative 1 comparison operator" })).toBeTruthy();
  expect(screen.getByTestId("rule-anyof").textContent).toMatch(/…and either/);
  expect(screen.getByTestId("rule-anyof").textContent).toMatch(/any one of these is enough/);

  fireEvent.click(screen.getByRole("button", { name: "Remove alternative 1" }));
  expect(screen.queryByTestId("rule-anyof")).toBeNull();
  expect(screen.getByRole("combobox", { name: "Condition 1 entity" })).toBeTruthy();
  // Still no unnamed control anywhere in the builder.
  expect(screen.queryByRole("button", { name: "×" })).toBeNull();
});

test("the Rule list states the OR group as its own claim, never folded into the If count", async () => {
  const client = fakeClient({
    ruleRegistry: () => Promise.resolve(REGISTRY),
    listRules: () =>
      Promise.resolve([
        listed("r1", { name: "ignite", any_of: [clause("Flammable", "lit"), clause("QuestState", "state")] }),
        listed("r2", { name: "plain" }),
      ]),
  });
  render(<RulesPanel client={client} />);

  const rows = await screen.findAllByTestId("rule-row");
  // "2 if" about a rule whose second claim is "any ONE of these" would be a false statement about when it
  // fires — so the alternatives are counted, and counted separately. Both figures are read off the rule.
  expect(rows[0].textContent).toMatch(/1 if · any of 2 · 1 then/);
  expect(rows[1].textContent).toMatch(/1 if · 1 then/);
  expect(rows[1].textContent).not.toMatch(/any of/);
});

test("a boolean or string field offers only = and ≠ — the orderings that can never discriminate are gone", async () => {
  await openBuilder();
  fireEvent.click(screen.getByText("+ condition"));

  const op = () => screen.getByRole("combobox", { name: "Condition 1 comparison operator" }) as HTMLSelectElement;
  // An integer field keeps the full vocabulary — this narrows what is meaningless, not what is useful.
  expect([...op().options].map((o) => o.value)).toEqual(["eq", "ne", "lt", "le", "gt", "ge"]);

  // `lit ≥ false` is true of every value a boolean can hold: a clause that can never fail, one click away
  // in the panel whose claim is that clicking cannot produce nonsense.
  fireEvent.change(screen.getByRole("combobox", { name: "Condition 1 component" }), { target: { value: "Flammable" } });
  expect([...op().options].map((o) => o.value)).toEqual(["eq", "ne"]);

  // A string field is a categorical name; ordering one would compare it alphabetically.
  fireEvent.change(screen.getByRole("combobox", { name: "Condition 1 component" }), { target: { value: "QuestState" } });
  expect([...op().options].map((o) => o.value)).toEqual(["eq", "ne"]);
});

test("retargeting a clause coerces its operator — a select never shows a value it has no option for", async () => {
  const authorRule = vi.fn((_rule: RuleData) => Promise.resolve({ id: "r1", error: null, mirror: null }));
  await openBuilder(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorRule }));
  fireEvent.click(screen.getByText("+ condition"));

  const op = () => screen.getByRole("combobox", { name: "Condition 1 comparison operator" }) as HTMLSelectElement;
  fireEvent.change(op(), { target: { value: "gt" } });
  expect(op().value).toBe("gt");

  // Re-point the clause at a boolean. `gt` is no longer offerable, so it must be replaced rather than
  // left selected: a <select> whose value matches no <option> reads as though nothing is chosen, and
  // would have submitted `lit > false` from a control showing "=".
  fireEvent.change(screen.getByRole("combobox", { name: "Condition 1 component" }), { target: { value: "Flammable" } });
  expect(op().value).toBe("eq");

  fireEvent.click(screen.getByTestId("rule-create"));
  await waitFor(() => expect(authorRule).toHaveBeenCalled());
  expect(authorRule.mock.calls[0][0].conditions[0].op).toBe("eq");
});

test("a rule can be turned off — and off means the runtime will not fire it, said in the row", async () => {
  const authorRule = vi.fn((_rule: RuleData, _id?: string | null) =>
    Promise.resolve({ id: "r1", error: null, mirror: null }),
  );
  let rules = [listed("r1", { name: "ignite" })];
  const client = fakeClient({
    ruleRegistry: () => Promise.resolve(REGISTRY),
    listRules: () => Promise.resolve(rules),
    authorRule,
  });
  render(<RulesPanel client={client} />);

  const row = await screen.findByTestId("rule-row");
  expect(row.getAttribute("data-enabled")).toBe("true");
  expect(row.textContent).not.toMatch(/does not run/);

  // The toggle REPLACES the rule by id through the existing author path — not a second write path, and
  // it carries the whole rule so nothing about it is lost on the way through.
  rules = [listed("r1", { name: "ignite", enabled: false })];
  fireEvent.click(screen.getByRole("button", { name: "Turn off rule ignite" }));
  await waitFor(() => expect(authorRule).toHaveBeenCalled());
  expect(authorRule.mock.calls[0][0]).toMatchObject({ name: "ignite", enabled: false, event: "EnemyDied" });
  expect(authorRule.mock.calls[0][0].conditions).toHaveLength(1);
  expect(authorRule.mock.calls[0][1]).toBe("r1");

  // The row now says what off MEANS, and offers the way back.
  await waitFor(() => expect(screen.getByTestId("rule-row").getAttribute("data-enabled")).toBe("false"));
  expect(screen.getByTestId("rule-row").textContent).toMatch(/off — does not run/);
  expect(screen.getByRole("button", { name: "Turn on rule ignite" })).toBeTruthy();
  // Undoable, and the toast says so rather than leaving the user to guess.
  expect(toastStore.getState().toasts.some((t) => /will not run · Ctrl-Z to undo/.test(t.text))).toBe(true);
});

test("a refused toggle says so and leaves the row alone — no optimistic lie", async () => {
  const authorRule = vi.fn(() => Promise.resolve({ id: null, error: "the rule's entity is gone", mirror: null }));
  const client = fakeClient({
    ruleRegistry: () => Promise.resolve(REGISTRY),
    listRules: () => Promise.resolve([listed("r1", { name: "ignite" })]),
    authorRule,
  });
  render(<RulesPanel client={client} />);

  await screen.findByTestId("rule-row");
  fireEvent.click(screen.getByRole("button", { name: "Turn off rule ignite" }));
  await waitFor(() => expect(authorRule).toHaveBeenCalled());

  await waitFor(() =>
    expect(toastStore.getState().toasts.some((t) => t.kind === "error" && /entity is gone/.test(t.text))).toBe(true),
  );
  // The list still reflects the core, which refused: the row is still on.
  expect(screen.getByTestId("rule-row").getAttribute("data-enabled")).toBe("true");
  expect(screen.getByTestId("rule-row").textContent).not.toMatch(/does not run/);
});

test("a registry-Blocked rule shows its explained reason inline (ADR-016), no toast-of-success", async () => {
  const authorRule = vi.fn(() =>
    Promise.resolve({ id: null, error: "“Frob” isn't an event the engine knows", mirror: null }),
  );
  await openBuilder(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorRule }));
  fireEvent.click(screen.getByTestId("rule-create"));
  const err = await screen.findByTestId("rule-error");
  expect(err.textContent).toMatch(/isn't an event the engine knows/);
  expect(toastStore.getState().toasts.some((t) => t.kind === "success")).toBe(false);
});

test("the offered mirror cleanup rule is surfaced with an explicit accept control (offered, never forced)", async () => {
  const mirror = {
    name: "flame on (cleanup)",
    enabled: true,
    event: "StateExited",
    conditions: [],
    actions: [{ action: "SetField", entity: "1_1", component: "Flammable", field: "lit", value: { Bool: false } }],
  };
  const authorRule = vi.fn((_rule: RuleData) => Promise.resolve({ id: "r1", error: null, mirror }));
  await openBuilder(fakeClient({ ruleRegistry: () => Promise.resolve(REGISTRY), authorRule }));

  fireEvent.click(screen.getByTestId("rule-create"));
  const offer = await screen.findByTestId("mirror-offer");
  expect(offer.textContent).toMatch(/flame on \(cleanup\)/);

  // Accepting authors the mirror as its own rule.
  fireEvent.click(screen.getByTestId("mirror-accept"));
  await waitFor(() => expect(authorRule).toHaveBeenCalledTimes(2));
  expect(authorRule.mock.calls[1][0].event).toBe("StateExited");
});

test("the Rule list renders authored rules and deletes one", async () => {
  const deleteRule = vi.fn(() => Promise.resolve(true));
  const client = fakeClient({
    ruleRegistry: () => Promise.resolve(REGISTRY),
    listRules: () =>
      Promise.resolve([
        listed("r1", { name: "ignite", conditions: [clause("KillCounter", "count"), clause("Flammable", "lit")] }),
      ]),
    deleteRule,
  });
  render(<RulesPanel client={client} />);

  const row = await screen.findByTestId("rule-row");
  expect(row.textContent).toMatch(/ignite/);
  expect(row.textContent).toMatch(/When EnemyDied/);
  expect(row.textContent).toMatch(/2 if/);
  expect(screen.queryByRole("button", { name: "×" })).toBeNull();
  fireEvent.click(screen.getByRole("button", { name: "Remove rule ignite" }));
  await waitFor(() => expect(deleteRule).toHaveBeenCalledWith("r1"));
});
