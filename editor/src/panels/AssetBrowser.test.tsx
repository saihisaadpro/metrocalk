//! AssetBrowser (M10.2) — verified headless in jsdom: it BROWSES the one catalog grouped by category,
//! SEARCHES it (the tiered resolver — ranked, with the no-match seam), and PLACES an item into the scene
//! via add_item (place-into-scene). Asserts real behaviour: the right items render per category, search
//! filters to the resolver's result, and clicking an item calls add_item with the right (id, source).

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { AssetBrowser } from "./AssetBrowser";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import { fakeClient } from "../transport/test-client";
import type { CatalogItem } from "../transport/protocol";

afterEach(() => {
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
});

const item = (id: string, label: string, category: string, source: string, price?: number): CatalogItem => ({
  id,
  label,
  bucket: category,
  category,
  source,
  provides: [],
  requires: [],
  price,
});

const GROUPS = {
  Health: [item("HealthBar", "HealthBar", "Health", "local")],
  Characters: [item("acme:Knight", "Knight", "Characters (acme)", "marketplace", 12)],
};

test("browses the one catalog grouped by category, and places an item via add_item (place-into-scene)", async () => {
  const addItem = vi.fn(() => Promise.resolve({ created: "e42", balance: null, seam: null }));
  render(
    <AssetBrowser
      client={fakeClient({ catalog: () => Promise.resolve(GROUPS), addItem })}
    />,
  );

  // grouped by category (reuses the ONE catalog — categories from ADR-019)
  const cats = await screen.findAllByTestId("asset-category");
  expect(cats).toHaveLength(2);
  const items = await screen.findAllByTestId("asset-item");
  expect(items.map((i) => i.getAttribute("data-id"))).toEqual(["HealthBar", "acme:Knight"]);

  // place-into-scene: click → add_item(id, source); the marketplace item carries its source
  fireEvent.click(items[1]);
  expect(addItem).toHaveBeenCalledWith("acme:Knight", "marketplace");
  await vi.waitFor(() => expect(uiStore.getState().status).toContain("Knight"));
});

test("search reuses the resolver (ranked items), and a no-match surfaces the generate seam", async () => {
  const catalogSearch = vi.fn((q: string) =>
    Promise.resolve(
      q === "zzz"
        ? { items: [], seam: "generate" }
        : { items: [item("HealthBar", "HealthBar", "Health", "local")] },
    ),
  );
  render(<AssetBrowser client={fakeClient({ catalog: () => Promise.resolve(GROUPS), catalogSearch })} />);

  // a matching search → ranked results replace the browse view (the one resolver, not a fork)
  fireEvent.change(await screen.findByTestId("asset-search"), { target: { value: "health" } });
  await vi.waitFor(() => expect(screen.getByTestId("asset-results")).toBeTruthy());
  expect(catalogSearch).toHaveBeenCalledWith("health");
  const r = await screen.findAllByTestId("asset-item");
  expect(r).toHaveLength(1);
  expect(r[0].getAttribute("data-id")).toBe("HealthBar");

  // a no-match → the explained generate seam (the honest fall-through)
  fireEvent.change(screen.getByTestId("asset-search"), { target: { value: "zzz" } });
  await vi.waitFor(() => expect(screen.getByTestId("asset-seam").textContent).toContain("generate"));
});
