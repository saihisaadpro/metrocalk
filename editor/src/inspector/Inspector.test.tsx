//! Inspector (M10.10 / C6) — verified headless: a selected entity WITH schema-backed components renders a
//! JSON Forms property form (real, editable properties); an entity with NO editable properties renders a
//! real EMPTY-STATE, never a blank pane beside the header. (Whether the LIVE core populates real
//! properties is the `.exe`-owed half of C6.)
//!
//! **ADR-170 — the panel has THREE absences and they are now one anatomy.** No selection
//! (`inspectorNoSelection` → `InspectorEmpty`), a selection with no editable field (`inspectorNoFields`),
//! and the Relations tab next door. All three are `EmptyPanelState`. The no-fields copy also stopped
//! naming a control that does not exist: `/core` has `RemoveComponent` and no `AddComponent`, so "add a
//! component to this object" was an instruction nothing in this product could carry out.

import { afterEach, expect, test } from "vitest";
import { fireEvent, render, screen, within } from "@testing-library/react";
import { Inspector } from "./Inspector";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  window.localStorage.clear();
});

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
  const empty = screen.getByTestId("inspectorNoFields");
  expect(empty.className).toContain("mtk-empty-panel"); // the shared anatomy, not a bare sentence
  expect(empty.textContent).toMatch(/no editable properties/i);
  // AND IT NO LONGER INSTRUCTS AN IMPOSSIBLE ACTION. There is no add-component control anywhere in this
  // editor, and there is no core op behind one either.
  expect(empty.textContent).not.toMatch(/add a component/i);
});

test("no selection renders the composed empty state — not a sentence, and not the no-fields one", () => {
  projectionStore.getState().bulkLoad([{ id: "e3", name: "Lamp", parentId: null, components: { Transform: { x: 1 } } }]);
  render(<Inspector client={fakeClient()} />);
  const empty = screen.getByTestId("inspectorEmpty");
  expect(empty.className).toContain("mtk-empty-panel");
  expect(screen.getByTestId("inspectorNoSelection")).toBeTruthy();
  expect(screen.queryByTestId("inspectorNoFields")).toBeNull();
  expect(empty.textContent).toMatch(/select an object to edit its properties/i);
  // Nothing is waiting for a binding in this scene, so the guest list renders nothing at all.
  expect(screen.queryByTestId("requirers")).toBeNull();
});

test("with no selection the objects waiting for a binding are offered, and clicking one selects it", () => {
  projectionStore.getState().applyDelta({
    ops: [
      { op: "upsert", id: "e-health", name: "Health Bar", parentId: null, kind: "requirer", rel: { requires: ["Health"], provides: [], bound: 0, needsBinding: true, isGroup: false } },
      { op: "upsert", id: "e-player", name: "Player", parentId: null, kind: "mesh" },
    ],
  });
  render(<Inspector client={fakeClient()} />);
  const row = screen.getByTestId("requirer");
  expect(row.getAttribute("data-id")).toBe("e-health");
  // The SHARED card surface, not a hand-written twin of it.
  expect(row.className).toContain("mtk-card");
  expect(row.className).toContain("cand");
  fireEvent.click(row);
  expect(projectionStore.getState().selectedId).toBe("e-health");
  // …and the panel has left the empty state for the object that was waiting.
  expect(screen.queryByTestId("inspectorNoSelection")).toBeNull();
  expect(screen.getByText("Health Bar")).toBeTruthy();
});

test("component groups use remembered shared disclosures: Transform opens first and other content stays mounted", () => {
  projectionStore.getState().bulkLoad([
    {
      id: "e1",
      name: "Lamp",
      parentId: null,
      components: {
        Transform: { x: 1, y: 2, z: 3 },
        HealthBar: { width: 1 },
      },
    },
  ]);
  projectionStore.getState().select("e1");

  const { unmount } = render(<Inspector client={fakeClient()} />);
  const groups = screen.getAllByTestId("inspectorGroup");
  const transform = groups.find((group) => group.getAttribute("data-group") === "Transform");
  const health = groups.find((group) => group.getAttribute("data-group") === "HealthBar");
  expect(transform).toBeTruthy();
  expect(health).toBeTruthy();

  const transformToggle = within(transform!).getByRole("button", { name: "Transform" });
  const healthToggle = within(health!).getByRole("button", { name: "HealthBar" });
  expect(transform!.className).toContain("mtk-disclosure--card");
  expect(transformToggle.getAttribute("aria-expanded")).toBe("true");
  expect(healthToggle.getAttribute("aria-expanded")).toBe("false");

  const healthRegion = health!.querySelector<HTMLElement>(".mtk-disclosure__region");
  expect(healthRegion).toBeTruthy();
  expect(healthRegion!.getAttribute("role")).toBeNull();
  expect(healthRegion!.getAttribute("aria-hidden")).toBe("true");
  expect(healthRegion!.inert).toBe(true);
  expect(healthRegion!.querySelector("input")).toBeTruthy();

  fireEvent.click(healthToggle);
  expect(healthToggle.getAttribute("aria-expanded")).toBe("true");
  expect(window.localStorage.getItem("metrocalk:disclosure:inspector-component:HealthBar")).toBe("open");

  unmount();
  render(<Inspector client={fakeClient()} />);
  expect(screen.getByRole("button", { name: "HealthBar" }).getAttribute("aria-expanded")).toBe("true");
});
