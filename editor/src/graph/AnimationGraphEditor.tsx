//! First-class animation graph authoring and live inspection. The versioned graph draft is authoritative
//! for this editor surface; React Flow is a controlled visual projection. Structural operations remain
//! local until one explicit Apply sends one atomic, undoable native intent. Parameter previews are a
//! separate transient channel and never enter the document draft.

import {
  useCallback,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import {
  type Connection,
  type Edge as FlowEdge,
  type OnSelectionChangeParams,
  type ReactFlowInstance,
  type Viewport,
} from "@xyflow/react";
import { useStore } from "zustand";
import {
  animationEditorStore,
  animationWorkspaceView,
  type AnimationDraftValue,
  type AnimationWorkspaceKey,
} from "../store/animation";
import { Icon } from "../theme/icons";
import {
  Badge,
  Button,
  NumericField,
  ReadOut,
  SearchField,
  SelectField,
  SliderField,
  TextField,
  TextAreaField,
  Toolbar,
  ToolbarGroup,
  ToolbarSeparator,
  ToolbarSpacer,
} from "../theme/primitives";
import { Callout, Checkbox, Field, FieldGrid, ProgressBar } from "../theme/fields";
import { ChoiceCard, ChoiceGrid, DisclosureSection, EmptyPanelState } from "../theme/workspace";
import { GraphSurface, graphEdge, type GraphCardNode } from "../theme/graph";
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

/** WHICH SHARED MARK EACH OPERATOR WEARS. The card is `theme/graph`'s, so the only thing this editor
 *  decides about a node's appearance is what KIND of thing it is — and that decision is a table, not
 *  a branch inside a renderer. */
const NODE_ICON: Record<AnimationGraphNodeKind, string> = {
  reference_pose: "character",
  sequence: "animate",
  blend_normalized: "meld",
  blend_direct: "meld",
  blend_1d: "meld",
  blend_2d_cartesian: "meld",
  layer_override: "group",
  layer_additive: "group",
  state_machine: "logic",
  output: "export",
};

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
  const flowRef = useRef<ReactFlowInstance<GraphCardNode, FlowEdge> | null>(null);
  const fitOnNextGraphRender = useRef(false);
  const paletteSearchRef = useRef<HTMLInputElement>(null);
  const focusPaletteSearchOnOpen = useRef(false);
  const loadEpoch = useRef(0);
  const compilePollEpoch = useRef(0);
  const currentStateRef = useRef(state);
  const draftKey = `graph-draft:${sequenceId}`;
  const fieldPrefix = useId();

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
          const next = { ...snapshot, activeNodes: snapshot.activeNodes.slice(0, 256), activeEdges: snapshot.activeEdges.slice(0, 512), watches: snapshot.watches.slice(0, 64), truncated: snapshot.truncated || bounded };
          // AN UNCHANGED SNAPSHOT IS NOT A CHANGE. This polls ten times a second, and it used to hand
          // React a fresh object every time — so the node and edge projections were rebuilt, React
          // Flow re-derived its whole edge set, and a graph sitting still with nothing happening in
          // it re-rendered 10x/s with its wires blinking in and out. Caught by a browser capture that
          // photographed the edges and an assertion, taken moments earlier, that counted none.
          setDebug((current) => (current && JSON.stringify(current) === JSON.stringify(next) ? current : next));
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

  /** THE SAME CARD EVERY OTHER GRAPH IN THE ENGINE DRAWS. This used to be a bespoke component — a
   *  bold name over a muted body, handles at `top: 34 + i * 18` — which is how the engine came to
   *  have two node anatomies, one of them the reference's (a small-caps role above the name) and one
   *  of them not. The domain's job here is only to say what the node IS, what it is doing, and where
   *  its ports are; `theme/graph` owns every pixel of the result. */
  const nodes = useMemo<GraphCardNode[]>(() => (draft?.nodes ?? []).map((node) => {
    const derived = presentation.get(node.id);
    const active = activeNodes.get(node.id);
    const readiness = derived?.readiness ?? "ready";
    const machine = node.stateMachineId ? draft?.stateMachines.find((item) => item.id === node.stateMachineId) : undefined;
    const stateName = active?.stateId
      ? machine?.states.find((item) => item.id === active.stateId)?.name ?? active.stateId
      : null;
    const meta: string[] = [];
    if (view.showWeights && active) meta.push(`weight ${active.weight.toFixed(3)}`);
    if (stateName) meta.push(`state ${stateName}`);
    if (view.showCosts && active?.costMicros != null) meta.push(`${active.costMicros} μs`);
    return {
      id: node.id,
      type: "mtkCard" as const,
      position: dragPositions.current[node.id] ?? node.position,
      selected: view.selectedNodeIds.includes(node.id),
      draggable: true,
      data: {
        title: node.name,
        eyebrow: nodeKindLabel(node.kind),
        meta: meta.length > 0 ? meta : undefined,
        kind: NODE_ICON[node.kind],
        emphasis: readiness === "blocked"
          ? "blocked" as const
          : readiness === "warning"
            ? "warning" as const
            : active
              ? "live" as const
              : "default" as const,
        ports: (derived?.ports.length ? derived.ports : animationGraphPorts(node.kind))
          .map((port) => ({ id: port.id, label: port.label, direction: port.direction })),
      },
      ariaLabel: `${node.name}, ${nodeKindLabel(node.kind)}, ${readiness}`,
      // The readiness sentence is the node's own explanation of its state; it belongs on the node,
      // not only in a diagnostics list one pane away.
      title: derived?.readinessReason ?? "Local draft; native readiness is checked on Apply.",
    };
  }), [activeNodes, draft?.nodes, draft?.stateMachines, presentation, view.selectedNodeIds, view.showCosts, view.showWeights]);

  /** `graphEdge` rather than a hand-assembled object, for the reason that helper exists: a wire built
   *  here would be free to forget the shared edge type, and React Flow's own label is a bare white
   *  rectangle where every other graph in the engine draws the design system's pill. */
  const edges = useMemo<FlowEdge[]>(() => (draft?.edges ?? []).map((edge) => {
    const active = activeEdges.get(edge.id);
    const selected = view.selectedEdgeIds.includes(edge.id);
    return {
      ...graphEdge({
        id: edge.id,
        source: edge.fromNodeId,
        sourceHandle: edge.fromPortId,
        target: edge.toNodeId,
        targetHandle: edge.toPortId,
        selected,
        label: view.showWeights && active ? active.weight.toFixed(3) : undefined,
        animated: Boolean(active && active.weight > 0),
        state: active ? "active" : !edge.enabled ? "disabled" : selected ? "selected" : "default",
      }),
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

  /** Every refusal in this editor, said in the user's words, in one place — so a control that goes
   *  grey always carries the sentence that says what would ungrey it (`<ux_quality>` 4 and 6). */
  const schemaRefusal = `This editor reads animation graph schema 1 or ${ANIMATION_GRAPH_SCHEMA_VERSION}; the document on disk uses another, and is left untouched.`;
  const applyRefusal = !schemaSupported
    ? schemaRefusal
    : conflictRevision
      ? "The saved graph advanced while this draft was open. Rebase or discard before applying."
      : saving
        ? "An Apply is already in flight."
        : "Nothing has changed in this draft yet.";

  /** Search filters the OPERATORS, which is what the field is beside. Sources are the graph's own
   *  inventory and stay whole — a search that silently emptied them would read as "there are none". */
  const paletteOperators = view.search
    ? NODE_LABELS.filter((item) => `${item.label} ${item.detail}`.toLocaleLowerCase().includes(view.search.toLocaleLowerCase()))
    : NODE_LABELS;

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

  const onSelectionChange = useCallback((selection: OnSelectionChangeParams<GraphCardNode, FlowEdge>) => {
    // React Flow emits an empty selection while reconciling controlled node arrays. Empty user intent is
    // handled explicitly by pane click/Escape so it cannot erase a keyboard/list selection asynchronously.
    if (selection.nodes.length === 0 && selection.edges.length === 0) return;
    animationEditorStore.getState().setGraphSelection(workspaceKey, selection.nodes.map((node) => node.id), selection.edges.map((edge) => edge.id));
  }, [workspaceKey]);

  const onNodeDragStop = (_event: MouseEvent | TouchEvent, node: GraphCardNode) => {
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


  const ids = {
    search: `${fieldPrefix}-search`,
    nodeName: `${fieldPrefix}-node-name`,
    nodeSource: `${fieldPrefix}-node-source`,
    nodeParameter: `${fieldPrefix}-node-parameter`,
    triangles: `${fieldPrefix}-triangles`,
    mask: `${fieldPrefix}-mask`,
    edgeWeight: `${fieldPrefix}-edge-weight`,
    edgeParameter: `${fieldPrefix}-edge-parameter`,
    entryState: `${fieldPrefix}-entry-state`,
    transition: `${fieldPrefix}-transition`,
    from: `${fieldPrefix}-from`,
    to: `${fieldPrefix}-to`,
    priority: `${fieldPrefix}-priority`,
    duration: `${fieldPrefix}-duration`,
    curve: `${fieldPrefix}-curve`,
    interruption: `${fieldPrefix}-interruption`,
    exitTime: `${fieldPrefix}-exit-time`,
    listSourceNode: `${fieldPrefix}-list-source-node`,
    listSourcePort: `${fieldPrefix}-list-source-port`,
    listTargetNode: `${fieldPrefix}-list-target-node`,
    listTargetPort: `${fieldPrefix}-list-target-port`,
    newParameterName: `${fieldPrefix}-new-parameter-name`,
    newParameterKind: `${fieldPrefix}-new-parameter-kind`,
  };

  if (loading && !draft) {
    return (
      <div className="animation-graph-editor" data-testid="animation-graph-editor">
        <EmptyPanelState
          icon={<Icon name="logic" size="xl" />}
          title="Loading the sequence graph…"
          description="Reading the authored graph and its compile state."
        />
      </div>
    );
  }

  const poseSourceNodes = draft?.nodes.filter((node) => animationGraphPorts(node.kind).some((port) => port.direction === "output" && port.kind === "pose")) ?? [];
  const poseTargetNodes = draft?.nodes.filter((node) => animationGraphPorts(node.kind).some((port) => port.direction === "input" && port.kind === "pose")) ?? [];
  const portOptions = (nodeId: string, direction: "input" | "output") => {
    const node = draft?.nodes.find((candidate) => candidate.id === nodeId);
    const ports = node ? animationGraphPorts(node.kind).filter((port) => port.direction === direction && port.kind === "pose") : [];
    return ports.length > 0 ? ports : [{ id: "pose", label: "Pose" }];
  };

  return (
    <div className="animation-graph-editor" data-testid="animation-graph-editor" data-palette-open={view.paletteOpen} data-inspector-open={view.inspectorOpen} onKeyDown={onKeyDownCapture}>
      <Toolbar className="animation-graph-toolbar" aria-label="Animation graph actions">
        {/* THE NAME, THEN THE STATE OF IT. What was here before put the graph's name, its sequence id
            and its revision in one 230px `overflow: hidden` box, so the identity of the thing being
            edited was cut mid-word with no scrollbar and no tooltip — caught by R3 on the first
            capture ever taken of this surface. A revision is a READING, so it is a read-out. */}
        <ToolbarGroup aria-label="Graph identity">
          <strong className="animation-graph-name" title={draft?.name ?? "Animation graph"}>{draft?.name ?? "Animation graph"}</strong>
          <ReadOut title={`Sequence ${sequenceId}, revision ${state.revision}`}>{state.revision}</ReadOut>
          <Badge tone={compileTone(state.compile.state)} title={state.compile.message}>{state.compile.state}</Badge>
          {dirty && <Badge tone="warn" title="This draft has changes that Apply has not committed.">unapplied draft</Badge>}
        </ToolbarGroup>
        <ToolbarSpacer />
        {(state.compile.state === "invalid" || state.compile.state === "stale") && state.compile.lastGoodRevision && (
          <ToolbarGroup aria-label="Preview revision">
            <Badge tone="warn" title={`The draft does not compile; the preview is still revision ${state.compile.lastGoodRevision}.`}>
              previewing {state.compile.lastGoodRevision}
            </Badge>
          </ToolbarGroup>
        )}
        <ToolbarGroup aria-label="Graph overlays">
          <Button compact variant="ghost" active={view.showWeights} aria-pressed={view.showWeights} title="Show each active node's runtime blend weight on its card" onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, showWeights: !current.showWeights }))}>Weights</Button>
          {hasMeasuredCost && <Button compact variant="ghost" active={view.showCosts} aria-pressed={view.showCosts} title="Show each active node's measured evaluation cost on its card" onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, showCosts: !current.showCosts }))}>Costs</Button>}
        </ToolbarGroup>
        <ToolbarSeparator />
        <ToolbarGroup aria-label="Graph panes and actions" data-testid="animation-graph-toolbar-actions">
          <Button compact variant="ghost" active={view.paletteOpen} aria-pressed={view.paletteOpen} aria-expanded={view.paletteOpen} aria-controls="animation-graph-palette" onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, paletteOpen: !current.paletteOpen }))}>Palette</Button>
          <Button compact variant="ghost" active={view.inspectorOpen} aria-pressed={view.inspectorOpen} aria-expanded={view.inspectorOpen} aria-controls="animation-graph-inspector" onClick={() => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, inspectorOpen: !current.inspectorOpen }))}>Inspector</Button>
          {(dirty || saving) && <>
            <Button compact variant="ghost" disabled={saving} disabledReason="An Apply is in flight; wait for it to finish." onClick={discard}>Discard</Button>
            <Button
              compact
              data-testid="animation-graph-apply"
              disabled={!draft || !dirty || saving || !schemaSupported || Boolean(conflictRevision)}
              disabledReason={applyRefusal}
              onClick={() => void apply()}
            >{saving ? "Applying…" : "Apply"}</Button>
          </>}
          {state.graph && <Button compact variant="ghost" aria-label="Delete graph" disabled={saving} disabledReason="An Apply is in flight; wait for it to finish." onClick={() => void deleteGraph()}>Delete</Button>}
        </ToolbarGroup>
      </Toolbar>

      {conflictRevision && (
        <Callout
          className="animation-graph-conflict"
          tone="warn"
          role="alert"
          data-testid="animation-graph-conflict"
          title="Newer graph revision detected"
        >
          <p>Apply is locked so this draft cannot overwrite collaborator edits. Rebase safely merges non-overlapping stable-ID records; Discard shows the latest native revision.</p>
          <div className="animation-graph-actions">
            <Button compact onClick={() => void rebase()} disabled={saving} disabledReason="An Apply is in flight; wait for it to finish.">Rebase local changes</Button>
            <Button compact variant="ghost" onClick={discard} disabled={saving} disabledReason="An Apply is in flight; wait for it to finish.">Discard local draft</Button>
          </div>
        </Callout>
      )}

      {!draft ? (
        <div className="animation-graph-empty">
          <EmptyPanelState
            icon={<Icon name="logic" size="xl" />}
            title="No graph is authored for this sequence"
            description="Create an explicit editable graph. Nothing is persisted until Apply succeeds."
            primaryAction={<Button data-testid="animation-graph-locomotion-preset" disabled={!schemaSupported} disabledReason={schemaRefusal} onClick={() => authorPreset(createLocomotionGraphPreset(sequenceId))}>Locomotion preset</Button>}
            secondaryAction={<Button variant="ghost" disabled={!schemaSupported} disabledReason={schemaRefusal} onClick={() => authorPreset(createEmptyAnimationGraph(sequenceId))}>Empty graph</Button>}
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
            <Field label="Search nodes" htmlFor={ids.search}>
              <SearchField
                id={ids.search}
                ref={paletteSearchRef}
                value={view.search}
                onChange={(event) => animationEditorStore.getState().setGraphSearch(workspaceKey, event.target.value)}
                placeholder="Press / to focus"
              />
            </Field>

            <DisclosureSection title="Sources" summary={`${state.sources.length}`} density="compact" defaultOpen>
              <ChoiceGrid label="Animation graph sources" minColumn={150}>
                {state.sources.map((source) => (
                  <ChoiceCard
                    key={source.id}
                    label={source.name}
                    description={`${source.kind.replaceAll("_", " ")} · ${source.readiness} · ${source.reason}`}
                    icon={<Icon name={source.kind === "reference_pose" ? "character" : "animate"} size="md" />}
                    disabled={source.readiness === "blocked"}
                    disabledReason={`${source.reason}${source.action ? ` Next: ${source.action}` : ""}`}
                    onSelect={() => addSourceNode(source)}
                  />
                ))}
              </ChoiceGrid>
            </DisclosureSection>

            <DisclosureSection title="Operators" summary={`${paletteOperators.length}`} density="compact" defaultOpen>
              <ChoiceGrid label="Supported graph nodes" minColumn={150}>
                {paletteOperators.map((item) => {
                  const duplicateOutput = item.kind === "output" && draft.nodes.some((node) => node.kind === "output");
                  return (
                    <ChoiceCard
                      key={item.kind}
                      label={item.label}
                      description={duplicateOutput ? "Already present" : item.detail}
                      icon={<Icon name={NODE_ICON[item.kind]} size="md" />}
                      disabled={duplicateOutput}
                      disabledReason="This graph already has an Output node."
                      onSelect={() => addNode(item.kind)}
                    />
                  );
                })}
              </ChoiceGrid>
            </DisclosureSection>

            <DisclosureSection title="Later gates" summary={`${FUTURE_NODES.length}`} density="compact" defaultOpen={false}>
              <ChoiceGrid label="Future graph nodes" minColumn={150}>
                {FUTURE_NODES.map(([label, reason]) => (
                  <ChoiceCard key={label} label={label} description={reason} disabled disabledReason={reason} onSelect={() => undefined} />
                ))}
              </ChoiceGrid>
            </DisclosureSection>

            <DisclosureSection
              title="Keyboard/list view"
              summary={`${draft.nodes.length} nodes · ${draft.edges.length} connections`}
              density="compact"
              open={view.listAlternativeOpen}
              onOpenChange={(open) => animationEditorStore.getState().updateGraph(workspaceKey, (current) => ({ ...current, listAlternativeOpen: open }))}
            >
              <ol className="animation-graph-node-list" aria-label="Animation graph nodes in document order">
                {draft.nodes.map((node) => (
                  <li key={node.id}>
                    <Button
                      variant="ghost"
                      compact
                      className="animation-graph-node-list__item"
                      aria-pressed={view.selectedNodeIds.includes(node.id)}
                      active={view.selectedNodeIds.includes(node.id)}
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
                    >{node.name} · {nodeKindLabel(node.kind)}</Button>
                  </li>
                ))}
              </ol>

              <div className="animation-graph-connect" aria-label="Keyboard connection editor">
                <FieldGrid minColumn={128}>
                  <Field label="Source node" htmlFor={ids.listSourceNode}>
                    <SelectField
                      id={ids.listSourceNode}
                      aria-label="Connection source node"
                      value={listSourceNodeId}
                      onChange={(event) => {
                        setListSourceNodeId(event.target.value);
                        const node = draft.nodes.find((candidate) => candidate.id === event.target.value);
                        setListSourcePortId(node ? animationGraphPorts(node.kind).find((port) => port.direction === "output" && port.kind === "pose")?.id ?? "pose" : "pose");
                      }}
                    >
                      <option value="">Choose source</option>
                      {poseSourceNodes.map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}
                    </SelectField>
                  </Field>
                  <Field label="Source port" htmlFor={ids.listSourcePort}>
                    <SelectField id={ids.listSourcePort} aria-label="Connection source port" value={listSourcePortId} onChange={(event) => setListSourcePortId(event.target.value)}>
                      {portOptions(listSourceNodeId, "output").map((port) => <option key={port.id} value={port.id}>{port.label}</option>)}
                    </SelectField>
                  </Field>
                  <Field label="Target node" htmlFor={ids.listTargetNode}>
                    <SelectField
                      id={ids.listTargetNode}
                      aria-label="Connection target node"
                      value={listTargetNodeId}
                      onChange={(event) => {
                        setListTargetNodeId(event.target.value);
                        const node = draft.nodes.find((candidate) => candidate.id === event.target.value);
                        setListTargetPortId(node ? animationGraphPorts(node.kind).find((port) => port.direction === "input" && port.kind === "pose")?.id ?? "pose" : "pose");
                      }}
                    >
                      <option value="">Choose target</option>
                      {poseTargetNodes.map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}
                    </SelectField>
                  </Field>
                  <Field label="Target port" htmlFor={ids.listTargetPort}>
                    <SelectField id={ids.listTargetPort} aria-label="Connection target port" value={listTargetPortId} onChange={(event) => setListTargetPortId(event.target.value)}>
                      {portOptions(listTargetNodeId, "input").map((port) => <option key={port.id} value={port.id}>{port.label}</option>)}
                    </SelectField>
                  </Field>
                </FieldGrid>
                <Button
                  compact
                  disabled={!listSourceNodeId || !listTargetNodeId}
                  disabledReason="Choose both a source node and a target node before adding a connection."
                  onClick={() => connect({ source: listSourceNodeId, sourceHandle: listSourcePortId, target: listTargetNodeId, targetHandle: listTargetPortId })}
                >Add connection</Button>

                <ul className="animation-graph-connection-list" aria-label="Animation graph connections">
                  {draft.edges.map((edge) => <li key={edge.id}>
                    <SelectField aria-label={`Source node for ${edge.id}`} value={edge.fromNodeId} onChange={(event) => editConnection(edge.id, { fromNodeId: event.target.value })}>
                      {poseSourceNodes.map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}
                    </SelectField>
                    <SelectField aria-label={`Source port for ${edge.id}`} value={edge.fromPortId} onChange={(event) => editConnection(edge.id, { fromPortId: event.target.value })}>
                      {portOptions(edge.fromNodeId, "output").map((port) => <option key={port.id} value={port.id}>{port.label}</option>)}
                    </SelectField>
                    <Icon name="arrow-right" size="sm" />
                    <SelectField aria-label={`Target node for ${edge.id}`} value={edge.toNodeId} onChange={(event) => editConnection(edge.id, { toNodeId: event.target.value, toPortId: animationGraphPorts(draft.nodes.find((node) => node.id === event.target.value)?.kind ?? "output").find((port) => port.direction === "input" && port.kind === "pose")?.id ?? "pose" })}>
                      {poseTargetNodes.map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}
                    </SelectField>
                    <SelectField aria-label={`Target port for ${edge.id}`} value={edge.toPortId} onChange={(event) => editConnection(edge.id, { toPortId: event.target.value })}>
                      {portOptions(edge.toNodeId, "input").map((port) => <option key={port.id} value={port.id}>{port.label}</option>)}
                    </SelectField>
                    <Button compact icon variant="ghost" aria-label={`Delete connection ${edge.id}`} onClick={() => removeConnectionById(edge.id)}><Icon name="close" size="sm" /></Button>
                  </li>)}
                </ul>
              </div>
            </DisclosureSection>
          </aside>}

          <main className="animation-graph-canvas" aria-label="Animation graph canvas">
            {/* THE SHARED CANVAS. Background, dot grid, zoom pill, mini map and legend used to be
                declared here and styled by a second copy of the graph stylesheet scoped to this
                editor's own class. `autoFit` is the one thing an AUTHORING canvas inverts: a fit that
                re-ran on every shape change would re-frame the graph mid-drag. */}
            <GraphSurface
              nodes={nodes}
              edges={edges}
              label="Animation graph"
              autoFit={false}
              minimapFrom={12}
              /* IN THE CHROME ROW, NOT FLOATING OVER THE CANVAS. Pinned to the bottom-left corner
                 it landed on top of the legend and hid two of its three keys — the collision
                 `GraphSurface` gave the head and the foot their own grid rows to make
                 unrepresentable, reintroduced by one panel putting something back on the canvas. */
              toolbar={debug ? (
                <div className="animation-graph-runtime-summary" data-testid="animation-graph-runtime-trace">
                  {debug.activeNodes.length} active · {debug.evaluationCostMicros ?? "—"} μs · tick {debug.localTick}
                  {debug.transition && <span> · transition {Math.round(debug.transition.progress * 100)}%</span>}
                  {(debug.eventsTruncated || debug.truncated) && <strong> · trace incomplete or safety-limited</strong>}
                </div>
              ) : undefined}
              legend={[
                { label: "Active this tick", state: "active" },
                { label: "Authored", state: "default" },
                { label: "Disabled", state: "disabled" },
              ]}
              flowProps={{
                nodesDraggable: true,
                nodesConnectable: true,
                minZoom: 0.1,
                maxZoom: 4,
                defaultViewport: view.viewport,
                deleteKeyCode: null,
                multiSelectionKeyCode: ["Meta", "Control"],
                selectionKeyCode: "Shift",
                onInit: (instance) => {
                  flowRef.current = instance as unknown as ReactFlowInstance<GraphCardNode, FlowEdge>;
                  if (fitOnNextGraphRender.current) {
                    fitOnNextGraphRender.current = false;
                    window.requestAnimationFrame(() => void instance.fitView({ duration: 180, padding: 0.18 }));
                  }
                },
                onConnect: connect,
                onNodeDrag: (_event, node) => { dragPositions.current[node.id] = { x: node.position.x, y: node.position.y }; },
                onNodeDragStop: onNodeDragStop as never,
                onSelectionChange: onSelectionChange as never,
                onPaneClick: () => animationEditorStore.getState().setGraphSelection(workspaceKey, [], []),
                onMoveEnd,
                "aria-label": "Editable animation graph. Press F to fit, slash to search, or open the keyboard list.",
              }}
            />
          </main>

          {view.inspectorOpen && <aside id="animation-graph-inspector" className="animation-graph-inspector" aria-label="Animation graph inspector">
            <DisclosureSection title="Selection" summary={selectedEdge ? "connection" : selectedNode ? nodeKindLabel(selectedNode.kind) : "nothing selected"} density="compact" defaultOpen>
              {!selectedNode && !selectedEdge ? <Callout tone="info">Select a node, a connection, or a diagnostic to edit it here.</Callout> : selectedEdge ? (
                <>
                  <Callout tone="neutral" icon={<Icon name="link" size="sm" />}>
                    {draft.nodes.find((node) => node.id === selectedEdge.fromNodeId)?.name ?? selectedEdge.fromNodeId}
                    {" → "}
                    {draft.nodes.find((node) => node.id === selectedEdge.toNodeId)?.name ?? selectedEdge.toNodeId}
                  </Callout>
                  <Checkbox label="Enabled" description="A disabled connection stays authored but contributes nothing." checked={selectedEdge.enabled} onChange={(checked) => updateSelectedEdge({ enabled: checked })} />
                  <FieldGrid minColumn={132}>
                    {selectedEdgeWeightSupport?.explicit && (
                      <Field label="Explicit weight" htmlFor={ids.edgeWeight} help="Leave empty to use the destination's own policy.">
                        <TextField
                          id={ids.edgeWeight}
                          inputMode="decimal"
                          value={selectedEdge.weight ?? ""}
                          placeholder="destination policy"
                          onChange={(event) => updateSelectedEdge({ weight: event.target.value === "" ? null : Math.max(0, Number(event.target.value)), weightParameterId: null })}
                        />
                      </Field>
                    )}
                    {selectedEdgeWeightSupport?.parameter && (
                      <Field label="Weight parameter" htmlFor={ids.edgeParameter} help="The stable connection ID retains this binding when inputs are reordered." span="wide">
                        <SelectField id={ids.edgeParameter} value={selectedEdge.weightParameterId ?? ""} onChange={(event) => updateSelectedEdge({ weightParameterId: event.target.value || null, weight: null })}>
                          <option value="">Use explicit/default weight</option>
                          {draft.parameters.filter((parameter) => parameter.kind === "float").map((parameter) => <option key={parameter.id} value={parameter.id}>{parameter.name}</option>)}
                        </SelectField>
                      </Field>
                    )}
                  </FieldGrid>
                  {selectedEdgeWeightSupport && <Callout tone="info">{selectedEdgeWeightSupport.reason}</Callout>}
                  <div className="animation-graph-actions">
                    {selectedEdgeWeightSupport && !selectedEdgeWeightSupport.explicit && !selectedEdgeWeightSupport.parameter
                      && (selectedEdge.weight !== null || selectedEdge.weightParameterId !== null)
                      && <Button compact variant="ghost" onClick={() => updateSelectedEdge({ weight: null, weightParameterId: null })}>Clear unsupported weight data</Button>}
                    {selectedEdgeWeightSupport && !selectedEdgeWeightSupport.explicit
                      && selectedEdge.toPortId === "layer"
                      && (selectedEdgeTarget?.kind === "layer_override" || selectedEdgeTarget?.kind === "layer_additive")
                      && selectedEdgeTarget.parameterIds.length > 0
                      && <Button compact variant="ghost" onClick={() => updateSelectedEdge({ weight: 1, weightParameterId: null })}>Use edge constant instead</Button>}
                    <Button compact variant="danger" onClick={removeSelection}>Remove connection</Button>
                  </div>
                </>
              ) : selectedNode ? (
                <>
                  <Field label="Name" htmlFor={ids.nodeName}>
                    <TextField id={ids.nodeName} aria-label="Name" value={selectedNode.name} onChange={(event) => updateSelectedNode({ name: event.target.value })} />
                  </Field>
                  <Checkbox label="Enabled" description="A disabled node stays authored but evaluates to its fallback." checked={selectedNode.enabled} onChange={(checked) => updateSelectedNode({ enabled: checked })} />
                  <div className="animation-graph-identity">
                    <code>{selectedNode.id}</code>
                    <span>{nodeKindLabel(selectedNode.kind)}</span>
                  </div>
                  {selectedNode.kind === "sequence" && (
                    <Field label="Source" htmlFor={ids.nodeSource} help={state.sources.find((source) => source.id === selectedNode.sourceId)?.reason}>
                      <SelectField id={ids.nodeSource} value={selectedNode.sourceId ?? ""} onChange={(event) => updateSelectedNode({ sourceId: event.target.value || null })}>
                        <option value="">Choose a ready source</option>
                        {state.sources.filter((source) => source.kind !== "reference_pose").map((source) => <option key={source.id} value={source.id} disabled={source.readiness === "blocked"}>{source.name}{source.readiness === "blocked" ? " — unavailable" : ""}</option>)}
                      </SelectField>
                    </Field>
                  )}
                  {(["blend_1d", "blend_2d_cartesian", "layer_override", "layer_additive"] as AnimationGraphNodeKind[]).includes(selectedNode.kind) && (
                    <Field
                      label="Parameter"
                      htmlFor={ids.nodeParameter}
                      help={(selectedNode.kind === "layer_override" || selectedNode.kind === "layer_additive")
                        ? "A node-level layer parameter owns the runtime weight. Clear any constant on the layer input edge; both cannot be authored together."
                        : undefined}
                    >
                      <SelectField id={ids.nodeParameter} value={selectedNode.parameterIds[0] ?? ""} onChange={(event) => updateSelectedNodeParameter(event.target.value)}>
                        <option value="">No parameter</option>
                        {draft.parameters.filter((parameter) => animationGraphCompatibleParameterKinds(selectedNode.kind).includes(parameter.kind)).map((parameter) => <option key={parameter.id} value={parameter.id}>{parameter.name} · {parameter.kind}</option>)}
                      </SelectField>
                    </Field>
                  )}
                  {(selectedNode.kind === "blend_1d" || selectedNode.kind === "blend_2d_cartesian") && (
                    <>
                      <div className="animation-graph-samples" role="group" aria-label="Stable blend samples">
                        <h4>Keyed samples</h4>
                        {selectedNode.samples.map((sample) => {
                          const edge = draft.edges.find((candidate) => candidate.id === sample.edgeId);
                          const source = draft.nodes.find((candidate) => candidate.id === edge?.fromNodeId);
                          return <div key={sample.id} className="animation-graph-sample">
                            <code title={`${sample.id} · ${sample.edgeId}`}>{source?.name ?? sample.edgeId}</code>
                            <NumericField
                              ariaLabel={`${source?.name ?? sample.id} sample X`}
                              value={sample.position[0]}
                              onCommit={(value) => updateSelectedNode({ samples: selectedNode.samples.map((candidate) => candidate.id === sample.id ? { ...candidate, position: [value, candidate.position[1]] } : candidate) })}
                            />
                            {selectedNode.kind === "blend_2d_cartesian" && <NumericField
                              ariaLabel={`${source?.name ?? sample.id} sample Y`}
                              value={sample.position[1]}
                              onCommit={(value) => updateSelectedNode({ samples: selectedNode.samples.map((candidate) => candidate.id === sample.id ? { ...candidate, position: [candidate.position[0], value] } : candidate) })}
                            />}
                          </div>;
                        })}
                        <Callout tone="info">Each coordinate is attached to a durable sample and connection ID, never list position.</Callout>
                      </div>
                      {selectedNode.kind === "blend_2d_cartesian" && (
                        <Field label="Authored triangles" htmlFor={ids.triangles} span="full" help="Three distinct connected sample IDs per triangle, one triangle per line. Triangulation is compiled, never rebuilt per frame.">
                          <TextAreaField
                            id={ids.triangles}
                            rows={3}
                            value={selectedNode.triangles.map((triangle) => triangle.join(", ")).join("\n")}
                            placeholder="sample-left, sample-right, sample-forward"
                            onChange={(event) => updateSelectedNode({ triangles: event.target.value.split(/\r?\n/).map((line) => line.split(",").map((id) => id.trim()).filter(Boolean)).filter((list) => list.length === 3).map((list) => [list[0], list[1], list[2]] as const) })}
                          />
                        </Field>
                      )}
                      {selectedNode.kind === "blend_2d_cartesian" && <Callout tone="neutral">{selectedNode.triangles.length} authored triangles. Native compilation validates stable point membership and hull coverage.</Callout>}
                    </>
                  )}
                  {(selectedNode.kind === "layer_override" || selectedNode.kind === "layer_additive") && (
                    <>
                      <Field label="Mask bindings" htmlFor={ids.mask} span="full" help="One selector per line. A full slash path is target/component/property[/subpath]; * matches one complete segment and ** matches any depth — for example **/Transform/rotation.">
                        <TextAreaField
                          id={ids.mask}
                          rows={3}
                          value={selectedNode.maskBindings.join("\n")}
                          placeholder="**/Transform/rotation"
                          onChange={(event) => updateSelectedNode({ maskBindings: event.target.value.split(/\r?\n/).map((item) => item.trim()).filter(Boolean) })}
                        />
                      </Field>
                      {selectedNode.maskBindings.map((selector) => animationGraphMaskSelectorError(selector) && (
                        <Callout key={selector} tone="danger" role="alert">{selector}: {animationGraphMaskSelectorError(selector)}</Callout>
                      ))}
                    </>
                  )}
                </>
              ) : null}
            </DisclosureSection>

            {selectedMachine && (
              <DisclosureSection
                title="States and transitions"
                summary={`${selectedMachine.states.length} states · ${selectedMachine.transitions.length} transitions`}
                density="compact"
                defaultOpen
                data-testid="animation-graph-state-inspector"
                actions={<>
                  <Button compact variant="ghost" onClick={addState}>+ State</Button>
                  <Button compact variant="ghost" onClick={addTransition}>+ Transition</Button>
                </>}
              >
                {/* ONE control, not one radio per row. "Which state does this machine start in" is a
                    single choice, and drawing it as N radios put a `entry` radio, a name field, a pose
                    select, a `reset` checkbox and a remove button on one row — which is the row that
                    wrapped and left a bare `×` orphaned on a line of its own in the first capture. */}
                <Field label="Entry state" htmlFor={ids.entryState} help="Where the machine starts when it is entered.">
                  <SelectField
                    id={ids.entryState}
                    value={selectedMachine.entryStateId}
                    onChange={(event) => mutateDraft((graph) => ({ ...graph, stateMachines: graph.stateMachines.map((machine) => machine.id === selectedMachine.id ? { ...machine, entryStateId: event.target.value } : machine) }))}
                  >
                    {selectedMachine.states.length === 0 && <option value="">No states yet</option>}
                    {selectedMachine.states.map((graphState) => <option key={graphState.id} value={graphState.id}>{graphState.name}</option>)}
                  </SelectField>
                </Field>

                <ul className="animation-graph-state-list">
                  {selectedMachine.states.map((graphState) => (
                    <li key={graphState.id} className="animation-graph-state" data-entry={selectedMachine.entryStateId === graphState.id || undefined}>
                      <div className="animation-graph-state__head">
                        <TextField aria-label={`State name ${graphState.name}`} value={graphState.name} onChange={(event) => updateMachineState(graphState.id, { name: event.target.value })} />
                        {selectedMachine.entryStateId === graphState.id && <Badge tone="accent" title="The machine starts here.">entry</Badge>}
                        <Button compact icon variant="ghost" aria-label={`Remove state ${graphState.name}`} onClick={() => removeMachineState(graphState.id)}><Icon name="close" size="sm" /></Button>
                      </div>
                      {/* LABELLED, because two unlabelled boxes stacked on top of each other read as
                          the same field twice: the capture showed `Idle` over `Idle`, with nothing on
                          screen saying the second one was what the state PLAYS. */}
                      <Field label="Plays" htmlFor={`${ids.entryState}-pose-${graphState.id}`}>
                        <SelectField id={`${ids.entryState}-pose-${graphState.id}`} aria-label={`Pose for ${graphState.name}`} value={graphState.poseNodeId} onChange={(event) => updateMachineState(graphState.id, { poseNodeId: event.target.value })}>
                          {draft.nodes.filter((node) => ["reference_pose", "sequence", "blend_normalized", "blend_direct", "blend_1d", "blend_2d_cartesian", "layer_override", "layer_additive"].includes(node.kind)).map((node) => <option key={node.id} value={node.id}>{node.name}</option>)}
                        </SelectField>
                      </Field>
                      <Checkbox label="Restart the source's local time on entry" checked={graphState.resetOnEntry} onChange={(checked) => updateMachineState(graphState.id, { resetOnEntry: checked })} />
                    </li>
                  ))}
                </ul>

                {selectedMachine.transitions.length > 0 && (
                  <Field label="Transition" htmlFor={ids.transition}>
                    <SelectField id={ids.transition} value={selectedTransition?.id ?? ""} onChange={(event) => setSelectedTransitionId(event.target.value)}>
                      {selectedMachine.transitions.map((transition) => <option key={transition.id} value={transition.id}>{selectedMachine.states.find((item) => item.id === transition.fromStateId)?.name} → {selectedMachine.states.find((item) => item.id === transition.toStateId)?.name}</option>)}
                    </SelectField>
                  </Field>
                )}
                {selectedTransition && (
                  <>
                    <FieldGrid minColumn={124}>
                      <Field label="From" htmlFor={ids.from}>
                        <SelectField id={ids.from} value={selectedTransition.fromStateId} onChange={(event) => updateTransition({ fromStateId: event.target.value })}>
                          {selectedMachine.states.map((graphState) => <option key={graphState.id} value={graphState.id}>{graphState.name}</option>)}
                        </SelectField>
                      </Field>
                      <Field label="To" htmlFor={ids.to}>
                        <SelectField id={ids.to} value={selectedTransition.toStateId} onChange={(event) => updateTransition({ toStateId: event.target.value })}>
                          {selectedMachine.states.map((graphState) => <option key={graphState.id} value={graphState.id}>{graphState.name}</option>)}
                        </SelectField>
                      </Field>
                      <Field label="Priority" htmlFor={ids.priority} help="Higher wins when several transitions are ready at once.">
                        <NumericField id={ids.priority} ariaLabel="Transition priority" integer value={selectedTransition.priority} onCommit={(value) => updateTransition({ priority: Math.trunc(value) })} />
                      </Field>
                      <Field label="Duration" htmlFor={ids.duration} unit="ticks">
                        <NumericField id={ids.duration} ariaLabel="Transition duration in ticks" integer value={selectedTransition.durationTick} min={0} onCommit={(value) => updateTransition({ durationTick: Math.max(0, Math.trunc(value)) })} />
                      </Field>
                      <Field label="Curve" htmlFor={ids.curve}>
                        <SelectField id={ids.curve} value={selectedTransition.curve} onChange={(event) => updateTransition({ curve: event.target.value as AnimationGraphTransition["curve"] })}>
                          <option value="linear">Linear</option><option value="ease_in">Ease in</option><option value="ease_out">Ease out</option><option value="ease_in_out">Ease in/out</option><option value="smoothstep">Smoothstep</option>
                        </SelectField>
                      </Field>
                      <Field label="Interruption" htmlFor={ids.interruption}>
                        <SelectField id={ids.interruption} value={selectedTransition.interruption} onChange={(event) => updateTransition({ interruption: event.target.value as AnimationGraphTransition["interruption"] })}>
                          <option value="none">None</option><option value="source">Source</option><option value="destination">Destination</option><option value="both">Both</option>
                        </SelectField>
                      </Field>
                      <Field label="Exit time" htmlFor={ids.exitTime} span="wide" help="Persisted for forward compatibility only. Native schema v2 reports a compile diagnostic and keeps the last-good preview when exit time is set.">
                        <TextField
                          id={ids.exitTime}
                          inputMode="decimal"
                          value={selectedTransition.exitTime ?? ""}
                          placeholder="condition only"
                          onChange={(event) => updateTransition({ exitTime: event.target.value === "" ? null : Math.max(0, Math.min(1, Number(event.target.value))) })}
                        />
                      </Field>
                    </FieldGrid>
                    <div className="animation-graph-actions">
                      <Button compact variant="ghost" onClick={addCondition}>+ Condition</Button>
                    </div>
                    {selectedTransition.conditions.map((condition) => {
                      const conditionParameter = draft.parameters.find((parameter) => parameter.id === condition.parameterId);
                      const booleanLike = conditionParameter?.kind === "boolean" || conditionParameter?.kind === "trigger";
                      return (
                        <div key={condition.id} className="animation-graph-condition">
                          <SelectField aria-label="Condition parameter" value={condition.parameterId} onChange={(event) => { const parameter = draft.parameters.find((item) => item.id === event.target.value); if (parameter) updateCondition(condition.id, { parameterId: parameter.id, operator: parameter.kind === "boolean" ? "is_true" : parameter.kind === "trigger" ? "triggered" : "greater", value: parameter.kind === "boolean" || parameter.kind === "trigger" ? null : parameter.defaultValue }); }}>
                            {draft.parameters.map((parameter) => <option key={parameter.id} value={parameter.id}>{parameter.name}</option>)}
                          </SelectField>
                          <SelectField aria-label="Condition operator" value={condition.operator} onChange={(event) => updateCondition(condition.id, { operator: event.target.value as AnimationGraphCondition["operator"] })}>
                            {booleanLike ? <><option value="is_true">is true</option><option value="is_false">is false</option>{conditionParameter?.kind === "trigger" && <option value="triggered">triggered</option>}</> : <><option value="greater">greater</option><option value="greater_equal">greater/equal</option><option value="less">less</option><option value="less_equal">less/equal</option><option value="equal">equal</option><option value="not_equal">not equal</option></>}
                          </SelectField>
                          {!booleanLike && <NumericField ariaLabel="Condition value" value={typeof condition.value === "number" ? condition.value : 0} onCommit={(value) => updateCondition(condition.id, { value })} />}
                          <Button compact icon variant="ghost" aria-label="Remove condition" onClick={() => removeCondition(condition.id)}><Icon name="close" size="sm" /></Button>
                        </div>
                      );
                    })}
                  </>
                )}
              </DisclosureSection>
            )}

            <DisclosureSection title="Typed parameters" summary={`${draft.parameters.length}`} density="compact" defaultOpen>
              {draft.parameters.length === 0 ? <Callout tone="info">Add a parameter from the preview strip below.</Callout> : draft.parameters.map((parameter) => (
                <div key={parameter.id} className="animation-graph-parameter">
                  <div className="animation-graph-parameter__head">
                    <TextField aria-label={`Parameter name ${parameter.name}`} value={parameter.name} onChange={(event) => updateParameter(parameter.id, { name: event.target.value })} />
                    <SelectField aria-label={`Parameter type ${parameter.name}`} value={parameter.kind} onChange={(event) => { const kind = event.target.value as AnimationGraphParameter["kind"]; updateParameter(parameter.id, { kind, defaultValue: defaultAnimationGraphValue(kind), min: kind === "float" || kind === "integer" ? 0 : null, max: kind === "float" || kind === "integer" ? 1 : null }); }}>
                      <option value="float">Float</option><option value="integer">Integer</option><option value="boolean">Boolean</option><option value="trigger">Trigger</option><option value="vec2">Vector 2</option>
                    </SelectField>
                    <Button compact icon variant="ghost" aria-label={`Remove parameter ${parameter.name}`} onClick={() => removeParameter(parameter.id)}><Icon name="close" size="sm" /></Button>
                  </div>
                  {parameter.kind === "trigger" ? (
                    <span className="animation-graph-parameter__note">
                      momentary · the authored default is always false
                      {parameter.defaultValue === true && <Button compact variant="ghost" onClick={() => updateParameter(parameter.id, { defaultValue: false })}>Reset invalid default</Button>}
                    </span>
                  ) : parameter.kind === "boolean" ? (
                    <Checkbox label="Default is on" checked={parameter.defaultValue === true} onChange={(checked) => updateParameter(parameter.id, { defaultValue: checked })} />
                  ) : parameter.kind === "vec2" ? (
                    <div className="animation-graph-parameter__row">
                      <NumericField ariaLabel={`Default ${parameter.name} X`} value={Array.isArray(parameter.defaultValue) ? parameter.defaultValue[0] : 0} onCommit={(value) => updateParameter(parameter.id, { defaultValue: [value, Array.isArray(parameter.defaultValue) ? parameter.defaultValue[1] : 0] })} />
                      <NumericField ariaLabel={`Default ${parameter.name} Y`} value={Array.isArray(parameter.defaultValue) ? parameter.defaultValue[1] : 0} onCommit={(value) => updateParameter(parameter.id, { defaultValue: [Array.isArray(parameter.defaultValue) ? parameter.defaultValue[0] : 0, value] })} />
                    </div>
                  ) : (
                    <div className="animation-graph-parameter__row">
                      <NumericField ariaLabel={`Default ${parameter.name}`} integer={parameter.kind === "integer"} step={parameter.kind === "integer" ? 1 : 0.01} value={typeof parameter.defaultValue === "number" ? parameter.defaultValue : 0} onCommit={(value) => updateParameter(parameter.id, { defaultValue: parameter.kind === "integer" ? Math.round(value) : value })} />
                      {/* A BOUND CAN BE UNSET, AND A SCRUBBER CANNOT SAY SO. `NumericField` always
                          shows a number, so an absent min would read as 0 — the same lie the exit
                          time and the explicit edge weight are text fields to avoid. */}
                      <TextField aria-label={`Minimum ${parameter.name}`} inputMode="decimal" placeholder="min" value={parameter.min ?? ""} onChange={(event) => updateParameter(parameter.id, { min: event.target.value === "" ? null : Number(event.target.value) })} />
                      <TextField aria-label={`Maximum ${parameter.name}`} inputMode="decimal" placeholder="max" value={parameter.max ?? ""} onChange={(event) => updateParameter(parameter.id, { max: event.target.value === "" ? null : Number(event.target.value) })} />
                    </div>
                  )}
                </div>
              ))}
            </DisclosureSection>

            <DisclosureSection title="Diagnostics" summary={`${diagnostics.length}`} density="compact" defaultOpen>
              {diagnostics.length === 0 ? <Callout tone="success">No graph diagnostics.</Callout> : <ul className="animation-graph-diagnostics">{diagnostics.map((diagnostic) => (
                <li key={diagnostic.id} data-severity={diagnostic.severity}>
                  <Button variant="ghost" className="animation-graph-diagnostic" onClick={() => navigateDiagnostic(diagnostic)}>
                    <span className="animation-graph-diagnostic__code">{diagnostic.code.replaceAll("_", " ")}</span>
                    <span className="animation-graph-diagnostic__message">{diagnostic.message}</span>
                    {diagnostic.fix && <span className="animation-graph-diagnostic__fix">{diagnostic.fix}</span>}
                  </Button>
                </li>
              ))}</ul>}
            </DisclosureSection>

            <DisclosureSection title="Runtime trace" summary={debug ? `${debug.activeNodes.length} active` : "no instance"} density="compact" defaultOpen={false}>
              {!debug ? <Callout tone="info">{state.compile.state === "ready" ? "Waiting for a matching runtime instance…" : state.compile.message}</Callout> : (
                <>
                  <div className="animation-graph-identity"><code>{debug.instanceId}</code><code>{debug.compiledHash}</code></div>
                  <p>{debug.activeNodes.length} active nodes · {debug.activeEdges.length} active paths · {debug.evaluationCostMicros ?? "unmeasured"} μs</p>
                  {debug.transition && <ProgressBar label="Active transition progress" value={debug.transition.progress} />}
                  {debug.watches.map((watch) => <div key={watch.id} className="animation-graph-watch"><code>{watch.id}</code> {JSON.stringify(watch.value)} · {watch.source}</div>)}
                  {(debug.eventsTruncated || debug.truncated) && <Callout tone="warn" role="alert">Runtime trace is incomplete or safety-limited; events or ambiguous edge provenance may be omitted.</Callout>}
                </>
              )}
            </DisclosureSection>
          </aside>}

          <section className="animation-graph-parameters" aria-label="Transient animation graph parameters">
            <span className="animation-graph-parameters__title">
              <strong>Preview parameters</strong>
              <span>transient · never saved</span>
            </span>
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
            <ToolbarSpacer />
            <TextField
              id={ids.newParameterName}
              aria-label="New graph parameter name"
              placeholder="Parameter name"
              value={newParameterName}
              onChange={(event) => setNewParameterName(event.target.value)}
            />
            <SelectField id={ids.newParameterKind} aria-label="New graph parameter type" value={newParameterKind} onChange={(event) => setNewParameterKind(event.target.value as AnimationGraphParameter["kind"])}>
              <option value="float">Float</option><option value="integer">Integer</option><option value="boolean">Boolean</option><option value="trigger">Trigger</option><option value="vec2">Vector 2</option>
            </SelectField>
            <Button compact variant="ghost" onClick={addParameter}>+ Parameter</Button>
            <Button
              compact
              variant="ghost"
              disabled={Object.keys(previewValues).length === 0}
              disabledReason="No preview override is active, so there is nothing to reset."
              onClick={() => void clearPreview()}
            >Reset preview</Button>
          </section>
        </>
      )}

      {/* The screen-reader channel for everything this editor decides — the shared visually-hidden
          class, not a fourth private one. The message is in a CHILD so the clipped 1px box holds no
          text of its own to be judged as "cut with no sign". */}
      <div className="mtk-visually-hidden" role="status" aria-live="polite"><span>{error ?? liveMessage}</span></div>
      {error && <Callout className="animation-graph-error" tone="danger" role="alert">{error}</Callout>}
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
    control = <Button compact variant="secondary" aria-label={`Fire ${parameter.name} trigger`} onClick={() => onChange(true)}>Fire {parameter.name}</Button>;
  } else if (parameter.kind === "boolean") {
    control = <Checkbox label={parameter.name} checked={value === true} onChange={(checked) => onChange(checked)} />;
  } else if (parameter.kind === "vec2") {
    const tuple = Array.isArray(value) ? value : [0, 0];
    control = (
      <span className="animation-graph-preview__vector">
        <span className="animation-graph-preview__name">{parameter.name}</span>
        <NumericField ariaLabel={`${parameter.name} X`} value={tuple[0]} onCommit={(next) => onChange([next, tuple[1]])} />
        <NumericField ariaLabel={`${parameter.name} Y`} value={tuple[1]} onCommit={(next) => onChange([tuple[0], next])} />
      </span>
    );
  } else {
    const numeric = typeof value === "number" ? value : 0;
    control = (
      <SliderField
        label={parameter.name}
        ariaLabel={parameter.name}
        min={parameter.min ?? 0}
        max={parameter.max ?? 1}
        step={parameter.kind === "integer" ? 1 : 0.01}
        value={numeric}
        valueLabel={numeric.toFixed(parameter.kind === "integer" ? 0 : 2)}
        onChange={(event) => onChange(parameter.kind === "integer" ? Math.round(Number(event.target.value)) : Number(event.target.value))}
      />
    );
  }
  return (
    <span className="animation-graph-preview">
      {control}
      <Button
        compact
        icon
        variant="ghost"
        active={watched}
        aria-pressed={watched}
        aria-label={watched ? `Stop watching ${parameter.name}` : `Watch ${parameter.name}`}
        title={watched ? "Remove from bounded runtime watches" : "Add to bounded runtime watches"}
        onClick={() => onWatch(!watched)}
      ><Icon name="pin" size="sm" /></Button>
    </span>
  );
}
