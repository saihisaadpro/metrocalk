//! Pure animation-graph authoring helpers. React Flow consumes a projection of this document; it never
//! owns graph state. Keeping preset construction and client-side preflight here makes them deterministic,
//! testable, and reusable by both the canvas and its accessible list alternative.

import {
  ANIMATION_GRAPH_LEGACY_SCHEMA_VERSION,
  ANIMATION_GRAPH_SCHEMA_VERSION,
  type AnimationGraphCondition,
  type AnimationGraphDiagnostic,
  type AnimationGraphDocument,
  type AnimationGraphEdge,
  type AnimationGraphNode,
  type AnimationGraphNodeKind,
  type AnimationGraphParameter,
  type AnimationGraphPortInfo,
  type AnimationGraphStateMachine,
  type AnimationGraphValue,
} from "../transport/protocol";

let localIdentity = 0;

export function animationGraphLocalId(prefix: string): string {
  localIdentity += 1;
  const random = globalThis.crypto?.randomUUID?.();
  return `${prefix}-${random ?? localIdentity.toString(36)}`;
}

export function cloneAnimationGraph(graph: AnimationGraphDocument): AnimationGraphDocument {
  return JSON.parse(JSON.stringify(graph)) as AnimationGraphDocument;
}

const POSE_INPUT = (id = "pose", label = "Pose", multiple = false): AnimationGraphPortInfo => ({
  id, label, direction: "input", kind: "pose", required: true, multiple,
});
const POSE_OUTPUT: AnimationGraphPortInfo = {
  id: "pose", label: "Pose", direction: "output", kind: "pose", required: false, multiple: true,
};

export function animationGraphPorts(kind: AnimationGraphNodeKind): AnimationGraphPortInfo[] {
  switch (kind) {
    case "reference_pose":
    case "sequence":
      return [{ ...POSE_OUTPUT }];
    case "blend_normalized":
    case "blend_direct":
    case "blend_1d":
    case "blend_2d_cartesian":
      return [POSE_INPUT("poses", "Pose inputs", true), { ...POSE_OUTPUT }];
    case "layer_override":
    case "layer_additive":
      return [POSE_INPUT("base", "Base pose"), POSE_INPUT("layer", "Layer pose"), { ...POSE_OUTPUT }];
    case "state_machine":
      return [POSE_INPUT("states", "State poses", true), { ...POSE_OUTPUT }];
    case "output":
      return [{ ...POSE_INPUT("pose", "Final pose") }, {
        id: "output", label: "Output", direction: "output", kind: "output", required: false, multiple: false,
      }];
  }
}

const NODE_LABELS: Record<AnimationGraphNodeKind, string> = {
  reference_pose: "Reference pose",
  sequence: "Sequence",
  blend_normalized: "Normalized blend",
  blend_direct: "Direct blend",
  blend_1d: "Blend 1D",
  blend_2d_cartesian: "Cartesian Blend 2D",
  layer_override: "Override layer + mask",
  layer_additive: "Additive layer + mask",
  state_machine: "State machine",
  output: "Output",
};

export function createAnimationGraphNode(
  kind: AnimationGraphNodeKind,
  position: { x: number; y: number },
  overrides: Partial<AnimationGraphNode> = {},
): AnimationGraphNode {
  return {
    id: animationGraphLocalId("node"),
    kind,
    name: NODE_LABELS[kind],
    position,
    enabled: true,
    sourceId: null,
    parameterIds: [],
    samples: [],
    thresholds: [],
    triangles: [],
    maskBindings: [],
    stateMachineId: null,
    ...overrides,
  };
}

function incomingPoseEdges(graph: AnimationGraphDocument, nodeId: string) {
  return graph.edges
    .filter((edge) => edge.enabled && edge.toNodeId === nodeId && edge.toPortId === "poses")
    .sort((left, right) => left.id.localeCompare(right.id));
}

export function animationGraphReadableSchemaVersion(value: unknown): 1 | 2 | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const version = (value as { schemaVersion?: unknown }).schemaVersion;
  return version === ANIMATION_GRAPH_LEGACY_SCHEMA_VERSION || version === ANIMATION_GRAPH_SCHEMA_VERSION
    ? version
    : null;
}

/** Upgrade only schema v1 positional compatibility fields into the schema v2 stable-ID contract.
 * The mapping is deliberately deterministic so old peers converge even though their lost positional
 * association cannot be recovered. Canonical v2 input is cloned without migration; unknown versions
 * fail closed instead of being guessed at. */
export function canonicalizeAnimationGraphDocument(document: unknown): AnimationGraphDocument {
  const version = animationGraphReadableSchemaVersion(document);
  if (version === null) {
    const received = document && typeof document === "object" && !Array.isArray(document)
      ? String((document as { schemaVersion?: unknown }).schemaVersion ?? "missing")
      : "missing";
    throw new Error(`Unsupported animation graph schema ${received}; this editor reads schema 1 or ${ANIMATION_GRAPH_SCHEMA_VERSION}.`);
  }
  const graph = JSON.parse(JSON.stringify(document)) as AnimationGraphDocument;
  if (!Array.isArray(graph.nodes) || !Array.isArray(graph.edges) || !Array.isArray(graph.parameters) || !Array.isArray(graph.stateMachines)) {
    throw new Error("The animation graph document is malformed; expected node, edge, parameter, and state-machine arrays.");
  }
  if (version === ANIMATION_GRAPH_SCHEMA_VERSION) return graph;

  graph.edges = graph.edges.map((edge) => ({ ...edge, weightParameterId: edge.weightParameterId ?? null }));
  graph.nodes = graph.nodes.map((node) => ({ ...node, samples: node.samples ?? [], thresholds: node.thresholds ?? [] }));
  for (const node of graph.nodes) {
    const incoming = incomingPoseEdges(graph, node.id);
    if ((node.kind === "blend_normalized" || node.kind === "blend_direct")
      && node.parameterIds.length === incoming.length
      && incoming.length > 0) {
      const positional = new Map(incoming.map((edge, index) => [edge.id, node.parameterIds[index]]));
      graph.edges = graph.edges.map((edge) => edge.toNodeId === node.id && edge.toPortId === "poses" && !edge.weightParameterId
        ? { ...edge, weightParameterId: positional.get(edge.id) ?? null }
        : edge);
      node.parameterIds = [];
    }
    if (node.kind !== "blend_1d" && node.kind !== "blend_2d_cartesian") continue;
    if (node.samples.length === 0 && node.thresholds.length > 0) {
      node.samples = incoming.flatMap((edge, index) => {
        const x = node.thresholds[node.kind === "blend_1d" ? index : index * 2];
        const y = node.kind === "blend_1d" ? 0 : node.thresholds[index * 2 + 1];
        return Number.isFinite(x) && Number.isFinite(y)
          ? [{ id: `sample-${edge.id}`, edgeId: edge.id, position: [x, y] as const }]
          : [];
      });
      if (node.kind === "blend_2d_cartesian") {
        const samplesBySource = new Map<string, string[]>();
        for (const sample of node.samples) {
          const source = graph.edges.find((edge) => edge.id === sample.edgeId)?.fromNodeId;
          if (source) samplesBySource.set(source, [...(samplesBySource.get(source) ?? []), sample.id]);
        }
        const sampleIds = new Set(node.samples.map((sample) => sample.id));
        node.triangles = node.triangles.map((triangle) => triangle.map((legacyId) => {
          if (sampleIds.has(legacyId)) return legacyId;
          const byEdge = node.samples.find((sample) => sample.edgeId === legacyId)?.id;
          if (byEdge) return byEdge;
          const bySource = samplesBySource.get(legacyId);
          return bySource?.length === 1 ? bySource[0] : legacyId;
        }) as [string, string, string]);
      }
    }
    if (node.samples.length > 0) node.thresholds = [];
  }
  graph.schemaVersion = ANIMATION_GRAPH_SCHEMA_VERSION;
  return graph;
}

export interface AnimationGraphEdgeWeightSupport {
  explicit: boolean;
  parameter: boolean;
  reason: string;
}

const NO_EDGE_WEIGHT: AnimationGraphEdgeWeightSupport = {
  explicit: false,
  parameter: false,
  reason: "This input derives its contribution from state, geometry, or destination policy.",
};

/** The single source of truth for authored edge weight fields. It mirrors the native adapter exactly. */
export function animationGraphEdgeWeightSupport(
  document: Pick<AnimationGraphDocument, "nodes">,
  edge: Pick<AnimationGraphEdge, "toNodeId" | "toPortId">,
): AnimationGraphEdgeWeightSupport {
  const target = document.nodes.find((node) => node.id === edge.toNodeId);
  if ((target?.kind === "blend_normalized" || target?.kind === "blend_direct") && edge.toPortId === "poses") {
    return { explicit: true, parameter: true, reason: "This blend consumes either one explicit constant or one stable float parameter binding." };
  }
  if ((target?.kind === "layer_override" || target?.kind === "layer_additive") && edge.toPortId === "layer") {
    if (target.parameterIds.length > 0) {
      return { explicit: false, parameter: false, reason: "This layer already binds its node-level weight parameter, so an edge constant would be ignored." };
    }
    return { explicit: true, parameter: false, reason: "The layer input consumes an explicit constant; edge-bound parameters are not implemented." };
  }
  return NO_EDGE_WEIGHT;
}

export function animationGraphCompatibleParameterKinds(
  kind: AnimationGraphNodeKind,
): AnimationGraphParameter["kind"][] {
  if (kind === "blend_2d_cartesian") return ["vec2"];
  if (kind === "blend_1d" || kind === "layer_override" || kind === "layer_additive") return ["float"];
  return [];
}

/** Keep the visual state-machine façade exactly aligned with semantic state pose roots. */
export function synchronizeStateMachineFacadeEdges(document: AnimationGraphDocument): AnimationGraphDocument {
  const graph = cloneAnimationGraph(document);
  for (const facade of graph.nodes.filter((node) => node.kind === "state_machine" && node.stateMachineId)) {
    const machine = graph.stateMachines.find((candidate) => candidate.id === facade.stateMachineId);
    if (!machine) continue;
    const desiredSources = [...new Set(machine.states.map((state) => state.poseNodeId))];
    const facadeEdges = graph.edges.filter((edge) => edge.toNodeId === facade.id && edge.toPortId === "states");
    const retained = new Set<string>();
    for (const sourceId of desiredSources) {
      const existing = facadeEdges.find((edge) => edge.fromNodeId === sourceId && !retained.has(edge.id));
      if (existing) {
        existing.enabled = true;
        retained.add(existing.id);
      } else {
        const state = machine.states.find((candidate) => candidate.poseNodeId === sourceId)!;
        const id = `edge-state-${facade.id}-${state.id}`;
        graph.edges.push({ id, fromNodeId: sourceId, fromPortId: "pose", toNodeId: facade.id, toPortId: "states", enabled: true, weight: null, weightParameterId: null });
        retained.add(id);
      }
    }
    graph.edges = graph.edges.filter((edge) => edge.toNodeId !== facade.id || edge.toPortId !== "states" || retained.has(edge.id));
  }
  return graph;
}

/** Slash-separated PropertyPath selector grammar. `*` matches one segment and `**` any depth. */
export function animationGraphMaskSelectorError(selector: string): string | null {
  if (!selector.trim()) return "Selector cannot be empty.";
  if (selector !== selector.trim() || selector.includes("\\") || selector.includes("\0")) return "Use a trimmed forward-slash path.";
  const segments = selector.split("/");
  if (segments.length < 3 || segments.some((segment) => !segment)) return "Use at least target/component/property segments with no empty segment.";
  if (segments.some((segment) => segment.includes("*") && segment !== "*" && segment !== "**")) return "Wildcards must occupy a complete segment: * or **.";
  return null;
}

export type AnimationGraphRebaseResult =
  | { ok: true; graph: AnimationGraphDocument }
  | { ok: false; conflicts: string[] };

/** Conservative stable-ID three-way rebase. Concurrent edits to different records merge; conflicting
 * edits to the same record stop for human review rather than silently choosing either peer. */
export function rebaseAnimationGraphDraft(
  base: AnimationGraphDocument | null,
  local: AnimationGraphDocument,
  remote: AnimationGraphDocument | null,
): AnimationGraphRebaseResult {
  if (!base) {
    if (!remote) return { ok: true, graph: canonicalizeAnimationGraphDocument(local) };
    return { ok: false, conflicts: ["graph identity (both peers created a graph)"] };
  }
  if (!remote) return { ok: false, conflicts: ["graph identity (the remote graph was deleted)"] };
  if (base.id !== local.id || base.id !== remote.id) return { ok: false, conflicts: ["graph identity"] };
  const conflicts: string[] = [];
  const equal = (left: unknown, right: unknown) => JSON.stringify(left) === JSON.stringify(right);
  const scalar = <T,>(label: string, baseValue: T, localValue: T, remoteValue: T): T => {
    if (equal(localValue, baseValue)) return remoteValue;
    if (equal(remoteValue, baseValue) || equal(localValue, remoteValue)) return localValue;
    conflicts.push(label);
    return remoteValue;
  };
  const records = <T extends { id: string }>(label: string, baseItems: T[], localItems: T[], remoteItems: T[]): T[] => {
    const baseById = new Map(baseItems.map((item) => [item.id, item]));
    const localById = new Map(localItems.map((item) => [item.id, item]));
    const remoteById = new Map(remoteItems.map((item) => [item.id, item]));
    const merged: T[] = [];
    for (const id of [...new Set([...baseById.keys(), ...localById.keys(), ...remoteById.keys()])].sort()) {
      const baseItem = baseById.get(id);
      const localItem = localById.get(id);
      const remoteItem = remoteById.get(id);
      const localChanged = !equal(localItem, baseItem);
      const remoteChanged = !equal(remoteItem, baseItem);
      if (localChanged && remoteChanged && !equal(localItem, remoteItem)) {
        conflicts.push(`${label} ${id}`);
        if (remoteItem) merged.push(remoteItem);
      } else {
        const chosen = localChanged ? localItem : remoteItem;
        if (chosen) merged.push(chosen);
      }
    }
    return merged;
  };
  const graph: AnimationGraphDocument = {
    schemaVersion: remote.schemaVersion,
    id: remote.id,
    sequenceId: remote.sequenceId,
    name: scalar("graph name", base.name, local.name, remote.name),
    outputNodeId: scalar("output selection", base.outputNodeId, local.outputNodeId, remote.outputNodeId),
    nodes: records("node", base.nodes, local.nodes, remote.nodes),
    edges: records("connection", base.edges, local.edges, remote.edges),
    parameters: records("parameter", base.parameters, local.parameters, remote.parameters),
    stateMachines: records("state machine", base.stateMachines, local.stateMachines, remote.stateMachines),
  };
  return conflicts.length
    ? { ok: false, conflicts }
    : { ok: true, graph: synchronizeStateMachineFacadeEdges(canonicalizeAnimationGraphDocument(graph)) };
}

export function defaultAnimationGraphValue(kind: AnimationGraphParameter["kind"]): AnimationGraphValue {
  if (kind === "boolean" || kind === "trigger") return false;
  if (kind === "vec2") return [0, 0];
  return 0;
}

export function createAnimationGraphParameter(
  name: string,
  kind: AnimationGraphParameter["kind"],
  defaultValue = defaultAnimationGraphValue(kind),
): AnimationGraphParameter {
  return {
    id: animationGraphLocalId("parameter"),
    name,
    kind,
    defaultValue,
    min: kind === "float" || kind === "integer" ? 0 : null,
    max: kind === "float" || kind === "integer" ? 1 : null,
  };
}

export function createEmptyAnimationGraph(sequenceId: string): AnimationGraphDocument {
  const output = createAnimationGraphNode("output", { x: 540, y: 160 }, { id: "output", name: "Final output" });
  return {
    schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
    id: animationGraphLocalId("graph"),
    sequenceId,
    name: "Animation graph",
    outputNodeId: output.id,
    nodes: [output],
    edges: [],
    parameters: [],
    stateMachines: [],
  };
}

/** An editable, schema-native locomotion graph. It deliberately uses authored sequence sources so the
 * preset works before imported-clip instancing is available. Every value stays visible in the inspector. */
export function createLocomotionGraphPreset(sequenceId: string): AnimationGraphDocument {
  const speed: AnimationGraphParameter = {
    id: "parameter-speed",
    name: "Speed",
    kind: "float",
    defaultValue: 0,
    min: 0,
    max: 1,
  };
  const moving: AnimationGraphParameter = {
    id: "parameter-moving",
    name: "Moving",
    kind: "boolean",
    defaultValue: false,
    min: null,
    max: null,
  };
  const reference = createAnimationGraphNode("reference_pose", { x: 20, y: 20 }, { id: "node-reference" });
  const idle = createAnimationGraphNode("sequence", { x: 20, y: 140 }, { id: "node-idle", name: "Idle", sourceId: sequenceId });
  const walk = createAnimationGraphNode("sequence", { x: 20, y: 260 }, { id: "node-walk", name: "Walk", sourceId: sequenceId });
  const run = createAnimationGraphNode("sequence", { x: 20, y: 380 }, { id: "node-run", name: "Run", sourceId: sequenceId });
  const locomotion = createAnimationGraphNode("blend_1d", { x: 270, y: 285 }, {
    id: "node-locomotion-blend",
    name: "Speed blend",
    parameterIds: [speed.id],
    samples: [
      { id: "sample-walk", edgeId: "edge-walk-blend", position: [0, 0] },
      { id: "sample-run", edgeId: "edge-run-blend", position: [1, 0] },
    ],
  });
  const machine = createAnimationGraphNode("state_machine", { x: 520, y: 190 }, {
    id: "node-locomotion-machine",
    name: "Locomotion states",
    stateMachineId: "machine-locomotion",
  });
  const output = createAnimationGraphNode("output", { x: 790, y: 190 }, { id: "node-output", name: "Final output" });
  const condition: AnimationGraphCondition = {
    id: "condition-start-moving",
    parameterId: moving.id,
    operator: "is_true",
    value: null,
  };
  const stateMachine: AnimationGraphStateMachine = {
    id: "machine-locomotion",
    name: "Locomotion",
    entryStateId: "state-idle",
    states: [
      { id: "state-idle", name: "Idle", poseNodeId: idle.id, resetOnEntry: true },
      { id: "state-moving", name: "Moving", poseNodeId: locomotion.id, resetOnEntry: false },
    ],
    transitions: [
      {
        id: "transition-start-moving",
        fromStateId: "state-idle",
        toStateId: "state-moving",
        priority: 10,
        durationTick: 9_000,
        curve: "ease_in_out",
        interruption: "destination",
        conditions: [condition],
        exitTime: null,
      },
      {
        id: "transition-stop-moving",
        fromStateId: "state-moving",
        toStateId: "state-idle",
        priority: 10,
        durationTick: 7_500,
        curve: "ease_out",
        interruption: "source",
        conditions: [{ ...condition, id: "condition-stop-moving", operator: "is_false" }],
        exitTime: null,
      },
    ],
  };
  return {
    schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
    id: "graph-locomotion",
    sequenceId,
    name: "Locomotion",
    outputNodeId: output.id,
    nodes: [reference, idle, walk, run, locomotion, machine, output],
    edges: [
      { id: "edge-walk-blend", fromNodeId: walk.id, fromPortId: "pose", toNodeId: locomotion.id, toPortId: "poses", enabled: true, weight: null, weightParameterId: null },
      { id: "edge-run-blend", fromNodeId: run.id, fromPortId: "pose", toNodeId: locomotion.id, toPortId: "poses", enabled: true, weight: null, weightParameterId: null },
      { id: "edge-idle-machine", fromNodeId: idle.id, fromPortId: "pose", toNodeId: machine.id, toPortId: "states", enabled: true, weight: null, weightParameterId: null },
      { id: "edge-blend-machine", fromNodeId: locomotion.id, fromPortId: "pose", toNodeId: machine.id, toPortId: "states", enabled: true, weight: null, weightParameterId: null },
      { id: "edge-machine-output", fromNodeId: machine.id, fromPortId: "pose", toNodeId: output.id, toPortId: "pose", enabled: true, weight: null, weightParameterId: null },
    ],
    parameters: [speed, moving],
    stateMachines: [stateMachine],
  };
}

/** Fast editor-side feedback only. Native validation remains authoritative at Apply. */
export function animationGraphPreflight(graph: AnimationGraphDocument): AnimationGraphDiagnostic[] {
  const diagnostics: AnimationGraphDiagnostic[] = [];
  const nodes = new Map(graph.nodes.map((node) => [node.id, node]));
  const parameterById = new Map(graph.parameters.map((parameter) => [parameter.id, parameter]));
  const parameters = new Set(parameterById.keys());
  const duplicate = (values: readonly string[]) => values.find((value, index) => values.indexOf(value) !== index);
  if ((graph as { schemaVersion: number }).schemaVersion !== ANIMATION_GRAPH_SCHEMA_VERSION) {
    diagnostics.push({ id: "preflight-schema", severity: "error", code: "unsupported_schema", message: `Graph schema ${(graph as { schemaVersion: number }).schemaVersion} is not canonical schema ${ANIMATION_GRAPH_SCHEMA_VERSION}.`, fix: "Run the explicit v1-to-v2 migration or upgrade the editor.", nodeId: null, edgeId: null, portId: null });
  }
  const duplicateNode = duplicate(graph.nodes.map((node) => node.id));
  const duplicateEdge = duplicate(graph.edges.map((edge) => edge.id));
  const duplicateParameter = duplicate(graph.parameters.map((parameter) => parameter.id));
  if (duplicateNode) diagnostics.push({ id: `preflight-node-id-${duplicateNode}`, severity: "error", code: "duplicate_node_id", message: `Node ID ${duplicateNode} is not unique.`, fix: "Restore stable unique node identities.", nodeId: duplicateNode, edgeId: null, portId: null });
  if (duplicateEdge) diagnostics.push({ id: `preflight-edge-id-${duplicateEdge}`, severity: "error", code: "duplicate_edge_id", message: `Edge ID ${duplicateEdge} is not unique.`, fix: "Restore stable unique edge identities.", nodeId: null, edgeId: duplicateEdge, portId: null });
  if (duplicateParameter) diagnostics.push({ id: `preflight-parameter-id-${duplicateParameter}`, severity: "error", code: "duplicate_parameter_id", message: `Parameter ID ${duplicateParameter} is not unique.`, fix: "Restore stable unique parameter identities.", nodeId: null, edgeId: null, portId: null });
  const normalizedNames = graph.parameters.map((parameter) => parameter.name.trim().toLocaleLowerCase());
  const duplicateName = duplicate(normalizedNames);
  if (duplicateName) diagnostics.push({ id: `preflight-parameter-name-${duplicateName}`, severity: "error", code: "duplicate_parameter_name", message: `Parameter name ${duplicateName} is ambiguous.`, fix: "Give every parameter a unique non-empty name.", nodeId: null, edgeId: null, portId: null });
  const output = nodes.get(graph.outputNodeId);
  const outputNodes = graph.nodes.filter((node) => node.kind === "output");
  if (outputNodes.length !== 1) {
    diagnostics.push({ id: "preflight-output-count", severity: "error", code: "output_node_count", message: `Graph needs exactly one Output node; found ${outputNodes.length}.`, fix: "Keep one final Output node.", nodeId: outputNodes[0]?.id ?? null, edgeId: null, portId: null });
  }
  if (!output || output.kind !== "output") {
    diagnostics.push({ id: "preflight-output", severity: "error", code: "missing_output", message: "Choose exactly one supported Output node.", fix: "Add an Output node or restore the graph output identity.", nodeId: graph.outputNodeId, edgeId: null, portId: null });
  }
  const incomingOutput = graph.edges.filter((edge) => edge.enabled && edge.toNodeId === graph.outputNodeId && edge.toPortId === "pose");
  if (output && incomingOutput.length !== 1) {
    diagnostics.push({ id: "preflight-output-input", severity: "error", code: "output_input_count", message: `Output needs exactly one pose input; found ${incomingOutput.length}.`, fix: "Connect one pose-producing node to Output.", nodeId: output.id, edgeId: null, portId: "pose" });
  }
  for (const edge of graph.edges) {
    if (!nodes.has(edge.fromNodeId) || !nodes.has(edge.toNodeId)) {
      diagnostics.push({ id: `preflight-edge-${edge.id}`, severity: "error", code: "dangling_edge", message: "This connection references a node that no longer exists.", fix: "Remove or reconnect the edge.", nodeId: null, edgeId: edge.id, portId: null });
      continue;
    }
    const weightSupport = animationGraphEdgeWeightSupport(graph, edge);
    const carriesUnsupportedWeight = (!weightSupport.explicit && edge.weight !== null)
      || (!weightSupport.parameter && edge.weightParameterId !== null)
      || (edge.weight !== null && edge.weightParameterId !== null);
    if (carriesUnsupportedWeight) {
      diagnostics.push({ id: `preflight-edge-weight-contract-${edge.id}`, severity: "error", code: "unsupported_edge_weight_contract", message: `This connection persists weight data that its ${nodes.get(edge.toNodeId)?.kind ?? "unknown"}/${edge.toPortId} adapter does not consume.`, fix: weightSupport.explicit || weightSupport.parameter ? "Choose either one supported weight source and clear the other field." : "Clear both edge weight fields; this input has no authored edge-weight contract.", nodeId: edge.toNodeId, edgeId: edge.id, portId: edge.toPortId });
    }
    if (edge.weight !== null && (!Number.isFinite(edge.weight) || edge.weight < 0)) {
      diagnostics.push({ id: `preflight-edge-weight-${edge.id}`, severity: "error", code: "invalid_edge_weight", message: "An explicit connection weight must be finite and non-negative.", fix: "Use a finite value of zero or greater, or clear the explicit weight.", nodeId: edge.toNodeId, edgeId: edge.id, portId: edge.toPortId });
    }
    if (edge.weightParameterId && !parameters.has(edge.weightParameterId)) {
      diagnostics.push({ id: `preflight-edge-parameter-${edge.id}`, severity: "error", code: "missing_edge_weight_parameter", message: "This connection references a missing weight parameter.", fix: "Choose an existing float parameter or clear the binding.", nodeId: edge.toNodeId, edgeId: edge.id, portId: edge.toPortId });
    } else if (edge.weightParameterId && parameterById.get(edge.weightParameterId)?.kind !== "float") {
      diagnostics.push({ id: `preflight-edge-parameter-type-${edge.id}`, severity: "error", code: "edge_weight_parameter_type", message: "A connection weight binding must reference a float parameter.", fix: "Choose a float parameter or clear the connection binding.", nodeId: edge.toNodeId, edgeId: edge.id, portId: edge.toPortId });
    }
    const source = nodes.get(edge.fromNodeId)!;
    const target = nodes.get(edge.toNodeId)!;
    const outputPort = animationGraphPorts(source.kind).find((port) => port.direction === "output" && port.id === edge.fromPortId);
    const inputPort = animationGraphPorts(target.kind).find((port) => port.direction === "input" && port.id === edge.toPortId);
    if (!outputPort || !inputPort || outputPort.kind !== inputPort.kind) {
      diagnostics.push({ id: `preflight-port-${edge.id}`, severity: "error", code: "incompatible_ports", message: "This connection does not join compatible typed ports.", fix: "Reconnect matching pose, number, boolean, trigger, mask, or output ports.", nodeId: target.id, edgeId: edge.id, portId: edge.toPortId });
    } else if (!inputPort.multiple && graph.edges.filter((candidate) => candidate.enabled && candidate.toNodeId === edge.toNodeId && candidate.toPortId === edge.toPortId).length > 1) {
      diagnostics.push({ id: `preflight-port-cardinality-${edge.toNodeId}-${edge.toPortId}`, severity: "error", code: "input_cardinality", message: `${target.name} accepts only one ${inputPort.label} connection.`, fix: "Insert a blend node or remove the extra connection.", nodeId: target.id, edgeId: edge.id, portId: edge.toPortId });
    }
  }
  // Pose dataflow is a DAG. State cycles are represented only by transitions inside a state-machine
  // composite and therefore do not weaken this structural cycle check.
  const adjacency = new Map<string, string[]>();
  for (const edge of graph.edges.filter((edge) => edge.enabled && nodes.has(edge.fromNodeId) && nodes.has(edge.toNodeId))) {
    adjacency.set(edge.fromNodeId, [...(adjacency.get(edge.fromNodeId) ?? []), edge.toNodeId]);
  }
  const visiting = new Set<string>();
  const visited = new Set<string>();
  let cycleNode: string | null = null;
  const visit = (id: string): boolean => {
    if (visiting.has(id)) { cycleNode = id; return true; }
    if (visited.has(id)) return false;
    visiting.add(id);
    for (const next of adjacency.get(id) ?? []) if (visit(next)) return true;
    visiting.delete(id);
    visited.add(id);
    return false;
  };
  for (const id of nodes.keys()) if (visit(id)) break;
  if (cycleNode) diagnostics.push({ id: `preflight-pose-cycle-${cycleNode}`, severity: "error", code: "pose_cycle", message: "Pose dataflow contains a cycle; only state transitions may cycle.", fix: "Remove the returning pose edge.", nodeId: cycleNode, edgeId: null, portId: null });
  for (const node of graph.nodes) {
    if ((node.thresholds?.length ?? 0) > 0
      || ((node.kind === "blend_normalized" || node.kind === "blend_direct") && node.parameterIds.length > 0)) {
      diagnostics.push({ id: `preflight-legacy-positional-${node.id}`, severity: "error", code: "legacy_positional_contract_in_v2", message: `${node.name} contains schema-v1 positional blend data inside a schema-v2 graph.`, fix: "Mark the source as schema v1 and run the explicit migration, or author stable edge/sample bindings.", nodeId: node.id, edgeId: null, portId: "poses" });
    }
    for (const port of animationGraphPorts(node.kind).filter((candidate) => candidate.direction === "input" && candidate.required)) {
      const incoming = graph.edges.filter((edge) => edge.enabled && edge.toNodeId === node.id && edge.toPortId === port.id);
      if (incoming.length === 0) diagnostics.push({ id: `preflight-required-port-${node.id}-${port.id}`, severity: "error", code: "missing_required_input", message: `${node.name} needs its ${port.label} input.`, fix: "Connect a compatible source.", nodeId: node.id, edgeId: null, portId: port.id });
    }
    for (const parameterId of node.parameterIds) {
      if (!parameters.has(parameterId)) diagnostics.push({ id: `preflight-parameter-${node.id}-${parameterId}`, severity: "error", code: "missing_parameter", message: `${node.name} references a missing parameter.`, fix: "Choose an existing typed parameter.", nodeId: node.id, edgeId: null, portId: null });
    }
    if ((node.kind === "layer_additive" || node.kind === "layer_override") && node.maskBindings.length === 0) {
      diagnostics.push({ id: `preflight-mask-${node.id}`, severity: "warning", code: "empty_mask", message: `${node.name} affects the complete animation bundle because its mask is empty.`, fix: "Add exact binding patterns when only part of the asset should be layered.", nodeId: node.id, edgeId: null, portId: null });
    }
    for (const selector of node.maskBindings) {
      const reason = animationGraphMaskSelectorError(selector);
      if (reason) diagnostics.push({ id: `preflight-mask-selector-${node.id}-${selector}`, severity: "error", code: "invalid_mask_selector", message: `Mask selector ${JSON.stringify(selector)} is invalid. ${reason}`, fix: "Use a slash path such as **/Transform/rotation; * matches one segment and ** matches any depth.", nodeId: node.id, edgeId: null, portId: "mask" });
    }
    const inputs = graph.edges.filter((edge) => edge.enabled && edge.toNodeId === node.id && edge.toPortId === "poses");
    const samples = node.samples ?? [];
    const duplicateSample = duplicate(samples.map((sample) => sample.id));
    const duplicateSampleEdge = duplicate(samples.map((sample) => sample.edgeId));
    if (duplicateSample) diagnostics.push({ id: `preflight-sample-id-${node.id}-${duplicateSample}`, severity: "error", code: "duplicate_sample_id", message: `Blend sample ID ${duplicateSample} is not unique.`, fix: "Restore durable unique sample identities.", nodeId: node.id, edgeId: null, portId: "poses" });
    if (duplicateSampleEdge) diagnostics.push({ id: `preflight-sample-edge-${node.id}-${duplicateSampleEdge}`, severity: "error", code: "duplicate_sample_edge", message: `More than one blend sample references edge ${duplicateSampleEdge}.`, fix: "Keep exactly one sample per connected pose edge.", nodeId: node.id, edgeId: duplicateSampleEdge, portId: "poses" });
    const inputEdgeIds = new Set(inputs.map((edge) => edge.id));
    const samplesMatchInputs = samples.length === inputs.length && samples.every((sample) => inputEdgeIds.has(sample.edgeId) && sample.position.length === 2 && sample.position.every(Number.isFinite));
    if (node.kind === "blend_1d" && !samplesMatchInputs) {
      diagnostics.push({ id: `preflight-blend1d-${node.id}`, severity: "error", code: "blend_1d_points", message: `${node.name} needs one finite stable-ID sample per pose connection.`, fix: `Author ${inputs.length} sample${inputs.length === 1 ? "" : "s"}, each keyed to its connection.`, nodeId: node.id, edgeId: null, portId: "poses" });
    }
    if (node.kind === "blend_1d" && (samples.some((sample) => sample.position[1] !== 0) || new Set(samples.map((sample) => sample.position[0])).size !== samples.length)) {
      diagnostics.push({ id: `preflight-blend1d-order-${node.id}`, severity: "error", code: "blend_1d_position", message: `${node.name} sample x positions must be unique and y must be zero.`, fix: "Give every keyed sample a distinct finite x position.", nodeId: node.id, edgeId: null, portId: "poses" });
    }
    if (node.kind === "blend_2d_cartesian") {
      const sampleIds = new Set(samples.map((sample) => sample.id));
      const samplesById = new Map(samples.map((sample) => [sample.id, sample]));
      if (inputs.length < 3) {
        diagnostics.push({ id: `preflight-blend2d-cardinality-${node.id}`, severity: "error", code: "blend_2d_cardinality", message: `${node.name} needs at least three distinct pose points.`, fix: "Connect three or more pose sources with non-collinear Cartesian points.", nodeId: node.id, edgeId: null, portId: "poses" });
      }
      if (!samplesMatchInputs) {
        diagnostics.push({ id: `preflight-blend2d-points-${node.id}`, severity: "error", code: "blend_2d_points", message: `${node.name} needs one finite keyed x/y sample per pose connection.`, fix: `Author ${inputs.length} durable samples keyed to their connection IDs.`, nodeId: node.id, edgeId: null, portId: "poses" });
      }
      if (inputs.length >= 3 && node.triangles.length === 0) {
        diagnostics.push({ id: `preflight-blend2d-triangles-${node.id}`, severity: "error", code: "blend_2d_triangulation", message: `${node.name} needs authored Cartesian triangulation.`, fix: "Author stable sample-ID triples; runtime Delaunay is intentionally not used.", nodeId: node.id, edgeId: null, portId: "poses" });
      }
      for (const triangle of node.triangles) {
        if (new Set(triangle).size !== 3 || triangle.some((id) => !sampleIds.has(id))) {
          diagnostics.push({ id: `preflight-blend2d-triangle-${node.id}-${triangle.join("-")}`, severity: "error", code: "blend_2d_triangle_membership", message: "A Blend2D triangle must reference three distinct connected sample IDs.", fix: "Rebuild the authored triangulation from keyed samples.", nodeId: node.id, edgeId: null, portId: "poses" });
        } else {
          const [a, b, c] = triangle.map((id) => samplesById.get(id)!.position);
          const twiceArea = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
          if (!Number.isFinite(twiceArea) || Math.abs(twiceArea) <= Number.EPSILON) diagnostics.push({ id: `preflight-blend2d-degenerate-${node.id}-${triangle.join("-")}`, severity: "error", code: "blend_2d_degenerate_triangle", message: "A Blend2D triangle is collinear or degenerate.", fix: "Move its Cartesian points so the triangle has a stable area.", nodeId: node.id, edgeId: null, portId: "poses" });
        }
      }
    }
  }
  for (const parameter of graph.parameters) {
    if (!parameter.name.trim()) diagnostics.push({ id: `preflight-parameter-empty-${parameter.id}`, severity: "error", code: "empty_parameter_name", message: "Parameter names cannot be empty.", fix: "Name the typed parameter.", nodeId: null, edgeId: null, portId: null });
    if ((parameter.kind === "float" || parameter.kind === "integer") && (typeof parameter.defaultValue !== "number" || !Number.isFinite(parameter.defaultValue))) diagnostics.push({ id: `preflight-parameter-default-${parameter.id}`, severity: "error", code: "parameter_default_type", message: `${parameter.name} needs a finite numeric default.`, fix: "Choose a finite default matching its type.", nodeId: null, edgeId: null, portId: null });
    if (parameter.kind === "trigger" && parameter.defaultValue !== false) diagnostics.push({ id: `preflight-trigger-default-${parameter.id}`, severity: "error", code: "trigger_default_true", message: `${parameter.name} is momentary and cannot have an authored true default.`, fix: "Keep trigger defaults false and fire them from the preview button or runtime input.", nodeId: null, edgeId: null, portId: null });
    if (parameter.min !== null && parameter.max !== null && parameter.min > parameter.max) diagnostics.push({ id: `preflight-parameter-range-${parameter.id}`, severity: "error", code: "parameter_range", message: `${parameter.name} has an inverted range.`, fix: "Set minimum less than or equal to maximum.", nodeId: null, edgeId: null, portId: null });
  }
  for (const machine of graph.stateMachines) {
    const stateIds = new Set(machine.states.map((state) => state.id));
    const facade = graph.nodes.find((node) => node.kind === "state_machine" && node.stateMachineId === machine.id);
    if (!stateIds.has(machine.entryStateId)) diagnostics.push({ id: `preflight-machine-entry-${machine.id}`, severity: "error", code: "missing_entry_state", message: `${machine.name} has no valid entry state.`, fix: "Choose one existing state as entry.", nodeId: facade?.id ?? null, edgeId: null, portId: null });
    if (facade) {
      const expectedSources = new Set(machine.states.map((state) => state.poseNodeId));
      const incoming = graph.edges.filter((edge) => edge.enabled && edge.toNodeId === facade.id && edge.toPortId === "states");
      const bySource = new Map<string, typeof incoming>();
      for (const edge of incoming) bySource.set(edge.fromNodeId, [...(bySource.get(edge.fromNodeId) ?? []), edge]);
      for (const sourceId of expectedSources) {
        const matching = bySource.get(sourceId) ?? [];
        if (matching.length === 0) diagnostics.push({ id: `preflight-facade-missing-${facade.id}-${sourceId}`, severity: "error", code: "missing_state_pose_edge", message: `${machine.name} hides state pose ${sourceId} because its façade edge is missing.`, fix: "Synchronize the state list so it creates exactly one enabled façade edge per pose root.", nodeId: facade.id, edgeId: null, portId: "states" });
        if (matching.length > 1) diagnostics.push({ id: `preflight-facade-duplicate-${facade.id}-${sourceId}`, severity: "error", code: "duplicate_state_pose_edge", message: `${machine.name} has ${matching.length} enabled façade edges for state pose ${sourceId}.`, fix: "Keep exactly one enabled façade edge for this pose root.", nodeId: facade.id, edgeId: matching[1].id, portId: "states" });
      }
      for (const edge of incoming) {
        if (!expectedSources.has(edge.fromNodeId)) diagnostics.push({ id: `preflight-facade-orphan-${edge.id}`, severity: "error", code: "orphan_state_pose_edge", message: "This façade edge is not referenced by any semantic state.", fix: "Remove the edge or assign its source pose to a state.", nodeId: facade.id, edgeId: edge.id, portId: "states" });
      }
    }
    for (const transition of machine.transitions) {
      if (!stateIds.has(transition.fromStateId) || !stateIds.has(transition.toStateId)) diagnostics.push({ id: `preflight-transition-${transition.id}`, severity: "error", code: "dangling_transition", message: "A transition references a state that no longer exists.", fix: "Reconnect or remove the transition.", nodeId: graph.nodes.find((node) => node.stateMachineId === machine.id)?.id ?? null, edgeId: null, portId: null });
      if (!Number.isInteger(transition.durationTick) || transition.durationTick < 0) diagnostics.push({ id: `preflight-transition-duration-${transition.id}`, severity: "error", code: "transition_duration", message: "Transition duration must be a non-negative whole tick.", fix: "Choose a deterministic integer duration.", nodeId: graph.nodes.find((node) => node.stateMachineId === machine.id)?.id ?? null, edgeId: null, portId: null });
      for (const condition of transition.conditions) if (!parameters.has(condition.parameterId)) diagnostics.push({ id: `preflight-condition-${condition.id}`, severity: "error", code: "missing_condition_parameter", message: "A transition condition references a missing parameter.", fix: "Choose an existing typed parameter.", nodeId: graph.nodes.find((node) => node.stateMachineId === machine.id)?.id ?? null, edgeId: null, portId: null });
    }
    const automatic = machine.transitions.filter((transition) => transition.durationTick === 0 && transition.conditions.length === 0 && transition.exitTime === null);
    const adjacency = new Map<string, Array<{ target: string; id: string }>>();
    for (const transition of automatic) adjacency.set(transition.fromStateId, [...(adjacency.get(transition.fromStateId) ?? []), { target: transition.toStateId, id: transition.id }]);
    for (const edges of adjacency.values()) edges.sort((left, right) => left.id.localeCompare(right.id) || left.target.localeCompare(right.target));
    const colors = new Map<string, number>();
    const stateStack: string[] = [];
    const edgeStack: string[] = [];
    const visitAutomatic = (stateId: string): string[] | null => {
      colors.set(stateId, 1);
      stateStack.push(stateId);
      for (const edge of adjacency.get(stateId) ?? []) {
        const color = colors.get(edge.target) ?? 0;
        if (color === 0) {
          edgeStack.push(edge.id);
          const cycle = visitAutomatic(edge.target);
          if (cycle) return cycle;
          edgeStack.pop();
        } else if (color === 1) {
          const start = stateStack.indexOf(edge.target);
          return [...edgeStack.slice(start), edge.id];
        }
      }
      stateStack.pop();
      colors.set(stateId, 2);
      return null;
    };
    let automaticCycle: string[] | null = null;
    for (const stateId of [...stateIds].sort()) {
      if ((colors.get(stateId) ?? 0) === 0 && (automaticCycle = visitAutomatic(stateId))) break;
    }
    if (automaticCycle) diagnostics.push({ id: `preflight-automatic-cycle-${machine.id}`, severity: "error", code: "unconditional_zero_duration_cycle", message: `Unconditional zero-duration transitions form a cycle: ${automaticCycle.join(", ")}.`, fix: "Add a typed condition, give at least one transition a positive duration, or break the cycle.", nodeId: graph.nodes.find((node) => node.stateMachineId === machine.id)?.id ?? null, edgeId: null, portId: "states" });
  }
  return [...new Map(diagnostics.map((diagnostic) => [diagnostic.id, diagnostic])).values()];
}
