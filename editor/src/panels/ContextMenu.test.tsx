//! ContextMenu (M3.3 + ADR-183) — verified headless in jsdom: the registry-derived actions render for
//! the SELECTION, an unavailable action is greyed WITH its reason (every "no" explained), an AVAILABLE
//! row dispatches the right contract verb over the whole set + closes, and a DISABLED row is inert.
//! Asserts REAL behavior (the right client method called with the right ids, the reason rendered, the
//! menu closed), not "it rendered".

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ContextMenu } from "./ContextMenu";
import { projectionStore } from "../store/projection";
import { fakeClient } from "../transport/test-client";
import type { ActionItem, EntityProjection, SelectionActions } from "../transport/protocol";

afterEach(() => projectionStore.getState().reset());

/** An action the way the engine states it — `appliesTo` IS the availability (ADR-183). */
const act = (action: string, label: string, appliesTo: number, mutates: boolean, reason?: string): ActionItem => ({
  action,
  label,
  available: appliesTo > 0,
  reason: appliesTo > 0 ? undefined : reason,
  mutates,
  appliesTo,
});

const answer = (count: number, items: ActionItem[], missing = 0): SelectionActions => ({ count, missing, items });

const ACTIONS = answer(1, [act("remove", "Delete", 1, true), act("bind", "Bind…", 0, false, "no unmet requirement")]);

/** Load `ids` into the projection so `entityLabel`/`similarTo` have something to read. */
function loadScene(entities: Array<Partial<EntityProjection> & { id: string }>) {
  projectionStore.getState().bulkLoad(
    entities.map((e) => ({ parentId: null, components: {}, name: e.id, ...e })) as never,
  );
}

test("actions render; an unavailable action is greyed WITH its reason; an available row dispatches + closes; a disabled row is inert", async () => {
  const deleteDeactivateMany = vi.fn((ids: string[]) => Promise.resolve(ids));
  const onClose = vi.fn();
  const client = fakeClient({
    entityActionsFor: () => Promise.resolve(ACTIONS),
    deleteDeactivateMany,
  });

  render(<ContextMenu client={client} ids={["e1"]} onClose={onClose} />);

  // (a) both rows render, plus the always-present `Select similar` row
  const rows = await screen.findAllByTestId("ctxitem");
  expect(rows).toHaveLength(3);
  expect(screen.getByRole("menu").getAttribute("aria-label")).toBe("Actions for e1");
  expect(rows.every((row) => row.getAttribute("role") === "menuitem")).toBe(true);

  const removeRow = rows.find((r) => r.dataset.action === "remove")!;
  const bindRow = rows.find((r) => r.dataset.action === "bind")!;
  expect(removeRow).toBeTruthy();
  expect(bindRow).toBeTruthy();

  // (b) the bind row is disabled AND its text carries the reason (every "no" explained)
  expect(bindRow.className).toContain("disabled");
  expect(bindRow.textContent).toContain("Bind…");
  expect(bindRow.textContent).toContain("no unmet requirement");
  // the available row is NOT disabled and shows only its label — a single object needs no scope note
  expect(removeRow.className).not.toContain("disabled");
  expect(removeRow.textContent).toBe("Delete");

  // (d) clicking the DISABLED Bind row does NOT dispatch and does NOT close
  fireEvent.click(bindRow);
  expect(onClose).not.toHaveBeenCalled();
  expect(projectionStore.getState().selectedId).toBeNull(); // bind would have select()ed

  // (c) clicking the AVAILABLE Delete row → the batched delete over the selection + onClose
  fireEvent.click(removeRow);
  expect(deleteDeactivateMany).toHaveBeenCalledTimes(1);
  expect(deleteDeactivateMany).toHaveBeenCalledWith(["e1"]);
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("the menu states the scope it acts on, and Delete acts on the WHOLE selection", async () => {
  loadScene([{ id: "b1", name: "Bolt M12" }, { id: "b2", name: "Bolt M12" }, { id: "b3", name: "Bolt M12" }]);
  const deleteDeactivateMany = vi.fn((ids: string[]) => Promise.resolve(ids));
  const onClose = vi.fn();
  const client = fakeClient({
    // Three live objects: Delete takes all three, Duplicate is primary-only.
    entityActionsFor: () =>
      Promise.resolve(answer(3, [act("remove", "Delete", 3, true), act("duplicate", "Duplicate", 1, true)])),
    deleteDeactivateMany,
  });

  render(<ContextMenu client={client} ids={["b1", "b2", "b3"]} onClose={onClose} />);

  // The subject line names the scope BEFORE the first verb — `<ux_quality>` 2.
  const subject = await screen.findByTestId("ctxmenu-subject");
  expect(subject.textContent).toBe("3 objects selected");

  const rows = await screen.findAllByTestId("ctxitem");
  const removeRow = rows.find((r) => r.dataset.action === "remove")!;
  const duplicateRow = rows.find((r) => r.dataset.action === "duplicate")!;

  // A verb that takes the whole set says nothing extra; a verb that does not MUST say so on the row.
  expect(removeRow.dataset.appliesTo).toBe("3");
  expect(removeRow.textContent).toBe("Delete");
  expect(duplicateRow.dataset.appliesTo).toBe("1");
  expect(duplicateRow.textContent).toContain("this one only");

  fireEvent.click(removeRow);
  expect(deleteDeactivateMany).toHaveBeenCalledWith(["b1", "b2", "b3"]);
  await waitFor(() => expect(projectionStore.getState().multiSelect).toEqual([]));
});

test("a partially-applicable verb prints how many of the selection it reaches", async () => {
  const client = fakeClient({
    entityActionsFor: () =>
      Promise.resolve(answer(9, [act("makedynamic", "Make dynamic", 4, true)])),
  });
  render(<ContextMenu client={client} ids={["a", "b", "c", "d", "e", "f", "g", "h", "i"]} onClose={vi.fn()} />);
  const rows = await screen.findAllByTestId("ctxitem");
  const md = rows.find((r) => r.dataset.action === "makedynamic")!;
  expect(md.textContent).toContain("4 of 9");
});

test("Select similar selects every object sharing the primary's geometry, through BOTH halves of the selection", async () => {
  loadScene([
    { id: "b1", name: "Bolt M12", components: { MeshRenderer: { mesh: "sha-bolt" } } },
    { id: "plate", name: "Plate", components: { MeshRenderer: { mesh: "sha-plate" } } },
    { id: "b2", name: "Bolt M12", components: { MeshRenderer: { mesh: "sha-bolt" } } },
  ]);
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  const onClose = vi.fn();
  const client = fakeClient({
    entityActionsFor: () => Promise.resolve(answer(1, [act("remove", "Delete", 1, true)])),
    selectEntities,
  });

  render(<ContextMenu client={client} ids={["b1"]} onClose={onClose} />);
  const rows = await screen.findAllByTestId("ctxitem");
  const similar = rows.find((r) => r.dataset.action === "selectsimilar")!;
  // The row says what it will do BEFORE the click, and names what it matched on.
  expect(similar.textContent).toContain("Select similar");
  expect(similar.textContent).toContain("sharing the geometry of Bolt M12");

  fireEvent.click(similar);
  // The STORE the inspector reads, and the ENGINE the picture is outlined from.
  expect(projectionStore.getState().multiSelect).toEqual(["b1", "b2"]);
  expect(selectEntities).toHaveBeenCalledWith(["b1", "b2"]);
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("Select similar refuses WITH a reason when the object has nothing to match on", async () => {
  loadScene([{ id: "bare", name: "Bare" }]);
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  const client = fakeClient({
    entityActionsFor: () => Promise.resolve(answer(1, [act("remove", "Delete", 1, true)])),
    selectEntities,
  });
  render(<ContextMenu client={client} ids={["bare"]} onClose={vi.fn()} />);
  const rows = await screen.findAllByTestId("ctxitem");
  const similar = rows.find((r) => r.dataset.action === "selectsimilar")!;
  expect(similar.className).toContain("disabled");
  expect(similar.textContent).toContain("nothing to match on");
  fireEvent.click(similar);
  expect(selectEntities).not.toHaveBeenCalled();
});

test("focus action routes to client.focusEntity(primary) and closes", async () => {
  const focusEntity = vi.fn();
  const onClose = vi.fn();
  const client = fakeClient({
    entityActionsFor: () => Promise.resolve(answer(1, [act("focus", "Focus", 1, false)])),
    focusEntity,
  });

  render(<ContextMenu client={client} ids={["e7"]} onClose={onClose} />);
  const rows = await screen.findAllByTestId("ctxitem");
  fireEvent.click(rows.find((r) => r.dataset.action === "focus")!);

  expect(focusEntity).toHaveBeenCalledWith("e7");
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("arrow keys rove focus across all explained actions; Home, End, and Escape follow the menu pattern", async () => {
  const onClose = vi.fn();
  const client = fakeClient({
    entityActionsFor: () =>
      Promise.resolve(
        answer(1, [
          act("inspect", "Inspect", 1, false),
          act("bind", "Bind", 0, false, "already bound"),
          act("remove", "Delete", 1, true),
        ]),
      ),
  });

  render(<ContextMenu client={client} ids={["e2"]} onClose={onClose} />);
  const items = await screen.findAllByRole("menuitem");
  // Three registry rows plus `Select similar`, which shares their roving-tabindex ring.
  expect(items).toHaveLength(4);

  // Row 0 (`Inspect`) is available, so it is where the menu opens.
  expect(document.activeElement).toBe(items[0]);
  expect(items.map((item) => item.tabIndex)).toEqual([0, -1, -1, -1]);

  fireEvent.keyDown(items[0], { key: "ArrowDown" });
  expect(document.activeElement).toBe(items[1]);
  expect(items[1].getAttribute("aria-disabled")).toBe("true");

  // Disabled actions remain focusable so their reason can be discovered, but never activate or close.
  fireEvent.click(items[1]);
  expect(onClose).not.toHaveBeenCalled();

  fireEvent.keyDown(items[1], { key: "End" });
  expect(document.activeElement).toBe(items[3]);
  fireEvent.keyDown(items[3], { key: "Home" });
  expect(document.activeElement).toBe(items[0]);

  fireEvent.keyDown(items[0], { key: "ArrowUp" });
  expect(document.activeElement).toBe(items[3]);

  fireEvent.keyDown(items[3], { key: "Escape" });
  expect(onClose).toHaveBeenCalledTimes(1);
});

test("the menu opens on the first row a person can USE, not on a refusal", async () => {
  // The capture is the argument: `Bind…` is the first row and is refused for most objects, so the menu
  // opened with a strong focus ring around the one row that does nothing, and Enter on it.
  const client = fakeClient({
    entityActionsFor: () =>
      Promise.resolve(
        answer(1, [
          act("bind", "Bind…", 0, false, "requires no capabilities"),
          act("remove", "Delete", 1, true),
        ]),
      ),
  });
  render(<ContextMenu client={client} ids={["e9"]} onClose={vi.fn()} />);
  const items = await screen.findAllByRole("menuitem");
  expect(document.activeElement).toBe(items[1]);
  expect(items[1].dataset.action).toBe("remove");
  // The refused row keeps its place in the ring — every "no" stays discoverable, it just is not first.
  expect(items[0].getAttribute("aria-disabled")).toBe("true");
  fireEvent.keyDown(items[1], { key: "Home" });
  expect(document.activeElement).toBe(items[0]);
});

test("with nothing usable the menu still opens on row 0, whose reason is the answer", async () => {
  const client = fakeClient({
    entityActionsFor: () =>
      Promise.resolve(
        answer(0, [
          act("bind", "Bind…", 0, false, "nothing is selected"),
          act("remove", "Delete", 0, true, "nothing is selected"),
        ]),
      ),
  });
  render(<ContextMenu client={client} ids={[]} onClose={vi.fn()} />);
  const items = await screen.findAllByRole("menuitem");
  expect(document.activeElement).toBe(items[0]);
  expect(items[0].textContent).toContain("nothing is selected");
  // A selection of nothing has no subject to state — the rows' own reasons say it instead.
  expect(screen.queryByTestId("ctxmenu-subject")).toBeNull();
});

test("loading and failed action queries expose explicit live feedback", async () => {
  let reject!: (reason: Error) => void;
  const entityActionsFor = vi.fn(
    () =>
      new Promise<SelectionActions>((_resolve, rejectPromise) => {
        reject = rejectPromise;
      }),
  );
  const client = fakeClient({ entityActionsFor });

  render(<ContextMenu client={client} ids={["offline"]} onClose={vi.fn()} />);
  expect(screen.getByTestId("ctxmenu-loading").getAttribute("role")).toBe("status");

  reject(new Error("offline"));
  const error = await screen.findByTestId("ctxmenu-error");
  expect(error.getAttribute("role")).toBe("alert");
  expect(error.getAttribute("aria-live")).toBe("assertive");
});
