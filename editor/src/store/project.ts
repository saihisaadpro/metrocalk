//! The **project** store (M10.3, ADR-033) — the editor's current `.mtk` project: its path, whether it
//! has **unsaved changes** (the dirty flag the File menu's guard reads), and the **recent projects**
//! list. Separate from the projection store (authoritative scene read-model, invariant 1): this is the
//! document-level chrome around it.
//!
//! Dirtiness is **authoritative on the Rust side** (the engine thread sets it on every commit, clears it
//! on save) and refreshed here whenever the File menu opens via `client.projectState()`; `markDirty()`
//! gives an instant optimistic indicator on edit (bumped from the projection store's edit chokepoint) so
//! the "•" lights up without waiting for a round-trip.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

/** The document-level project state mirrored from the shell (`project_state` / save / open / new). */
export interface ProjectInfo {
  /** The current project's file path, or `null` for an unsaved "untitled" project. */
  path: string | null;
  /** Unsaved changes since the last save (the unsaved-changes guard's signal). */
  dirty: boolean;
  /** Recent project paths, most-recent first. */
  recents: string[];
  /** An explained error from the last file operation (open/save), surfaced to the user; `null` if none. */
  error?: string | null;
}

interface ProjectState extends ProjectInfo {
  /** Replace the mirrored state from a fresh `ProjectInfo` (a shell read or a file-op result). */
  refresh(info: ProjectInfo): void;
  /** Optimistically mark unsaved (an edit happened) — instant "•" before the next authoritative read. */
  markDirty(): void;
  /** Reset to the untitled/empty default (test hygiene + a brand-new session). */
  reset(): void;
}

const EMPTY: ProjectInfo = { path: null, dirty: false, recents: [], error: null };

export const projectStore = createStore<ProjectState>((set) => ({
  ...EMPTY,
  refresh: (info) =>
    set({
      path: info.path,
      dirty: info.dirty,
      recents: info.recents,
      error: info.error ?? null,
    }),
  markDirty: () => set({ dirty: true }),
  reset: () => set({ ...EMPTY }),
}));

/** A short, OS-agnostic display name for a project path (the file's stem), or "untitled". */
export function projectName(path: string | null): string {
  if (!path) return "untitled";
  const file = path.split(/[\\/]/).pop() ?? path;
  return file.replace(/\.mtk$/i, "");
}

/** Project state for the File menu. Each field is selected with its own stable selector (a primitive or
 *  the unchanged array ref), so the snapshot is cached per-field — assembling the plain return object is
 *  ordinary React, not a new store snapshot each render (which would loop `useSyncExternalStore`). */
export const useProjectInfo = (): ProjectInfo => ({
  path: useStore(projectStore, (s) => s.path),
  dirty: useStore(projectStore, (s) => s.dirty),
  recents: useStore(projectStore, (s) => s.recents),
  error: useStore(projectStore, (s) => s.error),
});
