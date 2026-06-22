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
import type { EditorClient } from "../transport/session";
import type { DescribeResponse } from "../transport/protocol";

afterEach(() => {
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
});

/** Stub the contract surface; `describe` resolves to the canned tiered result, and we capture the
 *  query it was called with so the test can assert the real call (not just a render). */
function stubClient(result: DescribeResponse, describe = vi.fn()): EditorClient {
  return {
    setField: vi.fn(() => "op"),
    bind: vi.fn(() => "op"),
    onEphemeral: () => () => {},
    revealTargets: () => Promise.resolve({ required: [], compatible: [], greyed: [], bound: [] }),
    describe: (query: string) => {
      describe(query);
      return Promise.resolve(result);
    },
    walletInfo: () => Promise.resolve({ ok: true, balance: 100, cost: null, message: null }),
    topUp: () => Promise.resolve({ ok: true, balance: 200, cost: 100, message: null }),
    aiEdit: () => Promise.resolve({ ok: true, balance: 98, cost: 2, message: null }),
  };
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
