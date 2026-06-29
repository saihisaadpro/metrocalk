//! M12.4 (ADR-048) — the AI Compose panel, verified headless: a sentence is turned into a REVIEWABLE
//! proposal (the patches are listed for the user to read; nothing is applied yet); Apply sends the exact
//! reviewed composition back through `compose` (the validated pipeline) and toasts an undoable success; and an
//! offline / unrecognized / rejected proposal shows its plain-language reason inline with NO apply control
//! (close the loop · every "no" explained — the AI is a guest, not a raw mutation).

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ComposePanel } from "./ComposePanel";
import { fakeClient } from "../transport/test-client";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import type { Composition } from "../transport/protocol";

const COMPOSITION: Composition = {
  ops: [
    { op: "setField", entity: "1_5", component: "KillCounter", field: "count", value: { Integer: 0 } },
    {
      op: "authorRule",
      id: "r_ai_ignite",
      rule: {
        name: "ignite on kills",
        enabled: true,
        event: "EnemyDied",
        conditions: [{ entity: "1_5", component: "KillCounter", field: "count", op: "ge", value: { Integer: 4 } }],
        actions: [{ action: "SetField", entity: "1_5", component: "Flammable", field: "lit", value: { Bool: true } }],
      },
    },
  ],
};

afterEach(() => {
  projectionStore.getState().reset();
  toastStore.getState().reset();
});

test("Propose surfaces the proposed patches for review — nothing is applied yet", async () => {
  const compose = vi.fn();
  const proposeComposition = vi.fn(() => Promise.resolve({ ok: true, composition: COMPOSITION, ops: 2, error: null }));
  render(<ComposePanel client={fakeClient({ proposeComposition, compose })} />);

  fireEvent.change(screen.getByTestId("compose-sentence"), {
    target: { value: "when an enemy dies and kills reach 4, set it on fire" },
  });
  fireEvent.click(screen.getByTestId("compose-propose"));

  const proposal = await screen.findByTestId("compose-proposal");
  expect(screen.getByTestId("compose-opcount").textContent).toBe("2");
  // The patches are listed in plain language for review.
  expect(proposal.textContent).toMatch(/author rule "ignite on kills"/);
  expect(screen.getAllByTestId("compose-op")).toHaveLength(2);
  // Reviewing is not applying — `compose` hasn't been called.
  expect(compose).not.toHaveBeenCalled();
});

test("Apply sends the exact reviewed composition through compose, and toasts an undoable success", async () => {
  const compose = vi.fn((_c: Composition) => Promise.resolve({ ok: true, applied: 2, rules: 1, stateMachines: 0, error: null }));
  const proposeComposition = vi.fn(() => Promise.resolve({ ok: true, composition: COMPOSITION, ops: 2, error: null }));
  render(<ComposePanel client={fakeClient({ proposeComposition, compose })} />);

  fireEvent.change(screen.getByTestId("compose-sentence"), { target: { value: "ignite on kills" } });
  fireEvent.click(screen.getByTestId("compose-propose"));
  await screen.findByTestId("compose-apply");
  fireEvent.click(screen.getByTestId("compose-apply"));

  await waitFor(() => expect(compose).toHaveBeenCalled());
  // The reviewed composition is passed back verbatim (no re-derivation between review + apply).
  expect(compose.mock.calls[0][0]).toEqual(COMPOSITION);
  await waitFor(() =>
    expect(toastStore.getState().toasts.some((t) => t.kind === "success" && /Ctrl-Z to undo/.test(t.text))).toBe(true),
  );
});

test("an offline / unrecognized proposal shows its explained reason — no apply control", async () => {
  const proposeComposition = vi.fn(() =>
    Promise.resolve({ ok: false, composition: null, ops: 0, error: "select the entity the rule should act on first" }),
  );
  render(<ComposePanel client={fakeClient({ proposeComposition })} />);

  fireEvent.change(screen.getByTestId("compose-sentence"), { target: { value: "make it shiny" } });
  fireEvent.click(screen.getByTestId("compose-propose"));

  const err = await screen.findByTestId("compose-error");
  expect(err.textContent).toMatch(/select the entity/);
  // Rejected-as-UX: there's nothing to apply.
  expect(screen.queryByTestId("compose-apply")).toBeNull();
});

test("the selected entity is passed as the rule's target", async () => {
  const proposeComposition = vi.fn(() => Promise.resolve({ ok: true, composition: COMPOSITION, ops: 2, error: null }));
  projectionStore.getState().select("1_5");
  render(<ComposePanel client={fakeClient({ proposeComposition })} />);

  fireEvent.change(screen.getByTestId("compose-sentence"), { target: { value: "ignite on kills" } });
  fireEvent.click(screen.getByTestId("compose-propose"));

  await waitFor(() => expect(proposeComposition).toHaveBeenCalledWith("ignite on kills", "1_5"));
});
