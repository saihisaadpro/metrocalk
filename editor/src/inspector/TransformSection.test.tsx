//! ADR-172 — the Transform, as the Inspector draws it.
//!
//! Written against the defect first. On the real core, and in the dev mock, an entity's `Transform`
//! is `{x, y, z}` until something rotates or scales it — so the panel drew three rows labelled `x`,
//! `y` and `z`, with no unit column and no reset, and offered no way to rotate or scale an object at
//! all. Rotate it once with the gizmo and four more rows appeared: `qx`, `qy`, `qz`, `qw`, each an
//! independently editable box on a value that is only a rotation when its length is exactly 1.

import { afterEach, expect, test, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Inspector } from "./Inspector";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { uiStore } from "../store/ui";
import { fakeClient } from "../transport/test-client";
import { quatToEulerDeg } from "./transform";

afterEach(() => {
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
  toastStore.getState().reset();
  window.localStorage.clear();
});

/** What `capscene::create_entity` actually commits: three position fields and nothing else. */
function freshObject(): void {
  projectionStore.getState().bulkLoad([
    { id: "e1", name: "Crate", parentId: null, components: { Transform: { x: 1.5, y: 0, z: -2 } } },
  ] as never);
  projectionStore.getState().select("e1");
}

const q = (input: HTMLElement | null) => input as HTMLInputElement | null;

test("a sparse Transform still offers a rotation and a scale — the engine reads the absent fields as identity", () => {
  freshObject();
  const { container } = render(<Inspector client={fakeClient()} />);

  // Position, from the fields that ARE there.
  expect(q(container.querySelector("#mtk-prop-Transform-x"))!.value).toBe("1.5");
  expect(q(container.querySelector("#mtk-prop-Transform-z"))!.value).toBe("-2");
  // Rotation and scale, from the identity the renderer is already drawing this object with. Before
  // ADR-172 these rows did not exist for this entity at all, so its rotation was authorable only by
  // dragging the gizmo and its scale not at all.
  expect(q(container.querySelector("#mtk-prop-Transform-qy"))!.value).toBe("0");
  expect(q(container.querySelector("#mtk-prop-Transform-scale"))!.value).toBe("1");
});

test("the position rows carry the engine's own titles and units, not the raw wire names", () => {
  freshObject();
  render(<Inspector client={fakeClient()} />);
  // The curated schema fires for the first time: `x` reads "Position X" and the metres live in the
  // sheet's own unit column. Against the phantom `px`/`py`/`pz` table it never once did.
  expect(screen.getByText("Position X")).toBeTruthy();
  expect(screen.getByText("Rotation Y")).toBeTruthy();
  expect(screen.getByText("Scale")).toBeTruthy();
  const units = screen.getAllByTestId("prop-unit").map((el) => el.textContent);
  expect(units).toContain("m");
  expect(units).toContain("°");
  expect(units).toContain("×");
});

test("the four quaternion components are never four rows", () => {
  projectionStore.getState().bulkLoad([
    {
      id: "e1",
      name: "Boom",
      parentId: null,
      // A gizmo-rotated object: exactly what `capscene::set_transform` commits.
      components: { Transform: { x: 0, y: 0, z: 0, qx: 0, qy: Math.SQRT1_2, qz: 0, qw: Math.SQRT1_2, scale: 1 } },
    },
  ] as never);
  projectionStore.getState().select("e1");
  const { container } = render(<Inspector client={fakeClient()} />);

  // `qw` has no row of its own — it is not an angle, and a box for it is a box that can break the
  // unit-length invariant on its own.
  expect(container.querySelector("#mtk-prop-Transform-qw")).toBeNull();
  // And the three that DO have rows read degrees, not quaternion components: 0.7071 reads as 90.
  expect(q(container.querySelector("#mtk-prop-Transform-qy"))!.value).toBe("90");
  expect(q(container.querySelector("#mtk-prop-Transform-qx"))!.value).toBe("0");
});

test("typing a rotation commits ONE normalised quaternion, not four field writes", async () => {
  freshObject();
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);

  const yaw = q(container.querySelector("#mtk-prop-Transform-qy"))!;
  fireEvent.change(yaw, { target: { value: "45" } });
  fireEvent.blur(yaw);

  await waitFor(() => expect(client.setRotation).toHaveBeenCalledTimes(1));
  const [ids, quat] = vi.mocked(client.setRotation).mock.calls[0];
  expect(ids).toEqual(["e1"]);
  // The quaternion is a rotation, and it is the one that was typed.
  expect(Math.hypot(...quat)).toBeCloseTo(1, 12);
  expect(quatToEulerDeg(quat)[1]).toBeCloseTo(45, 4);
  // Never through the per-field path — that is the four-transactions-and-a-broken-quaternion route.
  expect(client.setField).not.toHaveBeenCalled();
});

test("a position edit still goes through the ordinary field path, one field at a time", async () => {
  freshObject();
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);

  const x = q(container.querySelector("#mtk-prop-Transform-x"))!;
  fireEvent.change(x, { target: { value: "4" } });
  fireEvent.blur(x);

  await waitFor(() => expect(client.setField).toHaveBeenCalledTimes(1));
  expect(vi.mocked(client.setField).mock.calls[0].slice(0, 4)).toEqual(["e1", "Transform", "x", 4]);
  expect(client.setRotation).not.toHaveBeenCalled();
});

test("an engine refusal is shown where the user acted", async () => {
  freshObject();
  const client = fakeClient();
  vi.mocked(client.setRotation).mockResolvedValueOnce({
    ok: false,
    changed: 0,
    reason: "1 of 1 selected objects have no position in the world, so they cannot be rotated",
  });
  const { container } = render(<Inspector client={client} />);

  const yaw = q(container.querySelector("#mtk-prop-Transform-qy"))!;
  fireEvent.change(yaw, { target: { value: "30" } });
  fireEvent.blur(yaw);

  await waitFor(() => expect(uiStore.getState().status).toContain("cannot be rotated"));
  expect(toastStore.getState().toasts.map((t) => t.text).join(" ")).toContain("cannot be rotated");
});

// ── the selection (ADR-169's rule, applied to a derived property) ──────────────────────────────────

/** Three crates: two share a rotation, the third does not; all three sit at different x. */
function threeCrates(): void {
  const s = Math.SQRT1_2;
  projectionStore.getState().bulkLoad([
    { id: "a", name: "A", parentId: null, components: { Transform: { x: 0, y: 1, z: 0, qy: s, qw: s } } },
    { id: "b", name: "B", parentId: null, components: { Transform: { x: 3, y: 1, z: 0, qy: s, qw: s } } },
    { id: "c", name: "C", parentId: null, components: { Transform: { x: 6, y: 1, z: 0 } } },
  ] as never);
  projectionStore.getState().select("a");
  projectionStore.getState().toggleSelect("b");
  projectionStore.getState().toggleSelect("c");
}

test("a selection that is rotated differently reads Mixed, and one that agrees shows the value", () => {
  threeCrates();
  const { container } = render(<Inspector client={fakeClient()} />);

  const yaw = q(container.querySelector("#mtk-prop-Transform-qy"))!;
  expect(yaw.value).toBe("");
  expect(yaw.placeholder).toBe("Mixed");
  // y is 1 on all three, so it is shown as an ordinary value — the Transform section obeys the same
  // rule the schema-driven rows do.
  expect(q(container.querySelector("#mtk-prop-Transform-y"))!.value).toBe("1");
  expect(q(container.querySelector("#mtk-prop-Transform-x"))!.placeholder).toBe("Mixed");
});

test("rotating a selection is ONE call carrying every id", async () => {
  threeCrates();
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);

  const yaw = q(container.querySelector("#mtk-prop-Transform-qy"))!;
  fireEvent.change(yaw, { target: { value: "180" } });
  fireEvent.blur(yaw);

  await waitFor(() => expect(client.setRotation).toHaveBeenCalledTimes(1));
  // Order is the order the user built the selection in, primary first — the claim is that EVERY id
  // rides one call, not which one leads it.
  expect([...vi.mocked(client.setRotation).mock.calls[0][0]].sort()).toEqual(["a", "b", "c"]);
  await waitFor(() => expect(uiStore.getState().status).toContain("rotation on 3"));
});

test("a position edit on a selection goes through the batched path, once", async () => {
  threeCrates();
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);

  const x = q(container.querySelector("#mtk-prop-Transform-x"))!;
  fireEvent.change(x, { target: { value: "0" } });
  fireEvent.blur(x);

  await waitFor(() => expect(client.multiEdit).toHaveBeenCalledTimes(1));
  const [ids, component, field, value] = vi.mocked(client.multiEdit).mock.calls[0];
  expect([...ids].sort()).toEqual(["a", "b", "c"]);
  expect([component, field, value]).toEqual(["Transform", "x", 0]);
});

test("a typed angle survives until the engine answers — the box does not snap back", async () => {
  // THE DEFECT THE FIRST CAPTURE OF THIS SECTION SHOWED. `set_rotation` is a command, not the
  // optimistic `setField` echo, so the projection answers a round trip later. The pending angle was
  // being thrown away on the very next render — its clearing effect depended on the object identity
  // `readTransform` rebuilds every time — so the box read the OLD angle while a transaction carrying
  // the new one was in flight.
  projectionStore.getState().bulkLoad([
    {
      id: "e1",
      name: "Boom",
      parentId: null,
      components: { Transform: { x: 0, y: 0, z: 0, qx: 0, qy: Math.sin(Math.PI / 8), qz: 0, qw: Math.cos(Math.PI / 8) } },
    },
  ] as never);
  projectionStore.getState().select("e1");
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);

  const yaw = q(container.querySelector("#mtk-prop-Transform-qy"))!;
  expect(yaw.value).toBe("45");
  fireEvent.change(yaw, { target: { value: "0" } });
  fireEvent.blur(yaw);

  await waitFor(() => expect(client.setRotation).toHaveBeenCalledTimes(1));

  // AND THE PANEL RE-RENDERS WHILE THE WRITE IS IN FLIGHT, which is the half that made this a real
  // defect rather than a theoretical one: an unrelated delta arrives (any scene change does it — a
  // rename, another editor, the log line the shots probe itself paints), the Inspector re-renders,
  // and `readTransform` hands the section a brand-new object. Keyed on that object's IDENTITY the
  // clearing effect fired on this render and threw the typed angle away; keyed on its NUMBERS it does
  // not. Without this line the test cannot tell the two apart, because a section whose parent never
  // re-renders keeps its draft either way.
  projectionStore.getState().applyDelta({
    ops: [{ op: "setField", id: "e1", component: "Health", field: "hp", value: 42 }],
    confirms: [],
    rejects: [],
    full: false,
  } as never);
  await waitFor(() => expect(container.querySelector("#mtk-prop-Health-hp")).toBeTruthy());

  // The projection still says 45 — nothing has echoed back — and the box must show what was typed.
  //
  // AND THIS ASSERTION IS NOT THE GATE FOR IT — measured, not assumed. Reintroducing the defect (key
  // the memo on `transforms[0]` and the clearing effect on the array it returns) leaves this test
  // GREEN, because a store update applied in a separate `act` does not reproduce the click's own
  // synchronous re-render, which is what actually threw the draft away. The `shots` scene
  // `inspector-rotation-is-one-transaction` asserts the committed angle through `aria-valuenow` in a
  // real Chromium and goes red on all three of its claims with the same mutation. What is left here
  // is worth keeping — the box must show what was typed — it is simply not the thing that can fail.
  await waitFor(() =>
    expect(q(container.querySelector("#mtk-prop-Transform-qy"))!.value).toBe("0"),
  );
});

test("a pending angle is dropped when the selection moves to another object", async () => {
  // BOTH OBJECTS ARE UNROTATED, deliberately: if they differed the committed angle would change on
  // reselect and the draft would be cleared by that alone, which is a test that proves nothing about
  // the selection. Here the only thing that changes is WHICH object is selected.
  projectionStore.getState().bulkLoad([
    { id: "e1", name: "A", parentId: null, components: { Transform: { x: 0, y: 0, z: 0 } } },
    { id: "e2", name: "B", parentId: null, components: { Transform: { x: 5, y: 0, z: 0 } } },
  ] as never);
  projectionStore.getState().select("e1");
  const client = fakeClient();
  const { container } = render(<Inspector client={client} />);

  const yaw = q(container.querySelector("#mtk-prop-Transform-qy"))!;
  fireEvent.change(yaw, { target: { value: "90" } });
  fireEvent.blur(yaw);
  await waitFor(() => expect(client.setRotation).toHaveBeenCalledTimes(1));
  expect(q(container.querySelector("#mtk-prop-Transform-qy"))!.value).toBe("90");

  projectionStore.getState().select("e2");
  // e2 is unrotated and nothing has been typed on it — 0, not e1's pending 90 carried across.
  await waitFor(() =>
    expect(q(container.querySelector("#mtk-prop-Transform-x"))!.value).toBe("5"),
  );
  await waitFor(() =>
    expect(q(container.querySelector("#mtk-prop-Transform-qy"))!.value).toBe("0"),
  );
});
