//! Transport-level provenance hint (M11.5 / ADR-044, SA-34) — verified headless: after an import the real
//! `TauriClient` reads the `asset_provenance` projection and, when the just-imported asset perceptually
//! matches an already-loaded one (a near-duplicate the exact content-hash dedup misses), surfaces a single
//! lightweight toast — never a silent merge, never a blocked import. A hint failure must not break the import.

import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { createMockSession, TauriClient } from "./session";
import { toastStore } from "../store/toasts";
import { projectionStore } from "../store/projection";
import type { AnimationClipInstanceSaveRequest, AnimationGraphSaveRequest } from "./protocol";
import { cloneAnimationGraph, createLocomotionGraphPreset } from "../graph/animation-graph-model";

// A fake Tauri core: a stub Channel (the constructor opens the projection channel) + a scripted `invoke`.
function clientWith(invoke: (cmd: string, args?: unknown) => Promise<unknown>): TauriClient {
  const core = {
    invoke: vi.fn(invoke),
    Channel: class {
      onmessage: (m: unknown) => void = () => {};
    },
  };
  return new TauriClient(core as unknown as ConstructorParameters<typeof TauriClient>[0]);
}

const texts = (): string[] => toastStore.getState().toasts.map((t) => t.text);

beforeEach(() => toastStore.getState().reset());
afterEach(() => {
  vi.restoreAllMocks();
  projectionStore.getState().reset();
});

test("native clip setup sends the complete revision-pinned request in one invoke", async () => {
  const request: AnimationClipInstanceSaveRequest = {
    expectedRevision: "clip-intents-7",
    requestId: "setup-request-1",
    instanceId: "clip-instance-1",
    name: "Hero turn",
    logicalAssetId: "project-animation:hero",
    expectedAssetRevision: "animation-revision-4",
    clipId: "turn",
    expectedSourceBindingHash: "binding-signature-9",
    targetMappings: [
      { sourceTargetId: "gltf-target:root", targetId: "12:34" },
      { sourceTargetId: "gltf-target:blade", targetId: "56:78" },
    ],
    targetId: "12:34",
  };
  const result = { ok: true, message: "ready", instanceId: request.instanceId, state: {} };
  const invoke = vi.fn((cmd: string) => Promise.resolve(
    cmd === "animation_clip_instance_save" ? result : null,
  ));
  const client = clientWith(invoke);

  await expect(client.animationClipInstanceSave(request)).resolves.toBe(result);
  expect(invoke).toHaveBeenCalledWith("animation_clip_instance_save", { request });
});

test("native clip discard carries stable identity, revision fence, and workspace selection", async () => {
  const result = { ok: true, message: "discarded", instanceId: "clip-instance-1", state: {} };
  const invoke = vi.fn(() => Promise.resolve(result));
  const client = clientWith(invoke);

  await expect(client.animationClipInstanceDelete(
    "clip-instance-1",
    "clip-intents-7",
    "12:34",
  )).resolves.toBe(result);
  expect(invoke).toHaveBeenCalledWith("animation_clip_instance_delete", {
    instanceId: "clip-instance-1",
    expectedRevision: "clip-intents-7",
    selectedId: "12:34",
  });
});

test("native clip preview cleanup carries an optional request fence", async () => {
  const response = {
    ok: true,
    message: "stopped",
    currentTick: 0,
    durationTick: 60_000,
    playing: false,
    loopPolicy: "once" as const,
    evaluatedTracks: 0,
    crossedEvents: [],
    eventsTruncated: false,
  };
  const invoke = vi.fn(() => Promise.resolve(response));
  const client = clientWith(invoke);

  await expect(client.animationClipInstancePreviewStop("setup-a")).resolves.toBe(response);
  expect(invoke).toHaveBeenCalledWith("animation_clip_instance_preview_stop", {
    expectedRequestId: "setup-a",
  });
  await expect(client.animationClipInstancePreviewStop()).resolves.toBe(response);
  expect(invoke).toHaveBeenLastCalledWith("animation_clip_instance_preview_stop", {
    expectedRequestId: null,
  });
});

test("browser clip mock previews and saves a complete distinct two-node map while refusing incomplete and many-to-one drafts", async () => {
  projectionStore.getState().bulkLoad([
    { id: "hero", name: "Hero", parentId: null, components: { Transform: { x: 0 }, MeshRenderer: {} } },
    { id: "child", name: "Child", parentId: null, components: { Transform: { x: 1 } } },
  ]);
  const client = createMockSession();
  const state = await client.animationState("hero");
  const multi = state.asset?.clips.find((clip) => clip.clipId === "multi-node-demo");
  expect(multi?.sourceTargetIds).toEqual(["gltf-target:fixture-root", "gltf-target:fixture-child"]);
  const request: AnimationClipInstanceSaveRequest = {
    expectedRevision: state.asset!.clipInstanceRevision,
    requestId: "setup-multi",
    instanceId: "clip-multi",
    name: "Two-part assembly",
    logicalAssetId: state.asset!.logicalId!,
    expectedAssetRevision: state.asset!.revisionId!,
    clipId: multi!.clipId,
    expectedSourceBindingHash: multi!.sourceBindingHash,
    targetId: "hero",
    targetMappings: [{ sourceTargetId: "gltf-target:fixture-root", targetId: "hero" }],
  };
  await expect(client.animationClipInstancePreview(request)).resolves.toMatchObject({
    ok: false,
    evaluatedTracks: 0,
  });
  await expect(client.animationClipInstancePreview({
    ...request,
    targetMappings: [
      { sourceTargetId: "gltf-target:fixture-root", targetId: "hero" },
      { sourceTargetId: "gltf-target:fixture-child", targetId: "hero" },
    ],
  })).resolves.toMatchObject({ ok: false, evaluatedTracks: 0 });
  const complete = {
    ...request,
    targetMappings: [
      { sourceTargetId: "gltf-target:fixture-root", targetId: "hero" },
      { sourceTargetId: "gltf-target:fixture-child", targetId: "child" },
    ],
  };
  await expect(client.animationClipInstancePreview(complete)).resolves.toMatchObject({
    ok: true,
    durationTick: 90_000,
    evaluatedTracks: 2,
  });
  const saved = await client.animationClipInstanceSave(complete);
  expect(saved.ok).toBe(true);
  expect(saved.state.asset?.clips.find((clip) => clip.clipId === "multi-node-demo")).toMatchObject({
    readiness: "ready",
    instanceId: "clip-multi",
  });
});

test("import surfaces ONE near-duplicate toast when the provenance projection reports a perceptual match", async () => {
  const client = clientWith((cmd) => {
    if (cmd === "import_asset") return Promise.resolve("ent-7");
    if (cmd === "asset_provenance")
      return Promise.resolve({ kind: "imported", source: "wide.glb", nearDuplicateOf: "ripple_quad.glb" });
    return Promise.resolve(null);
  });

  const id = await client.importAsset("/assets/wide.glb");

  expect(id).toBe("ent-7"); // the import contract is unchanged — callers still get the entity id
  const t = toastStore.getState().toasts;
  expect(t).toHaveLength(1);
  expect(t[0].kind).toBe("info");
  expect(t[0].text).toContain("ripple_quad.glb");
  expect(t[0].text).toMatch(/near-duplicate/i);
});

test("import surfaces NO toast when the asset is not a near-duplicate", async () => {
  const client = clientWith((cmd) => {
    if (cmd === "import_asset_dialog")
      return Promise.resolve({ entityId: "ent-8", outcome: "imported", message: "Imported unique.glb." });
    if (cmd === "asset_provenance")
      return Promise.resolve({ kind: "imported", source: "unique.glb", nearDuplicateOf: null });
    return Promise.resolve(null);
  });

  const reply = await client.importAssetDialog();

  expect(reply.entityId).toBe("ent-8");
  expect(texts()).toEqual([]);
});

// ADR-178 — A DISMISSED FILE DIALOG AND AN UNREADABLE FILE ARE DIFFERENT ANSWERS. The reply used to
// be `string | null` and `null` was both, so no caller could tell them apart and every one of them
// wrote the same "cancelled or unsupported". The transport now carries the outcome; these two cases
// assert that it survives the wire AND that neither one queries provenance for an entity that does
// not exist.
test("a cancelled import queries no provenance, posts no toast, and SAYS it was cancelled", async () => {
  const invoked: string[] = [];
  const client = clientWith((cmd) => {
    invoked.push(cmd);
    if (cmd === "import_asset_dialog")
      return Promise.resolve({
        entityId: null,
        outcome: "cancelled",
        message: "No file was chosen. Nothing in the scene changed.",
      });
    return Promise.resolve(null);
  });

  const reply = await client.importAssetDialog();

  expect(reply.entityId).toBeNull();
  expect(reply.outcome).toBe("cancelled");
  expect(texts()).toEqual([]);
  expect(invoked).not.toContain("asset_provenance");
});

test("a refused import is reported as FAILED, not as a cancellation", async () => {
  const invoked: string[] = [];
  const client = clientWith((cmd) => {
    invoked.push(cmd);
    if (cmd === "import_asset_dialog")
      return Promise.resolve({
        entityId: null,
        outcome: "failed",
        message: "sketch.dwg could not be read by this build.",
      });
    return Promise.resolve(null);
  });

  const reply = await client.importAssetDialog();

  expect(reply.outcome).toBe("failed");
  expect(reply.message).toContain("could not be read");
  expect(invoked).not.toContain("asset_provenance");
});

test("a chosen format narrows the native dialog's filter to that format's extensions", async () => {
  const seen: Record<string, unknown>[] = [];
  const client = clientWith((cmd, args) => {
    if (cmd === "import_asset_dialog") {
      seen.push((args ?? {}) as Record<string, unknown>);
      return Promise.resolve({ entityId: null, outcome: "cancelled", message: "No file was chosen." });
    }
    return Promise.resolve(null);
  });

  await client.importAssetDialog(["stp", "step"]);
  await client.importAssetDialog();

  // Two calls, two DIFFERENT arguments: a narrowed filter, then the explicit "no filter" that the
  // shell reads as every readable extension. `undefined` would be dropped from the payload, so the
  // unfiltered case is sent as an explicit null rather than by omission.
  expect(seen).toEqual([{ extensions: ["stp", "step"] }, { extensions: null }]);
});

test("a provenance-hint failure never breaks the import (best-effort)", async () => {
  vi.spyOn(console, "error").mockImplementation(() => {});
  const client = clientWith((cmd) => {
    if (cmd === "import_asset") return Promise.resolve("ent-9");
    if (cmd === "asset_provenance") return Promise.reject(new Error("projection unavailable"));
    return Promise.resolve(null);
  });

  await expect(client.importAsset("/x.glb")).resolves.toBe("ent-9");
  expect(texts()).toEqual([]);
});

test("browser graph service accepts v1 through explicit migration and rejects unknown peers", async () => {
  const client = createMockSession();
  const legacy = createLocomotionGraphPreset("main");
  (legacy as unknown as { schemaVersion: number }).schemaVersion = 1;
  const blend = legacy.nodes.find((node) => node.kind === "blend_1d")!;
  blend.samples = [];
  blend.thresholds = [0, 1];
  const migrated = await client.animationGraphSave("main", {
    schemaVersion: 1,
    expectedRevision: "mock-graph-0",
    requestId: "legacy-peer",
    graph: legacy,
  } as unknown as AnimationGraphSaveRequest);
  expect(migrated.ok).toBe(true);
  expect(migrated.state.graph?.schemaVersion).toBe(2);
  expect(migrated.state.graph?.nodes.find((node) => node.id === blend.id)?.samples).toHaveLength(2);

  const unknown = cloneAnimationGraph(migrated.state.graph!);
  (unknown as unknown as { schemaVersion: number }).schemaVersion = 3;
  const rejected = await client.animationGraphSave("main", {
    schemaVersion: 3,
    expectedRevision: migrated.state.revision,
    requestId: "newer-peer",
    graph: unknown,
  } as unknown as AnimationGraphSaveRequest);
  expect(rejected.ok).toBe(false);
  expect(rejected.state.revision).toBe(migrated.state.revision);
});

test("browser graph mock persists semantic-invalid drafts, merges levels, consumes triggers, and fabricates no weights", async () => {
  const client = createMockSession();
  const graph = createLocomotionGraphPreset("main");
  graph.parameters.push({ id: "parameter-jump", name: "Jump", kind: "trigger", defaultValue: false, min: null, max: null });
  const valid = await client.animationGraphSave("main", { schemaVersion: 2, expectedRevision: "mock-graph-0", requestId: "valid", graph });
  expect(valid.ok).toBe(true);
  expect(valid.state.compile.state).toBe("ready");

  const invalid = cloneAnimationGraph(graph);
  invalid.edges = invalid.edges.filter((edge) => edge.toNodeId !== invalid.outputNodeId);
  const savedInvalid = await client.animationGraphSave("main", { schemaVersion: 2, expectedRevision: valid.state.revision, requestId: "semantic-invalid", graph: invalid });
  expect(savedInvalid.ok).toBe(true);
  expect(savedInvalid.state.graph).toEqual(invalid);
  expect(savedInvalid.state.compile).toMatchObject({ state: "invalid", lastGoodRevision: valid.state.revision, lastGoodHash: valid.state.compile.compiledHash });

  await client.animationGraphSetPreviewParameters(graph.id, { "parameter-speed": 0.4 });
  const trigger = await client.animationGraphSetPreviewParameters(graph.id, { "parameter-moving": true, "parameter-jump": true });
  expect(trigger.accepted).toEqual({ "parameter-moving": true, "parameter-jump": true });
  const debug = await client.animationGraphDebug(graph.id, null, ["parameter-speed", "parameter-moving", "parameter-jump"]);
  expect(debug.parameterValues).toEqual({ "parameter-speed": 0.4, "parameter-moving": true });
  expect(debug.parameterValues).not.toHaveProperty("parameter-jump");
  expect(debug.activeNodes).toEqual([]);
  expect(debug.activeEdges).toEqual([]);
  expect(debug.evaluationCostMicros).toBeNull();
  expect(debug.truncated).toBe(true);
});
