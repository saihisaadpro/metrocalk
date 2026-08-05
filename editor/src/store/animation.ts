//! Persistent, editor-only animation workspace state.
//!
//! Authored tracks and preview transport remain authoritative in the native animation engine. This store
//! owns only the user's view of that data: context, selection, filtering, zoom, disclosure, and in-progress
//! form drafts. Keeping it outside `AnimationWorkspace` means collapsing or switching docks never discards
//! work. Every project + entity/sequence scope has three independent context snapshots over the same
//! authored sequence; switching 2D/3D/UI therefore changes presentation, not animation data.

import { createStore } from "zustand/vanilla";

export const ANIMATION_CONTEXTS = ["2d", "3d", "ui"] as const;
export type AnimationContext = (typeof ANIMATION_CONTEXTS)[number];
export type AnimationEditorView = "timeline" | "curves";
export type AnimationWorkspaceSurface = AnimationEditorView | "graph";
export type AnimationTimeDisplay = "frames" | "seconds" | "ticks";
export type AnimationSnapGrid = "frames" | "seconds" | "ticks";
export type AnimationWorkspaceScopeKind = "entity" | "sequence";

export interface AnimationWorkspaceIdentity {
  /** A stable project/session identity. A path is suitable after Save; untitled projects should use a session ID. */
  projectId: string;
  scope: {
    kind: AnimationWorkspaceScopeKind;
    id: string;
  };
}

declare const animationWorkspaceKeyBrand: unique symbol;
export type AnimationWorkspaceKey = string & { readonly [animationWorkspaceKeyBrand]: true };

/** JSON tuple encoding is collision-free even when paths and engine IDs contain separators. */
export function animationWorkspaceKey(identity: AnimationWorkspaceIdentity): AnimationWorkspaceKey {
  return JSON.stringify([identity.projectId, identity.scope.kind, identity.scope.id]) as AnimationWorkspaceKey;
}

export type AnimationDraftValue =
  | null
  | boolean
  | number
  | string
  | readonly AnimationDraftValue[]
  | { readonly [key: string]: AnimationDraftValue };

export interface AnimationSnapState {
  enabled: boolean;
  grid: AnimationSnapGrid;
  /** Positive grid units. For example, `0.5` with seconds or `1` with frames/ticks. */
  increment: number;
  toKeys: boolean;
  toMarkers: boolean;
}

export interface AnimationScrollState {
  x: number;
  y: number;
}

export interface AnimationInspectorState {
  open: boolean;
  /** Stable disclosure IDs to open/closed state. Unmentioned sections use their component default. */
  disclosures: Readonly<Record<string, boolean>>;
}

export interface AnimationContextViewState {
  view: AnimationEditorView;
  selectedTrackIds: readonly string[];
  selectedKeyIds: readonly string[];
  selectionAnchorKeyId: string | null;
  search: string;
  zoom: number;
  snap: Readonly<AnimationSnapState>;
  timeDisplay: AnimationTimeDisplay;
  scroll: Readonly<AnimationScrollState>;
  inspector: Readonly<AnimationInspectorState>;
  /** Draft IDs are owned by the editor control, e.g. `event:event-7` or `key:key-3:value`. */
  drafts: Readonly<Record<string, AnimationDraftValue>>;
}

export interface AnimationGraphViewportState {
  x: number;
  y: number;
  zoom: number;
}

/** Sequence-global, editor-only graph presentation. Authored graph documents remain native-authoritative;
 * drafts are frozen JSON snapshots retained only until one explicit Apply succeeds. */
export interface AnimationGraphViewState {
  viewport: Readonly<AnimationGraphViewportState>;
  selectedNodeIds: readonly string[];
  selectedEdgeIds: readonly string[];
  search: string;
  paletteOpen: boolean;
  inspectorOpen: boolean;
  listAlternativeOpen: boolean;
  watches: readonly string[];
  debugInstanceId: string | null;
  showWeights: boolean;
  showCosts: boolean;
  drafts: Readonly<Record<string, AnimationDraftValue>>;
}

export interface AnimationWorkspaceViewState {
  activeContext: AnimationContext;
  /** Timeline/curves presentation stays per context; Graph is sequence-global. */
  activeSurface: AnimationWorkspaceSurface;
  contexts: Readonly<Record<AnimationContext, AnimationContextViewState>>;
  graph: Readonly<AnimationGraphViewState>;
  /** Monotonic store-local revision used only for bounded LRU pruning. */
  lastTouched: number;
}

export const MIN_ANIMATION_ZOOM = 0.1;
export const MAX_ANIMATION_ZOOM = 64;

const EMPTY_IDS: readonly string[] = Object.freeze([]);
const EMPTY_BOOLEAN_RECORD: Readonly<Record<string, boolean>> = Object.freeze({});
const EMPTY_DRAFT_RECORD: Readonly<Record<string, AnimationDraftValue>> = Object.freeze({});
const DEFAULT_SNAP: Readonly<AnimationSnapState> = Object.freeze({
  enabled: true,
  grid: "frames",
  increment: 1,
  toKeys: true,
  toMarkers: true,
});
const DEFAULT_SCROLL: Readonly<AnimationScrollState> = Object.freeze({ x: 0, y: 0 });
const DEFAULT_INSPECTOR: Readonly<AnimationInspectorState> = Object.freeze({
  open: true,
  disclosures: EMPTY_BOOLEAN_RECORD,
});

export const DEFAULT_ANIMATION_CONTEXT_VIEW: AnimationContextViewState = Object.freeze({
  view: "timeline",
  selectedTrackIds: EMPTY_IDS,
  selectedKeyIds: EMPTY_IDS,
  selectionAnchorKeyId: null,
  search: "",
  zoom: 1,
  snap: DEFAULT_SNAP,
  timeDisplay: "seconds",
  scroll: DEFAULT_SCROLL,
  inspector: DEFAULT_INSPECTOR,
  drafts: EMPTY_DRAFT_RECORD,
});

const DEFAULT_CONTEXTS: Readonly<Record<AnimationContext, AnimationContextViewState>> = Object.freeze({
  "2d": DEFAULT_ANIMATION_CONTEXT_VIEW,
  "3d": DEFAULT_ANIMATION_CONTEXT_VIEW,
  ui: DEFAULT_ANIMATION_CONTEXT_VIEW,
});

export const DEFAULT_ANIMATION_GRAPH_VIEW: AnimationGraphViewState = Object.freeze({
  viewport: Object.freeze({ x: 0, y: 0, zoom: 1 }),
  selectedNodeIds: EMPTY_IDS,
  selectedEdgeIds: EMPTY_IDS,
  search: "",
  paletteOpen: false,
  inspectorOpen: false,
  listAlternativeOpen: false,
  watches: EMPTY_IDS,
  debugInstanceId: null,
  showWeights: true,
  showCosts: false,
  drafts: EMPTY_DRAFT_RECORD,
});

export const DEFAULT_ANIMATION_WORKSPACE_VIEW: AnimationWorkspaceViewState = Object.freeze({
  activeContext: "3d",
  activeSurface: "timeline",
  contexts: DEFAULT_CONTEXTS,
  graph: DEFAULT_ANIMATION_GRAPH_VIEW,
  lastTouched: 0,
});

export interface AnimationPruneOptions {
  /** Restrict pruning to one project. Omit to consider every cached workspace. */
  projectId?: string;
  /** When supplied, remove otherwise-in-scope entries not present in this set. */
  keepKeys?: readonly AnimationWorkspaceKey[];
  /** Then retain at most this many in-scope entries, newest first. Explicit keep keys are never evicted. */
  maxEntries?: number;
}

export interface AnimationEditorViewState {
  workspaces: Readonly<Record<string, AnimationWorkspaceViewState>>;
  revision: number;

  ensure(key: AnimationWorkspaceKey): void;
  setActiveContext(key: AnimationWorkspaceKey, context: AnimationContext): void;
  setActiveSurface(key: AnimationWorkspaceKey, surface: AnimationWorkspaceSurface): void;
  /** Atomic escape hatch. The input is frozen; the returned state is normalized, cloned, and frozen. */
  updateContext(
    key: AnimationWorkspaceKey,
    context: AnimationContext,
    update: (current: AnimationContextViewState) => AnimationContextViewState,
  ): void;
  setView(key: AnimationWorkspaceKey, context: AnimationContext, view: AnimationEditorView): void;
  setSelection(
    key: AnimationWorkspaceKey,
    context: AnimationContext,
    selectedTrackIds: readonly string[],
    selectedKeyIds: readonly string[],
    anchorKeyId?: string | null,
  ): void;
  setSearch(key: AnimationWorkspaceKey, context: AnimationContext, search: string): void;
  setZoom(key: AnimationWorkspaceKey, context: AnimationContext, zoom: number): void;
  setSnap(key: AnimationWorkspaceKey, context: AnimationContext, snap: Partial<AnimationSnapState>): void;
  setTimeDisplay(key: AnimationWorkspaceKey, context: AnimationContext, display: AnimationTimeDisplay): void;
  setScroll(key: AnimationWorkspaceKey, context: AnimationContext, scroll: Partial<AnimationScrollState>): void;
  setInspectorOpen(key: AnimationWorkspaceKey, context: AnimationContext, open: boolean): void;
  setInspectorDisclosure(key: AnimationWorkspaceKey, context: AnimationContext, disclosureId: string, open: boolean): void;
  setDraft(key: AnimationWorkspaceKey, context: AnimationContext, draftId: string, value: AnimationDraftValue): void;
  removeDraft(key: AnimationWorkspaceKey, context: AnimationContext, draftId: string): void;
  clearDrafts(key: AnimationWorkspaceKey, context: AnimationContext): void;
  updateGraph(key: AnimationWorkspaceKey, update: (current: AnimationGraphViewState) => AnimationGraphViewState): void;
  setGraphViewport(key: AnimationWorkspaceKey, viewport: Partial<AnimationGraphViewportState>): void;
  setGraphSelection(key: AnimationWorkspaceKey, nodeIds: readonly string[], edgeIds: readonly string[]): void;
  setGraphSearch(key: AnimationWorkspaceKey, search: string): void;
  setGraphWatches(key: AnimationWorkspaceKey, watches: readonly string[]): void;
  setGraphDraft(key: AnimationWorkspaceKey, graphId: string, value: AnimationDraftValue | null): void;
  /** Remove one workspace, or clear the whole view cache when `key` is omitted. */
  reset(key?: AnimationWorkspaceKey): void;
  /** Remove stale project/scope records and/or enforce an LRU bound. Returns the number removed. */
  prune(options?: AnimationPruneOptions): number;
}

type ContextUpdater = (current: AnimationContextViewState) => AnimationContextViewState;
type StoreSetter = (
  update: (state: AnimationEditorViewState) => AnimationEditorViewState,
) => void;

function finiteAtLeastZero(value: number, fallback: number): number {
  return Number.isFinite(value) ? Math.max(0, value) : fallback;
}

function normalizedZoom(value: number, fallback: number): number {
  return Number.isFinite(value) ? Math.min(MAX_ANIMATION_ZOOM, Math.max(MIN_ANIMATION_ZOOM, value)) : fallback;
}

function normalizedIncrement(value: number, fallback: number): number {
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

function uniqueIds(values: readonly string[]): readonly string[] {
  const out: string[] = [];
  const seen = new Set<string>();
  for (const value of values) {
    if (!value || seen.has(value)) continue;
    seen.add(value);
    out.push(value);
  }
  return Object.freeze(out);
}

function sameIds(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function cloneDraftValue(value: AnimationDraftValue): AnimationDraftValue {
  if (Array.isArray(value)) return Object.freeze(value.map(cloneDraftValue));
  if (value !== null && typeof value === "object") {
    const copy: Record<string, AnimationDraftValue> = {};
    for (const [key, child] of Object.entries(value)) copy[key] = cloneDraftValue(child);
    return Object.freeze(copy);
  }
  return value;
}

function cloneDrafts(values: Readonly<Record<string, AnimationDraftValue>>): Readonly<Record<string, AnimationDraftValue>> {
  const copy: Record<string, AnimationDraftValue> = {};
  for (const [key, value] of Object.entries(values)) copy[key] = cloneDraftValue(value);
  return Object.freeze(copy);
}

function normalizeContext(candidate: AnimationContextViewState, fallback: AnimationContextViewState): AnimationContextViewState {
  const selectedTrackIds = uniqueIds(candidate.selectedTrackIds);
  const selectedKeyIds = uniqueIds(candidate.selectedKeyIds);
  const anchor = candidate.selectionAnchorKeyId && selectedKeyIds.includes(candidate.selectionAnchorKeyId)
    ? candidate.selectionAnchorKeyId
    : selectedKeyIds[0] ?? null;
  return Object.freeze({
    view: candidate.view,
    selectedTrackIds,
    selectedKeyIds,
    selectionAnchorKeyId: anchor,
    search: candidate.search,
    zoom: normalizedZoom(candidate.zoom, fallback.zoom),
    snap: Object.freeze({
      enabled: candidate.snap.enabled,
      grid: candidate.snap.grid,
      increment: normalizedIncrement(candidate.snap.increment, fallback.snap.increment),
      toKeys: candidate.snap.toKeys,
      toMarkers: candidate.snap.toMarkers,
    }),
    timeDisplay: candidate.timeDisplay,
    scroll: Object.freeze({
      x: finiteAtLeastZero(candidate.scroll.x, fallback.scroll.x),
      y: finiteAtLeastZero(candidate.scroll.y, fallback.scroll.y),
    }),
    inspector: Object.freeze({
      open: candidate.inspector.open,
      disclosures: Object.freeze({ ...candidate.inspector.disclosures }),
    }),
    drafts: cloneDrafts(candidate.drafts),
  });
}

function normalizeGraph(candidate: AnimationGraphViewState, fallback: AnimationGraphViewState): AnimationGraphViewState {
  return Object.freeze({
    viewport: Object.freeze({
      x: Number.isFinite(candidate.viewport.x) ? candidate.viewport.x : fallback.viewport.x,
      y: Number.isFinite(candidate.viewport.y) ? candidate.viewport.y : fallback.viewport.y,
      zoom: normalizedZoom(candidate.viewport.zoom, fallback.viewport.zoom),
    }),
    selectedNodeIds: uniqueIds(candidate.selectedNodeIds),
    selectedEdgeIds: uniqueIds(candidate.selectedEdgeIds),
    search: candidate.search,
    paletteOpen: candidate.paletteOpen,
    inspectorOpen: candidate.inspectorOpen,
    listAlternativeOpen: candidate.listAlternativeOpen,
    watches: uniqueIds(candidate.watches),
    debugInstanceId: candidate.debugInstanceId || null,
    showWeights: candidate.showWeights,
    showCosts: candidate.showCosts,
    drafts: cloneDrafts(candidate.drafts),
  });
}

function touchedWorkspace(
  workspace: AnimationWorkspaceViewState,
  revision: number,
  patch: Partial<Pick<AnimationWorkspaceViewState, "activeContext" | "activeSurface" | "contexts" | "graph">>,
): AnimationWorkspaceViewState {
  return Object.freeze({ ...workspace, ...patch, lastTouched: revision });
}

function updateGraphEntry(
  set: StoreSetter,
  key: AnimationWorkspaceKey,
  update: (current: AnimationGraphViewState) => AnimationGraphViewState,
): void {
  set((state) => {
    const workspace = state.workspaces[key] ?? DEFAULT_ANIMATION_WORKSPACE_VIEW;
    const candidate = update(workspace.graph);
    if (candidate === workspace.graph) return state;
    const revision = state.revision + 1;
    const graph = normalizeGraph(candidate, workspace.graph);
    return {
      ...state,
      workspaces: Object.freeze({
        ...state.workspaces,
        [key]: touchedWorkspace(workspace, revision, { graph }),
      }),
      revision,
    };
  });
}

function updateContextEntry(
  set: StoreSetter,
  key: AnimationWorkspaceKey,
  context: AnimationContext,
  update: ContextUpdater,
): void {
  set((state) => {
    const existing = state.workspaces[key];
    const workspace = existing ?? DEFAULT_ANIMATION_WORKSPACE_VIEW;
    const current = workspace.contexts[context];
    const candidate = update(current);
    if (candidate === current) return state;
    const nextContext = normalizeContext(candidate, current);
    const revision = state.revision + 1;
    const contexts = Object.freeze({ ...workspace.contexts, [context]: nextContext });
    const nextWorkspace = touchedWorkspace(workspace, revision, { contexts });
    return {
      ...state,
      workspaces: Object.freeze({ ...state.workspaces, [key]: nextWorkspace }),
      revision,
    };
  });
}

function projectOf(key: string): string | null {
  try {
    const decoded: unknown = JSON.parse(key);
    return Array.isArray(decoded) && decoded.length === 3 && typeof decoded[0] === "string"
      ? decoded[0]
      : null;
  } catch {
    return null;
  }
}

const EMPTY_WORKSPACES: Readonly<Record<string, AnimationWorkspaceViewState>> = Object.freeze({});

export const animationEditorStore = createStore<AnimationEditorViewState>((set, get) => ({
  workspaces: EMPTY_WORKSPACES,
  revision: 0,

  ensure(key) {
    set((state) => {
      if (state.workspaces[key]) return state;
      const revision = state.revision + 1;
      return {
        ...state,
        workspaces: Object.freeze({
          ...state.workspaces,
          [key]: touchedWorkspace(DEFAULT_ANIMATION_WORKSPACE_VIEW, revision, {}),
        }),
        revision,
      };
    });
  },

  setActiveContext(key, context) {
    set((state) => {
      const workspace = state.workspaces[key] ?? DEFAULT_ANIMATION_WORKSPACE_VIEW;
      if (workspace.activeContext === context) return state;
      const revision = state.revision + 1;
      const nextWorkspace = touchedWorkspace(workspace, revision, { activeContext: context });
      return {
        ...state,
        workspaces: Object.freeze({ ...state.workspaces, [key]: nextWorkspace }),
        revision,
      };
    });
  },

  setActiveSurface(key, activeSurface) {
    set((state) => {
      const workspace = state.workspaces[key] ?? DEFAULT_ANIMATION_WORKSPACE_VIEW;
      if (workspace.activeSurface === activeSurface) return state;
      const revision = state.revision + 1;
      return {
        ...state,
        workspaces: Object.freeze({ ...state.workspaces, [key]: touchedWorkspace(workspace, revision, { activeSurface }) }),
        revision,
      };
    });
  },

  updateContext(key, context, update) {
    updateContextEntry(set, key, context, update);
  },

  setView(key, context, view) {
    updateContextEntry(set, key, context, (current) => current.view === view ? current : { ...current, view });
  },

  setSelection(key, context, selectedTrackIds, selectedKeyIds, anchorKeyId) {
    updateContextEntry(set, key, context, (current) => {
      const tracks = uniqueIds(selectedTrackIds);
      const keys = uniqueIds(selectedKeyIds);
      const anchor = anchorKeyId !== undefined
        ? anchorKeyId && keys.includes(anchorKeyId) ? anchorKeyId : keys[0] ?? null
        : current.selectionAnchorKeyId && keys.includes(current.selectionAnchorKeyId)
          ? current.selectionAnchorKeyId
          : keys[0] ?? null;
      return sameIds(current.selectedTrackIds, tracks)
        && sameIds(current.selectedKeyIds, keys)
        && current.selectionAnchorKeyId === anchor
        ? current
        : { ...current, selectedTrackIds: tracks, selectedKeyIds: keys, selectionAnchorKeyId: anchor };
    });
  },

  setSearch(key, context, search) {
    updateContextEntry(set, key, context, (current) => current.search === search ? current : { ...current, search });
  },

  setZoom(key, context, zoom) {
    updateContextEntry(set, key, context, (current) => {
      const next = normalizedZoom(zoom, current.zoom);
      return current.zoom === next ? current : { ...current, zoom: next };
    });
  },

  setSnap(key, context, snap) {
    updateContextEntry(set, key, context, (current) => {
      const next = {
        ...current.snap,
        ...snap,
        increment: normalizedIncrement(snap.increment ?? current.snap.increment, current.snap.increment),
      };
      return current.snap.enabled === next.enabled
        && current.snap.grid === next.grid
        && current.snap.increment === next.increment
        && current.snap.toKeys === next.toKeys
        && current.snap.toMarkers === next.toMarkers
        ? current
        : { ...current, snap: next };
    });
  },

  setTimeDisplay(key, context, timeDisplay) {
    updateContextEntry(set, key, context, (current) => current.timeDisplay === timeDisplay ? current : { ...current, timeDisplay });
  },

  setScroll(key, context, scroll) {
    updateContextEntry(set, key, context, (current) => {
      const next = {
        x: finiteAtLeastZero(scroll.x ?? current.scroll.x, current.scroll.x),
        y: finiteAtLeastZero(scroll.y ?? current.scroll.y, current.scroll.y),
      };
      return current.scroll.x === next.x && current.scroll.y === next.y ? current : { ...current, scroll: next };
    });
  },

  setInspectorOpen(key, context, open) {
    updateContextEntry(set, key, context, (current) => current.inspector.open === open
      ? current
      : { ...current, inspector: { ...current.inspector, open } });
  },

  setInspectorDisclosure(key, context, disclosureId, open) {
    if (!disclosureId) return;
    updateContextEntry(set, key, context, (current) => current.inspector.disclosures[disclosureId] === open
      ? current
      : {
          ...current,
          inspector: {
            ...current.inspector,
            disclosures: { ...current.inspector.disclosures, [disclosureId]: open },
          },
        });
  },

  setDraft(key, context, draftId, value) {
    if (!draftId) return;
    updateContextEntry(set, key, context, (current) => ({
      ...current,
      drafts: { ...current.drafts, [draftId]: value },
    }));
  },

  removeDraft(key, context, draftId) {
    updateContextEntry(set, key, context, (current) => {
      if (!(draftId in current.drafts)) return current;
      const drafts = { ...current.drafts };
      delete drafts[draftId];
      return { ...current, drafts };
    });
  },

  clearDrafts(key, context) {
    updateContextEntry(set, key, context, (current) => Object.keys(current.drafts).length === 0
      ? current
      : { ...current, drafts: EMPTY_DRAFT_RECORD });
  },

  updateGraph(key, update) {
    updateGraphEntry(set, key, update);
  },

  setGraphViewport(key, viewport) {
    updateGraphEntry(set, key, (current) => {
      const next = {
        x: Number.isFinite(viewport.x) ? viewport.x as number : current.viewport.x,
        y: Number.isFinite(viewport.y) ? viewport.y as number : current.viewport.y,
        zoom: normalizedZoom(viewport.zoom ?? current.viewport.zoom, current.viewport.zoom),
      };
      return current.viewport.x === next.x && current.viewport.y === next.y && current.viewport.zoom === next.zoom
        ? current
        : { ...current, viewport: next };
    });
  },

  setGraphSelection(key, nodeIds, edgeIds) {
    updateGraphEntry(set, key, (current) => {
      const nodes = uniqueIds(nodeIds);
      const edges = uniqueIds(edgeIds);
      return sameIds(nodes, current.selectedNodeIds) && sameIds(edges, current.selectedEdgeIds)
        ? current
        : { ...current, selectedNodeIds: nodes, selectedEdgeIds: edges };
    });
  },

  setGraphSearch(key, search) {
    updateGraphEntry(set, key, (current) => current.search === search ? current : { ...current, search });
  },

  setGraphWatches(key, watches) {
    updateGraphEntry(set, key, (current) => {
      const next = uniqueIds(watches).slice(0, 32);
      return sameIds(next, current.watches) ? current : { ...current, watches: next };
    });
  },

  setGraphDraft(key, graphId, value) {
    if (!graphId) return;
    updateGraphEntry(set, key, (current) => {
      const drafts = { ...current.drafts };
      if (value === null) {
        if (!(graphId in drafts)) return current;
        delete drafts[graphId];
      } else {
        drafts[graphId] = value;
      }
      return { ...current, drafts };
    });
  },

  reset(key) {
    if (!key) {
      set((state) => state.workspaces === EMPTY_WORKSPACES && state.revision === 0
        ? state
        : { ...state, workspaces: EMPTY_WORKSPACES, revision: 0 });
      return;
    }
    set((state) => {
      if (!state.workspaces[key]) return state;
      const workspaces = { ...state.workspaces };
      delete workspaces[key];
      return { ...state, workspaces: Object.freeze(workspaces), revision: state.revision + 1 };
    });
  },

  prune(options = {}) {
    const before = get().workspaces;
    const keep = new Set<string>(options.keepKeys ?? []);
    const inScope = (key: string) => options.projectId === undefined || projectOf(key) === options.projectId;
    const workspaces: Record<string, AnimationWorkspaceViewState> = { ...before };
    let removed = 0;

    if (options.keepKeys !== undefined) {
      for (const key of Object.keys(workspaces)) {
        if (inScope(key) && !keep.has(key)) {
          delete workspaces[key];
          removed += 1;
        }
      }
    }

    if (options.maxEntries !== undefined) {
      const limit = Math.max(0, Math.floor(options.maxEntries));
      const candidates = Object.entries(workspaces)
        .filter(([key]) => inScope(key) && !keep.has(key))
        .sort(([leftKey, left], [rightKey, right]) => left.lastTouched - right.lastTouched || leftKey.localeCompare(rightKey));
      const scopedCount = Object.keys(workspaces).filter(inScope).length;
      let excess = Math.max(0, scopedCount - limit);
      for (const [key] of candidates) {
        if (excess === 0) break;
        delete workspaces[key];
        removed += 1;
        excess -= 1;
      }
    }

    if (removed > 0) {
      set((state) => ({ ...state, workspaces: Object.freeze(workspaces), revision: state.revision + 1 }));
    }
    return removed;
  },
}));

/** Stable selector fallback; call `ensure` before editing if the record must exist for pruning. */
export function animationWorkspaceView(state: AnimationEditorViewState, key: AnimationWorkspaceKey): AnimationWorkspaceViewState {
  return state.workspaces[key] ?? DEFAULT_ANIMATION_WORKSPACE_VIEW;
}

export function animationContextView(
  state: AnimationEditorViewState,
  key: AnimationWorkspaceKey,
  context?: AnimationContext,
): AnimationContextViewState {
  const workspace = animationWorkspaceView(state, key);
  return workspace.contexts[context ?? workspace.activeContext];
}
