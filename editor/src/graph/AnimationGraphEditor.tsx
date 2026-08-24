//! First-class animation graph authoring and live inspection. The versioned graph draft is authoritative
//! for this editor surface; React Flow is a controlled visual projection. Structural operations remain
//! local until one explicit Apply sends one atomic, undoable native intent. Parameter previews are a
//! separate transient channel and never enter the document draft.

import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ChangeEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import {
  Background,
  Controls,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  type Connection,
  type Edge as FlowEdge,
  type Node as FlowNode,
  type NodeProps,
  type OnSelectionChangeParams,
  type ReactFlowInstance,
  type Viewport,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useStore } from "zustand";
import {
  animationEditorStore,
  animationWorkspaceView,
  type AnimationDraftValue,
  type AnimationWorkspaceKey,
} from "../store/animation";
import { Badge, Button, NumericField, Slider } from "../theme/primitives";
import { EmptyPanelState } from "../theme/workspace";
import { graphEdgeStyle, graphNodeStyle, graphTheme } from "../theme/graph";
import { color, elevation, fontSize, space } from "../theme/tokens";
import {
  ANIMATION_GRAPH_SCHEMA_VERSION,
  type AnimationGraphCondition,
  type AnimationGraphDebugInfo,
  type AnimationGraphDiagnostic,
  type AnimationGraphDocument,
  type AnimationGraphNode,
  type AnimationGraphNodeKind,
  type AnimationGraphParameter,
  type AnimationGraphStateInfo,
  type AnimationGraphSourceInfo,
  type AnimationGraphTransition,
  type AnimationGraphValue,
} from "../transport/protocol";
import type { EditorClient } from "../transport/session";
import {
  animationGraphLocalId,
  animationGraphCompatibleParameterKinds,
  animationGraphEdgeWeightSupport,
  animationGraphMaskSelectorError,
  animationGraphPorts,
  animationGraphPreflight,
  animationGraphReadableSchemaVersion,
  canonicalizeAnimationGraphDocument,
  cloneAnimationGraph,
  createAnimationGraphNode,
  createAnimationGraphParameter,
  createEmptyAnimationGraph,
  createLocomotionGraphPreset,
  defaultAnimationGraphValue,
  rebaseAnimationGraphDraft,
  synchronizeStateMachineFacadeEdges,
} from "./animation-graph-model";

interface AnimationFlowNodeData extends Record<string, unknown> {
  label: string;
  kind: AnimationGraphNodeKind;
  readiness: "ready" | "warning" | "blocked";
  readinessReason: string;
  activeWeight: number | null;
  activeState: string | null;
  costMicros: number | null;
  showWeights: boolean;
  showCosts: boolean;
  ports: ReturnType<typeof animationGraphPorts>;
}

type AnimationFlowNode = FlowNode<AnimationFlowNodeData, "animationGraph">;

const NODE_LABELS: ReadonlyArray<{ kind: AnimationGraphNodeKind; label: string; detail: string }> = [
  { kind: "reference_pose", label: "Reference pose", detail: "Typed authored fallback values" },
  { kind: "sequence", label: "Sequence", detail: "Authored sequence or ready clip source" },
  { kind: "blend_normalized", label: "Normalized blend", detail: "Weights normalized to one" },
  { kind: "blend_direct", label: "Direct blend", detail: "Use authored weights directly" },
  { kind: "blend_1d", label: "Blend 1D", detail: "Threshold blend over one number" },
  { kind: "blend_2d_cartesian", label: "Cartesian Blend 2D", detail: "Authored triangulation; no runtime Delaunay" },
  { kind: "layer_override", label: "Override layer + mask", detail: "Replace a typed binding subset" },
  { kind: "layer_additive", label: "Additive layer + mask", detail: "Apply deltas from a reference pose" },
  { kind: "state_machine", label: "State machine", detail: "Priority transitions and typed conditions" },
  { kind: "output", label: "Output", detail: "One final mixed property bundle" },
];

const FUTURE_NODES = [
  ["Motion matching", "Requires a searchable pose database and deterministic feature extraction."],
  ["IK / constraints", "Requires the skeletal constraint and solver gate."],
  ["Retarget", "Requires source/target rig maps and bind-pose validation."],
  ["Directional Blend 2D", "Schema v2 deliberately supports Cartesian triangulation only."],
] as const;

const EMPTY_STATE = (sequenceId: string): AnimationGraphStateInfo => ({
  schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
  sequenceId,
  revision: "loading",
  graph: null,
  nodePresentation: [],
  sources: [],
  compile: { state: "missing", authoredRevision: "loading", compiledRevision: null, compiledHash: null, lastGoodRevision: null, lastGoodHash: null, message: "Loading graph…" },
  diagnostics: [],
});

const GRAPH_COMPILE_POLL_INTERVAL_MS = 200;

function graphCompilePollKey(state: AnimationGraphStateInfo): string | null {
  if (state.compile.state !== "compiling" && state.compile.state !== "stale") return null;
  return [
    state.sequenceId,
    state.revision,
    state.compile.authoredRevision,
    state.graph?.id ?? "missing",
  ].join("\u0000");
}

function isGraphDocumentShape(value: unknown, sequenceId: string): boolean {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const graph = value as Record<string, unknown>;
  return graph.sequenceId === sequenceId
    && typeof graph.id === "string"
    && Array.isArray(graph.nodes)
    && Array.isArray(graph.edges);
}

function isReadableGraphDocument(value: AnimationDraftValue | undefined, sequenceId: string): value is AnimationDraftValue & AnimationGraphDocument {
  return isGraphDocumentShape(value, sequenceId) && animationGraphReadableSchemaVersion(value) !== null;
}

interface StoredGraphDraft {
  kind: "animation-graph-draft";
  graph: AnimationGraphDocument;
  baseRevision: string;
  baseGraph: AnimationGraphDocument | null;
  conflictRevision: string | null;
}

function isStoredGraphDraft(value: AnimationDraftValue | undefined, sequenceId: string): value is AnimationDraftValue & StoredGraphDraft {
  if (!value || typeof value !== "object" || Array.isArray(value)) return false;
  const candidate = value as Record<string, unknown>;
  return candidate.kind === "animation-graph-draft"
    && typeof candidate.baseRevision === "string"
    && (candidate.baseGraph === null || isReadableGraphDocument(candidate.baseGraph as AnimationDraftValue, sequenceId))
    && isReadableGraphDocument(candidate.graph as AnimationDraftValue, sequenceId);
}

function graphJson(graph: AnimationGraphDocument | null): string {
  return graph ? JSON.stringify(graph) : "";
}

function compileTone(state: AnimationGraphStateInfo["compile"]["state"]): "success" | "warn" | "neutral" {
  if (state === "ready") return "success";
  if (state === "invalid" || state === "stale") return "warn";
  return "neutral";
}

function nodeKindLabel(kind: AnimationGraphNodeKind): string {
  return NODE_LABELS.find((item) => item.kind === kind)?.label ?? kind.replaceAll("_", " ");
}

const AnimationGraphNodeCard = memo(function AnimationGraphNodeCard({ data, selected }: NodeProps<AnimationFlowNode>) {
  const border = data.readiness === "blocked"
    ? color.danger.border
    : data.readiness === "warning"
      ? color.warn.border
      : selected
        ? color.accent.base
        : color.border.strong;
  return (
    <div
      className="animation-graph-node"
      data-readiness={data.readiness}
      data-active={data.activeWeight !== null || undefined}
      title={data.readinessReason}
      role="group"
      aria-label={`${data.label}, ${nodeKindLabel(data.kind)}, ${data.readiness}`}
      aria-roledescription="animation graph node"
      style={{
        ...graphNodeStyle(data.readiness === "blocked" ? "blocked" : selected ? "selected" : data.activeWeight !== null ? "live" : "default"),
        minWidth: 154,
        border: `${selected ? 2 : 1}px solid ${border}`,
        boxShadow: data.activeWeight !== null ? `0 0 0 2px ${color.success.border}, ${elevation.e1}` : elevation.e2,
      }}
    >
      {data.ports.filter((port) => port.direction === "input").map((port, index) => (
        <Handle key={port.id} id={port.id} type="target" position={Position.Left} style={{ top: 34 + index * 18 }} aria-label={`${port.label} input`} />
      ))}
      <div style={{ padding: `${space.sm}px ${space.md}px`, borderBottom: `1px solid ${color.border.subtle}`, fontSize: fontSize.body, fontWeight: 600 }}>{data.label}</div>
      <div style={{ display: "grid", gap: 3, padding: `${space.xs}px ${space.md}px ${space.sm}px`, color: color.text.muted, fontSize: fontSize.micro }}>
        <span>{nodeKindLabel(data.kind)}</span>
        {data.showWeights && data.activeWeight !== null && <span data-testid="animation-graph-node-weight">weight {data.activeWeight.toFixed(3)}</span>}
        {data.activeState && <span>state {data.activeState}</span>}
        {data.showCosts && data.costMicros !== null && <span>{data.costMicros} μs</span>}
      </div>
      {data.ports.filter((port) => port.direction === "output").map((port, index) => (
        <Handle key={port.id} id={port.id} type="source" position={Position.Right} style={{ top: 34 + index * 18 }} aria-label={`${port.label} output`} />
      ))}
    </div>
  );
});

const NODE_TYPES = { animationGraph: AnimationGraphNodeCard } as const;
// React Flow normally measures custom nodes after mount. Supplying truthful initial bounds keeps the
// controlled projection renderable while an authoritative Apply swaps in new node-presentation data;
// measured dimensions still replace these estimates as soon as the browser observes the real cards.
const FLOW_NODE_INITIAL_WIDTH = 184;
const FLOW_NODE_INITIAL_HEIGHT = 58;

function parameterValueLabel(value: AnimationGraphValue): string {
  return Array.isArray(value) ? value.join(", ") : String(value);
}

function requestId(): string {
  return animationGraphLocalId("graph-request");
}

export function AnimationGraphEditor({
  client,
  sequenceId,
  workspaceKey,
}: {
  client: EditorClient;
  sequenceId: string;
  workspaceKey: AnimationWorkspaceKey;
}) {
  const view = useStore(animationEditorStore, (store) => animationWorkspaceView(store, workspaceKey).graph);
  const [state, setState] = useState<AnimationGraphStateInfo>(() => EMPTY_STATE(sequenceId));
  const [draft, setDraftState] = useState<AnimationGraphDocument | null>(null);
  const [draftBaseRevision, setDraftBaseRevision] = useState("loading");
  const [draftBaseGraph, setDraftBaseGraph] = useState<AnimationGraphDocument | null>(null);
  const [conflictRevision, setConflictRevision] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [debug, setDebug] = useState<AnimationGraphDebugInfo | null>(null);
  const [previewValues, setPreviewValues] = useState<Readonly<Record<string, AnimationGraphValue>>>({});
  const dragPositions = useRef<Record<string, { x: number; y: number }>>({});
  const [selectedTransitionId, setSelectedTransitionId] = useState<string | null>(null);
  const [newParameterKind, setNewParameterKind] = useState<AnimationGraphParameter["kind"]>("float");
  const [newParameterName, setNewParameterName] = useState("");
  const [listSourceNodeId, setListSourceNodeId] = useState("");
  const [listSourcePortId, setListSourcePortId] = useState("pose");
  const [listTargetNodeId, setListTargetNodeId] = useState("");
  const [listTargetPortId, setListTargetPortId] = useState("pose");
  const [liveMessage, setLiveMessage] = useState("Graph editor ready.");
  const flowRef = useRef<ReactFlowInstance<AnimationFlowNode, FlowEdge> | null>(null);
  const fitOnNextGraphRender = useRef(false);
  const paletteSearchRef = useRef<HTMLInputElement>(null);
  const focusPaletteSearchOnOpen = useRef(false);
  const loadEpoch = useRef(0);
  const compilePollEpoch = useRef(0);
  const currentStateRef = useRef(state);
  const draftKey = `graph-draft:${sequenceId}`;

  const commitGraphState = useCallback((next: AnimationGraphStateInfo) => {
    currentStateRef.current = next;
    setState(next);
    const nextDebugRevision = next.compile.state === "ready"
      ? next.compile.compiledRevision
      : next.compile.lastGoodRevision ?? next.compile.compiledRevision;
    const nextDebugHash = next.compile.state === "ready"
      ? next.compile.compiledHash
      : next.compile.lastGoodHash ?? next.compile.compiledHash;
    setDebug((current) => current
      && next.graph
      && nextDebugRevision
      && nextDebugHash
      && current.graphId === next.graph.id
      && current.graphRevision === nextDebugRevision
      && current.compiledHash === nextDebugHash
      ? current
      : null);
  }, []);

  const persistDraft = useCallback((next: AnimationGraphDocument | null, metadata?: Partial<Pick<StoredGraphDraft, "baseRevision" | "baseGraph" | "conflictRevision">>) => {
    const canonical = next ? canonicalizeAnimationGraphDocument(next) : null;
    const nextBaseRevision = metadata?.baseRevision ?? draftBaseRevision;
    const nextBaseGraph = metadata && "baseGraph" in metadata ? metadata.baseGraph ?? null : draftBaseGraph;
    const nextConflict = metadata && "conflictRevision" in metadata ? metadata.conflictRevision ?? null : conflictRevision;
    setDraftState(canonical);
    setDraftBaseRevision(nextBaseRevision);
    setDraftBaseGraph(nextBaseGraph ? canonicalizeAnimationGraphDocument(nextBaseGraph) : null);
    setConflictRevision(nextConflict);
    animationEditorStore.getState().setGraphDraft(
      workspaceKey,
      draftKey,
      canonical ? {
        kind: "animation-graph-draft",
        graph: canonical,
        baseRevision: nextBaseRevision,
        baseGraph: nextBaseGraph,
        conflictRevision: nextConflict,
      } as unknown as AnimationDraftValue : null,
    );
  }, [conflictRevision, draftBaseGraph, draftBaseRevision, draftKey, workspaceKey]);

  useEffect(() => {
    const epoch = ++loadEpoch.current;
    setLoading(true);
    void client.animationGraphState(sequenceId).then((next) => {
      if (epoch !== loadEpoch.current) return;
      const persisted = animationWorkspaceView(animationEditorStore.getState(), workspaceKey).graph.drafts[draftKey];
      const stateVersion = animationGraphReadableSchemaVersion(next);
      const graphVersion = next.graph ? animationGraphReadableSchemaVersion(next.graph) : ANIMATION_GRAPH_SCHEMA_VERSION;
      if (stateVersion === null || graphVersion === null) {
        commitGraphState(next);
        setDraftState(null);
        const received = stateVersion === null
          ? (next as { schemaVersion: number }).schemaVersion
          : (next.graph as unknown as { schemaVersion: number }).schemaVersion;
        setError(`Animation graph schema ${received} is unsupported. This editor only reads schema 1 or ${ANIMATION_GRAPH_SCHEMA_VERSION}; the document remains untouched.`);
        return;
      }
      const authoritative = next.graph ? canonicalizeAnimationGraphDocument(next.graph) : null;
      const migratedByBrowser = stateVersion !== ANIMATION_GRAPH_SCHEMA_VERSION || graphVersion !== ANIMATION_GRAPH_SCHEMA_VERSION;
      const canonicalState: AnimationGraphStateInfo = {
        ...next,
        schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
        graph: authoritative,
        diagnostics: migratedByBrowser && !next.diagnostics.some((item) => item.code === "legacy_graph_schema_migrated")
          ? [...next.diagnostics, { id: "browser-schema-v1-migrated", severity: "warning", code: "legacy_graph_schema_migrated", message: "Legacy schema 1 graph data was upgraded in memory to canonical schema 2.", fix: "Review keyed blend samples and edge weight bindings, then Apply once to persist schema 2.", nodeId: null, edgeId: null, portId: null }]
          : next.diagnostics,
      };
      commitGraphState(canonicalState);

      const persistedEnvelope = persisted && typeof persisted === "object" && !Array.isArray(persisted) && (persisted as { kind?: unknown }).kind === "animation-graph-draft"
        ? persisted as unknown as { graph?: unknown; baseGraph?: unknown }
        : null;
      const persistedGraphCandidate = persistedEnvelope?.graph ?? (isGraphDocumentShape(persisted, sequenceId) ? persisted : null);
      const persistedBaseCandidate = persistedEnvelope?.baseGraph;
      const persistedHasUnknownSchema = (isGraphDocumentShape(persistedGraphCandidate, sequenceId) && animationGraphReadableSchemaVersion(persistedGraphCandidate) === null)
        || (persistedBaseCandidate != null && isGraphDocumentShape(persistedBaseCandidate, sequenceId) && animationGraphReadableSchemaVersion(persistedBaseCandidate) === null);
      if (persistedHasUnknownSchema) {
        setDraftState(authoritative);
        setDraftBaseRevision(next.revision);
        setDraftBaseGraph(authoritative ? cloneAnimationGraph(authoritative) : null);
        setConflictRevision(null);
        setError(`A local animation graph draft uses an unsupported schema. It was left untouched in local storage; schema 1 or ${ANIMATION_GRAPH_SCHEMA_VERSION} is required.`);
        return;
      }
      if (isStoredGraphDraft(persisted, sequenceId)) {
        const restored = canonicalizeAnimationGraphDocument(persisted.graph);
        const restoredBase = persisted.baseGraph ? canonicalizeAnimationGraphDocument(persisted.baseGraph) : null;
        const matchesAuthoritative = graphJson(restored) === graphJson(authoritative);
        const advanced = persisted.baseRevision !== next.revision && !matchesAuthoritative;
        const nextBaseRevision = matchesAuthoritative ? next.revision : persisted.baseRevision;
        const nextBaseGraph = matchesAuthoritative ? authoritative : restoredBase;
        const nextConflict = matchesAuthoritative ? null : persisted.conflictRevision ?? (advanced ? next.revision : null);
        setDraftState(restored);
        setDraftBaseRevision(nextBaseRevision);
        setDraftBaseGraph(nextBaseGraph);
        setConflictRevision(nextConflict);
        if (animationGraphReadableSchemaVersion(persisted.graph) !== ANIMATION_GRAPH_SCHEMA_VERSION
          || (persisted.baseGraph && animationGraphReadableSchemaVersion(persisted.baseGraph) !== ANIMATION_GRAPH_SCHEMA_VERSION)) {
          animationEditorStore.getState().setGraphDraft(workspaceKey, draftKey, {
            kind: "animation-graph-draft",
            graph: restored,
            baseRevision: nextBaseRevision,
            baseGraph: nextBaseGraph,
            conflictRevision: nextConflict,
          } as unknown as AnimationDraftValue);
        }
        setError(advanced ? "The saved graph advanced while this local draft was open. Rebase or discard before applying." : null);
      } else if (isReadableGraphDocument(persisted, sequenceId)) {
        // A legacy editor draft did not retain its base revision. Keep it, but require an explicit choice.
        const restored = canonicalizeAnimationGraphDocument(persisted);
        setDraftState(restored);
        setDraftBaseRevision(next.revision);
        setDraftBaseGraph(authoritative);
        setConflictRevision(next.revision);
        animationEditorStore.getState().setGraphDraft(workspaceKey, draftKey, {
          kind: "animation-graph-draft",
          graph: restored,
          baseRevision: next.revision,
          baseGraph: authoritative,
          conflictRevision: next.revision,
        } as unknown as AnimationDraftValue);
        setError("This restored draft predates revision tracking. Rebase or discard before applying it.");
      } else {
        setDraftState(authoritative);
        setDraftBaseRevision(next.revision);
        setDraftBaseGraph(authoritative ? cloneAnimationGraph(authoritative) : null);
        setConflictRevision(null);
        setError(null);
      }
      setLiveMessage(canonicalState.compile.message);
    }).catch((cause: unknown) => {
      if (epoch === loadEpoch.current) setError(cause instanceof Error ? cause.message : "Animation graph service is unavailable.");
    }).finally(() => {
      if (epoch === loadEpoch.current) setLoading(false);
    });
    return () => { loadEpoch.current += 1; };
  }, [client, commitGraphState, draftKey, sequenceId, workspaceKey]);

  const mutateDraft = useCallback((update: (current: AnimationGraphDocument) => AnimationGraphDocument) => {
    if (!draft) return;
    persistDraft(update(cloneAnimationGraph(draft)));
  }, [draft, persistDraft]);

  useEffect(() => {
    if (!fitOnNextGraphRender.current || !draft || !flowRef.current) return;
    fitOnNextGraphRender.current = false;
    void flowRef.current.fitView({ duration: 180, padding: 0.18 });
  }, [draft]);

  useEffect(() => {
    if (!view.paletteOpen || !focusPaletteSearchOnOpen.current || !paletteSearchRef.current) return;
    focusPaletteSearchOnOpen.current = false;
    paletteSearchRef.current.focus();
  }, [view.paletteOpen]);

  const authorPreset = (graph: AnimationGraphDocument) => {
    fitOnNextGraphRender.current = true;
    persistDraft(graph);
  };

  const preflight = useMemo(() => draft ? animationGraphPreflight(draft) : [], [draft]);
  const schemaSupported = (state as { schemaVersion: number }).schemaVersion === ANIMATION_GRAPH_SCHEMA_VERSION
    && (!state.graph || (state.graph as { schemaVersion: number }).schemaVersion === ANIMATION_GRAPH_SCHEMA_VERSION);
  const diagnostics = useMemo(() => {
    const ids = new Set(state.diagnostics.map((diagnostic) => diagnostic.id));
    return [...state.diagnostics, ...preflight.filter((diagnostic) => !ids.has(diagnostic.id))];
  }, [preflight, state.diagnostics]);
  const dirty = graphJson(draft) !== graphJson(state.graph);
  const presentation = useMemo(() => new Map(state.nodePresentation.map((item) => [item.nodeId, item])), [state.nodePresentation]);
  const activeNodes = useMemo(() => new Map(debug?.activeNodes.map((item) => [item.nodeId, item]) ?? []), [debug]);
  const activeEdges = useMemo(() => new Map(debug?.activeEdges.map((item) => [item.edgeId, item]) ?? []), [debug]);
  const hasMeasuredCost = debug !== null
    && (debug.evaluationCostMicros !== null || debug.activeNodes.some((node) => node.costMicros !== null));

  const debugRevision = state.compile.state === "ready"
    ? state.compile.compiledRevision
    : state.compile.lastGoodRevision ?? state.compile.compiledRevision;
  const debugHash = state.compile.state === "ready"
    ? state.compile.compiledHash
    : state.compile.lastGoodHash ?? state.compile.compiledHash;
  const compilePollKey = graphCompilePollKey(state);

  useEffect(() => {
    const epoch = ++compilePollEpoch.current;
    if (!compilePollKey) return;

    let cancelled = false;
    let timer: number | null = null;

    const schedule = () => {
      if (cancelled || epoch !== compilePollEpoch.current) return;
      timer = window.setTimeout(() => void poll(), GRAPH_COMPILE_POLL_INTERVAL_MS);
    };

    async function poll() {
      if (cancelled
        || epoch !== compilePollEpoch.current
        || graphCompilePollKey(currentStateRef.current) !== compilePollKey) return;
      try {
        const next = await client.animationGraphState(sequenceId);
        if (cancelled
          || epoch !== compilePollEpoch.current
          || graphCompilePollKey(currentStateRef.current) !== compilePollKey) return;
        if (next.sequenceId !== sequenceId) {
          setLiveMessage("Compilation status returned for another sequence; retrying.");
          schedule();
          return;
        }
        const stateVersion = animationGraphReadableSchemaVersion(next);
        const graphVersion = next.graph ? animationGraphReadableSchemaVersion(next.graph) : ANIMATION_GRAPH_SCHEMA_VERSION;
        if (stateVersion === null || graphVersion === null) {
          const received = stateVersion === null
            ? (next as { schemaVersion: number }).schemaVersion
            : (next.graph as unknown as { schemaVersion: number }).schemaVersion;
          setError(`Animation graph schema ${received} is unsupported. Compile status polling stopped without changing the document.`);
          return;
        }
        const authoritative = next.graph ? canonicalizeAnimationGraphDocument(next.graph) : null;
        const canonicalState: AnimationGraphStateInfo = {
          ...next,
          schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
          graph: authoritative,
        };
        commitGraphState(canonicalState);
        setLiveMessage(canonicalState.compile.message);
        if (graphCompilePollKey(canonicalState)) schedule();
      } catch {
        if (!cancelled && epoch === compilePollEpoch.current) {
          setLiveMessage("Compilation status refresh failed; retrying.");
          schedule();
        }
      }
    }

    schedule();
    return () => {
      cancelled = true;
      compilePollEpoch.current += 1;
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [client, commitGraphState, compilePollKey, sequenceId]);

  useEffect(() => {
    if (!state.graph || !debugHash || !debugRevision || state.compile.state === "missing") {
      setDebug(null);
      return;
    }
    setDebug((current) => current && current.graphId === state.graph!.id && current.graphRevision === debugRevision && current.compiledHash === debugHash ? current : null);
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const snapshot = await client.animationGraphDebug(state.graph!.id, view.debugInstanceId, view.watches);
        if (cancelled) return;
        if (snapshot.graphId === state.graph!.id
          && snapshot.graphRevision === debugRevision
          && snapshot.compiledHash === debugHash) {
          const bounded = snapshot.activeNodes.length > 256 || snapshot.activeEdges.length > 512 || snapshot.watches.length > 64;
          setDebug({ ...snapshot, activeNodes: snapshot.activeNodes.slice(0, 256), activeEdges: snapshot.activeEdges.slice(0, 512), watches: snapshot.watches.slice(0, 64), truncated: snapshot.truncated || bounded });
          if (view.debugInstanceId !== snapshot.instanceId) {
            animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, debugInstanceId: snapshot.instanceId }));
          }
        }
      } catch (cause: unknown) {
        if (!cancelled) setError(cause instanceof Error ? cause.message : "Graph debug snapshot failed.");
      }
      if (!cancelled) timer = window.setTimeout(() => void poll(), 100);
    };
    void poll();
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [client, debugHash, debugRevision, state.compile.state, state.graph, view.debugInstanceId, view.watches, workspaceKey]);

  const nodes = useMemo<AnimationFlowNode[]>(() => (draft?.nodes ?? []).map((node) => {
    const derived = presentation.get(node.id);
    const active = activeNodes.get(node.id);
    return {
      id: node.id,
      type: "animationGraph",
      position: dragPositions.current[node.id] ?? node.position,
      initialWidth: FLOW_NODE_INITIAL_WIDTH,
      initialHeight: FLOW_NODE_INITIAL_HEIGHT,
      selected: view.selectedNodeIds.includes(node.id),
      draggable: true,
      data: {
        label: node.name,
        kind: node.kind,
        readiness: derived?.readiness ?? "ready",
        readinessReason: derived?.readinessReason ?? "Local draft; native readiness is checked on Apply.",
        activeWeight: active?.weight ?? null,
        activeState: active?.stateId ?? null,
        costMicros: active?.costMicros ?? null,
        showWeights: view.showWeights,
        showCosts: view.showCosts,
        ports: derived?.ports.length ? derived.ports : animationGraphPorts(node.kind),
      },
      ariaLabel: `${node.name}, ${nodeKindLabel(node.kind)}`,
    };
  }), [activeNodes, draft?.nodes, presentation, view.selectedNodeIds, view.showCosts, view.showWeights]);

  const edges = useMemo<FlowEdge[]>(() => (draft?.edges ?? []).map((edge) => {
    const active = activeEdges.get(edge.id);
    return {
      id: edge.id,
      source: edge.fromNodeId,
      sourceHandle: edge.fromPortId,
      target: edge.toNodeId,
      targetHandle: edge.toPortId,
      selected: view.selectedEdgeIds.includes(edge.id),
      label: view.showWeights && active ? active.weight.toFixed(3) : undefined,
      animated: Boolean(active && active.weight > 0),
      style: graphEdgeStyle(
        active
          ? "active"
          : !edge.enabled
            ? "disabled"
            : view.selectedEdgeIds.includes(edge.id)
              ? "selected"
              : "default",
      ),
      ariaLabel: `Connection from ${edge.fromNodeId} to ${edge.toNodeId}`,
    };
  }), [activeEdges, draft?.edges, view.selectedEdgeIds, view.showWeights]);

  const selectedNode = draft?.nodes.find((node) => view.selectedNodeIds.includes(node.id)) ?? null;
  const selectedEdge = draft?.edges.find((edge) => view.selectedEdgeIds.includes(edge.id)) ?? null;
  const selectedEdgeWeightSupport = draft && selectedEdge ? animationGraphEdgeWeightSupport(draft, selectedEdge) : null;
  const selectedEdgeTarget = draft?.nodes.find((node) => node.id === selectedEdge?.toNodeId) ?? null;
  const selectedMachine = selectedNode?.stateMachineId
    ? draft?.stateMachines.find((machine) => machine.id === selectedNode.stateMachineId) ?? null
    : null;
  const selectedTransition = selectedMachine?.transitions.find((transition) => transition.id === selectedTransitionId)
    ?? selectedMachine?.transitions[0]
    ?? null;

  const apply = async () => {
    if (!draft || saving || !schemaSupported || conflictRevision) return;
    setSaving(true);
    try {
      const canonical = canonicalizeAnimationGraphDocument(synchronizeStateMachineFacadeEdges(draft));
      const result = await client.animationGraphSave(sequenceId, {
        schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION,
        expectedRevision: draftBaseRevision,
        requestId: requestId(),
        graph: canonical,
      });
      commitGraphState(result.state);
      setLiveMessage(result.message);
      if (result.ok) {
        setError(null);
        const authoritative = result.state.graph ? canonicalizeAnimationGraphDocument(result.state.graph) : null;
        persistDraft(authoritative, { baseRevision: result.state.revision, baseGraph: authoritative, conflictRevision: null });
        animationEditorStore.getState().setGraphDraft(workspaceKey, draftKey, null);
      } else {
        // A rejected save must not destroy the local structural draft or selection.
        if (result.state.revision !== draftBaseRevision) {
          persistDraft(draft, { baseRevision: draftBaseRevision, baseGraph: draftBaseGraph, conflictRevision: result.state.revision });
          setError(`${result.message} The graph advanced to ${result.state.revision}; Rebase or Discard is required before another Apply.`);
        } else setError(result.message);
      }
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "The graph draft could not be applied.");
    } finally {
      setSaving(false);
    }
  };

  const deleteGraph = async () => {
    if (!state.graph || saving) return;
    setSaving(true);
    try {
      const result = await client.animationGraphDelete(sequenceId, state.graph.id, state.revision, requestId());
      commitGraphState(result.state);
      setLiveMessage(result.message);
      if (result.ok) {
        setError(null);
        persistDraft(null);
        setDebug(null);
        animationEditorStore.getState().setGraphSelection(workspaceKey, [], []);
      } else setError(result.message);
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "The graph could not be deleted.");
    } finally {
      setSaving(false);
    }
  };

  const discard = () => {
    const authoritative = state.graph ? canonicalizeAnimationGraphDocument(state.graph) : null;
    persistDraft(authoritative, { baseRevision: state.revision, baseGraph: authoritative, conflictRevision: null });
    animationEditorStore.getState().setGraphDraft(workspaceKey, draftKey, null);
    animationEditorStore.getState().setGraphSelection(workspaceKey, [], []);
    setError(null);
    setLiveMessage("Local graph draft discarded. The native revision is now shown.");
  };

  const rebase = async () => {
    if (!draft || saving) return;
    setSaving(true);
    try {
      const latest = await client.animationGraphState(sequenceId);
      if (animationGraphReadableSchemaVersion(latest) === null
        || (latest.graph && animationGraphReadableSchemaVersion(latest.graph) === null)) {
        throw new Error(`Rebase stopped because the latest graph uses an unsupported schema; this editor reads schema 1 or ${ANIMATION_GRAPH_SCHEMA_VERSION}.`);
      }
      const latestGraph = latest.graph ? canonicalizeAnimationGraphDocument(latest.graph) : null;
      commitGraphState({ ...latest, schemaVersion: ANIMATION_GRAPH_SCHEMA_VERSION, graph: latestGraph });
      const result = rebaseAnimationGraphDraft(draftBaseGraph, draft, latestGraph);
      if (!result.ok) {
        persistDraft(draft, { baseRevision: draftBaseRevision, baseGraph: draftBaseGraph, conflictRevision: latest.revision });
        setError(`Rebase stopped safely because both peers changed ${result.conflicts.join(", ")}. Discard or resolve those records manually; no collaborator edit was overwritten.`);
        return;
      }
      persistDraft(result.graph, { baseRevision: latest.revision, baseGraph: latestGraph, conflictRevision: null });
      setError(null);
      setLiveMessage(`Local changes rebased onto revision ${latest.revision}. Review, then Apply.`);
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "The latest graph could not be loaded for rebase.");
    } finally {
      setSaving(false);
    }
  };

  const addNode = (kind: AnimationGraphNodeKind) => {
    if (!draft || (kind === "output" && draft.nodes.some((node) => node.kind === "output"))) return;
    const index = draft.nodes.length;
    const node = createAnimationGraphNode(kind, { x: 100 + (index % 3) * 220, y: 80 + Math.floor(index / 3) * 150 });
    if (kind === "sequence") {
      node.sourceId = state.sources.find((source) => source.readiness === "ready" && source.kind !== "reference_pose")?.id ?? null;
    }
    const machine = kind === "state_machine" ? {
      id: animationGraphLocalId("machine"),
      name: node.name,
      entryStateId: "",
      states: [],
      transitions: [],
    } : null;
    if (machine) node.stateMachineId = machine.id;
    persistDraft({
      ...cloneAnimationGraph(draft),
      nodes: [...draft.nodes, node],
      stateMachines: machine ? [...draft.stateMachines, machine] : draft.stateMachines,
    });
    animationEditorStore.getState().setGraphSelection(workspaceKey, [node.id], []);
    setLiveMessage(`${node.name} added to the local draft. Apply to commit it.`);
  };

  const addSourceNode = (source: AnimationGraphSourceInfo) => {
    if (!draft || source.readiness === "blocked") return;
    const kind: AnimationGraphNodeKind = source.kind === "reference_pose" ? "reference_pose" : "sequence";
    const node = createAnimationGraphNode(kind, { x: 70, y: 80 + draft.nodes.length * 34 }, {
      name: source.name,
      sourceId: kind === "sequence" ? source.id : null,
    });
    persistDraft({ ...cloneAnimationGraph(draft), nodes: [...draft.nodes, node] });
    animationEditorStore.getState().setGraphSelection(workspaceKey, [node.id], []);
  };

  const connect = (connection: Connection) => {
    if (!draft || !connection.source || !connection.target || connection.source === connection.target) return;
    const sourceNode = draft.nodes.find((node) => node.id === connection.source);
    const targetNode = draft.nodes.find((node) => node.id === connection.target);
    const sourcePort = sourceNode && animationGraphPorts(sourceNode.kind).find((port) => port.id === connection.sourceHandle && port.direction === "output");
    const targetPort = targetNode && animationGraphPorts(targetNode.kind).find((port) => port.id === connection.targetHandle && port.direction === "input");
    if (!sourcePort || !targetPort || sourcePort.kind !== "pose" || targetPort.kind !== "pose") {
      setError("Only compatible pose outputs and pose inputs can be connected in schema v2.");
      return;
    }
    if (draft.edges.some((edge) => edge.fromNodeId === connection.source && edge.fromPortId === connection.sourceHandle && edge.toNodeId === connection.target && edge.toPortId === connection.targetHandle)) return;
    const edge = { id: animationGraphLocalId("edge"), fromNodeId: connection.source, fromPortId: connection.sourceHandle ?? "pose", toNodeId: connection.target, toPortId: connection.targetHandle ?? "pose", enabled: true, weight: null, weightParameterId: null };
    let next = { ...cloneAnimationGraph(draft), edges: [...draft.edges, edge] };
    if (targetNode.kind === "blend_1d" || targetNode.kind === "blend_2d_cartesian") {
      next = {
        ...next,
        nodes: next.nodes.map((node) => node.id === targetNode.id ? {
          ...node,
          samples: [...node.samples, { id: animationGraphLocalId("sample"), edgeId: edge.id, position: [node.samples.length, 0] }],
        } : node),
      };
    } else if (targetNode.kind === "state_machine" && targetNode.stateMachineId) {
      const machine = next.stateMachines.find((candidate) => candidate.id === targetNode.stateMachineId);
      if (machine && !machine.states.some((graphState) => graphState.poseNodeId === connection.source)) {
        const stateId = animationGraphLocalId("state");
        next = {
          ...next,
          stateMachines: next.stateMachines.map((candidate) => candidate.id === machine.id ? {
            ...candidate,
            entryStateId: candidate.entryStateId || stateId,
            states: [...candidate.states, { id: stateId, name: sourceNode.name, poseNodeId: connection.source!, resetOnEntry: true }],
          } : candidate),
        };
      }
      next = synchronizeStateMachineFacadeEdges(next);
    }
    persistDraft(next);
    animationEditorStore.getState().setGraphSelection(workspaceKey, [], [edge.id]);
    setError(null);
  };

  const removeSelection = () => {
    if (!draft) return;
    const nodeIds = new Set(view.selectedNodeIds);
    const edgeIds = new Set(view.selectedEdgeIds);
    if (nodeIds.has(draft.outputNodeId)) {
      setError("The active Output node cannot be removed. Choose another output before deleting it.");
      return;
    }
    const machineIds = new Set(draft.nodes.filter((node) => nodeIds.has(node.id)).flatMap((node) => node.stateMachineId ? [node.stateMachineId] : []));
    const removedEdgeIds = new Set(draft.edges.filter((edge) => edgeIds.has(edge.id) || nodeIds.has(edge.fromNodeId) || nodeIds.has(edge.toNodeId)).map((edge) => edge.id));
    const removedSampleIds = new Set(draft.nodes.flatMap((node) => node.samples.filter((sample) => removedEdgeIds.has(sample.edgeId)).map((sample) => sample.id)));
    const next = {
      ...cloneAnimationGraph(draft),
      nodes: draft.nodes.filter((node) => !nodeIds.has(node.id)).map((node) => ({
        ...node,
        samples: node.samples.filter((sample) => !removedEdgeIds.has(sample.edgeId)),
        triangles: node.triangles.filter((triangle) => triangle.every((sampleId) => !removedSampleIds.has(sampleId))),
      })),
      edges: draft.edges.filter((edge) => !edgeIds.has(edge.id) && !nodeIds.has(edge.fromNodeId) && !nodeIds.has(edge.toNodeId)),
      stateMachines: draft.stateMachines.filter((machine) => !machineIds.has(machine.id)).map((machine) => ({
        ...machine,
        states: machine.states.filter((graphState) => !nodeIds.has(graphState.poseNodeId)),
        transitions: machine.transitions.filter((transition) => !machine.states.some((graphState) => nodeIds.has(graphState.poseNodeId) && (transition.fromStateId === graphState.id || transition.toStateId === graphState.id))),
      })),
    };
    persistDraft(synchronizeStateMachineFacadeEdges(next));
    animationEditorStore.getState().setGraphSelection(workspaceKey, [], []);
  };

  const onSelectionChange = useCallback((selection: OnSelectionChangeParams<AnimationFlowNode, FlowEdge>) => {
    // React Flow emits an empty selection while reconciling controlled node arrays. Empty user intent is
    // handled explicitly by pane click/Escape so it cannot erase a keyboard/list selection asynchronously.
    if (selection.nodes.length === 0 && selection.edges.length === 0) return;
    animationEditorStore.getState().setGraphSelection(workspaceKey, selection.nodes.map((node) => node.id), selection.edges.map((edge) => edge.id));
  }, [workspaceKey]);

  const onNodeDragStop = (_event: MouseEvent | TouchEvent, node: AnimationFlowNode) => {
    const position = dragPositions.current[node.id] ?? node.position;
    delete dragPositions.current[node.id];
    mutateDraft((graph) => ({ ...graph, nodes: graph.nodes.map((item) => item.id === node.id ? { ...item, position: { x: position.x, y: position.y } } : item) }));
  };

  const onMoveEnd = (_event: MouseEvent | TouchEvent | null, viewport: Viewport) => {
    animationEditorStore.getState().setGraphViewport(workspaceKey, viewport);
  };

  const updateSelectedNode = (patch: Partial<AnimationGraphNode>) => {
    if (!selectedNode) return;
    mutateDraft((graph) => ({ ...graph, nodes: graph.nodes.map((node) => node.id === selectedNode.id ? { ...node, ...patch } : node) }));
  };

  const updateSelectedNodeParameter = (parameterId: string) => {
    if (!selectedNode) return;
    mutateDraft((graph) => ({
      ...graph,
      nodes: graph.nodes.map((node) => node.id === selectedNode.id ? { ...node, parameterIds: parameterId ? [parameterId] : [] } : node),
      edges: parameterId && (selectedNode.kind === "layer_override" || selectedNode.kind === "layer_additive")
        ? graph.edges.map((edge) => edge.toNodeId === selectedNode.id && edge.toPortId === "layer" ? { ...edge, weight: null, weightParameterId: null } : edge)
        : graph.edges,
    }));
  };

  const updateSelectedEdge = (patch: Partial<AnimationGraphDocument["edges"][number]>) => {
    if (!selectedEdge) return;
    mutateDraft((graph) => {
      const target = graph.nodes.find((node) => node.id === selectedEdge.toNodeId);
      const switchesLayerToConstant = patch.weight != null
        && selectedEdge.toPortId === "layer"
        && (target?.kind === "layer_override" || target?.kind === "layer_additive");
      return {
        ...graph,
        edges: graph.edges.map((edge) => edge.id === selectedEdge.id ? { ...edge, ...patch } : edge),
        nodes: switchesLayerToConstant
          ? graph.nodes.map((node) => node.id === target.id ? { ...node, parameterIds: [] } : node)
          : graph.nodes,
      };
    });
  };

  const editConnection = (edgeId: string, patch: Partial<AnimationGraphDocument["edges"][number]>) => {
    mutateDraft((graph) => {
      const edge = graph.edges.find((candidate) => candidate.id === edgeId);
      if (!edge) return graph;
      let updated = { ...edge, ...patch };
      const target = graph.nodes.find((node) => node.id === updated.toNodeId);
      const source = graph.nodes.find((node) => node.id === updated.fromNodeId);
      const sourcePort = source && animationGraphPorts(source.kind).find((port) => port.direction === "output" && port.id === updated.fromPortId);
      const targetPort = target && animationGraphPorts(target.kind).find((port) => port.direction === "input" && port.id === updated.toPortId);
      if (!sourcePort || !targetPort || sourcePort.kind !== targetPort.kind) return graph;
      const weightSupport = animationGraphEdgeWeightSupport(graph, updated);
      updated = {
        ...updated,
        weight: weightSupport.explicit ? updated.weight : null,
        weightParameterId: weightSupport.parameter ? updated.weightParameterId : null,
      };
      if (updated.weight !== null && updated.weightParameterId !== null) updated.weight = null;
      let next: AnimationGraphDocument = {
        ...graph,
        edges: graph.edges.map((candidate) => candidate.id === edgeId ? updated : candidate),
        nodes: graph.nodes.map((node) => {
          const existing = node.samples.find((sample) => sample.edgeId === edgeId);
          if (node.id === target.id && (node.kind === "blend_1d" || node.kind === "blend_2d_cartesian")) {
            return existing ? node : { ...node, samples: [...node.samples, { id: animationGraphLocalId("sample"), edgeId, position: [node.samples.length, 0] }] };
          }
          if (!existing) return node;
          return { ...node, samples: node.samples.filter((sample) => sample.edgeId !== edgeId), triangles: node.triangles.filter((triangle) => !triangle.includes(existing.id)) };
        }),
      };
      if (target.kind === "state_machine" && target.stateMachineId) {
        next = {
          ...next,
          stateMachines: next.stateMachines.map((machine) => {
            if (machine.id !== target.stateMachineId || machine.states.some((graphState) => graphState.poseNodeId === source.id)) return machine;
            const stateId = animationGraphLocalId("state");
            return { ...machine, entryStateId: machine.entryStateId || stateId, states: [...machine.states, { id: stateId, name: source.name, poseNodeId: source.id, resetOnEntry: true }] };
          }),
        };
      }
      return synchronizeStateMachineFacadeEdges(next);
    });
  };

  const removeConnectionById = (edgeId: string) => {
    mutateDraft((graph) => {
      const edge = graph.edges.find((candidate) => candidate.id === edgeId);
      if (!edge) return graph;
      const sampleIds = new Set(graph.nodes.flatMap((node) => node.samples.filter((sample) => sample.edgeId === edgeId).map((sample) => sample.id)));
      const facade = graph.nodes.find((node) => node.id === edge.toNodeId && node.kind === "state_machine");
      let stateMachines = graph.stateMachines;
      if (facade?.stateMachineId) {
        stateMachines = graph.stateMachines.map((machine) => {
          if (machine.id !== facade.stateMachineId) return machine;
          const removed = new Set(machine.states.filter((graphState) => graphState.poseNodeId === edge.fromNodeId).map((graphState) => graphState.id));
          const states = machine.states.filter((graphState) => !removed.has(graphState.id));
          return { ...machine, states, entryStateId: removed.has(machine.entryStateId) ? states[0]?.id ?? "" : machine.entryStateId, transitions: machine.transitions.filter((transition) => !removed.has(transition.fromStateId) && !removed.has(transition.toStateId)) };
        });
      }
      return synchronizeStateMachineFacadeEdges({
        ...graph,
        edges: graph.edges.filter((candidate) => candidate.id !== edgeId),
        nodes: graph.nodes.map((node) => ({ ...node, samples: node.samples.filter((sample) => sample.edgeId !== edgeId), triangles: node.triangles.filter((triangle) => triangle.every((id) => !sampleIds.has(id))) })),
        stateMachines,
      });
    });
    animationEditorStore.getState().setGraphSelection(workspaceKey, [], []);
  };

  const updateTransition = (patch: Partial<AnimationGraphTransition>) => {
    if (!selectedMachine || !selectedTransition) return;
    mutateDraft((graph) => ({
      ...graph,
      stateMachines: graph.stateMachines.map((machine) => machine.id === selectedMachine.id
        ? { ...machine, transitions: machine.transitions.map((transition) => transition.id === selectedTransition.id ? { ...transition, ...patch } : transition) }
        : machine),
    }));
  };

  const updateMachineState = (stateId: string, patch: Partial<AnimationGraphDocument["stateMachines"][number]["states"][number]>) => {
    if (!selectedMachine) return;
    mutateDraft((graph) => synchronizeStateMachineFacadeEdges({ ...graph, stateMachines: graph.stateMachines.map((machine) => machine.id === selectedMachine.id ? { ...machine, states: machine.states.map((graphState) => graphState.id === stateId ? { ...graphState, ...patch } : graphState) } : machine) }));
  };

  const removeMachineState = (stateId: string) => {
    if (!selectedMachine) return;
    mutateDraft((graph) => synchronizeStateMachineFacadeEdges({
      ...graph,
      stateMachines: graph.stateMachines.map((machine) => {
        if (machine.id !== selectedMachine.id) return machine;
        const states = machine.states.filter((graphState) => graphState.id !== stateId);
        return {
          ...machine,
          states,
          entryStateId: machine.entryStateId === stateId ? states[0]?.id ?? "" : machine.entryStateId,
          transitions: machine.transitions.filter((transition) => transition.fromStateId !== stateId && transition.toStateId !== stateId),
        };
      }),
    }));
  };

  const updateCondition = (conditionId: string, patch: Partial<AnimationGraphCondition>) => {
    if (!selectedTransition) return;
    updateTransition({ conditions: selectedTransition.conditions.map((condition) => condition.id === conditionId ? { ...condition, ...patch } : condition) });
  };

  const removeCondition = (conditionId: string) => {
    if (!selectedTransition) return;
    updateTransition({ conditions: selectedTransition.conditions.filter((condition) => condition.id !== conditionId) });
  };

  const addState = () => {
    if (!selectedMachine || !selectedNode || !draft) return;
    const source = draft.nodes.find((node) => node.kind === "sequence" || node.kind === "blend_1d" || node.kind === "blend_2d_cartesian" || node.kind === "reference_pose");
    if (!source) {
      setError("Add a pose-producing source before creating a state.");
      return;
    }
    const id = animationGraphLocalId("state");
    mutateDraft((graph) => synchronizeStateMachineFacadeEdges({
      ...graph,
      stateMachines: graph.stateMachines.map((machine) => machine.id === selectedMachine.id ? {
        ...machine,
        entryStateId: machine.entryStateId || id,
        states: [...machine.states, { id, name: `State ${machine.states.length + 1}`, poseNodeId: source.id, resetOnEntry: true }],
      } : machine),
    }));
  };

  const addTransition = () => {
    if (!selectedMachine || selectedMachine.states.length < 2) {
      setError("A transition needs at least two states.");
      return;
    }
    const transition: AnimationGraphTransition = {
      id: animationGraphLocalId("transition"),
      fromStateId: selectedMachine.states[0].id,
      toStateId: selectedMachine.states[1].id,
      priority: 0,
      durationTick: 6_000,
      curve: "linear",
      interruption: "none",
      conditions: [],
      exitTime: null,
    };
    mutateDraft((graph) => ({ ...graph, stateMachines: graph.stateMachines.map((machine) => machine.id === selectedMachine.id ? { ...machine, transitions: [...machine.transitions, transition] } : machine) }));
    setSelectedTransitionId(transition.id);
  };

  const addCondition = () => {
    if (!selectedTransition || !draft?.parameters[0]) {
      setError("Add a typed parameter before creating a transition condition.");
      return;
    }
    const parameter = draft.parameters[0];
    const condition: AnimationGraphCondition = {
      id: animationGraphLocalId("condition"),
      parameterId: parameter.id,
      operator: parameter.kind === "boolean" ? "is_true" : parameter.kind === "trigger" ? "triggered" : "greater",
      value: parameter.kind === "boolean" || parameter.kind === "trigger" ? null : parameter.defaultValue,
    };
    updateTransition({ conditions: [...selectedTransition.conditions, condition] });
  };

  const addParameter = () => {
    const name = newParameterName.trim() || `Parameter ${(draft?.parameters.length ?? 0) + 1}`;
    if (!draft || draft.parameters.some((parameter) => parameter.name.toLocaleLowerCase() === name.toLocaleLowerCase())) {
      setError("Parameter names must be unique in this graph.");
      return;
    }
    const parameter = createAnimationGraphParameter(name, newParameterKind);
    persistDraft({ ...cloneAnimationGraph(draft), parameters: [...draft.parameters, parameter] });
    setNewParameterName("");
  };

  const updateParameter = (parameterId: string, patch: Partial<AnimationGraphParameter>) => {
    mutateDraft((graph) => ({ ...graph, parameters: graph.parameters.map((parameter) => parameter.id === parameterId ? { ...parameter, ...patch } : parameter) }));
  };

  const removeParameter = (parameterId: string) => {
    mutateDraft((graph) => ({
      ...graph,
      parameters: graph.parameters.filter((parameter) => parameter.id !== parameterId),
      nodes: graph.nodes.map((node) => ({ ...node, parameterIds: node.parameterIds.filter((id) => id !== parameterId) })),
      stateMachines: graph.stateMachines.map((machine) => ({ ...machine, transitions: machine.transitions.map((transition) => ({ ...transition, conditions: transition.conditions.filter((condition) => condition.parameterId !== parameterId) })) })),
    }));
    const nextPreview = { ...previewValues };
    delete nextPreview[parameterId];
    setPreviewValues(nextPreview);
  };

  const setPreviewParameter = async (parameter: AnimationGraphParameter, value: AnimationGraphValue) => {
    if (!draft) return;
    const next = { ...previewValues, [parameter.id]: parameter.kind === "trigger" ? false : value };
    setPreviewValues(next);
    try {
      const result = await client.animationGraphSetPreviewParameters(draft.id, { [parameter.id]: value });
      if (!result.ok) setError(result.message);
      else setLiveMessage(`${parameter.name} is temporarily ${parameterValueLabel(value)}. Authored data is unchanged.`);
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "Parameter preview failed.");
    }
  };

  const clearPreview = async () => {
    if (!draft) return;
    try {
      const result = await client.animationGraphClearPreviewParameters(draft.id);
      if (!result.ok) setError(result.message);
      else {
        setPreviewValues({});
        setLiveMessage(result.message);
      }
    } catch (cause: unknown) {
      setError(cause instanceof Error ? cause.message : "Parameter preview reset failed.");
    }
  };

  const navigateDiagnostic = (diagnostic: AnimationGraphDiagnostic) => {
    animationEditorStore.getState().setGraphSelection(workspaceKey, diagnostic.nodeId ? [diagnostic.nodeId] : [], diagnostic.edgeId ? [diagnostic.edgeId] : []);
    if (diagnostic.nodeId) void flowRef.current?.fitView({ nodes: [{ id: diagnostic.nodeId }], duration: 180, padding: 0.7 });
  };

  const onKeyDownCapture = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const target = event.target as Element;
    const editable = Boolean(target.closest("input, select, textarea, button, [contenteditable='true']"));
    if (editable) {
      event.stopPropagation();
      return;
    }
    if (event.key === "/") {
      event.preventDefault();
      event.stopPropagation();
      if (paletteSearchRef.current) paletteSearchRef.current.focus();
      else {
        focusPaletteSearchOnOpen.current = true;
        animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, paletteOpen: true }));
      }
    } else if (event.key.toLocaleLowerCase() === "f") {
      event.preventDefault();
      event.stopPropagation();
      void flowRef.current?.fitView({ duration: 180, padding: 0.18 });
    } else if (event.key === "Delete" || event.key === "Backspace") {
      event.preventDefault();
      event.stopPropagation();
      removeSelection();
    } else if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      dragPositions.current = {};
      animationEditorStore.getState().setGraphSelection(workspaceKey, [], []);
      setError(null);
    } else if (event.key.startsWith("Arrow") && view.selectedNodeIds.length > 0) {
      event.preventDefault();
      event.stopPropagation();
      const amount = event.shiftKey ? 10 : 1;
      const dx = event.key === "ArrowLeft" ? -amount : event.key === "ArrowRight" ? amount : 0;
      const dy = event.key === "ArrowUp" ? -amount : event.key === "ArrowDown" ? amount : 0;
      mutateDraft((graph) => ({ ...graph, nodes: graph.nodes.map((node) => view.selectedNodeIds.includes(node.id) ? { ...node, position: { x: node.position.x + dx, y: node.position.y + dy } } : node) }));
    } else if (event.key === " " || event.key.startsWith("Arrow") || ["w", "e", "r"].includes(event.key.toLocaleLowerCase())) {
      // Graph focus owns editing/navigation keys; never let them reach viewport/timeline shortcuts.
      event.stopPropagation();
    }
  };

  if (loading && !draft) {
    return <div className="animation-graph-loading" role="status">Loading the sequence graph…</div>;
  }

  return (
    <div className="animation-graph-editor" data-testid="animation-graph-editor" data-palette-open={view.paletteOpen} data-inspector-open={view.inspectorOpen} onKeyDown={onKeyDownCapture}>
      <header className="animation-graph-toolbar">
        <div className="animation-graph-toolbar-identity">
          <strong className="animation-graph-toolbar-name">{draft?.name ?? "Animation graph"}</strong>
          <span className="animation-graph-toolbar-meta" style={{ marginLeft: space.sm, color: color.text.muted, fontSize: fontSize.micro }}>sequence {sequenceId} · rev {state.revision}</span>
        </div>
        <span className="animation-graph-toolbar-status">
          <Badge tone={compileTone(state.compile.state)}>{state.compile.state}</Badge>
          {dirty && <Badge tone="warn">unapplied draft</Badge>}
          {(state.compile.state === "invalid" || state.compile.state === "stale") && state.compile.lastGoodRevision && (
            <span className="animation-graph-stale">Draft invalid · previewing revision {state.compile.lastGoodRevision}</span>
          )}
        </span>
        <span className="animation-graph-toolbar-spacer" />
        <span className="animation-graph-toolbar-view-actions">
          <Button compact variant="ghost" active={view.showWeights} aria-pressed={view.showWeights} onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, showWeights: !current.showWeights }))}>Weights</Button>
          {hasMeasuredCost && <Button compact variant="ghost" active={view.showCosts} aria-pressed={view.showCosts} onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, showCosts: !current.showCosts }))}>Costs</Button>}
        </span>
        <span className="animation-graph-toolbar-actions" data-testid="animation-graph-toolbar-actions">
          <Button compact variant="ghost" active={view.paletteOpen} aria-pressed={view.paletteOpen} aria-expanded={view.paletteOpen} aria-controls="animation-graph-palette" onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, paletteOpen: !current.paletteOpen }))}>Palette</Button>
          <Button compact variant="ghost" active={view.inspectorOpen} aria-pressed={view.inspectorOpen} aria-expanded={view.inspectorOpen} aria-controls="animation-graph-inspector" onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, inspectorOpen: !current.inspectorOpen }))}>Inspector</Button>
          {(dirty || saving) && <>
            <Button compact variant="ghost" disabled={saving} onClick={discard}>Discard</Button>
            <Button compact data-testid="animation-graph-apply" disabled={!draft || !dirty || saving || !schemaSupported || Boolean(conflictRevision)} onClick={() => void apply()}>{saving ? "Applying…" : "Apply"}</Button>
          </>}
          {state.graph && <Button compact variant="ghost" aria-label="Delete graph" disabled={saving} onClick={() => void deleteGraph()}><span>Delete<span className="animation-graph-toolbar-delete-suffix"> graph</span></span></Button>}
        </span>
      </header>

      {conflictRevision && (
        <div className="animation-graph-conflict" role="alert" data-testid="animation-graph-conflict">
          <strong>Newer graph revision detected.</strong>
          <span>Apply is locked so this draft cannot overwrite collaborator edits. Rebase safely merges non-overlapping stable-ID records; Discard shows the latest native revision.</span>
          <Button compact onClick={() => void rebase()} disabled={saving}>Rebase local changes</Button>
          <Button compact variant="ghost" onClick={discard} disabled={saving}>Discard local draft</Button>
        </div>
      )}

      {!draft ? (
        <div className="animation-graph-empty">
          <EmptyPanelState
            icon="◇"
            title="No graph is authored for this sequence"
            description="Create an explicit editable graph. Nothing is persisted until Apply succeeds."
            primaryAction={<Button data-testid="animation-graph-locomotion-preset" disabled={!schemaSupported} onClick={() => authorPreset(createLocomotionGraphPreset(sequenceId))}>Locomotion preset</Button>}
            secondaryAction={<Button variant="ghost" disabled={!schemaSupported} onClick={() => authorPreset(createEmptyAnimationGraph(sequenceId))}>Empty graph</Button>}
          />
        </div>
      ) : (
        <>
          {view.paletteOpen && <aside
            id="animation-graph-palette"
            className="animation-graph-palette"
            aria-label="Animation graph node palette"
            onWheel={(event) => event.stopPropagation()}
          >
            <label className="animation-graph-field">
              Search nodes
              <input
                ref={paletteSearchRef}
                className="mtk-input"
                type="search"
                value={view.search}
                onChange={(event) => animationEditorStore.getState().setGraphSearch(workspaceKey, event.target.value)}
                placeholder="Press / to focus"
              />
            </label>
            <div className="animation-graph-palette-list" role="list" aria-label="Animation graph sources">
              <strong>Sources</strong>
              {state.sources.map((source) => (
                <div key={source.id} role="listitem">
                  <button type="button" disabled={source.readiness === "blocked"} title={`${source.reason}${source.action ? ` Next: ${source.action}` : ""}`} onClick={() => addSourceNode(source)}>
                    <span>{source.name}</span>
                    <small>{source.kind.replaceAll("_", " ")} · {source.readiness} · {source.reason}</small>
                  </button>
                </div>
              ))}
            </div>
            <div role="list" aria-label="Supported graph nodes" className="animation-graph-palette-list">
              <strong>Operators</strong>
              {NODE_LABELS.filter((item) => !view.search || `${item.label} ${item.detail}`.toLocaleLowerCase().includes(view.search.toLocaleLowerCase())).map((item) => {
                const duplicateOutput = item.kind === "output" && draft.nodes.some((node) => node.kind === "output");
                return (
                  <div key={item.kind} role="listitem">
                    <button type="button" disabled={duplicateOutput} title={duplicateOutput ? "This graph already has an Output node." : item.detail} onClick={() => addNode(item.kind)}>
                      <span>{item.label}</span><small>{duplicateOutput ? "Already present" : item.detail}</small>
                    </button>
                  </div>
                );
              })}
            </div>
            <div className="animation-graph-future" aria-label="Future graph nodes">
              <strong>Later gates</strong>
              {FUTURE_NODES.map(([label, reason]) => <button key={label} type="button" disabled title={reason}>{label}<small>{reason}</small></button>)}
            </div>
            <details open={view.listAlternativeOpen} onToggle={(event) => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, listAlternativeOpen: event.currentTarget.open }))}>
              <summary>Keyboard/list view</summary>
              <ol className="animation-graph-accessible-list" aria-label="Animation graph nodes in document order">
                {draft.nodes.map((node) => (
                  <li key={node.id}>
                    <button
                      type="button"
                      aria-pressed={view.selectedNodeIds.includes(node.id)}
                      onClick={() => animationEditorStore.getState().setGraphSelection(workspaceKey, [node.id], [])}
                      onKeyDown={(event) => {
                        if (!event.key.startsWith("Arrow")) return;
                        event.preventDefault();
                        event.stopPropagation();
                        const amount = event.shiftKey ? 10 : 1;
                        const dx = event.key === "ArrowLeft" ? -amount : event.key === "ArrowRight" ? amount : 0;
                        const dy = event.key === "ArrowUp" ? -amount : event.key === "ArrowDown" ? amount : 0;
                        mutateDraft((graph) => ({ ...graph, nodes: graph.nodes.map((item) => item.id === node.id ? { ...item, position: { x: item.position.x + dx, y: item.position.y + dy } } : item) }));
                      }}
                    >{node.name} · {nodeKindLabel(node.kind)}</button>
                  </li>
                ))}
              </ol>
              <div className="animation-graph-list-connections" aria-label="Keyboard connection editor">
                <strong>Create connection</strong>
                <label>Source node
                  <select aria-label="Connection source node" value={listSourceNodeId} onChange={(event) => {
                    setListSourceNodeId(event.target.value);
                    const node = draft.nodes.find((candidate) => candidate.id === event.target.value);
                    setListSourcePortId(node ? animationGraphPorts(node.kind).find((port) => port.direction === "output" && port.kind === "pose")?.id ?? "pose" : "pose");
                  }}><option value="">Choose source</option>{draft.nodes.filter((node) => animationGraphPorts(node.kind).some((port) => port.direction === "output" && port.kind === "pose")).map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}</select>
                </label>
                <label>Source port
                  <select aria-label="Connection source port" value={listSourcePortId} onChange={(event) => setListSourcePortId(event.target.value)}>{draft.nodes.find((node) => node.id === listSourceNodeId) ? animationGraphPorts(draft.nodes.find((node) => node.id === listSourceNodeId)!.kind).filter((port) => port.direction === "output" && port.kind === "pose").map((port) => <option key={port.id} value={port.id}>{port.label}</option>) : <option value="pose">Pose</option>}</select>
                </label>
                <label>Target node
                  <select aria-label="Connection target node" value={listTargetNodeId} onChange={(event) => {
                    setListTargetNodeId(event.target.value);
                    const node = draft.nodes.find((candidate) => candidate.id === event.target.value);
                    setListTargetPortId(node ? animationGraphPorts(node.kind).find((port) => port.direction === "input" && port.kind === "pose")?.id ?? "pose" : "pose");
                  }}><option value="">Choose target</option>{draft.nodes.filter((node) => animationGraphPorts(node.kind).some((port) => port.direction === "input" && port.kind === "pose")).map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}</select>
                </label>
                <label>Target port
                  <select aria-label="Connection target port" value={listTargetPortId} onChange={(event) => setListTargetPortId(event.target.value)}>{draft.nodes.find((node) => node.id === listTargetNodeId) ? animationGraphPorts(draft.nodes.find((node) => node.id === listTargetNodeId)!.kind).filter((port) => port.direction === "input" && port.kind === "pose").map((port) => <option key={port.id} value={port.id}>{port.label}</option>) : <option value="pose">Pose</option>}</select>
                </label>
                <Button compact disabled={!listSourceNodeId || !listTargetNodeId} onClick={() => connect({ source: listSourceNodeId, sourceHandle: listSourcePortId, target: listTargetNodeId, targetHandle: listTargetPortId })}>Add connection</Button>
                <strong>Connections</strong>
                <ul aria-label="Animation graph connections">
                  {draft.edges.map((edge) => <li key={edge.id}>
                    <select aria-label={`Source node for ${edge.id}`} value={edge.fromNodeId} onChange={(event) => editConnection(edge.id, { fromNodeId: event.target.value })}>{draft.nodes.filter((node) => animationGraphPorts(node.kind).some((port) => port.direction === "output" && port.kind === "pose")).map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}</select>
                    <select aria-label={`Source port for ${edge.id}`} value={edge.fromPortId} onChange={(event) => editConnection(edge.id, { fromPortId: event.target.value })}>{animationGraphPorts(draft.nodes.find((node) => node.id === edge.fromNodeId)?.kind ?? "reference_pose").filter((port) => port.direction === "output" && port.kind === "pose").map((port) => <option key={port.id} value={port.id}>{port.label}</option>)}</select>
                    <span aria-hidden="true">→</span>
                    <select aria-label={`Target node for ${edge.id}`} value={edge.toNodeId} onChange={(event) => editConnection(edge.id, { toNodeId: event.target.value, toPortId: animationGraphPorts(draft.nodes.find((node) => node.id === event.target.value)?.kind ?? "output").find((port) => port.direction === "input" && port.kind === "pose")?.id ?? "pose" })}>{draft.nodes.filter((node) => animationGraphPorts(node.kind).some((port) => port.direction === "input" && port.kind === "pose")).map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}</select>
                    <select aria-label={`Target port for ${edge.id}`} value={edge.toPortId} onChange={(event) => editConnection(edge.id, { toPortId: event.target.value })}>{animationGraphPorts(draft.nodes.find((node) => node.id === edge.toNodeId)?.kind ?? "output").filter((port) => port.direction === "input" && port.kind === "pose").map((port) => <option key={port.id} value={port.id}>{port.label}</option>)}</select>
                    <Button compact icon variant="ghost" aria-label={`Delete connection ${edge.id}`} onClick={() => removeConnectionById(edge.id)}>×</Button>
                  </li>)}
                </ul>
              </div>
            </details>
          </aside>}

          <main className="animation-graph-canvas" aria-label="Animation graph canvas">
            <ReactFlow<AnimationFlowNode, FlowEdge>
              nodes={nodes}
              edges={edges}
              nodeTypes={NODE_TYPES}
              onInit={(instance) => {
                flowRef.current = instance;
                if (fitOnNextGraphRender.current) {
                  fitOnNextGraphRender.current = false;
                  window.requestAnimationFrame(() => void instance.fitView({ duration: 180, padding: 0.18 }));
                }
              }}
              onConnect={connect}
              onNodeDrag={(_event, node) => {
                dragPositions.current[node.id] = { x: node.position.x, y: node.position.y };
              }}
              onNodeDragStop={onNodeDragStop}
              onSelectionChange={onSelectionChange}
              onPaneClick={() => animationEditorStore.getState().setGraphSelection(workspaceKey, [], [])}
              onMoveEnd={onMoveEnd}
              defaultViewport={view.viewport}
              fitView={false}
              minZoom={0.1}
              maxZoom={4}
              deleteKeyCode={null}
              multiSelectionKeyCode={["Meta", "Control"]}
              selectionKeyCode="Shift"
              proOptions={{ hideAttribution: true }}
              aria-label="Editable animation graph. Press F to fit, slash to search, or open the keyboard list."
            >
              <Background color={graphTheme.grid} gap={18} />
              <Controls showInteractive={false} />
              {nodes.length > 12 && <MiniMap pannable zoomable aria-label="Animation graph minimap" />}
            </ReactFlow>
            {debug && (
              <div className="animation-graph-runtime-summary" data-testid="animation-graph-runtime-trace">
                {debug.activeNodes.length} active · {debug.evaluationCostMicros ?? "—"} μs · tick {debug.localTick}
                {debug.transition && <span> · transition {Math.round(debug.transition.progress * 100)}%</span>}
                {(debug.eventsTruncated || debug.truncated) && <strong> · trace incomplete or safety-limited</strong>}
              </div>
            )}
          </main>

          {view.inspectorOpen && <aside id="animation-graph-inspector" className="animation-graph-inspector" aria-label="Animation graph inspector">
            <section>
              <h3>Selection</h3>
              {!selectedNode && !selectedEdge ? <p>Select a node, connection, or diagnostic.</p> : selectedEdge ? (
                <>
                  <div className="animation-graph-node-identity"><code>{selectedEdge.id}</code><span>{draft.nodes.find((node) => node.id === selectedEdge.fromNodeId)?.name ?? selectedEdge.fromNodeId} → {draft.nodes.find((node) => node.id === selectedEdge.toNodeId)?.name ?? selectedEdge.toNodeId}</span></div>
                  <label className="animation-graph-check"><input type="checkbox" checked={selectedEdge.enabled} onChange={(event) => updateSelectedEdge({ enabled: event.target.checked })} /> Enabled</label>
                  {selectedEdgeWeightSupport?.explicit && <label className="animation-graph-field">Explicit weight
                    <input className="mtk-input" type="number" min={0} step={0.01} value={selectedEdge.weight ?? ""} placeholder="destination policy" onChange={(event) => updateSelectedEdge({ weight: event.target.value === "" ? null : Math.max(0, Number(event.target.value)), weightParameterId: null })} />
                  </label>}
                  {selectedEdgeWeightSupport?.parameter && <label className="animation-graph-field">Weight parameter
                    <select value={selectedEdge.weightParameterId ?? ""} onChange={(event) => updateSelectedEdge({ weightParameterId: event.target.value || null, weight: null })}>
                      <option value="">Use explicit/default weight</option>
                      {draft.parameters.filter((parameter) => parameter.kind === "float").map((parameter) => <option key={parameter.id} value={parameter.id}>{parameter.name}</option>)}
                    </select>
                    <small>The stable connection ID retains this binding when inputs are reordered.</small>
                  </label>}
                  {selectedEdgeWeightSupport && <small>{selectedEdgeWeightSupport.reason}</small>}
                  {selectedEdgeWeightSupport && !selectedEdgeWeightSupport.explicit && !selectedEdgeWeightSupport.parameter
                    && (selectedEdge.weight !== null || selectedEdge.weightParameterId !== null)
                    && <Button compact variant="ghost" onClick={() => updateSelectedEdge({ weight: null, weightParameterId: null })}>Clear unsupported weight data</Button>}
                  {selectedEdgeWeightSupport && !selectedEdgeWeightSupport.explicit
                    && selectedEdge.toPortId === "layer"
                    && (selectedEdgeTarget?.kind === "layer_override" || selectedEdgeTarget?.kind === "layer_additive")
                    && selectedEdgeTarget.parameterIds.length > 0
                    && <Button compact variant="ghost" onClick={() => updateSelectedEdge({ weight: 1, weightParameterId: null })}>Use edge constant instead</Button>}
                  <Button compact variant="danger" onClick={removeSelection}>Remove connection</Button>
                </>
              ) : selectedNode ? (
                <>
                  <label className="animation-graph-field">Name<input className="mtk-input" value={selectedNode.name} onChange={(event) => updateSelectedNode({ name: event.target.value })} /></label>
                  <label className="animation-graph-check"><input type="checkbox" checked={selectedNode.enabled} onChange={(event) => updateSelectedNode({ enabled: event.target.checked })} /> Enabled</label>
                  <div className="animation-graph-node-identity"><code>{selectedNode.id}</code><span>{nodeKindLabel(selectedNode.kind)}</span></div>
                  {selectedNode.kind === "sequence" && (
                    <label className="animation-graph-field">Source
                      <select value={selectedNode.sourceId ?? ""} onChange={(event) => updateSelectedNode({ sourceId: event.target.value || null })}>
                        <option value="">Choose a ready source</option>
                        {state.sources.filter((source) => source.kind !== "reference_pose").map((source) => <option key={source.id} value={source.id} disabled={source.readiness === "blocked"}>{source.name}{source.readiness === "blocked" ? " — unavailable" : ""}</option>)}
                      </select>
                      {state.sources.find((source) => source.id === selectedNode.sourceId)?.reason && <small>{state.sources.find((source) => source.id === selectedNode.sourceId)?.reason}</small>}
                    </label>
                  )}
                  {(["blend_1d", "blend_2d_cartesian", "layer_override", "layer_additive"] as AnimationGraphNodeKind[]).includes(selectedNode.kind) && (
                    <label className="animation-graph-field">Parameter
                      <select value={selectedNode.parameterIds[0] ?? ""} onChange={(event) => updateSelectedNodeParameter(event.target.value)}>
                        <option value="">No parameter</option>
                        {draft.parameters.filter((parameter) => animationGraphCompatibleParameterKinds(selectedNode.kind).includes(parameter.kind)).map((parameter) => <option key={parameter.id} value={parameter.id}>{parameter.name} · {parameter.kind}</option>)}
                      </select>
                      {(selectedNode.kind === "layer_override" || selectedNode.kind === "layer_additive") && <small>A node-level layer parameter owns the runtime weight. Clear any constant on the layer input edge; both cannot be authored together.</small>}
                    </label>
                  )}
                  {(selectedNode.kind === "blend_1d" || selectedNode.kind === "blend_2d_cartesian") && (
                    <>
                      <div className="animation-graph-field" role="group" aria-label="Stable blend samples">
                        <strong>Keyed samples</strong>
                        {selectedNode.samples.map((sample) => {
                          const edge = draft.edges.find((candidate) => candidate.id === sample.edgeId);
                          const source = draft.nodes.find((candidate) => candidate.id === edge?.fromNodeId);
                          return <div key={sample.id} className="animation-graph-inline">
                            <code title={`${sample.id} · ${sample.edgeId}`}>{source?.name ?? sample.edgeId}</code>
                            <input className="mtk-input" aria-label={`${source?.name ?? sample.id} sample X`} type="number" value={sample.position[0]} onChange={(event) => updateSelectedNode({ samples: selectedNode.samples.map((candidate) => candidate.id === sample.id ? { ...candidate, position: [Number(event.target.value), candidate.position[1]] } : candidate) })} />
                            {selectedNode.kind === "blend_2d_cartesian" && <input className="mtk-input" aria-label={`${source?.name ?? sample.id} sample Y`} type="number" value={sample.position[1]} onChange={(event) => updateSelectedNode({ samples: selectedNode.samples.map((candidate) => candidate.id === sample.id ? { ...candidate, position: [candidate.position[0], Number(event.target.value)] } : candidate) })} />}
                          </div>;
                        })}
                        <small>Each coordinate is attached to a durable sample and connection ID, never list position.</small>
                      </div>
                      {selectedNode.kind === "blend_2d_cartesian" && <label className="animation-graph-field">Authored triangles
                        <textarea className="mtk-input" value={selectedNode.triangles.map((triangle) => triangle.join(", ")).join("\n")} onChange={(event) => updateSelectedNode({ triangles: event.target.value.split(/\r?\n/).map((line) => line.split(",").map((id) => id.trim()).filter(Boolean)).filter((ids) => ids.length === 3).map((ids) => [ids[0], ids[1], ids[2]] as const) })} placeholder="sample-left, sample-right, sample-forward" />
                        <small>Three distinct connected sample IDs per triangle. Triangulation is compiled, never rebuilt per frame.</small>
                      </label>}
                    </>
                  )}
                  {(selectedNode.kind === "layer_override" || selectedNode.kind === "layer_additive") && (
                    <label className="animation-graph-field">Mask bindings
                      <textarea className="mtk-input" value={selectedNode.maskBindings.join("\n")} placeholder="**/Transform/rotation" onChange={(event) => updateSelectedNode({ maskBindings: event.target.value.split(/\r?\n/).map((item) => item.trim()).filter(Boolean) })} />
                      <small>Full slash paths are target/component/property[/subpath]. <code>*</code> matches one complete segment; <code>**</code> matches any depth. Example: <code>**/Transform/rotation</code>.</small>
                      {selectedNode.maskBindings.map((selector) => animationGraphMaskSelectorError(selector) && <span key={selector} role="alert">{selector}: {animationGraphMaskSelectorError(selector)}</span>)}
                    </label>
                  )}
                  {selectedNode.kind === "blend_2d_cartesian" && <p>{selectedNode.triangles.length} authored triangles. Native compilation validates stable point membership and hull coverage.</p>}
                </>
              ) : null}
            </section>

            {selectedMachine && (
              <section data-testid="animation-graph-state-inspector">
                <h3>States and transitions</h3>
                <div className="animation-graph-inline"><strong>{selectedMachine.name}</strong><Button compact variant="ghost" onClick={addState}>+ State</Button><Button compact variant="ghost" onClick={addTransition}>+ Transition</Button></div>
                <div className="animation-graph-state-list">{selectedMachine.states.map((graphState) => (
                  <div key={graphState.id} className="animation-graph-state-row">
                    <label title="Entry state"><input type="radio" name={`entry-${selectedMachine.id}`} checked={selectedMachine.entryStateId === graphState.id} onChange={() => mutateDraft((graph) => ({ ...graph, stateMachines: graph.stateMachines.map((machine) => machine.id === selectedMachine.id ? { ...machine, entryStateId: graphState.id } : machine) }))} /> entry</label>
                    <input className="mtk-input" aria-label={`State name ${graphState.name}`} value={graphState.name} onChange={(event) => updateMachineState(graphState.id, { name: event.target.value })} />
                    <select aria-label={`Pose for ${graphState.name}`} value={graphState.poseNodeId} onChange={(event) => updateMachineState(graphState.id, { poseNodeId: event.target.value })}>
                      {draft.nodes.filter((node) => ["reference_pose", "sequence", "blend_normalized", "blend_direct", "blend_1d", "blend_2d_cartesian", "layer_override", "layer_additive"].includes(node.kind)).map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}
                    </select>
                    <label title="Restart local source time on entry"><input type="checkbox" checked={graphState.resetOnEntry} onChange={(event) => updateMachineState(graphState.id, { resetOnEntry: event.target.checked })} /> reset</label>
                    <Button compact icon variant="ghost" aria-label={`Remove state ${graphState.name}`} onClick={() => removeMachineState(graphState.id)}>×</Button>
                  </div>
                ))}</div>
                {selectedMachine.transitions.length > 0 && (
                  <label className="animation-graph-field">Transition
                    <select value={selectedTransition?.id ?? ""} onChange={(event) => setSelectedTransitionId(event.target.value)}>{selectedMachine.transitions.map((transition) => <option key={transition.id} value={transition.id}>{selectedMachine.states.find((item) => item.id === transition.fromStateId)?.name} → {selectedMachine.states.find((item) => item.id === transition.toStateId)?.name}</option>)}</select>
                  </label>
                )}
                {selectedTransition && (
                  <div className="animation-graph-transition-grid">
                    <label className="animation-graph-field">From<select value={selectedTransition.fromStateId} onChange={(event) => updateTransition({ fromStateId: event.target.value })}>{selectedMachine.states.map((graphState) => <option key={graphState.id} value={graphState.id}>{graphState.name}</option>)}</select></label>
                    <label className="animation-graph-field">To<select value={selectedTransition.toStateId} onChange={(event) => updateTransition({ toStateId: event.target.value })}>{selectedMachine.states.map((graphState) => <option key={graphState.id} value={graphState.id}>{graphState.name}</option>)}</select></label>
                    <label className="animation-graph-field">Priority<NumericField ariaLabel="Transition priority" integer value={selectedTransition.priority} onCommit={(value) => updateTransition({ priority: Math.trunc(value) })} /></label>
                    <label className="animation-graph-field">Duration (ticks)<NumericField ariaLabel="Transition duration in ticks" integer value={selectedTransition.durationTick} min={0} onCommit={(value) => updateTransition({ durationTick: Math.max(0, Math.trunc(value)) })} /></label>
                    <label className="animation-graph-field">Curve<select value={selectedTransition.curve} onChange={(event) => updateTransition({ curve: event.target.value as AnimationGraphTransition["curve"] })}><option value="linear">Linear</option><option value="ease_in">Ease in</option><option value="ease_out">Ease out</option><option value="ease_in_out">Ease in/out</option><option value="smoothstep">Smoothstep</option></select></label>
                    <label className="animation-graph-field">Interruption<select value={selectedTransition.interruption} onChange={(event) => updateTransition({ interruption: event.target.value as AnimationGraphTransition["interruption"] })}><option value="none">None</option><option value="source">Source</option><option value="destination">Destination</option><option value="both">Both</option></select></label>
                    <label className="animation-graph-field">Exit time · future runtime gate<input className="mtk-input" type="number" min={0} max={1} step={0.01} value={selectedTransition.exitTime ?? ""} placeholder="condition only" onChange={(event) => updateTransition({ exitTime: event.target.value === "" ? null : Math.max(0, Math.min(1, Number(event.target.value))) })} /><small>Persisted for forward compatibility. Native schema v2 reports a compile diagnostic and keeps the last-good preview when exit time is set.</small></label>
                    <div><Button compact variant="ghost" onClick={addCondition}>+ Condition</Button></div>
                    {selectedTransition.conditions.map((condition) => {
                      const conditionParameter = draft.parameters.find((parameter) => parameter.id === condition.parameterId);
                      const booleanLike = conditionParameter?.kind === "boolean" || conditionParameter?.kind === "trigger";
                      return (
                        <div key={condition.id} className="animation-graph-condition">
                          <select aria-label="Condition parameter" value={condition.parameterId} onChange={(event) => { const parameter = draft.parameters.find((item) => item.id === event.target.value); if (parameter) updateCondition(condition.id, { parameterId: parameter.id, operator: parameter.kind === "boolean" ? "is_true" : parameter.kind === "trigger" ? "triggered" : "greater", value: parameter.kind === "boolean" || parameter.kind === "trigger" ? null : parameter.defaultValue }); }}>{draft.parameters.map((parameter) => <option key={parameter.id} value={parameter.id}>{parameter.name}</option>)}</select>
                          <select aria-label="Condition operator" value={condition.operator} onChange={(event) => updateCondition(condition.id, { operator: event.target.value as AnimationGraphCondition["operator"] })}>
                            {booleanLike ? <><option value="is_true">is true</option><option value="is_false">is false</option>{conditionParameter?.kind === "trigger" && <option value="triggered">triggered</option>}</> : <><option value="greater">greater</option><option value="greater_equal">greater/equal</option><option value="less">less</option><option value="less_equal">less/equal</option><option value="equal">equal</option><option value="not_equal">not equal</option></>}
                          </select>
                          {!booleanLike && <input className="mtk-input" aria-label="Condition value" type="number" value={typeof condition.value === "number" ? condition.value : 0} onChange={(event) => updateCondition(condition.id, { value: Number(event.target.value) })} />}
                          <Button compact icon variant="ghost" aria-label="Remove condition" onClick={() => removeCondition(condition.id)}>×</Button>
                        </div>
                      );
                    })}
                  </div>
                )}
              </section>
            )}

            <section aria-label="Authored graph parameters">
              <h3>Typed parameters</h3>
              {draft.parameters.length === 0 ? <p>Add a parameter from the preview strip below.</p> : draft.parameters.map((parameter) => (
                <div key={parameter.id} className="animation-graph-authored-parameter">
                  <input className="mtk-input" aria-label={`Parameter name ${parameter.name}`} value={parameter.name} onChange={(event) => updateParameter(parameter.id, { name: event.target.value })} />
                  <select aria-label={`Parameter type ${parameter.name}`} value={parameter.kind} onChange={(event) => { const kind = event.target.value as AnimationGraphParameter["kind"]; updateParameter(parameter.id, { kind, defaultValue: defaultAnimationGraphValue(kind), min: kind === "float" || kind === "integer" ? 0 : null, max: kind === "float" || kind === "integer" ? 1 : null }); }}><option value="float">Float</option><option value="integer">Integer</option><option value="boolean">Boolean</option><option value="trigger">Trigger</option><option value="vec2">Vector 2</option></select>
                  {parameter.kind === "trigger" ? (
                    <span>momentary · authored default is always false{parameter.defaultValue === true && <Button compact variant="ghost" onClick={() => updateParameter(parameter.id, { defaultValue: false })}>Reset invalid default</Button>}</span>
                  ) : parameter.kind === "boolean" ? (
                    <label><input type="checkbox" aria-label={`Default ${parameter.name}`} checked={parameter.defaultValue === true} onChange={(event) => updateParameter(parameter.id, { defaultValue: event.target.checked })} /> default</label>
                  ) : parameter.kind === "vec2" ? (
                    <span className="animation-graph-inline"><input className="mtk-input" aria-label={`Default ${parameter.name} X`} type="number" value={Array.isArray(parameter.defaultValue) ? parameter.defaultValue[0] : 0} onChange={(event) => updateParameter(parameter.id, { defaultValue: [Number(event.target.value), Array.isArray(parameter.defaultValue) ? parameter.defaultValue[1] : 0] })} /><input className="mtk-input" aria-label={`Default ${parameter.name} Y`} type="number" value={Array.isArray(parameter.defaultValue) ? parameter.defaultValue[1] : 0} onChange={(event) => updateParameter(parameter.id, { defaultValue: [Array.isArray(parameter.defaultValue) ? parameter.defaultValue[0] : 0, Number(event.target.value)] })} /></span>
                  ) : (
                    <span className="animation-graph-parameter-range"><input className="mtk-input" aria-label={`Default ${parameter.name}`} type="number" step={parameter.kind === "integer" ? 1 : 0.01} value={typeof parameter.defaultValue === "number" ? parameter.defaultValue : 0} onChange={(event) => updateParameter(parameter.id, { defaultValue: parameter.kind === "integer" ? Math.round(Number(event.target.value)) : Number(event.target.value) })} /><input className="mtk-input" aria-label={`Minimum ${parameter.name}`} type="number" value={parameter.min ?? ""} placeholder="min" onChange={(event) => updateParameter(parameter.id, { min: event.target.value === "" ? null : Number(event.target.value) })} /><input className="mtk-input" aria-label={`Maximum ${parameter.name}`} type="number" value={parameter.max ?? ""} placeholder="max" onChange={(event) => updateParameter(parameter.id, { max: event.target.value === "" ? null : Number(event.target.value) })} /></span>
                  )}
                  <Button compact icon variant="ghost" aria-label={`Remove parameter ${parameter.name}`} onClick={() => removeParameter(parameter.id)}>×</Button>
                </div>
              ))}
            </section>

            <section>
              <h3>Diagnostics</h3>
              {diagnostics.length === 0 ? <p>No graph diagnostics.</p> : <ul className="animation-graph-diagnostics">{diagnostics.map((diagnostic) => (
                <li key={diagnostic.id} data-severity={diagnostic.severity}>
                  <button type="button" onClick={() => navigateDiagnostic(diagnostic)}><strong>{diagnostic.code.replaceAll("_", " ")}</strong><span>{diagnostic.message}</span>{diagnostic.fix && <small>{diagnostic.fix}</small>}</button>
                </li>
              ))}</ul>}
            </section>

            <section>
              <h3>Runtime trace</h3>
              {!debug ? <p>{state.compile.state === "ready" ? "Waiting for a matching runtime instance…" : state.compile.message}</p> : (
                <>
                  <p>instance <code>{debug.instanceId}</code> · graph <code>{debug.compiledHash}</code></p>
                  <p>{debug.activeNodes.length} active nodes · {debug.activeEdges.length} active paths · {debug.evaluationCostMicros ?? "unmeasured"} μs</p>
                  {debug.transition && <progress aria-label="Active transition progress" max={1} value={debug.transition.progress} />}
                  {debug.watches.map((watch) => <div key={watch.id}><code>{watch.id}</code> {JSON.stringify(watch.value)} · {watch.source}</div>)}
                  {(debug.eventsTruncated || debug.truncated) && <p role="alert">Runtime trace is incomplete or safety-limited; events or ambiguous edge provenance may be omitted.</p>}
                </>
              )}
            </section>
          </aside>}

          <section className="animation-graph-parameters" aria-label="Transient animation graph parameters">
            <div className="animation-graph-preview-heading"><strong>Preview parameters</strong><small> transient · never saved</small></div>
            {draft.parameters.map((parameter) => (
              <ParameterPreview
                key={parameter.id}
                parameter={parameter}
                value={previewValues[parameter.id] ?? parameter.defaultValue}
                watched={view.watches.includes(parameter.id)}
                onWatch={(watched) => animationEditorStore.getState().setGraphWatches(workspaceKey, watched ? [...view.watches, parameter.id] : view.watches.filter((id) => id !== parameter.id))}
                onChange={(value) => void setPreviewParameter(parameter, value)}
              />
            ))}
            <input className="mtk-input" aria-label="New graph parameter name" placeholder="Parameter name" value={newParameterName} onChange={(event) => setNewParameterName(event.target.value)} />
            <select aria-label="New graph parameter type" value={newParameterKind} onChange={(event) => setNewParameterKind(event.target.value as AnimationGraphParameter["kind"])}><option value="float">Float</option><option value="integer">Integer</option><option value="boolean">Boolean</option><option value="trigger">Trigger</option><option value="vec2">Vector 2</option></select>
            <Button compact variant="ghost" onClick={addParameter}>+ Parameter</Button>
            <Button compact variant="ghost" disabled={Object.keys(previewValues).length === 0} onClick={() => void clearPreview()}>Reset preview</Button>
          </section>
        </>
      )}

      <div className="animation-graph-live" role="status" aria-live="polite">{error ?? liveMessage}</div>
      {error && <div className="animation-graph-error" role="alert">{error}</div>}
    </div>
  );
}

function ParameterPreview({
  parameter,
  value,
  watched,
  onWatch,
  onChange,
}: {
  parameter: AnimationGraphParameter;
  value: AnimationGraphValue;
  watched: boolean;
  onWatch: (watched: boolean) => void;
  onChange: (value: AnimationGraphValue) => void;
}) {
  let control;
  if (parameter.kind === "trigger") {
    control = <button type="button" className="animation-graph-preview-parameter" aria-label={`Fire ${parameter.name} trigger`} onClick={() => onChange(true)}>Fire {parameter.name}</button>;
  } else if (parameter.kind === "boolean") {
    control = <label className="animation-graph-preview-parameter"><input type="checkbox" checked={value === true} onChange={(event) => onChange(event.target.checked)} /> {parameter.name}</label>;
  } else if (parameter.kind === "vec2") {
    const tuple = Array.isArray(value) ? value : [0, 0];
    const change = (index: 0 | 1, event: ChangeEvent<HTMLInputElement>) => {
      const next: [number, number] = [tuple[0], tuple[1]];
      next[index] = Number(event.target.value);
      onChange(next);
    };
    control = <label className="animation-graph-preview-parameter">{parameter.name}<input aria-label={`${parameter.name} X`} type="number" value={tuple[0]} onChange={(event) => change(0, event)} /><input aria-label={`${parameter.name} Y`} type="number" value={tuple[1]} onChange={(event) => change(1, event)} /></label>;
  } else {
    control = <label className="animation-graph-preview-parameter">{parameter.name}<Slider min={parameter.min ?? 0} max={parameter.max ?? 1} step={parameter.kind === "integer" ? 1 : 0.01} value={typeof value === "number" ? value : 0} onChange={(event) => onChange(parameter.kind === "integer" ? Math.round(Number(event.target.value)) : Number(event.target.value))} /><output>{typeof value === "number" ? value.toFixed(parameter.kind === "integer" ? 0 : 2) : "0"}</output></label>;
  }
  return <span className="animation-graph-preview-wrap">{control}<button type="button" aria-pressed={watched} title={watched ? "Remove from bounded runtime watches" : "Add to bounded runtime watches"} onClick={() => onWatch(!watched)}>{watched ? "watching" : "watch"}</button></span>;
}
