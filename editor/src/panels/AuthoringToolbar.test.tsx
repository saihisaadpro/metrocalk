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
