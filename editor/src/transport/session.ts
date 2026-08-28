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
  AuthoredMatch,
  CatalogItem,
  CookedMatch,
  MatchActor,
  MatchStatus,
  MatchValidation,
  CatalogSearch,
  CadReport,
  ReimportReport,
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
  MultiEditResult,
  JsonPatch,
  PhysicsWarning,
  ProjectionDelta,
  ProjectionOp,
  RevealResponse,
  AuthorRuleResult,
  RuleData,
  RuleRegistryInfo,
  RuleSummary,
  ConditionSpec,
  ShotSpec,
  CinemaReply,
  EffectSpec,
  VfxReply,
  VfxProbe,
  CameraProbe,
  ColourStatus,
  FormatSpec,
  ClauseRequest,
  ConditionListInfo,
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
  PipeForgeOptions,
  PipeForgeStatus,
  PipeBakeReport,
  PipeFittingKind,
  UserFittingCatalogEntry,
  AssetLabProcessRequest,
  AssetLabResponse,
  SceneExportFormat,
  SceneExportResponse,
  AnimationWorkspaceInfo,
  AnimationClipInstanceSaveRequest,
  AnimationClipInstanceSaveResult,
  AnimationClipTargetMapping,
  AnimationEditResult,
  AnimationPlaybackInfo,
  AnimationInterpolation,
  AnimationLoopPolicy,
  AnimationTrackInfo,
  AnimationKeyUpdateInfo,
  AnimationKeyDeleteInfo,
  AnimationGraphStateInfo,
  AnimationGraphSaveRequest,
  AnimationGraphSaveResult,
  AnimationGraphDeleteResult,
  AnimationGraphDebugInfo,
  AnimationGraphPreviewResult,
  AnimationGraphValue,
  TerrainPreset,
  TerrainRecipe,
  TerrainPlan,
  TerrainReading,
  TerrainReply,
  TerrainPathResult,
  TerrainStats,
  ShapeSpec,
  ShapeReply,
  RoleSpec,
  RoleReply,
  RoleStatusInfo,
} from "./protocol";
import { ANIMATION_GRAPH_SCHEMA_VERSION, GENERATE_COST } from "./protocol";
import { DeltaClient } from "./client";
import { MockCore } from "./mock-core";
import { inProcessPair } from "./transport";

/** The one client surface the React UI talks to (the real shell transport + the dev MockCore both satisfy it). */
export type ViewportRenderProfile = "cinematic" | "cad";

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
  /** Reapply the most recently undone committed transaction. */
  redo(): Promise<boolean>;
  /** The context-menu actions for an entity (M3.3) — each available-or-explained. */
  entityActions(id: string): Promise<ActionItem[]>;
  /** The hover-tooltip details for an entity (M3.3) — name + components + caps + bound. */
  entityDetails(id: string): Promise<EntityDetails | null>;
  /** The per-part CAD import report (M15.7) — the fidelity breakdown + a capped part list, off the ECS. */
  cadReport(): Promise<CadReport>;
  /** M15.10 — the last import's re-import diff (matched/added/removed/adjudicate + orphans + held matches). */
  cadReimportReport(): Promise<ReimportReport>;
  /** M15.10 — resolve a held low-confidence match: accept re-binds its overrides onto the matched new entity
   *  (one undoable commit), reject discards. Returns the updated report. */
  cadReimportResolve(oldId: string, accept: boolean): Promise<ReimportReport>;
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
  /** Read the compiled animation timeline, keyable properties and measured asset capabilities. */
  animationState(id: string | null): Promise<AnimationWorkspaceInfo>;
  /** Validate, persist and compile one explicit imported rigid-clip mapping. */
  animationClipInstanceSave(request: AnimationClipInstanceSaveRequest): Promise<AnimationClipInstanceSaveResult>;
  /** Revision-fenced discard of one explicitly chosen persisted clip instance. */
  animationClipInstanceDelete(instanceId: string, expectedRevision: string, selectedId: string | null): Promise<AnimationClipInstanceSaveResult>;
  /** Compile and play the exact mapping draft without changing document history or instance revision. */
  animationClipInstancePreview(request: AnimationClipInstanceSaveRequest): Promise<AnimationPlaybackInfo>;
  /** Tear down an unsaved imported-clip audition and restore authored preview authority.
   * When supplied, `expectedRequestId` makes delayed cleanup incapable of stopping a newer audition. */
  animationClipInstancePreviewStop(expectedRequestId?: string): Promise<AnimationPlaybackInfo>;
  /** Key the selected entity's current property value as one undoable, mergeable authored operation. */
  animationKey(id: string, component: string, property: string, tick: number, interpolation: AnimationInterpolation): Promise<AnimationEditResult>;
  /** Remove one stable keyframe without replacing the rest of its track. */
  animationDeleteKey(id: string, trackId: string, keyId: string): Promise<AnimationEditResult>;
  /** Remove multiple keys atomically as one native transaction and one undo step. */
  animationDeleteKeys(id: string | null, deletes: AnimationKeyDeleteInfo[]): Promise<AnimationEditResult>;
  /** Change one track's interpolation independently of its keyframe fields. */
  animationSetInterpolation(id: string, trackId: string, interpolation: AnimationInterpolation): Promise<AnimationEditResult>;
  /** Mute/unmute a track while preserving all authored keys. */
  animationSetTrackEnabled(id: string, trackId: string, enabled: boolean): Promise<AnimationEditResult>;
  /** Lock/unlock collaborative authoring for one track. */
  animationSetTrackLocked(id: string, trackId: string, locked: boolean): Promise<AnimationEditResult>;
  /** Commit a multi-key drag/value/tangent edit as one undo step. */
  animationUpdateKeys(id: string, trackId: string, updates: AnimationKeyUpdateInfo[]): Promise<AnimationEditResult>;
  animationAddMarker(id: string, name: string, tick: number): Promise<AnimationEditResult>;
  animationDeleteMarker(ownerId: string, markerId: string): Promise<AnimationEditResult>;
  animationAddEvent(id: string, name: string, tick: number, payload?: Json): Promise<AnimationEditResult>;
  animationDeleteEvent(ownerId: string, eventId: string): Promise<AnimationEditResult>;
  /** Control render-only animation preview. Playback never writes sampled values into the document. */
  animationTransport(action: "play" | "pause" | "stop" | "scrub", tick?: number, loopPolicy?: AnimationLoopPolicy): Promise<AnimationPlaybackInfo>;
  /** Lightweight clock/event read; does not reload properties, tracks, keys, or asset diagnostics. */
  animationPlaybackState(): Promise<AnimationPlaybackInfo>;
  /** Read the sequence-global authored graph, compilation state, sources, and addressable diagnostics. */
  animationGraphState(sequenceId: string): Promise<AnimationGraphStateInfo>;
  /** Validate and atomically commit one complete structural draft as one undoable backend intent. */
  animationGraphSave(sequenceId: string, request: AnimationGraphSaveRequest): Promise<AnimationGraphSaveResult>;
  animationGraphDelete(sequenceId: string, graphId: string, expectedRevision: string, requestId: string): Promise<AnimationGraphDeleteResult>;
  /** Bounded debug snapshot; poses never cross IPC. */
  animationGraphDebug(graphId: string, instanceId: string | null, watches: readonly string[]): Promise<AnimationGraphDebugInfo>;
  /** Transient preview overrides. These never dirty the authored graph or document. */
  animationGraphSetPreviewParameters(graphId: string, values: Readonly<Record<string, AnimationGraphValue>>): Promise<AnimationGraphPreviewResult>;
  animationGraphClearPreviewParameters(graphId: string): Promise<AnimationGraphPreviewResult>;
  /** Remove an entity + its edges (M3.3) — one undoable transaction (the delta streams back). */
  removeEntity(id: string): void;
  /** Duplicate an entity (M3.3) — one undoable transaction; resolves to the clone's id. */
  duplicateEntity(id: string): Promise<string | null>;
  /** Duplicate a whole SELECTION as ONE undoable transaction (ADR-169) → the clones' ids in source
   *  order. One Ctrl-Z removes all of them; N calls to `duplicateEntity` would need N. */
  duplicateEntities(ids: string[]): Promise<string[]>;
  /** Frame the camera on an entity (M3.3) — no mutation. */
  focusEntity(id: string): void;
  /**
   * Tell the renderer which fraction of the window the 3D is actually visible through.
   *
   * The wgpu surface is the whole window and this UI is composited over it, so framing had no way to
   * know that the docks hide a third of what it was aiming at. Reported on every layout change.
   */
  reportViewportRect(rect: { x: number; y: number; width: number; height: number }): void;
  /** M8.3: turn a dead mesh into a correct dynamic body — one undoable transaction. */
  makeDynamic(id: string): Promise<boolean>;
  /** Add a fixed rigid body and generated convex-hull collider as one undoable transaction. */
  makeStatic(id: string): Promise<boolean>;

  // ── M10.6 scene-authoring verbs (ADR-036) — each one undoable transaction over the Movable Tree +
  // override pipeline. reparent reuses `reparentPart`; delete=deactivate is distinct from `removeEntity`. ──
  /** Create an empty named entity at a position → its id (the caller selects it). */
  createEntity(x: number, y: number, z: number, name: string): Promise<string | null>;
  /** Start the non-destructive Pipe Forge source session; clicks are routed through `pipeForgePoint`. */
  pipeForgeStart(options: PipeForgeOptions): Promise<PipeForgeStatus>;
  /** Place a point from normalized viewport coordinates (native ray → surface/workplane → snapped route). */
  pipeForgePoint(x: number, y: number): Promise<PipeForgeStatus>;
  pipeForgeUndo(): Promise<PipeForgeStatus>;
  pipeForgeBake(): Promise<PipeBakeReport>;
  pipeForgeCancel(): Promise<PipeForgeStatus>;
  pipeForgeStatus(): Promise<PipeForgeStatus>;
  /** Restore a baked V2 recipe into the viewport without destructively replacing its current artifact. */
  pipeForgeEdit(id: string): Promise<PipeForgeStatus>;
  /** Start extending a branch from a stable route handle; subsequent viewport clicks author that branch. */
  pipeForgeBeginBranch(nodeId: number, diameterCm: number): Promise<PipeForgeStatus>;
  pipeForgeEndBranch(): Promise<PipeForgeStatus>;
  pipeForgeMoveHandle(nodeId: number, x: number, y: number, z: number): Promise<PipeForgeStatus>;
  pipeForgeRemoveHandle(nodeId: number): Promise<PipeForgeStatus>;
  pipeForgePlaceFitting(nodeId: number, kind: PipeFittingKind, catalogId?: string): Promise<PipeForgeStatus>;
  pipeForgeRemoveFitting(fittingId: number): Promise<PipeForgeStatus>;
  pipeForgeUpsertCatalog(entry: UserFittingCatalogEntry): Promise<PipeForgeStatus>;
  pipeForgeRemoveCatalog(id: string): Promise<PipeForgeStatus>;

  // ── Shape Studio (Build engine): parametric shapes · draw-to-3D · combine · meld. Every command is
  // one undoable engine transaction; a refusal (`reply.reason`) changes nothing and is explained. ──
  /** The shape catalog: kinds + plain-language parameter specs. Static — safe to read once. */
  shapeCatalog(): Promise<ShapeSpec[]>;
  /** Create a parametric shape (catalog defaults) — at `pos`, or the deterministic scatter when omitted. */
  shapeSpawn(kind: string, pos?: [number, number, number]): Promise<ShapeReply>;
  /** Re-bake the selected shape with edited parameters (one undo step; position untouched). */
  shapeUpdate(id: string, params: Record<string, number>): Promise<ShapeReply>;
  /** Turn a drawn outline into a solid: extrude a ground plan up (optionally tapered toward the
   *  centroid), or revolve a side profile around the vertical axis. */
  shapeDraw(mode: "extrude" | "revolve", profile: [number, number][], height?: number, segments?: number, taper?: number): Promise<ShapeReply>;
  /** Exact boolean of two objects (union|carve|intersect): result replaces both sources, one undo step. */
  shapeCombine(a: string, b: string, op: "union" | "carve" | "intersect"): Promise<ShapeReply>;
  /** Meld two shapes into one smooth blob (blend radius `k` metres), same replace semantics. */
  shapeMeld(a: string, b: string, k?: number): Promise<ShapeReply>;

  // ── Gameplay roles: one click from asset to live gameplay participant. Every command is one
  // undoable engine transaction; a refusal (`reply.reason`) changes nothing and is explained. ──
  /** The role catalog (label/blurb/what-it-adds). Static — safe to read once. */
  roleCatalog(): Promise<RoleSpec[]>;
  /** Assign a role: components + animation + rule + Score binding in ONE commit. */
  roleAssign(id: string, role: string): Promise<RoleReply>;
  /** Clear a role (keeps mesh + transform). */
  roleClear(id: string): Promise<RoleReply>;
  /** The roster + the score — live from the rules runtime during Play. */
  roleStatus(): Promise<RoleStatusInfo>;
  /** The player's movement axis (pressed keys, world x/z in [-1,1]). Fire-and-forget; send on CHANGE only. */
  playerInput(x: number, z: number): Promise<void>;
  /** Every format this build can read or write, with its declared fidelity. */
  formatCatalog(): Promise<FormatSpec[]>;
  colourStatus(): Promise<ColourStatus>;
  /**
   * Select the space the renderer SHADES in. Replies the accepted name; a name it does not know comes
   * back as `unknown: <what you sent>` rather than silently selecting the default.
   */
  setWorkingSpace(space: string): Promise<string>;
  /**
   * Declare what the loaded environment map's values mean. Non-linear spaces are refused with the
   * reason — an HDR panorama is radiance, so "this EXR is sRGB" is not a thing a person can mean.
   */
  setEnvironmentColourSpace(space: string): Promise<string>;
  /** What the renderer is drawing this instant (Play only; zeros otherwise). */
  vfxProbe(): Promise<VfxProbe>;
  /** Where the camera actually is this instant. */
  cameraProbe(): Promise<CameraProbe>;
  /** Every effect card the Effects block can offer. */
  vfxCatalog(): Promise<EffectSpec[]>;
  /** Add one effect layer to an object (one undoable commit). */
  vfxAdd(id: string, kind: string, trigger: string): Promise<VfxReply>;
  /** Remove one effect layer by index. */
  vfxRemove(id: string, index: number): Promise<VfxReply>;
  /** The object's effects, read back as sentences plus warnings. */
  vfxList(id: string): Promise<VfxReply>;
  /** Every shot card the Cinematics block can offer. */
  cinemaCatalog(): Promise<ShotSpec[]>;
  /** Append one shot to an object's cutscene (one undoable commit). */
  cinemaAddShot(id: string, kind: string): Promise<CinemaReply>;
  /** Remove one shot by index (one undoable commit). */
  cinemaRemoveShot(id: string, index: number): Promise<CinemaReply>;
  /** Set the cutscene's one pacing mood (one undoable commit). */
  cinemaSetMood(id: string, mood: "calm" | "normal" | "tense"): Promise<CinemaReply>;
  /** The object's cutscene, read back as sentences plus continuity warnings. */
  cinemaList(id: string): Promise<CinemaReply>;
  /** Every "only if" card the Behaviour block can offer. */
  conditionCatalog(): Promise<ConditionSpec[]>;
  /** Add one clause to an object (one undoable commit). */
  conditionAdd(id: string, request: ClauseRequest): Promise<RoleReply>;
  /** Remove one clause by index (`any` picks the OR group). */
  conditionRemove(id: string, index: number, any: boolean): Promise<RoleReply>;
  /** The object's clauses + the sentence its rule reads as. */
  conditionList(id: string): Promise<ConditionListInfo>;
  /** Measure topology, UV, material, texture and production-readiness facts for a selected mesh. */
  assetLabAudit(id: string): Promise<AssetLabResponse>;
  /** Build and place a content-addressed derivative while preserving the selected source. */
  assetLabProcess(id: string, request: AssetLabProcessRequest): Promise<AssetLabResponse>;
  /** Export a selected canonical mesh as an embedded-texture GLB. `path` is for automation. */
  assetLabExport(id: string, path?: string): Promise<AssetLabResponse>;
  /** Export the authoritative hierarchy, reusable meshes, skins and representable animation. */
  sceneExport(format: SceneExportFormat, path?: string): Promise<SceneExportResponse>;
  /** M11.3 — author a Light entity (kind = directional|point|spot) at a position with a linear RGB colour +
   *  intensity → its id. One undoable commit; the lit result is a render projection (not in the doc). */
  addLight(kind: string, x: number, y: number, z: number, r: number, g: number, b: number, intensity: number): Promise<string | null>;
  /** Rename an entity (`__meta__.name`) → applied; the projection re-reads it (inv. 1). */
  renameEntity(id: string, name: string): Promise<boolean>;
  /** Group a selection under a new parent node → the group id. */
  groupEntities(ids: string[], name: string): Promise<string | null>;
  /** Ungroup — dissolve a group (children to its parent, delete the group) → applied. */
  ungroupEntity(id: string): Promise<boolean>;
  /** Multi-edit — set one field (ANY scalar, ADR-169) on N entities as ONE batched, atomic, undoable
   *  transaction. Resolves to what it did, or to the sentence explaining why it refused. */
  multiEdit(ids: string[], component: string, field: string, value: Json): Promise<MultiEditResult>;
  /** Set a ROTATION on N entities as ONE batched, atomic, undoable transaction (ADR-172). A rotation
   *  is four stored fields and one property, so `multiEdit` — one field to N entities — cannot express
   *  it; the engine normalises the quaternion, so no caller can leave a non-rotation in the document.
   *  Resolves to what it did, or to the sentence explaining why it refused. */
  setRotation(ids: string[], quat: [number, number, number, number]): Promise<MultiEditResult>;
  /** Delete = deactivate (non-destructive; frees dependents) — undo restores → applied. */
  deleteDeactivate(id: string): Promise<boolean>;
  /** Delete a whole SELECTION as ONE undoable transaction (ADR-169) → applied. One Ctrl-Z restores
   *  all of it; N calls to `deleteDeactivate` would need N. */
  deleteDeactivateMany(ids: string[]): Promise<boolean>;
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
  /** Switch display presentation while retaining the same HDR/PBR lighting and authored materials. */
  setRenderProfile(profile: ViewportRenderProfile): Promise<ViewportRenderProfile>;
  /** Authoritative active viewport presentation profile. */
  renderProfileDebug(): Promise<ViewportRenderProfile>;

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

  // ── the authored match: author → validate → cook → run (ADR-097) ─────────────────────────────────────
  /** Validate the authored scene continuously. A read — it starts nothing and never touches the document.
   *  This is what makes the match panel's feedback live rather than start-time-only. */
  matchValidate(): Promise<MatchValidation>;
  /** Author a complete, playable starter match as ONE undoable transaction — the discoverable way in. */
  matchAuthorStarter(): Promise<AuthoredMatch>;
  /** Cook the authored scene and start a match over the live viewport. Rejects with the cook's
   *  diagnostics when the scene cannot run, so the panel can list the objects at fault. */
  matchStart(): Promise<MatchStatus>;
  /** Advance the authoritative match by `ticks` and redraw from its state. */
  matchStep(ticks: number): Promise<MatchStatus>;
  /** Order the player's hero to a lane position, exactly as a game client would. */
  matchOrderMove(xMm: number, yMm: number): Promise<MatchStatus>;
  /** Attack-move: advance and engage anything hostile noticed on the way. The order PERSISTS — this is
   *  what makes the hero keep swinging without a command per swing. */
  matchAttackMove(xMm: number, yMm: number): Promise<MatchStatus>;
  /** Lock onto one named target until it is gone. Never re-acquires. */
  matchAttackTarget(target: number): Promise<MatchStatus>;
  /** Hold position and engage whatever comes into range. */
  matchHold(): Promise<MatchStatus>;
  /** Cancel movement AND any standing order. */
  matchHalt(): Promise<MatchStatus>;
  /** Cast the hero's authored ability at one target — the first player-reachable cast in this editor. */
  matchCast(target: number): Promise<MatchStatus>;
  /** Apply crowd control to the hero — the live proof status effects run in the viewport. */
  matchStun(ticks: number): Promise<MatchStatus>;
  /** Read the match state without mutating it. */
  matchStatus(): Promise<MatchStatus>;
  /** The cooked definitions the running match was built from — the inspectable middle of the chain. */
  matchCooked(): Promise<CookedMatch | null>;
  /** End the match and restore the authored scene verbatim. */
  matchStop(): Promise<MatchStatus>;
  /** The terrain presets a picker offers (M19 / ADR-104). */
  terrainPresets(): Promise<TerrainPreset[]>;
  /** Create a terrain from a preset — one undoable transaction. */
  terrainCreate(preset: string): Promise<TerrainReply>;
  /** Build (or rebuild) the terrain a plain-language description asks for — one undoable transaction. */
  terrainDescribe(text: string): Promise<TerrainReply>;
  /** Read a description without building anything — safe to call while the author is still typing. */
  terrainReadDescription(text: string): Promise<TerrainReading | null>;
  /** Compile a sentence into a plan and report it, changing nothing. Safe per keystroke. */
  terrainPlan(text: string): Promise<TerrainPlan | null>;
  /** Apply one terrain edit, or (with no edit) read the current recipe and validation back. */
  terrainEdit(edit: Record<string, unknown> | null, entity?: string | null): Promise<TerrainReply>;
  /** Live streaming/memory counters — safe to poll, it never touches the commit pipeline. */
  terrainStats(): Promise<TerrainStats>;
  /** Whether a walkable route exists between two world points, over the resident nav grids. */
  terrainPath(from: [number, number, number], to: [number, number, number]): Promise<TerrainPathResult>;
  /** Arm the pointer with a terrain tool ("none" | "sculpt" | "route") and its brush settings. */
  terrainTool(mode: string, brush?: Partial<TerrainBrushArgs>): Promise<boolean>;
  /** Begin a sculpt gesture; the stroke is then traced natively until `terrainPaintEnd`. */
  terrainPaintBegin(): Promise<void>;
  /** End the gesture and commit what was traced as one undoable transaction. */
  terrainPaintEnd(): Promise<TerrainReply>;
  /** Drop a route control point where the cursor meets the terrain; resolves to the point count. */
  terrainRoutePoint(): Promise<number>;
  /** Remove the last route point, or clear them all; resolves to the remaining count. */
  terrainRouteClear(lastOnly: boolean): Promise<number>;
  /** Commit the drawn route as a road, river or pad. */
  terrainRouteCommit(
    kind: string,
    widthM: number,
    depthM: number,
    materialLayer: number | null,
  ): Promise<TerrainReply>;
}

/** Brush settings the pointer is armed with. `kind` is 0 raise, 1 smooth, 2 flatten, 3 noise. */
export interface TerrainBrushArgs {
  kind: number;
  radiusM: number;
  strength: number;
  hardness: number;
  targetM: number;
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
 *  used, so the command surface + the Channel are unchanged (M10.1 swaps the UI, not the core).
 *
 *  This comment used to say "the 61 commands". The shell registers **229** (2026-08-17) and has for a
 *  long time; the number was true when it was written and nothing re-read it since. `tools/ipc-contract-
 *  audit` now compares this file against `generate_handler!` at rest and in CI, so the count is measured
 *  in the audit header rather than asserted here — see ADR-119. */
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

  redo(): Promise<boolean> {
    return this.core.invoke<boolean>("redo").catch((e: unknown) => {
      console.error("redo failed", e);
      return false;
    });
  }

  entityActions(id: string): Promise<ActionItem[]> {
    return this.core.invoke<ActionItem[]>("entity_actions", { id }).catch((e: unknown) => { console.error("entity_actions failed", e); throw e; });
  }
  cadReimportReport(): Promise<ReimportReport> {
    return this.core.invoke<ReimportReport>("cad_reimport_report").catch((e: unknown) => { console.error("cad_reimport_report failed", e); throw e; });
  }

  cadReimportResolve(oldId: string, accept: boolean): Promise<ReimportReport> {
    return this.core.invoke<ReimportReport>("cad_reimport_resolve", { oldId, accept }).catch((e: unknown) => { console.error("cad_reimport_resolve failed", e); throw e; });
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
  animationState(id: string | null): Promise<AnimationWorkspaceInfo> {
    return this.core.invoke<AnimationWorkspaceInfo>("animation_state", { id }).catch((e: unknown) => { console.error("animation_state failed", e); throw e; });
  }
  animationClipInstanceSave(request: AnimationClipInstanceSaveRequest): Promise<AnimationClipInstanceSaveResult> {
    return this.core.invoke<AnimationClipInstanceSaveResult>("animation_clip_instance_save", { request }).catch((e: unknown) => { console.error("animation_clip_instance_save failed", e); throw e; });
  }
  animationClipInstanceDelete(instanceId: string, expectedRevision: string, selectedId: string | null): Promise<AnimationClipInstanceSaveResult> {
    return this.core.invoke<AnimationClipInstanceSaveResult>("animation_clip_instance_delete", {
      instanceId,
      expectedRevision,
      selectedId,
    }).catch((e: unknown) => { console.error("animation_clip_instance_delete failed", e); throw e; });
  }
  animationClipInstancePreview(request: AnimationClipInstanceSaveRequest): Promise<AnimationPlaybackInfo> {
    return this.core.invoke<AnimationPlaybackInfo>("animation_clip_instance_preview", { request }).catch((e: unknown) => { console.error("animation_clip_instance_preview failed", e); throw e; });
  }
  animationClipInstancePreviewStop(expectedRequestId?: string): Promise<AnimationPlaybackInfo> {
    return this.core.invoke<AnimationPlaybackInfo>("animation_clip_instance_preview_stop", {
      expectedRequestId: expectedRequestId ?? null,
    }).catch((e: unknown) => { console.error("animation_clip_instance_preview_stop failed", e); throw e; });
  }
  animationKey(id: string, component: string, property: string, tick: number, interpolation: AnimationInterpolation): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_key", { id, component, property, tick, interpolation }).catch((e: unknown) => { console.error("animation_key failed", e); throw e; });
  }
  animationDeleteKey(id: string, trackId: string, keyId: string): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_delete_key", { id, trackId, keyId }).catch((e: unknown) => { console.error("animation_delete_key failed", e); throw e; });
  }
  animationDeleteKeys(id: string | null, deletes: AnimationKeyDeleteInfo[]): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_delete_keys", { id, deletes }).catch((e: unknown) => { console.error("animation_delete_keys failed", e); throw e; });
  }
  animationSetInterpolation(id: string, trackId: string, interpolation: AnimationInterpolation): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_set_interpolation", { id, trackId, interpolation }).catch((e: unknown) => { console.error("animation_set_interpolation failed", e); throw e; });
  }
  animationSetTrackEnabled(id: string, trackId: string, enabled: boolean): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_set_track_enabled", { id, trackId, enabled }).catch((e: unknown) => { console.error("animation_set_track_enabled failed", e); throw e; });
  }
  animationSetTrackLocked(id: string, trackId: string, locked: boolean): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_set_track_locked", { id, trackId, locked }).catch((e: unknown) => { console.error("animation_set_track_locked failed", e); throw e; });
  }
  animationUpdateKeys(id: string, trackId: string, updates: AnimationKeyUpdateInfo[]): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_update_keys", { id, trackId, updates }).catch((e: unknown) => { console.error("animation_update_keys failed", e); throw e; });
  }
  animationAddMarker(id: string, name: string, tick: number): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_add_marker", { id, name, tick }).catch((e: unknown) => { console.error("animation_add_marker failed", e); throw e; });
  }
  animationDeleteMarker(ownerId: string, markerId: string): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_delete_marker", { ownerId, markerId }).catch((e: unknown) => { console.error("animation_delete_marker failed", e); throw e; });
  }
  animationAddEvent(id: string, name: string, tick: number, payload?: Json): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_add_event", { id, name, tick, payload: payload ?? null }).catch((e: unknown) => { console.error("animation_add_event failed", e); throw e; });
  }
  animationDeleteEvent(ownerId: string, eventId: string): Promise<AnimationEditResult> {
    return this.core.invoke<AnimationEditResult>("animation_delete_event", { ownerId, eventId }).catch((e: unknown) => { console.error("animation_delete_event failed", e); throw e; });
  }
  animationTransport(action: "play" | "pause" | "stop" | "scrub", tick?: number, loopPolicy?: AnimationLoopPolicy): Promise<AnimationPlaybackInfo> {
    return this.core.invoke<AnimationPlaybackInfo>("animation_transport", { action, tick: tick ?? null, loopPolicy: loopPolicy ?? null }).catch((e: unknown) => { console.error("animation_transport failed", e); throw e; });
  }
  animationPlaybackState(): Promise<AnimationPlaybackInfo> {
    return this.core.invoke<AnimationPlaybackInfo>("animation_transport", { action: "status", tick: null, loopPolicy: null }).catch((e: unknown) => { console.error("animation playback state failed", e); throw e; });
  }
  animationGraphState(sequenceId: string): Promise<AnimationGraphStateInfo> {
    return this.core.invoke<AnimationGraphStateInfo>("animation_graph_state", { sequenceId }).catch((e: unknown) => { console.error("animation_graph_state failed", e); throw e; });
  }
  animationGraphSave(sequenceId: string, request: AnimationGraphSaveRequest): Promise<AnimationGraphSaveResult> {
    return this.core.invoke<AnimationGraphSaveResult>("animation_graph_save", { sequenceId, request }).catch((e: unknown) => { console.error("animation_graph_save failed", e); throw e; });
  }
  animationGraphDelete(sequenceId: string, graphId: string, expectedRevision: string, requestId: string): Promise<AnimationGraphDeleteResult> {
    return this.core.invoke<AnimationGraphDeleteResult>("animation_graph_delete", { sequenceId, graphId, expectedRevision, requestId }).catch((e: unknown) => { console.error("animation_graph_delete failed", e); throw e; });
  }
  animationGraphDebug(graphId: string, instanceId: string | null, watches: readonly string[]): Promise<AnimationGraphDebugInfo> {
    return this.core.invoke<AnimationGraphDebugInfo>("animation_graph_debug", { graphId, instanceId, watches }).catch((e: unknown) => { console.error("animation_graph_debug failed", e); throw e; });
  }
  animationGraphSetPreviewParameters(graphId: string, values: Readonly<Record<string, AnimationGraphValue>>): Promise<AnimationGraphPreviewResult> {
    return this.core.invoke<AnimationGraphPreviewResult>("animation_graph_set_preview_parameters", { graphId, values }).catch((e: unknown) => { console.error("animation_graph_set_preview_parameters failed", e); throw e; });
  }
  animationGraphClearPreviewParameters(graphId: string): Promise<AnimationGraphPreviewResult> {
    return this.core.invoke<AnimationGraphPreviewResult>("animation_graph_clear_preview_parameters", { graphId }).catch((e: unknown) => { console.error("animation_graph_clear_preview_parameters failed", e); throw e; });
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
  duplicateEntities(ids: string[]): Promise<string[]> {
    return this.core.invoke<string[]>("duplicate_entities", { ids }).catch((e: unknown) => { console.error("duplicate_entities failed", e); throw e; });
  }
  focusEntity(id: string): void {
    void this.core.invoke("focus_entity", { id }).catch((e: unknown) => console.error("focus_entity failed", e));
  }
  reportViewportRect(rect: { x: number; y: number; width: number; height: number }): void {
    void this.core.invoke("set_viewport_rect", rect)
      .catch((e: unknown) => console.error("set_viewport_rect failed", e));
  }
  makeDynamic(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("make_dynamic", { id }).catch((e: unknown) => { console.error("make_dynamic failed", e); throw e; });
  }
  makeStatic(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("make_static", { id }).catch((e: unknown) => { console.error("make_static failed", e); throw e; });
  }

  // ── M10.6 scene-authoring verbs ──
  createEntity(x: number, y: number, z: number, name: string): Promise<string | null> {
    return this.core.invoke<string | null>("create_entity", { x, y, z, name }).catch((e: unknown) => { console.error("create_entity failed", e); throw e; });
  }
  pipeForgeStart(options: PipeForgeOptions): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_start", { options }).catch((e: unknown) => { console.error("pipe_forge_start failed", e); throw e; });
  }
  pipeForgePoint(x: number, y: number): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_point", { x, y }).catch((e: unknown) => { console.error("pipe_forge_point failed", e); throw e; });
  }
  pipeForgeUndo(): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_undo").catch((e: unknown) => { console.error("pipe_forge_undo failed", e); throw e; });
  }
  pipeForgeBake(): Promise<PipeBakeReport> {
    return this.core.invoke<PipeBakeReport>("pipe_forge_bake").catch((e: unknown) => { console.error("pipe_forge_bake failed", e); throw e; });
  }
  pipeForgeCancel(): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_cancel").catch((e: unknown) => { console.error("pipe_forge_cancel failed", e); throw e; });
  }
  pipeForgeStatus(): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_status").catch((e: unknown) => { console.error("pipe_forge_status failed", e); throw e; });
  }
  pipeForgeEdit(id: string): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_edit", { id }).catch((e: unknown) => { console.error("pipe_forge_edit failed", e); throw e; });
  }
  pipeForgeBeginBranch(nodeId: number, diameterCm: number): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_begin_branch", { nodeId, diameterCm }).catch((e: unknown) => { console.error("pipe_forge_begin_branch failed", e); throw e; });
  }
  pipeForgeEndBranch(): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_end_branch").catch((e: unknown) => { console.error("pipe_forge_end_branch failed", e); throw e; });
  }
  pipeForgeMoveHandle(nodeId: number, x: number, y: number, z: number): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_move_handle", { nodeId, x, y, z }).catch((e: unknown) => { console.error("pipe_forge_move_handle failed", e); throw e; });
  }
  pipeForgeRemoveHandle(nodeId: number): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_remove_handle", { nodeId }).catch((e: unknown) => { console.error("pipe_forge_remove_handle failed", e); throw e; });
  }
  pipeForgePlaceFitting(nodeId: number, kind: PipeFittingKind, catalogId?: string): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_place_fitting", { nodeId, kind, catalogId: catalogId ?? null }).catch((e: unknown) => { console.error("pipe_forge_place_fitting failed", e); throw e; });
  }
  pipeForgeRemoveFitting(fittingId: number): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_remove_fitting", { fittingId }).catch((e: unknown) => { console.error("pipe_forge_remove_fitting failed", e); throw e; });
  }
  pipeForgeUpsertCatalog(entry: UserFittingCatalogEntry): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_upsert_catalog", { entry }).catch((e: unknown) => { console.error("pipe_forge_upsert_catalog failed", e); throw e; });
  }
  pipeForgeRemoveCatalog(id: string): Promise<PipeForgeStatus> {
    return this.core.invoke<PipeForgeStatus>("pipe_forge_remove_catalog", { id }).catch((e: unknown) => { console.error("pipe_forge_remove_catalog failed", e); throw e; });
  }
  shapeCatalog(): Promise<ShapeSpec[]> {
    return this.core.invoke<ShapeSpec[]>("shape_catalog").catch((e: unknown) => { console.error("shape_catalog failed", e); throw e; });
  }
  shapeSpawn(kind: string, pos?: [number, number, number]): Promise<ShapeReply> {
    return this.core.invoke<ShapeReply>("shape_spawn", { kind, pos: pos ?? null }).catch((e: unknown) => { console.error("shape_spawn failed", e); throw e; });
  }
  shapeUpdate(id: string, params: Record<string, number>): Promise<ShapeReply> {
    return this.core.invoke<ShapeReply>("shape_update", { id, params }).catch((e: unknown) => { console.error("shape_update failed", e); throw e; });
  }
  shapeDraw(mode: "extrude" | "revolve", profile: [number, number][], height?: number, segments?: number, taper?: number): Promise<ShapeReply> {
    return this.core.invoke<ShapeReply>("shape_draw", { mode, profile, height: height ?? null, segments: segments ?? null, taper: taper ?? null }).catch((e: unknown) => { console.error("shape_draw failed", e); throw e; });
  }
  shapeCombine(a: string, b: string, op: "union" | "carve" | "intersect"): Promise<ShapeReply> {
    return this.core.invoke<ShapeReply>("shape_combine", { a, b, op }).catch((e: unknown) => { console.error("shape_combine failed", e); throw e; });
  }
  shapeMeld(a: string, b: string, k?: number): Promise<ShapeReply> {
    return this.core.invoke<ShapeReply>("shape_meld", { a, b, k: k ?? null }).catch((e: unknown) => { console.error("shape_meld failed", e); throw e; });
  }
  roleCatalog(): Promise<RoleSpec[]> {
    return this.core.invoke<RoleSpec[]>("role_catalog").catch((e: unknown) => { console.error("role_catalog failed", e); throw e; });
  }
  roleAssign(id: string, role: string): Promise<RoleReply> {
    return this.core.invoke<RoleReply>("role_assign", { id, role }).catch((e: unknown) => { console.error("role_assign failed", e); throw e; });
  }
  roleClear(id: string): Promise<RoleReply> {
    return this.core.invoke<RoleReply>("role_clear", { id }).catch((e: unknown) => { console.error("role_clear failed", e); throw e; });
  }
  roleStatus(): Promise<RoleStatusInfo> {
    return this.core.invoke<RoleStatusInfo>("role_status").catch((e: unknown) => { console.error("role_status failed", e); throw e; });
  }
  playerInput(x: number, z: number): Promise<void> {
    return this.core.invoke<void>("player_input", { x, z }).catch(() => { /* a dropped input tick is harmless */ });
  }
  formatCatalog(): Promise<FormatSpec[]> {
    return this.core.invoke<FormatSpec[]>("format_catalog").catch((e: unknown) => { console.error("format_catalog failed", e); throw e; });
  }
  colourStatus(): Promise<ColourStatus> {
    return this.core.invoke<ColourStatus>("colour_status").catch((e: unknown) => { console.error("colour_status failed", e); throw e; });
  }
  setWorkingSpace(space: string): Promise<string> {
    return this.core.invoke<string>("set_working_space", { space }).catch((e: unknown) => { console.error("set_working_space failed", e); throw e; });
  }
  setEnvironmentColourSpace(space: string): Promise<string> {
    return this.core.invoke<string>("set_environment_colour_space", { space }).catch((e: unknown) => { console.error("set_environment_colour_space failed", e); throw e; });
  }
  vfxProbe(): Promise<VfxProbe> {
    return this.core.invoke<VfxProbe>("vfx_probe").catch(() => ({ additive: 0, soft: 0, total: 0, bursts: 0, peakRadiance: 0 }));
  }
  cameraProbe(): Promise<CameraProbe> {
    return this.core.invoke<CameraProbe>("camera_probe").catch(() => ({ eye: [0, 0, 0] as [number, number, number], lookAt: [0, 0, 0] as [number, number, number], fovDeg: 45, cinematic: false, distance: 0 }));
  }
  vfxCatalog(): Promise<EffectSpec[]> {
    return this.core.invoke<EffectSpec[]>("vfx_catalog").catch((e: unknown) => { console.error("vfx_catalog failed", e); throw e; });
  }
  vfxAdd(id: string, kind: string, trigger: string): Promise<VfxReply> {
    return this.core.invoke<VfxReply>("vfx_add", { id, kind, trigger }).catch((e: unknown) => { console.error("vfx_add failed", e); throw e; });
  }
  vfxRemove(id: string, index: number): Promise<VfxReply> {
    return this.core.invoke<VfxReply>("vfx_remove", { id, index }).catch((e: unknown) => { console.error("vfx_remove failed", e); throw e; });
  }
  vfxList(id: string): Promise<VfxReply> {
    return this.core.invoke<VfxReply>("vfx_list", { id }).catch((e: unknown) => { console.error("vfx_list failed", e); throw e; });
  }
  cinemaCatalog(): Promise<ShotSpec[]> {
    return this.core.invoke<ShotSpec[]>("cinema_catalog").catch((e: unknown) => { console.error("cinema_catalog failed", e); throw e; });
  }
  cinemaAddShot(id: string, kind: string): Promise<CinemaReply> {
    return this.core.invoke<CinemaReply>("cinema_add_shot", { id, kind }).catch((e: unknown) => { console.error("cinema_add_shot failed", e); throw e; });
  }
  cinemaRemoveShot(id: string, index: number): Promise<CinemaReply> {
    return this.core.invoke<CinemaReply>("cinema_remove_shot", { id, index }).catch((e: unknown) => { console.error("cinema_remove_shot failed", e); throw e; });
  }
  cinemaSetMood(id: string, mood: "calm" | "normal" | "tense"): Promise<CinemaReply> {
    return this.core.invoke<CinemaReply>("cinema_set_mood", { id, mood }).catch((e: unknown) => { console.error("cinema_set_mood failed", e); throw e; });
  }
  cinemaList(id: string): Promise<CinemaReply> {
    return this.core.invoke<CinemaReply>("cinema_list", { id }).catch((e: unknown) => { console.error("cinema_list failed", e); throw e; });
  }
  conditionCatalog(): Promise<ConditionSpec[]> {
    return this.core.invoke<ConditionSpec[]>("condition_catalog").catch((e: unknown) => { console.error("condition_catalog failed", e); throw e; });
  }
  conditionAdd(id: string, request: ClauseRequest): Promise<RoleReply> {
    return this.core.invoke<RoleReply>("condition_add", { id, request }).catch((e: unknown) => { console.error("condition_add failed", e); throw e; });
  }
  conditionRemove(id: string, index: number, any: boolean): Promise<RoleReply> {
    return this.core.invoke<RoleReply>("condition_remove", { id, index, any }).catch((e: unknown) => { console.error("condition_remove failed", e); throw e; });
  }
  conditionList(id: string): Promise<ConditionListInfo> {
    return this.core.invoke<ConditionListInfo>("condition_list", { id }).catch((e: unknown) => { console.error("condition_list failed", e); throw e; });
  }
  assetLabAudit(id: string): Promise<AssetLabResponse> {
    return this.core.invoke<AssetLabResponse>("asset_lab_audit", { id }).catch((e: unknown) => { console.error("asset_lab_audit failed", e); throw e; });
  }
  assetLabProcess(id: string, request: AssetLabProcessRequest): Promise<AssetLabResponse> {
    return this.core.invoke<AssetLabResponse>("asset_lab_process", { id, request }).catch((e: unknown) => { console.error("asset_lab_process failed", e); throw e; });
  }
  assetLabExport(id: string, path?: string): Promise<AssetLabResponse> {
    return this.core.invoke<AssetLabResponse>("asset_lab_export", { id, path: path ?? null }).catch((e: unknown) => { console.error("asset_lab_export failed", e); throw e; });
  }
  sceneExport(format: SceneExportFormat, path?: string): Promise<SceneExportResponse> {
    return this.core.invoke<SceneExportResponse>("scene_export", { format, path: path ?? null }).catch((e: unknown) => { console.error("scene_export failed", e); throw e; });
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
  multiEdit(ids: string[], component: string, field: string, value: Json): Promise<MultiEditResult> {
    return this.core.invoke<MultiEditResult>("multi_edit", { ids, component, field, value }).catch((e: unknown) => { console.error("multi_edit failed", e); throw e; });
  }
  setRotation(ids: string[], quat: [number, number, number, number]): Promise<MultiEditResult> {
    return this.core.invoke<MultiEditResult>("set_rotation", { ids, quat }).catch((e: unknown) => { console.error("set_rotation failed", e); throw e; });
  }
  deleteDeactivate(id: string): Promise<boolean> {
    return this.core.invoke<boolean>("delete_deactivate", { id }).catch((e: unknown) => { console.error("delete_deactivate failed", e); throw e; });
  }
  deleteDeactivateMany(ids: string[]): Promise<boolean> {
    return this.core.invoke<boolean>("delete_deactivate_many", { ids }).catch((e: unknown) => { console.error("delete_deactivate_many failed", e); throw e; });
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
  setRenderProfile(profile: ViewportRenderProfile): Promise<ViewportRenderProfile> {
    return this.core.invoke<ViewportRenderProfile>("set_render_profile", { profile }).catch((e: unknown) => {
      console.error("set_render_profile failed", e);
      throw e;
    });
  }
  renderProfileDebug(): Promise<ViewportRenderProfile> {
    return this.core.invoke<ViewportRenderProfile>("render_profile_debug").catch((e: unknown) => {
      console.error("render_profile_debug failed", e);
      throw e;
    });
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

  // ── the authored match (ADR-097) ─────────────────────────────────────────────────────────────────────
  matchValidate(): Promise<MatchValidation> {
    return this.core.invoke<MatchValidation>("moba_validate");
  }
  matchAuthorStarter(): Promise<AuthoredMatch> {
    return this.core.invoke<AuthoredMatch>("moba_author_starter");
  }
  // Deliberately NOT swallowed into a console.error like the older commands: the rejection carries the
  // cook's diagnostics, and the panel needs them to tell the author which object to fix.
  matchStart(): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_start");
  }
  matchStep(ticks: number): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_step", { ticks });
  }
  matchOrderMove(xMm: number, yMm: number): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_order_move", { xMm, yMm });
  }
  // ── GP-08: the standing orders. One order, then the hero fights on its own. ──────────────────────
  matchAttackMove(xMm: number, yMm: number): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_attack_move", { xMm, yMm });
  }
  matchAttackTarget(target: number): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_attack_target", { target });
  }
  matchHold(): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_hold");
  }
  matchHalt(): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_halt");
  }
  matchCast(target: number): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_cast", { target });
  }
  matchStun(ticks: number): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_stun", { ticks });
  }
  matchStatus(): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_status");
  }
  matchCooked(): Promise<CookedMatch | null> {
    return this.core.invoke<CookedMatch | null>("moba_cooked");
  }
  matchStop(): Promise<MatchStatus> {
    return this.core.invoke<MatchStatus>("moba_stop");
  }

  terrainPresets(): Promise<TerrainPreset[]> {
    return this.core.invoke<TerrainPreset[]>("terrain_presets");
  }

  terrainCreate(preset: string): Promise<TerrainReply> {
    return this.core.invoke<TerrainReply>("terrain_create", { preset });
  }

  terrainDescribe(text: string): Promise<TerrainReply> {
    return this.core.invoke<TerrainReply>("terrain_describe", { text });
  }

  terrainReadDescription(text: string): Promise<TerrainReading | null> {
    return this.core.invoke<TerrainReading | null>("terrain_read_description", { text });
  }

  terrainPlan(text: string): Promise<TerrainPlan | null> {
    return this.core.invoke<TerrainPlan | null>("terrain_plan", { text });
  }

  terrainEdit(edit: Record<string, unknown> | null, entity?: string | null): Promise<TerrainReply> {
    return this.core.invoke<TerrainReply>("terrain_edit", { args: { entity: entity ?? null, edit } });
  }

  terrainStats(): Promise<TerrainStats> {
    return this.core.invoke<TerrainStats>("terrain_stats");
  }

  terrainPath(
    from: [number, number, number],
    to: [number, number, number],
  ): Promise<TerrainPathResult> {
    return this.core.invoke<TerrainPathResult>("terrain_path", { from, to });
  }

  terrainTool(mode: string, brush?: Partial<TerrainBrushArgs>): Promise<boolean> {
    return this.core.invoke<boolean>("terrain_tool", {
      mode,
      kind: brush?.kind ?? null,
      radiusM: brush?.radiusM ?? null,
      strength: brush?.strength ?? null,
      hardness: brush?.hardness ?? null,
      targetM: brush?.targetM ?? null,
    });
  }

  terrainPaintBegin(): Promise<void> {
    return this.core.invoke<void>("terrain_paint_begin");
  }

  terrainPaintEnd(): Promise<TerrainReply> {
    return this.core.invoke<TerrainReply>("terrain_paint_end");
  }

  terrainRoutePoint(): Promise<number> {
    return this.core.invoke<number>("terrain_route_point");
  }

  terrainRouteClear(lastOnly: boolean): Promise<number> {
    return this.core.invoke<number>("terrain_route_clear", { lastOnly });
  }

  terrainRouteCommit(
    kind: string,
    widthM: number,
    depthM: number,
    materialLayer: number | null,
  ): Promise<TerrainReply> {
    return this.core.invoke<TerrainReply>("terrain_route_commit", {
      kind,
      widthM,
      depthM,
      materialLayer,
    });
  }
}


/** A believable recipe for the dev shell and the panel tests — the same shape the engine emits. */
function mockRecipe(preset: string): TerrainRecipe {
  const named: Record<string, string> = {
    flat: "Flat Ground",
    "rolling-hills": "Rolling Hills",
    alpine: "Alpine Peaks",
  };
  return {
    version: 1,
    name: named[preset] ?? "Terrain",
    description: "",
    seed: 12345,
    world_size_m: 2048,
    chunk_size_m: 64,
    chunk_verts: 65,
    layers: [
      { name: "Base", kind: { Constant: { height: 0 } }, blend: "Replace", weight: 1, enabled: true, seed_offset: 0 },
      { name: "Hills", kind: { Fbm: { amplitude: 38 } }, blend: "Add", weight: 1, enabled: true, seed_offset: 11 },
    ],
    strokes: [],
    splines: [],
    materials: [
      { name: "Grass", albedo: [0.22, 0.34, 0.15], roughness: 0.86 },
      { name: "Rock", albedo: [0.38, 0.37, 0.35], roughness: 0.72 },
    ],
    biomes: [{ name: "Meadow", material_layer: 0, enabled: true }],
    protos: [
      { name: "Tree", mesh_key: "", lod_keys: [], impostor_key: null, radius_m: 2, height_m: 12, collide: true },
    ],
    scatter: [{ name: "Woodland", proto: 0, density_per_hectare: 110, enabled: true }],
    water: { enabled: true, sea_level_m: 0, shore_blend_m: 2, deep_m: 8 },
    lod: { levels: 4, screen_error_px: 1, max_view_distance_m: 1024, texture_res: 256, horizon_culling: true },
    budget: { mesh_mb: 256, texture_mb: 320, scatter_mb: 96, collider_mb: 64, max_resident_chunks: 1200 },
  };
}

function mockReading(text: string): TerrainReading {
  const t = text.toLowerCase();
  const has = (...w: string[]) => w.some((x) => t.includes(x));
  const landform = has("mountain", "peak", "alpine") ? "Mountains" : "Hills";
  const understood = [{ phrase: landform.toLowerCase(), meaning: `landform: ${landform.toLowerCase()}` }];
  if (has("river")) understood.push({ phrase: "river", meaning: "a river" });
  if (has("road")) understood.push({ phrase: "road", meaning: "a road" });
  let seed = 2166136261;
  for (const ch of t.trim()) seed = Math.imul(seed ^ ch.charCodeAt(0), 16777619) >>> 0;
  return {
    brief: {
      landform,
      climate: has("desert", "arid") ? "Arid" : "Temperate",
      relief: 1,
      worldSizeM: null,
      water: "Lakes",
      vegetation: 1,
      erosion: null,
      terraces: false,
      river: has("river"),
      road: has("road"),
      name: `Described ${landform}`,
      seed: (seed % 1000000) + 1,
    },
    understood,
    unused: [],
  };
}

function mockTerrainStats(active: boolean): TerrainStats {
  return {
    active,
    residentChunks: active ? 48 : 0,
    visibleChunks: active ? 21 : 0,
    culledFrustum: active ? 19 : 0,
    culledHorizon: active ? 8 : 0,
    pendingBuilds: 0,
    completedBuilds: active ? 48 : 0,
    drawnTriangles: active ? 184_000 : 0,
    drawnInstances: 0,
    impostorInstances: 0,
    meshMb: active ? 31.4 : 0,
    textureMb: active ? 18.2 : 0,
    scatterMb: 0,
    colliderMb: active ? 1.1 : 0,
    navMb: active ? 0.1 : 0,
    totalMb: active ? 50.8 : 0,
    budgetMb: 736,
    budgetFraction: active ? 0.069 : 0,
    overBudget: false,
    buildUsTotal: active ? 92_000 : 0,
    dominantStage: "field",
    problem: "",
  };
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

/**
 * The dev-mock mirror of `editor-shell/src/formats.rs::format_catalog()` — the one list the engine has
 * an opinion about, so the Formats panel and the Export dialog render the same rows under
 * `npm run dev`/Vitest as they do in the `.exe` (which serves the authoritative list from Rust).
 *
 * It was `[]` before, and an empty catalogue is not a neutral placeholder: every surface built on it
 * looks finished and says the engine can read and write nothing. `available` mirrors the PACKAGED
 * build's feature set (`editor-shell/src-tauri/Cargo.toml` forwards `assets-fbx`, `assets-ktx2` and
 * `interchange-3dxml`), because that is the build this mock stands in for.
 */
const MOCK_FORMATS: FormatSpec[] = [
  {
    id: "gltf",
    label: "glTF 2.0 / GLB",
    extensions: ["glb", "gltf"],
    domain: "Real-time",
    direction: "both",
    fidelity: "full",
    carries: { geometry: true, hierarchy: true, materials: true, textures: true, skinning: true, animation: true, cameras: false, metadata: false, physics: false },
    note: "The engine's best-supported path, both directions. Metallic-roughness PBR with embedded textures on export.",
    available: true,
  },
  {
    id: "obj",
    label: "Wavefront OBJ",
    extensions: ["obj"],
    domain: "Real-time",
    direction: "import",
    fidelity: "subset",
    carries: { geometry: true, hierarchy: false, materials: false, textures: false, skinning: false, animation: false, cameras: false, metadata: false, physics: false },
    note: "Geometry only. OBJ has no hierarchy, no animation and no PBR; a companion .mtl is not read.",
    available: true,
  },
  {
    id: "fbx",
    label: "Autodesk FBX",
    extensions: ["fbx"],
    domain: "Real-time",
    direction: "import",
    fidelity: "seam",
    carries: { geometry: true, hierarchy: true, materials: true, textures: false, skinning: true, animation: true, cameras: false, metadata: false, physics: false },
    note: "Read through the native ufbx reader.",
    available: true,
  },
  {
    id: "image",
    label: "PNG / JPEG",
    extensions: ["png", "jpg", "jpeg"],
    domain: "Textures",
    direction: "import",
    fidelity: "full",
    carries: { geometry: true, hierarchy: false, materials: false, textures: true, skinning: false, animation: false, cameras: false, metadata: false, physics: false },
    note: "Placed as a textured quad. Treated as sRGB colour data.",
    available: true,
  },
  {
    id: "ktx2",
    label: "KTX2 / Basis Universal",
    extensions: ["ktx2"],
    domain: "Textures",
    direction: "import",
    fidelity: "seam",
    carries: { geometry: false, hierarchy: false, materials: false, textures: true, skinning: false, animation: false, cameras: false, metadata: false, physics: false },
    note: "Supercompressed GPU texture, transcoded on import.",
    available: true,
  },
  {
    id: "hdr",
    label: "Radiance HDR (equirectangular)",
    extensions: ["hdr"],
    domain: "Textures",
    direction: "import",
    fidelity: "subset",
    carries: { geometry: false, hierarchy: false, materials: false, textures: false, skinning: false, animation: false, cameras: false, metadata: false, physics: false },
    note: "An environment map for image-based lighting. Equirectangular layout only.",
    available: true,
  },
  {
    id: "step",
    label: "STEP AP242",
    extensions: ["stp", "step"],
    domain: "CAD",
    direction: "both",
    fidelity: "subset",
    carries: { geometry: true, hierarchy: true, materials: true, textures: false, skinning: false, animation: false, cameras: false, metadata: true, physics: false },
    note: "Pure-Rust reader and writer. Planar faces plus analytic cylinders, cones, spheres and tori tessellate exactly; trimmed NURBS and freeform faces are reported per face and left to a licensed kernel. Semantic PMI (GD&T) round-trips machine-readable, never downgraded to a graphical annotation.",
    available: true,
  },
  {
    id: "3dxml",
    label: "CATIA 3DXML",
    extensions: ["3dxml"],
    domain: "CAD",
    direction: "import",
    fidelity: "subset",
    carries: { geometry: true, hierarchy: true, materials: false, textures: false, skinning: false, animation: false, cameras: false, metadata: true, physics: false },
    note: "Product structure, assembly tree and instance transforms are read in full. CATIA's proprietary .3DRep tessellation is not decodable here — if a sibling STEP file is present it is used automatically for the geometry, otherwise each affected part is reported as a placed proxy.",
    available: true,
  },
  {
    id: "urdf",
    label: "URDF",
    extensions: ["urdf", "xml"],
    domain: "Simulation",
    direction: "import",
    fidelity: "subset",
    carries: { geometry: false, hierarchy: true, materials: false, textures: false, skinning: false, animation: false, cameras: false, metadata: false, physics: true },
    note: "Links, joints and collision shapes with forward kinematics resolved to world poses. Visual geometry, inertia tensors and actuation are not read. Prismatic, floating and planar joints are declined with a reason rather than approximated.",
    available: true,
  },
  {
    id: "usd",
    label: "OpenUSD (USD-Physics)",
    extensions: ["usda", "usd"],
    domain: "Simulation",
    direction: "both",
    fidelity: "subset",
    carries: { geometry: true, hierarchy: true, materials: false, textures: false, skinning: false, animation: false, cameras: false, metadata: false, physics: true },
    note: "Export writes a USDA scene. Import reads ASCII .usda only, and only the USD-Physics subset: units, rigid bodies and primitive colliders. Binary .usdc, zipped .usdz and USD's composition system (references, payloads, variants, layers) are a declared seam — and composition, not geometry, is what USD is actually for, so this is an interop path and not a claim to speak USD.",
    available: true,
  },
];

/** The dev-mock mirror of the shell's shape catalog (same kinds/params, so the Build panel renders
 *  identically under `npm run dev`/Vitest; the .exe serves the authoritative list from Rust). */
const MOCK_SHAPE_SPECS: ShapeSpec[] = [
  { kind: "box", label: "Box", blurb: "A rectangular block", params: [
    { key: "width", label: "Width", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "height", label: "Height", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "depth", label: "Depth", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
  ] },
  { kind: "sphere", label: "Sphere", blurb: "A ball", params: [
    { key: "radius", label: "Radius", min: 0.05, max: 25, step: 0.05, default: 0.5, integer: false, unit: "m" },
    { key: "segments", label: "Smoothness", min: 8, max: 96, step: 4, default: 32, integer: true, unit: "" },
  ] },
  { kind: "cylinder", label: "Cylinder", blurb: "A round column", params: [
    { key: "radius", label: "Radius", min: 0.05, max: 25, step: 0.05, default: 0.5, integer: false, unit: "m" },
    { key: "height", label: "Height", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "segments", label: "Smoothness", min: 3, max: 96, step: 1, default: 32, integer: true, unit: "" },
  ] },
  { kind: "cone", label: "Cone", blurb: "A point over a round base", params: [
    { key: "radius", label: "Radius", min: 0.05, max: 25, step: 0.05, default: 0.5, integer: false, unit: "m" },
    { key: "height", label: "Height", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "segments", label: "Smoothness", min: 3, max: 96, step: 1, default: 32, integer: true, unit: "" },
  ] },
  { kind: "torus", label: "Ring", blurb: "A doughnut", params: [
    { key: "radius", label: "Ring radius", min: 0.1, max: 25, step: 0.05, default: 0.5, integer: false, unit: "m" },
    { key: "thickness", label: "Thickness", min: 0.02, max: 10, step: 0.02, default: 0.18, integer: false, unit: "m" },
    { key: "segments", label: "Smoothness", min: 8, max: 96, step: 4, default: 40, integer: true, unit: "" },
  ] },
  { kind: "capsule", label: "Capsule", blurb: "A pill — a cylinder with round ends", params: [
    { key: "radius", label: "Radius", min: 0.05, max: 10, step: 0.05, default: 0.3, integer: false, unit: "m" },
    { key: "height", label: "Height", min: 0.2, max: 50, step: 0.1, default: 1.2, integer: false, unit: "m" },
    { key: "segments", label: "Smoothness", min: 8, max: 96, step: 4, default: 32, integer: true, unit: "" },
  ] },
  { kind: "wedge", label: "Wedge", blurb: "A ramp", params: [
    { key: "width", label: "Width", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "height", label: "Height", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "depth", label: "Depth", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
  ] },
  { kind: "prism", label: "Prism", blurb: "A column with flat sides", params: [
    { key: "radius", label: "Radius", min: 0.05, max: 25, step: 0.05, default: 0.5, integer: false, unit: "m" },
    { key: "height", label: "Height", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "sides", label: "Sides", min: 3, max: 12, step: 1, default: 6, integer: true, unit: "" },
  ] },
];

/** A Shape Studio refusal — nothing changed, explained in the author's language. */
function shapeRefusal(reason: string): ShapeReply {
  return { created: null, handle: null, triangles: 0, ms: 0, message: reason, reason };
}

const MOCK_ANIMATION_GRAPH_ADMISSION_CODES = new Set([
  "unsupported_schema",
  "duplicate_node_id",
  "duplicate_edge_id",
  "duplicate_parameter_id",
  "duplicate_sample_id",
  "duplicate_sample_edge",
  "trigger_default_true",
  "legacy_positional_contract_in_v2",
  "unsupported_edge_weight_contract",
  "invalid_edge_weight",
]);

interface MockAnimationClipInstance {
  instanceId: string;
  targetId: string;
  clipId: string;
  targetMappings: AnimationClipTargetMapping[];
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
  private pipeStatus: PipeForgeStatus = {
    active: false,
    points: 0,
    lengthM: 0,
    previewTriangles: 0,
    canBake: false,
    message: "Pipe Forge is ready",
    handles: [],
    edges: [],
    fittings: [],
    fittingCatalog: [],
    branchFrom: null,
    editingEntity: null,
  };
  // M12.2 (dev MockCore): authored machines are kept in-memory so the state-graph actually RENDERS in
  // `npm run dev`. The real validation/no-dangling + undo + reload are the live `.exe` path; this dev
  // mock stores + returns, it does not validate.
  private machines: StateMachineInfo[] = [];
  private smSeq = 0;
  // M12.1 (dev MockCore): the same, for rules — and it was NOT the same until now. `listRules` returned
  // `[]` unconditionally while `authorRule` reported success, so `npm run dev` had a Create button that
  // announced "Rule created" over a list that stayed empty forever, and every list-side affordance (the
  // counts, the on/off toggle, the remove) was unreachable in the dev build. A mock that answers
  // "success" and then "nothing" is worse than one that refuses: it is the C6 failure — green against
  // the mock, and a different story against `/core` — pointed the other way.
  private rules: RuleSummary[] = [];
  private ruleSeq = 0;
  // Browser/dev animation model. The packaged editor routes the same contract to the deterministic Rust
  // service; this small mirror keeps the Animation workspace useful in Vite and component tests.
  private animationTracks: AnimationTrackInfo[] = [];
  private animationMarkers: AnimationWorkspaceInfo["markers"] = [];
  private animationEvents: AnimationWorkspaceInfo["events"] = [];
  private animationRevision = 0;
  private animationTick = 0;
  private animationPlaying = false;
  private animationLoop: AnimationLoopPolicy = "once";
  private animationGraphStates = new Map<string, AnimationGraphStateInfo>();
  private animationGraphRevision = 0;
  private animationGraphPreview = new Map<string, Readonly<Record<string, AnimationGraphValue>>>();
  private animationClipInstances = new Map<string, MockAnimationClipInstance>();
  private animationClipInstanceRevision = 0;
  private animationClipPreview: {
    requestId: string;
    targetId: string;
    instanceId: string;
    clipId: string;
    durationTick: number;
    evaluatedTracks: number;
  } | null = null;
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
  redo(): Promise<boolean> {
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
  private buildAnimationState(id: string | null): AnimationWorkspaceInfo {
    const entity = id ? projectionStore.getState().displayed[id] : undefined;
    const entityContext = entity?.components.UiStyle || entity?.components.HealthBar
      ? "ui" as const
      : entity?.components.Sprite
        ? "2d" as const
        : "3d" as const;
    const bindingMeta = (component: string, property: string, value: Json) => {
      const context = component === "Sprite" ? "2d" as const : ["UiStyle", "HealthBar"].includes(component) ? "ui" as const : entityContext;
      const transformSink = component === "Transform" && ["x", "y", "z", "px", "py", "pz", "qx", "qy", "qz", "qw", "scale"].includes(property) && typeof value === "number";
      const jointSink = ["KinematicJoint", "Joint"].includes(component) && property === "value" && typeof value === "number";
      const workspaceSink = context !== "3d" && (typeof value === "number" || typeof value === "boolean");
      const ready = transformSink || jointSink;
      return {
        context,
        editorKind: typeof value === "boolean" ? "toggle" as const : property.toLowerCase().includes("color") || ["r", "g", "b", "a"].includes(property) ? "color" as const : property.toLowerCase().includes("frame") ? "sprite_frame" as const : "scalar" as const,
        bindingState: ready ? "ready" as const : workspaceSink ? "preview_only" as const : "unsupported" as const,
        bindingReason: ready ? "A verified native runtime adapter consumes this property." : workspaceSink ? "The shared editor preview adapter consumes this property; packaged game rendering is not claimed yet." : "No verified animation adapter consumes this property.",
        runtimeSink: ready ? (jointSink ? "kinematic-joint" : "viewport-transform") : workspaceSink ? `workspace-${context}` : null,
      };
    };
    const protectedField = (component: string, property: string, value: Json): string | null => {
      const token = `${component}.${property}`.toLowerCase();
      if (["mesh", "material", "source", "handle", "parent", "bodya", "bodyb", "controller"].some((part) => token.includes(part))) {
        return "This field identifies authored data or another asset; key a numeric presentation property instead.";
      }
      if (typeof value !== "number" && typeof value !== "boolean") {
        return "Structured and reference fields stay authoritative.";
      }
      const meta = bindingMeta(component, property, value);
      return meta.bindingState === "ready" || meta.bindingState === "preview_only" ? null : "This typed field has no animation consumer yet, so keying remains disabled rather than failing silently.";
    };
    const properties = entity
      ? Object.entries(entity.components).flatMap(([component, fields]) =>
          Object.entries(fields).map(([property, value]) => {
            const reason = protectedField(component, property, value);
            return {
              component,
              property,
              label: `${component} / ${property}`,
              valueKind: typeof value === "boolean" ? "bool" : typeof value === "number" ? "number" : "unsupported",
              value,
              animatable: reason === null,
              reason,
              ...bindingMeta(component, property, value),
            };
          }),
        )
      : [];
    const tracks = this.animationTracks.filter((track) => !id || track.targetId === id);
    const hasMesh = Boolean(entity?.components.MeshRenderer);
    const hasJoint = Boolean(entity?.components.KinematicJoint ?? entity?.components.Joint);
    const durationTick = this.animationRuntimeDuration();
    const clipInstances = [...this.animationClipInstances.values()].filter((instance) => instance.targetId === id);
    const rigidInstance = clipInstances.filter((instance) => instance.clipId === "rigid-demo").at(-1) ?? null;
    const multiInstance = clipInstances.filter((instance) => instance.clipId === "multi-node-demo").at(-1) ?? null;
    const instanceNeedsRepair = (instance: MockAnimationClipInstance | null) =>
      Boolean(instance?.targetMappings.some((mapping) => {
        const target = projectionStore.getState().displayed[mapping.targetId];
        return !target?.components.Transform;
      }));
    const rigidNeedsRepair = instanceNeedsRepair(rigidInstance);
    const multiNeedsRepair = instanceNeedsRepair(multiInstance);
    return {
      revision: `mock-${this.animationRevision}`,
      sequenceId: "main",
      sequenceName: "Main sequence",
      ticksPerSecond: 60_000,
      durationTick,
      currentTick: this.animationTick,
      playing: this.animationPlaying,
      loopPolicy: this.animationLoop,
      selectedId: id,
      selectedName: entity?.name ?? null,
      properties,
      tracks,
      markers: this.animationMarkers,
      events: this.animationEvents,
      contexts: (["2d", "3d", "ui"] as const).map((context) => {
        const contextProperties = properties.filter((property) => property.context === context);
        const contextTracks = tracks.filter((track) => track.context === context);
        const state = contextProperties.some((property) => property.bindingState === "ready") ? "ready" as const : contextProperties.some((property) => property.bindingState === "preview_only") ? "preview_only" as const : "unsupported" as const;
        return { context, state, properties: contextProperties.length, tracks: contextTracks.length, reason: contextProperties.length ? "Shared sequence channels are available in this context." : "Select an asset with properties for this context.", action: null };
      }),
      asset: entity
        ? {
            displayName: entity.name,
            source: entity.components.CadPart ? "CAD import" : hasMesh ? "mesh asset" : "authored entity",
            provenance: entity.components.CadPart ? "source-linked" : "project-authored",
            qualityGrade: hasMesh ? "B · rigid-ready" : "B · native-sink measured",
            logicalId: hasMesh ? "mock-rigid-asset" : null,
            revisionId: hasMesh ? "mock-rigid-revision-1" : null,
            importState: hasMesh ? "ready" : null,
            dependencyCount: 0,
            sourceLocation: null,
            watchesSource: false,
            reimportDiagnostics: 0,
            skeletonJoints: 0,
            clipCount: hasMesh ? 2 : 0,
            morphTargets: 0,
            rootMotion: "not classified (unsupported in this tier)",
            reimportBinding: entity.components.CadPart ? "stable source identity retained" : "project entity identity",
            clipInstanceRevision: `mock-clip-instances-${this.animationClipInstanceRevision}`,
            clips: hasMesh ? [
              {
                clipId: "rigid-demo",
                sequenceId: "rigid-demo-source",
                name: "Rigid showcase",
                durationTick: 120_000,
                sourceBindingHash: "mock-rigid-bindings-v1",
                sourceTargets: ["Fixture root (FixtureRoot#0)"],
                sourceTargetIds: ["gltf-target:fixture-root"],
                sourceBindings: [
                  { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "translation", valueKind: "vec3" },
                  { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "rotation", valueKind: "quaternion" },
                  { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "scale", valueKind: "vec3" },
                ],
                channels: ["translation (vec3)", "rotation (quaternion)", "scale (vec3)"],
                readiness: rigidInstance ? (rigidNeedsRepair ? "repair_required" as const : "ready" as const) : "setup_available" as const,
                reason: rigidInstance
                  ? rigidNeedsRepair
                    ? "This persisted instance references a target that is no longer a live Transform. Repair its mapping without changing its identity."
                    : "This revision-pinned instance is explicitly mapped and ready for native graph playback."
                  : "One rigid source node can be explicitly mapped to this selected entity.",
                action: rigidInstance ? (rigidNeedsRepair ? "Repair the stale target mapping." : "Add this instance in Graph or set up another instance.") : "Review and set up this clip.",
                instanceId: rigidInstance?.instanceId ?? null,
                targetMappings: rigidInstance?.targetMappings.map((mapping) => ({ ...mapping })),
              },
              {
                clipId: "multi-node-demo",
                sequenceId: "multi-node-demo-source",
                name: "Two-part assembly",
                durationTick: 90_000,
                sourceBindingHash: "mock-multi-bindings-v1",
                sourceTargets: ["Fixture root (FixtureRoot#0)", "Fixture child (FixtureRoot#0/FixtureChild#0)"],
                sourceTargetIds: ["gltf-target:fixture-root", "gltf-target:fixture-child"],
                sourceBindings: [
                  { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "translation", valueKind: "vec3" },
                  { sourceTargetId: "gltf-target:fixture-child", sourceTargetLabel: "Fixture child (FixtureRoot#0/FixtureChild#0)", component: "Transform", property: "rotation", valueKind: "quaternion" },
                ],
                channels: ["translation (vec3)", "rotation (quaternion)"],
                readiness: multiInstance ? (multiNeedsRepair ? "repair_required" as const : "ready" as const) : "explicit_mapping_required" as const,
                reason: multiInstance
                  ? multiNeedsRepair
                    ? "A persisted target is no longer a live Transform. Repair the affected row while preserving this instance identity."
                    : "Every source node has a distinct explicit live target."
                  : "This clip animates 2 distinct source nodes. Unsafe one-entity auto-map is disabled.",
                action: multiInstance ? (multiNeedsRepair ? "Repair the stale target mapping." : "Add this instance in Graph or set up another instance.") : "Map every source node to a distinct scene entity.",
                instanceId: multiInstance?.instanceId ?? null,
                targetMappings: multiInstance?.targetMappings.map((mapping) => ({ ...mapping })),
              },
            ] : [],
            capabilities: [
              { capability: "Native property playback", state: properties.some((property) => property.animatable) ? "available" : "missing", reason: "Only Transform fields and validated kinematic-joint values expose a native sink.", action: null },
              { capability: "CAD mechanism", state: hasJoint ? "available" : "missing", reason: hasJoint ? "A kinematic joint is present." : "No kinematic joint is authored on this entity.", action: hasJoint ? null : "Author a joint only when the part has a mechanical degree of freedom." },
              { capability: "GPU skeletal deformation", state: "unsupported", reason: "No verified joint/weight stream is exposed by the dev projection.", action: "Import a skinned glTF and inspect its rig diagnostics in the packaged editor." },
              { capability: "Root-motion authority", state: "unsupported", reason: "Root trajectories are not classified or extracted in this runtime tier.", action: "Keep movement entity-driven." },
            ],
            suggestions: hasMesh ? ["Key Transform properties for a rigid preview."] : ["Choose a numeric property and add the first key."],
          }
        : null,
      issues: [],
    };
  }
  animationState(id: string | null): Promise<AnimationWorkspaceInfo> {
    return Promise.resolve(this.buildAnimationState(id));
  }
  private animationClipDraftError(request: AnimationClipInstanceSaveRequest): string | null {
    const currentRevision = `mock-clip-instances-${this.animationClipInstanceRevision}`;
    if (request.expectedRevision !== currentRevision) return "Clip instances changed elsewhere. Review the latest setup before continuing.";
    if (request.logicalAssetId !== "mock-rigid-asset" || request.expectedAssetRevision !== "mock-rigid-revision-1") return "The imported asset revision changed while setup was open.";
    const sourceTargetIds = request.clipId === "rigid-demo"
      ? ["gltf-target:fixture-root"]
      : request.clipId === "multi-node-demo"
        ? ["gltf-target:fixture-root", "gltf-target:fixture-child"]
        : null;
    const expectedHash = request.clipId === "rigid-demo"
      ? "mock-rigid-bindings-v1"
      : request.clipId === "multi-node-demo"
        ? "mock-multi-bindings-v1"
        : null;
    if (!sourceTargetIds || request.expectedSourceBindingHash !== expectedHash) return "The clip source signature is stale or unsupported.";
    const mappings = request.targetMappings?.length
      ? request.targetMappings
      : sourceTargetIds.length === 1
        ? [{ sourceTargetId: sourceTargetIds[0], targetId: request.targetId }]
        : [];
    if (mappings.length === 0) return `This clip animates ${sourceTargetIds.length} source nodes. Map every source node to a distinct live Transform entity.`;
    const expectedSources = new Set(sourceTargetIds);
    const mappedSources = new Set<string>();
    const mappedTargets = new Set<string>();
    for (const mapping of mappings) {
      if (!expectedSources.has(mapping.sourceTargetId)) return `Source mapping '${mapping.sourceTargetId}' is stale. Reopen setup.`;
      if (mappedSources.has(mapping.sourceTargetId)) return `Source node '${mapping.sourceTargetId}' is mapped more than once.`;
      if (mappedTargets.has(mapping.targetId)) return "Each source node needs a distinct scene target; many-to-one mapping is refused.";
      mappedSources.add(mapping.sourceTargetId);
      mappedTargets.add(mapping.targetId);
      const entity = projectionStore.getState().displayed[mapping.targetId];
      if (!entity?.components.Transform) return `Mapped target '${mapping.targetId}' is missing or no longer has a Transform component.`;
    }
    const missing = sourceTargetIds.filter((sourceTargetId) => !mappedSources.has(sourceTargetId));
    if (missing.length > 0) return `${missing.length} source node mapping(s) are missing.`;
    return null;
  }
  animationClipInstanceSave(request: AnimationClipInstanceSaveRequest): Promise<AnimationClipInstanceSaveResult> {
    const error = this.animationClipDraftError(request);
    if (error) return Promise.resolve({ ok: false, message: error, instanceId: null, state: this.buildAnimationState(request.targetId) });
    const sourceTargetId = request.clipId === "multi-node-demo" ? null : "gltf-target:fixture-root";
    const targetMappings = request.targetMappings?.length
      ? request.targetMappings.map((mapping) => ({ ...mapping }))
      : sourceTargetId
        ? [{ sourceTargetId, targetId: request.targetId }]
        : [];
    this.animationClipInstances.set(request.instanceId, {
      instanceId: request.instanceId,
      targetId: request.targetId,
      clipId: request.clipId,
      targetMappings,
    });
    this.animationClipInstanceRevision += 1;
    this.animationClipPreview = null;
    this.animationPlaying = false;
    this.animationTick = 0;
    this.animationGraphStates.clear();
    return Promise.resolve({
      ok: true,
      message: `Clip '${request.name}' is explicitly mapped and ready in the Graph source palette.`,
      instanceId: request.instanceId,
      state: this.buildAnimationState(request.targetId),
    });
  }
  animationClipInstanceDelete(instanceId: string, expectedRevision: string, selectedId: string | null): Promise<AnimationClipInstanceSaveResult> {
    if (expectedRevision !== `clip-instances-${this.animationClipInstanceRevision}`) {
      return Promise.resolve({
        ok: false,
        message: "Clip instances changed elsewhere. Refresh before discarding.",
        instanceId: null,
        state: this.buildAnimationState(selectedId),
      });
    }
    if (!this.animationClipInstances.delete(instanceId)) {
      return Promise.resolve({
        ok: false,
        message: `Clip instance '${instanceId}' no longer exists.`,
        instanceId: null,
        state: this.buildAnimationState(selectedId),
      });
    }
    this.animationClipInstanceRevision += 1;
    this.animationClipPreview = null;
    this.animationPlaying = false;
    this.animationTick = 0;
    this.animationGraphStates.clear();
    return Promise.resolve({
      ok: true,
      message: `Clip instance '${instanceId}' was discarded.`,
      instanceId,
      state: this.buildAnimationState(selectedId),
    });
  }
  animationClipInstancePreview(request: AnimationClipInstanceSaveRequest): Promise<AnimationPlaybackInfo> {
    const error = this.animationClipDraftError(request);
    if (error) return Promise.resolve({ ok: false, message: error, currentTick: this.animationTick, durationTick: this.animationRuntimeDuration(), playing: this.animationPlaying, loopPolicy: this.animationLoop, evaluatedTracks: 0, crossedEvents: [], eventsTruncated: false });
    const durationTick = request.clipId === "multi-node-demo" ? 90_000 : 120_000;
    const evaluatedTracks = request.clipId === "multi-node-demo" ? 2 : 3;
    this.animationClipPreview = {
      requestId: request.requestId,
      targetId: request.targetId,
      instanceId: request.instanceId,
      clipId: request.clipId,
      durationTick,
      evaluatedTracks,
    };
    this.animationTick = 0;
    this.animationPlaying = true;
    this.animationLoop = "once";
    return Promise.resolve({ ok: true, message: "Previewing the unsaved mapped clip. Stop preview restores authored transforms.", currentTick: 0, durationTick, playing: true, loopPolicy: "once", evaluatedTracks, crossedEvents: [], eventsTruncated: false });
  }
  animationClipInstancePreviewStop(expectedRequestId?: string): Promise<AnimationPlaybackInfo> {
    if (
      expectedRequestId
      && this.animationClipPreview
      && this.animationClipPreview.requestId !== expectedRequestId
    ) {
      return Promise.resolve({
        ok: true,
        message: "A newer imported clip preview remains active.",
        currentTick: this.animationTick,
        durationTick: this.animationRuntimeDuration(),
        playing: this.animationPlaying,
        loopPolicy: this.animationLoop,
        evaluatedTracks: this.animationClipPreview.evaluatedTracks,
        crossedEvents: [],
        eventsTruncated: false,
      });
    }
    this.animationClipPreview = null;
    this.animationPlaying = false;
    this.animationTick = 0;
    return Promise.resolve({ ok: true, message: "Imported clip preview stopped; authored transforms restored.", currentTick: 0, durationTick: this.animationRuntimeDuration(), playing: false, loopPolicy: this.animationLoop, evaluatedTracks: this.animationTracks.length, crossedEvents: [], eventsTruncated: false });
  }
  animationKey(id: string, component: string, property: string, tick: number, interpolation: AnimationInterpolation): Promise<AnimationEditResult> {
    const value = projectionStore.getState().displayed[id]?.components[component]?.[property];
    const entity = projectionStore.getState().displayed[id];
    const context = component === "Sprite" ? "2d" as const : ["UiStyle", "HealthBar"].includes(component) ? "ui" as const : entity?.components.UiStyle || entity?.components.HealthBar ? "ui" as const : entity?.components.Sprite ? "2d" as const : "3d" as const;
    const hasNativeSink = typeof value === "number" && ((component === "Transform" && ["x", "y", "z", "px", "py", "pz", "qx", "qy", "qz", "qw", "scale"].includes(property)) || (["KinematicJoint", "Joint"].includes(component) && property === "value"));
    const hasWorkspaceSink = context !== "3d" && (typeof value === "number" || typeof value === "boolean");
    if ((!Number.isInteger(tick) || tick < 0) || (typeof value !== "number" && typeof value !== "boolean") || (!hasNativeSink && !hasWorkspaceSink) || (typeof value === "number" && !Number.isFinite(value))) {
      return Promise.resolve({ ok: false, message: "The key needs a verified native property sink, a finite current value, and a non-negative whole tick.", trackId: null, keyId: null, state: this.buildAnimationState(id) });
    }
    const trackId = `${id}:${component}:${property}`;
    let track = this.animationTracks.find((item) => item.id === trackId);
    if (!track) {
      track = { id: trackId, name: `${component}.${property}`, targetId: id, targetName: projectionStore.getState().displayed[id]?.name ?? id, component, property, valueKind: typeof value === "boolean" ? "bool" : "number", interpolation, enabled: true, locked: false, context, editorKind: typeof value === "boolean" ? "toggle" : property.toLowerCase().includes("frame") ? "sprite_frame" : "scalar", bindingState: hasNativeSink ? "ready" : "preview_only", bindingReason: hasNativeSink ? "Verified native adapter." : "Shared workspace preview adapter.", runtimeSink: hasNativeSink ? "viewport-transform" : `workspace-${context}`, keys: [] };
      this.animationTracks.push(track);
    }
    const keyId = `${trackId}:${tick}`;
    track.interpolation = interpolation;
    track.keys = [...track.keys.filter((key) => key.tick !== tick), { id: keyId, tick, seconds: tick / 60_000, value, inTangent: null, outTangent: null }].sort((a, b) => a.tick - b.tick || a.id.localeCompare(b.id));
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: `Key added at tick ${tick}. Ctrl-Z removes it in the packaged editor.`, trackId, keyId, state: this.buildAnimationState(id) });
  }
  animationDeleteKey(id: string, trackId: string, keyId: string): Promise<AnimationEditResult> {
    const track = this.animationTracks.find((item) => item.id === trackId && item.targetId === id);
    if (!track || !track.keys.some((key) => key.id === keyId)) {
      return Promise.resolve({ ok: false, message: "That key no longer exists; the timeline was refreshed.", trackId, keyId, state: this.buildAnimationState(id) });
    }
    track.keys = track.keys.filter((key) => key.id !== keyId);
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: "Key removed.", trackId, keyId, state: this.buildAnimationState(id) });
  }
  animationDeleteKeys(id: string | null, deletes: AnimationKeyDeleteInfo[]): Promise<AnimationEditResult> {
    if (deletes.length === 0) return Promise.resolve({ ok: true, message: "No keys selected.", trackId: null, keyId: null, state: this.buildAnimationState(id) });
    const unique = [...new Map(deletes.map((item) => [JSON.stringify([item.targetId, item.trackId, item.keyId]), item])).values()];
    const requested = new Set(unique.map((item) => JSON.stringify([item.targetId, item.trackId, item.keyId])));
    for (const item of unique) {
      const track = this.animationTracks.find((candidate) => candidate.id === item.trackId && candidate.targetId === item.targetId);
      if (!track || track.locked || !track.keys.some((key) => key.id === item.keyId)) {
        return Promise.resolve({ ok: false, message: track?.locked ? "Unlock every selected track before deleting its keys." : "A selected key no longer exists; nothing was removed.", trackId: item.trackId, keyId: item.keyId, state: this.buildAnimationState(id) });
      }
    }
    for (const track of this.animationTracks) {
      track.keys = track.keys.filter((key) => !requested.has(JSON.stringify([track.targetId, track.id, key.id])));
    }
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: `${unique.length} key${unique.length === 1 ? "" : "s"} removed as one edit.`, trackId: unique[0]?.trackId ?? null, keyId: unique[0]?.keyId ?? null, state: this.buildAnimationState(id) });
  }
  animationSetInterpolation(id: string, trackId: string, interpolation: AnimationInterpolation): Promise<AnimationEditResult> {
    const track = this.animationTracks.find((item) => item.id === trackId && item.targetId === id);
    if (!track) return Promise.resolve({ ok: false, message: "That track no longer exists.", trackId, keyId: null, state: this.buildAnimationState(id) });
    track.interpolation = interpolation;
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: `Interpolation changed to ${interpolation}.`, trackId, keyId: null, state: this.buildAnimationState(id) });
  }
  animationSetTrackEnabled(id: string, trackId: string, enabled: boolean): Promise<AnimationEditResult> {
    const track = this.animationTracks.find((item) => item.id === trackId && item.targetId === id);
    if (!track || track.locked) return Promise.resolve({ ok: false, message: track?.locked ? "Unlock the track before muting it." : "That track no longer exists.", trackId, keyId: null, state: this.buildAnimationState(id) });
    track.enabled = enabled;
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: enabled ? "Track unmuted." : "Track muted; keys were preserved.", trackId, keyId: null, state: this.buildAnimationState(id) });
  }
  animationSetTrackLocked(id: string, trackId: string, locked: boolean): Promise<AnimationEditResult> {
    const track = this.animationTracks.find((item) => item.id === trackId && item.targetId === id);
    if (!track) return Promise.resolve({ ok: false, message: "That track no longer exists.", trackId, keyId: null, state: this.buildAnimationState(id) });
    track.locked = locked;
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: locked ? "Track locked." : "Track unlocked.", trackId, keyId: null, state: this.buildAnimationState(id) });
  }
  animationUpdateKeys(id: string, trackId: string, updates: AnimationKeyUpdateInfo[]): Promise<AnimationEditResult> {
    const track = this.animationTracks.find((item) => item.id === trackId && item.targetId === id);
    if (!track || track.locked) return Promise.resolve({ ok: false, message: track?.locked ? "Unlock the track before editing keys." : "That track no longer exists.", trackId, keyId: null, state: this.buildAnimationState(id) });
    for (const update of updates) {
      const key = track.keys.find((item) => item.id === update.keyId);
      if (!key || (update.tick !== undefined && (!Number.isInteger(update.tick) || update.tick < 0))) return Promise.resolve({ ok: false, message: "A key update was stale or invalid.", trackId, keyId: update.keyId, state: this.buildAnimationState(id) });
      if (update.tick !== undefined) { key.tick = update.tick; key.seconds = update.tick / 60_000; }
      if (update.value !== undefined) key.value = update.value;
      if ("inTangent" in update) key.inTangent = update.inTangent ?? null;
      if ("outTangent" in update) key.outTangent = update.outTangent ?? null;
    }
    track.keys.sort((a, b) => a.tick - b.tick || a.id.localeCompare(b.id));
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: `${updates.length} key update(s) committed as one edit.`, trackId, keyId: updates[0]?.keyId ?? null, state: this.buildAnimationState(id) });
  }
  animationAddMarker(id: string, name: string, tick: number): Promise<AnimationEditResult> {
    if (!Number.isInteger(tick) || tick < 0) return Promise.resolve({ ok: false, message: "Marker time must be a non-negative whole tick.", trackId: null, keyId: null, state: this.buildAnimationState(id) });
    this.animationRevision += 1;
    this.animationMarkers.push({ id: `marker-${this.animationRevision}`, ownerId: id, name: name.trim() || "Marker", tick, seconds: tick / 60_000, color: [89, 192, 255, 255] });
    return Promise.resolve({ ok: true, message: "Marker added.", trackId: null, keyId: null, state: this.buildAnimationState(id) });
  }
  animationDeleteMarker(ownerId: string, markerId: string): Promise<AnimationEditResult> {
    const before = this.animationMarkers.length;
    this.animationMarkers = this.animationMarkers.filter((marker) => marker.id !== markerId || marker.ownerId !== ownerId);
    if (before === this.animationMarkers.length) return Promise.resolve({ ok: false, message: "That marker no longer exists.", trackId: null, keyId: null, state: this.buildAnimationState(ownerId) });
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: "Marker removed.", trackId: null, keyId: null, state: this.buildAnimationState(ownerId) });
  }
  animationAddEvent(id: string, name: string, tick: number, payload?: Json): Promise<AnimationEditResult> {
    if (!Number.isInteger(tick) || tick < 0) return Promise.resolve({ ok: false, message: "Event time must be a non-negative whole tick.", trackId: null, keyId: null, state: this.buildAnimationState(id) });
    this.animationRevision += 1;
    this.animationEvents.push({ id: `event-${this.animationRevision}`, ownerId: id, name: name.trim() || "Event", tick, seconds: tick / 60_000, payload: payload ?? null });
    return Promise.resolve({ ok: true, message: "Event added.", trackId: null, keyId: null, state: this.buildAnimationState(id) });
  }
  animationDeleteEvent(ownerId: string, eventId: string): Promise<AnimationEditResult> {
    const before = this.animationEvents.length;
    this.animationEvents = this.animationEvents.filter((event) => event.id !== eventId || event.ownerId !== ownerId);
    if (before === this.animationEvents.length) return Promise.resolve({ ok: false, message: "That event no longer exists.", trackId: null, keyId: null, state: this.buildAnimationState(ownerId) });
    this.animationRevision += 1;
    return Promise.resolve({ ok: true, message: "Event removed.", trackId: null, keyId: null, state: this.buildAnimationState(ownerId) });
  }
  private animationRuntimeDuration(): number {
    const authoredDuration = Math.max(
      60_000,
      ...this.animationTracks.flatMap((track) => track.keys.map((key) => key.tick)),
    );
    const readyInstances = new Set([...this.animationClipInstances.values()].map((instance) => instance.instanceId));
    const graphUsesReadyInstance = [...this.animationGraphStates.values()].some((state) =>
      state.compile.state === "ready"
      && state.graph?.nodes.some((node) => node.kind === "sequence" && node.sourceId != null && readyInstances.has(node.sourceId)),
    );
    return this.animationClipPreview || graphUsesReadyInstance
      ? Math.max(this.animationClipPreview?.durationTick ?? 120_000, authoredDuration)
      : authoredDuration;
  }
  animationTransport(action: "play" | "pause" | "stop" | "scrub", tick?: number, loopPolicy?: AnimationLoopPolicy): Promise<AnimationPlaybackInfo> {
    if (loopPolicy) this.animationLoop = loopPolicy;
    if (action === "play") this.animationPlaying = true;
    if (action === "pause") this.animationPlaying = false;
    if (action === "stop") { this.animationClipPreview = null; this.animationPlaying = false; this.animationTick = 0; }
    if (action === "scrub" && Number.isFinite(tick) && Number.isInteger(tick) && (tick ?? -1) >= 0) { this.animationPlaying = false; this.animationTick = tick ?? 0; }
    const durationTick = this.animationRuntimeDuration();
    return Promise.resolve({ ok: true, message: action === "scrub" ? `Previewing tick ${this.animationTick}.` : `Animation ${action}.`, currentTick: this.animationTick, durationTick, playing: this.animationPlaying, loopPolicy: this.animationLoop, evaluatedTracks: this.animationTracks.length, crossedEvents: [], eventsTruncated: false });
  }
  animationPlaybackState(): Promise<AnimationPlaybackInfo> {
    const durationTick = this.animationRuntimeDuration();
    return Promise.resolve({ ok: true, message: "Animation clock synchronized.", currentTick: this.animationTick, durationTick, playing: this.animationPlaying, loopPolicy: this.animationLoop, evaluatedTracks: this.animationTracks.length, crossedEvents: [], eventsTruncated: false });
  }
  private buildAnimationGraphState(sequenceId: string): AnimationGraphStateInfo {
    const existing = this.animationGraphStates.get(sequenceId);
    if (existing) return existing;
    const state: AnimationGraphStateInfo = {
      schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
      sequenceId,
      revision: "mock-graph-0",
      graph: null,
      nodePresentation: [],
      sources: [
        { id: sequenceId, name: "Main authored sequence", kind: "authored_sequence", logicalAssetId: null, revisionId: `mock-${this.animationRevision}`, durationTick: 60_000, readiness: "ready", reason: "Authored tracks can be evaluated by the graph runtime.", action: null },
        { id: "reference-pose", name: "Reference pose", kind: "reference_pose", logicalAssetId: null, revisionId: null, durationTick: 0, readiness: "ready", reason: "The entity's typed authored values define its reference pose.", action: null },
        { id: "imported:mock-rigid-asset:rigid-demo-source", name: "Rigid showcase (source)", kind: "imported_clip", logicalAssetId: "mock-rigid-asset", revisionId: "mock-rigid-revision-1", durationTick: 120_000, readiness: "blocked", reason: "Immutable source provenance is not a live scene address.", action: "Select its entity in Animation and choose Set up clip." },
        { id: "imported:mock-rigid-asset:multi-node-demo-source", name: "Two-part assembly (source)", kind: "imported_clip", logicalAssetId: "mock-rigid-asset", revisionId: "mock-rigid-revision-1", durationTick: 90_000, readiness: "blocked", reason: "This multi-node source requires a distinct explicit mapping for every node.", action: "Use the advanced mapper; unsafe one-entity auto-map is disabled." },
        ...[...this.animationClipInstances.values()].map((instance) => ({
          id: instance.instanceId,
          name: instance.clipId === "multi-node-demo" ? "Two-part assembly instance" : "Rigid showcase instance",
          kind: "clip_instance" as const,
          logicalAssetId: "mock-rigid-asset",
          revisionId: "mock-rigid-instance-v1",
          durationTick: instance.clipId === "multi-node-demo" ? 90_000 : 120_000,
          readiness: instance.targetMappings.every((mapping) => projectionStore.getState().displayed[mapping.targetId]?.components.Transform)
            ? "ready" as const
            : "blocked" as const,
          reason: instance.targetMappings.every((mapping) => projectionStore.getState().displayed[mapping.targetId]?.components.Transform)
            ? "Revision-pinned source and explicit live target mapping validated."
            : "A persisted target is missing or no longer has a Transform; repair this instance in Animation.",
          action: instance.targetMappings.every((mapping) => projectionStore.getState().displayed[mapping.targetId]?.components.Transform)
            ? null
            : "Repair mapping in the imported clip inspector.",
        })),
      ],
      compile: { state: "missing", authoredRevision: "mock-graph-0", compiledRevision: null, compiledHash: null, lastGoodRevision: null, lastGoodHash: null, message: "Create or apply a graph to compile it." },
      diagnostics: [],
    };
    this.animationGraphStates.set(sequenceId, state);
    return state;
  }
  animationGraphState(sequenceId: string): Promise<AnimationGraphStateInfo> {
    return Promise.resolve(this.buildAnimationGraphState(sequenceId));
  }
  async animationGraphSave(sequenceId: string, request: AnimationGraphSaveRequest): Promise<AnimationGraphSaveResult> {
    // The browser graph validator is only needed when a dev/mock graph is explicitly applied. Keep the
    // complete authoring model out of the viewport shell's startup graph; the desktop path stays native.
    const {
      animationGraphPreflight,
      animationGraphReadableSchemaVersion,
      canonicalizeAnimationGraphDocument,
      cloneAnimationGraph,
    } = await import("../graph/animation-graph-model");
    const current = this.buildAnimationGraphState(sequenceId);
    const requestVersion = animationGraphReadableSchemaVersion(request);
    const graphVersion = animationGraphReadableSchemaVersion(request.graph);
    if (requestVersion === null || graphVersion === null || requestVersion !== graphVersion) {
      return { ok: false, message: "This graph uses an unsupported schema version.", state: current };
    }
    if (request.expectedRevision !== current.revision) {
      return { ok: false, message: "The graph changed elsewhere. Your draft was preserved; review the latest revision before applying again.", state: current };
    }
    const graph = canonicalizeAnimationGraphDocument(request.graph);
    if (graph.sequenceId !== sequenceId) {
      return { ok: false, message: "The graph belongs to a different sequence.", state: current };
    }
    const diagnostics = animationGraphPreflight(graph);
    if (diagnostics.some((diagnostic) => diagnostic.severity === "error" && MOCK_ANIMATION_GRAPH_ADMISSION_CODES.has(diagnostic.code))) {
      return {
        ok: false,
        message: "The draft contains an unsafe storage contract and was not persisted.",
        state: { ...current, diagnostics },
      };
    }
    this.animationGraphRevision += 1;
    const revision = `mock-graph-${this.animationGraphRevision}`;
    const persistedGraph = cloneAnimationGraph(graph);
    const semanticInvalid = diagnostics.some((diagnostic) => diagnostic.severity === "error");
    const compiledHash = semanticInvalid ? current.compile.compiledHash : `mock-compiled-${this.animationGraphRevision}`;
    const state: AnimationGraphStateInfo = {
      ...current,
      revision,
      graph: persistedGraph,
      nodePresentation: persistedGraph.nodes.map((node) => ({ nodeId: node.id, ports: [], readiness: semanticInvalid ? "blocked" : "ready", readinessReason: semanticInvalid ? "The authored draft is retained, but semantic diagnostics prevent compilation." : "Mock compilation accepted this schema-v2 node." })),
      compile: semanticInvalid
        ? { ...current.compile, state: "invalid", authoredRevision: revision, message: "Draft saved; previewing the last good compiled revision while semantic diagnostics remain." }
        : { state: "ready", authoredRevision: revision, compiledRevision: revision, compiledHash, lastGoodRevision: revision, lastGoodHash: compiledHash, message: "Graph compiled and installed." },
      diagnostics,
    };
    this.animationGraphStates.set(sequenceId, state);
    return { ok: true, message: semanticInvalid ? "Graph draft saved; fix blocking diagnostics while the last-good preview remains active." : "Graph applied as one undoable edit in the packaged editor.", state };
  }
  animationGraphDelete(sequenceId: string, graphId: string, expectedRevision: string, _requestId: string): Promise<AnimationGraphDeleteResult> {
    const current = this.buildAnimationGraphState(sequenceId);
    if (current.revision !== expectedRevision || current.graph?.id !== graphId) {
      return Promise.resolve({ ok: false, message: "The graph changed or no longer exists; nothing was deleted.", state: current });
    }
    this.animationGraphRevision += 1;
    const revision = `mock-graph-${this.animationGraphRevision}`;
    const state: AnimationGraphStateInfo = { ...current, revision, graph: null, nodePresentation: [], diagnostics: [], compile: { state: "missing", authoredRevision: revision, compiledRevision: null, compiledHash: null, lastGoodRevision: current.compile.lastGoodRevision, lastGoodHash: current.compile.lastGoodHash, message: "No graph is authored for this sequence." } };
    this.animationGraphStates.set(sequenceId, state);
    this.animationGraphPreview.delete(graphId);
    return Promise.resolve({ ok: true, message: "Animation graph deleted.", state });
  }
  animationGraphDebug(graphId: string, instanceId: string | null, watches: readonly string[]): Promise<AnimationGraphDebugInfo> {
    const state = [...this.animationGraphStates.values()].find((candidate) => candidate.graph?.id === graphId);
    const graph = state?.graph;
    const preview = this.animationGraphPreview.get(graphId) ?? {};
    // The browser mock has no graph evaluator, so it reports no activity instead of inventing node or
    // edge weights. `truncated` makes the missing runtime trace explicit to the UI.
    return Promise.resolve({ graphId, graphRevision: state?.compile.compiledRevision ?? state?.revision ?? "missing", compiledHash: state?.compile.compiledHash ?? "missing", instanceId: instanceId ?? "mock-instance", rawTick: this.animationTick, localTick: this.animationTick, activeNodes: [], activeEdges: [], transition: null, parameterValues: preview, watches: watches.slice(0, 32).map((id) => { const value = preview[id]; return { id, value: (Array.isArray(value) ? [...value] : value ?? null) as Json, source: value === undefined ? "unavailable in browser mock" : "preview parameter" }; }), eventsTruncated: false, evaluationCostMicros: null, truncated: Boolean(graph) || watches.length > 32 });
  }
  animationGraphSetPreviewParameters(graphId: string, values: Readonly<Record<string, AnimationGraphValue>>): Promise<AnimationGraphPreviewResult> {
    const state = [...this.animationGraphStates.values()].find((candidate) => candidate.graph?.id === graphId);
    if (!state?.graph) return Promise.resolve({ ok: false, message: "That graph is not the active browser preview graph.", accepted: {} });
    const parameters = new Map(state.graph.parameters.map((parameter) => [parameter.id, parameter]));
    const accepted: Record<string, AnimationGraphValue> = {};
    const replayable: Record<string, AnimationGraphValue> = { ...(this.animationGraphPreview.get(graphId) ?? {}) };
    for (const [id, value] of Object.entries(values)) {
      const parameter = parameters.get(id);
      if (!parameter) continue;
      accepted[id] = Array.isArray(value) ? [value[0], value[1]] : value;
      if (parameter.kind !== "trigger") replayable[id] = accepted[id];
    }
    this.animationGraphPreview.set(graphId, replayable);
    return Promise.resolve({ ok: Object.keys(accepted).length === Object.keys(values).length, message: "Transient graph parameters updated; the document remains clean.", accepted });
  }
  animationGraphClearPreviewParameters(graphId: string): Promise<AnimationGraphPreviewResult> {
    this.animationGraphPreview.delete(graphId);
    return Promise.resolve({ ok: true, message: "Transient graph parameters reset.", accepted: {} });
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
      // `null`, not omitted: the shell's `CadReportPart` is bare `Option<String>` with no
      // `skip_serializing_if`, so every one of these keys IS on the wire holding null. A mock that
      // omits them builds a part the real core cannot produce — the C6 shape, where a panel is green
      // against MockCore and wrong against `/core`.
      if (r.parts.length < 500)
        r.parts.push({ id, name: e.name, fidelity, reference: null, strategy: null, reason: null, fix: null, sourceFormat: null });
    }
    return Promise.resolve(r);
  }
  cadReimportReport(): Promise<ReimportReport> {
    return Promise.resolve({ isReimport: false, rebound: 0, added: 0, removed: 0, adjudicate: 0, rows: [], orphans: [], pending: [] });
  }
  cadReimportResolve(): Promise<ReimportReport> {
    return Promise.resolve({ isReimport: false, rebound: 0, added: 0, removed: 0, adjudicate: 0, rows: [], orphans: [], pending: [] });
  }
  removeEntity(_id: string): void {}
  duplicateEntity(_id: string): Promise<string | null> {
    return Promise.resolve(null);
  }
  duplicateEntities(_ids: string[]): Promise<string[]> {
    return Promise.resolve([]);
  }
  focusEntity(_id: string): void {}
  reportViewportRect(_rect: { x: number; y: number; width: number; height: number }): void {}
  makeDynamic(_id: string): Promise<boolean> {
    return Promise.resolve(true);
  }
  makeStatic(_id: string): Promise<boolean> {
    return Promise.resolve(true);
  }
  // ── M10.6 scene-authoring verbs — the real undoable commits run under Tauri (proven by the .exe gate);
  // the dev MockCore stubs are inert+deterministic so the menu/hierarchy render without a live core. ──
  createEntity(): Promise<string | null> {
    return Promise.resolve(null);
  }
  pipeForgeStart(options: PipeForgeOptions): Promise<PipeForgeStatus> {
    this.pipeStatus = {
      active: true,
      kit: options.kit,
      diameterCm: options.diameterCm,
      quality: options.quality,
      autoFittings: options.autoFittings,
      points: 0,
      lengthM: 0,
      previewTriangles: 0,
      canBake: false,
      message: "Click in the viewport to place the first point",
      handles: [],
      edges: [],
      fittings: [],
      fittingCatalog: [],
      branchFrom: null,
      editingEntity: null,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgePoint(x = 0, y = 0): Promise<PipeForgeStatus> {
    if (!this.pipeStatus.active) return Promise.resolve({ ...this.pipeStatus, message: "Start Pipe Forge first" });
    const handles = [...this.pipeStatus.handles];
    const edges = [...this.pipeStatus.edges];
    const nextNodeId = handles.reduce((max, handle) => Math.max(max, handle.nodeId), 0) + 1;
    const from = this.pipeStatus.branchFrom ?? handles.at(-1)?.nodeId ?? null;
    handles.push({
      nodeId: nextNodeId,
      position: [Number((x * 10).toFixed(2)), 0, Number((y * 10).toFixed(2))],
      connectedEdges: from == null ? [] : [edges.length + 1],
      fittingIds: [],
    });
    if (from != null) {
      const edgeId = edges.length + 1;
      edges.push({ id: edgeId, from, to: nextNodeId, diameterM: (this.pipeStatus.diameterCm ?? 5) / 100 });
      const parent = handles.find((handle) => handle.nodeId === from);
      if (parent && !parent.connectedEdges.includes(edgeId)) parent.connectedEdges = [...parent.connectedEdges, edgeId];
    }
    const points = handles.length;
    this.pipeStatus = {
      ...this.pipeStatus,
      points,
      lengthM: Math.max(0, points - 1),
      previewTriangles: Math.max(0, points - 1) * 576,
      canBake: points >= 2,
      handles,
      edges,
      branchFrom: this.pipeStatus.branchFrom == null ? null : nextNodeId,
      message: points === 1 ? "First point placed — click to extend the run" : "Run updated — keep drawing or bake the asset",
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeUndo(): Promise<PipeForgeStatus> {
    const handles = this.pipeStatus.handles.slice(0, -1);
    const removed = this.pipeStatus.handles.at(-1);
    const edges = removed
      ? this.pipeStatus.edges.filter((edge) => edge.from !== removed.nodeId && edge.to !== removed.nodeId)
      : this.pipeStatus.edges;
    const points = handles.length;
    this.pipeStatus = {
      ...this.pipeStatus,
      points,
      lengthM: Math.max(0, points - 1),
      previewTriangles: Math.max(0, points - 1) * 576,
      canBake: points >= 2,
      handles: handles.map((handle) => ({
        ...handle,
        connectedEdges: handle.connectedEdges.filter((id) => edges.some((edge) => edge.id === id)),
      })),
      edges,
      fittings: removed ? this.pipeStatus.fittings.filter((fitting) => fitting.nodeId !== removed.nodeId) : this.pipeStatus.fittings,
      branchFrom: this.pipeStatus.branchFrom === removed?.nodeId ? null : this.pipeStatus.branchFrom,
      message: this.pipeStatus.points > 0 ? "Last point removed" : "No points to undo",
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeBake(): Promise<PipeBakeReport> {
    if (!this.pipeStatus.canBake) {
      return Promise.resolve({ entityId: null, handle: null, vertices: 0, triangles: 0, lodTriangles: [], textureResolution: 0, collisionHulls: 0, collisionKind: "none", collisionTriangles: 0, watertight: false, warnings: ["Place at least two points"], message: "Place at least two points before baking" });
    }
    const id = this.place("Galvanized steel pipe", {
      Transform: { x: 0, y: 0, z: 0, scale: 1 },
      MeshRenderer: { mesh: "mtkasset:mock-pipe" },
      PipeRecipe: { version: 1, kit: "Galvanized steel" },
      RigidBody: { kind: "fixed" },
      Collider: { shape: "trimesh" },
    });
    const triangles = this.pipeStatus.previewTriangles;
    this.pipeStatus = { active: false, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Pipe asset baked", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null };
    return Promise.resolve({ entityId: id, handle: "mtkasset:mock-pipe", vertices: Math.ceil(triangles / 2), triangles, lodTriangles: [triangles, Math.floor(triangles / 2)], textureResolution: 256, collisionHulls: 0, collisionKind: "triangle mesh", collisionTriangles: triangles, watertight: true, warnings: [], message: "Pipe asset baked" });
  }
  pipeForgeCancel(): Promise<PipeForgeStatus> {
    this.pipeStatus = { active: false, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Pipe drawing cancelled — the scene was not changed", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeStatus(): Promise<PipeForgeStatus> {
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeEdit(id: string): Promise<PipeForgeStatus> {
    const handles = [
      { nodeId: 1, position: [0, 0, 0] as [number, number, number], connectedEdges: [1], fittingIds: [] },
      { nodeId: 2, position: [1, 0, 0] as [number, number, number], connectedEdges: [1], fittingIds: [] },
    ];
    this.pipeStatus = {
      ...this.pipeStatus,
      active: true,
      points: handles.length,
      lengthM: 1,
      previewTriangles: 576,
      canBake: true,
      message: "Editable route restored — select a handle or click Rebake",
      handles,
      edges: [{ id: 1, from: 1, to: 2, diameterM: (this.pipeStatus.diameterCm ?? 5) / 100 }],
      fittings: [],
      branchFrom: null,
      editingEntity: id,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeBeginBranch(nodeId: number, diameterCm: number): Promise<PipeForgeStatus> {
    this.pipeStatus = {
      ...this.pipeStatus,
      branchFrom: nodeId,
      diameterCm,
      message: `Branch started at handle ${nodeId} — click the viewport to extend it`,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeEndBranch(): Promise<PipeForgeStatus> {
    this.pipeStatus = { ...this.pipeStatus, branchFrom: null, message: "Branch complete — route handles remain editable" };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeMoveHandle(nodeId: number, x: number, y: number, z: number): Promise<PipeForgeStatus> {
    this.pipeStatus = {
      ...this.pipeStatus,
      handles: this.pipeStatus.handles.map((handle) => handle.nodeId === nodeId ? { ...handle, position: [x, y, z] } : handle),
      message: `Handle ${nodeId} moved`,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeRemoveHandle(nodeId: number): Promise<PipeForgeStatus> {
    const edgeIds = new Set(this.pipeStatus.edges.filter((edge) => edge.from === nodeId || edge.to === nodeId).map((edge) => edge.id));
    const handles = this.pipeStatus.handles
      .filter((handle) => handle.nodeId !== nodeId)
      .map((handle) => ({ ...handle, connectedEdges: handle.connectedEdges.filter((id) => !edgeIds.has(id)) }));
    this.pipeStatus = {
      ...this.pipeStatus,
      handles,
      edges: this.pipeStatus.edges.filter((edge) => !edgeIds.has(edge.id)),
      fittings: this.pipeStatus.fittings.filter((fitting) => fitting.nodeId !== nodeId),
      points: handles.length,
      canBake: handles.length >= 2,
      branchFrom: this.pipeStatus.branchFrom === nodeId ? null : this.pipeStatus.branchFrom,
      message: `Leaf handle ${nodeId} removed`,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgePlaceFitting(nodeId: number, kind: PipeFittingKind, catalogId?: string): Promise<PipeForgeStatus> {
    const id = this.pipeStatus.fittings.reduce((max, fitting) => Math.max(max, fitting.id), 0) + 1;
    const fittings = [...this.pipeStatus.fittings, { id, nodeId, kind, catalogId: catalogId ?? null, automatic: false }];
    this.pipeStatus = {
      ...this.pipeStatus,
      fittings,
      handles: this.pipeStatus.handles.map((handle) => handle.nodeId === nodeId ? { ...handle, fittingIds: [...handle.fittingIds, id] } : handle),
      message: `${kind} placed at handle ${nodeId}`,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeRemoveFitting(fittingId: number): Promise<PipeForgeStatus> {
    this.pipeStatus = {
      ...this.pipeStatus,
      fittings: this.pipeStatus.fittings.filter((fitting) => fitting.id !== fittingId),
      handles: this.pipeStatus.handles.map((handle) => ({ ...handle, fittingIds: handle.fittingIds.filter((id) => id !== fittingId) })),
      message: `Fitting ${fittingId} removed`,
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeUpsertCatalog(entry: UserFittingCatalogEntry): Promise<PipeForgeStatus> {
    const fittingCatalog = this.pipeStatus.fittingCatalog.filter((item) => item.id !== entry.id);
    fittingCatalog.push(entry);
    fittingCatalog.sort((left, right) => left.label.localeCompare(right.label));
    this.pipeStatus = { ...this.pipeStatus, fittingCatalog, message: `${entry.label} saved to this route's fitting catalog` };
    return Promise.resolve({ ...this.pipeStatus });
  }
  pipeForgeRemoveCatalog(id: string): Promise<PipeForgeStatus> {
    this.pipeStatus = {
      ...this.pipeStatus,
      fittingCatalog: this.pipeStatus.fittingCatalog.filter((entry) => entry.id !== id),
      fittings: this.pipeStatus.fittings.filter((fitting) => fitting.catalogId !== id),
      message: "Catalog fitting removed",
    };
    return Promise.resolve({ ...this.pipeStatus });
  }
  // ── Shape Studio mocks — same contract as the .exe: a landed reply places + a refusal changes nothing ──
  shapeCatalog(): Promise<ShapeSpec[]> {
    return Promise.resolve(MOCK_SHAPE_SPECS.map((spec) => ({ ...spec, params: spec.params.map((p) => ({ ...p })) })));
  }
  shapeSpawn(kind: string, pos?: [number, number, number]): Promise<ShapeReply> {
    const spec = MOCK_SHAPE_SPECS.find((s) => s.kind === kind);
    if (!spec) return Promise.resolve(shapeRefusal(`there is no shape called "${kind}"`));
    const [x, y, z] = pos ?? [0, 0, 0];
    const params = Object.fromEntries(spec.params.map((p) => [p.key, p.default]));
    const id = this.place(spec.label, {
      Transform: { x, y, z, scale: 1 },
      MeshRenderer: { mesh: `mtkasset:mock-shape-${kind}` },
      ShapeRecipe: { source: JSON.stringify({ v: 1, kind, params }), version: 1, kind, triangles: 480 },
    });
    return Promise.resolve({ created: id, handle: `mtkasset:mock-shape-${kind}`, triangles: 480, ms: 2, message: `Created a ${spec.label} · 480 triangles`, reason: null });
  }
  shapeUpdate(id: string, params: Record<string, number>): Promise<ShapeReply> {
    const entity = this.core.entity(id);
    if (!entity) return Promise.resolve(shapeRefusal("that object is no longer in the scene"));
    const source = entity.components["ShapeRecipe"]?.["source"];
    if (typeof source !== "string") return Promise.resolve(shapeRefusal("that object is not a Shape Studio shape"));
    const recipe = JSON.parse(source) as { v: number; kind: string; params?: Record<string, number> };
    recipe.params = { ...recipe.params, ...params };
    this.core.push([
      { op: "setField", id, component: "ShapeRecipe", field: "source", value: JSON.stringify(recipe) },
    ]);
    return Promise.resolve({ created: id, handle: `mtkasset:mock-shape-${recipe.kind}`, triangles: 480, ms: 2, message: "Updated · 480 triangles", reason: null });
  }
  shapeDraw(mode: "extrude" | "revolve", profile: [number, number][], height?: number, segments?: number, taper?: number): Promise<ShapeReply> {
    if (profile.length < 3) return Promise.resolve(shapeRefusal("draw at least three points to outline a shape"));
    const kind = mode === "extrude" ? "extrude" : "revolve";
    const params = mode === "extrude" ? { height: height ?? 1, taper: taper ?? 1 } : { segments: segments ?? 48 };
    const id = this.place(mode === "extrude" ? "Drawn shape" : "Spun shape", {
      Transform: { x: 0, y: 0, z: 0, scale: 1 },
      MeshRenderer: { mesh: `mtkasset:mock-shape-${kind}` },
      ShapeRecipe: { source: JSON.stringify({ v: 1, kind, params, profile }), version: 1, kind, triangles: profile.length * 4 },
    });
    return Promise.resolve({ created: id, handle: `mtkasset:mock-shape-${kind}`, triangles: profile.length * 4, ms: 3, message: mode === "extrude" ? `Raised your drawing into a solid · ${profile.length * 4} triangles` : `Spun your drawing into a solid · ${profile.length * 4} triangles`, reason: null });
  }
  shapeCombine(a: string, b: string, op: "union" | "carve" | "intersect"): Promise<ShapeReply> {
    if (a === b) return Promise.resolve(shapeRefusal("pick two different objects"));
    if (!this.core.entity(a) || !this.core.entity(b)) return Promise.resolve(shapeRefusal("one of the two objects is no longer in the scene"));
    const label = op === "union" ? "Union" : op === "carve" ? "Carved shape" : "Intersection";
    const id = this.place(label, {
      Transform: { x: 0, y: 0, z: 0, scale: 1 },
      MeshRenderer: { mesh: "mtkasset:mock-shape-combined" },
      ShapeRecipe: { source: JSON.stringify({ v: 1, kind: op, sources: [a, b] }), version: 1, kind: op, triangles: 960 },
    });
    this.core.push([{ op: "remove", id: a }, { op: "remove", id: b }]);
    return Promise.resolve({ created: id, handle: "mtkasset:mock-shape-combined", triangles: 960, ms: 4, message: `${label} · 960 triangles`, reason: null });
  }
  shapeMeld(a: string, b: string, k?: number): Promise<ShapeReply> {
    if (a === b) return Promise.resolve(shapeRefusal("pick two different objects"));
    const ra = this.core.entity(a)?.components["ShapeRecipe"];
    const rb = this.core.entity(b)?.components["ShapeRecipe"];
    if (!ra || !rb) return Promise.resolve(shapeRefusal("Meld works on Shape Studio shapes — use Combine for anything else"));
    // Engine parity: only spheres, boxes and cylinders express as fields (shape_forge::meld_field).
    const meldable = new Set(["sphere", "box", "cylinder"]);
    for (const r of [ra, rb]) {
      const kind = typeof r["kind"] === "string" ? (r["kind"] as string) : "";
      if (!meldable.has(kind)) {
        return Promise.resolve(shapeRefusal(`Meld works on spheres, boxes and cylinders for now — use Combine instead`));
      }
    }
    const id = this.place("Melded shape", {
      Transform: { x: 0, y: 0, z: 0, scale: 1 },
      MeshRenderer: { mesh: "mtkasset:mock-shape-meld" },
      ShapeRecipe: { source: JSON.stringify({ v: 1, kind: "meld", params: { k: k ?? 0.25 }, sources: [a, b] }), version: 1, kind: "meld", triangles: 1400 },
    });
    this.core.push([{ op: "remove", id: a }, { op: "remove", id: b }]);
    return Promise.resolve({ created: id, handle: "mtkasset:mock-shape-meld", triangles: 1400, ms: 6, message: "Melded into one shape · 1400 triangles", reason: null });
  }
  // ── Gameplay-role mocks — same contract as the .exe (landed reply or explained refusal). ──
  private mockRoles = new Map<string, string>();
  roleCatalog(): Promise<RoleSpec[]> {
    return Promise.resolve([
      { kind: "collectible", label: "Collectible", blurb: "Spins; vanishes and scores when something touches it", adds: "spin animation · touch trigger · pickup rule · +1 on the Score counter" },
      { kind: "solid", label: "Solid obstacle", blurb: "An immovable body other things collide with", adds: "fixed physics body · auto-fit collider" },
      { kind: "prop", label: "Physics prop", blurb: "Falls, rolls and collides under gravity", adds: "dynamic physics body · auto-fit collider" },
      { kind: "spinner", label: "Spinner", blurb: "Turns forever — ambient motion", adds: "looping spin animation" },
    ]);
  }
  roleAssign(id: string, role: string): Promise<RoleReply> {
    if (!this.core.entity(id)) return Promise.resolve({ applied: null, entity: null, added: [], scoreEntity: null, message: "that object is no longer in the scene", reason: "that object is no longer in the scene" });
    if (!["collectible", "solid", "prop", "spinner"].includes(role)) return Promise.resolve({ applied: null, entity: null, added: [], scoreEntity: null, message: `there is no role called "${role}"`, reason: `there is no role called "${role}"` });
    this.mockRoles.set(id, role);
    this.core.push([{ op: "setField", id, component: "GameRole", field: "role", value: role }, { op: "setField", id, component: "GameRole", field: "active", value: true }]);
    return Promise.resolve({ applied: role, entity: id, added: role === "collectible" ? ["spin animation", "the pickup rule", "a Score counter"] : ["a physics body"], scoreEntity: role === "collectible" ? "mock-score" : null, message: `Now a ${role}`, reason: null });
  }
  roleClear(id: string): Promise<RoleReply> {
    if (!this.mockRoles.has(id)) return Promise.resolve({ applied: null, entity: null, added: [], scoreEntity: null, message: "that object has no role to clear", reason: "that object has no role to clear" });
    this.mockRoles.delete(id);
    return Promise.resolve({ applied: null, entity: id, added: [], scoreEntity: null, message: "Role cleared — the object keeps its mesh and transform", reason: null });
  }
  roleStatus(): Promise<RoleStatusInfo> {
    const roster = [...this.mockRoles.entries()].map(([entity, role]) => ({ entity, name: this.core.entity(entity)?.name ?? entity, role }));
    return Promise.resolve({ roster, score: 0, scoreEntity: this.mockRoles.size > 0 ? "mock-score" : null, remaining: roster.filter((r) => r.role === "collectible").length, companions: [], won: false, health: null, blocked: null });
  }
  playerInput(): Promise<void> {
    return Promise.resolve();
  }
  formatCatalog(): Promise<FormatSpec[]> {
    return Promise.resolve(MOCK_FORMATS.map((spec) => ({ ...spec, carries: { ...spec.carries } })));
  }
  colourStatus(): Promise<ColourStatus> {
    return Promise.resolve({
      spaces: [],
      working: {
        current: this.workingSpace,
        label: this.workingSpace === "acesCg" ? "ACEScg (AP1)" : "Linear Rec.709",
        wired: true,
        options: [
          { id: "linearRec709", label: "Linear Rec.709", arg: "linearRec709" },
          { id: "acesCg", label: "ACEScg (AP1)", arg: "acesCg" },
        ],
        setCommand: "set_working_space",
        luminanceWeights:
          this.workingSpace === "acesCg" ? [0.2722287, 0.6740818, 0.0536895] : [0.2126, 0.7152, 0.0722],
      },
      views: [],
      activeView: "acesFit",
      activeViewLabel: "Filmic (ACES-like)",
      setViewCommand: "set_render_profile",
      setViewArg: "cinematic",
      presentationHash: this.workingSpace === "acesCg" ? "00000000000000a1" : "0000000000000709",
      exposure: 0.45,
      environment: {
        sourceSpace: this.envSpace,
        label: this.envSpace === "acesCg" ? "ACEScg (AP1)" : "Linear Rec.709",
        assumed: this.envSpace === "linearRec709",
        options: [
          { id: "linearRec709", label: "Linear Rec.709", arg: "linearRec709" },
          { id: "acesCg", label: "ACEScg (AP1)", arg: "acesCg" },
          { id: "aces2065_1", label: "ACES2065-1 (AP0)", arg: "aces2065_1" },
          { id: "linearRec2020", label: "Linear Rec.2020", arg: "linearRec2020" },
        ],
        setCommand: "set_environment_colour_space",
      },
      capabilities: {},
      notes: [],
    });
  }

  /** Mock render state, so the colour card's controls do something in the dev build too. */
  private workingSpace = "linearRec709";
  private envSpace = "linearRec709";

  setWorkingSpace(space: string): Promise<string> {
    this.workingSpace = space;
    return Promise.resolve(space);
  }

  setEnvironmentColourSpace(space: string): Promise<string> {
    this.envSpace = space;
    return Promise.resolve(space);
  }
  vfxProbe(): Promise<VfxProbe> {
    return Promise.resolve({ additive: 0, soft: 0, total: 0, bursts: 0, peakRadiance: 0 });
  }
  cameraProbe(): Promise<CameraProbe> {
    return Promise.resolve({ eye: [0, 0, 0], lookAt: [0, 0, 0], fovDeg: 45, cinematic: false, distance: 0 });
  }
  vfxCatalog(): Promise<EffectSpec[]> {
    return Promise.resolve([]);
  }
  vfxAdd(): Promise<VfxReply> {
    return Promise.resolve({ entity: null, layers: 0, particles: 0, reads: [], problems: [], message: "", reason: "Effects are available in the packaged desktop editor." });
  }
  vfxRemove(): Promise<VfxReply> {
    return Promise.resolve({ entity: null, layers: 0, particles: 0, reads: [], problems: [], message: "", reason: "Effects are available in the packaged desktop editor." });
  }
  vfxList(): Promise<VfxReply> {
    return Promise.resolve({ entity: null, layers: 0, particles: 0, reads: [], problems: [], message: "", reason: null });
  }
  cinemaCatalog(): Promise<ShotSpec[]> {
    return Promise.resolve([]);
  }
  cinemaAddShot(): Promise<CinemaReply> {
    return Promise.resolve({ entity: null, shots: 0, seconds: 0, mood: "normal", reads: [], problems: [], message: "", reason: "Cinematics are available in the packaged desktop editor." });
  }
  cinemaRemoveShot(): Promise<CinemaReply> {
    return Promise.resolve({ entity: null, shots: 0, seconds: 0, mood: "normal", reads: [], problems: [], message: "", reason: "Cinematics are available in the packaged desktop editor." });
  }
  cinemaSetMood(): Promise<CinemaReply> {
    return Promise.resolve({ entity: null, shots: 0, seconds: 0, mood: "normal", reads: [], problems: [], message: "", reason: "Cinematics are available in the packaged desktop editor." });
  }
  cinemaList(): Promise<CinemaReply> {
    return Promise.resolve({ entity: null, shots: 0, seconds: 0, mood: "normal", reads: [], problems: [], message: "", reason: null });
  }
  conditionCatalog(): Promise<ConditionSpec[]> {
    return Promise.resolve([]);
  }
  conditionAdd(): Promise<RoleReply> {
    return Promise.resolve({ applied: null, entity: null, added: [], scoreEntity: null, message: "", reason: "Conditions are available in the packaged desktop editor." });
  }
  conditionRemove(): Promise<RoleReply> {
    return Promise.resolve({ applied: null, entity: null, added: [], scoreEntity: null, message: "", reason: "Conditions are available in the packaged desktop editor." });
  }
  conditionList(): Promise<ConditionListInfo> {
    return Promise.resolve({ all: [], any: [], roleClause: null, sentence: "" });
  }
  assetLabAudit(id: string): Promise<AssetLabResponse> {
    return Promise.resolve({
      ok: false,
      message: "Asset Lab mesh analysis is available in the packaged desktop editor.",
      sourceEntity: id,
      sourceHandle: null,
      createdEntity: null,
      createdHandle: null,
      audit: null,
      change: null,
      warnings: [],
      exportedPath: null,
      bakeEvidence: null,
    });
  }
  assetLabProcess(id: string): Promise<AssetLabResponse> {
    return this.assetLabAudit(id);
  }
  assetLabExport(id: string): Promise<AssetLabResponse> {
    return this.assetLabAudit(id);
  }
  sceneExport(format: SceneExportFormat): Promise<SceneExportResponse> {
    return Promise.resolve({
      ok: false,
      message: "Complete-scene export is available in the packaged desktop editor.",
      format,
      exportedPath: null,
      nodes: 0,
      meshes: 0,
      skins: 0,
      animations: 0,
      fidelity: [],
    });
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
  multiEdit(ids: string[]): Promise<MultiEditResult> {
    return Promise.resolve({ ok: true, changed: ids.length, reason: null });
  }
  /** UNLIKE the scene-authoring stubs around it, this one really writes. `set_rotation` is the only
   *  path a rotation can be typed through, so an inert stub would leave the dev build (and every
   *  `shots` scene over it) unable to demonstrate the one gesture ADR-172 exists for. It pushes the
   *  four fields down the SAME `MockCore.push` delta path the real core's re-projection uses, so the
   *  Inspector's degree rows resolve from the committed quaternion exactly as they do under Tauri.
   *  The engine's normalisation is mirrored here for the same reason it exists there: a projection
   *  the panel reads back must be a rotation. */
  setRotation(ids: string[], quat: [number, number, number, number]): Promise<MultiEditResult> {
    const length = Math.hypot(...quat);
    if (!Number.isFinite(length) || length < 1e-9) {
      return Promise.resolve({
        ok: false,
        changed: 0,
        reason: "those four numbers are not a rotation (a quaternion must be finite and non-zero)",
      });
    }
    const unit = quat.map((c) => c / length);
    const ops: ProjectionOp[] = [];
    for (const id of ids) {
      (["qx", "qy", "qz", "qw"] as const).forEach((field, i) => {
        ops.push({ op: "setField", id, component: "Transform", field, value: unit[i] });
      });
    }
    this.core.push(ops);
    return Promise.resolve({ ok: true, changed: ids.length, reason: null });
  }
  deleteDeactivate(): Promise<boolean> {
    return Promise.resolve(true);
  }
  deleteDeactivateMany(): Promise<boolean> {
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
  setRenderProfile(profile: ViewportRenderProfile): Promise<ViewportRenderProfile> {
    return Promise.resolve(profile);
  }
  renderProfileDebug(): Promise<ViewportRenderProfile> {
    return Promise.resolve("cinematic");
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
  /** The dev stand-in for `metrocalk_core::catalog::grouped` — and it MIRRORS THAT PAYLOAD'S SHAPE, which
   *  the previous two-entry version did not.
   *
   *  It returned `{ Health: [...], UI: [...] }`: keys that are display words rather than the CANONICAL
   *  BUCKETS the real command sends (`std:Props`), items whose `provides`/`requires` were empty arrays, and
   *  no marketplace tier at all — so every capability chip, every price, every tier filter and the whole
   *  buy-confirm path were invisible in the dev build and in `shots`, which is exactly the
   *  `<verification_states_and_convergence>` (b) trap: green against the mock, unexercised against `/core`.
   *  The entries below are the ones the real core actually publishes — `core/src/stdlib.rs`'s components
   *  under their real categories, and all three of `core/src/marketplace.rs`'s `builtin_catalog` entries
   *  with their real prices, their aliased categories and the `Health (acme)` / `Health (brandx)` caps that
   *  make the two-authors-one-bucket taxonomy visible. */
  catalog(): Promise<Record<string, CatalogItem[]>> {
    const local = (id: string, bucket: string, provides: string[], requires: string[]): CatalogItem => ({
      id,
      label: id,
      bucket,
      // `local_item` display-formats the item's own category from its bucket, so a stdlib kind's category
      // and its bucket are the same string in two forms. Stating it here the same way keeps the tile's tag
      // correctly SILENT for locals (it only speaks when it differs from the heading).
      category: bucket.replace(/^std:/, ""),
      source: "local",
      provides,
      requires,
    });
    return Promise.resolve({
      "std:Props": [
        local("Camera", "std:Props", ["View"], ["Spatial"]),
        local("Light", "std:Props", ["Lighting"], ["Spatial"]),
        local("Mesh", "std:Props", ["Renderable"], ["Spatial"]),
        local("Sprite", "std:Props", ["Renderable"], ["Spatial"]),
        local("Transform", "std:Props", ["Spatial"], []),
        {
          id: "forge:rusty-sword",
          label: "Rusty Medieval Sword",
          bucket: "std:Props",
          category: "Props",
          source: "marketplace",
          provides: ["Renderable"],
          requires: ["Spatial"],
          asset: "prop",
          price: 4,
        },
      ],
      "std:Characters": [
        {
          id: "acme:companion-drone",
          label: "Companion Drone",
          bucket: "std:Characters",
          category: "Companions (acme)",
          source: "marketplace",
          provides: ["Health (acme)", "Renderable"],
          requires: ["Spatial"],
          asset: "prop",
          price: 3,
        },
        {
          id: "brandx:spirit-familiar",
          label: "Spirit Familiar",
          bucket: "std:Characters",
          category: "Familiars (brandx)",
          source: "marketplace",
          provides: ["Health (brandx)", "Renderable"],
          requires: ["Spatial"],
          asset: "prop",
          price: 2,
        },
      ],
      "std:Gameplay": [
        local("Collider", "std:Gameplay", ["Collision"], ["Spatial", "Physics"]),
        local("Health", "std:Gameplay", ["Health"], []),
        local("RigidBody", "std:Gameplay", ["Physics"], ["Spatial"]),
      ],
      "std:UI": [local("HealthBar", "std:UI", ["UIElement"], ["Health"])],
      "std:Audio": [local("AudioSource", "std:Audio", ["Audio"], ["Spatial"])],
      "std:Logic": [local("Behavior", "std:Logic", ["Behavior"], [])],
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
  // M12.1 rules (dev MockCore): stateful, for the same reason the machines are — a list needs data. It
  // stores and returns; it does not validate (the registry-Blocked path is the live `.exe`).
  listRules(): Promise<RuleSummary[]> {
    return Promise.resolve(this.rules.map((r) => ({ id: r.id, rule: { ...r.rule } })));
  }
  authorRule(rule: RuleData, id: string | null = null): Promise<AuthorRuleResult> {
    // `author_rule(rule, id)` REPLACES when an id is given — that is the path the on/off toggle uses,
    // so the mock has to honour it or the toggle looks like it adds a duplicate rule.
    const ruleId = id ?? `rule-dev-${++this.ruleSeq}`;
    const at = this.rules.findIndex((r) => r.id === ruleId);
    if (at >= 0) this.rules[at] = { id: ruleId, rule };
    else this.rules.push({ id: ruleId, rule });
    return Promise.resolve({ id: ruleId, error: null, mirror: null });
  }
  deleteRule(id: string): Promise<boolean> {
    const before = this.rules.length;
    this.rules = this.rules.filter((r) => r.id !== id);
    return Promise.resolve(this.rules.length < before);
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

  // ── the authored match: a dev stand-in so `npm run dev` renders the panel without the native kernel.
  // It models only what the panel needs — authored-or-not, and a match that ticks — and is honest about
  // being a stand-in: the digest is a fixed dev value, never a real cook.
  private matchAuthored: AuthoredMatch | null = null;
  private matchTick = 0;
  private matchRunning = false;
  private terrainRecipe: TerrainRecipe | null = null;
  private terrainRoutePoints = 0;
  private matchStunTicks = 0;
  private matchOrder: string | null = null;
  private matchAbilityReadyIn: number | null = 0;
  private devStatus(): MatchStatus {
    const stunned = this.matchStunTicks > 0;
    const actors: MatchActor[] = this.matchAuthored
      ? [
          { id: 1, team: 0, kind: "Structure", x_mm: 0, y_mm: 0, health: 2000, max_health: 2000, alive: true, owned: false, controls: [], speed: 0, ability_ready_in: null, attack_order: null, source: this.matchAuthored.actors[0] },
          { id: 2, team: 1, kind: "Structure", x_mm: 12000, y_mm: 0, health: 2000, max_health: 2000, alive: true, owned: false, controls: [], speed: 0, ability_ready_in: null, attack_order: null, source: this.matchAuthored.actors[1] },
          { id: 3, team: 0, kind: "Hero", x_mm: 1500 + this.matchTick * 10, y_mm: 0, health: 1400, max_health: 1400, alive: true, owned: true, controls: stunned ? ["Stun"] : [], speed: stunned ? 130 : 260, ability_ready_in: this.matchAbilityReadyIn, attack_order: this.matchOrder, source: this.matchAuthored.actors[2] },
        ]
      : [];
    return {
      running: this.matchRunning,
      tick: this.matchTick,
      phase: this.matchRunning ? "Active" : "Idle",
      world_digest: this.matchRunning ? "devworlddigest00" : "",
      lane_digest: this.matchRunning ? "devlanedigest000" : "",
      cook_digest: this.matchRunning ? "devcookdigest000" : "",
      cook_schema_version: 1,
      actor_count: actors.length,
      live_actors: actors.length,
      actors,
      events: [],
      last_rejection: null,
    };
  }
  matchValidate(): Promise<MatchValidation> {
    if (!this.matchAuthored) {
      return Promise.resolve({
        ok: false,
        is_match_scene: false,
        diagnostics: [{ severity: "error", code: "no-match-settings", message: "This scene has no match settings, so there is nothing to run. Add a Match Settings object to define the play area and match length.", entity: null, component: null, field: null }],
        cook_digest: null,
        actor_count: 0,
        wave_count: 0,
        lane_length_m: 0,
      });
    }
    return Promise.resolve({ ok: true, is_match_scene: true, diagnostics: [], cook_digest: "devcookdigest000", actor_count: 3, wave_count: 1, lane_length_m: 12 });
  }
  matchAuthorStarter(): Promise<AuthoredMatch> {
    this.matchAuthored = { settings: "dev_settings", lane: "dev_lane", waypoints: ["dev_wp0", "dev_wp1"], actors: ["dev_blue", "dev_red", "dev_hero"], waves: ["dev_wave"] };
    return Promise.resolve(this.matchAuthored);
  }
  matchStart(): Promise<MatchStatus> {
    if (!this.matchAuthored) {
      return Promise.reject({ message: "This scene cannot run a match yet — one problem needs fixing.", diagnostics: [{ severity: "error", code: "no-match-settings", message: "This scene has no match settings, so there is nothing to run. Add a Match Settings object to define the play area and match length.", entity: null, component: null, field: null }] });
    }
    this.matchRunning = true;
    this.matchTick = 0;
    this.matchStunTicks = 0;
    return Promise.resolve(this.devStatus());
  }
  matchStep(ticks: number): Promise<MatchStatus> {
    if (this.matchRunning) {
      this.matchTick += ticks;
      this.matchStunTicks = Math.max(0, this.matchStunTicks - ticks);
    }
    return Promise.resolve(this.devStatus());
  }
  matchOrderMove(): Promise<MatchStatus> {
    this.matchOrder = null;
    return Promise.resolve(this.devStatus());
  }
  matchAttackMove(xMm: number, yMm: number): Promise<MatchStatus> {
    this.matchOrder = `attack-move to ${(xMm / 1000).toFixed(1)}, ${(yMm / 1000).toFixed(1)}`;
    return Promise.resolve(this.devStatus());
  }
  matchAttackTarget(target: number): Promise<MatchStatus> {
    this.matchOrder = `attacking #${target}`;
    return Promise.resolve(this.devStatus());
  }
  matchHold(): Promise<MatchStatus> {
    this.matchOrder = "hold position";
    return Promise.resolve(this.devStatus());
  }
  matchHalt(): Promise<MatchStatus> {
    this.matchOrder = null;
    return Promise.resolve(this.devStatus());
  }
  matchCast(): Promise<MatchStatus> {
    this.matchAbilityReadyIn = 24;
    return Promise.resolve(this.devStatus());
  }
  matchStun(ticks: number): Promise<MatchStatus> {
    this.matchStunTicks = ticks;
    return Promise.resolve(this.devStatus());
  }
  matchStatus(): Promise<MatchStatus> {
    return Promise.resolve(this.devStatus());
  }
  matchCooked(): Promise<CookedMatch | null> {
    return Promise.resolve(null);
  }
  terrainPresets(): Promise<TerrainPreset[]> {
    return Promise.resolve([
      { id: "flat", name: "Flat Ground", description: "A level plate with one material." },
      { id: "rolling-hills", name: "Rolling Hills", description: "Gentle warped hills." },
      { id: "alpine", name: "Alpine Peaks", description: "Eroded ridged mountains." },
    ]);
  }

  terrainCreate(preset: string): Promise<TerrainReply> {
    this.terrainRecipe = mockRecipe(preset);
    return Promise.resolve(this.terrainReply());
  }

  terrainDescribe(text: string): Promise<TerrainReply> {
    // A shallow stand-in for the real Rust lexicon: enough that the browser build behaves plausibly, and
    // deliberately NOT a second implementation of it — the engine is the only place the reading is decided.
    const reading = mockReading(text);
    this.terrainRecipe = {
      ...mockRecipe(reading.brief.landform === "Mountains" ? "alpine" : "rolling-hills"),
      name: reading.brief.name,
      seed: reading.brief.seed,
    };
    return Promise.resolve({ ...this.terrainReply(), reading });
  }

  terrainReadDescription(text: string): Promise<TerrainReading | null> {
    return Promise.resolve(mockReading(text));
  }

  terrainPlan(text: string): Promise<TerrainPlan | null> {
    // The browser mock cannot resolve a spatial target — that needs the real world. It says so by
    // reporting a creation plan, rather than inventing steps the engine would not produce.
    const r = mockReading(text);
    return Promise.resolve({
      kind: "create" as const,
      understood: r.understood,
      unused: r.unused,
      notes: ["the browser preview cannot resolve “this” — run the app for local edits"],
      steps: [],
      ok: true,
    });
  }

  terrainEdit(edit: Record<string, unknown> | null): Promise<TerrainReply> {
    if (!this.terrainRecipe) {
      return Promise.resolve({
        ok: false,
        entity: "",
        message: "there is no terrain in the scene yet",
        recipe: null,
        issues: [],
        stats: mockTerrainStats(false),
      });
    }
    // Only the handful of edits the dev shell needs to feel real; everything else is acknowledged.
    if (edit && edit.op === "setSeed") this.terrainRecipe.seed = Number(edit.seed ?? 0);
    if (edit && edit.op === "rename") this.terrainRecipe.name = String(edit.name ?? "");
    if (edit && edit.op === "applyPreset") this.terrainRecipe = mockRecipe(String(edit.id ?? "flat"));
    if (edit && edit.op === "toggleLayer") {
      const i = Number(edit.index ?? 0);
      if (this.terrainRecipe.layers[i]) this.terrainRecipe.layers[i].enabled = Boolean(edit.enabled);
    }
    return Promise.resolve(this.terrainReply());
  }

  terrainStats(): Promise<TerrainStats> {
    return Promise.resolve(mockTerrainStats(this.terrainRecipe !== null));
  }

  terrainPath(
    from: [number, number, number],
    to: [number, number, number],
  ): Promise<TerrainPathResult> {
    // The dev shell has no resident nav grids, and saying so is the honest answer — a mock that always
    // reported a route would make the one control whose whole job is to tell the truth about the ground
    // lie in every dev session.
    return Promise.resolve({
      found: false,
      reason: "no navigation grids are resident yet",
      waypoints: [from, to].slice(0, 0),
    });
  }

  terrainTool(mode: string): Promise<boolean> {
    // The dev shell has no viewport to arm, so this only reports whether a tool was requested.
    return Promise.resolve(mode !== "none");
  }

  terrainPaintBegin(): Promise<void> {
    return Promise.resolve();
  }

  terrainPaintEnd(): Promise<TerrainReply> {
    return Promise.resolve(this.terrainReply());
  }

  terrainRoutePoint(): Promise<number> {
    this.terrainRoutePoints += 1;
    return Promise.resolve(this.terrainRoutePoints);
  }

  terrainRouteClear(lastOnly: boolean): Promise<number> {
    this.terrainRoutePoints = lastOnly ? Math.max(0, this.terrainRoutePoints - 1) : 0;
    return Promise.resolve(this.terrainRoutePoints);
  }

  terrainRouteCommit(): Promise<TerrainReply> {
    this.terrainRoutePoints = 0;
    return Promise.resolve(this.terrainReply());
  }

  private terrainReply(): TerrainReply {
    return {
      ok: true,
      entity: "terrain-1",
      message: "",
      recipe: this.terrainRecipe,
      issues: [],
      stats: mockTerrainStats(true),
    };
  }

  matchStop(): Promise<MatchStatus> {
    this.matchRunning = false;
    this.matchTick = 0;
    this.matchStunTicks = 0;
    this.matchOrder = null;
    return Promise.resolve(this.devStatus());
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

export function createMockSession(): EditorClient {
  const [uiT, coreT] = inProcessPair();
  // The dev/test first-run = the small NAMED sample scene (C10), not the 5k perf fixture. `buildWorld`
  // stays exported for the perf / selective-re-render tests that seed it explicitly.
  const core = new MockCore(coreT, sampleScene());
  const client = new DeltaClient(uiT);
  core.emitScene();
  return new MockClient(client, core);
}

/** Thrown when a PRODUCTION bundle finds no core to talk to. Named, so `main.tsx` can tell this apart
 *  from a render crash and say the true thing instead of showing a white page. */
export class NoCoreError extends Error {
  constructor() {
    super("The editor could not reach the Metrocalk engine.");
    this.name = "NoCoreError";
  }
}

/** Build the editor session: the real Tauri shell transport inside the WebView; the in-process MockCore
 *  in `npm run dev` and Vitest, and **only** there.
 *
 *  THE `__MTK_MOCK_CORE__` GUARD IS LOAD-BEARING, NOT DEFENSIVE. `dist/` is `frontendDist` for the
 *  packaged shell and nothing else serves it, so the mock has no role in that build — but the old
 *  ternary referenced `createMockSession()` unconditionally, which kept `MockClient`, `MockCore`,
 *  `DeltaClient` and the sample scene reachable and therefore **shipped**: measured at **76,736 bytes,
 *  one third of the production entry chunk**, for a fake core the packaged app can never construct.
 *  Vite substitutes the literal `false` there, the branch dies, and Rollup drops all of it (`mock-core`
 *  stops being emitted as a chunk at all — verified in `bundle-report.json`, not assumed).
 *
 *  It is a flag of its own and not `import.meta.env.DEV` because the shots harness disproved that
 *  shorthand immediately: the harness is a `vite build` (so `DEV` is false) that renders the real shell
 *  against the MockCore on purpose, and 15 of its 27 scenes captured a black frame the moment the two
 *  were conflated. "React's production mode" and "there is a real engine behind this window" are
 *  different facts; only the second may delete the mock. See `env.d.ts`.
 *
 *  It is also the stronger half of `<verification_states_and_convergence>` (b). "Verified against the
 *  mock is not verified against the real core" was a warning about tests; a production bundle that
 *  still CONTAINS a working fake core is the same mistake with a user on the other end, one missing
 *  global away from a shell that answers every call plausibly and commits nothing. Now the fake cannot
 *  be reached in production because it is not there, and its absence is a sentence rather than a
 *  white page. */
export function createSession(): EditorClient {
  const core = tauriCore();
  if (core) return new TauriClient(core);
  if (!__MTK_MOCK_CORE__) throw new NoCoreError();
  return createMockSession();
}
