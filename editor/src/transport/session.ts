//! The editor **session factory** (M10.1 / ADR-030) — the seam that makes the React `/editor` the
//! production shell UI. It picks the **real Tauri transport** (the packaged `.exe` talking to the live
//! `/core` over the `connect` Channel + the shell commands) when running inside the WebView, and falls
//! back to the in-process **MockCore** for `npm run dev` / Vitest. Either way the UI talks to one
//! [`EditorClient`] surface and the projection store is the single read-model (invariant 1): optimistic
//! echo on edit, reconcile on the authoritative `ProjectionDelta` (confirm/reject — every "no" explained,
//! ADR-010). The native viewport hot path never crosses this layer (invariant 4).

import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import type { ProjectInfo } from "../store/project";
import type { PlayInfo } from "../store/play";
import type {
  ActionItem,
  AddResponse,
  CatalogItem,
  CatalogSearch,
  CadReport,
  ContactInfo,
  JointInfo,
  DescribeResponse,
  EconResponse,
  EditIntent,
  EditTx,
  EntityDetails,
  EntityProjection,
  GenerateResponse,
  ImportResult,
  Json,
  JsonPatch,
  PhysicsWarning,
  ProjectionDelta,
  ProjectionOp,
  RevealResponse,
  AuthorRuleResult,
  RuleData,
  RuleRegistryInfo,
  RuleSummary,
  AuthorStateMachineResult,
  StateMachine,
  StateMachineInfo,
  Composition,
  ComposeProposal,
  ComposeResult,
  RuleDebugInfo,
  DecisionEvent,
  TruthState,
  RuleExplain,
  SnapHit,
  SolveResult,
  TimelineTuple,
} from "./protocol";
import { GENERATE_COST } from "./protocol";
import { DeltaClient } from "./client";
import { MockCore } from "./mock-core";
import { inProcessPair } from "./transport";

/** The one client surface the React UI talks to (the real shell transport + the dev MockCore both satisfy it). */
export interface EditorClient {
  /** Optimistic field edit → a JSON-Patch `EditTx` (the same language the AI layer emits). */
  setField(id: string, component: string, field: string, value: Json): string;
  /** Optimistic bind-by-intent; the authoritative edge streams back over the Channel. */
  bind(from: string, rel: string, to: string): string;
  /** Subscribe to ephemeral (preview/presence) frames — a no-op on the desktop shell for now. */
  onEphemeral(cb: (data: Json) => void): () => void;
  /** Reveal the ranked compatible bind targets (+ greyed-with-reason, + bound) for an entity (north-star #1). */
  revealTargets(id: string): Promise<RevealResponse>;
  /** Describe-to-create: resolve a free-text query (local → marketplace → generate seam). */
  describe(query: string): Promise<DescribeResponse>;
  /** The user's token balance (M7). */
  walletInfo(): Promise<EconResponse>;
  /** Sandbox top-up (M7 — no real money, ADR-004/018). */
  topUp(): Promise<EconResponse>;
  /** AI-edit: assign a named PBR material preset to an entity (M7 + M11.2 — schema-validated patch,
   *  debit-on-success). `material` defaults to "rusty" (the original weathered-metal look). */
  aiEdit(id: string, material?: string): Promise<EconResponse>;
  /** Generate (tier 3, opt-in — M6 / ADR-017): a placeholder drops in + the cost is metered; the real
   *  mesh streams in later over the projection Channel. The opt-in tier-3 generate, not the default path. */
  generate(query: string): Promise<GenerateResponse>;
  /** Undo the last committed transaction (Ctrl-Z); the reverting delta streams back over the Channel.
   *  Resolves to whether anything was actually reverted (false on an empty history → honest UI feedback). */
  undo(): Promise<boolean>;
  /** The context-menu actions for an entity (M3.3) — each available-or-explained. */
  entityActions(id: string): Promise<ActionItem[]>;
  /** The hover-tooltip details for an entity (M3.3) — name + components + caps + bound. */
  entityDetails(id: string): Promise<EntityDetails | null>;
  /** The per-part CAD import report (M15.7) — the fidelity breakdown + a capped part list, off the ECS. */
  cadReport(): Promise<CadReport>;
  /** M15.9 — author a joint (real axis + pivot; honesty-labeled source) as ONE undoable commit. */
  setJoint(id: string, revolute: boolean, axis: [number, number, number], pivot: [number, number, number], min: number, max: number, source: string): Promise<boolean>;
  /** M15.9 — key the joint's current DOF value at time t (ONE undoable commit). */
  jointKey(id: string, t: number): Promise<boolean>;
  /** M15.9 — drive a joint's DOF (preview or commit — the gizmo-drag pattern on a kinematic DOF). */
  jointValue(id: string, value: number, commit: boolean): Promise<boolean>;
  /** M15.9 — scrub the mechanism timeline to t (deterministic, render-only; t<0 clears). */
  jointScrub(t: number): Promise<number>;
  /** M15.9 — the selected joint's state (a read). */
  jointInfo(id: string): Promise<JointInfo | null>;
  /** Remove an entity + its edges (M3.3) — one undoable transaction (the delta streams back). */
  removeEntity(id: string): void;
  /** Duplicate an entity (M3.3) — one undoable transaction; resolves to the clone's id. */
  duplicateEntity(id: string): Promise<string | null>;
  /** Frame the camera on an entity (M3.3) — no mutation. */
  focusEntity(id: string): void;
  /** M8.3: turn a dead mesh into a correct dynamic body — one undoable transaction. */
  makeDynamic(id: string): Promise<boolean>;

  // ── M10.6 scene-authoring verbs (ADR-036) — each one undoable transaction over the Movable Tree +
  // override pipeline. reparent reuses `reparentPart`; delete=deactivate is distinct from `removeEntity`. ──
  /** Create an empty named entity at a position → its id (the caller selects it). */
  createEntity(x: number, y: number, z: number, name: string): Promise<string | null>;
  /** M11.3 — author a Light entity (kind = directional|point|spot) at a position with a linear RGB colour +
   *  intensity → its id. One undoable commit; the lit result is a render projection (not in the doc). */
  addLight(kind: string, x: number, y: number, z: number, r: number, g: number, b: number, intensity: number): Promise<string | null>;
  /** Rename an entity (`__meta__.name`) → applied; the projection re-reads it (inv. 1). */
  renameEntity(id: string, name: string): Promise<boolean>;
  /** Group a selection under a new parent node → the group id. */
  groupEntities(ids: string[], name: string): Promise<string | null>;
  /** Ungroup — dissolve a group (children to its parent, delete the group) → applied. */
  ungroupEntity(id: string): Promise<boolean>;
  /** Multi-edit — set one numeric field on N entities as ONE batched, atomic, undoable tx → applied. */
  multiEdit(ids: string[], component: string, field: string, value: number): Promise<boolean>;
  /** Delete = deactivate (non-destructive; frees dependents) — undo restores → applied. */
  deleteDeactivate(id: string): Promise<boolean>;
  /** Copy a sub-tree to the clipboard (cross-project = the serde Composition). */
  copySubtree(id: string): void;
  /** Cut = copy + delete(deactivate) → applied. */
  cutSubtree(id: string): Promise<boolean>;
  /** Paste the clipboard under fresh deterministic ids → the new root id. */
  pasteClipboard(): Promise<string | null>;

  // ── M8 physics (the React PhysicsPanel; the sim runs natively off the JS hot path — invariant 4) ─────
  /** Drop / spawn a dynamic body at a world position → the new body's id (or null). */
  spawnBody(x: number, y: number, z: number): Promise<string | null>;
  /** Pause / resume the deterministic sim (the M8 run flag). */
  setSimRunning(run: boolean): void;
  /** The "debug by looking" contact-overlay flag. */
  simOverlay(on: boolean): void;
  /** The sim timeline `[frame, maxFrame, running, overlaysOn, bodies]` — drives the transport chrome. */
  simTimeline(): Promise<TimelineTuple>;
  /** Scrub the deterministic replay to a frame (lands EXACTLY there + pauses). */
  simScrub(frame: number): Promise<TimelineTuple>;
  /** Shove the selected body with an impulse → applied (false if it isn't a body). */
  simShove(id: string, impulse: [number, number, number]): Promise<boolean>;
  /** The explained contact rows (the debugger overlay's "why"). */
  physicsContacts(): Promise<ContactInfo[]>;
  /** The collider-intelligence warnings for a body — each explained + a one-click fix. */
  physicsCheck(id: string): Promise<PhysicsWarning[]>;
  /** Apply a one-click physics fix (the `fixAction` from a warning). */
  physicsFix(id: string, action: string): Promise<boolean>;
  /** Import a URDF/USD interchange document → bodies + explained reconciliation notes. */
  importInterchange(format: string, source: string): Promise<ImportResult>;

  // ── M9 transform / gizmo / part / snap (the React TransformPanel) ───────────────────────────────────
  /** Set the gizmo mode (the W/E/R shortcut) — sticky tool state on the shared gizmo. */
  gizmoMode(mode: "translate" | "rotate" | "scale"): void;
  /** Select an entity for the gizmo (so an inspector button can act on it) → found. */
  gizmoSelect(id: string): Promise<boolean>;
  /** The currently gizmo-selected entity's id (so a button acts on the LIVE engine selection) — or null. */
  gizmoSelected(): Promise<string | null>;
  /** The gizmo HUD read `[mode, hasSel, dragging, space, pivot]`. */
  gizmoDebug(): Promise<[string, boolean, boolean, string, string]>;
  /** Toggle world/local space → the new label. */
  gizmoSpaceToggle(): Promise<string>;
  /** Toggle origin/center pivot → the new label. */
  gizmoPivotToggle(): Promise<string>;
  /** Begin a gizmo handle drag at normalized cursor coords → hit (so JS knows not to fall through). */
  gizmoPickDrag(x: number, y: number, ctrl: boolean): Promise<boolean>;
  /** End the gizmo drag — commits ONE undoable transform transaction. */
  gizmoDragEnd(): void;
  /** Read an entity's world transform `[x,y,z,qx,qy,qz,qw,scale]`. */
  readTransform(id: string): Promise<number[]>;
  /** Save the selected character (with its part overrides) for reuse → the comp id. */
  saveCharacter(id: string): Promise<string | null>;
  /** Drop a fresh instance of a saved character → the new root id. */
  instantiateCharacter(comp: string): Promise<string | null>;
  /** Deactivate-not-delete (or restore) a rigid part — recoverable, undoable. */
  setPartActive(id: string, active: boolean): Promise<boolean>;
  /** Reparent a part under a new parent (node.move) — undoable. */
  reparentPart(id: string, parent: string | null): void;
  /** Magnetic-snap toggle (M9.4). */
  setSnap(on: boolean): void;
  /** The ranked snap candidates within a radius, each with an explained `why`. */
  snapQuery(id: string, radius: number): Promise<SnapHit[]>;
  /** Apply a declared transform constraint → solve+commit, or refuse-with-reason (every "no" explained). */
  applyConstraint(id: string, kind: string, target: string | null, value: number): Promise<SolveResult>;
  /** Compile a natural-language placement sentence to ≥1 editable intent. */
  placementSentence(id: string, text: string): Promise<SolveResult>;

  // ── M3.3 focus mode (the FocusBanner) ───────────────────────────────────────────────────────────────
  /** Exit focus mode — restore the saved camera distance + drop the dim flag. */
  unfocus(): void;
  /** The focus read `[framedDistance, focusActive]` (the banner shows the distance). */
  focusDebug(): Promise<[number, boolean]>;

  // ── M10.7 camera & framing ergonomics (ADR-037) — pure camera ops, native (invariant 4) ──────────────
  /** Frame the whole scene (center + fit the bounds). */
  frameAll(): void;
  /** Snap the camera to a canonical view: `top` / `front` / `side` / `persp`. */
  viewPreset(preset: string): void;
  /** The camera state `[orbit, elevation, distance, tx, ty, tz]` (the orientation cube + the e2e). */
  cameraDebug(): Promise<number[]>;

  // ── native viewport input (Tauri-only; the dev MockCore has no viewport) — the M10.1 composite closeout ─
  /** Pick over the native wgpu region at NORMALIZED viewport coords (x,y ∈ [0,1]) → the picked entity id
   *  (or null). Computed synchronously in the command from the camera ray (no per-frame race, no OS-cursor
   *  dependency — so a synthetic click works too). */
  viewportPick(x: number, y: number): Promise<string | null>;
  /** Begin a right-drag orbit — the native render loop then polls the OS cursor and orbits with **0 IPC per
   *  frame** (invariant 4); only this call + `dragEnd` cross the boundary, once per gesture. */
  dragStart(): void;
  /** End the orbit drag. */
  dragEnd(): void;
  /** Wheel zoom — folded into the camera distance natively next frame (one call per wheel tick). */
  zoom(delta: number): void;

  // ── M14.2 live per-entity thumbnails (ADR-058) ───────────────────────────────────────────────────────
  /** Render a live thumbnail of one entity as it ACTUALLY renders in the viewport — a **discrete off-frame
   *  RTT** on the native renderer (its own encoder, before the swapchain frame, so it never touches the
   *  0-IPC orbit path, invariant 4) → a `data:` PNG URL at `size` px, or **null** when the renderer can't
   *  service it (over budget / no mesh / the dev/browser build has no wgpu surface → the icon fallback). */
  thumbnail(id: string, size: number): Promise<string | null>;

  /** The browse catalog (M3.4 / ADR-019) — the ONE catalog (registry + marketplace + imported), grouped
   *  by category. The asset browser reuses this; it never forks the search/category logic. */
  catalog(): Promise<Record<string, CatalogItem[]>>;
  /** Search the one catalog (reuses the tiered resolver) — ranked matches + a no-match seam. */
  catalogSearch(query: string): Promise<CatalogSearch>;
  /** Instantiate a catalog item into the scene (place-into-scene) — one undoable, persisted entity. */
  addItem(id: string, source: string): Promise<AddResponse>;

  // ── M11.1 File→Import (ADR-040): drop any file → a working asset (FBX/glTF/OBJ/PNG via the MAGIC router) ─
  /** Import an asset file from a known path → the new entity id (the e2e path). */
  importAsset(path: string): Promise<string | null>;
  /** File→Import: open the native file dialog + import the chosen file → the new entity id. */
  importAssetDialog(): Promise<string | null>;

  // ── project lifecycle (M10.3 / ADR-033): New / Open / Save / Save As over the `.mtk` document ──────
  /** The current project state — path, unsaved-changes flag, recent projects. The File menu refreshes
   *  this on open so the unsaved-changes guard reads the authoritative (shell-side) dirty flag. */
  projectState(): Promise<ProjectInfo>;
  /** New empty project (discarding the current scene — the menu guards on `dirty` first). */
  newProject(): Promise<ProjectInfo>;
  /** Open a `.mtk` project. With a `path` (a recent), opens it directly; without one, the shell shows a
   *  native Open dialog (the live half — owed). A corrupt/newer/missing file resolves with `error` set. */
  openProject(path?: string): Promise<ProjectInfo>;
  /** Save to the current path (atomic, ADR-033); if the project is untitled, the shell shows a Save
   *  dialog (the live half — owed). */
  saveProject(): Promise<ProjectInfo>;
  /** Save As — always picks a new path via the shell's native dialog (the live half — owed). */
  saveProjectAs(): Promise<ProjectInfo>;

  // ── Play mode (M10.4 / ADR-034): run the scene non-destructively ────────────────────────────────
  /** Enter Play — run the deterministic sim on the current scene (snapshots the edit state for Stop). */
  play(): Promise<PlayInfo>;
  /** Stop — restore the exact pre-Play edit state (non-destructive) and exit play mode. */
  stop(): Promise<PlayInfo>;
  /** Pause / resume the running sim (stays in play mode). */
  pause(): Promise<PlayInfo>;
  /** The current Play-mode state (a read) — the controls refresh from this. */
  playState(): Promise<PlayInfo>;

  // ── M12.1 Rules layer (When/If/Then) — the registry-fed builder + Rule list (ADR-045) ───────────────
  /** The typo-proof builder vocabulary: every event/action/component+field the builder may offer. */
  ruleRegistry(): Promise<RuleRegistryInfo>;
  /** All authored rules for the editor Rule list. */
  listRules(): Promise<RuleSummary[]>;
  /** Author (or replace, if `id` is given) a rule: the new id + the offered mirror, or a Blocked reason. */
  authorRule(rule: RuleData, id?: string | null): Promise<AuthorRuleResult>;
  /** Remove a rule (one undoable transaction). */
  deleteRule(id: string): Promise<boolean>;

  // ── M12.2 state machines (states + transitions = Rules) — the visual state-graph (ADR-046) ───────────
  /** All authored state machines for the state-graph view (the full machine + the live current state). */
  stateMachines(): Promise<StateMachineInfo[]>;
  /** Author (or replace, if `id` is given) a state machine: the new id + the unreachable warning, or a
   *  Blocked reason (no-dangling / typo'd transition Rule / not-a-state-change). One undoable transaction. */
  authorStateMachine(sm: StateMachine, id?: string | null): Promise<AuthorStateMachineResult>;
  /** Remove a state machine (one undoable transaction). */
  deleteStateMachine(id: string): Promise<boolean>;

  // ── M12.4 AI compose (ADR-048) — sentence → reviewable Composition → apply through the one pipeline ──────
  /** Turn a natural-language `sentence` into a REVIEWABLE composition proposal (the in-app AI compose seam),
   *  validated against the live scene. `target` = the selected entity the rule acts on. Nothing is applied. */
  proposeComposition(sentence: string, target: string | null): Promise<ComposeProposal>;
  /** Apply a reviewed `composition` through the one commit pipeline as a single undoable transaction, or
   *  reject it whole with a plain-language reason (nothing applied) — the AI is never a raw mutation. */
  compose(composition: Composition): Promise<ComposeResult>;

  // ── M12.5 Rules in Play + the live truth-state debugger (ADR-049) ────────────────────────────────────
  /** Fire a live gameplay `event` (e.g. `EnemyDied`) into the running Rules — the When-channel. A
   *  PROJECTION (never the doc); recorded so a scrub replays it. Returns the fresh truth-state for `selected`. */
  fireRuleEvent(event: string, subject: string | null, selected: string | null): Promise<RuleDebugInfo>;
  /** The "debug by looking" read: the clicked entity's live truth-state (✅/❌ per condition + machine state),
   *  `explain_rule` narration, the decision history, and the determinism-flagged rules. */
  ruleDebug(id: string | null): Promise<RuleDebugInfo>;
  /** Scrub the decision history to `frame` over the M8.4 replay channel (watch exactly when a counter
   *  incremented / a transition fired) and return the truth-state at that frame for `selected`. */
  ruleScrub(frame: number, selected: string | null): Promise<RuleDebugInfo>;
}

// ── the Tauri global (withGlobalTauri: true exposes window.__TAURI__.core; no @tauri-apps/api dep) ──────
interface TauriChannel<T> {
  onmessage: (msg: T) => void;
}
interface TauriCore {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  Channel: new <T>() => TauriChannel<T>;
}
function tauriCore(): TauriCore | null {
  const w = globalThis as unknown as { __TAURI__?: { core?: TauriCore } };
  return w.__TAURI__?.core ?? null;
}

/** True when running inside the packaged Tauri WebView (vs `npm run dev` / Vitest). */
export const isTauri = (): boolean => tauriCore() !== null;

/** The REAL shell transport: `connect` streams committed `ProjectionDelta`s into the store; edits go out
 *  through the shell's commands (`submit_edit` / `bind_target`) — the exact contract the vanilla scaffold
 *  used, so the 61 commands + the Channel are unchanged (M10.1 swaps the UI, not the core). */
export class TauriClient implements EditorClient {
  private opCounter = 0;
  private readonly core: TauriCore;

  constructor(core: TauriCore) {
    this.core = core;
    const channel = new core.Channel<ProjectionDelta>();
    channel.onmessage = (delta) => projectionStore.getState().applyDelta(delta);
    void this.core
      .invoke("connect", { channel })
      .catch((e: unknown) => console.error("connect failed", e));
  }

  private nextOp(): string {
    this.opCounter += 1;
    return `op-${this.opCounter}`;
  }

  setField(id: string, component: string, field: string, value: Json): string {
    const clientOpId = this.nextOp();
    const intent: EditIntent = { kind: "setField", id, component, field, value };
    const patches: JsonPatch[] = [
      { op: "replace", path: `/entities/${id}/components/${component}/${field}`, value },
    ];
    projectionStore.getState().optimisticEdit({ clientOpId, intent });
    const tx: EditTx = { clientOpId, label: `set ${component}.${field}`, patches, intent };
    void this.core.invoke("submit_edit", { tx }).catch((e: unknown) => console.error("submit_edit failed", e));
    return clientOpId;
  }

  bind(from: string, rel: string, to: string): string {
    const clientOpId = this.nextOp();
    const intent: EditIntent = { kind: "bind", from, rel, to };
    projectionStore.getState().optimisticEdit({ clientOpId, intent });
    // The shell's dedicated M3.1 bind command; the authoritative addEdge streams back over the Channel.
    void this.core.invoke("bind_target", { from, to }).catch((e: unknown) => console.error("bind_target failed", e));
    return clientOpId;
  }

  onEphemeral(): () => void {
    return () => {};
  }

  revealTargets(id: string): Promise<RevealResponse> {
    return this.core.invoke<RevealResponse>("reveal_targets", { id }).catch((e: unknown) => { console.error("reveal_targets failed", e); throw e; });
  }

  describe(query: string): Promise<DescribeResponse> {
    return this.core.invoke<DescribeResponse>("describe", { query }).catch((e: unknown) => { console.error("describe failed", e); throw e; });
  }

  walletInfo(): Promise<EconResponse> {
    return this.core.invoke<EconResponse>("wallet_info").catch((e: unknown) => { console.error("wallet_info failed", e); throw e; });
  }

  topUp(): Promise<EconResponse> {
    return this.core.invoke<EconResponse>("top_up").catch((e: unknown) => { console.error("top_up failed", e); throw e; });
  }

  aiEdit(id: string, material?: string): Promise<EconResponse> {
    return this.core.invoke<EconResponse>("ai_edit", { id, material: material ?? null }).catch((e: unknown) => { console.error("ai_edit failed", e); throw e; });
  }

  generate(query: string): Promise<GenerateResponse> {
    return this.core.invoke<GenerateResponse>("generate", { query }).catch((e: unknown) => { console.error("generate failed", e); throw e; });
  }

  undo(): Promise<boolean> {
    // Returns whether a transaction was actually reverted, so the UI can be honest ("undo" vs "nothing to
    // undo") instead of always claiming a revert.
    return this.core.invoke<boolean>("undo").catch((e: unknown) => {
      console.error("undo failed", e);
      return false;
    });
  }

  entityActions(id: string): Promise<ActionItem[]> {
    return this.core.invoke<ActionItem[]>("entity_actions", { id }).catch((e: unknown) => { console.error("entity_actions failed", e); throw e; });
  }
  cadReport(): Promise<CadReport> {
    return this.core.invoke<CadReport>("cad_report").catch((e: unknown) => { console.error("cad_report failed", e); throw e; });
  }
  setJoint(id: string, revolute: boolean, axis: [number, number, number], pivot: [number, number, number], min: number, max: number, source: string): Promise<boolean> {
    return this.core.invoke<boolean>("set_joint", { id, revolute, axis, pivot, min, max, source }).catch((e: unknown) => { console.error("set_joint failed", e); return false; });
  }
  jointKey(id: string, t: number): Promise<boolean> {
    return this.core.invoke<boolean>("joint_key", { id, t }).catch((e: unknown) => { console.error("joint_key failed", e); return false; });
  }
  jointValue(id: string, value: number, commit: boolean): Promise<boolean> {
    return this.core.invoke<boolean>("joint_value", { id, value, commit }).catch((e: unknown) => { console.error("joint_value failed", e); return false; });
  }
  jointScrub(t: number): Promise<number> {
    return this.core.invoke<number>("joint_scrub", { t }).catch((e: unknown) => { console.error("joint_scrub failed", e); return 0; });
  }
  jointInfo(id: string): Promise<JointInfo | null> {
    return this.core.invoke<JointInfo | null>("joint_info", { id }).catch((e: unknown) => { console.error("joint_info failed", e); return null; });
  }
  entityDetails(id: string): Promise<EntityDetails | null> {
    return this.core.invoke<EntityDetails | null>("entity_details", { id }).catch((e: unknown) => { console.error("entity_details failed", e); throw e; });
  }
  removeEntity(id: string): void {
    void this.core.invoke("remove_entity", { id }).catch((e: unknown) => console.error("remove_entity failed", e));
  }
  duplicateEntity(id: string): Promise<string | null> {
    return this.core.invoke<string | null>("duplicate_entity", { id }).catch((e: unknown) => { console.error("duplicate_entity failed", e); throw e; });
  }
  focusEntity(id: string): void {
    void this.core.invoke("focus_entity", { id }).catch((e: unknown) => console.error("focus_entity failed", e));
  }
  makeDynamic(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("make_dynamic", { id }).catch((e: unknown) => { console.error("make_dynamic failed", e); throw e; });
  }

  // ── M10.6 scene-authoring verbs ──
  createEntity(x: number, y: number, z: number, name: string): Promise<string | null> {
    return this.core.invoke<string | null>("create_entity", { x, y, z, name }).catch((e: unknown) => { console.error("create_entity failed", e); throw e; });
  }
  addLight(kind: string, x: number, y: number, z: number, r: number, g: number, b: number, intensity: number): Promise<string | null> {
    return this.core.invoke<string | null>("add_light", { kind, x, y, z, r, g, b, intensity }).catch((e: unknown) => { console.error("add_light failed", e); throw e; });
  }
  renameEntity(id: string, name: string): Promise<boolean> {
    return this.core.invoke<boolean>("rename_entity", { id, name }).catch((e: unknown) => { console.error("rename_entity failed", e); throw e; });
  }
  groupEntities(ids: string[], name: string): Promise<string | null> {
    return this.core.invoke<string | null>("group_entities", { ids, name }).catch((e: unknown) => { console.error("group_entities failed", e); throw e; });
  }
  ungroupEntity(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("ungroup_entity", { id }).catch((e: unknown) => { console.error("ungroup_entity failed", e); throw e; });
  }
  multiEdit(ids: string[], component: string, field: string, value: number): Promise<boolean> {
    return this.core.invoke<boolean>("multi_edit", { ids, component, field, value }).catch((e: unknown) => { console.error("multi_edit failed", e); throw e; });
  }
  deleteDeactivate(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("delete_deactivate", { id }).catch((e: unknown) => { console.error("delete_deactivate failed", e); throw e; });
  }
  copySubtree(id: string): void {
    void this.core.invoke("copy_subtree", { id }).catch((e: unknown) => console.error("copy_subtree failed", e));
  }
  cutSubtree(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("cut_subtree", { id }).catch((e: unknown) => { console.error("cut_subtree failed", e); throw e; });
  }
  pasteClipboard(): Promise<string | null> {
    return this.core.invoke<string | null>("paste_clipboard").catch((e: unknown) => { console.error("paste_clipboard failed", e); throw e; });
  }

  // ── M8 physics ──
  spawnBody(x: number, y: number, z: number): Promise<string | null> {
    return this.core.invoke<string | null>("spawn_body", { x, y, z }).catch((e: unknown) => { console.error("spawn_body failed", e); throw e; });
  }
  setSimRunning(run: boolean): void {
    void this.core.invoke("set_sim_running", { run }).catch((e: unknown) => console.error("set_sim_running failed", e));
  }
  simOverlay(on: boolean): void {
    void this.core.invoke("sim_overlay", { on }).catch((e: unknown) => console.error("sim_overlay failed", e));
  }
  simTimeline(): Promise<TimelineTuple> {
    return this.core.invoke<TimelineTuple>("sim_timeline").catch((e: unknown) => { console.error("sim_timeline failed", e); throw e; });
  }
  simScrub(frame: number): Promise<TimelineTuple> {
    return this.core.invoke<TimelineTuple>("sim_scrub", { frame }).catch((e: unknown) => { console.error("sim_scrub failed", e); throw e; });
  }
  simShove(id: string, impulse: [number, number, number]): Promise<boolean> {
    return this.core.invoke<boolean>("sim_shove", { id, impulse }).catch((e: unknown) => { console.error("sim_shove failed", e); throw e; });
  }
  physicsContacts(): Promise<ContactInfo[]> {
    return this.core.invoke<ContactInfo[]>("physics_contacts").catch((e: unknown) => { console.error("physics_contacts failed", e); throw e; });
  }
  physicsCheck(id: string): Promise<PhysicsWarning[]> {
    return this.core.invoke<PhysicsWarning[]>("physics_check", { id }).catch((e: unknown) => { console.error("physics_check failed", e); throw e; });
  }
  physicsFix(id: string, action: string): Promise<boolean> {
    return this.core.invoke<boolean>("physics_fix", { id, action }).catch((e: unknown) => { console.error("physics_fix failed", e); throw e; });
  }
  importInterchange(format: string, source: string): Promise<ImportResult> {
    return this.core.invoke<ImportResult>("import_interchange", { format, source }).catch((e: unknown) => { console.error("import_interchange failed", e); throw e; });
  }

  // ── M9 transform / gizmo / part / snap ──
  gizmoMode(mode: "translate" | "rotate" | "scale"): void {
    void this.core.invoke("gizmo_mode", { mode }).catch((e: unknown) => console.error("gizmo_mode failed", e));
  }
  gizmoSelect(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("gizmo_select", { id }).catch((e: unknown) => { console.error("gizmo_select failed", e); throw e; });
  }
  gizmoSelected(): Promise<string | null> {
    return this.core.invoke<string | null>("gizmo_selected").catch((e: unknown) => { console.error("gizmo_selected failed", e); throw e; });
  }
  gizmoDebug(): Promise<[string, boolean, boolean, string, string]> {
    return this.core.invoke<[string, boolean, boolean, string, string]>("gizmo_debug").catch((e: unknown) => { console.error("gizmo_debug failed", e); throw e; });
  }
  gizmoSpaceToggle(): Promise<string> {
    return this.core.invoke<string>("gizmo_space_toggle").catch((e: unknown) => { console.error("gizmo_space_toggle failed", e); throw e; });
  }
  gizmoPivotToggle(): Promise<string> {
    return this.core.invoke<string>("gizmo_pivot_toggle").catch((e: unknown) => { console.error("gizmo_pivot_toggle failed", e); throw e; });
  }
  gizmoPickDrag(x: number, y: number, ctrl: boolean): Promise<boolean> {
    return this.core.invoke<boolean>("gizmo_pick_drag", { x, y, ctrl }).catch((e: unknown) => { console.error("gizmo_pick_drag failed", e); throw e; });
  }
  gizmoDragEnd(): void {
    void this.core.invoke("gizmo_drag_end").catch((e: unknown) => console.error("gizmo_drag_end failed", e));
  }
  readTransform(id: string): Promise<number[]> {
    return this.core.invoke<number[]>("read_transform", { id }).catch((e: unknown) => { console.error("read_transform failed", e); throw e; });
  }
  saveCharacter(id: string): Promise<string | null> {
    return this.core.invoke<string | null>("save_character", { id }).catch((e: unknown) => { console.error("save_character failed", e); throw e; });
  }
  instantiateCharacter(comp: string): Promise<string | null> {
    return this.core.invoke<string | null>("instantiate_character", { comp }).catch((e: unknown) => { console.error("instantiate_character failed", e); throw e; });
  }
  setPartActive(id: string, active: boolean): Promise<boolean> {
    return this.core.invoke<boolean>("set_part_active", { id, active }).catch((e: unknown) => { console.error("set_part_active failed", e); throw e; });
  }
  reparentPart(id: string, parent: string | null): void {
    void this.core.invoke("reparent_part", { id, parent }).catch((e: unknown) => console.error("reparent_part failed", e));
  }
  setSnap(on: boolean): void {
    void this.core.invoke("set_snap", { on }).catch((e: unknown) => console.error("set_snap failed", e));
  }
  snapQuery(id: string, radius: number): Promise<SnapHit[]> {
    return this.core.invoke<SnapHit[]>("snap_query", { id, radius }).catch((e: unknown) => { console.error("snap_query failed", e); throw e; });
  }
  applyConstraint(id: string, kind: string, target: string | null, value: number): Promise<SolveResult> {
    return this.core.invoke<SolveResult>("apply_constraint", { id, kind, target, value }).catch((e: unknown) => { console.error("apply_constraint failed", e); throw e; });
  }
  placementSentence(id: string, text: string): Promise<SolveResult> {
    return this.core.invoke<SolveResult>("placement_sentence", { id, text }).catch((e: unknown) => { console.error("placement_sentence failed", e); throw e; });
  }

  // ── M3.3 focus ──
  unfocus(): void {
    void this.core.invoke("unfocus").catch((e: unknown) => console.error("unfocus failed", e));
  }
  focusDebug(): Promise<[number, boolean]> {
    return this.core.invoke<[number, boolean]>("focus_debug").catch((e: unknown) => { console.error("focus_debug failed", e); throw e; });
  }

  frameAll(): void {
    void this.core.invoke("frame_all").catch((e: unknown) => console.error("frame_all failed", e));
  }
  viewPreset(preset: string): void {
    void this.core.invoke("view_preset", { preset }).catch((e: unknown) => console.error("view_preset failed", e));
  }
  cameraDebug(): Promise<number[]> {
    return this.core.invoke<number[]>("camera_debug").catch((e: unknown) => { console.error("camera_debug failed", e); throw e; });
  }

  viewportPick(x: number, y: number): Promise<string | null> {
    return this.core.invoke<string | null>("viewport_pick", { x, y }).catch((e: unknown) => { console.error("viewport_pick failed", e); throw e; });
  }
  dragStart(): void {
    void this.core.invoke("drag_start").catch((e: unknown) => console.error("drag_start failed", e));
  }
  dragEnd(): void {
    void this.core.invoke("drag_end").catch((e: unknown) => console.error("drag_end failed", e));
  }
  zoom(delta: number): void {
    void this.core.invoke("zoom", { delta }).catch((e: unknown) => console.error("zoom failed", e));
  }

  thumbnail(id: string, size: number): Promise<string | null> {
    // A discrete render (off the per-frame path). On any failure resolve null → the store shows the icon
    // fallback (never throws into the dirty-tracking loop).
    return this.core.invoke<string | null>("thumbnail", { id, size }).catch((e: unknown) => {
      console.error("thumbnail failed", e);
      return null;
    });
  }

  catalog(): Promise<Record<string, CatalogItem[]>> {
    return this.core.invoke<Record<string, CatalogItem[]>>("catalog");
  }
  catalogSearch(query: string): Promise<CatalogSearch> {
    return this.core.invoke<CatalogSearch>("catalog_search", { query }).catch((e: unknown) => { console.error("catalog_search failed", e); throw e; });
  }
  addItem(id: string, source: string): Promise<AddResponse> {
    return this.core.invoke<AddResponse>("add_item", { id, source }).catch((e: unknown) => { console.error("add_item failed", e); throw e; });
  }
  async importAsset(path: string): Promise<string | null> {
    try {
      const id = await this.core.invoke<string | null>("import_asset", { path });
      await this.hintNearDuplicate(id);
      return id;
    } catch (e: unknown) {
      console.error("import_asset failed", e);
      throw e;
    }
  }
  async importAssetDialog(): Promise<string | null> {
    try {
      const id = await this.core.invoke<string | null>("import_asset_dialog");
      await this.hintNearDuplicate(id);
      return id;
    } catch (e: unknown) {
      console.error("import_asset_dialog failed", e);
      throw e;
    }
  }
  /** M11.5 (ADR-044, SA-34) — a lightweight import-time HINT: if the just-imported asset perceptually
   *  matches an already-loaded one (different bytes — a rescaled/recompressed copy the exact content-hash
   *  dedup misses), surface a non-blocking toast. It was imported anyway as a distinct asset — never a silent
   *  merge. Reads the `asset_provenance` projection already computed on import; the persistent panel + the
   *  thumbnail treatment are deferred to M14.3. Best-effort: a hint failure never breaks the import. */
  private async hintNearDuplicate(id: string | null): Promise<void> {
    if (!id) return;
    try {
      const prov = await this.core.invoke<{ nearDuplicateOf?: string | null } | null>(
        "asset_provenance",
        { id },
      );
      if (prov?.nearDuplicateOf) {
        pushToast(`Near-duplicate of ${prov.nearDuplicateOf} — kept as a separate asset`, "info");
      }
    } catch (e: unknown) {
      console.error("near-duplicate hint failed", e);
    }
  }

  projectState(): Promise<ProjectInfo> {
    return this.core.invoke<ProjectInfo>("project_state").catch((e: unknown) => { console.error("project_state failed", e); throw e; });
  }
  newProject(): Promise<ProjectInfo> {
    return this.core.invoke<ProjectInfo>("new_project").catch((e: unknown) => { console.error("new_project failed", e); throw e; });
  }
  openProject(path?: string): Promise<ProjectInfo> {
    return this.core.invoke<ProjectInfo>("open_project", { path: path ?? null }).catch((e: unknown) => { console.error("open_project failed", e); throw e; });
  }
  saveProject(): Promise<ProjectInfo> {
    return this.core.invoke<ProjectInfo>("save_project", { path: null }).catch((e: unknown) => { console.error("save_project failed", e); throw e; });
  }
  saveProjectAs(): Promise<ProjectInfo> {
    return this.core.invoke<ProjectInfo>("save_project_as").catch((e: unknown) => { console.error("save_project_as failed", e); throw e; });
  }

  play(): Promise<PlayInfo> {
    return this.core.invoke<PlayInfo>("play").catch((e: unknown) => { console.error("play failed", e); throw e; });
  }
  stop(): Promise<PlayInfo> {
    return this.core.invoke<PlayInfo>("stop").catch((e: unknown) => { console.error("stop failed", e); throw e; });
  }
  pause(): Promise<PlayInfo> {
    return this.core.invoke<PlayInfo>("pause").catch((e: unknown) => { console.error("pause failed", e); throw e; });
  }
  playState(): Promise<PlayInfo> {
    return this.core.invoke<PlayInfo>("play_state").catch((e: unknown) => { console.error("play_state failed", e); throw e; });
  }
  ruleRegistry(): Promise<RuleRegistryInfo> {
    return this.core.invoke<RuleRegistryInfo>("rule_registry").catch((e: unknown) => { console.error("rule_registry failed", e); throw e; });
  }
  listRules(): Promise<RuleSummary[]> {
    return this.core.invoke<RuleSummary[]>("list_rules").catch((e: unknown) => { console.error("list_rules failed", e); throw e; });
  }
  authorRule(rule: RuleData, id: string | null = null): Promise<AuthorRuleResult> {
    return this.core.invoke<AuthorRuleResult>("author_rule", { rule, id }).catch((e: unknown) => { console.error("author_rule failed", e); throw e; });
  }
  deleteRule(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("delete_rule", { id }).catch((e: unknown) => { console.error("delete_rule failed", e); throw e; });
  }
  stateMachines(): Promise<StateMachineInfo[]> {
    return this.core.invoke<StateMachineInfo[]>("state_machines").catch((e: unknown) => { console.error("state_machines failed", e); throw e; });
  }
  authorStateMachine(sm: StateMachine, id: string | null = null): Promise<AuthorStateMachineResult> {
    return this.core.invoke<AuthorStateMachineResult>("author_state_machine", { sm, id }).catch((e: unknown) => { console.error("author_state_machine failed", e); throw e; });
  }
  deleteStateMachine(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("delete_state_machine", { id }).catch((e: unknown) => { console.error("delete_state_machine failed", e); throw e; });
  }
  proposeComposition(sentence: string, target: string | null): Promise<ComposeProposal> {
    return this.core.invoke<ComposeProposal>("propose_composition", { sentence, target }).catch((e: unknown) => { console.error("propose_composition failed", e); throw e; });
  }
  compose(composition: Composition): Promise<ComposeResult> {
    return this.core.invoke<ComposeResult>("compose", { composition }).catch((e: unknown) => { console.error("compose failed", e); throw e; });
  }
  fireRuleEvent(event: string, subject: string | null, selected: string | null): Promise<RuleDebugInfo> {
    return this.core.invoke<RuleDebugInfo>("fire_rule_event", { event, subject, selected }).catch((e: unknown) => { console.error("fire_rule_event failed", e); throw e; });
  }
  ruleDebug(id: string | null): Promise<RuleDebugInfo> {
    return this.core.invoke<RuleDebugInfo>("rule_debug", { id }).catch((e: unknown) => { console.error("rule_debug failed", e); throw e; });
  }
  ruleScrub(frame: number, selected: string | null): Promise<RuleDebugInfo> {
    return this.core.invoke<RuleDebugInfo>("rule_scrub", { frame, selected }).catch((e: unknown) => { console.error("rule_scrub failed", e); throw e; });
  }
}

// ── dev / test transport: the in-process MockCore + the framed DeltaClient (the unchanged M2.5 path) ────
const CAPS = ["Health", "Shield", "Click", "Damage", "Light"];

/** The dev/test **first-run** scene (M10.10 / C10) — a small, *named*, meaningful starter scene (NOT the
 *  5k perf fixture): a real project the dev view + the Playwright/Vitest review drive, with one requirer
 *  (the Health Bar's `Socket`) and a matching provider (the Player's `Provides`) so bind-by-intent
 *  (north-star #1) is demonstrable. The `buildWorld` 5k fixture below is for the perf / selective-re-render
 *  tests ONLY — a fresh project must never open onto 5,000 anonymous "Entity N" rows. */
function sampleScene(): EntityProjection[] {
  // The REAL `/core` vocabulary (M10.10 closeout): a requirer carries a `HealthBar` marker (it *requires*
  // Health — a cap, not a projected field); a provider carries `Health{hp,maxHp}`; everything has
  // `Transform{x,y,z}`; renderable things carry `MeshRenderer{mesh}`. So the React panels are written once,
  // against this vocabulary, and are correct on both the dev MockCore and the live `/core`.
  return [
    { id: "health-bar", name: "Health Bar", parentId: null, components: { Transform: { x: 0, y: 2, z: 0 }, HealthBar: { width: 1 } } },
    { id: "player", name: "Player", parentId: null, components: { Transform: { x: 0, y: 0, z: 0 }, Health: { hp: 100, maxHp: 100 }, MeshRenderer: { mesh: "player" } } },
    { id: "medkit", name: "Medkit", parentId: null, components: { Transform: { x: 2, y: 0, z: 1 }, Health: { hp: 50, maxHp: 50 }, MeshRenderer: { mesh: "medkit" } } },
    { id: "ground", name: "Ground", parentId: null, components: { Transform: { x: 0, y: -1, z: 0 }, MeshRenderer: { mesh: "ground" } } },
    { id: "camera", name: "Camera", parentId: null, components: { Transform: { x: -2, y: 0, z: 4 } } },
  ];
}

/** Dev-only catalog kinds the MockClient's describe resolves LOCALLY (the match→place path); anything
 *  else falls through to the opt-in generate seam. The real tiered resolver runs under Tauri. */
const MOCK_KINDS = ["HealthBar", "Button"];
function matchCatalogKind(query: string): string | null {
  const norm = (s: string) => s.toLowerCase().replace(/[^a-z0-9]/g, "");
  const q = norm(query);
  if (!q) return null;
  return MOCK_KINDS.find((k) => q.includes(norm(k))) ?? null;
}

/** The PERF fixture (deterministic 5k scene) — used by the selective-re-render / scale tests ONLY, never
 *  as the first-run project (C10). Exported so a perf test can seed it explicitly. */
export function buildWorld(n: number): EntityProjection[] {
  const out: EntityProjection[] = [];
  let seed = 0x9e3779b9;
  const rnd = () => ((seed = (seed * 1664525 + 1013904223) >>> 0) / 0xffffffff);
  for (let i = 0; i < n; i++) {
    const components: EntityProjection["components"] = {
      Transform: { x: Math.round(rnd() * 100), y: Math.round(rnd() * 100), z: 0 },
    };
    if (i % 7 === 0) components.Material = { color: "#88ccff", metalness: 0.2 };
    if (i % 5 === 0) components.Provides = { capability: CAPS[i % CAPS.length] };
    if (i % 11 === 0) components.Socket = { accepts: CAPS[(i + 1) % CAPS.length] };
    if (i % 13 === 0) components.Targeting = { target: "" };
    out.push({ id: `e${i}`, name: `Entity ${i}`, parentId: i === 0 ? null : "e0", components });
  }
  return out;
}

/** The dev/test client: a framed `DeltaClient` for edits + minimal store-derived query mocks so
 *  `npm run dev` still renders the reveal/describe surfaces without a live core. (Vitest tests inject
 *  their own stubbed `EditorClient`; the real reveal/describe come from the shell commands under Tauri.) */
class MockClient implements EditorClient {
  private balance = 100;
  private project: ProjectInfo = { path: null, dirty: false, recents: [], error: null };
  private playInfo: PlayInfo = { playing: false, paused: false };
  private placeSeq = 0;
  private saveSeq = 0;
  // M12.2 (dev MockCore): authored machines are kept in-memory so the state-graph actually RENDERS in
  // `npm run dev` (a graph needs data — unlike the inert rules mock). The real validation/no-dangling +
  // undo + reload are the live `.exe` path; this dev mock stores + returns, it does not validate.
  private machines: StateMachineInfo[] = [];
  private smSeq = 0;
  // M12.5 (dev MockCore): a tiny deterministic Rules-in-Play stub so the truth-state debugger RENDERS in
  // `npm run dev` (the panel needs a running model). `ruleKills` = EnemyDied events fired so far (the live
  // head); the canonical sword ignites at 4. This dev stub stores + projects; the real runtime + determinism
  // + scrub-replay are the live `.exe`/headless path (`core::rule_runtime`).
  private ruleKills = 0;
  constructor(
    private readonly inner: DeltaClient,
    private readonly core: MockCore,
  ) {}

  /** Place a pre-componentized entity into the scene through the SAME delta path the real core uses
   *  (`MockCore.push` → committed `ProjectionDelta` → the projection store), so describe/generate/place
   *  actually CLOSE THE LOOP in the dev view (C1): the entity exists in the authoritative mock base (a
   *  later edit won't reject) AND streams into the store. Returns the created id so the caller selects it. */
  private place(name: string, components: Record<string, Record<string, Json>>): string {
    this.placeSeq += 1;
    const id = `new-${this.placeSeq}`;
    const ops: ProjectionOp[] = [{ op: "upsert", id, name, parentId: null }];
    for (const [c, fields] of Object.entries(components)) {
      for (const [f, v] of Object.entries(fields)) {
        ops.push({ op: "setField", id, component: c, field: f, value: v });
      }
    }
    this.core.push(ops);
    return id;
  }

  setField(id: string, component: string, field: string, value: Json): string {
    return this.inner.setField(id, component, field, value);
  }
  bind(from: string, rel: string, to: string): string {
    return this.inner.bind(from, rel, to);
  }
  onEphemeral(cb: (data: Json) => void): () => void {
    return this.inner.onEphemeral(cb);
  }
  revealTargets(id: string): Promise<RevealResponse> {
    // Dev stand-in for the live compat query (the real reveal is a command): a requirer (a `HealthBar`,
    // which requires Health) reveals the Health providers (entities carrying a `Health` component) as
    // ranked compatible targets. Real vocabulary, so the panel behaves the same on the live `/core`.
    const s = projectionStore.getState();
    const sel = s.displayed[id];
    const isRequirer = !!sel && "HealthBar" in sel.components;
    // Reflect existing (optimistic or confirmed) outgoing edges so a bind moves the target into
    // "tracking" — the dev stand-in previously returned bound:[] always, so binding showed nothing.
    const boundIds = new Set(Object.values(s.edges).filter((e) => e.from === id).map((e) => e.to));
    const providers = s.order
      .map((eid) => s.displayed[eid])
      .filter((e): e is EntityProjection => !!e && e.id !== id && "Health" in e.components);
    const bound = providers.filter((e) => boundIds.has(e.id)).map((e) => ({ id: e.id, name: e.name, kind: "tracks" }));
    const compatible = isRequirer
      ? providers.filter((e) => !boundIds.has(e.id)).slice(0, 8).map((e, i) => ({ id: e.id, name: e.name, distance: i, affinity: 100 - i * 5 }))
      : [];
    return Promise.resolve({ required: isRequirer ? ["Health"] : [], compatible, greyed: [], bound });
  }
  describe(query: string): Promise<DescribeResponse> {
    // The dev stand-in for the tiered resolver (ADR-012): a query that names a catalog kind resolves
    // LOCALLY and is PLACED + returned (match → place + select); anything else returns the opt-in generate
    // seam (no placeholder — the real backend's tiers run under Tauri). Closing the loop in the dev view is
    // what lets the Playwright/Vitest review re-drive C1 end-to-end (the bar then selects the created id).
    const kind = matchCatalogKind(query);
    if (kind) {
      // A HealthBar resolves as a real requirer (HealthBar marker); other kinds as a renderable.
      const comps: Record<string, Record<string, Json>> =
        kind === "HealthBar"
          ? { Transform: { x: 0, y: 0, z: 0 }, HealthBar: { width: 1 } }
          : { Transform: { x: 0, y: 0, z: 0 }, MeshRenderer: { mesh: kind } };
      const id = this.place(kind, comps);
      return Promise.resolve({ created: id, kind, source: "local", price: null, seam: null, balance: this.balance });
    }
    return Promise.resolve({ created: null, kind: null, source: null, price: null, seam: "generate", balance: this.balance });
  }
  walletInfo(): Promise<EconResponse> {
    return Promise.resolve({ ok: true, balance: this.balance, cost: null, message: null });
  }
  topUp(): Promise<EconResponse> {
    this.balance += 100;
    return Promise.resolve({ ok: true, balance: this.balance, cost: 100, message: null });
  }
  aiEdit(id: string, material?: string): Promise<EconResponse> {
    if (this.balance < 2) {
      return Promise.resolve({ ok: false, balance: this.balance, cost: null, message: "insufficient balance" });
    }
    this.balance -= 2;
    // Apply a VISIBLE result (C3 — "always show what changed"): the real AI-edit patches
    // `MeshRenderer.material` (ADR-017/041); the dev stand-in mirrors that, so the inspector reflects it.
    this.core.push([{ op: "setField", id, component: "MeshRenderer", field: "material", value: material ?? "rusty" }]);
    return Promise.resolve({ ok: true, balance: this.balance, cost: 2, message: null });
  }
  generate(query: string): Promise<GenerateResponse> {
    // Tier 3, opt-in. Reserve the cost; if broke, refuse-explained (no placeholder, no debit). Else place
    // the generated object (the dev stand-in for the placeholder-first stream-in) + debit, returning the
    // created id so the bar places + selects it — the closed loop the real backend streams in over Channel.
    if (this.balance < GENERATE_COST) {
      return Promise.resolve({
        created: null,
        cost: null,
        available: true,
        seam: `insufficient balance: a generation costs ${GENERATE_COST} tokens, you have ${this.balance} — top up?`,
        balance: this.balance,
      });
    }
    this.balance -= GENERATE_COST;
    const name = query.trim() ? query.trim().slice(0, 40) : "Generated object";
    const id = this.place(name, {
      Transform: { x: 0, y: 0, z: 0 },
      MeshRenderer: { mesh: "gen:mock", material: "default" },
    });
    return Promise.resolve({ created: id, cost: GENERATE_COST, available: true, seam: null, balance: this.balance });
  }
  undo(): Promise<boolean> {
    /* the dev MockCore has no undo stack — a no-op; resolves false so the UI says "nothing to undo" honestly */
    return Promise.resolve(false);
  }
  entityActions(id: string): Promise<ActionItem[]> {
    const e = projectionStore.getState().displayed[id];
    const canBind = !!e?.components.HealthBar; // a requirer (HealthBar) has an unmet requirement to bind
    return Promise.resolve([
      { action: "bind", label: "Bind…", available: canBind, reason: canBind ? undefined : "no unmet requirement to bind", mutates: false },
      { action: "remove", label: "Remove", available: true, mutates: true },
      { action: "duplicate", label: "Duplicate", available: true, mutates: true },
      { action: "focus", label: "Focus", available: true, mutates: false },
      { action: "inspect", label: "Inspect", available: true, mutates: false },
    ]);
  }
  entityDetails(id: string): Promise<EntityDetails | null> {
    const e = projectionStore.getState().displayed[id];
    if (!e) return Promise.resolve(null);
    const c = e.components;
    return Promise.resolve({
      id,
      name: e.name,
      components: Object.keys(c),
      provides: "Health" in c ? ["Health"] : [],
      requires: "HealthBar" in c ? ["Health"] : [],
      boundTo: [],
    });
  }
  setJoint(): Promise<boolean> {
    return Promise.resolve(false); // joints ride the real core (kinematic solve is engine-side)
  }
  jointKey(): Promise<boolean> {
    return Promise.resolve(false);
  }
  jointValue(): Promise<boolean> {
    return Promise.resolve(false);
  }
  jointScrub(): Promise<number> {
    return Promise.resolve(0);
  }
  jointInfo(): Promise<JointInfo | null> {
    return Promise.resolve(null);
  }
  cadReport(): Promise<CadReport> {
    // Dev stand-in: derive the report from the projection's persisted CadPart components (empty until a
    // CAD file is imported, which only happens under the real Tauri core — so this is normally all zeros).
    const r: CadReport = { total: 0, exactBrep: 0, tessellationOnly: 0, aiReconstructed: 0, proxy: 0, accessDenied: 0, failed: 0, parts: [] };
    for (const [id, e] of Object.entries(projectionStore.getState().displayed)) {
      const fidelity = e.components["CadPart"]?.["fidelity"];
      if (typeof fidelity !== "string") continue;
      r.total++;
      if (fidelity === "exact-brep") r.exactBrep++;
      else if (fidelity === "tessellation-only") r.tessellationOnly++;
      else if (fidelity === "ai-reconstructed") r.aiReconstructed++;
      else if (fidelity === "proxy") r.proxy++;
      else if (fidelity === "access-denied") r.accessDenied++;
      else r.failed++;
      if (r.parts.length < 500) r.parts.push({ id, name: e.name, fidelity });
    }
    return Promise.resolve(r);
  }
  removeEntity(_id: string): void {}
  duplicateEntity(_id: string): Promise<string | null> {
    return Promise.resolve(null);
  }
  focusEntity(_id: string): void {}
  makeDynamic(_id: string): Promise<boolean> {
    return Promise.resolve(true);
  }
  // ── M10.6 scene-authoring verbs — the real undoable commits run under Tauri (proven by the .exe gate);
  // the dev MockCore stubs are inert+deterministic so the menu/hierarchy render without a live core. ──
  createEntity(): Promise<string | null> {
    return Promise.resolve(null);
  }
  addLight(): Promise<string | null> {
    return Promise.resolve(null);
  }
  renameEntity(): Promise<boolean> {
    return Promise.resolve(true);
  }
  groupEntities(): Promise<string | null> {
    return Promise.resolve(null);
  }
  ungroupEntity(): Promise<boolean> {
    return Promise.resolve(true);
  }
  multiEdit(): Promise<boolean> {
    return Promise.resolve(true);
  }
  deleteDeactivate(): Promise<boolean> {
    return Promise.resolve(true);
  }
  copySubtree(): void {}
  cutSubtree(): Promise<boolean> {
    return Promise.resolve(true);
  }
  pasteClipboard(): Promise<string | null> {
    return Promise.resolve(null);
  }
  // M8 physics / M9 transform / M3.3 focus are Tauri-only (the dev MockCore has no sim/gizmo/native camera)
  // — inert, deterministic stubs so the panels render + the dev view never throws. The live behavior is
  // proven by the real-`.exe` acceptance gate (physics/transform/context-actions specs).
  spawnBody(): Promise<string | null> {
    return Promise.resolve(null);
  }
  setSimRunning(): void {}
  simOverlay(): void {}
  simTimeline(): Promise<TimelineTuple> {
    return Promise.resolve([0, 0, false, false, 0]);
  }
  simScrub(): Promise<TimelineTuple> {
    return Promise.resolve([0, 0, false, false, 0]);
  }
  simShove(): Promise<boolean> {
    return Promise.resolve(false);
  }
  physicsContacts(): Promise<ContactInfo[]> {
    return Promise.resolve([]);
  }
  physicsCheck(): Promise<PhysicsWarning[]> {
    return Promise.resolve([]);
  }
  physicsFix(): Promise<boolean> {
    return Promise.resolve(false);
  }
  importInterchange(): Promise<ImportResult> {
    return Promise.resolve({ ok: false, format: "", bodies: 0, joints: 0, meters_per_unit: 1, kilograms_per_unit: 1, reconciled: false, notes: [], error: "import is live-only (the .exe)" });
  }
  gizmoMode(): void {}
  gizmoSelect(): Promise<boolean> {
    return Promise.resolve(false);
  }
  gizmoSelected(): Promise<string | null> {
    return Promise.resolve(null);
  }
  gizmoDebug(): Promise<[string, boolean, boolean, string, string]> {
    return Promise.resolve(["translate", false, false, "world", "origin"]);
  }
  gizmoSpaceToggle(): Promise<string> {
    return Promise.resolve("world");
  }
  gizmoPivotToggle(): Promise<string> {
    return Promise.resolve("origin");
  }
  gizmoPickDrag(): Promise<boolean> {
    return Promise.resolve(false);
  }
  gizmoDragEnd(): void {}
  readTransform(): Promise<number[]> {
    return Promise.resolve([0, 0, 0, 0, 0, 0, 1, 1]);
  }
  saveCharacter(): Promise<string | null> {
    return Promise.resolve(null);
  }
  instantiateCharacter(): Promise<string | null> {
    return Promise.resolve(null);
  }
  setPartActive(): Promise<boolean> {
    return Promise.resolve(true);
  }
  reparentPart(): void {}
  setSnap(): void {}
  snapQuery(): Promise<SnapHit[]> {
    return Promise.resolve([]);
  }
  applyConstraint(): Promise<SolveResult> {
    return Promise.resolve({ ok: false, reason: "constraints are live-only (the .exe)", intents: [] });
  }
  placementSentence(): Promise<SolveResult> {
    return Promise.resolve({ ok: false, reason: "placement is live-only (the .exe)", intents: [] });
  }
  unfocus(): void {}
  focusDebug(): Promise<[number, boolean]> {
    return Promise.resolve([20, true]); // ≤40 so the dev view's focus read is consistent
  }
  frameAll(): void {}
  viewPreset(): void {}
  cameraDebug(): Promise<number[]> {
    return Promise.resolve([0.785, 0.5, 60, 0, 0, 0]);
  }
  // The dev MockCore has no native viewport — these are inert (the real wgpu input is Tauri-only).
  viewportPick(_x: number, _y: number): Promise<string | null> {
    return Promise.resolve(null);
  }
  dragStart(): void {}
  dragEnd(): void {}
  zoom(_delta: number): void {}
  // The dev/browser MockCore has no native wgpu surface → no live render. Resolve null so the thumbnail store
  // falls back to the styled type-icon (the documented ADR-006/ADR-058 divergence; real RTT pixels = the .exe).
  thumbnail(_id: string, _size: number): Promise<string | null> {
    return Promise.resolve(null);
  }
  catalog(): Promise<Record<string, CatalogItem[]>> {
    const item = (id: string, label: string, category: string, source: string): CatalogItem => ({
      id, label, bucket: category, category, source, provides: [], requires: [],
    });
    return Promise.resolve({
      Health: [item("HealthBar", "HealthBar", "Health", "local")],
      UI: [item("Button", "Button", "UI", "local")],
    });
  }
  catalogSearch(query: string): Promise<CatalogSearch> {
    return this.catalog().then((groups) => {
      const all = Object.values(groups).flat();
      const items = all.filter((i) => i.label.toLowerCase().includes(query.toLowerCase()));
      return { items, seam: items.length === 0 ? "generate" : undefined };
    });
  }
  addItem(id: string, source: string): Promise<AddResponse> {
    // Place-into-scene (the dev stand-in): instantiate the catalog item as a real entity (so the asset
    // browser's place ACTUALLY places + the caller selects it — the closed loop). A marketplace source
    // debits; local is free.
    const created = this.place(id, { Transform: { x: 0, y: 0, z: 0 }, MeshRenderer: { mesh: id } });
    let balance: number | null = null;
    if (source === "marketplace") {
      this.balance = Math.max(0, this.balance - 2);
      balance = this.balance;
    }
    return Promise.resolve({ created, balance, seam: null });
  }
  // Import is live-only (the native MAGIC router + ufbx FFI + the file dialog) — inert in the dev MockCore.
  importAsset(): Promise<string | null> {
    return Promise.resolve(null);
  }
  importAssetDialog(): Promise<string | null> {
    return Promise.resolve(null);
  }

  // The dev MockCore has no real document; track a plausible in-memory project so the File menu renders.
  projectState(): Promise<ProjectInfo> {
    return Promise.resolve({ ...this.project });
  }
  newProject(): Promise<ProjectInfo> {
    this.project = { path: null, dirty: false, recents: this.project.recents, error: null };
    return Promise.resolve({ ...this.project });
  }
  openProject(path?: string): Promise<ProjectInfo> {
    const p = path ?? "untitled.mtk";
    this.project = {
      path: p,
      dirty: false,
      recents: [p, ...this.project.recents.filter((r) => r !== p)].slice(0, 8),
      error: null,
    };
    return Promise.resolve({ ...this.project });
  }
  saveProject(): Promise<ProjectInfo> {
    // Honest save (C9): an UNTITLED project has no path — the FileMenu routes its Save → Save As, but guard
    // here too (never report "saved" on an unnamed doc by inventing "untitled.mtk"). A titled doc re-saves.
    if (!this.project.path) return this.saveProjectAs();
    this.project = { ...this.project, dirty: false, error: null };
    return Promise.resolve({ ...this.project });
  }
  saveProjectAs(): Promise<ProjectInfo> {
    // Save As always assigns a NEW name (the shell's native Save dialog on the `.exe`; a deterministic
    // stand-in here) — so the title can reflect the real filename afterward.
    this.saveSeq += 1;
    const p = this.saveSeq === 1 ? "my-project.mtk" : `my-project-${this.saveSeq}.mtk`;
    this.project = {
      path: p,
      dirty: false,
      recents: [p, ...this.project.recents.filter((r) => r !== p)].slice(0, 8),
      error: null,
    };
    return Promise.resolve({ ...this.project });
  }

  play(): Promise<PlayInfo> {
    this.playInfo = { playing: true, paused: false };
    return Promise.resolve({ ...this.playInfo });
  }
  stop(): Promise<PlayInfo> {
    this.playInfo = { playing: false, paused: false };
    return Promise.resolve({ ...this.playInfo });
  }
  pause(): Promise<PlayInfo> {
    if (this.playInfo.playing) this.playInfo = { playing: true, paused: !this.playInfo.paused };
    return Promise.resolve({ ...this.playInfo });
  }
  playState(): Promise<PlayInfo> {
    return Promise.resolve({ ...this.playInfo });
  }
  // M12.1 Rules (dev MockCore): a representative registry-fed vocabulary so the builder renders in `npm run
  // dev`; authoring is inert here (the real list + validation + mirror are the live `.exe` path).
  ruleRegistry(): Promise<RuleRegistryInfo> {
    return Promise.resolve({
      events: [
        { name: "EnemyDied", description: "an enemy was defeated" },
        { name: "StateEntered", description: "a quest/state machine entered a state" },
        { name: "StateExited", description: "a quest/state machine left a state" },
      ],
      actions: [
        { name: "SetField", description: "set a component field to a value" },
        { name: "AdjustCounter", description: "add a number to a numeric counter field" },
      ],
      components: [
        { name: "KillCounter", fields: [{ name: "count", ty: "integer" }] },
        { name: "QuestState", fields: [{ name: "state", ty: "string" }] },
        { name: "Flammable", fields: [{ name: "lit", ty: "boolean" }] },
      ],
    });
  }
  listRules(): Promise<RuleSummary[]> {
    return Promise.resolve([]);
  }
  authorRule(): Promise<AuthorRuleResult> {
    return Promise.resolve({ id: "rule-dev", error: null, mirror: null });
  }
  deleteRule(): Promise<boolean> {
    return Promise.resolve(true);
  }
  // M12.2 state machines (dev MockCore): stateful so the state-graph renders in `npm run dev`. Each new
  // (empty-id) transition gets a dev edge id; `current` defaults to `initial` (the M12.5 seam).
  stateMachines(): Promise<StateMachineInfo[]> {
    return Promise.resolve(this.machines.map((m) => ({ ...m, machine: { ...m.machine } })));
  }
  authorStateMachine(sm: StateMachine, id: string | null = null): Promise<AuthorStateMachineResult> {
    const machine: StateMachine = {
      ...sm,
      transitions: sm.transitions.map((t) => (t.id ? t : { ...t, id: `t-dev-${this.smSeq++}` })),
    };
    const smId = id ?? `sm-dev-${this.smSeq++}`;
    const info: StateMachineInfo = { id: smId, current: machine.initial, machine };
    const at = this.machines.findIndex((m) => m.id === smId);
    if (at >= 0) this.machines[at] = info;
    else this.machines.push(info);
    return Promise.resolve({ id: smId, error: null, unreachable: [] });
  }
  deleteStateMachine(id: string): Promise<boolean> {
    this.machines = this.machines.filter((m) => m.id !== id);
    return Promise.resolve(true);
  }
  // M12.4 AI compose (dev MockCore): mirrors the DemoComposer — the ignite-on-kills intent composes; an
  // unrecognized sentence / no target is an honest miss; compose echoes the op/rule counts (so the panel
  // works in `npm run dev` + vitest without the real engine).
  proposeComposition(sentence: string, target: string | null): Promise<ComposeProposal> {
    const s = sentence.toLowerCase();
    const onKill = /die|dies|kill|defeat|slain/.test(s);
    const ignite = /fire|ignite|burn|flam|lit/.test(s);
    if (!onKill || !ignite) {
      return Promise.resolve({ ok: false, composition: null, ops: 0, error: `the offline demo composer doesn't recognize "${sentence}" — try "when an enemy dies, set it on fire"` });
    }
    if (!target) {
      return Promise.resolve({ ok: false, composition: null, ops: 0, error: "select the entity the rule should act on first" });
    }
    const threshold = Number((s.match(/\d+/) ?? ["4"])[0]);
    const composition: Composition = {
      ops: [
        { op: "setField", entity: target, component: "KillCounter", field: "count", value: { Integer: 0 } },
        {
          op: "authorRule",
          id: "r_ai_ignite",
          rule: {
            name: "ignite on kills",
            enabled: true,
            event: "EnemyDied",
            conditions: [{ entity: target, component: "KillCounter", field: "count", op: "ge", value: { Integer: threshold } }],
            actions: [{ action: "SetField", entity: target, component: "Flammable", field: "lit", value: { Bool: true } }],
          },
        },
      ],
    };
    return Promise.resolve({ ok: true, composition, ops: composition.ops.length, error: null });
  }
  compose(composition: Composition): Promise<ComposeResult> {
    const rules = composition.ops.filter((o) => o.op === "authorRule").length;
    return Promise.resolve({ ok: true, applied: composition.ops.length, rules, stateMachines: 0, error: null });
  }
  // M12.5 (dev MockCore): a deterministic Rules-in-Play stub — fire EnemyDied → the sword's KillCounter
  // climbs toward 4; click it → the truth-state shows "❌ KillCounter = N of 4" + "✅ state = FacingBoss"
  // until the 4th kill ignites it. Scrub recomputes from the head. A projection only (no doc mutation).
  fireRuleEvent(_event: string, _subject: string | null, selected: string | null): Promise<RuleDebugInfo> {
    this.ruleKills += 1;
    return Promise.resolve(this.ruleDebugAt(this.ruleKills, selected));
  }
  ruleDebug(id: string | null): Promise<RuleDebugInfo> {
    return Promise.resolve(this.ruleDebugAt(this.ruleKills, id));
  }
  ruleScrub(frame: number, selected: string | null): Promise<RuleDebugInfo> {
    return Promise.resolve(this.ruleDebugAt(Math.min(frame, this.ruleKills), selected));
  }
  /** Build the dev truth-state at `frame` kills (the head is `this.ruleKills`). */
  private ruleDebugAt(frame: number, selected: string | null): RuleDebugInfo {
    const lit = frame >= 4;
    const decisions: DecisionEvent[] = [];
    for (let k = 1; k <= frame; k++) {
      decisions.push({ frame: k - 1, kind: "counterChanged", entity: selected ?? "1_0", component: "KillCounter", field: "count", from: { Integer: k - 1 }, to: { Integer: k } });
      if (k === 4) decisions.push({ frame: k - 1, kind: "fieldSet", entity: selected ?? "1_0", component: "Flammable", field: "lit", value: { Bool: true } });
    }
    const truth: TruthState | null = selected
      ? {
          entity: selected,
          rules: [
            {
              rule: "r_ignite",
              name: "rusty sword ignites",
              event: "EnemyDied",
              fires: lit,
              conditions: [
                { satisfied: lit, entity: selected, component: "KillCounter", field: "count", actual: { Integer: frame }, expected: { Integer: 4 }, display: `KillCounter = ${frame} of 4` },
                { satisfied: true, entity: selected, component: "Zone", field: "current", actual: { Str: "BossArena" }, expected: { Str: "BossArena" }, display: "Zone.current = BossArena (want to be exactly BossArena)" },
              ],
            },
          ],
          machines: [{ machine: "sm_quest", name: "quest", field: "state", current: "FacingBoss", display: "state = FacingBoss" }],
        }
      : null;
    const explanations: RuleExplain[] = lit
      ? [{ rule: "r_ignite", text: "'rusty sword ignites' is ready — every condition holds, so it fires on EnemyDied" }]
      : [{ rule: "r_ignite", text: `'rusty sword ignites' is blocked: KillCounter.count is ${frame}, but the rule needs to be at least 4 (waiting on EnemyDied)` }];
    return { playing: true, frame, head: this.ruleKills, truth, explanations, decisions, flagged: [] };
  }
}

function mockSession(): EditorClient {
  const [uiT, coreT] = inProcessPair();
  // The dev/test first-run = the small NAMED sample scene (C10), not the 5k perf fixture. `buildWorld`
  // stays exported for the perf / selective-re-render tests that seed it explicitly.
  const core = new MockCore(coreT, sampleScene());
  const client = new DeltaClient(uiT);
  core.emitScene();
  return new MockClient(client, core);
}

/** Build the editor session: the real Tauri shell transport inside the WebView, else the dev MockCore. */
export function createSession(): EditorClient {
  const core = tauriCore();
  return core ? new TauriClient(core) : mockSession();
}
