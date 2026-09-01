//! The transient UI/status store — the "last action" message the status bar shows (the scaffold's
//! bottom-left `#status`). Separate from the projection store (which holds authoritative read-model
//! state, invariant 1): status is ephemeral chrome, not projected core state. Rejections (the
//! "every 'no' explained" toasts) live in the projection store and are surfaced by the Rejections panel.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

/** What is on the clipboard, as the editor needs to *say* it (ADR-198).
 *
 *  It was one boolean — enough to enable a Paste row and nothing else, so the row read "Paste" over a
 *  clipboard holding fourteen objects and over one holding a bolt. A verb that cannot name what it is
 *  about has no information scent (`<ux_quality>` 4), and paste is the one verb whose subject is not
 *  on screen anywhere: the selection has moved on since the copy. */
export interface ClipboardInfo {
  /** How many objects a paste would produce. `0` is an empty clipboard. */
  objects: number;
  /** How many entities it would create in total — bigger than `objects` when anything has children. */
  parts: number;
  /** Whether what is held was CUT. A cut pastes back in place; a copy pastes beside its source. */
  cut: boolean;
  /** What to call it — the single object's name, or "14 objects". Captured at copy time, because by
   *  paste time the sources may be deselected, cut away, or in another project. */
  label: string;
}

const EMPTY_CLIPBOARD: ClipboardInfo = { objects: 0, parts: 0, cut: false, label: "" };

interface UiState {
  status: string;
  setStatus(s: string): void;
  /** What Copy/Cut put on the clipboard this session — so Paste can be gated (no enabled-inert CTA,
   *  C5) *and* can say what it would paste. The shell clipboard persists for the session. */
  clipboard: ClipboardInfo;
  setClipboard(info: ClipboardInfo): void;
}

export const uiStore = createStore<UiState>((set) => ({
  status: "",
  setStatus: (status) => set({ status }),
  clipboard: EMPTY_CLIPBOARD,
  setClipboard: (clipboard) => set({ clipboard }),
}));

/** Set the transient status line (any component, on any action). */
export const setStatus = (s: string): void => uiStore.getState().setStatus(s);

/** Subscribe to the status line. */
export const useStatus = (): string => useStore(uiStore, (s) => s.status);

/** Record what Copy/Cut just put on the clipboard, so Paste can be enabled AND described. */
export const setClipboard = (info: ClipboardInfo): void => uiStore.getState().setClipboard(info);

/** Subscribe to what is on the clipboard. */
export const useClipboard = (): ClipboardInfo => useStore(uiStore, (s) => s.clipboard);

/** Subscribe to whether the clipboard has content (gates the Paste verb). */
export const useClipboardHasContent = (): boolean => useStore(uiStore, (s) => s.clipboard.objects > 0);
