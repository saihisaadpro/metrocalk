import { afterEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { AnimationWorkspace } from "./AnimationWorkspace";
import { animationContextView, animationEditorStore, animationWorkspaceKey } from "../store/animation";
import { projectionStore } from "../store/projection";
import { projectStore } from "../store/project";
import { uiStore } from "../store/ui";
import { fakeClient } from "../transport/test-client";
import type { AnimationClipInstanceSaveRequest, AnimationEditResult, AnimationPlaybackInfo, AnimationWorkspaceInfo } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

afterEach(() => {
  cleanup();
  projectionStore.getState().reset();
  animationEditorStore.getState().reset();
  projectStore.getState().reset();
});

const selectedKeyId = (trackId: string, keyId: string) => JSON.stringify([trackId, keyId]);
const heroWorkspaceKey = () => animationWorkspaceKey({ projectId: projectStore.getState().sessionId, scope: { kind: "entity", id: "hero" } });

function model(overrides: Partial<AnimationWorkspaceInfo> = {}): AnimationWorkspaceInfo {
  return {
    revision: "rev-7",
    sequenceId: "main",
    sequenceName: "Main sequence",
    ticksPerSecond: 60,
    durationTick: 120,
    currentTick: 0,
    playing: false,
    loopPolicy: "once",
    selectedId: "hero",
    selectedName: "Hero",
    properties: [
      { component: "Transform", property: "x", label: "Transform / x", valueKind: "number", value: 4, animatable: true, reason: null, context: "3d", editorKind: "scalar", bindingState: "ready", bindingReason: "Native transform sink is ready.", runtimeSink: "transform" },
      { component: "Sprite", property: "frame", label: "Sprite / frame", valueKind: "number", value: 2, animatable: true, reason: null, context: "2d", editorKind: "sprite_frame", bindingState: "ready", bindingReason: "Sprite frame adapter is ready.", runtimeSink: "sprite" },
      { component: "UiStyle", property: "opacity", label: "UI / opacity", valueKind: "number", value: 1, animatable: true, reason: null, context: "ui", editorKind: "scalar", bindingState: "preview_only", bindingReason: "UI preview adapter is available; runtime sink is pending.", runtimeSink: "ui-preview" },
    ],
    tracks: [
      {
        id: "track-x", name: "Transform.x", targetId: "hero", targetName: "Hero", component: "Transform", property: "x", valueKind: "number", interpolation: "cubic", enabled: true, locked: false, context: "3d", editorKind: "scalar", bindingState: "ready", bindingReason: "Native transform sink is ready.", runtimeSink: "transform",
        keys: [{ id: "key-0", tick: 0, seconds: 0, value: 4, inTangent: 0, outTangent: 1 }],
      },
      {
        id: "track-frame", name: "Sprite.frame", targetId: "hero", targetName: "Hero", component: "Sprite", property: "frame", valueKind: "number", interpolation: "step", enabled: true, locked: false, context: "2d", editorKind: "sprite_frame", bindingState: "ready", bindingReason: "Sprite frame adapter is ready.", runtimeSink: "sprite",
        keys: [{ id: "frame-0", tick: 0, seconds: 0, value: 2, inTangent: null, outTangent: null }],
      },
      {
        id: "track-opacity", name: "UI.opacity", targetId: "hero", targetName: "Hero", component: "UiStyle", property: "opacity", valueKind: "number", interpolation: "linear", enabled: true, locked: false, context: "ui", editorKind: "scalar", bindingState: "preview_only", bindingReason: "UI runtime sink is pending.", runtimeSink: "ui-preview",
        keys: [{ id: "opacity-0", tick: 30, seconds: 0.5, value: 0.5, inTangent: null, outTangent: null }],
      },
    ],
    markers: [{ id: "marker-contact", ownerId: "hero", name: "Contact", tick: 20, seconds: 1 / 3, color: [1, 0.7, 0.2, 1] }],
    events: [{ id: "event-step", ownerId: "hero", name: "Footstep", tick: 25, seconds: 25 / 60, payload: { sound: "step" } }],
    contexts: [
      { context: "2d", state: "ready", properties: 1, tracks: 1, reason: "Sprite adapter ready.", action: null },
      { context: "3d", state: "ready", properties: 1, tracks: 1, reason: "Rigid transform adapter ready.", action: null },
      { context: "ui", state: "preview_only", properties: 1, tracks: 1, reason: "UI preview only.", action: "Add the UI runtime sink." },
    ],
    asset: {
      displayName: "Hero",
      source: "glTF import",
      provenance: "content-addressed",
      qualityGrade: "B · rigid-ready",
      logicalId: "animation:hero",
      revisionId: "revision:hero-v1",
      importState: "ready_with_warnings",
      dependencyCount: 0,
      sourceLocation: "project_relative",
      watchesSource: true,
      reimportDiagnostics: 1,
      skeletonJoints: 42,
      clipCount: 1,
      morphTargets: 0,
      rootMotion: "not detected",
      reimportBinding: "stable skeleton signature",
      clipInstanceRevision: "clip-instances-0",
      clips: [],
      capabilities: [
        { capability: "Property animation", state: "available", reason: "Typed values are ready.", action: null },
        { capability: "GPU skeletal deformation", state: "unsupported", reason: "Skin palette is not uploaded yet.", action: "Use rigid transform preview." },
      ],
      suggestions: ["Use a rigid transform track for this tier."],
    },
    issues: [],
    ...overrides,
  };
}

function editResult(state = model(), overrides: Partial<AnimationEditResult> = {}): AnimationEditResult {
  return { ok: true, message: "Animation updated.", trackId: null, keyId: null, state, ...overrides };
}

function playback(action: "play" | "pause" | "stop" | "scrub", tick?: number): AnimationPlaybackInfo {
  return { ok: true, message: "Preview updated.", currentTick: tick ?? 0, durationTick: 120, playing: action === "play", loopPolicy: "once", evaluatedTracks: 1, crossedEvents: [], eventsTruncated: false };
}

function selectHero() {
  projectionStore.getState().bulkLoad([{ id: "hero", name: "Hero", parentId: null, components: { Transform: { x: 4 }, Sprite: { frame: 2 }, UiStyle: { opacity: 1 } } }]);
  projectionStore.getState().select("hero");
}

async function renderWorkspace(overrides: Partial<EditorClient> = {}) {
  selectHero();
  const client = fakeClient({
    animationState: () => Promise.resolve(model()),
    animationTransport: (action, tick) => Promise.resolve(playback(action, tick)),
    ...overrides,
  });
  await act(async () => {
    render(<AnimationWorkspace client={client} />);
  });
  await screen.findByTestId("animation-context-readiness");
  return client;
}

test("keys the current typed property and scrubs through the native animation contract", async () => {
  const animationKey = vi.fn(() => Promise.resolve(editResult(model(), { message: "Key added.", trackId: "track-x", keyId: "key-0" })));
  const animationTransport = vi.fn((action: "play" | "pause" | "stop" | "scrub", tick?: number) => Promise.resolve(playback(action, tick)));
  await renderWorkspace({ animationKey, animationTransport });

  await waitFor(() => expect((screen.getByTestId("animation-add-key") as HTMLButtonElement).disabled).toBe(false));
  fireEvent.click(screen.getByTestId("animation-add-key"));
  await waitFor(() => expect(animationKey).toHaveBeenCalledWith("hero", "Transform", "x", 0, "linear"));
  fireEvent.change(screen.getByTestId("animation-scrub"), { target: { value: "30" } });
  await waitFor(() => expect(animationTransport).toHaveBeenCalledWith("scrub", 30, undefined));
});

test("context and view switches preserve per-context selection, filter, zoom, and drafts over one sequence", async () => {
  await renderWorkspace();
  expect(screen.getAllByTestId("animation-track")).toHaveLength(1);
  expect(screen.getByText("Transform.x")).toBeTruthy();
  fireEvent.click(screen.getByTestId("animation-key"));
  fireEvent.change(screen.getByTestId("animation-search"), { target: { value: "transform" } });
  fireEvent.change(screen.getByTestId("animation-zoom"), { target: { value: String(Math.log2(3)) } });
  fireEvent.click(screen.getByRole("tab", { name: /^Curves/ }));
  expect(await screen.findByTestId("animation-curves")).toBeTruthy();

  fireEvent.click(screen.getByRole("tab", { name: /^2D/ }));
  expect(screen.getByText("Sprite.frame")).toBeTruthy();
  expect((screen.getByTestId("animation-search") as HTMLInputElement).value).toBe("");
  fireEvent.change(screen.getByTestId("animation-search"), { target: { value: "sprite" } });
  fireEvent.change(screen.getByTestId("animation-marker-name"), { target: { value: "Loop point" } });

  fireEvent.click(screen.getByRole("tab", { name: /^3D/ }));
  expect(screen.getByRole("tab", { name: /^Curves/ }).getAttribute("aria-selected")).toBe("true");
  expect((screen.getByTestId("animation-search") as HTMLInputElement).value).toBe("transform");
  const key = heroWorkspaceKey();
  const state = animationEditorStore.getState();
  expect(animationContextView(state, key, "3d")).toMatchObject({ selectedTrackIds: ["track-x"], selectedKeyIds: [selectedKeyId("track-x", "key-0")], search: "transform", zoom: 3, view: "curves" });
  expect(animationContextView(state, key, "2d")).toMatchObject({ search: "sprite", drafts: { "workspace:new-marker-name": "Loop point" } });
});

test("Graph is a sequence-scoped workspace surface across 2D, 3D, and UI context changes", async () => {
  const animationGraphState = vi.fn((sequenceId: string) => Promise.resolve({
    schemaVersion: 2 as const,
    sequenceId,
    revision: "graph-0",
    graph: null,
    nodePresentation: [],
    sources: [],
    compile: { state: "missing" as const, authoredRevision: "graph-0", compiledRevision: null, compiledHash: null, lastGoodRevision: null, lastGoodHash: null, message: "No graph." },
    diagnostics: [],
  }));
  await renderWorkspace({ animationGraphState });
  const surfaces = screen.getByRole("tablist", { name: "Animation authoring surface" });
  fireEvent.click(within(surfaces).getByRole("tab", { name: "Graph" }));
  expect(await screen.findByText("No graph is authored for this sequence")).toBeTruthy();
  expect(animationGraphState).toHaveBeenCalledWith("main");

  fireEvent.click(screen.getByRole("tab", { name: /^UI/ }));
  expect(screen.getByTestId("animation-graph-editor")).toBeTruthy();
  fireEvent.click(within(surfaces).getByRole("tab", { name: "Timeline" }));
  expect(await screen.findByTestId("animation-timeline-viewport")).toBeTruthy();
  fireEvent.click(within(surfaces).getByRole("tab", { name: "Graph" }));
  expect(await screen.findByTestId("animation-graph-editor")).toBeTruthy();
});

test("filters only the active context and explains readiness without duplicating tracks", async () => {
  await renderWorkspace();
  expect(screen.queryByText("Sprite.frame")).toBeNull();
  fireEvent.click(screen.getByRole("tab", { name: /^UI/ }));
  expect(screen.getAllByTestId("animation-track")).toHaveLength(1);
  expect(screen.getByText("UI.opacity")).toBeTruthy();
  expect(screen.getByTestId("animation-context-readiness").getAttribute("data-state")).toBe("preview_only");
  fireEvent.change(screen.getByTestId("animation-search"), { target: { value: "not-present" } });
  expect(screen.queryByText("UI.opacity")).toBeNull();
  expect(screen.getByText("No matching tracks")).toBeTruthy();
});

test("keeps unsupported channels inspectable while preventing unsafe keying", async () => {
  const invalid = model({
    properties: [
      ...model().properties,
      { component: "Light", property: "cookie", label: "Light / cookie", valueKind: "string", value: "asset.png", animatable: false, reason: "Texture references need a dedicated discrete adapter.", context: "3d", editorKind: "text", bindingState: "unsupported", bindingReason: "No safe animation adapter is registered.", runtimeSink: null },
    ],
  });
  await renderWorkspace({ animationState: () => Promise.resolve(invalid) });
  fireEvent.change(screen.getByTestId("animation-property"), { target: { value: `Light\u0000cookie` } });
  expect((screen.getByTestId("animation-add-key") as HTMLButtonElement).disabled).toBe(true);
  expect(screen.getByText("No safe animation adapter is registered.")).toBeTruthy();
});

test("renders typed 2D and UI previews from the same sequence without a parallel editor", async () => {
  await renderWorkspace();
  expect(screen.queryByTestId("animation-context-preview")).toBeNull();

  fireEvent.click(screen.getByRole("tab", { name: /^2D/ }));
  const spritePreview = screen.getByTestId("animation-context-preview");
  expect(spritePreview.getAttribute("data-context")).toBe("2d");
  expect(within(spritePreview).getByText(/deployment adapter required.*frame 2/)).toBeTruthy();
  expect(within(spritePreview).getByLabelText("2D animation preview")).toBeTruthy();

  fireEvent.click(screen.getByRole("tab", { name: /^UI/ }));
  const uiPreview = screen.getByTestId("animation-context-preview");
  expect(uiPreview.getAttribute("data-context")).toBe("ui");
  expect(within(uiPreview).getByText(/deployment adapter required/)).toBeTruthy();
  expect(within(uiPreview).getByLabelText("UI animation preview")).toBeTruthy();
});

test("curve view renders authored interpolation instead of a misleading straight key polyline", async () => {
  const curveModel = model({
    tracks: model().tracks.map((track) => track.id === "track-x" ? {
      ...track,
      keys: [
        { id: "key-0", tick: 0, seconds: 0, value: 4, inTangent: 0, outTangent: 0.3 },
        { id: "key-120", tick: 120, seconds: 2, value: 10, inTangent: -0.2, outTangent: 0 },
      ],
    } : track),
  });
  await renderWorkspace({ animationState: () => Promise.resolve(curveModel) });
  fireEvent.click(screen.getAllByTestId("animation-key")[0]);
  fireEvent.click(screen.getByRole("tab", { name: /^Curves/ }));
  const curve = await screen.findByTestId("animation-curve-path");
  expect(curve.getAttribute("data-interpolation")).toBe("cubic");
  expect((curve.getAttribute("d")?.match(/ L /g) ?? []).length).toBeGreaterThan(8);
});

test("mute and lock are granular native edits and update their structured visual state", async () => {
  const muted = model({ tracks: model().tracks.map((track) => track.id === "track-x" ? { ...track, enabled: false } : track) });
  const locked = model({ tracks: muted.tracks.map((track) => track.id === "track-x" ? { ...track, locked: true } : track) });
  const animationSetTrackEnabled = vi.fn(() => Promise.resolve(editResult(muted)));
  const animationSetTrackLocked = vi.fn(() => Promise.resolve(editResult(locked)));
  await renderWorkspace({ animationSetTrackEnabled, animationSetTrackLocked });

  fireEvent.click(screen.getByRole("button", { name: "Mute Transform.x" }));
  await waitFor(() => expect(animationSetTrackEnabled).toHaveBeenCalledWith("hero", "track-x", false));
  expect(screen.getByTestId("animation-track").getAttribute("data-enabled")).toBe("false");

  fireEvent.click(screen.getByRole("button", { name: "Lock Transform.x" }));
  await waitFor(() => expect(animationSetTrackLocked).toHaveBeenCalledWith("hero", "track-x", true));
  expect(screen.getByTestId("animation-track").getAttribute("data-locked")).toBe("true");
});

test("authors and deletes shared markers and events with stable owner IDs", async () => {
  const animationAddMarker = vi.fn(() => Promise.resolve(editResult()));
  const animationDeleteMarker = vi.fn(() => Promise.resolve(editResult()));
  const animationAddEvent = vi.fn(() => Promise.resolve(editResult()));
  const animationDeleteEvent = vi.fn(() => Promise.resolve(editResult()));
  await renderWorkspace({ animationAddMarker, animationDeleteMarker, animationAddEvent, animationDeleteEvent });

  fireEvent.change(screen.getByTestId("animation-marker-name"), { target: { value: "Apex" } });
  fireEvent.click(screen.getByTestId("animation-add-marker"));
  await waitFor(() => expect(animationAddMarker).toHaveBeenCalledWith("hero", "Apex", 0));
  fireEvent.change(screen.getByTestId("animation-event-name"), { target: { value: "Footstep" } });
  fireEvent.change(screen.getByTestId("animation-event-payload"), { target: { value: '{"sound":"stone"}' } });
  fireEvent.click(screen.getByTestId("animation-add-event"));
  await waitFor(() => expect(animationAddEvent).toHaveBeenCalledWith("hero", "Footstep", 0, { sound: "stone" }));

  fireEvent.click(screen.getByRole("button", { name: "Delete marker Contact" }));
  await waitFor(() => expect(animationDeleteMarker).toHaveBeenCalledWith("hero", "marker-contact"));
  fireEvent.click(screen.getByRole("button", { name: "Delete event Footstep" }));
  await waitFor(() => expect(animationDeleteEvent).toHaveBeenCalledWith("hero", "event-step"));
});

test("moves an exact key tick and edits cubic tangents without storing a stale key snapshot", async () => {
  const moved = model({ tracks: model().tracks.map((track) => track.id === "track-x" ? { ...track, keys: [{ ...track.keys[0], tick: 37, seconds: 37 / 60 }] } : track) });
  const animationUpdateKeys = vi.fn(() => Promise.resolve(editResult(moved)));
  await renderWorkspace({ animationUpdateKeys });
  fireEvent.click(screen.getByTestId("animation-key"));

  const tickField = await screen.findByTestId("animation-key-tick");
  fireEvent.change(tickField, { target: { value: "37" } });
  fireEvent.blur(tickField);
  await waitFor(() => expect(animationUpdateKeys).toHaveBeenCalledWith("hero", "track-x", [{ keyId: "key-0", tick: 37 }]));
  expect((screen.getByTestId("animation-key-tick") as HTMLInputElement).value).toBe("37");

  const tangent = screen.getByTestId("animation-key-out-tangent");
  fireEvent.change(tangent, { target: { value: "2.5" } });
  fireEvent.blur(tangent);
  await waitFor(() => expect(animationUpdateKeys).toHaveBeenLastCalledWith("hero", "track-x", [{ keyId: "key-0", outTangent: 2.5 }]));
});

test("plain, modifier, and range clicks provide stable multi-key selection and batch delete", async () => {
  const multiKeyModel = model({
    tracks: model().tracks.map((track) => track.id === "track-x" ? {
      ...track,
      keys: [
        { id: "key-0", tick: 0, seconds: 0, value: 4, inTangent: 0, outTangent: 1 },
        { id: "key-30", tick: 30, seconds: 0.5, value: 8, inTangent: 1, outTangent: 1 },
        { id: "key-60", tick: 60, seconds: 1, value: 2, inTangent: 1, outTangent: 0 },
      ],
    } : track),
  });
  const animationDeleteKeys = vi.fn(() => Promise.resolve(editResult(multiKeyModel)));
  await renderWorkspace({ animationState: () => Promise.resolve(multiKeyModel), animationDeleteKeys });

  const keys = screen.getAllByTestId("animation-key");
  fireEvent.click(keys[0]);
  fireEvent.click(keys[2], { shiftKey: true });
  expect(keys.every((key) => key.getAttribute("data-selected") === "true")).toBe(true);

  fireEvent.click(keys[1], { ctrlKey: true });
  const workspaceKey = heroWorkspaceKey();
  expect(animationContextView(animationEditorStore.getState(), workspaceKey, "3d")).toMatchObject({
    selectedTrackIds: ["track-x"],
    selectedKeyIds: [selectedKeyId("track-x", "key-0"), selectedKeyId("track-x", "key-60")],
    selectionAnchorKeyId: selectedKeyId("track-x", "key-0"),
  });

  keys[0].focus();
  fireEvent.keyDown(keys[0], { key: "Delete" });
  await waitFor(() => expect(animationDeleteKeys).toHaveBeenCalledTimes(1));
  expect(animationDeleteKeys).toHaveBeenCalledWith("hero", [
    { targetId: "hero", trackId: "track-x", keyId: "key-0" },
    { targetId: "hero", trackId: "track-x", keyId: "key-60" },
  ]);
});

test("keeps focused animation controls from leaking viewport shortcuts to the window", async () => {
  const animationTransport = vi.fn((action: "play" | "pause" | "stop" | "scrub", tick?: number) => Promise.resolve(playback(action, tick)));
  await renderWorkspace({ animationTransport });
  const leaked = vi.fn();
  window.addEventListener("keydown", leaked);
  const loopPolicy = screen.getByLabelText("Loop policy");
  fireEvent.keyDown(loopPolicy, { key: "e" });
  expect(leaked).not.toHaveBeenCalled();
  const workspace = screen.getByTestId("animation-workspace");
  workspace.focus();
  fireEvent.keyDown(workspace, { key: "w" });
  expect(leaked).not.toHaveBeenCalled();
  fireEvent.keyDown(screen.getByRole("button", { name: "Select track Transform.x" }), { key: " " });
  expect(animationTransport).not.toHaveBeenCalledWith("play", undefined, undefined);
  window.removeEventListener("keydown", leaked);
});

test("keeps annotation drafts after a rejected native edit", async () => {
  const animationAddEvent = vi.fn(() => Promise.resolve(editResult(model(), { ok: false, message: "Payload type is not supported." })));
  await renderWorkspace({ animationAddEvent });
  fireEvent.change(screen.getByTestId("animation-event-name"), { target: { value: "Impact" } });
  fireEvent.change(screen.getByTestId("animation-event-payload"), { target: { value: "keep-me" } });
  fireEvent.click(screen.getByTestId("animation-add-event"));
  await waitFor(() => expect(animationAddEvent).toHaveBeenCalled());
  expect((screen.getByTestId("animation-event-name") as HTMLInputElement).value).toBe("Impact");
  expect((screen.getByTestId("animation-event-payload") as HTMLInputElement).value).toBe("keep-me");
});

test("shows measured capability limits and deletes a stable key independently", async () => {
  const withoutKey = model({ tracks: model().tracks.map((track) => track.id === "track-x" ? { ...track, keys: [] } : track) });
  const animationDeleteKeys = vi.fn(() => Promise.resolve(editResult(withoutKey, { message: "Key removed.", trackId: "track-x", keyId: "key-0" })));
  await renderWorkspace({ animationDeleteKeys });

  fireEvent.click(screen.getByText("Asset readiness"));
  expect(await screen.findByTestId("animation-asset-lifecycle")).toBeTruthy();
  expect(screen.getByText("ready with warnings")).toBeTruthy();
  const facts = await screen.findAllByTestId("animation-capability");
  expect(facts.some((fact) => fact.getAttribute("data-state") === "unsupported")).toBe(true);
  fireEvent.click(screen.getByTestId("animation-key"));
  const inspector = screen.getByRole("complementary", { name: "Animation inspector" });
  fireEvent.click(within(inspector).getByTestId("animation-delete-key"));
  await waitFor(() => expect(animationDeleteKeys).toHaveBeenCalledWith("hero", [{ targetId: "hero", trackId: "track-x", keyId: "key-0" }]));
});

test("reviews, confirms, and surfaces a ready single-node rigid clip instance", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root (FixtureRoot#0)"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [
      { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "translation", valueKind: "vec3" as const },
      { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "rotation", valueKind: "quaternion" as const },
      { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "scale", valueKind: "vec3" as const },
    ],
    channels: ["translation (vec3)", "rotation (quaternion)", "scale (vec3)"],
    readiness: "setup_available" as const,
    reason: "One rigid source node can be explicitly mapped to the selected entity.",
    action: "Review and set up this clip.",
    instanceId: null,
  };
  const multi = {
    ...clip,
    clipId: "assembly",
    name: "Two-part assembly",
    sourceTargets: ["Fixture root (FixtureRoot#0)", "Fixture child (FixtureRoot#0/FixtureChild#0)"],
    sourceTargetIds: ["gltf-target:fixture-root", "gltf-target:fixture-child"],
    sourceBindings: [
      { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "translation", valueKind: "vec3" as const },
      { sourceTargetId: "gltf-target:fixture-child", sourceTargetLabel: "Fixture child (FixtureRoot#0/FixtureChild#0)", component: "Transform", property: "rotation", valueKind: "quaternion" as const },
    ],
    readiness: "explicit_mapping_required" as const,
    reason: "Unsafe one-entity auto-map is disabled.",
  };
  const setupState = model({
    asset: { ...base.asset!, clipInstanceRevision: "clip-instances-0", clips: [clip, multi] },
  });
  const readyState = model({
    durationTick: 120,
    asset: {
      ...base.asset!,
      clipInstanceRevision: "clip-instances-1",
      clips: [{ ...clip, readiness: "ready" as const, reason: "Explicit mapping compiled.", instanceId: "clip-ready" }, multi],
    },
  });
  const animationClipInstanceSave = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve({
    ok: true,
    message: "Clip is ready in Graph.",
    instanceId: "clip-ready",
    state: readyState,
  }));
  const animationClipInstancePreview = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve({
    ok: true,
    message: "Loaded unsaved clip at frame zero.",
    currentTick: 0,
    durationTick: 120_000,
    playing: false,
    loopPolicy: "once" as const,
    evaluatedTracks: 3,
    crossedEvents: [],
    eventsTruncated: false,
  }));
  const animationClipInstancePreviewStop = vi.fn(() => Promise.resolve({
    ok: true,
    message: "Authored transforms restored.",
    currentTick: 0,
    durationTick: 60_000,
    playing: false,
    loopPolicy: "once" as const,
    evaluatedTracks: 0,
    crossedEvents: [],
    eventsTruncated: false,
  }));
  const animationTransport = vi.fn((action: "play" | "pause" | "stop" | "scrub", tick?: number) =>
    Promise.resolve({
      ...playback(action, tick),
      durationTick: 120_000,
      importedClipPreviewActive: true,
    })
  );
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstanceSave,
    animationClipInstancePreview,
    animationClipInstancePreviewStop,
    animationTransport,
  });

  fireEvent.click(screen.getByText("Asset readiness"));
  expect((screen.getByTestId("animation-clip-map-assembly") as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(screen.getByTestId("animation-clip-setup-rigid-demo"));
  const dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  expect(within(dialog).getByText(/Fixture root \(FixtureRoot#0\)/)).toBeTruthy();
  expect(dialog.textContent).toContain("Transform.scale · vec3 → Transform.scale3 · vec3");
  expect(dialog.textContent).toContain("Target: Hero");
  const previewButton = within(dialog).getByTestId("animation-clip-preview");
  expect(document.activeElement).toBe(previewButton);
  fireEvent.click(previewButton);
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledTimes(1));
  expect(animationClipInstanceSave).not.toHaveBeenCalled();
  expect(within(dialog).getByText("Stop preview")).toBeTruthy();
  expect(within(dialog).getByText("Resume audition")).toBeTruthy();
  fireEvent.click(within(dialog).getByTestId("animation-clip-audition-play"));
  await waitFor(() => expect(animationTransport).toHaveBeenCalledWith("play", undefined, undefined));
  await waitFor(() => expect(within(dialog).getByText("Pause audition")).toBeTruthy());
  fireEvent.click(within(dialog).getByTestId("animation-clip-audition-play"));
  await waitFor(() => expect(animationTransport).toHaveBeenCalledWith("pause", undefined, undefined));
  fireEvent.change(within(dialog).getByTestId("animation-clip-audition-scrub"), { target: { value: "60000" } });
  await waitFor(() => expect(animationTransport).toHaveBeenCalledWith("scrub", 60000, undefined));
  expect(within(dialog).getByText(/Pause or scrub here while setup stays open/)).toBeTruthy();
  fireEvent.click(within(dialog).getByText("Stop preview"));
  await waitFor(() => expect(animationClipInstancePreviewStop).toHaveBeenCalledTimes(1));
  fireEvent.click(within(dialog).getByTestId("animation-clip-confirm"));

  await waitFor(() => expect(animationClipInstanceSave).toHaveBeenCalledWith(expect.objectContaining({
    expectedRevision: "clip-instances-0",
    logicalAssetId: "animation:hero",
    expectedAssetRevision: "revision:hero-v1",
    clipId: "rigid-demo",
    expectedSourceBindingHash: "binding-v1",
    targetId: "hero",
  })));
  const previewRequest = animationClipInstancePreview.mock.calls[0][0];
  const saveRequest = animationClipInstanceSave.mock.calls[0][0];
  expect(previewRequest.requestId).toMatch(new RegExp(`^${saveRequest.requestId}:preview:`));
  expect(saveRequest.instanceId).toBe(previewRequest.instanceId);
  expect((await screen.findByTestId("animation-clip-ready")).textContent).toContain("Ready for Graph");
});

test("maps every multi-node source from a user perspective, refuses conflicts, and keeps exact-name suggestions non-committing", async () => {
  const base = model();
  const multiClip = {
    clipId: "assembly",
    sequenceId: "assembly-source",
    name: "Two-part assembly",
    durationTick: 90_000,
    sourceBindingHash: "assembly-bindings-v1",
    sourceTargets: ["Fixture root (FixtureRoot#0)", "Fixture child (FixtureRoot#0/FixtureChild#0)"],
    sourceTargetIds: ["gltf-target:fixture-root", "gltf-target:fixture-child"],
    sourceBindings: [
      { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "translation", valueKind: "vec3" as const },
      { sourceTargetId: "gltf-target:fixture-root", sourceTargetLabel: "Fixture root (FixtureRoot#0)", component: "Transform", property: "scale", valueKind: "vec3" as const },
      { sourceTargetId: "gltf-target:fixture-child", sourceTargetLabel: "Fixture child (FixtureRoot#0/FixtureChild#0)", component: "Transform", property: "rotation", valueKind: "quaternion" as const },
    ],
    channels: ["translation (vec3)", "scale (vec3)", "rotation (quaternion)"],
    readiness: "explicit_mapping_required" as const,
    reason: "Every source node needs a distinct live scene target.",
    action: "Map every source node.",
    instanceId: null,
  };
  const setupState = model({
    asset: { ...base.asset!, clipInstanceRevision: "clip-instances-0", clips: [multiClip] },
  });
  const readyState = model({
    asset: {
      ...base.asset!,
      clipInstanceRevision: "clip-instances-1",
      clips: [{ ...multiClip, readiness: "ready" as const, reason: "Every source node is mapped.", instanceId: "clip-assembly-ready" }],
    },
  });
  const animationClipInstancePreview = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve({
    ok: true,
    message: "Previewing both mapped parts.",
    currentTick: 0,
    durationTick: 90_000,
    playing: true,
    loopPolicy: "once" as const,
    evaluatedTracks: 3,
    crossedEvents: [],
    eventsTruncated: false,
  }));
  const animationClipInstancePreviewStop = vi.fn(() => Promise.resolve({
    ok: true,
    message: "Authored transforms restored.",
    currentTick: 0,
    durationTick: 60_000,
    playing: false,
    loopPolicy: "once" as const,
    evaluatedTracks: 0,
    crossedEvents: [],
    eventsTruncated: false,
  }));
  const animationClipInstanceSave = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve({
    ok: true,
    message: "Two-part clip is ready in Graph.",
    instanceId: "clip-assembly-ready",
    state: readyState,
  }));
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstancePreview,
    animationClipInstancePreviewStop,
    animationClipInstanceSave,
  });
  act(() => {
    projectionStore.getState().bulkLoad([
      { id: "hero", name: "Hero", parentId: null, components: { Transform: { x: 4 }, MeshRenderer: {} } },
      { id: "target-root", name: "Fixture root", parentId: null, components: { Transform: { x: 0 } } },
      { id: "target-child-a", name: "Fixture child", parentId: null, components: { Transform: { x: 1 } } },
      { id: "target-child-b", name: "Fixture child", parentId: null, components: { Transform: { x: 2 } } },
      { id: "target-decoy", name: "Fixture root", parentId: null, components: { MeshRenderer: {} } },
    ]);
  });

  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-map-assembly"));
  const dialog = screen.getByRole("dialog", { name: "Map targets for Two-part assembly" });
  const rootTarget = within(dialog).getByTestId("animation-clip-target-0") as HTMLSelectElement;
  const childTarget = within(dialog).getByTestId("animation-clip-target-1") as HTMLSelectElement;
  const previewButton = within(dialog).getByTestId("animation-clip-preview") as HTMLButtonElement;
  const confirmButton = within(dialog).getByTestId("animation-clip-confirm") as HTMLButtonElement;
  expect(document.activeElement).toBe(childTarget);
  expect(previewButton.disabled).toBe(true);
  expect(confirmButton.disabled).toBe(true);
  expect(dialog.textContent).toContain("Transform.scale · vec3 → Transform.scale3 · vec3");

  fireEvent.change(childTarget, { target: { value: "hero" } });
  expect(within(dialog).getAllByText(/distinct target/)).toHaveLength(2);
  expect(rootTarget.getAttribute("aria-invalid")).toBe("true");
  expect(childTarget.getAttribute("aria-invalid")).toBe("true");
  expect(childTarget.getAttribute("aria-describedby")).toBeTruthy();
  expect(document.getElementById(childTarget.getAttribute("aria-describedby")!)).toBeTruthy();
  expect(previewButton.disabled).toBe(true);

  fireEvent.change(rootTarget, { target: { value: "" } });
  fireEvent.change(childTarget, { target: { value: "" } });
  fireEvent.click(within(dialog).getByTestId("animation-clip-suggest-exact"));
  await waitFor(() => expect(rootTarget.value).toBe("target-root"));
  expect(childTarget.value).toBe("");
  expect(animationClipInstancePreview).not.toHaveBeenCalled();
  expect(animationClipInstanceSave).not.toHaveBeenCalled();

  fireEvent.change(childTarget, { target: { value: "target-child-a" } });
  await waitFor(() => expect(previewButton.disabled).toBe(false));
  fireEvent.click(previewButton);
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledWith(expect.objectContaining({
    expectedRevision: "clip-instances-0",
    expectedSourceBindingHash: "assembly-bindings-v1",
    targetMappings: [
      { sourceTargetId: "gltf-target:fixture-root", targetId: "target-root" },
      { sourceTargetId: "gltf-target:fixture-child", targetId: "target-child-a" },
    ],
  })));

  fireEvent.change(childTarget, { target: { value: "target-child-b" } });
  await waitFor(() => expect(animationClipInstancePreviewStop).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(childTarget.value).toBe("target-child-b"));
  fireEvent.click(confirmButton);
  await waitFor(() => expect(animationClipInstanceSave).toHaveBeenCalledWith(expect.objectContaining({
    requestId: animationClipInstancePreview.mock.calls[0][0].requestId.split(":preview:")[0],
    instanceId: animationClipInstancePreview.mock.calls[0][0].instanceId,
    targetMappings: [
      { sourceTargetId: "gltf-target:fixture-root", targetId: "target-root" },
      { sourceTargetId: "gltf-target:fixture-child", targetId: "target-child-b" },
    ],
  })));
  expect((await screen.findByTestId("animation-clip-ready")).textContent).toContain("Ready for Graph");
});

test("repairs a stale mapping in place while a deliberate second instance gets a new identity", async () => {
  const base = model();
  const sourceTargetId = "gltf-target:fixture-root";
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v2",
    sourceTargets: ["Fixture root (FixtureRoot#0)"],
    sourceTargetIds: [sourceTargetId],
    sourceBindings: [{
      sourceTargetId,
      sourceTargetLabel: "Fixture root (FixtureRoot#0)",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "repair_required" as const,
    reason: "The persisted instance needs its source and live target fences revalidated.",
    action: "Repair the mapping.",
    instanceId: "clip-stable-graph-source",
    targetMappings: [{ sourceTargetId, targetId: "hero" }],
    repairChanges: [
      "Removed · old-target/Transform/scale · vec3",
      "Added · gltf-target:fixture-root/Transform/translation · vec3",
    ],
  };
  const repairState = model({
    asset: { ...base.asset!, revisionId: "revision:hero-v2", clipInstanceRevision: "clip-instances-4", clips: [clip] },
  });
  const readyState = model({
    asset: {
      ...base.asset!,
      revisionId: "revision:hero-v2",
      clipInstanceRevision: "clip-instances-5",
      clips: [{ ...clip, readiness: "ready" as const, reason: "Compiled.", instanceId: "clip-stable-graph-source" }],
    },
  });
  const animationClipInstanceSave = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve({
    ok: true,
    message: "Mapping saved.",
    instanceId: "clip-stable-graph-source",
    state: readyState,
  }));
  await renderWorkspace({
    animationState: () => Promise.resolve(repairState),
    animationClipInstanceSave,
  });

  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-repair-rigid-demo"));
  const repairDialog = screen.getByRole("dialog", { name: "Repair mapping for Rigid showcase" });
  const changes = within(repairDialog).getByTestId("animation-clip-repair-changes");
  expect(changes.textContent).toContain("Removed · old-target/Transform/scale · vec3");
  expect(changes.textContent).toContain("Added · gltf-target:fixture-root/Transform/translation · vec3");
  expect((within(repairDialog).getByTestId("animation-clip-target-0") as HTMLSelectElement).value).toBe("hero");
  fireEvent.click(within(repairDialog).getByTestId("animation-clip-confirm"));
  await waitFor(() => expect(animationClipInstanceSave).toHaveBeenCalledTimes(1));
  const repaired = animationClipInstanceSave.mock.calls[0][0];
  expect(repaired).toMatchObject({
    expectedRevision: "clip-instances-4",
    expectedAssetRevision: "revision:hero-v2",
    instanceId: "clip-stable-graph-source",
    targetMappings: [{ sourceTargetId, targetId: "hero" }],
  });

  const another = await screen.findByTestId("animation-clip-setup-another-rigid-demo");
  fireEvent.click(another);
  const createDialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(createDialog).getByTestId("animation-clip-confirm"));
  await waitFor(() => expect(animationClipInstanceSave).toHaveBeenCalledTimes(2));
  const created = animationClipInstanceSave.mock.calls[1][0];
  expect(created.instanceId).not.toBe("clip-stable-graph-source");
  expect(created.instanceId).toMatch(/^clip-/);
  expect(created.requestId).not.toBe(repaired.requestId);
});

test("lists ambiguous stale instances by stable identity and lets the user repair or discard each one", async () => {
  const base = model();
  const sourceTargetId = "gltf-target:fixture-root";
  const instances = [
    {
      instanceId: "clip-stale-a",
      name: "Left fixture",
      readiness: "repair_required" as const,
      reason: "Source revision changed.",
      targetMappings: [{ sourceTargetId, targetId: "hero" }],
      repairChanges: ["Removed · old-root/Transform/scale · vec3"],
    },
    {
      instanceId: "clip-stale-b",
      name: "Right fixture",
      readiness: "repair_required" as const,
      reason: "Mapped target changed.",
      targetMappings: [{ sourceTargetId, targetId: "hero" }],
      repairChanges: ["Added · gltf-target:fixture-root/Transform/rotation · quaternion"],
    },
  ];
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source-v2",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v2",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: [sourceTargetId],
    sourceBindings: [{
      sourceTargetId,
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "repair_required" as const,
    reason: "Two persisted instances require individual review.",
    action: "Choose an instance.",
    instanceId: null,
    targetMappings: [],
    repairChanges: [],
    instances,
  };
  const repairState = model({
    asset: {
      ...base.asset!,
      revisionId: "revision:hero-v2",
      clipInstanceRevision: "clip-instances-7",
      clips: [clip],
    },
  });
  const afterDeleteState = model({
    asset: {
      ...base.asset!,
      revisionId: "revision:hero-v2",
      clipInstanceRevision: "clip-instances-8",
      clips: [{ ...clip, instances: [instances[1]] }],
    },
  });
  const animationClipInstanceSave = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve({
    ok: true,
    message: "Chosen instance repaired.",
    instanceId: "clip-stale-b",
    state: repairState,
  }));
  const animationClipInstanceDelete = vi.fn(() => Promise.resolve({
    ok: true,
    message: "Chosen instance discarded.",
    instanceId: "clip-stale-a",
    state: afterDeleteState,
  }));
  await renderWorkspace({
    animationState: () => Promise.resolve(repairState),
    animationClipInstanceSave,
    animationClipInstanceDelete,
  });

  fireEvent.click(screen.getByText("Asset readiness"));
  const options = screen.getAllByTestId("animation-clip-instance-option");
  expect(options).toHaveLength(2);
  expect(options[0].textContent).toContain("clip-stale-a");
  expect(options[1].textContent).toContain("clip-stale-b");

  fireEvent.click(screen.getByTestId("animation-clip-repair-instance-clip-stale-b"));
  const repairDialog = screen.getByRole("dialog", { name: "Repair mapping for Rigid showcase" });
  expect(within(repairDialog).getByTestId("animation-clip-repair-changes").textContent)
    .toContain("Added · gltf-target:fixture-root/Transform/rotation · quaternion");
  fireEvent.click(within(repairDialog).getByTestId("animation-clip-confirm"));
  await waitFor(() => expect(animationClipInstanceSave).toHaveBeenCalledWith(expect.objectContaining({
    expectedRevision: "clip-instances-7",
    instanceId: "clip-stale-b",
    targetMappings: [{ sourceTargetId, targetId: "hero" }],
  })));

  fireEvent.click(screen.getByTestId("animation-clip-discard-clip-stale-a"));
  expect(screen.getByRole("alert").textContent).toContain("Discard this persisted instance?");
  fireEvent.click(screen.getByTestId("animation-clip-discard-confirm-clip-stale-a"));
  await waitFor(() => expect(animationClipInstanceDelete).toHaveBeenCalledWith(
    "clip-stale-a",
    "clip-instances-7",
    "hero",
  ));
  await waitFor(() => expect(screen.getAllByTestId("animation-clip-instance-option")).toHaveLength(1));
  expect(screen.getByTestId("animation-clip-instance-option").textContent).toContain("clip-stale-b");
});

test("a cancelled in-flight preview cannot stop a newer preview", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const setupState = model({ asset: { ...base.asset!, clips: [clip] } });
  let resolveFirst!: (value: AnimationPlaybackInfo) => void;
  const first = new Promise<AnimationPlaybackInfo>((resolve) => { resolveFirst = resolve; });
  const success: AnimationPlaybackInfo = {
    ok: true,
    message: "Previewing.",
    currentTick: 0,
    durationTick: 120_000,
    playing: true,
    loopPolicy: "once",
    evaluatedTracks: 1,
    crossedEvents: [],
    eventsTruncated: false,
  };
  const animationClipInstancePreview = vi.fn()
    .mockImplementationOnce(() => first)
    .mockResolvedValueOnce(success);
  const animationClipInstancePreviewStop = vi.fn((_expectedRequestId?: string) => Promise.resolve({
    ...success,
    playing: false,
    message: "Stopped matching preview.",
  }));
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstancePreview,
    animationClipInstancePreviewStop,
  });
  fireEvent.click(screen.getByText("Asset readiness"));

  const trigger = screen.getByTestId("animation-clip-setup-rigid-demo");
  fireEvent.click(trigger);
  let dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledTimes(1));
  const firstRequestId = animationClipInstancePreview.mock.calls[0][0].requestId;
  fireEvent.click(within(dialog).getByText("Cancel"));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Set up Rigid showcase" })).toBeNull());
  await waitFor(() => expect(animationClipInstancePreviewStop).toHaveBeenCalledWith(firstRequestId));

  fireEvent.click(trigger);
  dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledTimes(2));
  const secondRequestId = animationClipInstancePreview.mock.calls[1][0].requestId;
  expect(secondRequestId).not.toBe(firstRequestId);
  await waitFor(() => expect(within(dialog).getByText("Stop preview")).toBeTruthy());

  await act(async () => resolveFirst(success));
  await waitFor(() => expect(animationClipInstancePreviewStop.mock.calls.length).toBeGreaterThanOrEqual(2));
  expect(animationClipInstancePreviewStop.mock.calls.every(([requestId]) => requestId === firstRequestId)).toBe(true);
  expect(within(dialog).getByText("Stop preview")).toBeTruthy();
});

test("global Stop token-cancels an in-flight imported preview before it can install late", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const setupState = model({ asset: { ...base.asset!, clips: [clip] } });
  let resolvePreview!: (value: AnimationPlaybackInfo) => void;
  const pendingPreview = new Promise<AnimationPlaybackInfo>((resolve) => {
    resolvePreview = resolve;
  });
  const previewing: AnimationPlaybackInfo = {
    ok: true,
    message: "Previewing.",
    currentTick: 0,
    durationTick: 120_000,
    playing: true,
    importedClipPreviewActive: true,
    loopPolicy: "once",
    evaluatedTracks: 1,
    crossedEvents: [],
    eventsTruncated: false,
  };
  const restored: AnimationPlaybackInfo = {
    ...previewing,
    message: "Matching preview cancelled before installation.",
    playing: false,
    importedClipPreviewActive: false,
  };
  const animationClipInstancePreview = vi.fn((_request: AnimationClipInstanceSaveRequest) =>
    pendingPreview
  );
  const animationClipInstancePreviewStop = vi.fn((_expectedRequestId?: string) =>
    Promise.resolve(restored)
  );
  const animationTransport = vi.fn((action: "play" | "pause" | "stop" | "scrub", tick?: number) =>
    Promise.resolve(playback(action, tick))
  );
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstancePreview,
    animationClipInstancePreviewStop,
    animationTransport,
  });
  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-setup-rigid-demo"));
  const dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledTimes(1));
  const requestId = animationClipInstancePreview.mock.calls[0][0].requestId;

  fireEvent.click(screen.getByTestId("animation-stop"));
  await waitFor(() =>
    expect(animationClipInstancePreviewStop).toHaveBeenCalledWith(requestId)
  );
  expect(animationTransport.mock.calls.some(([action]) => action === "stop")).toBe(false);

  await act(async () => resolvePreview(previewing));
  await waitFor(() => expect(animationClipInstancePreviewStop).toHaveBeenCalledTimes(2));
  expect(animationClipInstancePreviewStop.mock.calls.every(([token]) => token === requestId)).toBe(true);
  expect(screen.getByRole("dialog", { name: "Set up Rigid showcase" })).toBeTruthy();
});

test("pending Cancel retains its cleanup token and mapper until restoration is confirmed", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const setupState = model({ asset: { ...base.asset!, clips: [clip] } });
  let resolvePreview!: (value: AnimationPlaybackInfo) => void;
  const pendingPreview = new Promise<AnimationPlaybackInfo>((resolve) => {
    resolvePreview = resolve;
  });
  const previewing: AnimationPlaybackInfo = {
    ok: true,
    message: "Previewing.",
    currentTick: 0,
    durationTick: 120_000,
    playing: true,
    importedClipPreviewActive: true,
    loopPolicy: "once",
    evaluatedTracks: 1,
    crossedEvents: [],
    eventsTruncated: false,
  };
  const restored: AnimationPlaybackInfo = {
    ...previewing,
    message: "Authored transforms restored.",
    playing: false,
    importedClipPreviewActive: false,
  };
  const stillActive: AnimationPlaybackInfo = {
    ...previewing,
    message: "A newer native preview remains active.",
    importedClipPreviewActive: true,
  };
  const animationClipInstancePreviewStop = vi.fn()
    .mockRejectedValueOnce(new Error("Stop response connection lost."))
    .mockResolvedValueOnce(stillActive)
    .mockResolvedValue(restored);
  const animationClipInstancePreview = vi.fn((_request: AnimationClipInstanceSaveRequest) => pendingPreview);
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstancePreview,
    animationClipInstancePreviewStop,
  });
  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-setup-rigid-demo"));
  let dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledTimes(1));
  const requestId = animationClipInstancePreview.mock.calls[0][0].requestId;

  fireEvent.click(within(dialog).getByText("Cancel"));
  expect(await within(dialog).findByText("Stop response connection lost.")).toBeTruthy();
  expect(screen.getByRole("dialog", { name: "Set up Rigid showcase" })).toBeTruthy();
  expect(animationClipInstancePreviewStop).toHaveBeenNthCalledWith(1, requestId);

  dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByText("Cancel"));
  expect(await within(dialog).findByText(
    "A native imported-clip preview is still active. The mapper retained its cleanup token; retry Cancel.",
  )).toBeTruthy();
  expect(animationClipInstancePreviewStop).toHaveBeenNthCalledWith(2, requestId);

  dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByText("Cancel"));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Set up Rigid showcase" })).toBeNull());
  expect(animationClipInstancePreviewStop).toHaveBeenNthCalledWith(3, requestId);

  await act(async () => resolvePreview(previewing));
  await waitFor(() => expect(animationClipInstancePreviewStop).toHaveBeenCalledTimes(4));
  expect(animationClipInstancePreviewStop.mock.calls.every(([token]) => token === requestId)).toBe(true);
});

test("reconciles a native one-shot preview lease after automatic authored-pose restoration", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const setupState = model({ asset: { ...base.asset!, clips: [clip] } });
  const previewing: AnimationPlaybackInfo = {
    ok: true,
    message: "Previewing.",
    currentTick: 0,
    durationTick: 120_000,
    playing: true,
    importedClipPreviewActive: true,
    loopPolicy: "once",
    evaluatedTracks: 1,
    crossedEvents: [],
    eventsTruncated: false,
  };
  const animationPlaybackState = vi.fn(() => Promise.resolve({
    ...previewing,
    message: "Animation clock synchronized.",
    playing: false,
    importedClipPreviewActive: false,
  }));
  const animationClipInstancePreviewStop = vi.fn(() => Promise.resolve({
    ...previewing,
    playing: false,
    importedClipPreviewActive: false,
  }));
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstancePreview: () => Promise.resolve(previewing),
    animationClipInstancePreviewStop,
    animationPlaybackState,
  });
  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-setup-rigid-demo"));
  const dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await within(dialog).findByText("Stop preview");

  await waitFor(() => expect(animationPlaybackState).toHaveBeenCalled(), { timeout: 2_000 });
  await waitFor(() => expect(uiStore.getState().status).toBe(
    "Imported clip preview finished; authored transforms and normal graph authority were restored.",
  ), { timeout: 2_000 });
  await waitFor(() => expect(within(dialog).getByText("Preview")).toBeTruthy());
  expect(animationPlaybackState).toHaveBeenCalled();
  expect(animationClipInstancePreviewStop).not.toHaveBeenCalled();
});

test("an unavailable clock reply cannot masquerade as confirmed preview restoration", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const setupState = model({ asset: { ...base.asset!, clips: [clip] } });
  const previewing: AnimationPlaybackInfo = {
    ok: true,
    message: "Previewing.",
    currentTick: 0,
    durationTick: 120_000,
    playing: true,
    importedClipPreviewActive: true,
    loopPolicy: "once",
    evaluatedTracks: 1,
    crossedEvents: [],
    eventsTruncated: false,
  };
  let resolveUnavailable!: (value: AnimationPlaybackInfo) => void;
  let resolveRestored!: (value: AnimationPlaybackInfo) => void;
  const unavailable = new Promise<AnimationPlaybackInfo>((resolve) => {
    resolveUnavailable = resolve;
  });
  const restored = new Promise<AnimationPlaybackInfo>((resolve) => {
    resolveRestored = resolve;
  });
  const animationPlaybackState = vi.fn()
    .mockImplementationOnce(() => unavailable)
    .mockImplementation(() => restored);
  const animationClipInstancePreviewStop = vi.fn();
  await renderWorkspace({
    animationState: () => Promise.resolve(setupState),
    animationClipInstancePreview: () => Promise.resolve(previewing),
    animationClipInstancePreviewStop,
    animationPlaybackState,
  });
  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-setup-rigid-demo"));
  const dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await within(dialog).findByText("Stop preview");
  await waitFor(() => expect(animationPlaybackState).toHaveBeenCalledTimes(1));

  await act(async () => resolveUnavailable({
    ...previewing,
    ok: false,
    message: "Animation clock reply unavailable.",
    playing: false,
    importedClipPreviewActive: false,
  }));
  expect(await within(dialog).findByText("Animation clock reply unavailable.")).toBeTruthy();
  expect(within(dialog).getByText("Stop preview")).toBeTruthy();

  await waitFor(() => expect(animationPlaybackState.mock.calls.length).toBeGreaterThanOrEqual(2));
  await act(async () => resolveRestored({
    ...previewing,
    message: "Animation clock synchronized.",
    playing: false,
    importedClipPreviewActive: false,
  }));
  await waitFor(() => expect(uiStore.getState().status).toBe(
    "Imported clip preview finished; authored transforms and normal graph authority were restored.",
  ));
  expect(within(dialog).getByText("Preview")).toBeTruthy();
  expect(animationClipInstancePreviewStop).not.toHaveBeenCalled();
});

test("a rejected preview refreshes the clip-instance fence before the mapper is reopened", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const staleState = model({ asset: { ...base.asset!, clipInstanceRevision: "clip-instances-0", clips: [clip] } });
  const currentState = model({ asset: { ...base.asset!, clipInstanceRevision: "clip-instances-1", clips: [clip] } });
  const animationState = vi.fn()
    .mockResolvedValueOnce(staleState)
    .mockResolvedValue(currentState);
  const rejected: AnimationPlaybackInfo = {
    ok: false,
    message: "Clip instances changed elsewhere while setup was open.",
    currentTick: 0,
    durationTick: 120_000,
    playing: false,
    loopPolicy: "once",
    evaluatedTracks: 0,
    crossedEvents: [],
    eventsTruncated: false,
  };
  const animationClipInstancePreview = vi.fn((_request: AnimationClipInstanceSaveRequest) => Promise.resolve(rejected));
  await renderWorkspace({ animationState, animationClipInstancePreview });
  fireEvent.click(screen.getByText("Asset readiness"));
  const trigger = screen.getByTestId("animation-clip-setup-rigid-demo");

  fireEvent.click(trigger);
  let dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await waitFor(() => expect(animationState).toHaveBeenCalledTimes(2));
  expect(await within(dialog).findByText(rejected.message)).toBeTruthy();
  fireEvent.click(within(dialog).getByText("Cancel"));
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Set up Rigid showcase" })).toBeNull());

  fireEvent.click(trigger);
  dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));
  await waitFor(() => expect(animationClipInstancePreview).toHaveBeenCalledTimes(2));
  expect(animationClipInstancePreview.mock.calls[1][0].expectedRevision).toBe("clip-instances-1");
});

test("a preview transport failure token-stops any native preview before releasing ownership", async () => {
  const base = model();
  const clip = {
    clipId: "rigid-demo",
    sequenceId: "rigid-source",
    name: "Rigid showcase",
    durationTick: 120_000,
    sourceBindingHash: "binding-v1",
    sourceTargets: ["Fixture root"],
    sourceTargetIds: ["gltf-target:fixture-root"],
    sourceBindings: [{
      sourceTargetId: "gltf-target:fixture-root",
      sourceTargetLabel: "Fixture root",
      component: "Transform",
      property: "translation",
      valueKind: "vec3" as const,
    }],
    channels: ["translation (vec3)"],
    readiness: "setup_available" as const,
    reason: "Ready to map.",
    action: "Set up.",
    instanceId: null,
  };
  const setupState = model({
    asset: {
      ...base.asset!,
      clipInstanceRevision: "clip-instances-0",
      clips: [clip],
    },
  });
  const animationClipInstancePreview = vi.fn((_request: AnimationClipInstanceSaveRequest) => (
    Promise.reject(new Error("Preview response connection lost."))
  ));
  const animationClipInstancePreviewStop = vi.fn((_expectedRequestId?: string) => Promise.resolve({
    ok: true,
    message: "Matching native preview stopped.",
    currentTick: 0,
    durationTick: 120_000,
    playing: false,
    loopPolicy: "once" as const,
    evaluatedTracks: 0,
    crossedEvents: [],
    eventsTruncated: false,
  }));
  const animationState = vi.fn(() => Promise.resolve(setupState));
  await renderWorkspace({
    animationState,
    animationClipInstancePreview,
    animationClipInstancePreviewStop,
  });
  fireEvent.click(screen.getByText("Asset readiness"));
  fireEvent.click(screen.getByTestId("animation-clip-setup-rigid-demo"));
  const dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  fireEvent.click(within(dialog).getByTestId("animation-clip-preview"));

  await waitFor(() => expect(animationClipInstancePreviewStop).toHaveBeenCalledTimes(1));
  const requestId = animationClipInstancePreview.mock.calls[0][0].requestId;
  expect(animationClipInstancePreviewStop).toHaveBeenCalledWith(requestId);
  expect(await within(dialog).findByText("Preview response connection lost.")).toBeTruthy();
  expect(within(dialog).getByTestId("animation-clip-preview")).toBeTruthy();
});

test("clip setup traps the first decision and Escape cancels back to its trigger", async () => {
  const base = model();
  const setupState: AnimationWorkspaceInfo = {
    ...base,
    selectedId: "hero",
    selectedName: "Hero",
    asset: {
      ...base.asset!,
      logicalId: "animation:hero",
      revisionId: "revision:hero-v1",
      clipInstanceRevision: "clip-instances-0",
      clips: [{
        clipId: "rigid-demo",
        sequenceId: "rigid-source",
        name: "Rigid showcase",
        durationTick: 120_000,
        sourceBindingHash: "binding-v1",
        sourceTargets: ["Fixture root (FixtureRoot#0)"],
        channels: ["translation (vec3)"],
        readiness: "setup_available",
        reason: "One rigid source node can be mapped.",
        action: "Review mapping.",
        instanceId: null,
      }],
    },
  };
  await renderWorkspace({ animationState: () => Promise.resolve(setupState) });
  fireEvent.click(screen.getByText("Asset readiness"));
  const trigger = screen.getByTestId("animation-clip-setup-rigid-demo");
  fireEvent.click(trigger);
  const dialog = screen.getByRole("dialog", { name: "Set up Rigid showcase" });
  expect(document.activeElement).toBe(within(dialog).getByTestId("animation-clip-preview"));
  fireEvent.keyDown(dialog, { key: "Escape" });
  await waitFor(() => expect(screen.queryByRole("dialog", { name: "Set up Rigid showcase" })).toBeNull());
  await waitFor(() => expect(document.activeElement).toBe(trigger));
});
