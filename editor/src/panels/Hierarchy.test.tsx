//! Hierarchy (M14.2 / ADR-058) — verified headless: rows surface the live relational truth keyed off the
//! REAL `/core` projection (the C6 closure) as STRUCTURED signals (`data-needs-binding`, `data-kind`), each
//! row carries a thumbnail (the icon fallback in jsdom), and clicking a row selects it (cross-panel
//! coherence). Asserts behaviour, not styled copy.

import { afterEach, expect, test } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { Hierarchy } from "./Hierarchy";
import { projectionStore } from "../store/projection";
import { thumbnailStore } from "../store/thumbnails";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  thumbnailStore.getState().reset();
});

test("rows surface live relational truth (C6): a requirer is data-needs-binding=1, a renderable is 0", () => {
  projectionStore.getState().bulkLoad([
    { id: "hb", name: "Health Bar", parentId: null, components: { HealthBar: { width: 1 } } },
    { id: "lamp", name: "Lamp", parentId: null, components: { MeshRenderer: { mesh: "lamp" } } },
  ]);
  render(<Hierarchy client={fakeClient()} />);

  const byId = Object.fromEntries(screen.getAllByTestId("hrow").map((r) => [r.getAttribute("data-id"), r]));
  expect(byId["hb"].getAttribute("data-needs-binding")).toBe("1");
  expect(byId["hb"].getAttribute("data-kind")).toBe("requirer");
  expect(byId["lamp"].getAttribute("data-needs-binding")).toBe("0");
  expect(byId["lamp"].getAttribute("data-kind")).toBe("mesh");
  // every row carries a thumbnail slot (fallback icon in jsdom — keyed off the structured status)
  expect(byId["hb"].querySelector('[data-testid="thumb"]')).toBeTruthy();
});

test("clicking a row selects it (cross-panel coherence: the engine selection follows)", () => {
  projectionStore.getState().bulkLoad([{ id: "e1", name: "One", parentId: null, components: { MeshRenderer: { mesh: "x" } } }]);
  render(<Hierarchy client={fakeClient()} />);
  expect(projectionStore.getState().selectedId).toBeNull();
  fireEvent.click(screen.getByTestId("hrow"));
  expect(projectionStore.getState().selectedId).toBe("e1");
});

test("exposes a searchable semantic tree with result and no-result states", () => {
  projectionStore.getState().bulkLoad([
    { id: "valve-01", name: "Intake Valve", parentId: null, components: { MeshRenderer: { mesh: "valve" } } },
    { id: "lamp-01", name: "Work Lamp", parentId: null, components: { Light: { intensity: 1 } } },
  ]);
  render(<Hierarchy client={fakeClient()} />);

  const tree = screen.getByRole("tree", { name: "Scene objects" });
  expect(tree.getAttribute("aria-multiselectable")).toBe("true");
  expect(screen.getAllByRole("treeitem")).toHaveLength(2);
  expect(screen.getAllByRole("treeitem")[0].getAttribute("aria-level")).toBe("1");

  fireEvent.change(screen.getByRole("searchbox", { name: "Search scene objects" }), { target: { value: "lamp" } });
  expect(screen.getAllByRole("treeitem")).toHaveLength(1);
  expect(screen.getByRole("treeitem").textContent).toContain("Work Lamp");
  expect(document.getElementById("count")?.textContent).toContain("1 of 2");

  fireEvent.change(screen.getByRole("searchbox", { name: "Search scene objects" }), { target: { value: "missing" } });
  expect(screen.queryByRole("tree")).toBeNull();
  expect(screen.getByText("No matching objects")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Clear search" }));
  expect(screen.getAllByRole("treeitem")).toHaveLength(2);
});

test("empty hierarchy explains how to add the first object", () => {
  render(<Hierarchy client={fakeClient()} />);
  expect(screen.getByText("No objects in this scene")).toBeTruthy();
  expect(screen.getByText(/add an entity above/i)).toBeTruthy();
});
