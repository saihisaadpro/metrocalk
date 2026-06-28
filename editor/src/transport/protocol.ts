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
  /** `true` only for a server-initiated **full re-projection** (`project_full`: connect/undo/sim-restart/
   *  open). The store REPLACES (drops stale entities/edges) instead of merging — so an undone bind's edge,
   *  or a deleted entity, can't linger. Default/absent = an incremental delta (merged). */
  full?: boolean;
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

/** `generate(query)` — the tier-3, opt-in generation result (M6 / ADR-017): the grey placeholder that
 *  dropped in instantly (`created`) + the metered `cost`, or — when generation is unavailable or the
 *  wallet refuses — `available: false` / a `seam`. The real mesh streams in later over the projection
 *  Channel (the ADR-017 patch). `balance` is the wallet after reserving the generation, for the wallet UI. */
export interface GenerateResponse {
  created: string | null;
  cost: number | null;
  available: boolean;
  seam: string | null;
  balance: number | null;
}

/** The opt-in generation cost shown in the UI (mirrors the shell's `GENERATE_TOKENS`). The real debit is
 *  authoritative on the shell; this is only the legible estimate the Generate button shows up-front. */
export const GENERATE_COST = 10;

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

// ── M8 physics surface (the React PhysicsPanel; mirrors the Rust serde shapes in editor-shell) ──────────

/** `sim_timeline` / `sim_scrub` — returned to JS as a POSITIONAL 5-tuple, not an object:
 *  `[frame, maxFrame, running, overlaysOn, bodies]`. */
export type TimelineTuple = [number, number, boolean, boolean, number];

/** `import_interchange(format, source)` — the URDF/USD import outcome (snake_case serde). */
export interface ImportResult {
  ok: boolean;
  format: string;
  bodies: number;
  joints: number;
  meters_per_unit: number;
  kilograms_per_unit: number;
  reconciled: boolean;
  notes: string[];
  error: string | null;
}

/** `physics_contacts()` — one EXPLAINED contact row ("debug by looking"): `explain` names the penetration
 *  + normal impulse; `depth`/`friction_saturated` are the structured fields (snake_case serde). */
export interface ContactInfo {
  explain: string;
  depth: number;
  friction_saturated: boolean;
}

/** `physics_check(id)` — a collider-intelligence warning (camelCase serde): each is EXPLAINED (`message`)
 *  and carries a one-click fix (`fixLabel` + `fixAction` the shell maps back through `physics_fix`). */
export interface PhysicsWarning {
  issue: "no-collider" | "concave-dynamic" | "bad-scale" | "bad-mass";
  message: string;
  fixLabel: string;
  fixAction: string;
}

// ── M9 transform surface (the React TransformPanel) ─────────────────────────────────────────────────────

/** `snap_query(id, radius)` — a ranked snap candidate, each with an explained `why` (camelCase serde). */
export interface SnapHit {
  id: string;
  kind: string;
  x: number;
  y: number;
  z: number;
  distance: number;
  why: string;
}

/** `apply_constraint` / `placement_sentence` — solve-or-explain: `ok` + the compiled `intents`, or a
 *  `reason` when refused (every "no" explained, camelCase serde). */
export interface SolveResult {
  ok: boolean;
  reason: string | null;
  intents: string[];
}

// ── M12.1 Rules layer (When/If/Then) — the registry-fed builder + Rule list (ADR-045) ───────────────────
// These mirror the Rust serde shapes exactly (`metrocalk_core::rules`): `FieldValue` is externally tagged
// (`{ Integer: 4 }`), `CompareOp` is snake_case, so a `RuleData` round-trips JS → core → JS unchanged.

/** A typed literal in a condition/action — externally tagged to match `metrocalk_core::FieldValue`. */
export type FieldValue =
  | { Integer: number }
  | { Number: number }
  | { Bool: boolean }
  | { Str: string };

/** The comparison operator in an If-condition (matches `CompareOp`'s snake_case serde). */
export type CompareOp = "eq" | "ne" | "lt" | "le" | "gt" | "ge";

/** One If-condition: `<entity>.<component>.<field> <op> <value>` — every part registry-typed. */
export interface RuleCondition {
  entity: string;
  component: string;
  field: string;
  op: CompareOp;
  value: FieldValue;
}

/** One Then-action: a registry verb over `<entity>.<component>.<field> = <value>` (closed vocabulary). */
export interface RuleAction {
  action: string;
  entity: string;
  component: string;
  field: string;
  value: FieldValue;
}

/** A whole rule — the shape `author_rule` takes + the mirror it returns. */
export interface RuleData {
  name: string;
  enabled: boolean;
  event: string;
  conditions: RuleCondition[];
  actions: RuleAction[];
}

/** A row in the editor Rule list (`list_rules`, camelCase serde). */
export interface RuleSummary {
  id: string;
  name: string;
  enabled: boolean;
  event: string;
  conditionCount: number;
  actionCount: number;
}

/** One dropdown entry — a registry event or action verb (name + plain-language description). */
export interface RuleVocabItem {
  name: string;
  description: string;
}

/** A component the If/Then can target, with its fields' scalar types (so the value input is type-matched). */
export interface RuleComponentVocab {
  name: string;
  fields: { name: string; ty: "integer" | "number" | "boolean" | "string" }[];
}

/** The whole registry-fed vocabulary (`rule_registry`) the builder's dropdowns are assembled from. */
export interface RuleRegistryInfo {
  events: RuleVocabItem[];
  actions: RuleVocabItem[];
  components: RuleComponentVocab[];
}

/** `author_rule` outcome: the new `id`, a plain-language `error` if the registry Blocked it (ADR-016), and
 *  the proactively-offered `mirror` "cleanup" rule (the missing-"off"-switch guard) — `null` if none. */
export interface AuthorRuleResult {
  id: string | null;
  error: string | null;
  mirror: RuleData | null;
}

// ── M12.2 state machines (states + transitions = Rules) — the visual state-graph (ADR-046) ───────────────
// A transition IS a `RuleData` (the reuse, not a parallel model). The machine is data on the Loro doc; the
// state-graph reuses the M2.5 React Flow layer. Mirrors `metrocalk_core::state_machine` serde (plain field
// names) — so a `StateMachine` round-trips JS → core → JS unchanged.

/** One transition — a graph EDGE: from one state to another, guarded by an M12.1 `RuleData` (When/If) whose
 *  single action enters `to`. `id` is the stable React Flow edge id (server-allocated; the e2e keys off it). */
export interface Transition {
  id: string;
  from: string;
  to: string;
  rule: RuleData;
}

/** A whole state machine — the shape `author_state_machine` takes + `state_machines` returns. States are the
 *  graph NODES (`states`, ids = the names); transitions are the EDGES. */
export interface StateMachine {
  name: string;
  entity: string;
  component: string;
  field: string;
  states: string[];
  initial: string;
  transitions: Transition[];
}

/** One state machine for the state-graph view (`state_machines`, camelCase serde): the full machine (so the
 *  React Flow graph can render nodes + edges), its `id`, and the live `current` state (the M12.5 seam,
 *  defaults to `initial`). */
export interface StateMachineInfo {
  id: string;
  current: string;
  machine: StateMachine;
}

/** `author_state_machine` outcome: the new `id`, a plain-language `error` if Blocked (ADR-016: no name /
 *  dangling transition / a typo'd transition Rule / not-a-state-change), and the `unreachable` states — a
 *  warning surfaced (explained), never a rejection. */
export interface AuthorStateMachineResult {
  id: string | null;
  error: string | null;
  unreachable: string[];
}

const te = new TextEncoder();
const td = new TextDecoder();
export const encodeJson = (v: unknown): Uint8Array => te.encode(JSON.stringify(v));
export const decodeJson = <T>(b: Uint8Array): T => JSON.parse(td.decode(b)) as T;
