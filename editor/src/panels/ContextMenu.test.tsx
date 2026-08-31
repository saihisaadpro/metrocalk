//! ContextMenu (M3.3 + ADR-183) — verified headless in jsdom: the registry-derived actions render for
//! the SELECTION, an unavailable action is greyed WITH its reason (every "no" explained), an AVAILABLE
//! row dispatches the right contract verb over the whole set + closes, and a DISABLED row is inert.
//! Asserts REAL behavior (the right client method called with the right ids, the reason rendered, the
//! menu closed), not "it rendered".

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { ContextMenu } from "./ContextMenu";
import { projectionStore } from "../store/projection";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import type { ActionItem, EntityProjection, SelectionActions } from "../transport/protocol";

afterEach(() => {
  projectionStore.getState().reset();
  toastStore.getState().reset();
});

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

test("focus frames the WHOLE selection, in one command, and closes", async () => {
  // The row used to call `focusEntity(primary)` and then `focusDebug()` for the banner's number — a set
  // of three framed one of them, in three round trips, under the word "focused" (ADR-194). It now asks
  // the engine to frame what the engine already knows is selected, and the reply carries the number.
  const focusSelection = vi.fn(() => Promise.resolve({ framed: 3, distance: 18.5, primary: "e7" }));
  const focusEntity = vi.fn();
  const focusDebug = vi.fn(() => Promise.resolve([0, false] as [number, boolean]));
  const onClose = vi.fn();
  const onFocus = vi.fn();
  const client = fakeClient({
    entityActionsFor: () => Promise.resolve(answer(3, [act("focus", "Focus", 3, false)])),
    focusSelection,
    focusEntity,
    focusDebug,
  });

  render(<ContextMenu client={client} ids={["e7", "e8", "e9"]} onClose={onClose} onFocus={onFocus} />);
  const rows = await screen.findAllByTestId("ctxitem");
  fireEvent.click(rows.find((r) => r.dataset.action === "focus")!);

  expect(focusSelection).toHaveBeenCalledTimes(1);
  expect(focusEntity).not.toHaveBeenCalled();
  expect(focusDebug).not.toHaveBeenCalled();
  expect(onClose).toHaveBeenCalledTimes(1);
  // The banner names the SET and gets the distance the framing itself chose.
  await waitFor(() => expect(onFocus).toHaveBeenCalledWith("3 objects", 18.5));
});

test("focus that frames nothing says so instead of raising a banner over an unchanged camera", async () => {
  // A stale list can name rows the render state has already lost. `framed: 0` means the camera did not
  // move, and a banner claiming a focus the engine did not enter is the `<ux_quality>` 6 defect.
  const onFocus = vi.fn();
  const client = fakeClient({
    entityActionsFor: () => Promise.resolve(answer(1, [act("focus", "Focus", 1, false)])),
    focusSelection: vi.fn(() => Promise.resolve({ framed: 0, distance: 60, primary: null })),
  });

  render(<ContextMenu client={client} ids={["gone"]} onClose={() => {}} onFocus={onFocus} />);
  const rows = await screen.findAllByTestId("ctxitem");
  fireEvent.click(rows.find((r) => r.dataset.action === "focus")!);

  await waitFor(() => expect(toastStore.getState().toasts.at(-1)?.text).toBe("select something to frame"));
  expect(onFocus).not.toHaveBeenCalled();
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

test("with nothing selected there are no verbs — SUPERSEDES the seven-fold refusal (ADR-191)", async () => {
  // ADR-183's version of this test asserted the opposite: that a menu opened over an empty selection
  // renders the engine's refusal rows and opens focus on the first of them, "whose reason is the
  // answer". That state was unreachable in the product — the stage refused to open a menu with
  // nothing selected at all — and the first time ADR-191 made it reachable, the `.exe` capture showed
  // what it actually looks like: two live rows above SEVEN greyed ones, each carrying the identical
  // six-word reason, taking two thirds of the surface, with the focus ring on the first refusal.
  //
  // Every "no" is still explained. It is explained ONCE, by the empty selection the user can see in
  // the Inspector beside it, instead of seven times by controls that do nothing — which is
  // `<ux_quality>` 6's inert controls rather than ADR-016's explained refusals.
  const entityActionsFor = vi.fn(() =>
    Promise.resolve(
      answer(0, [
        act("bind", "Bind…", 0, false, "nothing is selected"),
        act("remove", "Delete", 0, true, "nothing is selected"),
      ]),
    ),
  );
  loadScene([{ id: "e2", name: "Weld Gun" }]);
  render(<ContextMenu client={fakeClient({ entityActionsFor })} ids={[]} candidates={[under("e2", 12)]} onClose={vi.fn()} />);

  const rows = await screen.findAllByTestId("ctxcandidate");
  expect(rows).toHaveLength(1);
  expect(screen.queryAllByTestId("ctxitem")).toHaveLength(0);
  expect(screen.queryByText("nothing is selected")).toBeNull();
  expect(entityActionsFor).not.toHaveBeenCalled();
  // Focus opens on the only row that does anything.
  await waitFor(() => expect(document.activeElement).toBe(rows[0]));
  // A selection of nothing still has no subject to state.
  expect(screen.queryByTestId("ctxmenu-subject")).toBeNull();
});

test("a menu with nothing in it says so rather than rendering an empty box", async () => {
  render(<ContextMenu client={fakeClient({})} ids={[]} candidates={[]} onClose={vi.fn()} />);
  expect(await screen.findByText("Nothing here, and nothing selected.")).toBeTruthy();
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

// ── Under the pointer (ADR-191) ────────────────────────────────────────────────────────────────────
//
// The menu now also answers the one question a right-click can ask that nothing else can: what is at
// the point it was opened at. These assert the two halves that make it worth having — the SECOND
// object under a pixel is reachable, and the row does not lie about which one the click already took.

/** Candidates the way `pick_candidates` answers them: nearest first, with the depth that ordered them. */
const under = (id: string, distance: number, selected = false) => ({ id, kind: "Mesh", distance, selected });

test("the objects under the pointer are listed nearest-first, and the one already selected says so", async () => {
  loadScene([{ id: "e1", name: "Bracket" }, { id: "e2", name: "Weld Gun" }]);
  render(
    <ContextMenu
      client={fakeClient({ entityActionsFor: () => Promise.resolve(ACTIONS) })}
      ids={["e1"]}
      candidates={[under("e1", 25.7, true), under("e2", 28.2)]}
      onClose={vi.fn()}
    />,
  );

  const rows = await screen.findAllByTestId("ctxcandidate");
  expect(rows.map((r) => r.dataset.id)).toEqual(["e1", "e2"]);
  // NAMED, not keyed. A row reading `1_4a3f` is the exact failure `selectionText` exists to prevent.
  expect(rows[0].textContent).toContain("Bracket");
  expect(rows[1].textContent).toContain("Weld Gun");
  // The depth is why the list has an order; saying it is what makes "the one behind" a fact.
  expect(rows[1].textContent).toContain("28.2 m");
  // A list of two where one is the current answer reads very differently from a list of two.
  expect(rows[0].dataset.selected).toBe("true");
  expect(rows[0].textContent).toContain("selected");
  expect(rows[1].dataset.selected).toBe("false");
  expect(screen.getByTestId("ctxmenu-under").textContent).toContain("2");
});

test("choosing the one BEHIND selects it — the whole point of the section", async () => {
  loadScene([{ id: "e1", name: "Bracket" }, { id: "e2", name: "Weld Gun" }]);
  const selectEntities = vi.fn((ids: string[]) => Promise.resolve(ids));
  const onClose = vi.fn();
  render(
    <ContextMenu
      client={fakeClient({ entityActionsFor: () => Promise.resolve(ACTIONS), selectEntities })}
      ids={["e1"]}
      candidates={[under("e1", 25.7, true), under("e2", 28.2)]}
      onClose={onClose}
    />,
  );

  const rows = await screen.findAllByTestId("ctxcandidate");
  fireEvent.click(rows[1]);
  // THE ENGINE IS TOLD, and it is told a REPLACE: this row exists because the click took the wrong
  // object, and the fix for that is the right object selected — not a set of two.
  await waitFor(() => expect(selectEntities).toHaveBeenCalledWith(["e2"]));
  expect(projectionStore.getState().multiSelect).toEqual(["e2"]);
  expect(onClose).toHaveBeenCalled();
});

test("with NOTHING selected the list is the menu — no verbs, and the engine is not asked", async () => {
  loadScene([{ id: "e2", name: "Weld Gun" }]);
  // The engine answers an empty selection with a list of REFUSALS — six rows all reading "nothing is
  // selected". The `.exe` capture showed two live rows above seven of them, taking two thirds of the
  // menu, with the focus ring on the first. Every "no" is explained here: once, by the empty selection
  // the user can see beside it.
  const entityActionsFor = vi.fn(() =>
    Promise.resolve(answer(0, [act("bind", "Bind…", 0, false, "nothing is selected"), act("remove", "Delete", 0, true, "nothing is selected")])),
  );
  render(
    <ContextMenu client={fakeClient({ entityActionsFor })} ids={[]} candidates={[under("e2", 12)]} onClose={vi.fn()} />,
  );

  const rows = await screen.findAllByTestId("ctxcandidate");
  expect(rows).toHaveLength(1);
  expect(screen.queryAllByTestId("ctxitem")).toHaveLength(0);
  expect(screen.queryByText("nothing is selected")).toBeNull();
  // Not a question worth asking, either.
  expect(entityActionsFor).not.toHaveBeenCalled();
  // "No actions available." under a live list of objects reads as a failure OF THE LIST.
  expect(screen.queryByText("No actions available.")).toBeNull();
  expect(screen.getByRole("menu").getAttribute("aria-label")).toBe("What is under the pointer");
  // ONE ARROW-KEY RING over both sections: with no verbs, focus has to reach the candidate rather
  // than stopping at an empty action list.
  await waitFor(() => expect(document.activeElement).toBe(rows[0]));
});

test("a candidate row joins the SAME ring as the verbs — the keyboard contract does not change shape", async () => {
  loadScene([{ id: "e1", name: "Bracket" }, { id: "e2", name: "Weld Gun" }]);
  render(
    <ContextMenu
      client={fakeClient({ entityActionsFor: () => Promise.resolve(ACTIONS) })}
      ids={["e1"]}
      candidates={[under("e1", 25.7, true), under("e2", 28.2)]}
      onClose={vi.fn()}
    />,
  );

  const rows = await screen.findAllByTestId("ctxitem");
  // Focus opens on the first row a person can USE — the first AVAILABLE verb, which is now two rows
  // further down the ring than it used to be. An off-by-`candidates.length` here would put the ring's
  // origin on a candidate while `tabIndex=0` sat on a verb.
  const removeRow = rows.find((r) => r.dataset.action === "remove")!;
  await waitFor(() => expect(document.activeElement).toBe(removeRow));
  expect(removeRow.getAttribute("tabindex")).toBe("0");

  const menu = screen.getByRole("menu");
  fireEvent.keyDown(menu, { key: "Home" });
  // Home is the top of the WHOLE menu, which is now the nearest object under the pointer.
  expect(document.activeElement).toBe(screen.getAllByTestId("ctxcandidate")[0]);
  fireEvent.keyDown(menu, { key: "End" });
  expect((document.activeElement as HTMLElement).dataset.action).toBe("selectsimilar");
});

test("a ray through a dense assembly is CAPPED, and says how many it did not show", async () => {
  const many = Array.from({ length: 23 }, (_, i) => under(`e${i}`, 10 + i));
  loadScene(many.map((c, i) => ({ id: c.id, name: `Bolt ${i}` })));
  render(
    <ContextMenu
      client={fakeClient({ entityActionsFor: () => Promise.resolve(answer(0, [])) })}
      ids={[]}
      candidates={many}
      onClose={vi.fn()}
    />,
  );

  const rows = await screen.findAllByTestId("ctxcandidate");
  // A menu is edge-aware but not scrollable: 23 rows is a menu taller than the window with its verbs
  // off the bottom of the screen.
  expect(rows).toHaveLength(8);
  // NOT A SILENT CAP. A truncation nobody is told about reads as "that is everything" — and the line
  // hands the rest to the gesture that can still reach them, rather than stranding them.
  const more = screen.getByTestId("ctxmenu-under-more");
  expect(more.textContent).toContain("15 more behind");
  expect(more.textContent).toContain("Alt-click");
  // The ordinals count positions in the WHOLE stack, not in the shown window — the same numbers
  // alt-click reports, or the two surfaces would disagree about where you are.
  expect(rows[7].textContent).toContain("8 of 23");
});
