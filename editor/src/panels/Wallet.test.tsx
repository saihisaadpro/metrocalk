//! Wallet (M10.10 / C4·C7) — verified headless in jsdom: the balance reads from `wallet_info` into the
//! centralized wallet store and renders, and "Top up" is an HONEST sandbox **dev grant** — the new
//! balance shows AND the change is loud (an honest "+100 dev tokens" status, never "topped up" implying a
//! purchase, never a silent mutation). The AI-edit "rustier" action has MOVED out of the wallet (C4) to
//! `AiEditPanel` — so it must NOT appear here (the collision-with-the-balance fix).

import { afterEach, expect, test } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { Wallet } from "./Wallet";
import { walletStore } from "../store/wallet";
import { uiStore } from "../store/ui";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  walletStore.getState().reset();
  uiStore.getState().setStatus("");
  toastStore.getState().reset();
});

test("mount reads wallet_info into the store and shows the balance", async () => {
  render(<Wallet client={fakeClient()} />);
  const bal = await screen.findByTestId("balance");
  await waitFor(() => expect(bal.textContent).toBe("100"));
});

test("Top up is an HONEST dev grant: new balance shown + an honest (non-purchase) status, no silent mutation", async () => {
  render(<Wallet client={fakeClient()} />);
  const bal = await screen.findByTestId("balance");
  await waitFor(() => expect(bal.textContent).toBe("100"));

  fireEvent.click(screen.getByTestId("topup"));

  // the balance updates …
  await waitFor(() => expect(bal.textContent).toBe("200"));
  // … and the change is VISIBLE + HONEST: "dev tokens", not "topped up" (a purchase implication) — C7
  expect(uiStore.getState().status).toContain("dev tokens");
  expect(uiStore.getState().status).not.toMatch(/purchase|bought/i);
  expect(toastStore.getState().toasts.some((t) => /dev tokens/.test(t.text))).toBe(true);
});

test("the button is labelled as a dev grant, not 'Top up +100' (C7 honesty)", async () => {
  render(<Wallet client={fakeClient()} />);
  expect(screen.getByTestId("topup").textContent).toContain("dev tokens");
});

test("the AI-edit 'rustier' action is NOT in the wallet (moved off it — C4 collision fix)", () => {
  render(<Wallet client={fakeClient()} />);
  expect(screen.queryByTestId("rustier")).toBeNull();
});
