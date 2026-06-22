//! DescribeBar (describe-to-create) — north-star #2, verified headless in jsdom: free text → the
//! core resolves it across the tiers, and the status the user reads is DERIVED from the structured
//! result (created+kind for a local hit; the explained generate seam when nothing matched), with a
//! local hit also SELECTING the new entity so its attach targets reveal. Asserts real behavior — the
//! client called with the typed query, the status store set with the stable message, the projection
//! store's selectedId — not "it rendered".

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { DescribeBar } from "./DescribeBar";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import { walletStore } from "../store/wallet";
import { toastStore } from "../store/toasts";
import type { EditorClient } from "../transport/session";
import { GENERATE_COST } from "../transport/protocol";
import type { DescribeResponse, GenerateResponse } from "../transport/protocol";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
  walletStore.getState().reset();
  toastStore.getState().reset();
});

/** Stub the contract surface; `describe` resolves to the canned tiered result, and we capture the
 *  query it was called with so the test can assert the real call (not just a render). */
function stubClient(result: DescribeResponse, describe = vi.fn()): EditorClient {
  return fakeClient({
    describe: (query: string) => {
      describe(query);
      return Promise.resolve(result);
    },
  });
}

const LOCAL_HIT: DescribeResponse = {
  created: "e9",
  kind: "HealthBar",
  source: "local",
  price: null,
  seam: null,
  balance: 100,
};

const NO_MATCH: DescribeResponse = {
  created: null,
  kind: null,
  source: null,
  price: null,
  seam: "generate",
  balance: 100,
};

test("a local create → status names the created kind AND the new entity is selected", async () => {
  const describe = vi.fn();
  render(<DescribeBar client={stubClient(LOCAL_HIT, describe)} />);

  // type the query + click Create (the stable scaffold ids the page-object keys on)
  fireEvent.change(screen.getByTestId("describe"), { target: { value: "health bar" } });
  fireEvent.click(screen.getByTestId("describeBtn"));

  // the client was called with the typed query (real describe-to-create, not a render)
  await waitFor(() => expect(describe).toHaveBeenCalledWith("health bar"));

  // the status surfaces the structured result — the created KIND (not a generic "created")
  await waitFor(() => {
    const status = uiStore.getState().status;
    expect(status).toContain("HealthBar");
    expect(status).toContain("e9");
  });

  // a hit FOCUSES the new entity so its compatible bind targets reveal
  expect(projectionStore.getState().selectedId).toBe("e9");
});

test("a no-match → status surfaces the explained generate seam, and nothing is selected", async () => {
  render(<DescribeBar client={stubClient(NO_MATCH)} />);

  fireEvent.change(screen.getByTestId("describe"), { target: { value: "frobnicator" } });
  fireEvent.click(screen.getByTestId("describeBtn"));

  // the generate seam is EXPLAINED (every dead-end carries the next step) — not a silent no-op
  await waitFor(() => {
    const status = uiStore.getState().status;
    expect(status).toMatch(/no local or marketplace match/i);
    expect(status).toMatch(/generate/i);
  });

  // no `created` → no selection (no phantom focus)
  expect(projectionStore.getState().selectedId).toBeNull();
});

test("Enter submits too (parity with the click path)", async () => {
  const describe = vi.fn();
  render(<DescribeBar client={stubClient(LOCAL_HIT, describe)} />);

  const input = screen.getByTestId("describe");
  fireEvent.change(input, { target: { value: "health bar" } });
  fireEvent.keyDown(input, { key: "Enter" });

  await waitFor(() => expect(describe).toHaveBeenCalledWith("health bar"));
  await waitFor(() => expect(projectionStore.getState().selectedId).toBe("e9"));
});

test("an empty query is a no-op — no describe call, no status churn", () => {
  const describe = vi.fn();
  render(<DescribeBar client={stubClient(LOCAL_HIT, describe)} />);

  // whitespace-only → trimmed to empty → guarded
  fireEvent.change(screen.getByTestId("describe"), { target: { value: "   " } });
  fireEvent.click(screen.getByTestId("describeBtn"));

  expect(describe).not.toHaveBeenCalled();
  expect(uiStore.getState().status).toBe("");
});

// ── C1: the headline fix — no-match dead-ends in an EXPLICIT, actionable Generate button, not a footer ──
test("no-match → an explicit Generate button (#genBtn) appears, and clicking it generates → places + selects", async () => {
  const generate = vi.fn(
    (): Promise<GenerateResponse> => Promise.resolve({ created: "gen-9", cost: 10, available: true, seam: null, balance: 90 }),
  );
  const client = fakeClient({ describe: () => Promise.resolve(NO_MATCH), generate });
  render(<DescribeBar client={client} />);

  fireEvent.change(screen.getByTestId("describe"), { target: { value: "a flying dragon" } });
  fireEvent.click(screen.getByTestId("describeBtn"));

  // an EXPLICIT, actionable Generate button appears IN THE BAR (not a passive footer line) — the C1 fix
  const gen = await screen.findByTestId("genBtn");
  expect(gen.textContent).toMatch(/generate with ai/i);
  expect(gen.textContent).toContain(`~${GENERATE_COST} tokens`); // cost is legible UP-FRONT (C3)

  // clicking it generates with the typed query → places + SELECTS the result (the loop closes)
  fireEvent.click(gen);
  await waitFor(() => expect(generate).toHaveBeenCalledWith("a flying dragon"));
  await waitFor(() => expect(projectionStore.getState().selectedId).toBe("gen-9"));
  // the spend is loud: the balance updated + a toast confirms what was bought (C7/C11)
  expect(walletStore.getState().balance).toBe(90);
  expect(toastStore.getState().toasts.some((t) => /generated/i.test(t.text))).toBe(true);
});

test("the generate offer carries NO 'last resort' in-joke jargon (C1 copy)", async () => {
  render(<DescribeBar client={fakeClient({ describe: () => Promise.resolve(NO_MATCH) })} />);
  fireEvent.change(screen.getByTestId("describe"), { target: { value: "zzz" } });
  fireEvent.click(screen.getByTestId("describeBtn"));
  const panel = await screen.findByTestId("describePanel");
  expect(panel.textContent).not.toMatch(/last resort/i);
  // status carries the offer (contains "Generate"), but the actionable control is the button, not the footer
  expect(uiStore.getState().status).toMatch(/generate/i);
});

test("Create is DISABLED while the field is empty (no enabled-inert CTA — C5)", () => {
  const describe = vi.fn();
  render(<DescribeBar client={stubClient(NO_MATCH, describe)} />);
  const btn = screen.getByTestId("describeBtn") as HTMLButtonElement;
  expect(btn.disabled).toBe(true);
  fireEvent.click(btn);
  expect(describe).not.toHaveBeenCalled();
  // typing enables it
  fireEvent.change(screen.getByTestId("describe"), { target: { value: "x" } });
  expect((screen.getByTestId("describeBtn") as HTMLButtonElement).disabled).toBe(false);
});
