//! The asset shelf (ADR-144) — favourites and recents, which are per-machine preferences and must never
//! be mistaken for scene state (invariant 1). Asserts the two properties a shortcut list lives or dies by:
//! a compound key that cannot collide across the catalog's two id namespaces, and a recent list that
//! promotes rather than duplicates.

import { beforeEach, expect, test } from "vitest";
import { RECENT_LIMIT, assetShelfStore, parseShelfList, recordPlacement, shelfKey, toggleFavourite } from "./assetShelf";

beforeEach(() => {
  window.localStorage.clear();
  assetShelfStore.getState().reset();
});

test("a key is source-qualified, so a marketplace entry can never inherit a stdlib kind's star", () => {
  // `metrocalk_core::catalog` draws ids from two namespaces at once — a stdlib kind name and a
  // marketplace entry id — and nothing forbids a marketplace entry called `HealthBar`.
  expect(shelfKey("local", "HealthBar")).not.toBe(shelfKey("marketplace", "HealthBar"));
  toggleFavourite("local", "HealthBar");
  expect(assetShelfStore.getState().favourites).toEqual(["local:HealthBar"]);
  expect(assetShelfStore.getState().favourites).not.toContain("marketplace:HealthBar");
});

test("starring is a toggle and is written through to storage", () => {
  toggleFavourite("marketplace", "forge:rusty-sword");
  expect(assetShelfStore.getState().favourites).toEqual(["marketplace:forge:rusty-sword"]);
  expect(window.localStorage.getItem("metrocalk:asset-shelf:v1:favourites")).toContain("rusty-sword");
  toggleFavourite("marketplace", "forge:rusty-sword");
  expect(assetShelfStore.getState().favourites).toEqual([]);
});

test("placing the same asset twice promotes it instead of filling the shortcut with one name", () => {
  recordPlacement("local", "Transform");
  recordPlacement("local", "Light");
  recordPlacement("local", "Transform");
  expect(assetShelfStore.getState().recent).toEqual(["local:Transform", "local:Light"]);
});

test("the recent list is bounded, so a long session cannot turn a shortcut into a second catalog", () => {
  for (let i = 0; i < RECENT_LIMIT + 4; i++) recordPlacement("local", `Kind${i}`);
  const recent = assetShelfStore.getState().recent;
  expect(recent).toHaveLength(RECENT_LIMIT);
  expect(recent[0]).toBe(`local:Kind${RECENT_LIMIT + 3}`); // newest first
  expect(recent).not.toContain("local:Kind0"); // oldest dropped
});

test("junk in storage reads as an empty shelf rather than throwing on boot", () => {
  // A locked-down webview, a private window, or a value another version wrote. None of them may be the
  // reason the panel fails to mount. Tested on the parser directly because the store reads at
  // construction — a test that wrote to storage afterwards would be asserting nothing at all.
  expect(parseShelfList(null)).toEqual([]);
  expect(parseShelfList("")).toEqual([]);
  expect(parseShelfList("{not json")).toEqual([]);
  expect(parseShelfList('"a string, not a list"')).toEqual([]);
  expect(parseShelfList("{}")).toEqual([]);
  // A list that is PARTLY junk keeps the entries it can read: one corrupt element must not cost the user
  // every star they ever set.
  expect(parseShelfList('["local:Health", 7, null, "marketplace:forge:rusty-sword"]')).toEqual([
    "local:Health",
    "marketplace:forge:rusty-sword",
  ]);
});
