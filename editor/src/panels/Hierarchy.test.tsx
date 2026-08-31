//! Hierarchy (M14.2 / ADR-058) — verified headless: rows surface the live relational truth keyed off the
//! REAL `/core` projection (the C6 closure) as STRUCTURED signals (`data-needs-binding`, `data-kind`), each
//! row carries a thumbnail (the icon fallback in jsdom), and clicking a row selects it (cross-panel
//! coherence). Asserts behaviour, not styled copy.

import { afterEach, expect, test, vi } from "vitest";
import { act, render, screen, fireEvent } from "@testing-library/react";
import { Hierarchy } from "./Hierarchy";
import { projectionStore } from "../store/projection";
import { thumbnailStore } from "../store/thumbnails";
import { requestObjectSearch } from "../store/find";
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

test("the WHOLE multi-selection reaches the engine, not just the row last clicked", () => {
  // This used to send `gizmoSelect(id)` — ONE id — after building a multi-selection in the store, so
  // ctrl-clicking three rows highlighted three rows in the list and outlined exactly one object in the
  // 3D view. The list and the stage were two selections that never compared notes, and the stage's
  // answer was the one the user was looking at.
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  projectionStore.getState().bulkLoad([
    { id: "a", name: "A", parentId: null, components: {} },
    { id: "b", name: "B", parentId: null, components: {} },
    { id: "c", name: "C", parentId: null, components: {} },
  ]);
  render(<Hierarchy client={fakeClient({ selectEntities })} />);
  const rows = Object.fromEntries(
    screen.getAllByTestId("hrow").map((r) => [r.getAttribute("data-id"), r]),
  );

  fireEvent.click(rows.a!);
  expect(selectEntities).toHaveBeenLastCalledWith(["a"]);
  fireEvent.click(rows.c!, { ctrlKey: true });
  expect(selectEntities).toHaveBeenLastCalledWith(["a", "c"]);
  fireEvent.click(rows.b!, { shiftKey: true });
  // Shift is a RANGE over the visible order, anchored on the PRIMARY — which the ctrl-click above
  // moved to `c`. So it takes `c`..`b`, and `a` is dropped: a range replaces, it does not accumulate.
  expect(selectEntities).toHaveBeenLastCalledWith(["b", "c"]);
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

test("right-clicking a SELECTED row keeps the selection; right-clicking elsewhere replaces it", () => {
  // THE DEFECT THIS PINS (ADR-183): this handler called `select(id)` unconditionally, so ctrl-clicking
  // three rows and then right-clicking one of them threw the other two away before the menu had
  // opened — over a set the user had just spent three gestures building. Every direct-manipulation
  // surface a person has used draws the same line: a member of the selection acts on the selection, a
  // non-member replaces it.
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  const onContextMenu = vi.fn();
  projectionStore.getState().bulkLoad([
    { id: "a", name: "A", parentId: null, components: {} },
    { id: "b", name: "B", parentId: null, components: {} },
    { id: "c", name: "C", parentId: null, components: {} },
  ]);
  render(<Hierarchy client={fakeClient({ selectEntities })} onContextMenu={onContextMenu} />);
  const rows = Object.fromEntries(screen.getAllByTestId("hrow").map((r) => [r.getAttribute("data-id"), r]));

  fireEvent.click(rows.a!);
  fireEvent.click(rows.b!, { ctrlKey: true });
  expect(projectionStore.getState().multiSelect).toEqual(["a", "b"]);

  // (a) a MEMBER: the set survives, and the menu is told the whole set — not the row under the cursor.
  fireEvent.contextMenu(rows.a!, { clientX: 10, clientY: 20 });
  expect(projectionStore.getState().multiSelect).toEqual(["a", "b"]);
  expect(onContextMenu).toHaveBeenLastCalledWith(["a", "b"], 10, 20);
  expect(selectEntities).toHaveBeenLastCalledWith(["a", "b"]);

  // (b) a NON-member: pointing somewhere else IS a statement about where you are pointing.
  fireEvent.contextMenu(rows.c!, { clientX: 30, clientY: 40 });
  expect(projectionStore.getState().multiSelect).toEqual(["c"]);
  expect(onContextMenu).toHaveBeenLastCalledWith(["c"], 30, 40);
  expect(selectEntities).toHaveBeenLastCalledWith(["c"]);
});

// ---------------------------------------------------------------------------------------------
// ADR-185 — the list could NAME a set and could not select it, and its search could ask only one
// question. What follows pins the three defects and the capability that replaced them.
// ---------------------------------------------------------------------------------------------

/** Four objects, two of them lights, interleaved so a filtered range has hidden rows INSIDE it. */
function interleavedScene() {
  projectionStore.getState().bulkLoad([
    { id: "key", name: "Key Light", parentId: null, components: { Light: { intensity: 1 } } },
    { id: "bolt-a", name: "Bolt A", parentId: null, components: { MeshRenderer: { mesh: "bolt" } } },
    { id: "bolt-b", name: "Bolt B", parentId: null, components: { MeshRenderer: { mesh: "bolt" } } },
    { id: "fill", name: "Fill Light", parentId: null, components: { Light: { intensity: 1 } } },
  ]);
}

function search(): HTMLElement {
  return screen.getByRole("searchbox", { name: "Search scene objects" });
}

test("the search asks what an object IS, not only what it is called", () => {
  interleavedScene();
  render(<Hierarchy client={fakeClient()} />);

  fireEvent.change(search(), { target: { value: "kind:light" } });
  expect(screen.getAllByTestId("hrow").map((r) => r.getAttribute("data-id"))).toEqual(["key", "fill"]);
  expect(document.getElementById("count")?.textContent).toContain("2 of 4");

  // A name and a kind NARROW each other, which is what makes the second word worth typing.
  fireEvent.change(search(), { target: { value: "fill kind:light" } });
  expect(screen.getAllByTestId("hrow").map((r) => r.getAttribute("data-id"))).toEqual(["fill"]);

  // `has:` reaches the component map — the vocabulary `kind` summarises but does not exhaust.
  fireEvent.change(search(), { target: { value: "has:meshrenderer" } });
  expect(screen.getAllByTestId("hrow").map((r) => r.getAttribute("data-id"))).toEqual(["bolt-a", "bolt-b"]);
});

test("the chips are this scene's own kinds, and each one is a toggle", () => {
  interleavedScene();
  render(<Hierarchy client={fakeClient()} />);

  const chips = screen.getByTestId("scene-facets");
  const tokens = Array.from(chips.querySelectorAll("[data-facet]")).map((b) => b.getAttribute("data-facet"));
  expect(tokens).toEqual(["kind:light", "kind:mesh"]);

  const lights = chips.querySelector("[data-facet='kind:light']") as HTMLButtonElement;
  expect(lights.getAttribute("aria-pressed")).toBe("false");
  fireEvent.click(lights);
  expect((search() as HTMLInputElement).value).toBe("kind:light");
  expect(screen.getAllByTestId("hrow")).toHaveLength(2);

  // Pressed, and the way back out — a chip that could only ever add would strand the user in a filter.
  const pressed = screen.getByTestId("scene-facets").querySelector("[data-facet='kind:light']") as HTMLButtonElement;
  expect(pressed.getAttribute("aria-pressed")).toBe("true");
  fireEvent.click(pressed);
  expect((search() as HTMLInputElement).value).toBe("");
  expect(screen.getAllByTestId("hrow")).toHaveLength(4);
});

test("the result carries a VERB: Select all states the whole match on both sides", async () => {
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  interleavedScene();
  render(<Hierarchy client={fakeClient({ selectEntities })} />);

  // Nothing to act on until a search names a set — the button is absent, not enabled-and-inert.
  expect(screen.queryByTestId("select-matches")).toBeNull();

  fireEvent.change(search(), { target: { value: "kind:light" } });
  const button = screen.getByTestId("select-matches");
  expect(button.getAttribute("data-count")).toBe("2");

  fireEvent.click(button);
  // BOTH sides: the store the Inspector and the rows read, and the engine the 3D outline is drawn from.
  expect(projectionStore.getState().multiSelect).toEqual(["key", "fill"]);
  expect(selectEntities).toHaveBeenLastCalledWith(["key", "fill"]);
});

test("a shift-click range cannot reach THROUGH the filter into rows nobody can see", () => {
  // THE DEFECT THIS PINS (ADR-185): `selectRange` walked `order` — every entity in the scene — so
  // shift-clicking the first and last VISIBLE rows of a filtered list selected everything between them
  // in the unfiltered scene. On a 15,711-part import that is hundreds of invisible objects, silently,
  // immediately before the key ADR-183 bound to Delete.
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  interleavedScene();
  render(<Hierarchy client={fakeClient({ selectEntities })} />);

  fireEvent.change(search(), { target: { value: "kind:light" } });
  const rows = Object.fromEntries(screen.getAllByTestId("hrow").map((r) => [r.getAttribute("data-id"), r]));
  fireEvent.click(rows.key!);
  fireEvent.click(rows.fill!, { shiftKey: true });

  expect(projectionStore.getState().multiSelect).toEqual(["key", "fill"]);
  expect(selectEntities).toHaveBeenLastCalledWith(["key", "fill"]);

  // And the unfiltered range still spans the whole list, so the fix narrowed the gesture and nothing else.
  fireEvent.change(search(), { target: { value: "" } });
  const all = Object.fromEntries(screen.getAllByTestId("hrow").map((r) => [r.getAttribute("data-id"), r]));
  fireEvent.click(all.key!);
  fireEvent.click(all.fill!, { shiftKey: true });
  expect(projectionStore.getState().multiSelect).toEqual(["key", "bolt-a", "bolt-b", "fill"]);
});

test("arrow-key navigation states its selection through the same seam as the mouse", () => {
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  interleavedScene();
  render(<Hierarchy client={fakeClient({ selectEntities })} />);
  fireEvent.keyDown(screen.getByRole("tree", { name: "Scene objects" }), { key: "ArrowDown" });
  expect(projectionStore.getState().selectedId).toBe("key");
  expect(selectEntities).toHaveBeenLastCalledWith(["key"]);
});

test("Ctrl/Cmd-F reaches this box, and starts a new search rather than appending to the last", () => {
  interleavedScene();
  render(<Hierarchy client={fakeClient()} />);
  fireEvent.change(search(), { target: { value: "bolt" } });
  expect(document.activeElement).not.toBe(search());

  act(() => requestObjectSearch());
  expect(document.activeElement).toBe(search());
  // The previous query is selected, so typing replaces it — the behaviour of every find field.
  expect((search() as HTMLInputElement).selectionStart).toBe(0);
  expect((search() as HTMLInputElement).selectionEnd).toBe(4);
});

test("the no-results state names the filters that exist, at the moment the user wants one", () => {
  interleavedScene();
  render(<Hierarchy client={fakeClient()} />);
  fireEvent.change(search(), { target: { value: "kinds:light" } });
  expect(screen.getByText("No matching objects")).toBeTruthy();
  const description = screen.getByText(/Nothing matches/);
  expect(description.textContent).toContain("kind:");
  expect(description.textContent).toContain("has:");
  expect(description.textContent).toContain("needs:binding");
});

test("the range at the size the defect actually had: 57 drawn rows, not the 169 they span", () => {
  // THE MEASUREMENT BEHIND ADR-185 defect 3, on a scene shaped like an import: 171 parts, every third
  // one a bolt. Filter `bolt`, shift-click the first and last visible rows, and the OLD code took
  // `order.slice(0, 169)` — 112 objects the user could not see, in a selection they were about to
  // press Delete on. The rows drawn between those two clicks are 57.
  const rows = [];
  for (let i = 0; i < 171; i += 1) {
    rows.push({
      id: `part-${i}`,
      name: i % 3 === 0 ? "Bolt M12" : `Skid Frame Member ${i}`,
      parentId: null,
      components: { MeshRenderer: { mesh: "m" } },
    });
  }
  projectionStore.getState().bulkLoad(rows as never);
  render(<Hierarchy client={fakeClient()} />);

  fireEvent.change(search(), { target: { value: "bolt" } });
  expect(document.getElementById("count")?.getAttribute("data-matches")).toBe("57");

  // THE GESTURE, not a re-computation of it: click the first row, scroll the virtualized list to its
  // end, shift-click the last row that is drawn. Recomputing the filter here to hand `selectRange` a
  // scope would be a second statement of the thing under test.
  fireEvent.click(screen.getAllByTestId("hrow")[0]);
  fireEvent.scroll(screen.getByRole("tree", { name: "Scene objects" }), { target: { scrollTop: 57 * 32 } });
  const drawn = screen.getAllByTestId("hrow");
  const last = drawn[drawn.length - 1];
  expect(last.getAttribute("data-id")).toBe("part-168");
  fireEvent.click(last, { shiftKey: true });

  const selected = projectionStore.getState().multiSelect;
  expect(selected).toHaveLength(57);
  // Stated as a PROPERTY, not only a count: every member answers the query. A count alone would still
  // pass if the range had drifted by one row at each end, which is the shape of the defect it pins.
  const summaries = projectionStore.getState().summaries;
  expect(selected.every((id) => summaries[id]?.name === "Bolt M12")).toBe(true);
});
