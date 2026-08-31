//! **Ctrl/Cmd-F reaches the box that searches the scene.**
//!
//! The outliner owns the search field; the shell owns the keyboard and the Engines rail. Neither can
//! do this alone, and neither should hold a ref into the other — so the request travels as state, the
//! way every other cross-panel signal in this editor does.
//!
//! A COUNTER rather than a boolean: pressing the chord twice in a row is two requests, and a boolean
//! that is already `true` is indistinguishable from one that was never set. The panel focuses on each
//! new value and never has to reset a flag someone else might be reading.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

interface FindState {
  /** Increments once per request. The outliner focuses (and selects) its search field on each change. */
  focusRequest: number;
  requestObjectSearch(): void;
}

export const findStore = createStore<FindState>((set, get) => ({
  focusRequest: 0,
  requestObjectSearch: () => set({ focusRequest: get().focusRequest + 1 }),
}));

/** Ask the outliner to take the keyboard for a search. The caller is responsible for making the panel
 *  visible first — the rail owns which workspace is open, and a focus into a `hidden` panel is a focus
 *  the user cannot see. */
export const requestObjectSearch = (): void => findStore.getState().requestObjectSearch();

/** Subscribe to the request counter. */
export const useObjectSearchRequest = (): number => useStore(findStore, (s) => s.focusRequest);
