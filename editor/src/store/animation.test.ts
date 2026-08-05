//! Animation workspace view-state contract: contexts are independent projections over one animation
//! sequence, local drafts survive component unmounts, updates preserve unaffected references, and stale
//! project/entity caches can be pruned without touching explicitly retained workspaces.

import { afterEach, expect, test } from "vitest";
import {
  MAX_ANIMATION_ZOOM,
  MIN_ANIMATION_ZOOM,
  animationContextView,
  animationEditorStore,
  animationWorkspaceKey,
  animationWorkspaceView,
  type AnimationDraftValue,
  type AnimationWorkspaceIdentity,
} from "./animation";

const identity = (
  projectId: string,
  kind: "entity" | "sequence",
  id: string,
): AnimationWorkspaceIdentity => ({ projectId, scope: { kind, id } });

afterEach(() => animationEditorStore.getState().reset());

test("collision-free keys isolate projects, entities, and sequences", () => {
  const a = animationWorkspaceKey(identity("project/a", "entity", "b:c"));
  const b = animationWorkspaceKey(identity("project", "entity", "a/b:c"));
  const c = animationWorkspaceKey(identity("project/a", "sequence", "b:c"));
  expect(new Set([a, b, c]).size).toBe(3);

  const store = animationEditorStore.getState();
  store.setSearch(a, "3d", "walk");
  store.setSearch(b, "3d", "idle");
  store.setSearch(c, "3d", "camera");
  expect(animationContextView(animationEditorStore.getState(), a, "3d").search).toBe("walk");
  expect(animationContextView(animationEditorStore.getState(), b, "3d").search).toBe("idle");
  expect(animationContextView(animationEditorStore.getState(), c, "3d").search).toBe("camera");
});

test("switching 2D, 3D, and UI preserves each context's complete workspace view", () => {
  const key = animationWorkspaceKey(identity("game", "sequence", "main"));
  const store = animationEditorStore.getState();

  store.setActiveContext(key, "2d");
  store.setView(key, "2d", "curves");
  store.setSelection(key, "2d", ["sprite-x", "sprite-x"], ["key-2", "key-1", "key-2"], "key-2");
  store.setSearch(key, "2d", "sprite");
  store.setZoom(key, "2d", 4);
  store.setSnap(key, "2d", { grid: "frames", increment: 2, toMarkers: false });
  store.setTimeDisplay(key, "2d", "frames");
  store.setScroll(key, "2d", { x: 320, y: 48 });
  store.setInspectorOpen(key, "2d", false);
  store.setInspectorDisclosure(key, "2d", "asset-readiness", true);
  store.setDraft(key, "2d", "key:key-2:value", { frame: 12, value: [0.25, 0.75] });

  store.setActiveContext(key, "ui");
  store.setView(key, "ui", "timeline");
  store.setSelection(key, "ui", ["opacity"], ["hover-end"]);
  store.setSearch(key, "ui", "hover");
  store.setZoom(key, "ui", 1.5);
  store.setTimeDisplay(key, "ui", "seconds");

  const state = animationEditorStore.getState();
  const workspace = animationWorkspaceView(state, key);
  const twoD = animationContextView(state, key, "2d");
  const ui = animationContextView(state, key, "ui");
  expect(workspace.activeContext).toBe("ui");
  expect(twoD).toMatchObject({
    view: "curves",
    selectedTrackIds: ["sprite-x"],
    selectedKeyIds: ["key-2", "key-1"],
    selectionAnchorKeyId: "key-2",
    search: "sprite",
    zoom: 4,
    timeDisplay: "frames",
    scroll: { x: 320, y: 48 },
    inspector: { open: false, disclosures: { "asset-readiness": true } },
  });
  expect(twoD.snap).toMatchObject({ grid: "frames", increment: 2, toMarkers: false });
  expect(twoD.drafts["key:key-2:value"]).toEqual({ frame: 12, value: [0.25, 0.75] });
  expect(ui).toMatchObject({ selectedTrackIds: ["opacity"], selectedKeyIds: ["hover-end"], search: "hover", zoom: 1.5 });
  expect(animationContextView(state, key, "3d")).toMatchObject({ view: "timeline", search: "", zoom: 1 });
});

test("updates are immutable, clone caller drafts, and preserve unaffected context references", () => {
  const key = animationWorkspaceKey(identity("cad", "entity", "arm"));
  const store = animationEditorStore.getState();
  store.ensure(key);
  const beforeWorkspace = animationWorkspaceView(animationEditorStore.getState(), key);
  const before3d = beforeWorkspace.contexts["3d"];
  const beforeUi = beforeWorkspace.contexts.ui;

  const source = { value: [1, 2], nested: { unit: "degrees" } } satisfies AnimationDraftValue;
  store.setDraft(key, "3d", "joint:value", source);
  (source.value as number[]).push(3);
  source.nested.unit = "radians";

  const afterWorkspace = animationWorkspaceView(animationEditorStore.getState(), key);
  const after3d = afterWorkspace.contexts["3d"];
  expect(afterWorkspace).not.toBe(beforeWorkspace);
  expect(after3d).not.toBe(before3d);
  expect(afterWorkspace.contexts.ui).toBe(beforeUi);
  expect(after3d.drafts["joint:value"]).toEqual({ value: [1, 2], nested: { unit: "degrees" } });
  expect(Object.isFrozen(after3d)).toBe(true);
  expect(Object.isFrozen(after3d.drafts)).toBe(true);
  expect(Object.isFrozen((after3d.drafts["joint:value"] as { value: readonly number[] }).value)).toBe(true);
  expect(() => {
    (after3d.selectedKeyIds as string[]).push("illegal");
  }).toThrow(TypeError);
});

test("atomic context updates normalize selection, zoom, snapping, scroll, and anchor state", () => {
  const key = animationWorkspaceKey(identity("game", "entity", "hero"));
  const store = animationEditorStore.getState();
  store.updateContext(key, "3d", (current) => ({
    ...current,
    selectedTrackIds: ["", "root", "root", "hand"],
    selectedKeyIds: ["a", "b", "a"],
    selectionAnchorKeyId: "missing",
    zoom: Number.POSITIVE_INFINITY,
    snap: { ...current.snap, increment: 0 },
    scroll: { x: -10, y: Number.NaN },
  }));
  let view = animationContextView(animationEditorStore.getState(), key, "3d");
  expect(view.selectedTrackIds).toEqual(["root", "hand"]);
  expect(view.selectedKeyIds).toEqual(["a", "b"]);
  expect(view.selectionAnchorKeyId).toBe("a");
  expect(view.zoom).toBe(1);
  expect(view.snap.increment).toBe(1);
  expect(view.scroll).toEqual({ x: 0, y: 0 });

  store.setZoom(key, "3d", 1000);
  view = animationContextView(animationEditorStore.getState(), key, "3d");
  expect(view.zoom).toBe(MAX_ANIMATION_ZOOM);
  store.setZoom(key, "3d", -1000);
  expect(animationContextView(animationEditorStore.getState(), key, "3d").zoom).toBe(MIN_ANIMATION_ZOOM);
});

test("draft and disclosure lifecycle is granular and empty IDs cannot create hidden state", () => {
  const key = animationWorkspaceKey(identity("ui-kit", "sequence", "button-transition"));
  const store = animationEditorStore.getState();
  store.setInspectorDisclosure(key, "ui", "", true);
  store.setDraft(key, "ui", "", "ignored");
  expect(animationEditorStore.getState().workspaces[key]).toBeUndefined();

  store.setInspectorDisclosure(key, "ui", "diagnostics", true);
  store.setInspectorDisclosure(key, "ui", "asset", false);
  store.setDraft(key, "ui", "hover", { opacity: 0.8 });
  store.setDraft(key, "ui", "click", { scale: 0.95 });
  store.removeDraft(key, "ui", "hover");
  let view = animationContextView(animationEditorStore.getState(), key, "ui");
  expect(view.inspector.disclosures).toEqual({ diagnostics: true, asset: false });
  expect(view.drafts).toEqual({ click: { scale: 0.95 } });

  store.clearDrafts(key, "ui");
  view = animationContextView(animationEditorStore.getState(), key, "ui");
  expect(view.drafts).toEqual({});
  expect(view.inspector.disclosures).toEqual({ diagnostics: true, asset: false });
});

test("sequence graph surface, viewport, selection, watches, and draft persist independently of contexts", () => {
  const key = animationWorkspaceKey(identity("moba", "sequence", "hero-locomotion"));
  const store = animationEditorStore.getState();
  store.setActiveSurface(key, "graph");
  store.setGraphViewport(key, { x: -240, y: 90, zoom: 2.5 });
  store.setGraphSelection(key, ["walk", "walk", "blend"], ["edge-1", "edge-1"]);
  store.setGraphSearch(key, "blend");
  store.setGraphWatches(key, ["parameter-speed", "parameter-speed", "state"]);
  store.setGraphDraft(key, "graph-draft:hero-locomotion", { schemaVersion: 1, id: "graph", nodes: ["walk"] });
  store.setActiveContext(key, "ui");
  store.setSearch(key, "ui", "opacity");

  const view = animationWorkspaceView(animationEditorStore.getState(), key);
  expect(view.activeSurface).toBe("graph");
  expect(view.graph).toMatchObject({
    viewport: { x: -240, y: 90, zoom: 2.5 },
    selectedNodeIds: ["walk", "blend"],
    selectedEdgeIds: ["edge-1"],
    search: "blend",
    watches: ["parameter-speed", "state"],
  });
  expect(view.graph.drafts["graph-draft:hero-locomotion"]).toEqual({ schemaVersion: 1, id: "graph", nodes: ["walk"] });
  expect(animationContextView(animationEditorStore.getState(), key, "ui").search).toBe("opacity");
  expect(Object.isFrozen(view.graph.viewport)).toBe(true);
});

test("reset and prune remove only stale scoped records and honor retained keys", () => {
  const p1Entity = animationWorkspaceKey(identity("p1", "entity", "hero"));
  const p1Sequence = animationWorkspaceKey(identity("p1", "sequence", "main"));
  const p2Old = animationWorkspaceKey(identity("p2", "entity", "old"));
  const p2New = animationWorkspaceKey(identity("p2", "entity", "new"));
  const store = animationEditorStore.getState();
  store.setSearch(p1Entity, "3d", "hero");
  store.setSearch(p1Sequence, "3d", "main");
  store.setSearch(p2Old, "3d", "old");
  store.setSearch(p2New, "3d", "new");

  expect(store.prune({ projectId: "p1", keepKeys: [p1Sequence] })).toBe(1);
  expect(animationEditorStore.getState().workspaces[p1Entity]).toBeUndefined();
  expect(animationEditorStore.getState().workspaces[p1Sequence]).toBeDefined();
  expect(animationEditorStore.getState().workspaces[p2Old]).toBeDefined();

  expect(animationEditorStore.getState().prune({ projectId: "p2", maxEntries: 1 })).toBe(1);
  expect(animationEditorStore.getState().workspaces[p2Old]).toBeUndefined();
  expect(animationEditorStore.getState().workspaces[p2New]).toBeDefined();

  animationEditorStore.getState().reset(p1Sequence);
  expect(animationEditorStore.getState().workspaces[p1Sequence]).toBeUndefined();
  expect(animationEditorStore.getState().workspaces[p2New]).toBeDefined();
  animationEditorStore.getState().reset();
  expect(animationEditorStore.getState().workspaces).toEqual({});
  expect(animationEditorStore.getState().revision).toBe(0);
});
