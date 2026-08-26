//! **The asset shelf** (ADR-144) — the two collections the catalog cannot supply: what this user marked
//! as a favourite, and what they last placed. Both are per-machine preferences, so they live beside the
//! disclosure and shell-layout preferences in `localStorage` and never enter the document (invariant 1:
//! the ECS is authoritative; this is not scene state and must never be mistaken for it).
//!
//! WHY A STORE AND NOT TWO `useState`s IN THE PANEL. `recent` is written by the ACT of placing, which is
//! the browser's own click today and could be the command palette's or a drag-drop's tomorrow, while
//! `favourites` is read by the browser and by anything that later offers a picker. A module both sides
//! can import is the difference between one list and two that disagree.
//!
//! A KEY IS `source:id`, NOT `id`. `metrocalk_core::catalog` draws ids from two namespaces at once — a
//! stdlib kind name (`HealthBar`) and a marketplace entry id (`forge:rusty-sword`) — and nothing forbids
//! a future marketplace entry called `HealthBar`. The catalog's own React keys are already `source:id`
//! for that reason; the shelf uses the same compound so a favourite can never follow a collision.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

const STORAGE_PREFIX = "metrocalk:asset-shelf:v1:";
/** How many placements are remembered. Enough to re-reach today's working set, short enough that the
 *  collection stays a shortcut rather than a second catalog. */
export const RECENT_LIMIT = 8;

/** The stable identity of one catalog item across its two id namespaces. */
export const shelfKey = (source: string, id: string): string => `${source}:${id}`;

interface ShelfState {
  favourites: string[];
  recent: string[];
  toggleFavourite(key: string): void;
  recordPlacement(key: string): void;
  reset(): void;
}

/** Parse one persisted list. Exported because it is the only part of the shelf that can fail: storage
 *  may hold something another version wrote, or something no version wrote, and neither may be the
 *  reason the panel does not mount. The I/O around it is not interesting; this is. */
export function parseShelfList(raw: string | null): string[] {
  if (!raw) return [];
  try {
    const parsed: unknown = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((v): v is string => typeof v === "string") : [];
  } catch {
    return [];
  }
}

/** Read a persisted list. Storage itself may be unavailable (private mode, a locked-down webview). */
function load(name: string): string[] {
  if (typeof window === "undefined") return [];
  try {
    return parseShelfList(window.localStorage.getItem(`${STORAGE_PREFIX}${name}`));
  } catch {
    return [];
  }
}

function save(name: string, value: string[]): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(`${STORAGE_PREFIX}${name}`, JSON.stringify(value));
  } catch {
    // A preference that cannot be persisted still works for this session.
  }
}

export const assetShelfStore = createStore<ShelfState>((set, get) => ({
  favourites: load("favourites"),
  recent: load("recent"),
  toggleFavourite: (key) => {
    const on = get().favourites.includes(key);
    const favourites = on ? get().favourites.filter((k) => k !== key) : [...get().favourites, key];
    save("favourites", favourites);
    set({ favourites });
  },
  recordPlacement: (key) => {
    // Most-recent first, deduplicated: placing the same item twice moves it up rather than filling the
    // shortcut with one name.
    const recent = [key, ...get().recent.filter((k) => k !== key)].slice(0, RECENT_LIMIT);
    save("recent", recent);
    set({ recent });
  },
  reset: () => {
    save("favourites", []);
    save("recent", []);
    set({ favourites: [], recent: [] });
  },
}));

/** Star / un-star one catalog item. */
export const toggleFavourite = (source: string, id: string): void =>
  assetShelfStore.getState().toggleFavourite(shelfKey(source, id));

/** Remember that this catalog item was just placed. Called by whatever performed the placement. */
export const recordPlacement = (source: string, id: string): void =>
  assetShelfStore.getState().recordPlacement(shelfKey(source, id));

/** Subscribe to the starred keys. */
export const useFavourites = (): string[] => useStore(assetShelfStore, (s) => s.favourites);

/** Subscribe to the recently placed keys, most recent first. */
export const useRecent = (): string[] => useStore(assetShelfStore, (s) => s.recent);
