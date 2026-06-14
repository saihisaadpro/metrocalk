//! The UI-facing projection contract (invariant 1: the UI holds projections only, never a second
//! source of truth).
//!
//! INBOUND (`%LOR`): the WASM core decodes Loro update bytes into a [`ProjectionDelta`] — a list of
//! per-entity ops the projection store applies. The UI never parses Loro. OUTBOUND: the UI emits an
//! [`EditTx`] (JSON-Patch shaped) — the **same language the AI layer emits**, so human and AI edits
//! are one path. In this scaffold the `ProjectionDelta`/`EditTx` ARE the `%LOR` payloads (JSON);
//! production swaps the payload for Loro bytes that the core decodes into this exact `ProjectionDelta`.

export type Json = string | number | boolean | null | Json[] | { [k: string]: Json };

/** A projected entity — the UI's read-model. Components are `component → field → value`. */
export interface EntityProjection {
  id: string;
  name: string;
  parentId: string | null;
  components: Record<string, Record<string, Json>>;
}

/** A `{id,name,parentId}` summary — the list/hierarchy reads this so detail edits never re-render the tree. */
export interface EntitySummary {
  id: string;
  name: string;
  parentId: string | null;
}

export type ProjectionOp =
  | { op: "upsert"; id: string; name?: string; parentId?: string | null }
  | { op: "remove"; id: string }
  | { op: "setField"; id: string; component: string; field: string; value: Json }
  | { op: "removeField"; id: string; component: string; field: string }
  | { op: "addEdge"; from: string; rel: string; to: string }
  | { op: "removeEdge"; from: string; rel: string; to: string };

/** A merge-validation rejection — the north-star "every 'no' explained". */
export interface RejectInfo {
  clientOpId: string;
  reason: string;
}

/** A committed delta from the core: authoritative ops + which optimistic ops it confirms/rejects. */
export interface ProjectionDelta {
  ops: ProjectionOp[];
  confirms?: string[];
  rejects?: RejectInfo[];
}

/** RFC-6902 subset — the edit language shared with the AI layer. */
export type JsonPatch =
  | { op: "add"; path: string; value: Json }
  | { op: "replace"; path: string; value: Json }
  | { op: "remove"; path: string };

/** A structured echo of intent so the core can validate semantically (and the store can apply an
 *  optimistic effect) without re-parsing JSON-Patch paths. */
export type EditIntent =
  | { kind: "setField"; id: string; component: string; field: string; value: Json }
  | { kind: "bind"; from: string; rel: string; to: string };

export interface EditTx {
  clientOpId: string;
  label: string;
  patches: JsonPatch[];
  intent: EditIntent;
}

const te = new TextEncoder();
const td = new TextDecoder();
export const encodeJson = (v: unknown): Uint8Array => te.encode(JSON.stringify(v));
export const decodeJson = <T>(b: Uint8Array): T => JSON.parse(td.decode(b)) as T;
