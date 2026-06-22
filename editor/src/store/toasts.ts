//! The **toast** store (M10.10) — transient, auto-dismissing confirmations shown AT THE GESTURE (over
//! the stage), not exclusively in the 16px footer that overwrites itself (C11 / C5). Any action posts a
//! toast ("Created HealthBar", "−2 tokens · 98 left", "Saved to my-project.mtk") that the user sees where
//! the action happened. Separate from the status line (`store/ui`) and the projection store (invariant 1):
//! toasts are ephemeral chrome. Ids are a monotonic counter (no `Math.random` → stable across test runs);
//! the auto-dismiss timer lives in the host component (the store stays pure + synchronously testable).

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

export type ToastKind = "info" | "cost" | "success" | "error";

export interface Toast {
  id: number;
  text: string;
  kind: ToastKind;
}

interface ToastState {
  toasts: Toast[];
  push(text: string, kind: ToastKind): number;
  dismiss(id: number): void;
  reset(): void;
}

/** Auto-dismiss after this many ms (kept short so a confirmation never lingers or goes stale). */
export const TOAST_TTL_MS = 4000;

let seq = 0;

export const toastStore = createStore<ToastState>((set, get) => ({
  toasts: [],
  push: (text, kind) => {
    seq += 1;
    const id = seq;
    set({ toasts: [...get().toasts, { id, text, kind }] });
    return id;
  },
  dismiss: (id) => set({ toasts: get().toasts.filter((t) => t.id !== id) }),
  reset: () => set({ toasts: [] }),
}));

/** Post a transient toast (any component, on any action). Returns its id. */
export const pushToast = (text: string, kind: ToastKind = "info"): number =>
  toastStore.getState().push(text, kind);

/** Subscribe to the live toasts. */
export const useToasts = (): Toast[] => useStore(toastStore, (s) => s.toasts);
