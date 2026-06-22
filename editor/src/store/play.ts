//! The **Play-mode** store (M10.4, ADR-034) — is the editor RUNNING the scene (Play) vs authoring it
//! (Stop), and is the running sim frozen (Pause). The runtime state is **authoritative on the shell**
//! (the engine thread owns the deterministic sim); this mirrors it for the Play/Stop/Pause controls and
//! the Play-mode indicator, and lets edit affordances disable themselves while playing (the
//! edit↔play boundary, deliverable 4).

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

/** Play-mode state mirrored from the shell (`play`/`stop`/`pause`/`play_state`). */
export interface PlayInfo {
  /** The scene is running (Play entered, not yet Stopped). */
  playing: boolean;
  /** Running but the sim is frozen (Pause). Only meaningful when `playing`. */
  paused: boolean;
}

interface PlayState extends PlayInfo {
  refresh(info: PlayInfo): void;
  reset(): void;
}

export const playStore = createStore<PlayState>((set) => ({
  playing: false,
  paused: false,
  refresh: (info) => set({ playing: info.playing, paused: info.paused }),
  reset: () => set({ playing: false, paused: false }),
}));

/** Whether the editor is currently running the scene (Play mode) — edit affordances key off this. */
export const usePlaying = (): boolean => useStore(playStore, (s) => s.playing);
export const usePaused = (): boolean => useStore(playStore, (s) => s.paused);
