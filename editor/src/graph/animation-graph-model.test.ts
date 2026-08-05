import { expect, test } from "vitest";
import {
  animationGraphPorts,
  animationGraphPreflight,
  animationGraphCompatibleParameterKinds,
  animationGraphEdgeWeightSupport,
  animationGraphMaskSelectorError,
  canonicalizeAnimationGraphDocument,
  createEmptyAnimationGraph,
  createLocomotionGraphPreset,
  rebaseAnimationGraphDraft,
  synchronizeStateMachineFacadeEdges,
} from "./animation-graph-model";

test("locomotion preset is an editable, cardinality-consistent schema-v2 graph", () => {
  const graph = createLocomotionGraphPreset("main");
  expect(graph).toMatchObject({ schemaVersion: 2, sequenceId: "main", outputNodeId: "node-output" });
  expect(new Set(graph.nodes.map((node) => node.id)).size).toBe(graph.nodes.length);
  expect(new Set(graph.edges.map((edge) => edge.id)).size).toBe(graph.edges.length);
  expect(graph.parameters.map((parameter) => [parameter.name, parameter.kind])).toEqual([["Speed", "float"], ["Moving", "boolean"]]);

  const blend = graph.nodes.find((node) => node.kind === "blend_1d");
  const blendInputs = graph.edges.filter((edge) => edge.toNodeId === blend?.id && edge.toPortId === "poses");
  expect(blend?.thresholds).toEqual([]);
  expect(blend?.samples).toEqual([
    expect.objectContaining({ id: "sample-walk", edgeId: "edge-walk-blend", position: [0, 0] }),
    expect.objectContaining({ id: "sample-run", edgeId: "edge-run-blend", position: [1, 0] }),
  ]);
  expect(blend?.samples).toHaveLength(blendInputs.length);
  expect(blend?.samples.find((sample) => sample.position[0] === 0)?.edgeId).toBe("edge-walk-blend");
  expect(graph.nodes.every((node) => Array.isArray(node.triangles))).toBe(true);
  expect(graph.stateMachines[0].transitions).toHaveLength(2);
  expect(animationGraphPreflight(graph).filter((item) => item.severity === "error")).toEqual([]);
});

test("empty graph explains its missing input and ports remain derived presentation", () => {
  const graph = createEmptyAnimationGraph("ui-sequence");
  expect(Object.hasOwn(graph.nodes[0], "ports")).toBe(false);
  expect(Object.hasOwn(graph.nodes[0], "readiness")).toBe(false);
  expect(animationGraphPorts("output")).toEqual(expect.arrayContaining([expect.objectContaining({ id: "pose", direction: "input", kind: "pose" })]));
  expect(animationGraphPreflight(graph)).toEqual(expect.arrayContaining([
    expect.objectContaining({ code: "output_input_count", nodeId: graph.outputNodeId, portId: "pose" }),
  ]));
});

test("preflight addresses dangling edges, missing parameters, and broad layer masks", () => {
  const graph = createLocomotionGraphPreset("main");
  graph.edges.push({ id: "edge-dangling", fromNodeId: "missing", fromPortId: "pose", toNodeId: graph.outputNodeId, toPortId: "pose", enabled: true, weight: null, weightParameterId: null });
  graph.nodes.find((node) => node.kind === "blend_1d")!.parameterIds = ["missing-parameter"];
  const layer = { ...graph.nodes[0], id: "layer", kind: "layer_additive" as const, name: "Upper body", maskBindings: [] };
  graph.nodes.push(layer);
  expect(animationGraphPreflight(graph).map((item) => item.code)).toEqual(expect.arrayContaining(["output_input_count", "dangling_edge", "missing_parameter", "empty_mask"]));
});

test("legacy positional blend fields migrate once into stable edge and sample identities", () => {
  const legacy = createLocomotionGraphPreset("main");
  (legacy as unknown as { schemaVersion: number }).schemaVersion = 1;
  const blend = legacy.nodes.find((node) => node.kind === "blend_1d")!;
  blend.samples = [];
  blend.thresholds = [0, 1];
  // Lexical order is run, walk. The migration is deterministic; new presets avoid this ambiguity.
  const migrated = canonicalizeAnimationGraphDocument(legacy);
  const migratedBlend = migrated.nodes.find((node) => node.id === blend.id)!;
  expect(migratedBlend.thresholds).toEqual([]);
  expect(migratedBlend.samples.map((sample) => [sample.edgeId, sample.position[0]])).toEqual([
    ["edge-run-blend", 0],
    ["edge-walk-blend", 1],
  ]);
  expect(migrated.schemaVersion).toBe(2);
});

test("schema v2 never guesses at positional data and unknown peers fail closed", () => {
  const falselyCurrent = createLocomotionGraphPreset("main");
  const blend = falselyCurrent.nodes.find((node) => node.kind === "blend_1d")!;
  blend.samples = [];
  blend.thresholds = [0, 1];
  const canonical = canonicalizeAnimationGraphDocument(falselyCurrent);
  expect(canonical.nodes.find((node) => node.id === blend.id)?.samples).toEqual([]);
  expect(animationGraphPreflight(canonical)).toEqual(expect.arrayContaining([
    expect.objectContaining({ code: "legacy_positional_contract_in_v2" }),
  ]));

  expect(() => canonicalizeAnimationGraphDocument({ ...falselyCurrent, schemaVersion: 3 })).toThrow(/unsupported animation graph schema 3/i);
});

test("mask grammar and state-machine façade synchronization are explicit", () => {
  expect(animationGraphMaskSelectorError("**/Transform/rotation")).toBeNull();
  expect(animationGraphMaskSelectorError("Skeleton.LeftArm.*")).toContain("target/component/property");
  const graph = createLocomotionGraphPreset("main");
  graph.edges = graph.edges.filter((edge) => edge.id !== "edge-blend-machine");
  const synchronized = synchronizeStateMachineFacadeEdges(graph);
  expect(synchronized.edges.some((edge) => edge.toNodeId === "node-locomotion-machine" && edge.fromNodeId === "node-locomotion-blend")).toBe(true);
});

test("edge weight contracts match only the adapter paths that consume them", () => {
  const graph = createLocomotionGraphPreset("main");
  const blend = graph.nodes.find((node) => node.kind === "blend_1d")!;
  const blendEdge = graph.edges.find((edge) => edge.toNodeId === blend.id)!;
  blendEdge.weight = 0.5;
  expect(animationGraphEdgeWeightSupport(graph, blendEdge)).toMatchObject({ explicit: false, parameter: false });
  expect(animationGraphPreflight(graph)).toEqual(expect.arrayContaining([
    expect.objectContaining({ code: "unsupported_edge_weight_contract", edgeId: blendEdge.id }),
  ]));

  const normalized = { ...blend, id: "normalized", kind: "blend_normalized" as const, name: "Weighted blend", parameterIds: [], samples: [] };
  graph.nodes.push(normalized);
  const weighted = { ...blendEdge, id: "weighted", toNodeId: normalized.id, weight: 0.25, weightParameterId: null };
  graph.edges.push(weighted);
  expect(animationGraphEdgeWeightSupport(graph, weighted)).toMatchObject({ explicit: true, parameter: true });
  expect(animationGraphPreflight(graph).filter((item) => item.edgeId === weighted.id && item.code === "unsupported_edge_weight_contract")).toEqual([]);

  const layer = { ...blend, id: "layer", kind: "layer_additive" as const, name: "Layer", parameterIds: ["parameter-speed"], samples: [], maskBindings: ["**/Transform/rotation"] };
  graph.nodes.push(layer);
  const layerEdge = { ...blendEdge, id: "layer-weight", toNodeId: layer.id, toPortId: "layer", weight: 0.4, weightParameterId: null };
  graph.edges.push(layerEdge);
  expect(animationGraphEdgeWeightSupport(graph, layerEdge)).toMatchObject({ explicit: false, parameter: false });
  expect(animationGraphPreflight(graph)).toEqual(expect.arrayContaining([
    expect.objectContaining({ code: "unsupported_edge_weight_contract", edgeId: layerEdge.id }),
  ]));
  expect(animationGraphCompatibleParameterKinds("blend_1d")).toEqual(["float"]);
  expect(animationGraphCompatibleParameterKinds("layer_additive")).toEqual(["float"]);
  expect(animationGraphCompatibleParameterKinds("blend_2d_cartesian")).toEqual(["vec2"]);
});

test("duplicate enabled state façade edges are rejected even when source sets match", () => {
  const graph = createLocomotionGraphPreset("main");
  const edge = graph.edges.find((candidate) => candidate.id === "edge-idle-machine")!;
  graph.edges.push({ ...edge, id: "edge-idle-machine-duplicate" });
  expect(animationGraphPreflight(graph)).toEqual(expect.arrayContaining([
    expect.objectContaining({ code: "duplicate_state_pose_edge", edgeId: "edge-idle-machine-duplicate" }),
  ]));
});

test("three-way rebase merges disjoint stable records and refuses same-record collisions", () => {
  const base = createLocomotionGraphPreset("main");
  const local = structuredClone(base);
  const remote = structuredClone(base);
  local.nodes.find((node) => node.id === "node-walk")!.name = "Walk local";
  remote.parameters.find((parameter) => parameter.id === "parameter-speed")!.max = 2;
  const merged = rebaseAnimationGraphDraft(base, local, remote);
  expect(merged.ok).toBe(true);
  if (merged.ok) {
    expect(merged.graph.nodes.find((node) => node.id === "node-walk")?.name).toBe("Walk local");
    expect(merged.graph.parameters.find((parameter) => parameter.id === "parameter-speed")?.max).toBe(2);
  }
  remote.nodes.find((node) => node.id === "node-walk")!.name = "Walk remote";
  expect(rebaseAnimationGraphDraft(base, local, remote)).toEqual(expect.objectContaining({ ok: false, conflicts: expect.arrayContaining(["node node-walk"]) }));
});

test("only unconditional zero-duration transition cycles are rejected", () => {
  const graph = createLocomotionGraphPreset("main");
  for (const transition of graph.stateMachines[0].transitions) {
    transition.conditions = [];
    transition.durationTick = 1;
  }
  expect(animationGraphPreflight(graph).map((item) => item.code)).not.toContain("unconditional_zero_duration_cycle");
  for (const transition of graph.stateMachines[0].transitions) transition.durationTick = 0;
  expect(animationGraphPreflight(graph)).toEqual(expect.arrayContaining([expect.objectContaining({ code: "unconditional_zero_duration_cycle" })]));
});
