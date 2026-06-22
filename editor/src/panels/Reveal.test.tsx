//! Reveal (bind-by-intent) — the north-star #1 surface, verified headless in jsdom: a selected entity's
//! ranked compatible targets render, incompatible ones are greyed WITH the reason (every "no" explained),
//! and one click binds. Mirrors the build-acceptance dimensions at the component level (the live composite
//! / 0-IPC / budget run is local).

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Reveal } from "./Reveal";
import { projectionStore } from "../store/projection";
import type { EditorClient } from "../transport/session";
import type { RevealResponse } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

function stubClient(reveal: RevealResponse, bind = vi.fn()): EditorClient {
  return {
    setField: vi.fn(() => "op"),
    bind: (from: string, rel: string, to: string) => {
      bind(from, rel, to);
      return "op";
    },
    onEphemeral: () => () => {},
    revealTargets: () => Promise.resolve(reveal),
    describe: () => Promise.resolve({ created: null, kind: null, source: null, price: null, seam: "generate", balance: null }),
    walletInfo: () => Promise.resolve({ ok: true, balance: 100, cost: null, message: null }),
    topUp: () => Promise.resolve({ ok: true, balance: 200, cost: 100, message: null }),
    aiEdit: () => Promise.resolve({ ok: true, balance: 98, cost: 2, message: null }),
    undo: () => {},
  };
}

const REVEAL: RevealResponse = {
  required: ["Health"],
  compatible: [
    { id: "p1", name: "Player Health", distance: 0, affinity: 90 },
    { id: "p2", name: "Boss Health", distance: 1, affinity: 80 },
  ],
  greyed: [{ id: "x1", name: "Lamp", reason: "doesn't provide Health" }],
  bound: [],
};

test("ranked compatible targets render, every greyed 'no' carries its reason, one click binds", async () => {
  projectionStore.getState().bulkLoad([
    { id: "e1", name: "HealthBar", parentId: null, components: { Socket: { accepts: "Health" } } },
  ]);
  projectionStore.getState().select("e1");
  const bind = vi.fn();
  render(<Reveal client={stubClient(REVEAL, bind)} />);

  // ranked compatible candidates appear (proximity·affinity order preserved from the response)
  const cands = await screen.findAllByTestId("candidate");
  expect(cands).toHaveLength(2);
  expect(cands[0].textContent).toContain("Player Health");
  expect(cands[1].textContent).toContain("Boss Health");

  // EVERY "no" explained: the greyed/incompatible target shows the registry-derived reason
  const greyed = screen.getByTestId("greyed");
  expect(greyed.textContent).toContain("doesn't provide Health");

  // ONE-CLICK bind → client.bind(selected, "tracks", candidate)
  fireEvent.click(cands[0]);
  expect(bind).toHaveBeenCalledTimes(1);
  expect(bind).toHaveBeenCalledWith("e1", "tracks", "p1");
});

test("no selection → a prompt, not a crash", () => {
  render(<Reveal client={stubClient(REVEAL)} />);
  expect(screen.getByText(/select an entity/i)).toBeTruthy();
});
