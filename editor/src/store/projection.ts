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

  applyDelta(delta: ProjectionDelta): void;
  optimisticEdit(op: PendingOp): void;
  select(id: string | null): void;
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
    // Copy each map ONCE per delta, then mutate the copies in place — O(n + ops), not O(n × ops). A
    // bulk initial-load delta of 5k entities through per-op spreads would be O(n²). Per-entity refs
    // still change only for touched entities (new entity objects), so selective subscription holds.
    const base = { ...s.base };
    const summaries = { ...s.summaries };
    const edges = { ...s.edges };
    let order = s.order;
    let orderCopied = false;
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

    // Drop confirmed optimistic ops (their authoritative form is now in `base`/`edges`).
    const pending = { ...s.pending };
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

    // Recompute `displayed` only for entities whose base or pending changed.
    let displayed = s.displayed;
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
    set({ selectedId: id });
  },

  dismissRejection(clientOpId) {
    set({ rejections: get().rejections.filter((r) => r.clientOpId !== clientOpId) });
  },

  reset() {
    set({ base: {}, displayed: {}, summaries: {}, order: [], edges: {}, pending: {}, rejections: [], selectedId: null });
  },
}));

// ── selector hooks (selective subscription) ─────────────────────────────────────

export const useDisplayedEntity = (id: string): EntityProjection | undefined =>
  useStore(projectionStore, (s) => s.displayed[id]);

export const useSummary = (id: string): EntitySummary | undefined =>
  useStore(projectionStore, (s) => s.summaries[id]);

export const useEntityOrder = (): string[] => useStore(projectionStore, (s) => s.order);

export const useSelectedId = (): string | null => useStore(projectionStore, (s) => s.selectedId);

export const useRejections = (): RejectInfo[] => useStore(projectionStore, (s) => s.rejections);

export const useFieldValue = (id: string, component: string, field: string): Json | undefined =>
  useStore(projectionStore, (s) => s.displayed[id]?.components[component]?.[field]);

export const useEdges = (): Record<string, BindEdge> => useStore(projectionStore, (s) => s.edges);
