//! ADR-169 — the Inspector when the selection is more than one object.
//!
//! Written against the defect first: with three objects selected the panel named one of them, showed
//! its properties, and editing a field wrote to that one. Nothing on screen said the other two were
//! selected, so "I changed the intensity and only one light moved" was the report.

import { afterEach, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Inspector } from "./Inspector";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";

afterEach(() => {
  projectionStore.getState().reset();
  window.localStorage.clear();
});

/** Three lights: two agree about intensity, the third does not; all three agree about kind. */
function threeLights(): void {
  projectionStore.getState().bulkLoad([
    { id: "l1", name: "Key", parentId: null, components: { Transform: { x: 0, y: 2, z: 0 }, Light: { kind: "point", intensity: 60 } } },
    { id: "l2", name: "Fill", parentId: null, components: { Transform: { x: 1, y: 2, z: 0 }, Light: { kind: "point", intensity: 60 } } },
    { id: "l3", name: "Rim", parentId: null, components: { Transform: { x: 2, y: 2, z: 0 }, Light: { kind: "point", intensity: 12 } } },
  ] as never);
  projectionStore.getState().select("l1");
  projectionStore.getState().toggleSelect("l2");
  projectionStore.getState().toggleSelect("l3");
}

test("selecting three objects shows a panel about the SELECTION, not about one of them", () => {
  threeLights();
  render(<Inspector client={fakeClient()} />);
  expect(screen.getByTestId("inspectorMulti")).toBeTruthy();
  // It counts them and it says what they are — the header used to name one object and nothing else.
  expect(screen.getByTestId("multiTitle").textContent).toBe("3 lights selected");
  expect(screen.getByTestId("multiSubtitle").textContent).toContain("Key");
  expect(screen.getByTestId("multiSubtitle").textContent).toContain("Rim");
});

test("a field the selection disagrees about reads Mixed, and one it agrees about shows the value", () => {
  threeLights();
  const { container } = render(<Inspector client={fakeClient()} />);
  const intensity = container.querySelector<HTMLInputElement>("#mtk-prop-Light-intensity");
  const kind = container.querySelector<HTMLSelectElement>("#mtk-prop-Light-kind");
  expect(intensity).toBeTruthy();
  expect(kind).toBeTruthy();
  // 60, 60, 12 → no value is true of all three, so the box shows none and says so.
  expect(intensity!.value).toBe("");
  expect(intensity!.placeholder).toBe("Mixed");
  expect(intensity!.getAttribute("data-mixed")).toBe("1");
  // point, point, point → the value IS true of all three, so it is shown as an ordinary value.
  expect(kind!.value).toBe("point");
  expect(kind!.getAttribute("data-mixed")).toBeNull();
});

test("editing a shared field writes to EVERY selected object, in ONE transaction", async () => {
  threeLights();
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);
  const intensity = container.querySelector<HTMLInputElement>("#mtk-prop-Light-intensity")!;

  fireEvent.focus(intensity);
  fireEvent.change(intensity, { target: { value: "24" } });
  fireEvent.blur(intensity);

  // JSON Forms dispatches its `onChange` off the commit, not inside it — asserting synchronously
  // here passes for a panel that emits NOTHING, which is the whole defect under test.
  await waitFor(() => expect(client.multiEdit).toHaveBeenCalledTimes(1));
  const [ids, component, field, value] = vi.mocked(client.multiEdit).mock.calls[0];
  expect([...ids].sort()).toEqual(["l1", "l2", "l3"]);
  expect(component).toBe("Light");
  expect(field).toBe("intensity");
  expect(value).toBe(24);
  // And NOT through the single-entity path, which would have been three transactions and three undos.
  expect(client.setField).not.toHaveBeenCalled();
});

test("mounting the multi panel emits nothing — a mixed field must not commit a default nobody typed", async () => {
  // The single-object panel proves this by diffing against the projection (data === projection at
  // mount). Here a mixed field's data is `undefined`, so a validator filling in the schema's declared
  // default would diff as a change and write it to all three before anything was touched.
  //
  // THE WAIT IS PART OF THE CLAIM. `onChange` is dispatched asynchronously, so a synchronous
  // `not.toHaveBeenCalled()` is satisfied by any panel at all, including one that writes a moment
  // later — the vacuous pass this repository keeps paying for. It is given the same window the
  // positive case above needs, and one confirmed edit inside it, before it says nothing happened.
  threeLights();
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);
  const kind = container.querySelector<HTMLSelectElement>("#mtk-prop-Light-kind")!;
  fireEvent.change(kind, { target: { value: "spot" } });
  await waitFor(() => expect(client.multiEdit).toHaveBeenCalledTimes(1));
  // The ONE call is the one just made — nothing rode along from the mount.
  expect(vi.mocked(client.multiEdit).mock.calls[0][1]).toBe("Light");
  expect(vi.mocked(client.multiEdit).mock.calls[0][2]).toBe("kind");
});

test("a component only part of the selection carries is withheld, counted and named", () => {
  projectionStore.getState().bulkLoad([
    { id: "a", name: "Lamp", parentId: null, components: { Transform: { x: 0 }, Light: { intensity: 60 } } },
    { id: "b", name: "Crate", parentId: null, components: { Transform: { x: 1 } } },
  ] as never);
  projectionStore.getState().select("a");
  projectionStore.getState().toggleSelect("b");
  const { container } = render(<Inspector client={fakeClient()} />);

  // Offering `Light.intensity` here would either refuse the whole batch or — because `Op::SetField`
  // CREATES a component it does not find — give the crate a one-field Light it never had.
  expect(container.querySelector("#mtk-prop-Light-intensity")).toBeNull();
  expect(container.querySelector("#mtk-prop-Transform-x")).toBeTruthy();
  const note = screen.getByTestId("multiPartial").textContent!;
  expect(note).toContain("1 property is on only some of these");
  expect(note).toContain("Light");
  expect(note).toContain("select one object");
});

test("a selection with nothing in common gets a real empty state, not a blank pane", () => {
  projectionStore.getState().bulkLoad([
    { id: "a", name: "Lamp", parentId: null, components: { Light: { intensity: 60 } } },
    { id: "b", name: "Crate", parentId: null, components: { RigidBody: { mass: 5 } } },
  ] as never);
  projectionStore.getState().select("a");
  projectionStore.getState().toggleSelect("b");
  render(<Inspector client={fakeClient()} />);
  expect(screen.getByTestId("multiEmpty").textContent).toMatch(/no properties in common/i);
});

test("a refusal is shown where the user acted, with the engine's own sentence", async () => {
  threeLights();
  const client = fakeClient();
  vi.mocked(client.multiEdit).mockResolvedValue({
    ok: false,
    changed: 0,
    reason: "1 of 3 selected objects have no Light",
  });
  const { container } = render(<Inspector client={client} />);
  const intensity = container.querySelector<HTMLInputElement>("#mtk-prop-Light-intensity")!;
  fireEvent.focus(intensity);
  fireEvent.change(intensity, { target: { value: "24" } });
  fireEvent.blur(intensity);

  const said = await screen.findByTestId("multiRefusal");
  expect(said.textContent).toBe("1 of 3 selected objects have no Light");
});

test("a mixed selection of different kinds names the makeup instead of pretending it is one thing", () => {
  // The kind is DERIVED from the components (`deriveKind`, mirroring the shell's `classify_kind`), so
  // the fixture states the components and lets the store reach the same answer the real core would.
  projectionStore.getState().bulkLoad([
    { id: "a", name: "Lamp", parentId: null, components: { Transform: { x: 0 }, Light: { intensity: 60 } } },
    { id: "b", name: "Crate", parentId: null, components: { Transform: { x: 1 }, MeshRenderer: { mesh: "m" } } },
    { id: "c", name: "Box", parentId: null, components: { Transform: { x: 2 }, MeshRenderer: { mesh: "m" } } },
  ] as never);
  projectionStore.getState().select("a");
  projectionStore.getState().toggleSelect("b");
  projectionStore.getState().toggleSelect("c");
  render(<Inspector client={fakeClient()} />);
  expect(screen.getByTestId("multiTitle").textContent).toBe("3 objects selected");
  expect(screen.getByTestId("multiSubtitle").textContent).toBe("2 meshes · 1 light");
});

test("dropping back to one selected object restores the single-object panel unchanged", () => {
  threeLights();
  const { rerender } = render(<Inspector client={fakeClient()} />);
  expect(screen.queryByTestId("inspectorMulti")).toBeTruthy();
  projectionStore.getState().select("l2");
  rerender(<Inspector client={fakeClient()} />);
  expect(screen.queryByTestId("inspectorMulti")).toBeNull();
  expect(screen.getByText("Fill")).toBeTruthy();
});
