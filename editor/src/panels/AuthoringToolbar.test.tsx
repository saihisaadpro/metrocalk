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

test("Delete acts on the WHOLE selection, in one transaction, and dims exactly what went", async () => {
  // The trigger has always counted the selection — `Actions · 3` — while `Delete` called the
  // single-id command on the primary and left the other two in the scene, reporting "deactivated".
  const deleteDeactivateMany = vi.fn((ids: string[]) => Promise.resolve(ids.slice(0, 2)));
  const deleteDeactivate = vi.fn(() => Promise.resolve(true));
  act(() => {
    projectionStore.getState().bulkLoad([
      { id: "a", name: "Bolt", parentId: null, components: {} },
      { id: "b", name: "Nut", parentId: null, components: {} },
      { id: "c", name: "Washer", parentId: null, components: {} },
    ]);
    projectionStore.getState().setSelection(["a", "b", "c"]);
  });
  render(<AuthoringToolbar client={fakeClient({ deleteDeactivateMany, deleteDeactivate })} />);

  expect(screen.getByRole("button", { name: /^actions/i }).textContent).toContain("· 3");
  fireEvent.click(screen.getByRole("button", { name: /^actions/i }));
  await act(async () => {
    fireEvent.click(screen.getByTestId("authDelete"));
  });

  expect(deleteDeactivate).not.toHaveBeenCalled();
  expect(deleteDeactivateMany).toHaveBeenCalledWith(["a", "b", "c"]);
  // The ENGINE says which ids went. Dimming the list we sent would badge a row that is still there
  // the moment one id is stale — which is the whole reason the command returns ids and not a bool.
  await waitFor(() => expect(projectionStore.getState().deactivated).toEqual({ a: true, b: true }));
  expect(projectionStore.getState().multiSelect).toEqual([]);
  expect(uiStore.getState().status).toContain("2 objects");
});

test("a verb that only acts on one says which one, rather than counting three beside it", async () => {
  act(() => {
    projectionStore.getState().bulkLoad([
      { id: "a", name: "Bolt", parentId: null, components: {} },
      { id: "b", name: "Nut", parentId: null, components: {} },
    ]);
    projectionStore.getState().setSelection(["a", "b"]);
  });
  render(<AuthoringToolbar client={fakeClient()} />);
  fireEvent.click(screen.getByRole("button", { name: /^actions/i }));

  // `<ux_quality>` 4 + 6: an enabled control that quietly narrows its own scope is the same defect
  // Delete had; naming the object is the honest version until duplicate is batched too.
  expect(screen.getByTestId("authDuplicate").textContent).toContain("Nut");
  expect(screen.getByTestId("authDuplicate").textContent).toContain("the other 1 are left alone");
  // …and the refusal that used to name a gesture the stage did not have now names one it does.
  expect(screen.getByTestId("authGroup").getAttribute("aria-disabled")).not.toBe("true");
});
