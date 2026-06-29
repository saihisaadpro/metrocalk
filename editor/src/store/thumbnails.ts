//! The live-thumbnail policy store (M14.2 / ADR-058) — the headless half of the flagship. The real
//! pixels are rendered by the native wgpu renderer (a discrete off-frame RTT, the `thumbnail` command);
//! this store owns the *policy* the prompt's guardrails demand, all of it unit-testable without a GPU:
//!
//! - **Dirty-only, off the op-stream.** `ingestDelta` invalidates an entity's thumbnail ONLY when a
//!   *silhouette-affecting* op touches it (mesh · material · visible transform · visibility) — never on a
//!   Health/HealthBar/other field edit, and never on camera/orbit (those aren't entity state, so orbit
//!   fires **0** thumbnail requests → invariant 4 holds with thumbnails active).
//! - **Visible-only + budget cap (min-spec).** Only the rows the hierarchy reports `visible` request a
//!   thumbnail; a budget caps refreshes per interval (scaled down on the min-spec profile); over budget /
//!   offline / dev / a `null` from the renderer → the entry stays a styled type-icon fallback.
//! - **Determinism-safe.** A thumbnail is a presentation artifact — it never enters the op-stream/Loro doc.
//!
//! The 5000-row virtualized list therefore generates ≤ the ~30 visible rows' thumbnails, never 5000, and
//! editing one entity invalidates exactly that one entity's thumbnail.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import type { EditorClient } from "../transport/session";
import type { ProjectionDelta } from "../transport/protocol";

/** A cached thumbnail. `ready` ⇒ `url` is a real rendered data-URL; `fallback` ⇒ show the styled type-icon
 *  (the renderer returned null / over budget / dev/browser build). `data-thumb-status` keys off this — the
 *  structured signal a test asserts, never a styled string. */
export type ThumbStatus = "ready" | "fallback";
export interface ThumbEntry {
  url: string | null;
  status: ThumbStatus;
}

/** The budget cap — a min-spec gate (principle 3): at most `maxPerInterval` thumbnail renders per
 *  `intervalMs`, at `size` px square. The entry-level profile scales both the rate and the resolution down. */
export interface ThumbBudget {
  maxPerInterval: number;
  intervalMs: number;
  size: number;
}
export const DEFAULT_BUDGET: ThumbBudget = { maxPerInterval: 8, intervalMs: 250, size: 112 };
export const MINSPEC_BUDGET: ThumbBudget = { maxPerInterval: 4, intervalMs: 400, size: 72 };

/** A silhouette-affecting component: a change to its fields can change how the entity *looks* in the
 *  viewport, so its thumbnail must re-render. Everything else (Health, HealthBar, Counter, …) does not. */
const SILHOUETTE_COMPONENTS = new Set(["MeshRenderer", "Material", "Transform"]);

interface ThumbState {
  entries: Record<string, ThumbEntry>;
  /** ids whose cached thumbnail is stale (a silhouette edit landed) — re-rendered when next visible+budget. */
  dirty: Record<string, true>;
  /** ids with a request in flight (so a drain never double-fires the same entity). */
  inflight: Record<string, true>;
  /** the entity ids the hierarchy currently has on screen (the visible-only gate). */
  visible: string[];
  budget: ThumbBudget;
  /** the renderer seam — `EditorClient.thumbnail`. Null before connect / in pure-unit tests ⇒ all fallback. */
  client: EditorClient | null;
  /** budget window bookkeeping (a sliding [start, start+intervalMs) counter). */
  windowStart: number;
  windowCount: number;

  setClient(c: EditorClient | null): void;
  setMinSpec(on: boolean): void;
  /** Scan a committed delta and invalidate the thumbnails of entities whose silhouette state changed. */
  ingestDelta(delta: ProjectionDelta): void;
  /** Mark one entity's thumbnail stale (keeps the stale image visible until the fresh one lands — no flicker). */
  invalidate(id: string): void;
  /** The hierarchy reports its visible window; replaces the visible set and drains within budget. */
  setVisible(ids: string[], now?: number): void;
  /** Fire thumbnail renders for visible ids that are stale/never-seen, up to the remaining budget this
   *  window. Returns the ids actually fired (the test hook). `now` is injectable for deterministic tests. */
  drain(now?: number): string[];
  /** A render result arrived (a data-URL, or null ⇒ fall back to the icon). */
  receive(id: string, url: string | null): void;
  reset(): void;
}

const nowMs = (): number => (typeof performance !== "undefined" ? performance.now() : 0);

export const thumbnailStore = createStore<ThumbState>((set, get) => ({
  entries: {},
  dirty: {},
  inflight: {},
  visible: [],
  budget: DEFAULT_BUDGET,
  client: null,
  windowStart: 0,
  windowCount: 0,

  setClient(c) {
    set({ client: c });
  },

  setMinSpec(on) {
    set({ budget: on ? MINSPEC_BUDGET : DEFAULT_BUDGET });
  },

  ingestDelta(delta) {
    // A full re-projection replaced the scene → every cached thumbnail is suspect. Drop the caches and
    // mark the upserted entities dirty (they re-render only when visible + within budget — never 5000 at once).
    if (delta.full) {
      set({ entries: {}, inflight: {} });
    }
    const dirtyIds: string[] = [];
    const removed: string[] = [];
    for (const op of delta.ops) {
      switch (op.op) {
        case "upsert":
          // A new entity, a visibility (active) flip, or a re-projected one — its look may have changed.
          dirtyIds.push(op.id);
          break;
        case "remove":
          removed.push(op.id);
          break;
        case "setField":
          if (SILHOUETTE_COMPONENTS.has(op.component)) dirtyIds.push(op.id);
          break;
        // removeField/addEdge/removeEdge never change an entity's silhouette → no invalidation.
      }
    }
    if (!dirtyIds.length && !removed.length) return;
    const s = get();
    const entries = removed.length ? { ...s.entries } : s.entries;
    const dirty = { ...s.dirty };
    const inflight = removed.length ? { ...s.inflight } : s.inflight;
    for (const id of removed) {
      delete entries[id];
      delete dirty[id];
      delete inflight[id];
    }
    for (const id of dirtyIds) dirty[id] = true;
    set({ entries, dirty, inflight });
    get().drain();
  },

  invalidate(id) {
    set({ dirty: { ...get().dirty, [id]: true } });
    get().drain();
  },

  setVisible(ids, now) {
    set({ visible: ids });
    get().drain(now);
  },

  drain(now = nowMs()) {
    const s = get();
    if (!s.client) return []; // no renderer (dev/browser/pre-connect) → entries stay fallback (the icon)
    // Slide the budget window.
    let { windowStart, windowCount } = s;
    if (now - windowStart >= s.budget.intervalMs) {
      windowStart = now;
      windowCount = 0;
    }
    let remaining = s.budget.maxPerInterval - windowCount;
    if (remaining <= 0) {
      if (windowStart !== s.windowStart) set({ windowStart, windowCount });
      return [];
    }
    const fired: string[] = [];
    const inflight = { ...s.inflight };
    const dirty = { ...s.dirty };
    // Visible-only: walk ONLY the visible window (≤ ~30 rows), never the whole scene — the 5000-row list
    // can never generate 5000 thumbnails.
    for (const id of s.visible) {
      if (remaining <= 0) break;
      if (inflight[id]) continue;
      const stale = dirty[id] || s.entries[id] === undefined;
      if (!stale) continue;
      inflight[id] = true;
      delete dirty[id]; // cleared at fire; a later edit re-dirties → re-fires next drain
      remaining -= 1;
      windowCount += 1;
      fired.push(id);
      const client = s.client;
      const size = s.budget.size;
      void client
        .thumbnail(id, size)
        .then((url) => get().receive(id, url))
        .catch(() => get().receive(id, null));
    }
    set({ inflight, dirty, windowStart, windowCount });
    return fired;
  },

  receive(id, url) {
    const s = get();
    const inflight = { ...s.inflight };
    delete inflight[id];
    set({
      entries: { ...s.entries, [id]: { url: url ?? null, status: url ? "ready" : "fallback" } },
      inflight,
    });
  },

  reset() {
    set({ entries: {}, dirty: {}, inflight: {}, visible: [], windowStart: 0, windowCount: 0 });
  },
}));

// ── selector hooks ───────────────────────────────────────────────────────────────

/** The cached thumbnail for an entity (undefined until first requested). A row re-renders only when ITS
 *  own thumbnail entry changes (selective subscription — editing one entity refreshes only its thumbnail). */
export const useThumb = (id: string): ThumbEntry | undefined =>
  useStore(thumbnailStore, (s) => s.entries[id]);

// ── the production pump (drains the dirty backlog within budget; tests call `drain` directly instead) ─────

let pump: ReturnType<typeof setInterval> | null = null;

/** Start the background drain so an over-budget/backlogged thumbnail still refreshes without waiting on the
 *  next scroll. Idempotent; returns a stop fn. Never started in unit tests (they drive `drain` deterministically). */
export function startThumbnailPump(): () => void {
  if (pump) return stopThumbnailPump;
  pump = setInterval(() => thumbnailStore.getState().drain(), thumbnailStore.getState().budget.intervalMs);
  return stopThumbnailPump;
}

export function stopThumbnailPump(): void {
  if (pump) {
    clearInterval(pump);
    pump = null;
  }
}
