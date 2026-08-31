import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, expect, test, vi } from "vitest";
import { animationEditorStore, animationWorkspaceKey, animationWorkspaceView, type AnimationDraftValue } from "../store/animation";
import { fakeClient } from "../transport/test-client";
import type {
  AnimationGraphDebugInfo,
  AnimationGraphDocument,
  AnimationGraphSaveRequest,
  AnimationGraphStateInfo,
} from "../transport/protocol";
import { AnimationGraphEditor } from "./AnimationGraphEditor";
import { animationGraphPorts, cloneAnimationGraph, createLocomotionGraphPreset } from "./animation-graph-model";

// **QUARANTINED, NOT SUPPRESSED — this file runs over vitest's 5 s default on ordinary hardware.**
//
// Measured 2026-09-01 on the dev box, this file alone with one worker and nothing else running:
// **77.4 s of test time across 11 tests — ~7 s each** — against a 5000 ms default that was never
// chosen for it. It is not a flake in the "sometimes" sense: it is reliably over budget here and only
// passes on a faster machine or a warmer transform cache, which is why the same 11 tests were green on
// one checkout and red on another with `AnimationGraphEditor.tsx`, its test and `theme/graph.tsx`
// byte-identical between the two. Raised to 30 s so a slow box reports the truth instead of eleven
// `Unable to find role="button"` errors that read exactly like the panel being broken.
//
// **This number is a machine-speed accommodation, not a licence for the panel to get slower.** Every
// test here mounts React Flow, which does no useful work under jsdom (it renders no nodes at all —
// ADR-135) and costs the mount anyway. **Closing gate:** the ADR-135 third-graph migration
// (`claude/happy-sutherland-39be13`, "the last graph that drew its own node") lands this editor on the
// shared `theme/graph.tsx` framework; re-measure then and take the timeout back down if it fits.
vi.setConfig({ testTimeout: 30_000 });

const workspaceKey = animationWorkspaceKey({ projectId: "test-project", scope: { kind: "sequence", id: "main" } });
const pendingDebug = () => new Promise<AnimationGraphDebugInfo>(() => {});
let queuedResponses: Array<() => void> = [];

function response<T>(value: T) {
  return new Promise<T>((resolve) => queuedResponses.push(() => resolve(value)));
}

async function flushResponses() {
  await act(async () => {
    do {
      const current = queuedResponses;
      queuedResponses = [];
      current.forEach((resolve) => resolve());
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    } while (queuedResponses.length > 0);
  });
}

async function mountEditor(ui: Parameters<typeof render>[0]) {
  render(ui);
  await flushResponses();
  // The loaded document commits a second effect that begins runtime tracing.
  await flushResponses();
}

function graphState(graph: AnimationGraphDocument | null, patch: Partial<AnimationGraphStateInfo> = {}): AnimationGraphStateInfo {
  const revision = graph ? "graph-rev-1" : "graph-rev-0";
  return {
    schemaVersion: 2,
    sequenceId: "main",
    revision,
    graph,
    nodePresentation: graph?.nodes.map((node) => ({ nodeId: node.id, ports: animationGraphPorts(node.kind), readiness: "ready", readinessReason: "Compiled and ready." })) ?? [],
    sources: [
      { id: "main", name: "Main authored sequence", kind: "authored_sequence", logicalAssetId: null, revisionId: "sequence-rev-1", durationTick: 60_000, readiness: "ready", reason: "Authored source is playable.", action: null },
      { id: "blocked-clip", name: "Imported clip", kind: "imported_clip", logicalAssetId: "asset-1", revisionId: "asset-rev-1", durationTick: 60_000, readiness: "blocked", reason: "Clip is decoded only; source-node instancing is unavailable.", action: "Use an authored sequence." },
    ],
    compile: graph
      ? { state: "ready", authoredRevision: revision, compiledRevision: revision, compiledHash: "compiled-1", lastGoodRevision: revision, lastGoodHash: "compiled-1", message: "Graph compiled." }
      : { state: "missing", authoredRevision: revision, compiledRevision: null, compiledHash: null, lastGoodRevision: null, lastGoodHash: null, message: "No graph." },
    diagnostics: [],
    ...patch,
  };
}

afterEach(() => {
  cleanup();
  queuedResponses = [];
  animationEditorStore.getState().reset();
});

async function openDrawer(name: "Palette" | "Inspector") {
  const button = await screen.findByRole("button", { name });
  if (button.getAttribute("aria-expanded") !== "true") fireEvent.click(button);
}

test("locomotion preset remains local until one explicit atomic Apply", async () => {
  const empty = graphState(null);
  const save = vi.fn((_sequenceId: string, request: AnimationGraphSaveRequest) => response({
    ok: true,
    message: "Graph applied.",
    state: graphState(cloneAnimationGraph(request.graph), { revision: "graph-rev-2", compile: { state: "ready", authoredRevision: "graph-rev-2", compiledRevision: "graph-rev-2", compiledHash: "compiled-2", lastGoodRevision: "graph-rev-2", lastGoodHash: "compiled-2", message: "Graph compiled." } }),
  }));
  const client = fakeClient({ animationGraphState: () => response(empty), animationGraphSave: save, animationGraphDebug: pendingDebug });
  await mountEditor(<AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} />);

  fireEvent.click(await screen.findByTestId("animation-graph-locomotion-preset"));
  await openDrawer("Palette");
  expect(screen.getByText("unapplied draft")).toBeTruthy();
  expect(screen.getByText("Preview parameters")).toBeTruthy();
  const blockedSource = screen.getByRole("button", { name: /Imported clip/ }) as HTMLButtonElement;
  expect(blockedSource.disabled).toBe(true);
  expect(blockedSource.title).toContain("decoded only");
  expect(save).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Delete graph" })).toBeNull();

  fireEvent.click(screen.getByTestId("animation-graph-apply"));
  await flushResponses();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  const [sequenceId, request] = save.mock.calls[0];
  expect(sequenceId).toBe("main");
  expect(request).toMatchObject({ schemaVersion: 2, expectedRevision: "graph-rev-0" });
  expect(request.graph.nodes.some((node) => node.kind === "state_machine")).toBe(true);
  expect(request.graph.stateMachines[0].transitions).toHaveLength(2);
  expect(screen.queryByText("unapplied draft")).toBeNull();
  const renderedNodes = Array.from(document.querySelectorAll<HTMLElement>(".react-flow__node"));
  expect(renderedNodes).toHaveLength(request.graph.nodes.length);
  expect(renderedNodes.every((node) => node.style.visibility === "visible")).toBe(true);
});

test("an asynchronously compiling Apply refreshes to ready without a remount", async () => {
  const initial = graphState(null);
  const graph = createLocomotionGraphPreset("main");
  const compiling = graphState(graph, {
    revision: "graph-rev-2",
    compile: {
      state: "compiling",
      authoredRevision: "graph-rev-2",
      compiledRevision: null,
      compiledHash: null,
      lastGoodRevision: null,
      lastGoodHash: null,
      message: "Graph compile queued.",
    },
  });
  const ready = graphState(graph, {
    revision: "graph-rev-2",
    compile: {
      state: "ready",
      authoredRevision: "graph-rev-2",
      compiledRevision: "graph-rev-2",
      compiledHash: "compiled-2",
      lastGoodRevision: "graph-rev-2",
      lastGoodHash: "compiled-2",
      message: "Graph compiled.",
    },
  });
  const stateCall = vi.fn()
    .mockImplementationOnce(() => response(initial))
    .mockResolvedValue(ready);
  const save = vi.fn().mockResolvedValue({ ok: true, message: "Graph compile queued.", state: compiling });
  const debug = vi.fn(() => pendingDebug());
  const client = fakeClient({
    animationGraphState: stateCall,
    animationGraphSave: save,
    animationGraphDebug: debug,
  });
  await mountEditor(<AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} />);

  fireEvent.click(await screen.findByTestId("animation-graph-locomotion-preset"));
  vi.useFakeTimers();
  try {
    await act(async () => {
      fireEvent.click(screen.getByTestId("animation-graph-apply"));
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.getByText("compiling")).toBeTruthy();
    expect(stateCall).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(debug).not.toHaveBeenCalled();

    await act(async () => {
      await vi.advanceTimersByTimeAsync(200);
    });
    expect(screen.getByText("ready")).toBeTruthy();
    expect(screen.getByText("Graph compiled.")).toBeTruthy();
    expect(stateCall).toHaveBeenCalledTimes(2);
    expect(save).toHaveBeenCalledTimes(1);
    expect(debug).toHaveBeenCalledWith(graph.id, null, []);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    expect(stateCall).toHaveBeenCalledTimes(2);
  } finally {
    vi.useRealTimers();
  }
}, 15_000);

test("a rejected save preserves the edited draft and exposes addressable diagnostics", async () => {
  const authoritative = createLocomotionGraphPreset("main");
  const initial = graphState(authoritative);
  const rejected = graphState(authoritative, {
    compile: { ...initial.compile, state: "invalid", message: "Draft invalid; previewing the last good revision." },
    diagnostics: [{ id: "native-cycle", severity: "error", code: "pose_cycle", message: "Pose dataflow contains a cycle.", fix: "Remove the returning edge.", nodeId: "node-idle", edgeId: null, portId: "pose" }],
  });
  const save = vi.fn(() => response({ ok: false, message: "Draft rejected; last good preview retained.", state: rejected }));
  const client = fakeClient({ animationGraphState: () => response(initial), animationGraphSave: save, animationGraphDebug: pendingDebug });
  await mountEditor(<AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} />);

  await openDrawer("Palette");
  await openDrawer("Inspector");
  await screen.findAllByText("Locomotion states");
  fireEvent.click(screen.getByText("Keyboard/list view"));
  fireEvent.click(screen.getByRole("button", { name: "Idle · Sequence" }));
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Idle draft" } });
  fireEvent.click(screen.getByTestId("animation-graph-apply"));
  await flushResponses();

  expect((await screen.findAllByText("Draft rejected; last good preview retained.")).length).toBe(2);
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Idle draft");
  expect(screen.getByText("unapplied draft")).toBeTruthy();
  expect(screen.getByText("Pose dataflow contains a cycle.")).toBeTruthy();
  expect(save).toHaveBeenCalledTimes(1);
});

test("matching bounded runtime traces expose active weights, state, cost, and truncation", async () => {
  const graph = createLocomotionGraphPreset("main");
  const initial = graphState(graph);
  const snapshot: AnimationGraphDebugInfo = {
    graphId: graph.id,
    graphRevision: "graph-rev-1",
    compiledHash: "compiled-1",
    instanceId: "hero-runtime",
    rawTick: 12_000,
    localTick: 12_000,
    activeNodes: [{ nodeId: "node-locomotion-machine", weight: 0.75, localTick: 12_000, stateId: "state-moving", costMicros: 31 }],
    activeEdges: [{ edgeId: "edge-machine-output", weight: 0.75 }],
    transition: { transitionId: "transition-start-moving", fromStateId: "state-idle", toStateId: "state-moving", elapsedTick: 4_500, durationTick: 9_000, progress: 0.5 },
    parameterValues: { "parameter-speed": 0.7 },
    watches: [{ id: "parameter-speed", value: 0.7, source: "runtime parameter" }],
    eventsTruncated: true,
    evaluationCostMicros: 44,
    truncated: false,
  };
  const debug = vi.fn().mockImplementationOnce(() => response(snapshot)).mockImplementation(pendingDebug);
  const client = fakeClient({ animationGraphState: () => response(initial), animationGraphDebug: debug });
  await mountEditor(<AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} />);

  expect((await screen.findByTestId("animation-graph-runtime-trace")).textContent).toContain("1 active");
  expect(screen.getByTestId("animation-graph-runtime-trace").textContent).toContain("transition 50%");
  expect(screen.getByTestId("animation-graph-runtime-trace").textContent).toContain("trace incomplete or safety-limited");
  expect(await screen.findByRole("button", { name: "Costs" })).toBeTruthy();
  expect(await screen.findByText("weight 0.750")).toBeTruthy();
  expect(debug).toHaveBeenCalledWith(graph.id, null, []);
});

test("graph keyboard ownership and transient parameters never trigger authored save", async () => {
  const graph = createLocomotionGraphPreset("main");
  const parentKey = vi.fn();
  const preview = vi.fn((_graphId, values) => response({ ok: true, message: "Preview updated.", accepted: values }));
  const save = vi.fn();
  const client = fakeClient({ animationGraphState: () => response(graphState(graph)), animationGraphSetPreviewParameters: preview, animationGraphSave: save, animationGraphDebug: pendingDebug });
  await mountEditor(<div onKeyDown={parentKey}><AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} /></div>);

  const editor = await screen.findByTestId("animation-graph-editor");
  await openDrawer("Palette");
  fireEvent.keyDown(editor, { key: " " });
  expect(parentKey).not.toHaveBeenCalled();
  fireEvent.keyDown(editor, { key: "/" });
  expect(screen.getByPlaceholderText("Press / to focus")).toBe(document.activeElement);

  fireEvent.click(screen.getByLabelText("Moving"));
  await flushResponses();
  await waitFor(() => expect(preview).toHaveBeenCalledWith(graph.id, { "parameter-moving": true }));
  expect(save).not.toHaveBeenCalled();

  fireEvent.click(screen.getByText("Keyboard/list view"));
  const idle = screen.getByRole("button", { name: "Idle · Sequence" });
  fireEvent.click(idle);
  fireEvent.keyDown(idle, { key: "ArrowRight", shiftKey: true });
  expect(parentKey).not.toHaveBeenCalled();
  expect(screen.getByText("unapplied draft")).toBeTruthy();
});

test("switching a layer to an edge constant atomically clears its node weight parameter", async () => {
  const graph = createLocomotionGraphPreset("main");
  const layer = graph.nodes.find((node) => node.id === "node-locomotion-blend")!;
  layer.kind = "layer_additive";
  layer.parameterIds = ["parameter-speed"];
  layer.samples = [];
  layer.maskBindings = ["**/Transform/rotation"];
  graph.edges.find((edge) => edge.id === "edge-walk-blend")!.toPortId = "base";
  graph.edges.find((edge) => edge.id === "edge-run-blend")!.toPortId = "layer";
  animationEditorStore.getState().setGraphSelection(workspaceKey, [], ["edge-run-blend"]);
  await mountEditor(<AnimationGraphEditor client={fakeClient({ animationGraphState: () => response(graphState(graph)), animationGraphDebug: pendingDebug })} sequenceId="main" workspaceKey={workspaceKey} />);

  await openDrawer("Inspector");
  fireEvent.click(await screen.findByRole("button", { name: "Use edge constant instead" }));
  const persistedGraph = () => (animationWorkspaceView(animationEditorStore.getState(), workspaceKey).graph.drafts["graph-draft:main"] as unknown as { graph: AnimationGraphDocument }).graph;
  await waitFor(() => expect(persistedGraph().nodes.find((node) => node.id === layer.id)?.parameterIds).toEqual([]));
  expect(persistedGraph().edges.find((edge) => edge.id === "edge-run-blend")?.weight).toBe(1);
});

test("stale drafts stay locked until a safe explicit rebase or discard", async () => {
  const base = createLocomotionGraphPreset("main");
  const initial = graphState(base);
  const remote = cloneAnimationGraph(base);
  remote.parameters.find((parameter) => parameter.id === "parameter-speed")!.max = 2;
  const advanced = graphState(remote, { revision: "graph-rev-2", compile: { ...initial.compile, authoredRevision: "graph-rev-2", compiledRevision: "graph-rev-2" } });
  const stateCall = vi.fn()
    .mockImplementationOnce(() => response(initial))
    .mockImplementationOnce(() => response(advanced));
  const save = vi.fn(() => response({ ok: false, message: "animation graph changed while this edit was open", state: advanced }));
  const client = fakeClient({ animationGraphState: stateCall, animationGraphSave: save, animationGraphDebug: pendingDebug });
  await mountEditor(<AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} />);

  await openDrawer("Palette");
  await openDrawer("Inspector");
  fireEvent.click(await screen.findByText("Keyboard/list view"));
  fireEvent.click(screen.getByRole("button", { name: "Walk · Sequence" }));
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Walk local" } });
  fireEvent.click(screen.getByTestId("animation-graph-apply"));
  await flushResponses();
  expect(await screen.findByTestId("animation-graph-conflict")).toBeTruthy();
  expect((screen.getByTestId("animation-graph-apply") as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByTestId("animation-graph-apply"));
  expect(save).toHaveBeenCalledTimes(1);

  fireEvent.click(screen.getByRole("button", { name: "Rebase local changes" }));
  await flushResponses();
  await waitFor(() => expect(screen.queryByTestId("animation-graph-conflict")).toBeNull());
  expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("Walk local");
  expect((screen.getByTestId("animation-graph-apply") as HTMLButtonElement).disabled).toBe(false);
});

test("restored draft with no local difference fast-forwards its base revision", async () => {
  const graph = createLocomotionGraphPreset("main");
  animationEditorStore.getState().setGraphDraft(workspaceKey, "graph-draft:main", {
    kind: "animation-graph-draft",
    graph,
    baseRevision: "graph-rev-1",
    baseGraph: graph,
    conflictRevision: "graph-rev-stale",
  } as unknown as AnimationDraftValue);
  const advanced = graphState(graph, { revision: "graph-rev-2", compile: { ...graphState(graph).compile, authoredRevision: "graph-rev-2", compiledRevision: "graph-rev-2" } });
  const save = vi.fn((_sequenceId: string, _request: AnimationGraphSaveRequest) => response({ ok: true, message: "saved", state: advanced }));
  await mountEditor(<AnimationGraphEditor client={fakeClient({ animationGraphState: () => response(advanced), animationGraphSave: save, animationGraphDebug: pendingDebug })} sequenceId="main" workspaceKey={workspaceKey} />);
  await openDrawer("Palette");
  await openDrawer("Inspector");
  fireEvent.click(await screen.findByText("Keyboard/list view"));
  fireEvent.click(screen.getByRole("button", { name: "Walk · Sequence" }));
  fireEvent.change(screen.getByLabelText("Name"), { target: { value: "Walk edit" } });
  fireEvent.click(screen.getByTestId("animation-graph-apply"));
  await flushResponses();
  await waitFor(() => expect(save).toHaveBeenCalledTimes(1));
  expect(save.mock.calls[0][1].expectedRevision).toBe("graph-rev-2");
});

test("trigger preview is a momentary button and authored true defaults are not latched", async () => {
  const graph = createLocomotionGraphPreset("main");
  graph.parameters.push({ id: "parameter-jump", name: "Jump", kind: "trigger", defaultValue: false, min: null, max: null });
  const preview = vi.fn((_graphId, values) => response({ ok: true, message: "Preview updated.", accepted: values }));
  const client = fakeClient({ animationGraphState: () => response(graphState(graph)), animationGraphSetPreviewParameters: preview, animationGraphDebug: pendingDebug });
  await mountEditor(<AnimationGraphEditor client={client} sequenceId="main" workspaceKey={workspaceKey} />);

  const fire = await screen.findByRole("button", { name: "Fire Jump trigger" });
  expect(screen.queryByRole("checkbox", { name: "Jump" })).toBeNull();
  fireEvent.click(fire);
  await flushResponses();
  await waitFor(() => expect(preview).toHaveBeenCalledWith(graph.id, { "parameter-jump": true }));
  expect(screen.getByRole("button", { name: "Fire Jump trigger" })).toBeTruthy();
});

test("compact palette and inspector drawers are explicitly toggleable while the canvas remains mounted", async () => {
  const graph = createLocomotionGraphPreset("main");
  const localDraft = cloneAnimationGraph(graph);
  const parentWheel = vi.fn();
  localDraft.name = "Local locomotion draft";
  animationEditorStore.getState().setGraphDraft(workspaceKey, "graph-draft:main", {
    kind: "animation-graph-draft",
    graph: localDraft,
    baseRevision: "graph-rev-1",
    baseGraph: graph,
    conflictRevision: null,
  } as unknown as AnimationDraftValue);
  await mountEditor(<div onWheel={parentWheel}><AnimationGraphEditor client={fakeClient({ animationGraphState: () => response(graphState(graph)), animationGraphDebug: pendingDebug })} sequenceId="main" workspaceKey={workspaceKey} /></div>);
  const palette = await screen.findByRole("button", { name: "Palette" });
  const inspector = screen.getByRole("button", { name: "Inspector" });
  const actions = screen.getByTestId("animation-graph-toolbar-actions");
  expect(Array.from(actions.querySelectorAll("button"), (button) => button.getAttribute("aria-label") ?? button.textContent)).toEqual([
    "Palette",
    "Inspector",
    "Discard",
    "Apply",
    "Delete graph",
  ]);
  expect(screen.queryByRole("button", { name: "Costs" })).toBeNull();
  expect(palette.getAttribute("aria-expanded")).toBe("false");
  expect(inspector.getAttribute("aria-expanded")).toBe("false");
  expect(screen.getByLabelText("Animation graph canvas")).toBeTruthy();
  fireEvent.keyDown(screen.getByTestId("animation-graph-editor"), { key: "/" });
  await waitFor(() => expect(screen.getByPlaceholderText("Press / to focus")).toBe(document.activeElement));
  expect(palette.getAttribute("aria-expanded")).toBe("true");
  fireEvent.click(palette);
  fireEvent.click(palette);
  fireEvent.click(inspector);
  expect(palette.getAttribute("aria-expanded")).toBe("true");
  expect(inspector.getAttribute("aria-expanded")).toBe("true");
  const paletteRegion = screen.getByLabelText("Animation graph node palette");
  expect(paletteRegion).toBeTruthy();
  fireEvent.wheel(paletteRegion, { deltaY: 800 });
  expect(parentWheel).not.toHaveBeenCalled();
  expect(screen.getByLabelText("Animation graph inspector")).toBeTruthy();
  fireEvent.click(palette);
  fireEvent.click(inspector);
  expect(screen.queryByLabelText("Animation graph node palette")).toBeNull();
  expect(screen.queryByLabelText("Animation graph inspector")).toBeNull();
  expect(screen.getByLabelText("Animation graph canvas")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  await waitFor(() => expect(screen.queryByTestId("animation-graph-apply")).toBeNull());
  expect(screen.queryByRole("button", { name: "Discard" })).toBeNull();
  expect(screen.getByRole("button", { name: "Delete graph" })).toBeTruthy();
});

test("keyboard list creates and deletes connections through explicit port controls", async () => {
  const graph = createLocomotionGraphPreset("main");
  await mountEditor(<AnimationGraphEditor client={fakeClient({ animationGraphState: () => response(graphState(graph)), animationGraphDebug: pendingDebug })} sequenceId="main" workspaceKey={workspaceKey} />);
  await openDrawer("Palette");
  fireEvent.click(await screen.findByText("Keyboard/list view"));
  fireEvent.change(screen.getByLabelText("Connection source node"), { target: { value: "node-idle" } });
  fireEvent.change(screen.getByLabelText("Connection target node"), { target: { value: "node-locomotion-blend" } });
  fireEvent.click(screen.getByRole("button", { name: "Add connection" }));
  expect(screen.getAllByLabelText(/Delete connection/)).toHaveLength(graph.edges.length + 1);
  fireEvent.click(screen.getAllByLabelText(/Delete connection/).at(-1)!);
  expect(screen.getAllByLabelText(/Delete connection/)).toHaveLength(graph.edges.length);
});
