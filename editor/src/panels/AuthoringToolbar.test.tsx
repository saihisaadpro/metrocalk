import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import { fakeClient } from "../transport/test-client";
import { AuthoringToolbar } from "./AuthoringToolbar";

afterEach(() => {
  act(() => {
    projectionStore.getState().reset();
    uiStore.getState().setStatus("");
    uiStore.getState().setClipboard(false);
  });
});

test("keeps the hierarchy quiet and reveals stable creation commands through a keyboard menu", () => {
  projectionStore.getState().bulkLoad([
    { id: "one", name: "One", parentId: null, components: { Transform: { x: 0, y: 0, z: 0 } } },
  ]);
  projectionStore.getState().select("one");
  render(<AuthoringToolbar client={fakeClient()} />);

  const toolbar = screen.getByRole("toolbar", { name: "Scene actions" });
  const add = screen.getByRole("button", { name: /add/i });
  const actions = screen.getByRole("button", { name: /actions/i });
  expect(toolbar.querySelectorAll("button")).toHaveLength(2);
  expect(add.id).toBe("authAdd");
  expect(actions.id).toBe("authMore");
  expect(document.getElementById("authCreate")).toBeNull();

  fireEvent.click(add);
  const menu = screen.getByRole("menu", { name: "Add scene object" });
  expect(add.getAttribute("aria-expanded")).toBe("true");
  expect(document.getElementById("authCreate")).toBeTruthy();
  expect(document.getElementById("authLight")).toBeTruthy();
  expect(document.activeElement).toBe(screen.getByTestId("authCreate"));

  fireEvent.keyDown(menu, { key: "ArrowDown" });
  expect(document.activeElement).toBe(screen.getByTestId("authLight"));
  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu", { name: "Add scene object" })).toBeNull();
  expect(add.getAttribute("aria-expanded")).toBe("false");
  expect(document.activeElement).toBe(add);
});

test("groups every selection and clipboard command with visible unavailable reasons", () => {
  render(<AuthoringToolbar client={fakeClient()} />);

  const actions = screen.getByRole("button", { name: /^actions/i });
  fireEvent.click(actions);
  expect(screen.getByRole("menu", { name: "Selection and clipboard actions" })).toBeTruthy();
  for (const id of [
    "authDuplicate",
    "authDelete",
    "authGroup",
    "authUngroup",
    "authNudge",
    "authCopy",
    "authCut",
    "authPaste",
  ]) {
    expect(document.getElementById(id)).toBeTruthy();
  }
  expect(document.getElementById("auth-selection-heading")).toBeTruthy();
  expect(document.getElementById("auth-clipboard-heading")).toBeTruthy();
  expect(screen.getByTestId("authDuplicate").getAttribute("aria-disabled")).toBe("true");
  expect(screen.getAllByText("Select an object to duplicate")).toHaveLength(1);
  expect(screen.getByTestId("authPaste").getAttribute("aria-disabled")).toBe("true");
  expect(screen.getAllByText("Copy or cut something first")).toHaveLength(1);

  fireEvent.keyDown(window, { key: "Escape" });
  expect(screen.queryByRole("menu", { name: "Selection and clipboard actions" })).toBeNull();
  expect(document.activeElement).toBe(actions);
});

test("shows immediate progress, prevents duplicate authoring commands, and restores controls on completion", async () => {
  let finishCreate: (id: string | null) => void = () => {};
  const createEntity = vi.fn(
    () =>
      new Promise<string | null>((resolve) => {
        finishCreate = resolve;
      }),
  );
  render(<AuthoringToolbar client={fakeClient({ createEntity })} />);

  const add = screen.getByRole("button", { name: /add/i });
  fireEvent.click(add);
  const create = screen.getByTestId("authCreate");
  const light = screen.getByTestId("authLight");
  fireEvent.click(create);
  fireEvent.click(create);

  expect(createEntity).toHaveBeenCalledTimes(1);
  expect(create.getAttribute("aria-busy")).toBe("true");
  expect(create.textContent).toContain("Creating entity…");
  expect(create.getAttribute("aria-disabled")).toBe("true");
  expect(light.getAttribute("aria-disabled")).toBe("true");
  expect(uiStore.getState().status).toBe("Creating entity…");

  await act(async () => finishCreate("created-1"));
  await waitFor(() => expect(screen.queryByRole("menu", { name: "Add scene object" })).toBeNull());
  expect(document.activeElement).toBe(add);
  expect(uiStore.getState().status).toBe("created an entity · Ctrl-Z to undo");

  fireEvent.click(add);
  expect(screen.getByTestId("authCreate").getAttribute("aria-disabled")).toBeNull();
  expect(screen.getByTestId("authLight").getAttribute("aria-disabled")).toBeNull();
});

// ── ADR-169 — THE TRIGGER COUNTED THE SELECTION AND THE VERBS DID ONE OF IT ────────────────────────
// The Actions trigger has read "Actions · 12" off `ids.length` since M10.6, and Duplicate and Delete
// both closed over `primary`. So the control named twelve objects, the tooltip said "the selection",
// and one object was cloned or deactivated — with a toast promising a Ctrl-Z that would have needed
// twelve if the loop had simply been written the obvious way.

/** Three objects, all selected, the third one primary — the state the menu already counted. */
function threeSelected(): void {
  act(() => {
    projectionStore.getState().bulkLoad([
      { id: "a", name: "A", parentId: null, components: { Transform: { x: 0, y: 0, z: 0 } } },
      { id: "b", name: "B", parentId: null, components: { Transform: { x: 1, y: 0, z: 0 } } },
      { id: "c", name: "C", parentId: null, components: { Transform: { x: 2, y: 0, z: 0 } } },
    ]);
    projectionStore.getState().select("a");
    projectionStore.getState().toggleSelect("b");
    projectionStore.getState().toggleSelect("c");
  });
}

test("Delete acts on the WHOLE selection, in one transaction, and says how many", async () => {
  threeSelected();
  const client = fakeClient();
  render(<AuthoringToolbar client={client} />);
  fireEvent.click(screen.getByRole("button", { name: /^actions/i }));

  const del = screen.getByTestId("authDelete");
  expect(del.textContent).toContain("Delete 3");
  fireEvent.click(del);

  await waitFor(() => expect(client.deleteDeactivateMany).toHaveBeenCalledTimes(1));
  expect([...vi.mocked(client.deleteDeactivateMany).mock.calls[0][0]].sort()).toEqual(["a", "b", "c"]);
  // The single-entity command is NOT how a selection is deleted: three calls would be three undo steps.
  expect(client.deleteDeactivate).not.toHaveBeenCalled();
  await waitFor(() => expect(uiStore.getState().status).toContain("deactivated 3"));
  // Every one of them is dimmed in the hierarchy, not just the primary.
  for (const id of ["a", "b", "c"]) {
    expect(projectionStore.getState().deactivated[id]).toBe(true);
  }
});

test("Duplicate clones the WHOLE selection, in one transaction, and selects a clone", async () => {
  threeSelected();
  const client = fakeClient();
  vi.mocked(client.duplicateEntities).mockResolvedValue(["a2", "b2", "c2"]);
  render(<AuthoringToolbar client={client} />);
  fireEvent.click(screen.getByRole("button", { name: /^actions/i }));

  const dup = screen.getByTestId("authDuplicate");
  expect(dup.textContent).toContain("Duplicate 3");
  fireEvent.click(dup);

  await waitFor(() => expect(client.duplicateEntities).toHaveBeenCalledTimes(1));
  expect([...vi.mocked(client.duplicateEntities).mock.calls[0][0]].sort()).toEqual(["a", "b", "c"]);
  expect(client.duplicateEntity).not.toHaveBeenCalled();
  await waitFor(() => expect(uiStore.getState().status).toContain("duplicated 3"));
});

test("with one object selected both verbs read and behave exactly as they did", async () => {
  act(() => {
    projectionStore.getState().bulkLoad([
      { id: "a", name: "A", parentId: null, components: { Transform: { x: 0, y: 0, z: 0 } } },
    ]);
    projectionStore.getState().select("a");
  });
  const client = fakeClient();
  render(<AuthoringToolbar client={client} />);
  fireEvent.click(screen.getByRole("button", { name: /^actions/i }));
  // No count in the label when there is nothing to count — a "Delete 1" is noise, not information.
  expect(screen.getByTestId("authDelete").textContent).toContain("Delete");
  expect(screen.getByTestId("authDelete").textContent).not.toContain("Delete 1");
  fireEvent.click(screen.getByTestId("authDelete"));
  await waitFor(() => expect(client.deleteDeactivateMany).toHaveBeenCalledWith(["a"]));
  await waitFor(() => expect(uiStore.getState().status).toContain("deactivated —"));
});

test("a refused batch says the engine's reason, not a generic sentence", async () => {
  threeSelected();
  const client = fakeClient();
  vi.mocked(client.multiEdit).mockResolvedValue({
    ok: false,
    changed: 0,
    reason: "1 of 3 selected objects have no Transform",
  });
  render(<AuthoringToolbar client={client} />);
  fireEvent.click(screen.getByRole("button", { name: /^actions/i }));
  fireEvent.click(screen.getByTestId("authNudge"));
  await waitFor(() =>
    expect(uiStore.getState().status).toBe("1 of 3 selected objects have no Transform"),
  );
});
