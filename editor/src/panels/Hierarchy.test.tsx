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
