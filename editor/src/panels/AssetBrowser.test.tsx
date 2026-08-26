//! AssetBrowser (M10.2, rebuilt for constitution gate 6 in ADR-144) — verified headless in jsdom.
//!
//! WHAT THESE ASSERT, AND WHY EACH ONE IS HERE RATHER THAN A SCREENSHOT. Four of the eight are about
//! defects the previous suite could not have caught because the previous suite asserted the same
//! behaviour the defect was: a whole card that placed on one click was the CONTRACT, so a marketplace
//! buy debiting on that click passed. The claims are written against the *decision* a user makes —
//! nothing is spent unless a confirm was answered, a group is filed under a word a person would say,
//! a no-match offers a control and not a question — and never against the prose that states it
//! (`<test_and_ci_discipline>` 3: the old suite gated on the substring "generate" and would have gone
//! red on a capitalisation).

import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { act, render, screen, fireEvent, within } from "@testing-library/react";
import { AssetBrowser } from "./AssetBrowser";
import { assetShelfStore } from "../store/assetShelf";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { uiStore } from "../store/ui";
import { walletStore } from "../store/wallet";
import { fakeClient } from "../transport/test-client";
import type { CatalogItem } from "../transport/protocol";

beforeEach(() => {
  window.localStorage.clear();
  assetShelfStore.getState().reset();
  walletStore.getState().setBalance(100);
});

afterEach(() => {
  projectionStore.getState().reset();
  toastStore.getState().reset();
  walletStore.getState().reset();
  uiStore.getState().setStatus("");
});

const local = (id: string, bucket: string, provides: string[] = [], requires: string[] = []): CatalogItem => ({
  id,
  label: id,
  bucket,
  category: bucket.replace(/^std:/, ""),
  source: "local",
  provides,
  requires,
});

const market = (id: string, label: string, bucket: string, category: string, price: number): CatalogItem => ({
  id,
  label,
  bucket,
  category,
  source: "marketplace",
  provides: ["Renderable"],
  requires: ["Spatial"],
  price,
});

/** The shape `metrocalk_core::catalog::grouped` actually sends: keys are CANONICAL BUCKETS. */
const GROUPS = {
  "std:UI": [local("HealthBar", "std:UI", ["UIElement"], ["Health"])],
  "std:Characters": [market("acme:knight", "Knight", "std:Characters", "Companions (acme)", 12)],
};

const placed = (created: string, balance: number | null = null) =>
  vi.fn(() => Promise.resolve({ created, balance, seam: null }));

test("files each group under a word a person would say, not the canonical bucket key", async () => {
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS) })} />);

  const groups = await screen.findAllByTestId("asset-category");
  expect(groups).toHaveLength(2);
  // The shipped panel printed the map key. `std:` is the engine's namespace and never a user's word.
  const headings = groups.map((g) => within(g).getByRole("heading").textContent ?? "");
  expect(headings.some((h) => h.includes("UI"))).toBe(true);
  expect(headings.some((h) => h.includes("Characters"))).toBe(true);
  expect(headings.some((h) => h.includes("std:"))).toBe(false);

  const items = await screen.findAllByTestId("asset-item");
  expect(items.map((i) => i.getAttribute("data-id"))).toEqual(["HealthBar", "acme:knight"]);
});

test("a free local asset places on one click, and the placed entity becomes the selection", async () => {
  const addItem = placed("e42");
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS), addItem })} />);

  const items = await screen.findAllByTestId("asset-item");
  fireEvent.click(items[0]);
  expect(addItem).toHaveBeenCalledWith("HealthBar", "local");
  // place + SELECT (C11): the placed entity is what the inspector is now looking at.
  await vi.waitFor(() => expect(projectionStore.getState().selectedId).toBe("e42"));
  // …and it is remembered, so the shortcut collection is real rather than decorative.
  expect(assetShelfStore.getState().recent).toEqual(["local:HealthBar"]);
});

test("a PRICED asset spends nothing on the click — the confirm names the cost and the balance first", async () => {
  const addItem = placed("e7", 88);
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS), addItem })} />);

  const items = await screen.findAllByTestId("asset-item");
  fireEvent.click(items[1]);
  // THE DEFECT THIS REPLACES: the click WAS the buy. `<ux_quality>` 3 — never debit on one unconfirmed
  // click. Nothing may have been sent to the shell yet.
  expect(addItem).not.toHaveBeenCalled();

  const guard = await screen.findByTestId("asset-buy-guard");
  expect(guard.textContent).toContain("12");
  expect(guard.textContent).toContain("100"); // the balance, so the price is legible against something

  fireEvent.click(screen.getByTestId("asset-buy-confirm"));
  expect(addItem).toHaveBeenCalledWith("acme:knight", "marketplace");
  // The wallet mirrors the AUTHORITATIVE balance the response carried, never a local subtraction.
  await vi.waitFor(() => expect(walletStore.getState().balance).toBe(88));
  // The spend is visible where the gesture happened, and states what it actually cost.
  await vi.waitFor(() => expect(toastStore.getState().toasts.map((t) => t.text).join(" ")).toContain("−12 tokens"));
});

test("cancelling the confirm spends nothing and leaves the catalog exactly as it was", async () => {
  const addItem = placed("e7", 88);
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS), addItem })} />);

  fireEvent.click((await screen.findAllByTestId("asset-item"))[1]);
  fireEvent.click(await screen.findByTestId("asset-buy-cancel"));
  expect(addItem).not.toHaveBeenCalled();
  expect(walletStore.getState().balance).toBe(100);
  expect(screen.queryByTestId("asset-buy-guard")).toBeNull();
});

test("a no-match offers a CONTROL that generates, not a sentence with a question mark in it", async () => {
  const describe = vi.fn(() =>
    Promise.resolve({ created: "gen-1", kind: null, source: "generated", price: 6, seam: null, balance: 94 }),
  );
  const catalogSearch = vi.fn(() => Promise.resolve({ items: [], seam: "generate" }));
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS), catalogSearch, describe })} />);

  fireEvent.change(await screen.findByTestId("asset-search"), { target: { value: "zzz" } });
  // The claim is that the decisive step is REACHABLE from here — a button, not a rhetorical status line
  // (`<ux_quality>` 1). Asserting the button's copy would gate on prose; asserting it runs the tier does not.
  const generate = await screen.findByTestId("asset-generate");
  fireEvent.click(generate);
  expect(describe).toHaveBeenCalledWith("zzz");
  await vi.waitFor(() => expect(projectionStore.getState().selectedId).toBe("gen-1"));
  await vi.waitFor(() => expect(walletStore.getState().balance).toBe(94));
});

test("a slow reply for an OLD query cannot overwrite the results of the current one", async () => {
  const pendingSlow: Array<() => void> = [];
  const catalogSearch = vi.fn((q: string) => {
    if (q === "he") {
      // The stale one: resolved only after the newer query has already landed.
      return new Promise<{ items: CatalogItem[]; seam?: string }>((resolve) => {
        pendingSlow.push(() => resolve({ items: [local("Stale", "std:UI")] }));
      });
    }
    return Promise.resolve({ items: [local("HealthBar", "std:UI")] });
  });
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS), catalogSearch })} />);

  const box = await screen.findByTestId("asset-search");
  fireEvent.change(box, { target: { value: "he" } });
  fireEvent.change(box, { target: { value: "health" } });
  await vi.waitFor(() => expect(screen.getAllByTestId("asset-item")).toHaveLength(1));
  // `act` IS THE ASSERTION HERE. Releasing the stale promise and then reading the DOM proves nothing:
  // `findAllBy` resolves against the element that is already there, so the stale `setResults` had not
  // been flushed yet and this test passed with the sequence guard deleted — a vacuous pass, caught by
  // mutating the guard away and watching it stay green. `act` runs the continuation and React's commit
  // before the read, which is what makes the next line able to fail.
  await act(async () => {
    pendingSlow.forEach((release) => release());
  });
  // Without the sequence guard the grid would now be showing the results for "he".
  const items = screen.getAllByTestId("asset-item");
  expect(items.map((i) => i.getAttribute("data-id"))).toEqual(["HealthBar"]);
});

test("a tier filter narrows the library to the tier it names, and says so when nothing is left", async () => {
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS) })} />);
  await screen.findAllByTestId("asset-item");

  const marketplaceFilter = screen
    .getAllByTestId("asset-filter")
    .find((c) => c.getAttribute("data-filter") === "marketplace")!;
  fireEvent.click(marketplaceFilter);
  expect(marketplaceFilter.getAttribute("aria-pressed")).toBe("true");
  const items = await screen.findAllByTestId("asset-item");
  expect(items.map((i) => i.getAttribute("data-source"))).toEqual(["marketplace"]);

  // A filter that empties the panel must explain itself and offer the way back — an empty grid under a
  // heading is indistinguishable from an empty catalog.
  fireEvent.click(screen.getByTestId("asset-favourite")); // star the marketplace item…
  fireEvent.click(marketplaceFilter); // …then drop the tier filter and keep only favourites
  const favouritesFilter = screen
    .getAllByTestId("asset-filter")
    .find((c) => c.getAttribute("data-filter") === "favourites")!;
  fireEvent.click(favouritesFilter);
  expect(screen.getAllByTestId("asset-item")).toHaveLength(1);
  fireEvent.click(screen.getByTestId("asset-favourite")); // un-star it — now nothing passes
  fireEvent.click(await screen.findByTestId("asset-clear-filters"));
  expect((await screen.findAllByTestId("asset-item")).length).toBe(2);
});

test("starring survives a remount, because a favourite is a preference and not scene state", async () => {
  const view = render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS) })} />);
  await screen.findAllByTestId("asset-item");
  const stars = screen.getAllByTestId("asset-favourite");
  expect(stars[0].getAttribute("aria-pressed")).toBe("false");
  fireEvent.click(stars[0]);
  expect(assetShelfStore.getState().favourites).toEqual(["local:HealthBar"]);

  view.unmount();
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS) })} />);
  await screen.findAllByTestId("asset-item");
  expect(screen.getAllByTestId("asset-favourite")[0].getAttribute("aria-pressed")).toBe("true");
});
