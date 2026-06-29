//! The transient UI/status store — the "last action" message the status bar shows (the scaffold's
//! bottom-left `#status`). Separate from the projection store (which holds authoritative read-model
//! state, invariant 1): status is ephemeral chrome, not projected core state. Rejections (the
//! "every 'no' explained" toasts) live in the projection store and are surfaced by the Rejections panel.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

interface UiState {
  status: string;
  setStatus(s: string): void;
  /** Whether a subtree has been Copy/Cut into the clipboard this session — so Paste can be gated
   *  (no enabled-inert CTA, C5). Set true by Copy/Cut; the shell clipboard persists for the session. */
  clipboardHasContent: boolean;
  setClipboard(has: boolean): void;
}

export const uiStore = createStore<UiState>((set) => ({
  status: "",
  setStatus: (status) => set({ status }),
  clipboardHasContent: false,
  setClipboard: (clipboardHasContent) => set({ clipboardHasContent }),
}));

/** Set the transient status line (any component, on any action). */
export const setStatus = (s: string): void => uiStore.getState().setStatus(s);

/** Subscribe to the status line. */
export const useStatus = (): string => useStore(uiStore, (s) => s.status);

/** Mark the clipboard populated (Copy/Cut) so Paste becomes enabled. */
export const setClipboard = (has: boolean): void => uiStore.getState().setClipboard(has);

/** Subscribe to whether the clipboard has content (gates the Paste verb). */
export const useClipboardHasContent = (): boolean => useStore(uiStore, (s) => s.clipboardHasContent);
