//! File menu (M10.3 / ADR-033) — verified headless in jsdom: the menu opens + refreshes the
//! authoritative project state; New/Open/Save/Save As dispatch the right `EditorClient` verbs; the
//! **unsaved-changes guard** asks before New/Open when the project is dirty (Cancel aborts, Discard
//! proceeds) but never guards Save; the recent list renders + opens by path; the "•" reflects dirty.
//!
//! (The native Open/Save dialogs + the live engine-swap-on-open are the shell's job — exercised on a
//! local GUI run, not here; this verifies the menu's logic against the client contract.)

import { afterEach, expect, test, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { FileMenu } from "./FileMenu";
import { projectStore } from "../store/project";
import { projectionStore } from "../store/projection";
import { uiStore } from "../store/ui";
import { toastStore } from "../store/toasts";
import { fakeClient } from "../transport/test-client";
import type { ProjectInfo } from "../store/project";

afterEach(() => {
  projectStore.getState().reset();
  projectionStore.getState().reset();
  uiStore.getState().setStatus("");
  toastStore.getState().reset();
});

const info = (over: Partial<ProjectInfo> = {}): ProjectInfo => ({
  path: null,
  dirty: false,
  recents: [],
  error: null,
  ...over,
});

test("opening the menu refreshes project state and shows the actions", async () => {
  const projectState = vi.fn(() => Promise.resolve(info({ path: "a.mtk" })));
  render(<FileMenu client={fakeClient({ projectState })} />);

  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(projectState).toHaveBeenCalled());
  expect(screen.getByTestId("fileNew")).toBeTruthy();
  expect(screen.getByTestId("fileOpen")).toBeTruthy();
  expect(screen.getByTestId("fileSave")).toBeTruthy();
  expect(screen.getByTestId("fileSaveAs")).toBeTruthy();
});

test("New on a clean project runs immediately (no guard)", async () => {
  const newProject = vi.fn(() => Promise.resolve(info()));
  render(<FileMenu client={fakeClient({ projectState: () => Promise.resolve(info({ dirty: false })), newProject })} />);

  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(screen.getByTestId("fileNew")).toBeTruthy());
  fireEvent.click(screen.getByTestId("fileNew"));

  await waitFor(() => expect(newProject).toHaveBeenCalledTimes(1));
  expect(screen.queryByTestId("unsavedGuard")).toBeNull();
});

test("New on a DIRTY project guards: Cancel aborts, Discard proceeds", async () => {
  const newProject = vi.fn(() => Promise.resolve(info()));
  const client = fakeClient({ projectState: () => Promise.resolve(info({ dirty: true })), newProject });
  render(<FileMenu client={client} />);

  // Open → refresh marks the store dirty.
  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(projectStore.getState().dirty).toBe(true));
  expect(screen.getByTestId("projectDirty")).toBeTruthy(); // the "•" indicator

  // New → the guard appears; New is NOT yet called.
  fireEvent.click(screen.getByTestId("fileNew"));
  expect(screen.getByTestId("unsavedGuard")).toBeTruthy();
  expect(newProject).not.toHaveBeenCalled();

  // Cancel → aborts, nothing happens.
  fireEvent.click(screen.getByTestId("guardCancel"));
  expect(screen.queryByTestId("unsavedGuard")).toBeNull();
  expect(newProject).not.toHaveBeenCalled();

  // New again → guard → Discard → New runs.
  fireEvent.click(screen.getByTestId("fileNew"));
  fireEvent.click(screen.getByTestId("guardDiscard"));
  await waitFor(() => expect(newProject).toHaveBeenCalledTimes(1));
});

test("Save on a TITLED project is never guarded (no data loss) and dispatches save_project", async () => {
  const saveProject = vi.fn(() => Promise.resolve(info({ path: "a.mtk" })));
  render(<FileMenu client={fakeClient({ projectState: () => Promise.resolve(info({ path: "a.mtk", dirty: true })), saveProject })} />);

  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(projectStore.getState().dirty).toBe(true));
  fireEvent.click(screen.getByTestId("fileSave"));

  await waitFor(() => expect(saveProject).toHaveBeenCalledTimes(1));
  expect(screen.queryByTestId("unsavedGuard")).toBeNull(); // never guards a save
  await waitFor(() => expect(projectStore.getState().dirty).toBe(false)); // save cleared dirty
  expect(uiStore.getState().status).toContain("a.mtk"); // status names the file, not a bare "saved"
});

test("Save on an UNTITLED project routes to Save As (no 'saved' on an unnamed doc — C9); the title reflects the filename", async () => {
  const saveProject = vi.fn();
  const saveProjectAs = vi.fn(() => Promise.resolve(info({ path: "proj/my-project.mtk" })));
  render(
    <FileMenu
      client={fakeClient({ projectState: () => Promise.resolve(info({ path: null, dirty: true })), saveProject, saveProjectAs })}
    />,
  );

  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(projectStore.getState().dirty).toBe(true));
  fireEvent.click(screen.getByTestId("fileSave"));

  // untitled Save → Save As (the dialog assigns a name); plain Save is NOT used on an unnamed doc
  await waitFor(() => expect(saveProjectAs).toHaveBeenCalledTimes(1));
  expect(saveProject).not.toHaveBeenCalled();
  // the title now reflects the real filename, and the status names the file (never "saved" on "untitled")
  await waitFor(() => expect(projectStore.getState().path).toBe("proj/my-project.mtk"));
  expect(uiStore.getState().status).toContain("my-project.mtk");
});

test("Save As dispatches the always-dialog save", async () => {
  const saveProjectAs = vi.fn(() => Promise.resolve(info({ path: "b.mtk" })));
  render(<FileMenu client={fakeClient({ saveProjectAs })} />);
  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(screen.getByTestId("fileSaveAs")).toBeTruthy());
  fireEvent.click(screen.getByTestId("fileSaveAs"));
  await waitFor(() => expect(saveProjectAs).toHaveBeenCalledTimes(1));
});

test("recent projects render and open by path", async () => {
  const openProject = vi.fn((p?: string) => Promise.resolve(info({ path: p ?? null })));
  const client = fakeClient({
    projectState: () => Promise.resolve(info({ recents: ["proj/alpha.mtk", "proj/beta.mtk"], dirty: false })),
    openProject,
  });
  render(<FileMenu client={client} />);

  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(document.querySelectorAll(".fileRecentItem").length).toBe(2));

  const first = document.querySelector('.fileRecentItem[data-path="proj/alpha.mtk"]') as HTMLElement;
  expect(first.textContent).toBe("alpha"); // display name = file stem, no .mtk
  fireEvent.click(first);
  await waitFor(() => expect(openProject).toHaveBeenCalledWith("proj/alpha.mtk"));
});

test("opening a project clears a stale selection (modules stay connected to the new scene)", async () => {
  // A leftover selection from the old scene must not survive a project switch — the Inspector/Reveal/
  // Wallet read `selectedId`, and the new scene streams in fresh over the Channel.
  projectionStore.getState().bulkLoad([{ id: "old-1", name: "Old", parentId: null, components: {} }]);
  projectionStore.getState().select("old-1");
  expect(projectionStore.getState().selectedId).toBe("old-1");

  const client = fakeClient({
    projectState: () => Promise.resolve(info({ recents: ["x.mtk"], dirty: false })),
  });
  render(<FileMenu client={client} />);
  fireEvent.click(screen.getByTestId("fileMenu"));
  await waitFor(() => expect(document.querySelector(".fileRecentItem")).toBeTruthy());
  fireEvent.click(document.querySelector(".fileRecentItem") as HTMLElement);

  await waitFor(() => expect(projectionStore.getState().selectedId).toBeNull());
});
