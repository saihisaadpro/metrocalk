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

/** M15.9 (ADR-079) — a joint's state for the panel: type, the REAL axis + pivot, the honesty-labeled
 *  source rung ("manual" gizmo-authored · "inferred" · "urdf"), the DOF value + limits, and the timeline
 *  length. Mirrors `JointInfoResp`. */
export interface JointInfo {
  jointType: "revolute" | "prismatic" | string;
  axis: [number, number, number];
  pivot: [number, number, number];
  source: string;
  value: number;
  min: number;
  max: number;
  trackEnd: number;
  keys: number;
}

// -- Animation engine: deterministic authored timelines + self-aware asset diagnostics. --

export type AnimationInterpolation = "step" | "linear" | "cubic";
export type AnimationLoopPolicy = "once" | "loop" | "ping_pong";
export type AnimationCapabilityState = "available" | "missing" | "unsupported" | "decoded_only" | "not_present";
export type AnimationContext = "2d" | "3d" | "ui";
export type AnimationBindingState = "ready" | "preview_only" | "read_only" | "unsupported" | "invalid";
export type AnimationEditorKind = "scalar" | "toggle" | "text" | "vector" | "quaternion" | "color" | "sprite_frame" | "layout";

/** A property exposed by the authoritative component data, including an honest reason when it cannot
 * be keyed. Asset handles, identity fields and other source-of-truth references are intentionally not
 * presented as animatable values. */
export interface AnimationPropertyInfo {
  component: string;
  property: string;
  label: string;
  valueKind: string;
  value: Json;
  animatable: boolean;
  reason: string | null;
  context: AnimationContext;
  editorKind: AnimationEditorKind;
  bindingState: AnimationBindingState;
  bindingReason: string;
  /** Named sink/adaptor, or null when the property is inspectable but has no consumer. */
  runtimeSink: string | null;
}

export interface AnimationKeyInfo {
  id: string;
  tick: number;
  seconds: number;
  value: Json;
  inTangent: Json | null;
  outTangent: Json | null;
}

export interface AnimationTrackInfo {
  id: string;
  name: string;
  targetId: string;
  targetName: string;
  component: string;
  property: string;
  valueKind: string;
  interpolation: AnimationInterpolation;
  enabled: boolean;
  locked: boolean;
  context: AnimationContext;
  editorKind: AnimationEditorKind;
  bindingState: AnimationBindingState;
  bindingReason: string;
  runtimeSink: string | null;
  keys: AnimationKeyInfo[];
}

export interface AnimationKeyUpdateInfo {
  keyId: string;
  tick?: number;
  value?: Json;
  inTangent?: Json | null;
  outTangent?: Json | null;
}

/** A stable key identity is scoped by both its target track and key ID. Key IDs alone are not
 * sequence-global for compatibility tracks, so batch operations always carry the complete identity. */
export interface AnimationKeyDeleteInfo {
  targetId: string;
  trackId: string;
  keyId: string;
}

export interface AnimationMarkerInfo {
  id: string;
  ownerId: string;
  name: string;
  tick: number;
  seconds: number;
  color: [number, number, number, number] | null;
}

export interface AnimationEventInfo {
  id: string;
  ownerId: string;
  name: string;
  tick: number;
  seconds: number;
  payload: Json | null;
}

export interface AnimationContextReadiness {
  context: AnimationContext;
  state: AnimationBindingState;
  properties: number;
  tracks: number;
  reason: string;
  action: string | null;
}

export interface AnimationCapabilityFact {
  capability: string;
  state: AnimationCapabilityState;
  reason: string;
  action: string | null;
}

export type AnimationImportedClipReadiness =
  | "setup_available"
  | "ready"
  | "repair_required"
  | "explicit_mapping_required"
  | "blocked";

/** One immutable imported clip as presented for explicit scene binding. Raw source identities never
 * become graph sources; only a `ready` persisted instance may be used by playback. */
export interface AnimationImportedClipInfo {
  clipId: string;
  sequenceId: string;
  name: string;
  durationTick: number;
  sourceBindingHash: string;
  sourceTargets: string[];
  /** Stable importer identities parallel to the human-readable sourceTargets labels. */
  sourceTargetIds?: string[];
  /** Typed source rows used by the explicit mapper; labels never substitute for binding identity. */
  sourceBindings?: AnimationImportedSourceBindingInfo[];
  channels: string[];
  readiness: AnimationImportedClipReadiness;
  reason: string;
  action: string | null;
  instanceId: string | null;
  /** Last persisted source-to-scene bindings when a unique instance can be inspected or repaired. */
  targetMappings?: AnimationClipTargetMapping[];
  /** Bounded old-versus-current typed source evidence shown before an explicit repair save. */
  repairChanges?: string[];
  /** Every persisted instance corresponding to this source clip; never collapse ambiguous repairs. */
  instances?: AnimationImportedClipInstanceInfo[];
}

export interface AnimationImportedClipInstanceInfo {
  instanceId: string;
  name: string;
  readiness: "ready" | "repair_required";
  reason: string;
  targetMappings: AnimationClipTargetMapping[];
  repairChanges: string[];
}

export interface AnimationImportedSourceBindingInfo {
  sourceTargetId: string;
  sourceTargetLabel: string;
  component: string;
  property: string;
  valueKind: AnimationValueKind;
}

export type AnimationValueKind =
  | "number"
  | "integer"
  | "bool"
  | "string"
  | "vec3"
  | "vec4"
  | "quaternion"
  | "weights";

export interface AnimationClipTargetMapping {
  sourceTargetId: string;
  targetId: string;
}

/** Derived, inspectable facts travel beside an imported or authored asset. These values describe what
 * the engine measured; they never claim skeletal playback, retargeting or root motion without evidence. */
export interface AnimationAssetProfile {
  displayName: string;
  source: string;
  provenance: string;
  qualityGrade: string;
  logicalId: string | null;
  revisionId: string | null;
  importState: string | null;
  dependencyCount: number;
  sourceLocation: string | null;
  watchesSource: boolean;
  reimportDiagnostics: number;
  skeletonJoints: number;
  clipCount: number;
  morphTargets: number;
  rootMotion: string;
  reimportBinding: string;
  clipInstanceRevision: string;
  clips: AnimationImportedClipInfo[];
  capabilities: AnimationCapabilityFact[];
  suggestions: string[];
}

export interface AnimationIssueInfo {
  severity: "info" | "warning" | "error" | string;
  code: string;
  message: string;
  fix: string | null;
}

/** One coherent animation read model. The timeline is authored data; the transport state is a
 * render-only projection and therefore cannot dirty the document. */
export interface AnimationWorkspaceInfo {
  /** Changes only when authored animation data changes; transport preview does not dirty it. */
  revision: string;
  sequenceId: string;
  sequenceName: string;
  ticksPerSecond: number;
  durationTick: number;
  currentTick: number;
  playing: boolean;
  loopPolicy: AnimationLoopPolicy;
  selectedId: string | null;
  selectedName: string | null;
  properties: AnimationPropertyInfo[];
  tracks: AnimationTrackInfo[];
  markers: AnimationMarkerInfo[];
  events: AnimationEventInfo[];
  contexts: AnimationContextReadiness[];
  asset: AnimationAssetProfile | null;
  issues: AnimationIssueInfo[];
}

export interface AnimationEditResult {
  ok: boolean;
  message: string;
  trackId: string | null;
  keyId: string | null;
  state: AnimationWorkspaceInfo;
}

export interface AnimationClipInstanceSaveRequest {
  expectedRevision: string;
  requestId: string;
  instanceId: string;
  name: string;
  logicalAssetId: string;
  expectedAssetRevision: string;
  clipId: string;
  expectedSourceBindingHash: string;
  /** Omitted for the backwards-compatible one-source→targetId shortcut. */
  targetMappings?: AnimationClipTargetMapping[];
  targetId: string;
}

export interface AnimationClipInstanceSaveResult {
  ok: boolean;
  message: string;
  instanceId: string | null;
  state: AnimationWorkspaceInfo;
}

export interface AnimationPlaybackInfo {
  ok: boolean;
  message: string;
  currentTick: number;
  durationTick: number;
  playing: boolean;
  /** Native truth for an unsaved imported-clip render projection. Older/mock transports may omit it. */
  importedClipPreviewActive?: boolean;
  loopPolicy: AnimationLoopPolicy;
  evaluatedTracks: number;
  crossedEvents: AnimationEventInfo[];
  /** True when the bounded native handoff omitted additional crossings. */
  eventsTruncated: boolean;
}

// -- Animation graph authoring, compilation, and bounded runtime inspection. --

/** Persisted graph documents are explicitly versioned. Readers must reject a newer schema instead of
 * guessing at node semantics; migrations can therefore remain deterministic and inspectable. */
export const ANIMATION_GRAPH_LEGACY_SCHEMA_VERSION = 1 as const;
export const ANIMATION_GRAPH_SCHEMA_VERSION = 2 as const;
export type AnimationGraphSchemaVersion = typeof ANIMATION_GRAPH_SCHEMA_VERSION;
export type AnimationGraphReadableSchemaVersion =
  | typeof ANIMATION_GRAPH_LEGACY_SCHEMA_VERSION
  | AnimationGraphSchemaVersion;

export type AnimationGraphParameterKind = "float" | "integer" | "boolean" | "trigger" | "vec2";
export type AnimationGraphValue = number | boolean | readonly [number, number];
export type AnimationGraphPortKind = "pose" | "number" | "boolean" | "trigger" | "mask" | "output";
export type AnimationGraphNodeKind =
  | "reference_pose"
  | "sequence"
  | "blend_normalized"
  | "blend_direct"
  | "blend_1d"
  | "blend_2d_cartesian"
  | "layer_override"
  | "layer_additive"
  | "state_machine"
  | "output";
export type AnimationGraphCompileState = "missing" | "compiling" | "ready" | "stale" | "invalid";
export type AnimationGraphReadinessState = "ready" | "warning" | "blocked";
export type AnimationGraphCurve = "linear" | "ease_in" | "ease_out" | "ease_in_out" | "smoothstep";
export type AnimationGraphInterruption = "none" | "source" | "destination" | "both";
export type AnimationGraphConditionOperator = "greater" | "greater_equal" | "less" | "less_equal" | "equal" | "not_equal" | "is_true" | "is_false" | "triggered";

export interface AnimationGraphPosition {
  x: number;
  y: number;
}

export interface AnimationGraphPortInfo {
  id: string;
  label: string;
  direction: "input" | "output";
  kind: AnimationGraphPortKind;
  required: boolean;
  multiple: boolean;
}

/** A durable blend sample. The stable sample and edge identities keep authored positions attached to
 * the intended input when collaborators add, remove, or reorder connections. */
export interface AnimationGraphBlendSample {
  id: string;
  edgeId: string;
  /** Blend1D consumes x and requires y=0; Cartesian Blend2D consumes both coordinates. */
  position: readonly [number, number];
}

/** One stable semantic node. Layout is authored for collaboration but is excluded from runtime hashes. */
export interface AnimationGraphNode {
  id: string;
  kind: AnimationGraphNodeKind;
  name: string;
  position: AnimationGraphPosition;
  enabled: boolean;
  /** Source logical identity for sequence nodes. Null for operators. */
  sourceId: string | null;
  /** Typed controls consumed by blend/layer nodes, in stable order. */
  parameterIds: string[];
  /** Stable-ID keyed Blend1D/Cartesian Blend2D samples. */
  samples: AnimationGraphBlendSample[];
  /** @deprecated Schema-v1 positional migration input. Canonical documents always store an empty list. */
  thresholds: number[];
  /** Authored stable triangulation for Cartesian Blend2D. Entries reference sample IDs. */
  triangles: Array<readonly [string, string, string]>;
  /** Exact property/bone binding patterns affected by a layer; empty means the complete bundle. */
  maskBindings: string[];
  /** Stable state-machine identity for composite state-machine nodes. */
  stateMachineId: string | null;
}

/** Canonical derived presentation returned by native compilation. It is not authored, is never directly
 * edited, and must be excluded from document/runtime hashes. Draft nodes use kind-derived local ports. */
export interface AnimationGraphNodePresentation {
  nodeId: string;
  ports: AnimationGraphPortInfo[];
  readiness: AnimationGraphReadinessState;
  readinessReason: string;
}

export interface AnimationGraphEdge {
  id: string;
  fromNodeId: string;
  fromPortId: string;
  toNodeId: string;
  toPortId: string;
  enabled: boolean;
  /** Optional explicit weight used when no parameter is bound. */
  weight: number | null;
  /** Stable parameter binding for this input; supersedes positional destination-node parameter lists. */
  weightParameterId: string | null;
}

export interface AnimationGraphParameter {
  id: string;
  name: string;
  kind: AnimationGraphParameterKind;
  defaultValue: AnimationGraphValue;
  min: number | null;
  max: number | null;
}

export interface AnimationGraphCondition {
  id: string;
  parameterId: string;
  operator: AnimationGraphConditionOperator;
  value: AnimationGraphValue | null;
}

export interface AnimationGraphState {
  id: string;
  name: string;
  /** The pose-producing graph node evaluated while this state is active. */
  poseNodeId: string;
  resetOnEntry: boolean;
}

export interface AnimationGraphTransition {
  id: string;
  fromStateId: string;
  toStateId: string;
  priority: number;
  durationTick: number;
  curve: AnimationGraphCurve;
  interruption: AnimationGraphInterruption;
  conditions: AnimationGraphCondition[];
  /** Normalized source time, or null when the transition is purely condition driven. */
  exitTime: number | null;
}

export interface AnimationGraphStateMachine {
  id: string;
  name: string;
  entryStateId: string;
  states: AnimationGraphState[];
  transitions: AnimationGraphTransition[];
}

export interface AnimationGraphDocument {
  schemaVersion: AnimationGraphSchemaVersion;
  id: string;
  sequenceId: string;
  name: string;
  outputNodeId: string;
  nodes: AnimationGraphNode[];
  edges: AnimationGraphEdge[];
  parameters: AnimationGraphParameter[];
  stateMachines: AnimationGraphStateMachine[];
}

export interface AnimationGraphSourceInfo {
  id: string;
  name: string;
  kind: "authored_sequence" | "imported_clip" | "clip_instance" | "reference_pose";
  logicalAssetId: string | null;
  revisionId: string | null;
  durationTick: number;
  readiness: AnimationGraphReadinessState;
  reason: string;
  action: string | null;
}

export interface AnimationGraphDiagnostic {
  id: string;
  severity: "info" | "warning" | "error";
  code: string;
  message: string;
  fix: string | null;
  nodeId: string | null;
  edgeId: string | null;
  portId: string | null;
}

export interface AnimationGraphCompileInfo {
  state: AnimationGraphCompileState;
  authoredRevision: string;
  compiledRevision: string | null;
  compiledHash: string | null;
  lastGoodRevision: string | null;
  lastGoodHash: string | null;
  message: string;
}

/** Sequence-global graph state. It must not change merely because the selected entity changes. */
export interface AnimationGraphStateInfo {
  schemaVersion: AnimationGraphSchemaVersion;
  sequenceId: string;
  revision: string;
  graph: AnimationGraphDocument | null;
  nodePresentation: AnimationGraphNodePresentation[];
  sources: AnimationGraphSourceInfo[];
  compile: AnimationGraphCompileInfo;
  diagnostics: AnimationGraphDiagnostic[];
}

export interface AnimationGraphSaveRequest {
  schemaVersion: AnimationGraphSchemaVersion;
  expectedRevision: string;
  requestId: string;
  /** The whole versioned draft is validated, then committed atomically as one undoable intent. */
  graph: AnimationGraphDocument;
}

export interface AnimationGraphSaveResult {
  ok: boolean;
  message: string;
  /** Present after both accept and reject so the editor can explain concurrent/stale writes. */
  state: AnimationGraphStateInfo;
}

export interface AnimationGraphDeleteResult {
  ok: boolean;
  message: string;
  state: AnimationGraphStateInfo;
}

export interface AnimationGraphActiveNode {
  nodeId: string;
  weight: number;
  localTick: number;
  stateId: string | null;
  costMicros: number | null;
}

export interface AnimationGraphActiveEdge {
  edgeId: string;
  weight: number;
}

export interface AnimationGraphTransitionDebug {
  transitionId: string;
  fromStateId: string;
  toStateId: string;
  elapsedTick: number;
  durationTick: number;
  progress: number;
}

export interface AnimationGraphWatchValue {
  id: string;
  value: Json;
  source: string;
}

/** Bounded runtime facts only; poses never cross IPC. Snapshots are fenced by graph revision/hash. */
export interface AnimationGraphDebugInfo {
  graphId: string;
  graphRevision: string;
  compiledHash: string;
  instanceId: string;
  rawTick: number;
  localTick: number;
  activeNodes: AnimationGraphActiveNode[];
  activeEdges: AnimationGraphActiveEdge[];
  transition: AnimationGraphTransitionDebug | null;
  parameterValues: Readonly<Record<string, AnimationGraphValue>>;
  watches: AnimationGraphWatchValue[];
  eventsTruncated: boolean;
  evaluationCostMicros: number | null;
  truncated: boolean;
}

export interface AnimationGraphPreviewResult {
  ok: boolean;
  message: string;
  accepted: Readonly<Record<string, AnimationGraphValue>>;
}

/** One row of the M15.7 (ADR-077) import report — a CAD part + its honesty class (the persisted `CadPart`
 *  fidelity token, so this survives reload). Mirrors `CadReportPart`. */
export interface CadReportPart {
  id: string;
  name: string;
  fidelity: string; // "exact-brep" | "tessellation-only" | "ai-reconstructed" | "proxy" | "access-denied" | "failed"
  reference?: string;
  strategy?: string;
  reason?: string;
  fix?: string;
  sourceFormat?: string;
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

/** M15.10 (ADR-080) — one per-part fate in the re-import diff. `kind`: "unchanged" | "moved" | "matched" |
 *  "adjudicate" | "removed" | "added". Mirrors `ReimportDiffRow`. */
export interface ReimportDiffRow {
  newEntity: string | null;
  name: string;
  kind: string;
  confidence: number;
  reason: string;
  hadOverrides: boolean;
}

/** A removed part whose overrides are preserved + flagged. Mirrors `ReimportOrphanRow`. */
export interface ReimportOrphanRow {
  oldId: string;
  name: string;
  material: string | null;
  hasJoint: boolean;
}

/** A held low-confidence match to confirm/reject (never auto-applied). Mirrors `ReimportAdjRow`. */
export interface ReimportAdjRow {
  oldId: string;
  newEntity: string;
  name: string;
  confidence: number;
  material: string | null;
  hasJoint: boolean;
}

/** The M15.10 re-import diff the editor renders: matched/added/removed/adjudicate + orphans + the held
 *  low-confidence matches. `isReimport=false` when the last import was a first import. Mirrors
 *  `ReimportReportResp`. */
export interface ReimportReport {
  isReimport: boolean;
  rebound: number;
  added: number;
  removed: number;
  adjudicate: number;
  rows: ReimportDiffRow[];
  orphans: ReimportOrphanRow[];
  pending: ReimportAdjRow[];
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

// ── Pipe Forge: deterministic procedural source → production asset, directly in the viewport. ──

export type PipeKit = "galvanized" | "copper" | "pvc" | "scifi";
export type PipeQuality = "preview" | "production" | "hero";

export interface PipeForgeOptions {
  kit: PipeKit;
  diameterCm: number;
  quality: PipeQuality;
  autoFittings: boolean;
}

export type PipeFittingKind = "elbow" | "coupling" | "tee" | "valve" | "flange";

/** Stable graph handle projected into the viewport. IDs survive bake, reopen and rebake. */
export interface PipeRouteHandle {
  nodeId: number;
  position: [number, number, number];
  connectedEdges: number[];
  fittingIds: number[];
}

/** One semantic centerline edge; branch runs may use a smaller per-edge diameter. */
export interface PipeRouteEdge {
  id: number;
  from: number;
  to: number;
  diameterM: number;
}

/** A fitting anchored to a stable route node rather than baked into anonymous geometry. */
export interface PipeFittingPlacement {
  id: number;
  nodeId: number;
  kind: PipeFittingKind;
  catalogId: string | null;
  automatic: boolean;
}

/** Project-local fitting metadata. The native compiler provides a safe proxy if the asset is unavailable. */
export interface UserFittingCatalogEntry {
  id: string;
  label: string;
  kind: PipeFittingKind;
  assetHandle: string | null;
  diameterScale: number;
  lengthScale: number;
}

/** Authoritative render-only tool state. It never dirties scene history until a successful bake. */
export interface PipeForgeStatus {
  active: boolean;
  /** Active recipe values are echoed by the engine so reconnects and polling never relabel a live run. */
  kit?: PipeKit;
  diameterCm?: number;
  quality?: PipeQuality;
  autoFittings?: boolean;
  points: number;
  lengthM: number;
  previewTriangles: number;
  canBake: boolean;
  message: string;
  handles: PipeRouteHandle[];
  edges: PipeRouteEdge[];
  fittings: PipeFittingPlacement[];
  fittingCatalog: UserFittingCatalogEntry[];
  /** The current branch tip while viewport clicks are extending a branch, otherwise null. */
  branchFrom: number | null;
  /** Entity whose persisted V2 recipe is being edited, otherwise null for a new route. */
  editingEntity: string | null;
}

/** Actual output facts from the native compiler; no aspirational/export-only values are reported. */
export interface PipeBakeReport {
  entityId: string | null;
  handle: string | null;
  vertices: number;
  triangles: number;
  lodTriangles: number[];
  textureResolution: number;
  collisionHulls: number;
  collisionKind: string;
  collisionTriangles: number;
  watertight: boolean;
  warnings: string[];
  message: string;
}

// -- Asset Lab: measured source inspection and source-preserving derived assets. --

export interface AssetLabAvailabilityFact {
  state: "available" | "missing" | "unsupported";
  reason: string;
}

export interface AssetLabAuditReport {
  name: string;
  logicalObjects: number;
  connectedComponents: number;
  primitives: number;
  nonEmptyPrimitives: number;
  vertices: number;
  uniquePositionVertices: number;
  duplicatePositionVertices: number;
  isolatedVertices: number;
  indices: number;
  trailingIndices: number;
  invalidIndices: number;
  triangles: number;
  validTriangles: number;
  degenerateTriangles: number;
  duplicateTriangles: number;
  edges: number;
  boundaryEdges: number;
  manifoldEdges: number;
  nonManifoldEdges: number;
  invalidPositions: number;
  drawCalls: number;
  materials: {
    slots: number;
    usedSlots: number;
    unusedSlots: number;
    invalidPrimitiveAssignments: number;
    invalidTextureReferences: number;
  };
  textures: {
    count: number;
    referenced: number;
    totalTexels: number;
    decodedBytes: number;
    invalidLayouts: number;
    descriptors: Array<{
      index: number;
      width: number;
      height: number;
      rgba8Bytes: number;
      layoutValid: boolean;
      referenced: boolean;
    }>;
  };
  normals: {
    completePrimitives: number;
    missingVertices: number;
    invalidVectors: number;
    zeroLengthVectors: number;
    nonUnitVectors: number;
  };
  uvs: {
    completePrimitives: number;
    missingPrimitives: number;
    partialPrimitives: number;
    missingVertices: number;
    invalidCoordinates: number;
    outOfUnitRangeVertices: number;
    degenerateUvTriangles: number;
    overlap: {
      state: "clear" | "detected" | "inconclusive" | "notApplicable";
      trianglesConsidered: number;
      pairsTested: number;
      overlappingPairs: number;
      pairBudget: number;
      method: string;
    };
  };
  bounds: { min: [number, number, number]; max: [number, number, number]; dimensions: [number, number, number] } | null;
  estimatedBytes: {
    positions: number;
    normals: number;
    uvs: number;
    indices: number;
    skin: number;
    textures: number;
    materialFactors: number;
    totalPayload: number;
  };
  capabilities: {
    cleanup: AssetLabAvailabilityFact;
    planarUvGeneration: AssetLabAvailabilityFact;
    holeFilling: AssetLabAvailabilityFact;
    hiddenGeometryRemoval: AssetLabAvailabilityFact;
    remeshing: AssetLabAvailabilityFact;
    lodGeneration: AssetLabAvailabilityFact;
    lodData: AssetLabAvailabilityFact;
    collisionGeneration: AssetLabAvailabilityFact;
    collisionData: AssetLabAvailabilityFact;
    textureBakeInput: AssetLabAvailabilityFact;
    highToLowTextureBaking: AssetLabAvailabilityFact;
  };
  warnings: string[];
}

export interface AssetLabChangeReport {
  before: AssetLabAuditReport;
  after: AssetLabAuditReport;
  changes: string[];
  warnings: string[];
}

export interface AssetLabProcessRequest {
  operation: "repair" | "optimize" | "uv" | "material" | "bake";
  preset?: string | null;
  targetRatio?: number | null;
  candidateLevels?: number;
  baseFraction?: number;
  weldThreshold?: number;
  removeDegenerateTriangles?: boolean;
  removeDuplicateTriangles?: boolean;
  removeIsolatedVertices?: boolean;
  removeComponentsSmallerThanTriangles?: number;
  repairWinding?: boolean;
  normalRepair?: "preserve" | "missingOrInvalid" | "recomputeAreaWeightedSmooth";
  preserveAttributeSeams?: boolean;
  resolution?: number;
  paddingPx?: number;
  texelsPerUnit?: number | null;
  replaceTexturedUv0?: boolean;
  highSourceIds?: string[];
  maps?: string[];
  cageDistance?: number;
  aoDistance?: number;
  aoSamples?: number;
  curvatureScale?: number;
  alignmentPolicy?: "autoRelated" | "worldSpace";
  minProjectionHitRatio?: number;
}

/** Measured proof that requested bake maps contain projected high-detail samples. */
export interface AssetLabBakeEvidence {
  resolution: number;
  requestedMaps: string[];
  sourceCount: number;
  sourceTriangles: number;
  charts: number;
  atlasTexels: number;
  coveredTexels: number;
  projectedTexels: number;
  projectionMisses: number;
  dilatedTexels: number;
  coverageRatio: number;
  projectionHitRatio: number;
  requiredHitRatio: number;
  alignmentPolicy: "autoRelated" | "worldSpace" | string;
  autoAlignedSources: number;
  worldSpaceSources: number;
}

export interface AssetLabResponse {
  ok: boolean;
  message: string;
  sourceEntity: string | null;
  sourceHandle: string | null;
  createdEntity: string | null;
  createdHandle: string | null;
  audit: AssetLabAuditReport | null;
  change: AssetLabChangeReport | null;
  warnings: string[];
  exportedPath: string | null;
  bakeEvidence: AssetLabBakeEvidence | null;
}

/** One machine-readable fidelity decision from complete-scene export. */
export interface SceneExportFidelity {
  status: "preserved" | "converted" | "omitted";
  feature: string;
  count: number;
  detail: string;
}

/** Result of exporting the authoritative hierarchy rather than one selected mesh. */
export interface SceneExportResponse {
  ok: boolean;
  message: string;
  format: "glb" | "usda" | "usd" | string;
  exportedPath: string | null;
  nodes: number;
  meshes: number;
  skins: number;
  animations: number;
  fidelity: SceneExportFidelity[];
}

// ── the authored match (ADR-097) ───────────────────────────────────────────────────────────────────────
// A match is authored as ordinary ECS components, cooked into runtime definitions, and only then run. These
// mirror `editor-shell/src/match_cook.rs`; the shell never sends kernel types across the boundary.

/** One thing the cook has to say about the authored scene, anchored to the object the author can click. */
export interface CookDiagnostic {
  severity: "error" | "warning";
  /** Stable machine code. The UI groups and tests match on this, never on the prose. */
  code: string;
  message: string;
  /** The authored entity at fault, as the same id the hierarchy uses. */
  entity: string | null;
  component: string | null;
  field: string | null;
}

/** Continuous authoring feedback: would this scene run, and what would it run? */
export interface MatchValidation {
  ok: boolean;
  /** Whether this is a match scene at all — the difference between "broken" and "not a match". */
  is_match_scene: boolean;
  diagnostics: CookDiagnostic[];
  cook_digest: string | null;
  actor_count: number;
  wave_count: number;
  lane_length_m: number;
}

/** The entities one "create a starter match" authored, so the panel can select and frame them. */
export interface AuthoredMatch {
  settings: string;
  lane: string;
  waypoints: string[];
  actors: string[];
  waves: string[];
}

/** One actor in the running match, as the editor surface sees it. */
export interface MatchActor {
  id: number;
  team: number;
  kind: string;
  x_mm: number;
  y_mm: number;
  health: number;
  max_health: number;
  alive: boolean;
  owned: boolean;
  /** Active control effects by name — the non-colour channel for runtime status. */
  controls: string[];
  speed: number;
  /** The standing attack order in plain words, or null when this actor has none. Worded by the kernel,
   *  not by this panel, so the UI can never describe behaviour the runtime does not have. */
  attack_order: string | null;
  /** The authored entity this actor was cooked from; null for a unit the match spawned at runtime. */
  source: string | null;
}

/** The observable state of a running match. Every value is read back from the kernel. */
export interface MatchStatus {
  running: boolean;
  tick: number;
  phase: string;
  world_digest: string;
  lane_digest: string;
  /** The cooked artifact this match was built from — what ties the run to the authored scene. */
  cook_digest: string;
  cook_schema_version: number;
  actor_count: number;
  live_actors: number;
  actors: MatchActor[];
  events: string[];
  last_rejection: string | null;
}

/** Why a match could not start, with the cook's diagnostics attached. */
export interface MatchStartError {
  message: string;
  diagnostics: CookDiagnostic[];
}

/** The deterministic, versioned cook output. Deliberately inspectable. */
export interface CookedMatch {
  schemaVersion: number;
  digest: string;
  seed: number;
  tickRateHz: number;
  maxMatchTicks: number;
  boundsMm: [number, number, number, number];
  lane: { centerline: [number, number][]; halfWidthMm: number; sources: string[] };
  actors: Array<Record<string, unknown>>;
  waves: Array<Record<string, unknown>>;
}

const te = new TextEncoder();
const td = new TextDecoder();
export const encodeJson = (v: unknown): Uint8Array => te.encode(JSON.stringify(v));
export const decodeJson = <T>(b: Uint8Array): T => JSON.parse(td.decode(b)) as T;
