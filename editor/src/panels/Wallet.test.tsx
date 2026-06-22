//! Wallet + AI-edit (M7) — verified headless in jsdom: the mounted balance reads from `wallet_info`,
//! "Top up" grants the sandbox stipend and re-shows the new balance + status, and "Make it rustier"
//! debits-on-success (new balance + the charge in status) BUT — the load-bearing refuse-when-broke
//! case — when `ai_edit` refuses (`!ok`), the surfaced reason lands in the status line and the
//! balance is LEFT UNCHANGED (the charge never happened; every "no" explained, ADR-016).

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { Wallet } from "./Wallet";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { EconResponse } from "../transport/protocol";

afterEach(() => {
  projectionStore.getState().reset();
  uiStore.getState().setStatus(""); // hygiene: status is global; don't let it leak across tests
});

/** A full `EditorClient` stub — only the econ verbs are exercised; the rest are inert vi.fn()s so the
 *  object satisfies the contract surface the Wallet imports. */
function stubClient(over: Partial<EditorClient> = {}): EditorClient {
  return {
    setField: vi.fn(() => "op"),
    bind: vi.fn(() => "op"),
    onEphemeral: vi.fn(() => () => {}),
    revealTargets: vi.fn(() => Promise.resolve({ required: [], compatible: [], greyed: [], bound: [] })),
    describe: vi.fn(() => Promise.resolve({ created: null, kind: null, source: null, price: null, seam: null, balance: null })),
    walletInfo: vi.fn(() => Promise.resolve<EconResponse>({ ok: true, balance: 100, cost: null, message: null })),
    topUp: vi.fn(() => Promise.resolve<EconResponse>({ ok: true, balance: 200, cost: 100, message: null })),
    aiEdit: vi.fn(() => Promise.resolve<EconResponse>({ ok: true, balance: 198, cost: 2, message: null })),
    undo: vi.fn(),
    ...over,
  };
}

function selectAnEntity() {
  projectionStore.getState().bulkLoad([{ id: "e1", name: "Rover", parentId: null, components: {} }]);
  projectionStore.getState().select("e1");
}

test("mount reads wallet_info and shows the balance", async () => {
  render(<Wallet client={stubClient()} />);
  const bal = await screen.findByTestId("balance");
  await waitFor(() => expect(bal.textContent).toBe("100"));
});

test("Top up grants the stipend → new balance shown + status set", async () => {
  render(<Wallet client={stubClient()} />);
  const bal = await screen.findByTestId("balance");
  await waitFor(() => expect(bal.textContent).toBe("100"));

  fireEvent.click(screen.getByTestId("topup"));

  await waitFor(() => expect(bal.textContent).toBe("200"));
  expect(uiStore.getState().status).toBe("topped up · 200 tokens");
});

test("AI-edit button only appears with a selection, and debits-on-success (balance + charge in status)", async () => {
  // No selection → no rustier button.
  const { rerender } = render(<Wallet client={stubClient()} />);
  expect(screen.queryByTestId("rustier")).toBeNull();

  // Select an entity → the button appears.
  selectAnEntity();
  rerender(<Wallet client={stubClient()} />);
  const bal = await screen.findByTestId("balance");
  await waitFor(() => expect(bal.textContent).toBe("100"));

  fireEvent.click(screen.getByTestId("rustier"));

  await waitFor(() => expect(bal.textContent).toBe("198"));
  expect(uiStore.getState().status).toBe("rustier · −2");
});

test("refuse-when-broke is EXPLAINED: !ok surfaces the message and the balance is UNCHANGED", async () => {
  selectAnEntity();
  const refuse = vi.fn(() =>
    Promise.resolve<EconResponse>({ ok: false, balance: 0, cost: null, message: "insufficient balance" }),
  );
  render(<Wallet client={stubClient({ aiEdit: refuse })} />);
  const bal = await screen.findByTestId("balance");
  await waitFor(() => expect(bal.textContent).toBe("100"));

  fireEvent.click(screen.getByTestId("rustier"));

  // The refusal is surfaced verbatim …
  await waitFor(() => expect(uiStore.getState().status).toBe("insufficient balance"));
  // … the AI-edit was attempted on the selected entity …
  expect(refuse).toHaveBeenCalledTimes(1);
  // … and NO charge landed: the displayed balance is left exactly as it was.
  expect(bal.textContent).toBe("100");
});
