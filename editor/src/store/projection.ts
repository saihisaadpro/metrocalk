//! The projection store — the UI's read-model cache (invariant 1). Zustand as the thin
//! selector-backed wrapper over `useSyncExternalStore`, so a delta re-renders only the components
//! whose slice changed.
//!
//! Three keyed maps, each updated **immutably per-entity** so reference equality changes only for
//! touched entities:
//! - `base`      — the authoritative projection (what the core has confirmed).
//! - `displayed` — `base ⊕ pending` (optimistic overlay); detail components read this.
//! - `summaries` — `{id,name,parentId}` only; the list/hierarchy reads this so a field edit (which
//!   changes `displayed[id]` but not `summaries[id]`) never re-renders the tree.
//!
//! `displayed[id]` is kept **identical by reference to `base[id]` when no pending op touches the
//! entity**, so an edit to entity X cannot re-render entity Y's subscribers.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import { projectStore } from "./project";
import type {
  EditIntent,
  EntityProjection,
  EntitySummary,
  Json,
  ProjectionDelta,
  RejectInfo,
} from "../transport/protocol";

export interface BindEdge {
  id: string; // `${from}|${rel}|${to}`
  from: string;
  rel: string;
  to: string;
  status: "confirmed" | "pending" | "rejected";
}

interface PendingOp {
  clientOpId: string;
  intent: EditIntent;
}

export interface ProjectionState {
  base: Record<string, EntityProjection>;
  displayed: Record<string, EntityProjection>;
  summaries: Record<string, EntitySummary>;
  order: string[];
  edges: Record<string, BindEdge>;
  pending: Record<string, PendingOp>;
  rejections: RejectInfo[];
  selectedId: string | null;
  /** M10.6 multi-selection (always includes `selectedId` as the primary/anchor). Verbs that act on a
   *  selection (group, multi-edit, multi-delete) read this; the inspector/reveal read `selectedId`. */
  multiSelect: string[];

  applyDelta(delta: ProjectionDelta): void;
  optimisticEdit(op: PendingOp): void;
  select(id: string | null): void;
  /** Ctrl/Cmd-click — toggle `id` in/out of the multi-selection (the primary becomes `id`). */
  toggleSelect(id: string): void;
  /** Shift-click — extend the selection from the current primary to `id` over the visible `order`. */
  selectRange(id: string): void;
  dismissRejection(clientOpId: string): void;
  bulkLoad(entities: EntityProjection[]): void;
  reset(): void;
}

const edgeId = (from: string, rel: string, to: string) => `${from}|${rel}|${to}`;

/** Apply the `setField` pending ops touching `id` to `base[id]`; returns `base[id]` UNCHANGED (same
 *  ref) when none touch it — the key to selective subscription. */
function deriveDisplayed(
  baseEnt: EntityProjection | undefined,
  pending: Record<string, PendingOp>,
  id: string,
): EntityProjection | undefined {
  if (!baseEnt) return undefined;
  const ops = Object.values(pending).filter(
    (p) => p.intent.kind === "setField" && p.intent.id === id,
  );
  if (ops.length === 0) return baseEnt; // ref-identical → no re-render
  const components: EntityProjection["components"] = {};
  for (const [c, fields] of Object.entries(baseEnt.components)) components[c] = { ...fields };
  for (const p of ops) {
    if (p.intent.kind !== "setField") continue;
    (components[p.intent.component] ??= {})[p.intent.field] = p.intent.value;
  }
  return { ...baseEnt, components };
}

export const projectionStore = createStore<ProjectionState>((set, get) => ({
  base: {},
  displayed: {},
  summaries: {},
  order: [],
  edges: {},
  pending: {},
  rejections: [],
  selectedId: null,
  multiSelect: [],

  bulkLoad(entities) {
    const base: Record<string, EntityProjection> = {};
    const summaries: Record<string, EntitySummary> = {};
    const order: string[] = [];
    for (const e of entities) {
      base[e.id] = e;
      summaries[e.id] = { id: e.id, name: e.name, parentId: e.parentId };
      order.push(e.id);
    }
    set({ base, displayed: { ...base }, summaries, order, edges: {}, pending: {}, rejections: [] });
  },

  optimisticEdit(op) {
    // An edit means unsaved changes — light the File menu's "•" instantly (ADR-033). Authoritative
    // dirtiness is the shell's (refreshed on menu open); this is the optimistic indicator.
    projectStore.getState().markDirty();
    const s = get();
    const pending = { ...s.pending, [op.clientOpId]: op };
    if (op.intent.kind === "setField") {
      const id = op.intent.id;
      const displayed = { ...s.displayed, [id]: deriveDisplayed(s.base[id], pending, id)! };
      set({ pending, displayed });
    } else if (op.intent.kind === "bind") {
      const { from, rel, to } = op.intent;
      const id = edgeId(from, rel, to);
      const edges = { ...s.edges, [id]: { id, from, rel, to, status: "pending" as const } };
      set({ pending, edges });
    } else {
      set({ pending });
    }
  },

  applyDelta(delta) {
    const s = get();
    // A server-initiated FULL re-projection (`project_full`: connect/undo/sim-restart/open) REPLACES the
    // scene — start from EMPTY so stale entities/edges (e.g. an undone bind's edge, or an undone-create's
    // entity) can't linger (invariant 1: the store mirrors the authoritative scene). An incremental delta
    // MERGES onto the current maps. Either way each map is copied ONCE per delta, then mutated in place —
    // O(n + ops), not O(n × ops); per-entity refs change only for touched entities (selective subscription).
    const full = delta.full === true;
    const base = full ? {} : { ...s.base };
    const summaries = full ? {} : { ...s.summaries };
    const edges = full ? {} : { ...s.edges };
    let order = full ? [] : s.order;
    let orderCopied = full;
    const orderMut = (): string[] => {
      if (!orderCopied) {
        order = [...order];
        orderCopied = true;
      }
      return order;
    };
    const touched = new Set<string>();

    for (const op of delta.ops) {
      switch (op.op) {
        case "upsert": {
          const id = op.id;
          const prev = base[id];
          const next: EntityProjection = {
            id,
            name: op.name ?? prev?.name ?? id,
            parentId: op.parentId !== undefined ? op.parentId : (prev?.parentId ?? null),
            components: prev?.components ?? {},
          };
          base[id] = next;
          // summaries change only if name/parent changed (so field edits never touch the tree)
          const sm = summaries[id];
          if (!sm || sm.name !== next.name || sm.parentId !== next.parentId) {
            summaries[id] = { id, name: next.name, parentId: next.parentId };
          }
          if (!prev) orderMut().push(id);
          touched.add(id);
          break;
        }
        case "remove": {
          if (base[op.id]) {
            delete base[op.id];
            delete summaries[op.id];
            order = orderMut().filter((x) => x !== op.id);
            touched.add(op.id);
          }
          break;
        }
        case "setField": {
          const prev = base[op.id];
          if (!prev) break;
          const components = { ...prev.components, [op.component]: { ...prev.components[op.component], [op.field]: op.value } };
          base[op.id] = { ...prev, components };
          touched.add(op.id);
          break;
        }
        case "removeField": {
          const prev = base[op.id];
          if (!prev?.components[op.component]) break;
          const comp = { ...prev.components[op.component] };
          delete comp[op.field];
          base[op.id] = { ...prev, components: { ...prev.components, [op.component]: comp } };
          touched.add(op.id);
          break;
        }
        case "addEdge": {
          const id = edgeId(op.from, op.rel, op.to);
          edges[id] = { id, from: op.from, rel: op.rel, to: op.to, status: "confirmed" };
          break;
        }
        case "removeEdge": {
          delete edges[edgeId(op.from, op.rel, op.to)];
          break;
        }
      }
    }

    // Drop confirmed optimistic ops (their authoritative form is now in `base`/`edges`). A full
    // re-projection supersedes ALL pending optimistic ops (the authoritative scene is now complete).
    const pending = full ? {} : { ...s.pending };
    if (delta.confirms?.length) {
      for (const cid of delta.confirms) delete pending[cid];
    }

    // Rejections: revert the optimistic effect + surface the reason ("every 'no' explained").
    let rejections = s.rejections;
    if (delta.rejects?.length) {
      for (const r of delta.rejects) {
        const op = pending[r.clientOpId];
        if (op?.intent.kind === "bind") {
          delete edges[edgeId(op.intent.from, op.intent.rel, op.intent.to)];
        }
        if (op?.intent.kind === "setField") touched.add(op.intent.id);
        delete pending[r.clientOpId];
      }
      rejections = [...delta.rejects, ...rejections].slice(0, 50);
    }

    // Recompute `displayed` only for entities whose base or pending changed (on a full re-projection,
    // start from empty so dropped entities don't survive in the overlay).
    let displayed = full ? {} : s.displayed;
    if (touched.size) {
      displayed = { ...displayed };
      for (const id of touched) {
        const d = deriveDisplayed(base[id], pending, id);
        if (d === undefined) delete displayed[id];
        else displayed[id] = d;
      }
    }

    set({ base, displayed, summaries, order, edges, pending, rejections });
  },

  select(id) {
    set({ selectedId: id, multiSelect: id ? [id] : [] });
  },

  toggleSelect(id) {
    const s = get();
    const has = s.multiSelect.includes(id);
    const multiSelect = has ? s.multiSelect.filter((x) => x !== id) : [...s.multiSelect, id];
    // The primary follows: `id` when adding, else the last remaining (or null when the set empties).
    set({ multiSelect, selectedId: has ? (multiSelect[multiSelect.length - 1] ?? null) : id });
  },

  selectRange(id) {
    const s = get();
    const anchor = s.selectedId;
    const ai = anchor ? s.order.indexOf(anchor) : -1;
    const bi = s.order.indexOf(id);
    if (ai < 0 || bi < 0) {
      set({ selectedId: id, multiSelect: [id] });
      return;
    }
    const [lo, hi] = ai <= bi ? [ai, bi] : [bi, ai];
    set({ multiSelect: s.order.slice(lo, hi + 1), selectedId: id });
  },

  dismissRejection(clientOpId) {
    set({ rejections: get().rejections.filter((r) => r.clientOpId !== clientOpId) });
  },

  reset() {
    set({ base: {}, displayed: {}, summaries: {}, order: [], edges: {}, pending: {}, rejections: [], selectedId: null, multiSelect: [] });
  },
}));

// ── selector hooks (selective subscription) ─────────────────────────────────────

export const useDisplayedEntity = (id: string): EntityProjection | undefined =>
  useStore(projectionStore, (s) => s.displayed[id]);

export const useSummary = (id: string): EntitySummary | undefined =>
  useStore(projectionStore, (s) => s.summaries[id]);

export const useEntityOrder = (): string[] => useStore(projectionStore, (s) => s.order);

export const useSelectedId = (): string | null => useStore(projectionStore, (s) => s.selectedId);

export const useMultiSelect = (): string[] => useStore(projectionStore, (s) => s.multiSelect);

/** Per-row membership selector (a boolean) — so a hierarchy row re-renders only when ITS selection
 *  membership changes, not on every selection change (selective re-render at 5k, principle 3). */
export const useIsMultiSelected = (id: string): boolean =>
  useStore(projectionStore, (s) => s.multiSelect.includes(id));

export const useRejections = (): RejectInfo[] => useStore(projectionStore, (s) => s.rejections);

export const useFieldValue = (id: string, component: string, field: string): Json | undefined =>
  useStore(projectionStore, (s) => s.displayed[id]?.components[component]?.[field]);

export const useEdges = (): Record<string, BindEdge> => useStore(projectionStore, (s) => s.edges);
