//! HoverTooltip (M3.3) — verified headless in jsdom: hovering an entity surfaces its name + capability
//! contract READ-ONLY (never selects, never mutates), and an absent target leaves no DOM behind. The two
//! cases below pin the load-bearing behavior: a real `entityDetails` lookup whose name + required cap reach
//! the screen, and the null-id collapse (queryByTestId === null) that keeps a non-hovered surface inert.

import { afterEach, expect, test, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { HoverTooltip } from "./HoverTooltip";
import { projectionStore } from "../store/projection";
import type { EntityDetails } from "../transport/protocol";
import { fakeClient } from "../transport/test-client";

afterEach(() => projectionStore.getState().reset());

const DETAILS: EntityDetails = {
  id: "e1",
  name: "HealthBar",
  components: ["Transform", "Socket"],
  provides: [],
  requires: ["Health"],
  boundTo: [],
};

test("hovering an entity surfaces its name + required cap, and LEAVES an existing selection alone (read-only)", async () => {
  // Seed a real selection + a mutation spy, so "read-only" is a TESTED invariant, not a trivial null.
  projectionStore.getState().bulkLoad([{ id: "e9", name: "Other", parentId: null, components: {} }]);
  projectionStore.getState().select("e9");
  const removeEntity = vi.fn();
  render(<HoverTooltip client={fakeClient({ entityDetails: () => Promise.resolve(DETAILS), removeEntity })} id="e1" />);

  const tip = await screen.findByTestId("tooltip");
  expect(tip.textContent).toContain("HealthBar"); // the name is the headline signal
  expect(tip.textContent).toContain("Health"); // the required cap is the stable assert
  expect(screen.queryByTestId("tooltip-provides")).toBeNull(); // empty sections omitted
  expect(screen.queryByTestId("tooltip-boundto")).toBeNull();

  // Hover is inert: it neither changes the existing selection nor fires any mutation.
  expect(projectionStore.getState().selectedId).toBe("e9");
  expect(removeEntity).not.toHaveBeenCalled();
});

test("no target (id=null) → nothing renders AND entityDetails is never fetched (early-return contract)", () => {
  const entityDetails = vi.fn(() => Promise.resolve(DETAILS));
  render(<HoverTooltip client={fakeClient({ entityDetails })} id={null} />);
  expect(screen.queryByTestId("tooltip")).toBeNull();
  expect(entityDetails).not.toHaveBeenCalled(); // a null hover must not even hit the core
});
