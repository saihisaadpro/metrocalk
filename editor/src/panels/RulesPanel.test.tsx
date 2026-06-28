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
import type { RuleData, RuleRegistryInfo } from "../transport/protocol";

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
  ],
};

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
  expect([...comp.options].map((o) => o.value)).toEqual(["KillCounter", "Flammable"]);
  // Adding an action surfaces the closed action vocabulary.
  fireEvent.click(screen.getByText("+ action"));
  const action = screen.getByTestId("rule-action") as HTMLSelectElement;
  expect([...action.options].map((o) => o.value)).toEqual(["SetField", "AdjustCounter"]);
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
        { id: "r1", name: "ignite", enabled: true, event: "EnemyDied", conditionCount: 2, actionCount: 1 },
      ]),
    deleteRule,
  });
  render(<RulesPanel client={client} />);

  const row = await screen.findByTestId("rule-row");
  expect(row.textContent).toMatch(/ignite/);
  expect(row.textContent).toMatch(/When EnemyDied/);
  fireEvent.click(screen.getByTitle("remove rule"));
  await waitFor(() => expect(deleteRule).toHaveBeenCalledWith("r1"));
});
