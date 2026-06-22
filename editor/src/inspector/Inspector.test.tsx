//! Inspector (M10.10 / C6) — verified headless: a selected entity WITH schema-backed components renders a
//! JSON Forms property form (real, editable properties); an entity with NO editable properties renders a
//! real EMPTY-STATE ("No editable properties yet — add a component"), never a blank pane beside the header.
//! (Whether the LIVE core populates real properties is the `.exe`-owed half of C6.)

import { afterEach, expect, test } from "vitest";
import { render, screen } from "@testing-library/react";
import { Inspector } from "./Inspector";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";

afterEach(() => projectionStore.getState().reset());

test("an entity with editable components renders editable property INPUTS (data-driven, real properties)", () => {
  projectionStore.getState().bulkLoad([
    // the real /core vocabulary: Transform numbers + a HealthBar marker field (no curated schema → inferred)
    { id: "e1", name: "Lamp", parentId: null, components: { Transform: { x: 1, y: 2, z: 3 }, HealthBar: { width: 1 } } },
  ]);
  projectionStore.getState().select("e1");
  const { container } = render(<Inspector client={fakeClient()} />);
  expect(screen.getByText("Lamp")).toBeTruthy(); // the header
  expect(screen.queryByTestId("inspectorEmpty")).toBeNull(); // a form, not the empty-state
  // the data-driven schema produces EDITABLE inputs for the real fields (x/y/z/width) — the C6 fix
  expect(container.querySelectorAll("input").length).toBeGreaterThan(0);
  // the component NAME is visible (the Group label) — the prompt-40 north-star-1 keys on "Transform"
  expect(container.textContent).toContain("Transform");
});

test("an entity with NO editable properties shows a real empty-state, not a blank pane (C6)", () => {
  projectionStore.getState().bulkLoad([{ id: "e2", name: "Marker", parentId: null, components: {} }]);
  projectionStore.getState().select("e2");
  render(<Inspector client={fakeClient()} />);
  expect(screen.getByText("Marker")).toBeTruthy(); // still names the entity
  expect(screen.getByTestId("inspectorEmpty").textContent).toMatch(/no editable properties yet/i);
});
