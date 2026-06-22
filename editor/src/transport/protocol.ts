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

// ── shell query-command results (M10.1) — mirror the Rust serde shapes in editor-shell/src-tauri ───────

/** A ranked compatible target the selection can bind to (north-star #1). */
export interface Candidate {
  id: string;
  name: string;
  distance: number;
  affinity: number;
}
/** An incompatible target, greyed WITH the registry-derived reason ("every 'no' explained"). */
export interface Greyed {
  id: string;
  name: string;
  reason: string;
}
/** An existing outgoing binding of the selection ("tracking …" after a bind / reload). */
export interface Bound {
  id: string;
  name: string;
  kind: string;
}
/** `reveal_targets(id)` — the ranked reveal for a selected entity. */
export interface RevealResponse {
  required: string[];
  compatible: Candidate[];
  greyed: Greyed[];
  bound: Bound[];
}
/** `describe(query)` — the tiered describe-to-create result (local → marketplace → generate seam). */
export interface DescribeResponse {
  created: string | null;
  kind: string | null;
  source: string | null;
  price: number | null;
  seam: string | null;
  balance: number | null;
}

/** `wallet_info` / `top_up` / `ai_edit` — the token-economy result (M7): the balance after, the
 *  charge/grant if any, and a refusal/seam `message` when `!ok` (refuse-when-broke, explained). */
export interface EconResponse {
  ok: boolean;
  balance: number;
  cost: number | null;
  message: string | null;
}

/** `entity_actions(id)` — one context-menu action's availability (M3.3). `action` is the lowercase tag
 *  ("bind"|"remove"|"duplicate"|"focus"|"inspect"|"makedynamic"); when `!available`, `reason` explains why
 *  (every "no" explained, ADR-016); `mutates` ⇒ it's an undoable transaction. */
export interface ActionItem {
  action: string;
  label: string;
  available: boolean;
  reason?: string;
  mutates: boolean;
}

/** `entity_details(id)` — the hover tooltip read (M3.3): name + component names + provided/required caps
 *  + the entities it's bound to. */
export interface EntityDetails {
  id: string;
  name: string;
  components: string[];
  provides: string[];
  requires: string[];
  boundTo: string[];
}

/** A catalog entry (M3.4 / ADR-019) — the ONE catalog surface the asset browser reuses (registry +
 *  marketplace + imported), mirroring `metrocalk_core::catalog::CatalogItem`. */
export interface CatalogItem {
  id: string;
  label: string;
  bucket: string;
  category: string;
  source: string; // "local" | "marketplace" | …
  provides: string[];
  requires: string[];
  asset?: string;
  price?: number;
  score?: number;
}

/** `catalog_search(query)` — ranked matches over the one catalog + the no-match seam. */
export interface CatalogSearch {
  items: CatalogItem[];
  seam?: string;
}

/** `add_item(id, source)` — instantiate a catalog item into the scene (place-into-scene): the created
 *  entity id, the balance after (marketplace buy), or the seam. */
export interface AddResponse {
  created: string | null;
  balance: number | null;
  seam: string | null;
}

const te = new TextEncoder();
const td = new TextDecoder();
export const encodeJson = (v: unknown): Uint8Array => te.encode(JSON.stringify(v));
export const decodeJson = <T>(b: Uint8Array): T => JSON.parse(td.decode(b)) as T;
