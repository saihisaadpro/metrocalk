//! A fully-stubbed `EditorClient` for component tests — override only the methods the test exercises.
//! Keeps the per-component tests robust as the client surface grows (one place to default new methods).
//! Imported only by `*.test.tsx`, so it's never in the production bundle.

import { vi } from "vitest";
import type { EditorClient } from "./session";
import { ANIMATION_GRAPH_SCHEMA_VERSION, type AnimationGraphStateInfo, type AnimationWorkspaceInfo, type DeliveryFrame, type FramingCatalog, type FramingEdit, type MatchStatus, type RenderReply, type ShotRow, type SubjectCatalog, type TerrainReply, type TerrainStats } from "./protocol";

/** The render a test drives. MUTABLE and module-scoped on purpose: a render is the one thing in this
 *  client with a life longer than a single call — start, poll, poll, done — and a stub that answered
 *  each call independently could not express that. `resetTestClientRender()` puts it back, because a
 *  fixture that leaks between tests decides verdicts by run order. */
const RENDER_FIXTURE: RenderReply = {
  running: false,
  done: false,
  entity: null,
  frames: 0,
  written: 0,
  width: 0,
  height: 0,
  fps: 24,
  seconds: 0,
  folder: "",
  stem: "",
  bytes: 0,
  elapsedMs: 0,
  failures: [],
  message: "",
  reason: null,
};

/** Put the render fixture back to "nothing has been rendered". Call it in a `beforeEach` of any test
 *  file that starts a render. */
export function resetTestClientRender(): void {
  Object.assign(RENDER_FIXTURE, {
    running: false,
    done: false,
    entity: null,
    frames: 0,
    written: 0,
    width: 0,
    height: 0,
    fps: 24,
    seconds: 0,
    folder: "",
    stem: "",
    bytes: 0,
    elapsedMs: 0,
    failures: [],
    message: "",
    reason: null,
  });
}

/** One authored shot, with the numbers a timeline needs. Shaped like what `cinema_list` really
 *  sends — a `startSeconds` of 0, an `effectiveSeconds` that is the authored length at Normal
 *  pacing — so a test that renders it is rendering the same object the `.exe` produces. */
const HERO_ROW: ShotRow = {
  id: "shot-0000hero",
  index: 0,
  reads: "a full shot of Crate from three-quarters, pushing in — 2.5s",
  seconds: 2.5,
  effectiveSeconds: 2.5,
  startSeconds: 0,
  openSeconds: 0,
  blendSeconds: 0,
  size: "full",
  angle: "three_quarter",
  motion: "push_in",
  amount: 0.35,
  subject: "e1",
  subjectName: "Crate",
};

/** A subject catalogue shaped like the one `cinema_subject_catalog` really sends: the owner under
 *  its own heading, the assembly it belongs to, one part and one neighbour — with the `parts` counts
 *  that are the whole reason the list is worth reading. `Empty Marker` has none, which is the row a
 *  test needs to prove the picker warns before the film does.
 *
 *  The GROUP STRINGS are the engine's, not the shell's: the headings are sent, so a stub that
 *  invented one would make a grouping test green against a word the engine never says. */
const SUBJECTS: SubjectCatalog = {
  owner: "e1",
  ownerName: "Crate",
  current: "e1",
  candidates: [
    { id: "e1", name: "Crate", group: "This object", parts: 1, framable: true, current: true },
    { id: "e9", name: "Assembly Hall", group: "What it is part of", parts: 46, framable: true, current: false },
    { id: "e2", name: "Lid", group: "What it is made of", parts: 1, framable: true, current: false },
    { id: "e3", name: "Empty Marker", group: "Beside it", parts: 0, framable: false, current: false },
  ],
  query: "",
  matches: 4,
  truncated: false,
};

/** A framing vocabulary with the same WIRE VALUES the Rust catalogue publishes. The labels are the
 *  shell's to choose and a test must not assert them; the values are the contract, and a stub that
 *  invented one would make every framing test green against a word the engine refuses. */
const FRAMING_CATALOG: FramingCatalog = {
  sizes: [
    { value: "extreme_wide", label: "Distant", blurb: "The subject is a speck in its world" },
    { value: "wide", label: "Wide", blurb: "The whole subject with air around it" },
    { value: "full", label: "Full", blurb: "The subject fills most of the height" },
    { value: "medium", label: "Medium", blurb: "Closer - detail starts to read" },
    { value: "close", label: "Close", blurb: "Tight on the subject" },
    { value: "extreme_close", label: "Very close", blurb: "One thing, very large" },
  ],
  angles: [
    { value: "front", label: "Front", blurb: "Facing the subject head-on" },
    { value: "three_quarter", label: "Three-quarter", blurb: "Off to one side, slightly above" },
    { value: "profile", label: "Profile", blurb: "Directly to the side" },
    { value: "behind", label: "Behind", blurb: "Looking where it looks" },
    { value: "low", label: "From below", blurb: "The subject towers" },
    { value: "high", label: "From above", blurb: "The subject is small" },
  ],
  motions: [
    { value: "hold", label: "Hold", blurb: "Locked off - the camera does not move" },
    { value: "push_in", label: "Push in", blurb: "Creep toward the subject" },
    { value: "pull_out", label: "Pull out", blurb: "Drift away" },
    { value: "orbit", label: "Orbit", blurb: "Circle the subject" },
    { value: "crane_up", label: "Crane up", blurb: "Rise while holding the aim" },
    { value: "crane_down", label: "Crane down", blurb: "Descend while holding the aim" },
  ],
  minSeconds: 0.2,
  maxSeconds: 20,
  maxShots: 12,
  stillMotions: ["hold"],
  deliveries: [
    { value: "viewport", label: "Match viewport", blurb: "Compose for the stage as it is now" },
    { value: "widescreen", label: "16:9 widescreen", blurb: "The broadcast and web default" },
    { value: "scope", label: "2.39:1 scope", blurb: "Anamorphic scope" },
    { value: "academy", label: "4:3 academy", blurb: "Classic" },
    { value: "square", label: "1:1 square", blurb: "Square" },
    { value: "vertical", label: "9:16 vertical", blurb: "Vertical" },
  ],
};

function emptyAnimationState(id: string | null): AnimationWorkspaceInfo {
  return {
    revision: "fixture-0",
    sequenceId: "main",
    sequenceName: "Main sequence",
    ticksPerSecond: 60_000,
    durationTick: 60_000,
    currentTick: 0,
    playing: false,
    loopPolicy: "once",
    selectedId: id,
    selectedName: null,
    properties: [],
    tracks: [],
    markers: [],
    events: [],
    contexts: [
      { context: "2d", state: "unsupported", properties: 0, tracks: 0, reason: "No 2D fixture.", action: null },
      { context: "3d", state: "unsupported", properties: 0, tracks: 0, reason: "No 3D fixture.", action: null },
      { context: "ui", state: "unsupported", properties: 0, tracks: 0, reason: "No UI fixture.", action: null },
    ],
    asset: null,
    issues: [],
  };
}

function emptyAnimationGraphState(sequenceId: string): AnimationGraphStateInfo {
  return {
    schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
    sequenceId,
    revision: "fixture-graph-0",
    graph: null,
    nodePresentation: [],
    sources: [],
    compile: { state: "missing", authoredRevision: "fixture-graph-0", compiledRevision: null, compiledHash: null, lastGoodRevision: null, lastGoodHash: null, message: "No graph fixture." },
    diagnostics: [],
  };
}

/** An idle match — nothing authored, nothing running. */
function emptyMatchStatus(): MatchStatus {
  return {
    running: false,
    tick: 0,
    phase: "Idle",
    world_digest: "",
    lane_digest: "",
    cook_digest: "",
    cook_schema_version: 1,
    actor_count: 0,
    live_actors: 0,
    actors: [],
    events: [],
    last_rejection: null,
  };
}

/** The gizmo's space and pivot, per `fakeClient()` call — see the comment on `gizmoDebug` below for
 *  why these are state and not two more constants. */
export function fakeClient(over: Partial<EditorClient> = {}): EditorClient {
  const gizmoState = { space: "world", pivot: "origin" };
  return {
    setField: vi.fn(() => "op"),
    bind: vi.fn(() => "op"),
    onEphemeral: () => () => {},
    revealTargets: () => Promise.resolve({ required: [], compatible: [], greyed: [], bound: [] }),
    describe: () => Promise.resolve({ created: null, kind: null, source: null, price: null, seam: null, balance: null }),
    walletInfo: () => Promise.resolve({ ok: true, balance: 100, cost: null, message: null }),
    topUp: () => Promise.resolve({ ok: true, balance: 200, cost: 100, message: null }),
    aiEdit: () => Promise.resolve({ ok: true, balance: 198, cost: 2, message: null }),
    generate: () => Promise.resolve({ created: "gen-1", cost: 10, available: true, seam: null, balance: 90 }),
    undo: vi.fn(() => Promise.resolve(false)),
    redo: vi.fn(() => Promise.resolve(false)),
    entityActions: () => Promise.resolve([]),
    entityDetails: () => Promise.resolve(null),
    cadReport: () =>
      Promise.resolve({
        total: 0,
        exactBrep: 0,
        tessellationOnly: 0,
        aiReconstructed: 0,
        proxy: 0,
        accessDenied: 0,
        failed: 0,
        parts: [],
      }),
    cadReimportReport: () =>
      Promise.resolve({ isReimport: false, rebound: 0, added: 0, removed: 0, adjudicate: 0, rows: [], orphans: [], pending: [] }),
    cadReimportResolve: () =>
      Promise.resolve({ isReimport: false, rebound: 0, added: 0, removed: 0, adjudicate: 0, rows: [], orphans: [], pending: [] }),
    setJoint: () => Promise.resolve(true),
    jointKey: () => Promise.resolve(true),
    jointValue: () => Promise.resolve(true),
    jointScrub: () => Promise.resolve(0),
    jointInfo: () => Promise.resolve(null),
    animationState: (id) => Promise.resolve(emptyAnimationState(id)),
    animationClipInstanceSave: (request) => Promise.resolve({
      ok: false,
      message: "No imported clip fixture",
      instanceId: null,
      state: emptyAnimationState(request.targetId),
    }),
    animationClipInstanceDelete: (_instanceId, _expectedRevision, selectedId) => Promise.resolve({
      ok: false,
      message: "No imported clip fixture",
      instanceId: null,
      state: emptyAnimationState(selectedId),
    }),
    animationClipInstancePreview: () => Promise.resolve({ ok: false, message: "No imported clip fixture", currentTick: 0, durationTick: 60_000, playing: false, loopPolicy: "once", evaluatedTracks: 0, crossedEvents: [], eventsTruncated: false }),
    animationClipInstancePreviewStop: (_expectedRequestId?: string) => Promise.resolve({ ok: true, message: "Authored animation restored.", currentTick: 0, durationTick: 60_000, playing: false, loopPolicy: "once", evaluatedTracks: 0, crossedEvents: [], eventsTruncated: false }),
    animationKey: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationDeleteKey: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationDeleteKeys: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationSetInterpolation: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationSetTrackEnabled: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationSetTrackLocked: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationUpdateKeys: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationAddMarker: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationDeleteMarker: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationAddEvent: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationDeleteEvent: (id) => Promise.resolve({ ok: false, message: "No animation fixture", trackId: null, keyId: null, state: emptyAnimationState(id) }),
    animationTransport: (action, tick, loopPolicy) => Promise.resolve({ ok: true, message: `Animation ${action}.`, currentTick: tick ?? 0, durationTick: 60_000, playing: action === "play", loopPolicy: loopPolicy ?? "once", evaluatedTracks: 0, crossedEvents: [], eventsTruncated: false }),
    animationPlaybackState: () => Promise.resolve({ ok: true, message: "Animation clock synchronized.", currentTick: 0, durationTick: 60_000, playing: false, loopPolicy: "once", evaluatedTracks: 0, crossedEvents: [], eventsTruncated: false }),
    animationGraphState: (sequenceId) => Promise.resolve(emptyAnimationGraphState(sequenceId)),
    animationGraphSave: (sequenceId) => Promise.resolve({ ok: false, message: "No animation graph fixture", state: emptyAnimationGraphState(sequenceId) }),
    animationGraphDelete: (sequenceId) => Promise.resolve({ ok: false, message: "No animation graph fixture", state: emptyAnimationGraphState(sequenceId) }),
    animationGraphDebug: (graphId, instanceId) => Promise.resolve({ graphId, graphRevision: "fixture-graph-0", compiledHash: "missing", instanceId: instanceId ?? "fixture-instance", rawTick: 0, localTick: 0, activeNodes: [], activeEdges: [], transition: null, parameterValues: {}, watches: [], eventsTruncated: false, evaluationCostMicros: 0, truncated: false }),
    animationGraphSetPreviewParameters: (_graphId, values) => Promise.resolve({ ok: true, message: "Transient parameters updated.", accepted: values }),
    animationGraphClearPreviewParameters: () => Promise.resolve({ ok: true, message: "Transient parameters reset.", accepted: {} }),
    removeEntity: vi.fn(),
    duplicateEntity: vi.fn(() => Promise.resolve(null as string | null)),
    duplicateEntities: vi.fn(() => Promise.resolve([] as string[])),
    focusEntity: vi.fn(),
    reportViewportRect: vi.fn(),
    makeDynamic: () => Promise.resolve(true),
    makeStatic: () => Promise.resolve(true),
    // M10.6 scene-authoring verbs (a test overrides what it exercises).
    createEntity: () => Promise.resolve("e-created"),
    pipeForgeStart: () => Promise.resolve({ active: true, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Click the viewport to place the first point", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgePoint: () => Promise.resolve({ active: true, points: 1, lengthM: 0, previewTriangles: 0, canBake: false, message: "First point placed", handles: [{ nodeId: 1, position: [0, 0, 0], connectedEdges: [], fittingIds: [] }], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeUndo: () => Promise.resolve({ active: true, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Last point removed", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeBake: () => Promise.resolve({ entityId: "pipe-1", handle: "mtkasset:pipe", vertices: 128, triangles: 256, lodTriangles: [256, 128], textureResolution: 256, collisionHulls: 0, collisionKind: "triangle mesh", collisionTriangles: 256, watertight: true, warnings: [], message: "Pipe asset baked" }),
    pipeForgeCancel: () => Promise.resolve({ active: false, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Pipe drawing cancelled", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeStatus: () => Promise.resolve({ active: false, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Pipe Forge is ready", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeEdit: (id) => Promise.resolve({ active: true, points: 2, lengthM: 1, previewTriangles: 576, canBake: true, message: "Editable route restored", handles: [{ nodeId: 1, position: [0, 0, 0], connectedEdges: [1], fittingIds: [] }, { nodeId: 2, position: [1, 0, 0], connectedEdges: [1], fittingIds: [] }], edges: [{ id: 1, from: 1, to: 2, diameterM: 0.05 }], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: id }),
    pipeForgeBeginBranch: (nodeId) => Promise.resolve({ active: true, points: 2, lengthM: 1, previewTriangles: 576, canBake: true, message: "Click to extend the branch", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: nodeId, editingEntity: null }),
    pipeForgeEndBranch: () => Promise.resolve({ active: true, points: 2, lengthM: 1, previewTriangles: 576, canBake: true, message: "Branch complete", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeMoveHandle: (nodeId, x, y, z) => Promise.resolve({ active: true, points: 1, lengthM: 0, previewTriangles: 0, canBake: false, message: "Handle moved", handles: [{ nodeId, position: [x, y, z], connectedEdges: [], fittingIds: [] }], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeRemoveHandle: () => Promise.resolve({ active: true, points: 0, lengthM: 0, previewTriangles: 0, canBake: false, message: "Handle removed", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgePlaceFitting: (nodeId, kind, catalogId) => Promise.resolve({ active: true, points: 1, lengthM: 0, previewTriangles: 0, canBake: false, message: "Fitting placed", handles: [], edges: [], fittings: [{ id: 1, nodeId, kind, catalogId: catalogId ?? null, automatic: false }], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeRemoveFitting: () => Promise.resolve({ active: true, points: 1, lengthM: 0, previewTriangles: 0, canBake: false, message: "Fitting removed", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    pipeForgeUpsertCatalog: (entry) => Promise.resolve({ active: true, points: 1, lengthM: 0, previewTriangles: 0, canBake: false, message: "Catalog saved", handles: [], edges: [], fittings: [], fittingCatalog: [entry], branchFrom: null, editingEntity: null }),
    pipeForgeRemoveCatalog: () => Promise.resolve({ active: true, points: 1, lengthM: 0, previewTriangles: 0, canBake: false, message: "Catalog removed", handles: [], edges: [], fittings: [], fittingCatalog: [], branchFrom: null, editingEntity: null }),
    shapeCatalog: vi.fn(() => Promise.resolve([
      { kind: "box", label: "Box", blurb: "A rectangular block", params: [
        { key: "width", label: "Width", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
        { key: "height", label: "Height", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
        { key: "depth", label: "Depth", min: 0.05, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
      ] },
      { kind: "sphere", label: "Sphere", blurb: "A ball", params: [
        { key: "radius", label: "Radius", min: 0.05, max: 25, step: 0.05, default: 0.5, integer: false, unit: "m" },
        { key: "segments", label: "Smoothness", min: 8, max: 96, step: 4, default: 32, integer: true, unit: "" },
      ] },
    ])),
    shapeSpawn: vi.fn((kind: string) => Promise.resolve({ created: `shape-${kind}`, handle: `mtkasset:test-${kind}`, triangles: 480, ms: 2, message: `Created a ${kind} · 480 triangles`, reason: null })),
    shapeUpdate: vi.fn(() => Promise.resolve({ created: "shape-1", handle: "mtkasset:test-updated", triangles: 512, ms: 2, message: "Updated · 512 triangles", reason: null })),
    shapeDraw: vi.fn((mode: string, profile: [number, number][]) => Promise.resolve(profile.length >= 3
      ? { created: `drawn-${mode}`, handle: "mtkasset:test-drawn", triangles: profile.length * 4, ms: 3, message: "Raised your drawing into a solid", reason: null }
      : { created: null, handle: null, triangles: 0, ms: 0, message: "draw at least three points to outline a shape", reason: "draw at least three points to outline a shape" })),
    shapeCombine: vi.fn((a: string, b: string, op: string) => Promise.resolve(a === b
      ? { created: null, handle: null, triangles: 0, ms: 0, message: "pick two different objects", reason: "pick two different objects" }
      : { created: `combined-${op}`, handle: "mtkasset:test-combined", triangles: 960, ms: 4, message: `Union · 960 triangles`, reason: null })),
    shapeMeld: vi.fn(() => Promise.resolve({ created: "melded-1", handle: "mtkasset:test-meld", triangles: 1400, ms: 6, message: "Melded into one shape · 1400 triangles", reason: null })),
    roleCatalog: vi.fn(() => Promise.resolve([
      { kind: "collectible", label: "Collectible", blurb: "Spins; vanishes and scores when something touches it", adds: "spin animation · touch trigger · pickup rule · +1 on the Score counter" },
      { kind: "solid", label: "Solid obstacle", blurb: "An immovable body other things collide with", adds: "fixed physics body · auto-fit collider" },
      { kind: "prop", label: "Physics prop", blurb: "Falls, rolls and collides under gravity", adds: "dynamic physics body · auto-fit collider" },
      { kind: "spinner", label: "Spinner", blurb: "Turns forever — ambient motion", adds: "looping spin animation" },
      { kind: "companion", label: "Companion", blurb: "Follows your props, patrols your waypoints, fights your enemies", adds: "dynamic physics body · auto-fit collider · a live brain (follow / patrol / attack)" },
      { kind: "enemy", label: "Enemy", blurb: "Companions attack it; it falls when struck", adds: "dynamic physics body · auto-fit collider · the defeat rule · +1 Score when beaten" },
      { kind: "waypoint", label: "Waypoint", blurb: "A patrol stop — companions with nothing to follow walk the chain in order", adds: "a numbered patrol marker (no physics)" },
      { kind: "player", label: "Player", blurb: "YOU, during Play — drive it with the arrow keys or WASD; companions follow you first", adds: "dynamic physics body · auto-fit collider · live keyboard control while playing" },
    ])),
    roleAssign: vi.fn((id: string, role: string) => Promise.resolve({ applied: role, entity: id, added: ["spin animation"], scoreEntity: role === "collectible" ? "score-1" : null, message: `Now a ${role}`, reason: null })),
    roleClear: vi.fn((id: string) => Promise.resolve({ applied: null, entity: id, added: [], scoreEntity: null, message: "Role cleared — the object keeps its mesh and transform", reason: null })),
    roleStatus: vi.fn(() => Promise.resolve({ roster: [], score: 0, scoreEntity: null, remaining: 0, companions: [], won: false, health: null, blocked: null })),
    playerInput: vi.fn(() => Promise.resolve()),
    formatCatalog: vi.fn(() => Promise.resolve([])),
    colourStatus: vi.fn(() =>
      Promise.resolve({
        spaces: [
          { id: "srgb", label: "sRGB", isColour: true, isLinear: false },
          { id: "data", label: "Raw data (not colour)", isColour: false, isLinear: true },
        ],
        working: {
          current: "linearRec709",
          label: "Linear Rec.709",
          wired: true,
          options: [
            { id: "linearRec709", label: "Linear Rec.709", arg: "linearRec709" },
            { id: "acesCg", label: "ACEScg (AP1)", arg: "acesCg" },
          ],
          setCommand: "set_working_space",
          luminanceWeights: [0.2126, 0.7152, 0.0722] as [number, number, number],
        },
        views: [
          { id: "acesFit", label: "Filmic (ACES-like)", blurb: "contrasty" },
          { id: "pbrNeutral", label: "Neutral (Khronos PBR)", blurb: "preserves albedo" },
        ],
        activeView: "acesFit",
        activeViewLabel: "Filmic (ACES-like)",
        setViewCommand: "set_render_profile",
        setViewArg: "cinematic",
        presentationHash: "0000000000000709",
        exposure: 0.45,
        environment: {
          sourceSpace: "linearRec709",
          label: "Linear Rec.709",
          assumed: true,
          options: [
            { id: "linearRec709", label: "Linear Rec.709", arg: "linearRec709" },
            { id: "acesCg", label: "ACEScg (AP1)", arg: "acesCg" },
          ],
          setCommand: "set_environment_colour_space",
        },
        // One wired and one deliberately NOT, so a test can prove the panel shows both. The unwired one
        // is OCIO, which is genuinely unavailable in this build — see `ocio_status` for the two reasons.
        capabilities: { sceneLinearWorkingSpace: true, ocioConfigLoading: false },
        notes: ["Loading a studio .ocio config is not available in this build."],
      }),
    ),
    setWorkingSpace: vi.fn((space: string) => Promise.resolve(space)),
    setEnvironmentColourSpace: vi.fn((space: string) => Promise.resolve(space)),
    vfxProbe: vi.fn(() => Promise.resolve({ additive: 0, soft: 0, total: 0, bursts: 0, peakRadiance: 0 })),
    cameraProbe: vi.fn(() => Promise.resolve({ eye: [0, 0, 0] as [number, number, number], lookAt: [0, 0, 0] as [number, number, number], fovDeg: 45, cinematic: false, distance: 0, frame: [0, 0, 1, 1] as [number, number, number, number], visibleRect: [0, 0, 1, 1] as [number, number, number, number] })),
    vfxCatalog: vi.fn(() => Promise.resolve([
      { kind: "fire", label: "Fire", blurb: "It burns", icon: "\u{1F525}", adds: "a rising flame", burst: false },
      { kind: "sparks", label: "Sparks", blurb: "A hit landing", icon: "\u26A1", adds: "a one-shot spray of sparks", burst: true },
    ])),
    vfxAdd: vi.fn((id: string) => Promise.resolve({ entity: id, layers: 1, particles: 72, reads: ["\u{1F525} Fire - 72 particles, 1.0s per particle"], problems: [], message: "Added Fire", reason: null })),
    vfxRemove: vi.fn((id: string) => Promise.resolve({ entity: id, layers: 0, particles: 0, reads: [], problems: [], message: "Effect removed", reason: null })),
    vfxList: vi.fn((id: string) => Promise.resolve({ entity: id, layers: 0, particles: 0, reads: [], problems: [], message: "", reason: null })),
    // The `icon` field these three carried was removed from `ShotSpec` on both sides by ADR-137 —
    // `theme/icons.tsx` keys its drawings on `kind` — so the emoji here described a contract that no
    // longer exists on either side of the boundary.
    cinemaCatalog: vi.fn(() => Promise.resolve([
      { kind: "establish", label: "Establishing", blurb: "Show where we are before we look at anything closely", adds: "a wide, slowly pulling-out shot from the front" },
      { kind: "hero", label: "Hero shot", blurb: "The workhorse - three-quarters on, pushing in", adds: "a full-body three-quarter shot that creeps closer" },
      { kind: "closeup", label: "Close-up", blurb: "Tight and still - for the moment that matters", adds: "a close, locked-off shot in profile" },
    ])),
    cinemaAddShot: vi.fn((id: string) => Promise.resolve({ entity: id, shots: 1, seconds: 2.5, mood: "normal" as const, delivery: "viewport" as const, reads: [HERO_ROW.reads], rows: [HERO_ROW], problems: [], message: "Added a hero shot", reason: null })),
    cinemaRemoveShot: vi.fn((id: string) => Promise.resolve({ entity: id, shots: 0, seconds: 0, mood: "normal" as const, delivery: "viewport" as const, reads: [], rows: [], problems: [], message: "Shot removed", reason: null })),
    cinemaSetMood: vi.fn((id: string, mood: "calm" | "normal" | "tense") => Promise.resolve({ entity: id, shots: 1, seconds: mood === "calm" ? 6.25 : 2.5, mood, delivery: "viewport" as const, reads: [], rows: [], problems: [], message: `Pacing set to ${mood}`, reason: null })),
    cinemaSetDelivery: vi.fn((id: string, delivery: DeliveryFrame) => Promise.resolve({ entity: id, shots: 1, seconds: 2.5, mood: "normal" as const, delivery, reads: [HERO_ROW.reads], rows: [HERO_ROW], problems: [], message: `Composing for ${delivery}`, reason: null })),
    cinemaList: vi.fn((id: string) => Promise.resolve({ entity: id, shots: 0, seconds: 0, mood: "normal" as const, delivery: "viewport" as const, reads: [], rows: [], problems: [], message: "", reason: null })),
    cinemaFramingCatalog: vi.fn(() => Promise.resolve(FRAMING_CATALOG)),
    cinemaSetShotSubject: vi.fn((id: string, index: number, subject: string) => Promise.resolve({ entity: id, shots: 1, seconds: 2.5, mood: "normal" as const, delivery: "viewport" as const, reads: [HERO_ROW.reads], rows: [{ ...HERO_ROW, index, subject, subjectName: SUBJECTS.candidates.find((c) => c.id === subject)?.name ?? subject }], problems: [], message: `Shot ${index + 1} now frames ${subject}`, reason: null })),
    // The stub SEARCHES, rather than returning the same four rows to every query: a picker that
    // ignored its own search box would otherwise pass every assertion about having one.
    cinemaSubjectCatalog: vi.fn((_id: string, _index: number | null, query: string) => {
      const needle = query.trim().toLowerCase();
      if (!needle) return Promise.resolve(SUBJECTS);
      const hits = SUBJECTS.candidates
        .filter((c) => c.name.toLowerCase().includes(needle))
        .map((c) => ({ ...c, group: "Matches" }));
      return Promise.resolve({ ...SUBJECTS, candidates: hits, query: query.trim(), matches: hits.length });
    }),
    // The chain a click could mean: the object, then what it is part of. Built from the same fixture
    // the picker's list uses, so a rung and a row can never disagree about a name or a count.
    cinemaSubjectChain: vi.fn((id: string) => {
      const self = SUBJECTS.candidates.find((c) => c.id === id);
      const chain = self
        ? [{ ...self, group: "This object", current: false },
           ...SUBJECTS.candidates.filter((c) => c.group === "What it is part of").map((c) => ({ ...c, current: false }))]
        : [];
      return Promise.resolve({ ...SUBJECTS, owner: id, ownerName: self?.name ?? "", current: null, candidates: chain, matches: chain.length });
    }),
    cinemaSetShotSeconds: vi.fn((id: string, index: number, seconds: number) => Promise.resolve({ entity: id, shots: 1, seconds, mood: "normal" as const, delivery: "viewport" as const, reads: [HERO_ROW.reads], rows: [{ ...HERO_ROW, index, seconds, effectiveSeconds: seconds }], problems: [], message: `Shot ${index + 1} now runs ${seconds.toFixed(1)}s`, reason: null })),
    cinemaMoveShot: vi.fn((id: string, _from: number, to: number) => Promise.resolve({ entity: id, shots: 1, seconds: 2.5, mood: "normal" as const, delivery: "viewport" as const, reads: [HERO_ROW.reads], rows: [HERO_ROW], problems: [], message: `Shot moved to position ${to + 1}`, reason: null })),
    cinemaSetShotFraming: vi.fn((id: string, index: number, edit: FramingEdit) => Promise.resolve({ entity: id, shots: 1, seconds: 2.5, mood: "normal" as const, delivery: "viewport" as const, reads: [HERO_ROW.reads], rows: [{ ...HERO_ROW, index, size: (edit.size as ShotRow["size"]) ?? HERO_ROW.size, angle: (edit.angle as ShotRow["angle"]) ?? HERO_ROW.angle, motion: (edit.motion as ShotRow["motion"]) ?? HERO_ROW.motion, amount: edit.amount ?? HERO_ROW.amount }], problems: [], message: `Shot ${index + 1} is now re-framed`, reason: null })),
    // A pose that MOVES WITH THE CLOCK. A stub returning one fixed camera would let a panel that
    // ignored `seconds` entirely pass every assertion about previewing, which is the one thing these
    // tests exist to catch; the push-in here is the same shape `push_in` solves for, at fixture scale.
    cinemaPreview: vi.fn((id: string, seconds: number, active: boolean) =>
      Promise.resolve(
        active
          ? {
              active: true,
              entity: id,
              seconds,
              shotIndex: 0,
              shots: 1,
              reads: HERO_ROW.reads,
              subjectName: HERO_ROW.subjectName,
              progress: Math.min(1, seconds / HERO_ROW.effectiveSeconds),
              blending: false,
              eye: [3, 2, 8 - seconds] as [number, number, number],
              lookAt: [0, 1, 0] as [number, number, number],
              fovDeg: 50,
              message: `Previewing shot 1 of 1 at ${seconds.toFixed(1)}s`,
              reason: null,
            }
          : {
              active: false,
              entity: null,
              seconds: 0,
              shotIndex: null,
              shots: 0,
              reads: "",
              subjectName: "",
              progress: 0,
              blending: false,
              eye: [0, 0, 0] as [number, number, number],
              lookAt: [0, 0, 0] as [number, number, number],
              fovDeg: 50,
              message: "Preview off \u2014 the editor camera is back.",
              reason: null,
            },
      ),
    ),
    // A render that ACTUALLY ADVANCES. A stub answering one fixed row would let a panel that never
    // polls pass every assertion about progress, which is the one thing these tests exist to catch:
    // each `cinemaRenderStatus` call writes one more frame until the plan is full, then goes `done`.
    cinemaRenderPlan: vi.fn((id: string, fps: number, shot: number | null) => {
      const seconds = shot === null ? 12.5 : 2.5;
      const frames = Math.max(1, Math.round(seconds * fps));
      return Promise.resolve({ ...RENDER_FIXTURE, entity: id, fps, frames, seconds, message: `${frames} frames · ${seconds.toFixed(1)}s at ${fps} fps` });
    }),
    cinemaRenderStart: vi.fn((id: string, fps: number, shot: number | null, stem: string) => {
      RENDER_FIXTURE.running = true;
      RENDER_FIXTURE.done = false;
      RENDER_FIXTURE.entity = id;
      RENDER_FIXTURE.fps = fps;
      RENDER_FIXTURE.stem = stem;
      RENDER_FIXTURE.frames = shot === null ? 60 : 24;
      RENDER_FIXTURE.seconds = RENDER_FIXTURE.frames / Math.max(1, fps);
      RENDER_FIXTURE.written = 0;
      RENDER_FIXTURE.bytes = 0;
      RENDER_FIXTURE.width = 0;
      RENDER_FIXTURE.height = 0;
      RENDER_FIXTURE.folder = "C:/renders/skid-weld-line";
      RENDER_FIXTURE.failures = [];
      RENDER_FIXTURE.message = `Rendering frame 1 of ${RENDER_FIXTURE.frames}`;
      RENDER_FIXTURE.reason = null;
      return Promise.resolve({ ...RENDER_FIXTURE });
    }),
    cinemaRenderStatus: vi.fn(() => {
      if (RENDER_FIXTURE.running) {
        RENDER_FIXTURE.written = Math.min(RENDER_FIXTURE.frames, RENDER_FIXTURE.written + 20);
        RENDER_FIXTURE.width = 1920;
        RENDER_FIXTURE.height = 803;
        RENDER_FIXTURE.bytes = RENDER_FIXTURE.written * 512_000;
        RENDER_FIXTURE.elapsedMs = RENDER_FIXTURE.written * 40;
        if (RENDER_FIXTURE.written >= RENDER_FIXTURE.frames) {
          RENDER_FIXTURE.running = false;
          RENDER_FIXTURE.done = true;
          RENDER_FIXTURE.message = `Rendered ${RENDER_FIXTURE.frames} frames at 1920x803 in 2.4s`;
        } else {
          RENDER_FIXTURE.message = `Rendering frame ${RENDER_FIXTURE.written + 1} of ${RENDER_FIXTURE.frames}`;
        }
      }
      return Promise.resolve({ ...RENDER_FIXTURE });
    }),
    cinemaRenderCancel: vi.fn(() => {
      RENDER_FIXTURE.running = false;
      RENDER_FIXTURE.done = true;
      RENDER_FIXTURE.message = `Render stopped — ${RENDER_FIXTURE.written} frame(s) kept.`;
      return Promise.resolve({ ...RENDER_FIXTURE });
    }),
    viewportCapture: vi.fn(() =>
      Promise.resolve({
        ...RENDER_FIXTURE,
        running: false,
        done: true,
        frames: 1,
        written: 1,
        width: 1920,
        height: 803,
        folder: "C:/renders/frame.png",
        bytes: 512_000,
        message: "Saved 1920x803 to C:/renders/frame.png",
        reason: null,
      }),
    ),
    conditionCatalog: vi.fn(() => Promise.resolve([
      { kind: "score_at_least", label: "The Score is at least…", blurb: "gate this behind points the player has already earned", needs: "number", reads: "the Score is at least {n}" },
      { kind: "still_active", label: "It hasn't been used yet", blurb: "this object has not been collected or beaten", needs: "none", reads: "it hasn't been used yet" },
      { kind: "other_gone", label: "Another object is gone", blurb: "that collectible has been collected, or that enemy beaten", needs: "object", reads: "{name} is gone" },
    ])),
    conditionAdd: vi.fn((id: string) => Promise.resolve({ applied: "score_at_least", entity: id, added: ["the Score is at least 3"], scoreEntity: null, message: "Only if the Score is at least 3", reason: null })),
    conditionRemove: vi.fn((id: string) => Promise.resolve({ applied: null, entity: id, added: [], scoreEntity: null, message: "Condition removed", reason: null })),
    conditionList: vi.fn(() => Promise.resolve({ all: [], any: [], roleClause: null, sentence: "" })),
    assetLabAudit: (id) => Promise.resolve({ ok: false, message: "No mesh audit fixture", sourceEntity: id, sourceHandle: null, createdEntity: null, createdHandle: null, audit: null, change: null, warnings: [], exportedPath: null, bakeEvidence: null }),
    assetLabProcess: (id) => Promise.resolve({ ok: false, message: "No mesh process fixture", sourceEntity: id, sourceHandle: null, createdEntity: null, createdHandle: null, audit: null, change: null, warnings: [], exportedPath: null, bakeEvidence: null }),
    assetLabExport: (id) => Promise.resolve({ ok: false, message: "No mesh export fixture", sourceEntity: id, sourceHandle: null, createdEntity: null, createdHandle: null, audit: null, change: null, warnings: [], exportedPath: null, bakeEvidence: null }),
    sceneExport: (format) => Promise.resolve({ ok: false, message: "No scene export fixture", format, exportedPath: null, nodes: 0, meshes: 0, skins: 0, animations: 0, fidelity: [] }),
    addLight: () => Promise.resolve("light-created"),
    renameEntity: () => Promise.resolve(true),
    groupEntities: () => Promise.resolve("group-1"),
    ungroupEntity: () => Promise.resolve(true),
    multiEdit: vi.fn((ids: string[]) => Promise.resolve({ ok: true, changed: ids.length, reason: null })),
    setRotation: vi.fn((ids: string[]) => Promise.resolve({ ok: true, changed: ids.length, reason: null })),
    deleteDeactivate: vi.fn(() => Promise.resolve(true)),
    deleteDeactivateMany: vi.fn(() => Promise.resolve(true)),
    copySubtree: vi.fn(),
    cutSubtree: () => Promise.resolve(true),
    pasteClipboard: () => Promise.resolve("paste-1"),
    // M8 physics / M9 transform / focus (Tauri-only; inert defaults — a test overrides what it exercises).
    spawnBody: () => Promise.resolve("body-1"),
    setSimRunning: vi.fn(),
    simOverlay: vi.fn(),
    simTimeline: () => Promise.resolve([0, 0, false, false, 0]),
    simScrub: () => Promise.resolve([0, 0, false, false, 0]),
    simShove: () => Promise.resolve(true),
    physicsContacts: () => Promise.resolve([]),
    physicsCheck: () => Promise.resolve([]),
    physicsFix: () => Promise.resolve(true),
    importInterchange: () => Promise.resolve({ ok: true, format: "urdf", bodies: 2, joints: 1, meters_per_unit: 1, kilograms_per_unit: 1, reconciled: true, notes: [], error: null }),
    gizmoMode: vi.fn(),
    gizmoSelect: () => Promise.resolve(true),
    gizmoSelected: () => Promise.resolve(null),
    // THE THREE OF THESE ARE ONE PIECE OF STATE, AND THEY USED TO BE THREE CONSTANTS THAT
    // CONTRADICTED EACH OTHER. `gizmoSpaceToggle` answered "local" for ever while `gizmoDebug` kept
    // answering "world", so any refresh landing AFTER a toggle reverted the toolbar's label to the
    // value it had just been told to leave. Whether the assertion or the refresh won was decided by
    // how loaded the machine was — a real product would answer both reads from one place, and a fake
    // that does not is a flake generator, not a simplification.
    gizmoDebug: () => Promise.resolve(["translate", false, false, gizmoState.space, gizmoState.pivot]),
    gizmoSpaceToggle: () => {
      gizmoState.space = gizmoState.space === "world" ? "local" : "world";
      return Promise.resolve(gizmoState.space);
    },
    gizmoPivotToggle: () => {
      gizmoState.pivot = gizmoState.pivot === "origin" ? "center" : "origin";
      return Promise.resolve(gizmoState.pivot);
    },
    gizmoPickDrag: () => Promise.resolve(false),
    gizmoDragEnd: vi.fn(),
    readTransform: () => Promise.resolve([0, 0, 0, 0, 0, 0, 1, 1]),
    saveCharacter: () => Promise.resolve("comp-1"),
    instantiateCharacter: () => Promise.resolve("inst-1"),
    setPartActive: () => Promise.resolve(true),
    reparentPart: vi.fn(),
    setSnap: vi.fn(),
    snapQuery: () => Promise.resolve([]),
    applyConstraint: () => Promise.resolve({ ok: true, reason: null, intents: [] }),
    placementSentence: () => Promise.resolve({ ok: true, reason: null, intents: ["upright"] }),
    unfocus: vi.fn(),
    focusDebug: () => Promise.resolve([20, true]),
    frameAll: vi.fn(),
    viewPreset: vi.fn(),
    cameraDebug: () => Promise.resolve([0.785, 0.5, 60, 0, 0, 0]),
    setRenderProfile: (profile) => Promise.resolve(profile),
    renderProfileDebug: () => Promise.resolve("cinematic"),
    viewportPick: () => Promise.resolve(null),
    // Nothing under the cursor by default — the honest answer for a client with no viewport. A test
    // that drives the aim gesture overrides it with the id it means to be pointing at.
    viewportPeek: vi.fn((): Promise<string | null> => Promise.resolve(null)),
    // The lit count the real engine answers with. A test that cares asserts on the ARGUMENT — which
    // subjects the editor asked the stage to light — because the count is the engine's to produce.
    viewportHover: vi.fn((_ids: string[]): Promise<number> => Promise.resolve(0)),
    dragStart: vi.fn(),
    dragEnd: vi.fn(),
    zoom: vi.fn(),
    thumbnail: () => Promise.resolve(null), // M14.2: default to the icon fallback (a test overrides for the ready path)
    catalog: () => Promise.resolve({}),
    catalogSearch: () => Promise.resolve({ items: [] }),
    addItem: vi.fn(() => Promise.resolve({ created: "e-new", balance: null, seam: null })),
    importAsset: vi.fn(() => Promise.resolve("imported-1")),
    importAssetDialog: vi.fn(() => Promise.resolve("imported-1")),
    projectState: () => Promise.resolve({ path: null, dirty: false, recents: [], error: null }),
    newProject: vi.fn(() => Promise.resolve({ path: null, dirty: false, recents: [], error: null })),
    openProject: vi.fn(() => Promise.resolve({ path: "p.mtk", dirty: false, recents: ["p.mtk"], error: null })),
    saveProject: vi.fn(() => Promise.resolve({ path: "p.mtk", dirty: false, recents: ["p.mtk"], error: null })),
    saveProjectAs: vi.fn(() => Promise.resolve({ path: "p.mtk", dirty: false, recents: ["p.mtk"], error: null })),
    play: vi.fn(() => Promise.resolve({ playing: true, paused: false })),
    stop: vi.fn(() => Promise.resolve({ playing: false, paused: false })),
    pause: vi.fn(() => Promise.resolve({ playing: true, paused: true })),
    playState: () => Promise.resolve({ playing: false, paused: false }),
    // M12.1 Rules (a test overrides what it exercises).
    ruleRegistry: () => Promise.resolve({ events: [], actions: [], components: [] }),
    listRules: vi.fn(() => Promise.resolve([])),
    authorRule: vi.fn(() => Promise.resolve({ id: "rule-1", error: null, mirror: null })),
    deleteRule: vi.fn(() => Promise.resolve(true)),
    // M12.2 state machines (a test overrides what it exercises).
    stateMachines: vi.fn(() => Promise.resolve([])),
    authorStateMachine: vi.fn(() => Promise.resolve({ id: "sm-1", error: null, unreachable: [] })),
    deleteStateMachine: vi.fn(() => Promise.resolve(true)),
    // M12.4 AI compose (a test overrides what it exercises).
    proposeComposition: vi.fn(() => Promise.resolve({ ok: false, composition: null, ops: 0, error: null })),
    compose: vi.fn(() => Promise.resolve({ ok: true, applied: 0, rules: 0, stateMachines: 0, error: null })),
    // M12.5 Rules in Play + the truth-state debugger (a test overrides what it exercises).
    fireRuleEvent: vi.fn(() => Promise.resolve({ playing: true, frame: 0, head: 0, truth: null, explanations: [], decisions: [], flagged: [] })),
    ruleDebug: vi.fn(() => Promise.resolve({ playing: false, frame: 0, head: 0, truth: null, explanations: [], decisions: [], flagged: [] })),
    ruleScrub: vi.fn(() => Promise.resolve({ playing: true, frame: 0, head: 0, truth: null, explanations: [], decisions: [], flagged: [] })),
    // The authored match (ADR-097). The default is a scene with NO match, because that is the state every
    // other panel's test starts in — a test that wants a match overrides `matchValidate`.
    matchValidate: vi.fn(() =>
      Promise.resolve({ ok: false, is_match_scene: false, diagnostics: [], cook_digest: null, actor_count: 0, wave_count: 0, lane_length_m: 0 }),
    ),
    matchAuthorStarter: vi.fn(() =>
      Promise.resolve({ settings: "s", lane: "l", waypoints: ["w0", "w1"], actors: ["a0", "a1", "a2"], waves: ["v0"] }),
    ),
    matchStart: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchStep: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchOrderMove: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchAttackMove: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchAttackTarget: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchHold: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchHalt: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchCast: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchStun: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchStatus: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    matchCooked: vi.fn(() => Promise.resolve(null)),
    matchStop: vi.fn(() => Promise.resolve(emptyMatchStatus())),
    terrainPresets: vi.fn(() => Promise.resolve([])),
    terrainCreate: vi.fn(() => Promise.resolve(emptyTerrainReply())),
    terrainDescribe: vi.fn((_text: string) => Promise.resolve(emptyTerrainReply())),
    terrainReadDescription: vi.fn((_text: string) => Promise.resolve(null)),
    terrainPlan: vi.fn((_text: string) => Promise.resolve(null)),
    terrainEdit: vi.fn(() => Promise.resolve(emptyTerrainReply())),
    terrainStats: vi.fn(() => Promise.resolve(emptyTerrainStats())),
    terrainPath: vi.fn(() =>
      Promise.resolve({ found: false, reason: "no navigation grids are resident yet", waypoints: [] }),
    ),
    terrainTool: vi.fn(() => Promise.resolve(true)),
    terrainPaintBegin: vi.fn(() => Promise.resolve()),
    terrainPaintEnd: vi.fn(() => Promise.resolve(emptyTerrainReply())),
    terrainRoutePoint: vi.fn(() => Promise.resolve(1)),
    terrainRouteClear: vi.fn(() => Promise.resolve(0)),
    terrainRouteCommit: vi.fn(() => Promise.resolve(emptyTerrainReply())),
    ...over,
  };
}

/** A terrain runtime with nothing resident — the honest default for a test that has not created one. */
export function emptyTerrainStats(): TerrainStats {
  return {
    active: false,
    residentChunks: 0,
    visibleChunks: 0,
    culledFrustum: 0,
    culledHorizon: 0,
    pendingBuilds: 0,
    completedBuilds: 0,
    drawnTriangles: 0,
    drawnInstances: 0,
    impostorInstances: 0,
    meshMb: 0,
    textureMb: 0,
    scatterMb: 0,
    colliderMb: 0,
    navMb: 0,
    totalMb: 0,
    budgetMb: 0,
    budgetFraction: 0,
    overBudget: false,
    buildUsTotal: 0,
    dominantStage: "field",
    problem: "",
  };
}

/** "There is no terrain" — the reply the shell sends before one is created. */
export function emptyTerrainReply(): TerrainReply {
  return {
    ok: false,
    entity: "",
    message: "there is no terrain in the scene yet",
    recipe: null,
    issues: [],
    stats: emptyTerrainStats(),
  };
}
