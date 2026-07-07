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

/** A per-entity **relational summary** (M14.2 / ADR-058) — the live relational/binding/requirer truth keyed
 *  off the REAL `/core` `(Provides/Requires, cap)` pairs + `bindings()` (the C6 closure). Rides the `upsert`
 *  op + the `EntitySummary` (NOT the components map), so a field edit never re-renders a hierarchy row (M2.5)
 *  and a row re-renders only when its relational status flips. A read/render projection — never authored. */
export interface RelSummary {
  /** Capability names this entity REQUIRES (display names, e.g. `["Health"]`). Non-empty ⇒ a requirer. */
  requires: string[];
  /** Capability names this entity PROVIDES (e.g. `["Health"]`). */
  provides: string[];
  /** Count of this entity's outgoing bindings (BindsTo edges). */
  bound: number;
  /** A required capability is not yet satisfied by an existing binding ("needs a binding"). The authoritative
   *  requirer signal — replaces the brittle `HealthBar`-component-name filter (C6). */
  needsBinding: boolean;
  /** This entity is a group/identity parent node (children grouped under it). */
  isGroup: boolean;
}

/** A `{id,name,parentId}` summary — the list/hierarchy reads this so detail edits never re-render the tree.
 *  M14.2 adds the type `kind` (for the type-icon, derived server-side from the salient component so the row
 *  needs no component subscription) + the `rel`ational summary (the live binding/requirer truth). */
export interface EntitySummary {
  id: string;
  name: string;
  parentId: string | null;
  /** Salient type for the type-icon/thumbnail fallback (`mesh`/`group`/`light`/`camera`/`requirer`/…). */
  kind?: string;
  rel?: RelSummary;
}

export type ProjectionOp =
  | { op: "upsert"; id: string; name?: string; parentId?: string | null; active?: boolean; kind?: string; rel?: RelSummary }
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

/** One row of the M15.7 (ADR-077) import report — a CAD part + its honesty class (the persisted `CadPart`
 *  fidelity token, so this survives reload). Mirrors `CadReportPart`. */
export interface CadReportPart {
  id: string;
  name: string;
  fidelity: string; // "exact-brep" | "tessellation-only" | "ai-reconstructed" | "proxy" | "access-denied" | "failed"
}

/** The per-part CAD import report aggregated from the ECS — the fidelity breakdown (the header) + a capped
 *  part list (the queryable body). "Explain every no" applied to import; nothing silent. Mirrors
 *  `CadReportResp`. */
export interface CadReport {
  total: number;
  exactBrep: number;
  tessellationOnly: number;
  aiReconstructed: number;
  proxy: number;
  accessDenied: number;
  failed: number;
  parts: CadReportPart[];
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

// ── M12.4 AI compose (ADR-048) — a natural-language sentence → a validated Composition proposal → applied
// through the SAME one commit pipeline a human/plugin uses. The AI only proposes; the engine validates +
// commits (one undoable tx) or refuses. The shipped live path is the metrocalk-mcp server; this is the
// in-editor seam beside it. The proposal's `composition` is opaque JSON handed straight back to `compose`. ──

/** One allow-listed compose op (externally-tagged `op` — the SA-22 grammar's shape). The UI treats a
 *  composition as opaque (it previews via the op count + the explained reason) and passes it back verbatim. */
export type ComposeOp =
  | { op: "setField"; entity: string; component: string; field: string; value: FieldValue }
  | { op: "authorRule"; id: string; rule: RuleData }
  | { op: "authorStateMachine"; id: string; machine: StateMachine };

/** A composition the AI proposes — applied as ONE undoable transaction (or rejected whole). */
export interface Composition {
  ops: ComposeOp[];
}

/** `propose_composition` outcome: a reviewable `composition` (validated against the live scene) + its op
 *  count, or a plain-language `error` (offline, no selected target, an unrecognized sentence, or a proposal
 *  that fails validation). `ok` ⇒ safe to apply. Nothing is applied — review, then call `compose`. */
export interface ComposeProposal {
  ok: boolean;
  composition: Composition | null;
  ops: number;
  error: string | null;
}

/** `compose` outcome: how many ops `applied` (one undoable transaction) + the project's `rules` /
 *  `stateMachines` counts after, or a plain-language `error` if rejected-as-UX (nothing applied,
 *  all-or-nothing). The AI is never a raw mutation — this is the same validated path a human uses. */
export interface ComposeResult {
  ok: boolean;
  applied: number;
  rules: number;
  stateMachines: number;
  error: string | null;
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

// ── M12.5 Rules in Play + the live truth-state debugger (ADR-049) ────────────────────────────────────────
// The "debug by looking" payload: running Rules are a PROJECTION over a runtime state (never the Loro doc,
// ADR-021/034), so this mirrors `metrocalk_core::rule_runtime` serde. The structured fields (`satisfied` /
// `actual` / `expected`) are the stable assertion surface; `display` is the human overlay copy.

/** A scalar runtime value, as `metrocalk_core::FieldValue` serializes (`{ Integer: 4 }` / `{ Str: "x" }` /
 *  `{ Bool: true }` / `{ Number: 1.5 }`) — the same untagged-by-key shape `RuleData` values use. */
export type RuntimeValue =
  | { Integer: number }
  | { Number: number }
  | { Bool: boolean }
  | { Str: string };

/** One If-condition's live truth at the current frame — the ✅/❌ fact made visible. `satisfied` drives the
 *  check/cross; `display` is the human copy (e.g. `"KillCounter = 3 of 4"`). */
export interface ConditionTruth {
  satisfied: boolean;
  entity: string;
  component: string;
  field: string;
  actual: RuntimeValue | null;
  expected: RuntimeValue;
  display: string;
}

/** One rule's live truth for the clicked entity — does it fire now, and why/why not (per condition). */
export interface RuleTruth {
  rule: string;
  name: string;
  event: string;
  fires: boolean;
  conditions: ConditionTruth[];
}

/** A state machine's live current state for the clicked entity (e.g. `"state = FacingBoss"`). */
export interface MachineTruth {
  machine: string;
  name: string;
  field: string;
  current: string;
  display: string;
}

/** The full truth-state for one entity at the current frame — the debug projection the overlay renders. */
export interface TruthState {
  entity: string;
  rules: RuleTruth[];
  machines: MachineTruth[];
}

/** One frame-stamped decision-history entry — `kind` tags the consequence (`ruleFired` / `counterChanged` /
 *  `fieldSet` / `stateTransition` / `pluginInvoked`), the rest are that variant's fields. Time-travelable. */
export interface DecisionEvent {
  frame: number;
  kind: "ruleFired" | "counterChanged" | "fieldSet" | "stateTransition" | "pluginInvoked";
  // Variant fields (present per `kind`):
  rule?: string;
  name?: string;
  entity?: string;
  component?: string;
  field?: string;
  from?: RuntimeValue | string;
  to?: RuntimeValue | string;
  value?: RuntimeValue;
  machine?: string;
  plugin?: string;
}

/** A rule held OUT of the deterministic Play path (a `RunPlugin` to a non-deterministic plugin) — surfaced,
 *  never silently dropped. */
export interface FlaggedRule {
  rule: string;
  reason: string;
}

/** One rule's plain-language `explain_rule` narration at the current frame (why it did/didn't fire). */
export interface RuleExplain {
  rule: string;
  text: string;
}

/** The live truth-state debugger payload (`rule_debug` / `fire_rule_event` / `rule_scrub`): the clicked
 *  entity's `truth`, each rule's `explanations`, the frame-stamped `decisions` history (scrubbable over
 *  [0, `head`]), and the determinism-`flagged` rules. `playing:false` ⇒ not in Play (the rest is empty). */
export interface RuleDebugInfo {
  playing: boolean;
  frame: number;
  head: number;
  truth: TruthState | null;
  explanations: RuleExplain[];
  decisions: DecisionEvent[];
  flagged: FlaggedRule[];
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
