//! Unified animation workspace. 2D, 3D, and UI are context views over one native sequence and playhead;
//! they never create parallel authored models. View choices live in the editor-only animation store, while
//! every key, marker, event, track flag, and preview command still flows through the native animation API.

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { useStore } from "zustand";
import { projectionStore, useDisplayedEntity, useSelectedId } from "../store/projection";
import { useProjectSessionId } from "../store/project";
import { setStatus } from "../store/ui";
import {
  ANIMATION_CONTEXTS,
  MAX_ANIMATION_ZOOM,
  MIN_ANIMATION_ZOOM,
  animationContextView,
  animationEditorStore,
  animationWorkspaceKey,
  animationWorkspaceView,
  type AnimationDraftValue,
} from "../store/animation";
import { Badge, Button, NumericField } from "../theme/primitives";
import { Modal } from "../theme/Popover";
import { DisclosureSection, DockTabs, EmptyPanelState, ShortcutBadge } from "../theme/workspace";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import { AnimationGraphEditor } from "../graph/AnimationGraphEditor";
import type {
  AnimationBindingState,
  AnimationContext,
  AnimationContextReadiness,
  AnimationClipInstanceSaveRequest,
  AnimationEditResult,
  AnimationInterpolation,
  AnimationImportedClipInfo,
  AnimationKeyDeleteInfo,
  AnimationKeyInfo,
  AnimationKeyUpdateInfo,
  AnimationLoopPolicy,
  AnimationMarkerInfo,
  AnimationPropertyInfo,
  AnimationTrackInfo,
  AnimationWorkspaceInfo,
  Json,
} from "../transport/protocol";
import type { EditorClient } from "../transport/session";

const EMPTY: AnimationWorkspaceInfo = {
  revision: "empty",
  sequenceId: "main",
  sequenceName: "Main sequence",
  ticksPerSecond: 60_000,
  durationTick: 60_000,
  currentTick: 0,
  playing: false,
  loopPolicy: "once",
  selectedId: null,
  selectedName: null,
  properties: [],
  tracks: [],
  markers: [],
  events: [],
  contexts: [],
  asset: null,
  issues: [],
};

const CONTEXT_LABELS: Record<AnimationContext, string> = { "2d": "2D", "3d": "3D", ui: "UI" };
const PROPERTY_DRAFT = "workspace:selected-property";
const INTERPOLATION_DRAFT = "workspace:new-interpolation";
const MARKER_NAME_DRAFT = "workspace:new-marker-name";
const EVENT_NAME_DRAFT = "workspace:new-event-name";
const EVENT_PAYLOAD_DRAFT = "workspace:new-event-payload";

const card: CSSProperties = {
  minWidth: 0,
  border: `1px solid ${color.border.subtle}`,
  borderRadius: radius.lg,
  background: color.bg.panel,
};

const selectStyle: CSSProperties = {
  minWidth: 0,
  background: color.bg.input,
  color: color.text.primary,
  border: `1px solid ${color.border.default}`,
  borderRadius: radius.md,
  padding: `${space.xs}px ${space.sm}px`,
};

function valueLabel(value: unknown): string {
  if (typeof value === "number") return Number.isInteger(value) ? `${value}` : value.toPrecision(5);
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "string") return value;
  return JSON.stringify(value);
}

function normalizedExactSceneName(value: string): string {
  const displayName = value
    .replace(/\s*\([^)]*\)\s*$/, "")
    .split(/[\\/]/)
    .at(-1)
    ?.trim() ?? "";
  return displayName.normalize("NFKC").replace(/\s+/g, " ").toLocaleLowerCase();
}

function propertyId(property: AnimationPropertyInfo): string {
  return `${property.component}\u0000${property.property}`;
}

/** Key IDs are stable within a track, not necessarily across a whole sequence (legacy mechanism
 * tracks intentionally share `legacy-{tick}` IDs). Editor selection therefore uses the full identity. */
function keySelectionId(trackId: string, keyId: string): string {
  return JSON.stringify([trackId, keyId]);
}

function runtimeSinkLabel(sink: string | null): string {
  switch (sink) {
    case "viewport-transform": return "Native viewport playback";
    case "kinematic-joint": return "Native mechanism playback";
    case "workspace-2d": return "2D editor preview only";
    case "workspace-ui": return "UI editor preview only";
    case null: return "No playback adapter";
    default: return sink.replaceAll("-", " ");
  }
}

function isContext(value: unknown): value is AnimationContext {
  return value === "2d" || value === "3d" || value === "ui";
}

/** Old projects and dev fixtures predate explicit context metadata. Keep them visible, but never fork data. */
function inferredContext(item: { component: string; property: string; context?: AnimationContext; editorKind?: string }): AnimationContext {
  if (isContext(item.context)) return item.context;
  const token = `${item.component}.${item.property}.${item.editorKind ?? ""}`.toLocaleLowerCase();
  if (/sprite|frame|flipx|flipy|2d/.test(token)) return "2d";
  if (/ui|layout|opacity|visibility|style|healthbar|button|anchor|colour|color/.test(token)) return "ui";
  return "3d";
}

function bindingState(item: { bindingState?: AnimationBindingState; animatable?: boolean }): AnimationBindingState {
  if (item.bindingState) return item.bindingState;
  return item.animatable === false ? "unsupported" : "ready";
}

function bindingTone(state: AnimationBindingState): "success" | "warn" | "neutral" {
  if (state === "ready") return "success";
  if (state === "preview_only" || state === "invalid") return "warn";
  return "neutral";
}

function capabilityTone(state: string): "success" | "warn" | "neutral" {
  if (state === "available") return "success";
  if (state === "missing" || state === "decoded_only") return "warn";
  return "neutral";
}

function draftString(value: AnimationDraftValue | undefined): string {
  return typeof value === "string" ? value : "";
}

function contextReadiness(model: AnimationWorkspaceInfo, context: AnimationContext): AnimationContextReadiness {
  const explicit = model.contexts?.find((item) => item.context === context);
  if (explicit) return explicit;
  const properties = model.properties.filter((item) => inferredContext(item) === context);
  const tracks = model.tracks.filter((item) => inferredContext(item) === context);
  return {
    context,
    state: properties.some((item) => item.animatable) || tracks.length > 0 ? "ready" : "unsupported",
    properties: properties.length,
    tracks: tracks.length,
    reason: properties.length > 0 || tracks.length > 0
      ? "Legacy metadata was classified by its typed binding."
      : `No ${CONTEXT_LABELS[context]} channels are present on this selection.`,
    action: null,
  };
}

function timeLabel(tick: number, ticksPerSecond: number, display: "frames" | "seconds" | "ticks"): string {
  if (display === "ticks") return `${Math.round(tick)}t`;
  if (display === "frames") return `${Math.round((tick / Math.max(1, ticksPerSecond)) * 60)}f`;
  return `${(tick / Math.max(1, ticksPerSecond)).toFixed(2)}s`;
}

function markerColor(marker: AnimationMarkerInfo): string {
  if (!marker.color) return color.warn.text;
  const [red, green, blue, alpha] = marker.color;
  const scale = Math.max(red, green, blue) <= 1 ? 255 : 1;
  const normalizedAlpha = alpha > 1 ? alpha / 255 : alpha;
  return `rgba(${Math.round(red * scale)}, ${Math.round(green * scale)}, ${Math.round(blue * scale)}, ${Math.min(1, Math.max(0, normalizedAlpha))})`;
}

function isInteractiveTarget(target: EventTarget | null): boolean {
  return target instanceof Element && Boolean(target.closest("input, select, textarea, button, [contenteditable='true'], [role='button'], [role='slider'], [role='spinbutton'], [role='tab']"));
}

function disclosureOpen(disclosures: Readonly<Record<string, boolean>>, id: string, fallback: boolean): boolean {
  return disclosures[id] ?? fallback;
}

function errorText(cause: unknown, fallback: string): string {
  return cause instanceof Error && cause.message.trim() ? cause.message : fallback;
}

function crossedEventStatus(response: { crossedEvents: readonly { name: string }[]; eventsTruncated: boolean }): string | null {
  const names = response.crossedEvents.map((event) => event.name).join(" · ");
  if (response.eventsTruncated) {
    return `${names ? `${names} · ` : ""}additional event crossings were omitted by the safety limit`;
  }
  return names || null;
}

function sampleTrack(track: AnimationTrackInfo, tick: number): Json | null {
  const keys = track.keys;
  if (!track.enabled || keys.length === 0) return null;
  let low = 0;
  let high = keys.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if (keys[middle].tick <= tick) low = middle + 1;
    else high = middle;
  }
  const upper = low === keys.length ? -1 : low;
  if (upper === 0) return keys[0].value;
  if (upper < 0) return keys[keys.length - 1].value;
  const left = keys[upper - 1];
  const right = keys[upper];
  if (left.tick === tick || right.tick === left.tick || track.interpolation === "step") return left.value;
  if (typeof left.value !== "number" || typeof right.value !== "number") return left.value;
  const span = right.tick - left.tick;
  const factor = (tick - left.tick) / span;
  if (track.interpolation === "linear") {
    const value = left.value + (right.value - left.value) * factor;
    return track.valueKind === "integer" ? Math.round(value) : value;
  }
  const h00 = 2 * factor ** 3 - 3 * factor ** 2 + 1;
  const h10 = factor ** 3 - 2 * factor ** 2 + factor;
  const h01 = -2 * factor ** 3 + 3 * factor ** 2;
  const h11 = factor ** 3 - factor ** 2;
  const outgoing = typeof left.outTangent === "number" ? left.outTangent : 0;
  const incoming = typeof right.inTangent === "number" ? right.inTangent : 0;
  const value = h00 * left.value + h10 * span * outgoing + h01 * right.value + h11 * span * incoming;
  return track.valueKind === "integer" ? Math.round(value) : value;
}

function finiteNumber(value: Json | undefined, fallback: number): number {
  return typeof value === "number" && Number.isFinite(value) ? value : fallback;
}

function finiteChannel(value: Json | undefined, fallback: number): number {
  const channel = finiteNumber(value, fallback);
  return Math.max(0, Math.min(1, channel > 1 ? channel / 255 : channel));
}

function ContextPreview({
  context,
  values,
  selectedName,
  readiness,
}: {
  context: Exclude<AnimationContext, "3d">;
  values: Readonly<Record<string, Json>>;
  selectedName: string | null;
  readiness: AnimationContextReadiness;
}) {
  const previewFrame: CSSProperties = {
    position: "relative",
    height: 118,
    overflow: "hidden",
    border: `1px solid ${color.border.subtle}`,
    borderRadius: radius.md,
    backgroundColor: color.bg.inset,
    backgroundImage: `linear-gradient(${color.border.subtle} 1px, transparent 1px), linear-gradient(90deg, ${color.border.subtle} 1px, transparent 1px)`,
    backgroundSize: "16px 16px",
  };
  const opacity = Math.max(0, Math.min(1, finiteNumber(
    values[context === "2d" ? "Sprite.opacity" : "UiStyle.opacity"],
    1,
  )));

  if (context === "2d") {
    const x = Math.max(-42, Math.min(42, finiteNumber(values["Transform.px"] ?? values["Transform.x"], 0)));
    const y = Math.max(-32, Math.min(32, finiteNumber(values["Transform.py"] ?? values["Transform.y"], 0)));
    const scaleX = Math.max(0.15, Math.min(2.5, finiteNumber(values["Transform.sx"], 1)));
    const scaleY = Math.max(0.15, Math.min(2.5, finiteNumber(values["Transform.sy"], 1)));
    const flipX = values["Sprite.flipX"] === true ? -1 : 1;
    const flipY = values["Sprite.flipY"] === true ? -1 : 1;
    const red = finiteChannel(values["Sprite.tintR"], 0.2);
    const green = finiteChannel(values["Sprite.tintG"], 0.65);
    const blue = finiteChannel(values["Sprite.tintB"], 1);
    const alpha = finiteChannel(values["Sprite.tintA"], 1);
    const frame = Math.round(finiteNumber(values["Sprite.frame"], 0));
    return (
      <div data-testid="animation-context-preview" data-context="2d" data-readiness={readiness.state} title={readiness.reason} style={{ marginBottom: space.sm }}>
        <div style={{ display: "flex", justifyContent: "space-between", marginBottom: space.xs, color: color.text.muted, fontSize: fontSize.micro }}>
          <span>Editor preview · deployment adapter required for Sprite channels · frame {frame}</span><span>{selectedName ?? "Selection"}</span>
        </div>
        <div style={previewFrame} aria-label="2D animation preview">
          <div style={{ position: "absolute", left: "50%", top: "50%", width: 46, height: 46, margin: "-23px", display: "grid", placeItems: "center", border: `1px solid ${color.accent.base}`, borderRadius: radius.md, color: color.text.primary, background: `rgba(${Math.round(red * 255)}, ${Math.round(green * 255)}, ${Math.round(blue * 255)}, ${alpha})`, opacity, transform: `translate(${x}px, ${y}px) scale(${scaleX * flipX}, ${scaleY * flipY})`, boxShadow: `0 8px 24px rgba(0, 0, 0, ${0.18 * opacity})`, font: font.mono, fontSize: fontSize.micro }}>
            {frame}
          </div>
        </div>
      </div>
    );
  }

  const x = Math.max(-38, Math.min(38, finiteNumber(values["UiStyle.x"], 0)));
  const y = Math.max(-28, Math.min(28, finiteNumber(values["UiStyle.y"], 0)));
  const width = Math.max(52, Math.min(156, finiteNumber(values["UiStyle.width"], 118)));
  const height = Math.max(28, Math.min(72, finiteNumber(values["UiStyle.height"], 48)));
  const scale = Math.max(0.25, Math.min(2, finiteNumber(values["UiStyle.scale"], 1)));
  const visible = values["UiStyle.visible"] !== false;
  const red = finiteChannel(values["UiStyle.r"], 0.12);
  const green = finiteChannel(values["UiStyle.g"], 0.48);
  const blue = finiteChannel(values["UiStyle.b"], 0.9);
  const alpha = finiteChannel(values["UiStyle.a"], 1);
  const healthWidth = Math.max(0, Math.min(100, finiteNumber(values["HealthBar.width"], 72)));
  return (
    <div data-testid="animation-context-preview" data-context="ui" data-readiness={readiness.state} title={readiness.reason} style={{ marginBottom: space.sm }}>
      <div style={{ display: "flex", justifyContent: "space-between", marginBottom: space.xs, color: color.text.muted, fontSize: fontSize.micro }}>
        <span>Editor preview · deployment adapter required</span><span>{visible ? "visible" : "hidden"}</span>
      </div>
      <div style={previewFrame} aria-label="UI animation preview">
        <div style={{ position: "absolute", left: "50%", top: "50%", width, height, marginLeft: -width / 2, marginTop: -height / 2, padding: 8, border: `1px solid ${color.accent.base}`, borderRadius: radius.md, background: `rgba(${Math.round(red * 255)}, ${Math.round(green * 255)}, ${Math.round(blue * 255)}, ${alpha})`, opacity: visible ? opacity : 0.12, transform: `translate(${x}px, ${y}px) scale(${scale})`, boxShadow: "0 10px 26px rgba(0, 0, 0, 0.22)" }}>
          <div style={{ overflow: "hidden", height: 7, borderRadius: 99, background: "rgba(0, 0, 0, 0.28)" }}>
            <div style={{ width: `${healthWidth}%`, height: "100%", background: color.success.text }} />
          </div>
          <div style={{ marginTop: 7, color: color.text.primary, fontSize: fontSize.micro, whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{selectedName ?? "UI element"}</div>
        </div>
      </div>
    </div>
  );
}

export function AnimationWorkspace({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const projectSessionId = useProjectSessionId();
  const selectedProjection = useDisplayedEntity(selectedId ?? "");
  const [model, setModel] = useState<AnimationWorkspaceInfo>(EMPTY);
  const [displayTick, setDisplayTick] = useState(0);
  const [busy, setBusy] = useState(false);
  const [clipSetupId, setClipSetupId] = useState<string | null>(null);
  const [clipPreviewId, setClipPreviewId] = useState<string | null>(null);
  const [clipPreviewPending, setClipPreviewPending] = useState(false);
  const [clipTargetMappings, setClipTargetMappings] = useState<Record<string, string>>({});
  const [clipDiscardConfirmId, setClipDiscardConfirmId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const refreshEpoch = useRef(0);
  const editEpoch = useRef(0);
  const transportEpoch = useRef(0);
  const timelineRef = useRef<HTMLDivElement>(null);
  const workspaceRef = useRef<HTMLDivElement>(null);
  const scrollCommitTimer = useRef<number | null>(null);
  const pendingScrollCommit = useRef<(() => void) | null>(null);
  const clipDraftRequestToken = useRef<string | null>(null);
  const clipDraftFence = useRef<{
    clipId: string;
    mode: "create" | "repair";
    instanceId: string;
    clipInstanceRevision: string;
    logicalAssetId: string;
    assetRevision: string;
    sourceBindingHash: string;
    sourceTargetIds: string[];
    sourceTargetLabels: string[];
    sourceBindings: NonNullable<AnimationImportedClipInfo["sourceBindings"]>;
    repairChanges: string[];
  } | null>(null);
  const clipSetupTrigger = useRef<HTMLButtonElement | null>(null);
  const clipFirstUnresolvedTarget = useRef<HTMLSelectElement | null>(null);
  const clipPreviewButton = useRef<HTMLButtonElement | null>(null);
  const clipPreviewActive = useRef(false);
  const clipPreviewRequestId = useRef<string | null>(null);
  const clipPreviewMayHaveReachedNative = useRef(false);
  const clipPreviewAttempt = useRef<{ epoch: number; cancelled: boolean } | null>(null);
  const clockAnchor = useRef({ tick: 0, at: 0 });
  const sceneOrder = useStore(projectionStore, (state) => state.order);
  const sceneEntities = useStore(projectionStore, (state) => state.displayed);
  const deactivatedEntities = useStore(projectionStore, (state) => state.deactivated);
  const liveTransformTargets = useMemo(
    () => sceneOrder.flatMap((id) => {
      const entity = sceneEntities[id];
      return entity?.components.Transform && !deactivatedEntities[id]
        ? [{ id, name: entity.name }]
        : [];
    }),
    [deactivatedEntities, sceneEntities, sceneOrder],
  );
  const liveTransformTargetIds = useMemo(
    () => new Set(liveTransformTargets.map((target) => target.id)),
    [liveTransformTargets],
  );

  function clipSourceTargetIds(clip: AnimationImportedClipInfo): string[] {
    const fence = clipDraftFence.current;
    return fence?.clipId === clip.clipId
      ? fence.sourceTargetIds
      : (clip.sourceTargetIds ?? []);
  }

  function clipSourceTargetLabel(clip: AnimationImportedClipInfo, sourceTargetId: string, index: number): string {
    const fence = clipDraftFence.current;
    if (fence?.clipId === clip.clipId) return fence.sourceTargetLabels[index] ?? sourceTargetId;
    return clip.sourceBindings?.find((binding) => binding.sourceTargetId === sourceTargetId)?.sourceTargetLabel
      ?? clip.sourceTargets[index]
      ?? sourceTargetId;
  }

  function clipSourceBindings(
    clip: AnimationImportedClipInfo,
    sourceTargetId?: string,
  ): NonNullable<AnimationImportedClipInfo["sourceBindings"]> {
    const fence = clipDraftFence.current;
    const bindings = fence?.clipId === clip.clipId
      ? fence.sourceBindings
      : (clip.sourceBindings ?? []);
    return sourceTargetId
      ? bindings.filter((binding) => binding.sourceTargetId === sourceTargetId)
      : bindings;
  }

  function clipDraftIsStale(clip: AnimationImportedClipInfo): boolean {
    const fence = clipDraftFence.current;
    const asset = model.asset;
    if (!fence || fence.clipId !== clip.clipId || !asset) return true;
    const currentSourceTargetIds = clip.sourceTargetIds ?? [];
    return fence.clipInstanceRevision !== asset.clipInstanceRevision
      || fence.logicalAssetId !== asset.logicalId
      || fence.assetRevision !== asset.revisionId
      || fence.sourceBindingHash !== clip.sourceBindingHash
      || fence.sourceTargetIds.length !== currentSourceTargetIds.length
      || fence.sourceTargetIds.some((sourceTargetId, index) => sourceTargetId !== currentSourceTargetIds[index]);
  }

  useEffect(() => () => {
    if (scrollCommitTimer.current !== null) window.clearTimeout(scrollCommitTimer.current);
    pendingScrollCommit.current?.();
  }, []);

  useEffect(() => {
    setBusy(false);
    setClipSetupId(null);
    setClipPreviewId(null);
    setClipPreviewPending(false);
    setClipTargetMappings({});
    setClipDiscardConfirmId(null);
    clipDraftRequestToken.current = null;
    clipDraftFence.current = null;
    return () => {
      transportEpoch.current += 1;
      if (clipPreviewAttempt.current) clipPreviewAttempt.current.cancelled = true;
      const shouldStop = clipPreviewMayHaveReachedNative.current || clipPreviewActive.current;
      const expectedRequestId = clipPreviewRequestId.current ?? undefined;
      clipPreviewAttempt.current = null;
      clipPreviewRequestId.current = null;
      clipPreviewMayHaveReachedNative.current = false;
      clipPreviewActive.current = false;
      if (shouldStop) {
        void client.animationClipInstancePreviewStop(expectedRequestId).catch(() => {});
      }
    };
  }, [client, selectedId]);

  const workspaceKey = useMemo(
    () => animationWorkspaceKey({
      projectId: projectSessionId,
      scope: selectedId
        ? { kind: "entity", id: selectedId }
        : { kind: "sequence", id: model.sequenceId || "main" },
    }),
    [model.sequenceId, projectSessionId, selectedId],
  );
  const graphWorkspaceKey = useMemo(
    () => animationWorkspaceKey({
      projectId: projectSessionId,
      scope: { kind: "sequence", id: model.sequenceId || "main" },
    }),
    [model.sequenceId, projectSessionId],
  );
  const workspaceView = useStore(animationEditorStore, (state) => animationWorkspaceView(state, workspaceKey));
  const graphWorkspaceView = useStore(animationEditorStore, (state) => animationWorkspaceView(state, graphWorkspaceKey));
  const activeContext = workspaceView.activeContext;
  const activeSurface = graphWorkspaceView.activeSurface;
  const contextView = animationContextView(animationEditorStore.getState(), workspaceKey, activeContext);

  useEffect(() => {
    if (activeSurface === "graph" || activeSurface === contextView.view) return;
    animationEditorStore.getState().setActiveSurface(graphWorkspaceKey, contextView.view);
  }, [activeSurface, contextView.view, graphWorkspaceKey]);

  useEffect(() => {
    const store = animationEditorStore.getState();
    store.ensure(workspaceKey);
    store.ensure(graphWorkspaceKey);
    store.prune({ maxEntries: 64 });
  }, [graphWorkspaceKey, workspaceKey]);

  const refresh = useCallback(async () => {
    // The selected projection is intentionally a dependency: edits/undo replace its immutable object and
    // refresh this richer native read model without making React parse AnimationTrack storage itself.
    void selectedProjection;
    const epoch = ++refreshEpoch.current;
    try {
      const next = await client.animationState(selectedId);
      if (epoch !== refreshEpoch.current) return;
      setModel(next);
      setError(null);
    } catch (cause) {
      if (epoch === refreshEpoch.current) setError(errorText(cause, "Animation service is unavailable."));
    }
  }, [client, selectedId, selectedProjection]);

  useEffect(() => {
    void refresh();
    return () => {
      refreshEpoch.current += 1;
    };
  }, [refresh]);

  useEffect(() => {
    clockAnchor.current = { tick: model.currentTick, at: performance.now() };
    setDisplayTick(model.currentTick);
    const percent = (Math.max(0, Math.min(model.currentTick, Math.max(1, model.durationTick))) / Math.max(1, model.durationTick)) * 100;
    workspaceRef.current?.style.setProperty("--animation-playhead", `${percent}%`);
  }, [model.currentTick, model.durationTick, model.playing]);

  // Smooth playhead motion is extrapolated locally from an authoritative native clock snapshot. This is
  // presentation only: keys/events still use native integer ticks and every transport response reconciles.
  useEffect(() => {
    if (!model.playing) return;
    let frame = 0;
    let lastReactUpdate = 0;
    const animate = (now: number) => {
      const elapsed = ((now - clockAnchor.current.at) / 1000) * model.ticksPerSecond;
      const raw = clockAnchor.current.tick + elapsed;
      const duration = Math.max(1, model.durationTick);
      let local = raw;
      if (model.loopPolicy === "loop") local = ((raw % duration) + duration) % duration;
      else if (model.loopPolicy === "ping_pong") {
        const period = duration * 2;
        const phase = ((raw % period) + period) % period;
        local = phase <= duration ? phase : period - phase;
      } else local = Math.min(duration, raw);
      workspaceRef.current?.style.setProperty("--animation-playhead", `${(local / duration) * 100}%`);
      // Keep the DOM playhead at display refresh rate while limiting expensive React curve/preview
      // recomputation to 30 Hz. Authoritative transport still reconciles every 500 ms.
      if (now - lastReactUpdate >= 1000 / 30) {
        lastReactUpdate = now;
        setDisplayTick(Math.round(local));
      }
      frame = window.requestAnimationFrame(animate);
    };
    frame = window.requestAnimationFrame(animate);
    return () => window.cancelAnimationFrame(frame);
  }, [model.durationTick, model.loopPolicy, model.playing, model.ticksPerSecond]);

  // Reconcile through the lightweight clock/event contract; static tracks/properties/assets are fetched
  // only on selection or authored-document changes, not ten times per second.
  useEffect(() => {
    // A transient imported-clip lease remains authoritative even if a concurrent static refresh
    // briefly reports the authored transport as stopped. Keep polling until native explicitly
    // reports that the transient projection has been released.
    if (!model.playing && clipPreviewId === null) return;
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const response = await client.animationPlaybackState();
        if (cancelled) return;
        const importedClipPreviewFinished =
          response.ok
          && clipPreviewId !== null
          && response.importedClipPreviewActive === false;
        if (importedClipPreviewFinished) {
          clipPreviewAttempt.current = null;
          clipPreviewRequestId.current = null;
          clipPreviewMayHaveReachedNative.current = false;
          clipPreviewActive.current = false;
          setClipPreviewPending(false);
          setClipPreviewId(null);
          setDisplayTick(response.currentTick);
          setError(null);
        } else if (!response.ok) {
          // Unavailable fallbacks deliberately report `active: false` because native truth is
          // unknown. Retain the token and poller until a successful response confirms teardown.
          setError(response.message);
        }
        setModel((current) => ({ ...current, currentTick: response.currentTick, durationTick: response.durationTick, playing: response.playing, loopPolicy: response.loopPolicy }));
        const eventMessage = crossedEventStatus(response);
        if (!response.ok) {
          setStatus(response.message);
        } else if (importedClipPreviewFinished) {
          setStatus(eventMessage
            ? `Imported clip preview finished; authored transforms were restored. ${eventMessage}`
            : "Imported clip preview finished; authored transforms and normal graph authority were restored.");
        } else if (eventMessage) setStatus(eventMessage);
      } catch (cause) {
        if (!cancelled) setError(errorText(cause, "Animation clock synchronization failed."));
      }
      if (!cancelled) timer = window.setTimeout(() => void poll(), 500);
    };
    timer = window.setTimeout(() => void poll(), 500);
    return () => { cancelled = true; window.clearTimeout(timer); };
  }, [client, clipPreviewId, model.playing]);

  const contextProperties = useMemo(
    () => model.properties.filter((property) => inferredContext(property) === activeContext),
    [activeContext, model.properties],
  );
  const contextTracks = useMemo(
    () => model.tracks.filter((track) => inferredContext(track) === activeContext),
    [activeContext, model.tracks],
  );
  const previewValues = useMemo(() => {
    const values: Record<string, Json> = {};
    for (const property of contextProperties) values[`${property.component}.${property.property}`] = property.value;
    for (const track of contextTracks) {
      const sampled = sampleTrack(track, displayTick);
      if (sampled !== null) values[`${track.component}.${track.property}`] = sampled;
    }
    return values;
  }, [contextProperties, contextTracks, displayTick]);
  const query = contextView.search.trim().toLocaleLowerCase();
  const visibleTracks = useMemo(
    () => contextTracks.filter((track) => !query || [track.name, track.targetName, track.component, track.property, track.bindingReason]
      .some((value) => value?.toLocaleLowerCase().includes(query))),
    [contextTracks, query],
  );

  const propertyDraft = draftString(contextView.drafts[PROPERTY_DRAFT]);
  const currentProperty = contextProperties.find((property) => propertyId(property) === propertyDraft)
    ?? contextProperties.find((property) => property.animatable)
    ?? contextProperties[0]
    ?? null;
  useEffect(() => {
    const next = currentProperty ? propertyId(currentProperty) : "";
    if (propertyDraft === next) return;
    const store = animationEditorStore.getState();
    if (next) store.setDraft(workspaceKey, activeContext, PROPERTY_DRAFT, next);
    else store.removeDraft(workspaceKey, activeContext, PROPERTY_DRAFT);
  }, [activeContext, currentProperty, propertyDraft, workspaceKey]);

  const interpolationDraft = draftString(contextView.drafts[INTERPOLATION_DRAFT]);
  const interpolation: AnimationInterpolation = currentProperty && ["bool", "string"].includes(currentProperty.valueKind)
    ? "step"
    : interpolationDraft === "step" || interpolationDraft === "cubic" ? interpolationDraft : "linear";

  const selectedKeyId = contextView.selectedKeyIds[0] ?? null;
  const selectedKeyLocation = useMemo(() => {
    if (!selectedKeyId) return null;
    for (const track of contextTracks) {
      const key = track.keys.find((item) => keySelectionId(track.id, item.id) === selectedKeyId);
      if (key) return { track, key };
    }
    return null;
  }, [contextTracks, selectedKeyId]);
  const selectedTrack = selectedKeyLocation?.track
    ?? contextTracks.find((track) => contextView.selectedTrackIds.includes(track.id))
    ?? null;
  const selectedKey = selectedKeyLocation?.key ?? null;

  useEffect(() => {
    const trackIds = contextView.selectedTrackIds.filter((id) => contextTracks.some((track) => track.id === id));
    const keyIds = contextView.selectedKeyIds.filter((id) => contextTracks.some((track) => track.keys.some((key) => keySelectionId(track.id, key.id) === id)));
    if (trackIds.length === contextView.selectedTrackIds.length && keyIds.length === contextView.selectedKeyIds.length) return;
    animationEditorStore.getState().setSelection(workspaceKey, activeContext, trackIds, keyIds);
  }, [activeContext, contextTracks, contextView.selectedKeyIds, contextView.selectedTrackIds, workspaceKey]);

  useEffect(() => {
    const timeline = timelineRef.current;
    if (!timeline) return;
    timeline.scrollLeft = contextView.scroll.x;
    timeline.scrollTop = contextView.scroll.y;
  }, [activeContext, contextView.scroll.x, contextView.scroll.y, workspaceKey, contextView.view]);

  async function runEdit(operation: () => Promise<AnimationEditResult>, after?: (result: AnimationEditResult) => void) {
    const epoch = ++editEpoch.current;
    refreshEpoch.current += 1;
    setBusy(true);
    try {
      const result = await operation();
      if (epoch !== editEpoch.current) return;
      setModel(result.state);
      setError(result.ok ? null : result.message);
      setStatus(result.message);
      if (result.ok) after?.(result);
    } catch (cause) {
      if (epoch === editEpoch.current) setError(errorText(cause, "The animation edit could not be completed."));
    } finally {
      if (epoch === editEpoch.current) setBusy(false);
    }
  }

  function clipMappingValidation(clip: AnimationImportedClipInfo): {
    complete: boolean;
    message: string | null;
    bySource: Record<string, string>;
  } {
    const sourceTargetIds = clipSourceTargetIds(clip);
    const bySource: Record<string, string> = {};
    if (clip.sourceTargets.length > 1 && sourceTargetIds.length !== clip.sourceTargets.length) {
      return {
        complete: false,
        message: "Stable source-node identities are unavailable. Reimport this clip before mapping it.",
        bySource,
      };
    }
    const repairing = clipDraftFence.current?.clipId === clip.clipId
      && clipDraftFence.current.mode === "repair";
    if (sourceTargetIds.length <= 1 && !repairing) {
      const complete = Boolean(selectedId && liveTransformTargetIds.has(selectedId));
      return {
        complete,
        message: complete ? null : "The selected entity is missing or no longer has a live Transform.",
        bySource,
      };
    }
    if (new Set(sourceTargetIds).size !== sourceTargetIds.length) {
      return {
        complete: false,
        message: "The imported source contains duplicate node identities. Reimport it before mapping.",
        bySource,
      };
    }
    const usedTargets = new Map<string, string[]>();
    for (const sourceTargetId of sourceTargetIds) {
      const targetId = clipTargetMappings[sourceTargetId] ?? "";
      if (!targetId) {
        bySource[sourceTargetId] = "Choose a live scene target.";
        continue;
      }
      if (!liveTransformTargetIds.has(targetId)) {
        bySource[sourceTargetId] = "This target is missing, inactive, or no longer has a Transform.";
        continue;
      }
      const owners = usedTargets.get(targetId) ?? [];
      owners.push(sourceTargetId);
      usedTargets.set(targetId, owners);
    }
    for (const owners of usedTargets.values()) {
      if (owners.length < 2) continue;
      for (const sourceTargetId of owners) {
        bySource[sourceTargetId] = "Choose a distinct target; another source node already uses this entity.";
      }
    }
    const missingCount = sourceTargetIds.filter((sourceTargetId) => !clipTargetMappings[sourceTargetId]).length;
    const conflictCount = Object.values(bySource).filter((issue) => issue.includes("distinct")).length;
    const staleCount = Object.values(bySource).filter((issue) => issue.includes("missing, inactive")).length;
    const message = missingCount > 0
      ? `${missingCount} source node${missingCount === 1 ? "" : "s"} still need a target.`
      : conflictCount > 0
        ? "Every source node must map to a distinct scene entity."
        : staleCount > 0
          ? "One or more mapped targets are no longer available."
          : null;
    return { complete: Object.keys(bySource).length === 0, message, bySource };
  }

  function openClipSetup(
    event: ReactMouseEvent<HTMLButtonElement>,
    clip: AnimationImportedClipInfo,
    intent: "create" | "repair" = clip.readiness === "repair_required" ? "repair" : "create",
  ) {
    const asset = model.asset;
    if (!asset?.logicalId || !asset.revisionId) {
      setError("This imported asset has no stable revision identity. Reimport it before mapping.");
      return;
    }
    if (intent === "repair" && !clip.instanceId) {
      setError("This stale mapping has no persisted instance identity. Reimport the asset before repairing it.");
      return;
    }
    clipSetupTrigger.current = event.currentTarget;
    const requestToken = globalThis.crypto?.randomUUID?.()
      ?? `${Date.now()}-${Math.round(Math.random() * 1_000_000)}`;
    clipDraftRequestToken.current = requestToken;
    clipDraftFence.current = {
      clipId: clip.clipId,
      mode: intent,
      instanceId: intent === "repair" && clip.instanceId ? clip.instanceId : `clip-${requestToken}`,
      clipInstanceRevision: asset.clipInstanceRevision,
      logicalAssetId: asset.logicalId,
      assetRevision: asset.revisionId,
      sourceBindingHash: clip.sourceBindingHash,
      sourceTargetIds: [...(clip.sourceTargetIds ?? [])],
      sourceTargetLabels: (clip.sourceTargetIds ?? []).map((sourceTargetId, index) =>
        clip.sourceBindings?.find((binding) => binding.sourceTargetId === sourceTargetId)?.sourceTargetLabel
          ?? clip.sourceTargets[index]
          ?? sourceTargetId
      ),
      sourceBindings: (clip.sourceBindings ?? []).map((binding) => ({ ...binding })),
      repairChanges: [...(clip.repairChanges ?? [])],
    };
    const sourceTargetIds = clipSourceTargetIds(clip);
    const initialMappings: Record<string, string> = {};
    const persistedMappings = new Map((clip.targetMappings ?? []).map((mapping) => [
      mapping.sourceTargetId,
      mapping.targetId,
    ]));
    sourceTargetIds.forEach((sourceTargetId, index) => {
      const persistedTarget = intent === "repair" ? persistedMappings.get(sourceTargetId) : null;
      initialMappings[sourceTargetId] = persistedTarget && liveTransformTargetIds.has(persistedTarget)
        ? persistedTarget
        : intent === "create" && index === 0 && selectedId && liveTransformTargetIds.has(selectedId)
          ? selectedId
          : "";
    });
    setClipTargetMappings(initialMappings);
    setClipSetupId(clip.clipId);
    setError(null);
  }

  async function updateClipTargetMapping(sourceTargetId: string, targetId: string) {
    if (clipPreviewActive.current) {
      await stopClipPreview();
      if (clipPreviewActive.current) return;
    }
    setClipTargetMappings((current) => ({ ...current, [sourceTargetId]: targetId }));
  }

  async function suggestExactClipTargets(clip: AnimationImportedClipInfo) {
    if (clipPreviewActive.current) {
      await stopClipPreview();
      if (clipPreviewActive.current) return;
    }
    const sourceTargetIds = clipSourceTargetIds(clip);
    let filled = 0;
    const next = { ...clipTargetMappings };
    const usedTargets = new Set(Object.values(next).filter(Boolean));
    sourceTargetIds.forEach((sourceTargetId, index) => {
      if (next[sourceTargetId]) return;
      const sourceName = normalizedExactSceneName(clipSourceTargetLabel(clip, sourceTargetId, index));
      const candidates = liveTransformTargets.filter((target) =>
        !usedTargets.has(target.id) && normalizedExactSceneName(target.name) === sourceName
      );
      if (candidates.length !== 1) return;
      next[sourceTargetId] = candidates[0].id;
      usedTargets.add(candidates[0].id);
      filled += 1;
    });
    setClipTargetMappings(next);
    setStatus(filled > 0
      ? `Suggested ${filled} unique exact-name target${filled === 1 ? "" : "s"}. Review before previewing.`
      : "No unique exact-name targets were available; unresolved rows were left unchanged.");
  }

  function clipInstanceDraftRequest(clip: AnimationImportedClipInfo): AnimationClipInstanceSaveRequest | null {
    const asset = model.asset;
    const fence = clipDraftFence.current;
    if (!selectedId || !asset?.logicalId || !asset.revisionId || !fence || fence.clipId !== clip.clipId) {
      setError("Select the imported asset entity before setting up its clip.");
      return null;
    }
    if (clipDraftIsStale(clip)) {
      setError("The asset or clip-instance set changed while this mapper was open. Close and reopen it before previewing or saving.");
      return null;
    }
    const validation = clipMappingValidation(clip);
    if (!validation.complete) {
      setError(validation.message ?? "Complete every source-node mapping before continuing.");
      return null;
    }
    const requestToken = clipDraftRequestToken.current
      ?? (globalThis.crypto?.randomUUID?.() ?? `${Date.now()}-${Math.round(Math.random() * 1_000_000)}`);
    clipDraftRequestToken.current = requestToken;
    const sourceTargetIds = fence.sourceTargetIds;
    return {
      expectedRevision: fence.clipInstanceRevision,
      requestId: `setup-${requestToken}`,
      instanceId: fence.instanceId,
      name: `${asset.displayName} - ${clip.name}`,
      logicalAssetId: fence.logicalAssetId,
      expectedAssetRevision: fence.assetRevision,
      clipId: clip.clipId,
      expectedSourceBindingHash: fence.sourceBindingHash,
      ...(sourceTargetIds.length > 1 || fence.mode === "repair"
        ? {
            targetMappings: sourceTargetIds.map((sourceTargetId) => ({
              sourceTargetId,
              targetId: clipTargetMappings[sourceTargetId],
            })),
          }
        : {}),
      targetId: selectedId,
    };
  }

  function closeClipSetup() {
    setClipSetupId(null);
    setClipPreviewPending(false);
    setClipTargetMappings({});
    clipDraftRequestToken.current = null;
    clipDraftFence.current = null;
    window.setTimeout(() => clipSetupTrigger.current?.focus(), 0);
  }

  async function cancelPendingClipPreviewAndClose() {
    const epoch = ++transportEpoch.current;
    if (clipPreviewAttempt.current) clipPreviewAttempt.current.cancelled = true;
    const shouldStop = clipPreviewMayHaveReachedNative.current || clipPreviewActive.current;
    const expectedRequestId = clipPreviewRequestId.current ?? undefined;
    setBusy(true);
    setError(null);
    try {
      if (shouldStop) {
        const result = await client.animationClipInstancePreviewStop(expectedRequestId);
        if (epoch !== transportEpoch.current) return;
        setModel((current) => ({
          ...current,
          currentTick: result.currentTick,
          durationTick: result.durationTick,
          playing: result.playing,
          loopPolicy: result.loopPolicy,
        }));
        setDisplayTick(result.currentTick);
        setStatus(result.message);
        const restorationConfirmed =
          result.ok && result.importedClipPreviewActive !== true;
        if (!restorationConfirmed) {
          setError(result.ok
            ? "A native imported-clip preview is still active. The mapper retained its cleanup token; retry Cancel."
            : result.message);
          return;
        }
      }
      if (epoch !== transportEpoch.current) return;
      clipPreviewAttempt.current = null;
      clipPreviewRequestId.current = null;
      clipPreviewMayHaveReachedNative.current = false;
      clipPreviewActive.current = false;
      setClipPreviewId(null);
      closeClipSetup();
    } catch (cause) {
      if (epoch === transportEpoch.current) {
        setError(errorText(
          cause,
          "Preview cancellation was not confirmed. The mapper retained its cleanup token; retry Cancel.",
        ));
      }
    } finally {
      if (epoch === transportEpoch.current) setBusy(false);
    }
  }

  function requestCloseClipSetup() {
    if (busy && !clipPreviewPending) return;
    if (clipPreviewPending) {
      void cancelPendingClipPreviewAndClose();
      return;
    }
    if (clipPreviewActive.current) void stopClipPreview(true);
    else closeClipSetup();
  }

  async function saveClipInstance(clip: AnimationImportedClipInfo) {
    const request = clipInstanceDraftRequest(clip);
    if (!request) return;
    const epoch = ++editEpoch.current;
    refreshEpoch.current += 1;
    setBusy(true);
    try {
      const result = await client.animationClipInstanceSave(request);
      if (epoch !== editEpoch.current) return;
      setModel(result.state);
      setError(result.ok ? null : result.message);
      setStatus(result.message);
      if (result.ok) {
        clipPreviewAttempt.current = null;
        clipPreviewRequestId.current = null;
        clipPreviewMayHaveReachedNative.current = false;
        clipPreviewActive.current = false;
        setClipPreviewPending(false);
        setClipPreviewId(null);
        closeClipSetup();
      }
    } catch (cause) {
      if (epoch === editEpoch.current) {
        setError(errorText(cause, "The imported clip mapping could not be saved."));
      }
    } finally {
      if (epoch === editEpoch.current) setBusy(false);
    }
  }

  async function discardClipInstance(instanceId: string) {
    const expectedRevision = model.asset?.clipInstanceRevision;
    if (!expectedRevision) {
      setError("Clip-instance revision evidence is unavailable. Refresh before discarding.");
      return;
    }
    const epoch = ++editEpoch.current;
    refreshEpoch.current += 1;
    setBusy(true);
    try {
      const result = await client.animationClipInstanceDelete(
        instanceId,
        expectedRevision,
        selectedId,
      );
      if (epoch !== editEpoch.current) return;
      setModel(result.state);
      setError(result.ok ? null : result.message);
      setStatus(result.message);
      if (result.ok) setClipDiscardConfirmId(null);
    } catch (cause) {
      if (epoch === editEpoch.current) {
        setError(errorText(cause, "The clip instance could not be discarded."));
      }
    } finally {
      if (epoch === editEpoch.current) setBusy(false);
    }
  }

  async function previewClipInstance(clip: AnimationImportedClipInfo) {
    const request = clipInstanceDraftRequest(clip);
    if (!request) return;
    const previewAttemptToken = globalThis.crypto?.randomUUID?.()
      ?? `${Date.now()}-${Math.round(Math.random() * 1_000_000)}`;
    request.requestId = `${request.requestId}:preview:${previewAttemptToken}`;
    const epoch = ++transportEpoch.current;
    const attempt = { epoch, cancelled: false };
    clipPreviewAttempt.current = attempt;
    clipPreviewRequestId.current = request.requestId;
    clipPreviewMayHaveReachedNative.current = true;
    setClipPreviewPending(true);
    setBusy(true);
    try {
      const result = await client.animationClipInstancePreview(request);
      if (attempt.cancelled || epoch !== transportEpoch.current) {
        if (result.ok) {
          void client.animationClipInstancePreviewStop(request.requestId).catch(() => {});
        }
        return;
      }
      clipPreviewAttempt.current = null;
      setClipPreviewPending(false);
      if (!result.ok) {
        if (clipPreviewRequestId.current === request.requestId) {
          clipPreviewRequestId.current = null;
        }
        clipPreviewMayHaveReachedNative.current = false;
        const failureRefreshEpoch = ++refreshEpoch.current;
        try {
          const current = await client.animationState(selectedId);
          if (
            epoch !== transportEpoch.current
            || failureRefreshEpoch !== refreshEpoch.current
          ) return;
          setModel(current);
        } catch {
          // Keep the native preview rejection as the actionable mapper error even if refresh also fails.
        }
        if (epoch !== transportEpoch.current) return;
        setError(result.message);
        setStatus(result.message);
        return;
      }
      setModel((current) => ({
        ...current,
        currentTick: result.currentTick,
        durationTick: result.durationTick,
        playing: result.playing,
        loopPolicy: result.loopPolicy,
      }));
      setDisplayTick(result.currentTick);
      setError(null);
      setStatus(result.message);
      clipPreviewActive.current = true;
      setClipPreviewId(clip.clipId);
    } catch (cause) {
      if (attempt.cancelled || epoch !== transportEpoch.current) return;
      let restorationConfirmed = false;
      try {
        const cleanup = await client.animationClipInstancePreviewStop(request.requestId);
        restorationConfirmed = cleanup.ok && cleanup.importedClipPreviewActive !== true;
        if (restorationConfirmed) {
          setModel((current) => ({
            ...current,
            currentTick: cleanup.currentTick,
            durationTick: cleanup.durationTick,
            playing: cleanup.playing,
            loopPolicy: cleanup.loopPolicy,
          }));
          setDisplayTick(cleanup.currentTick);
        }
      } catch {
        // The preview request may have reached native before its response was lost. Stop is
        // request-token fenced, so retaining ownership here cannot interrupt a newer preview.
      }
      if (attempt.cancelled || epoch !== transportEpoch.current) return;
      clipPreviewMayHaveReachedNative.current = !restorationConfirmed;
      clipPreviewActive.current = !restorationConfirmed;
      if (restorationConfirmed) {
        if (clipPreviewRequestId.current === request.requestId) {
          clipPreviewRequestId.current = null;
        }
        setClipPreviewId(null);
      } else {
        setClipPreviewId(clip.clipId);
      }
      clipPreviewAttempt.current = null;
      setClipPreviewPending(false);
      const baseMessage = errorText(cause, "The imported clip could not be previewed.");
      const message = restorationConfirmed
        ? baseMessage
        : `${baseMessage} Authored-transform restoration was not confirmed; use Stop preview to retry.`;
      const failureRefreshEpoch = ++refreshEpoch.current;
      try {
        const current = await client.animationState(selectedId);
        if (
          epoch !== transportEpoch.current
          || failureRefreshEpoch !== refreshEpoch.current
        ) return;
        setModel(current);
      } catch {
        // Preserve the preview failure below; it is more directly actionable in this mapper.
      }
      if (epoch === transportEpoch.current) setError(message);
    } finally {
      if (epoch === transportEpoch.current) {
        setClipPreviewPending(false);
        setBusy(false);
      }
    }
  }

  async function stopClipPreview(closeAfter = false) {
    const epoch = ++transportEpoch.current;
    if (clipPreviewAttempt.current) clipPreviewAttempt.current.cancelled = true;
    clipPreviewAttempt.current = null;
    const expectedRequestId = clipPreviewRequestId.current ?? undefined;
    setClipPreviewPending(false);
    setBusy(true);
    try {
      const result = await client.animationClipInstancePreviewStop(expectedRequestId);
      if (epoch !== transportEpoch.current) return;
      setModel((current) => ({
        ...current,
        currentTick: result.currentTick,
        durationTick: result.durationTick,
        playing: result.playing,
        loopPolicy: result.loopPolicy,
      }));
      setDisplayTick(result.currentTick);
      setStatus(result.message);
      const restorationConfirmed =
        result.ok && result.importedClipPreviewActive !== true;
      setError(restorationConfirmed
        ? null
        : result.ok
          ? "A native imported-clip preview is still active. Retry Stop preview."
          : result.message);
      if (restorationConfirmed) {
        clipPreviewMayHaveReachedNative.current = false;
        clipPreviewRequestId.current = null;
        clipPreviewActive.current = false;
        setClipPreviewId(null);
        if (closeAfter) closeClipSetup();
      }
    } catch (cause) {
      if (epoch === transportEpoch.current) {
        setError(errorText(cause, "Authored transforms could not be confirmed restored."));
      }
    } finally {
      if (epoch === transportEpoch.current) setBusy(false);
    }
  }

  function stopAnimation() {
    if (
      clipPreviewPending
      || clipPreviewMayHaveReachedNative.current
      || clipPreviewActive.current
      || clipPreviewRequestId.current !== null
    ) {
      void stopClipPreview();
      return;
    }
    void transport("stop");
  }

  async function transport(action: "play" | "pause" | "stop" | "scrub", tick?: number, loopPolicy?: AnimationLoopPolicy) {
    const epoch = ++transportEpoch.current;
    try {
      const response = await client.animationTransport(action, tick, loopPolicy);
      if (epoch !== transportEpoch.current) return;
      setModel((current) => ({
        ...current,
        currentTick: response.currentTick,
        durationTick: response.durationTick,
        playing: response.playing,
        loopPolicy: response.loopPolicy,
      }));
      const importedClipRestorationConfirmed =
        response.ok && response.importedClipPreviewActive !== true;
      if (action === "stop" && importedClipRestorationConfirmed) {
        if (clipPreviewAttempt.current) clipPreviewAttempt.current.cancelled = true;
        clipPreviewAttempt.current = null;
        clipPreviewRequestId.current = null;
        clipPreviewMayHaveReachedNative.current = false;
        clipPreviewActive.current = false;
        setClipPreviewPending(false);
        setClipPreviewId(null);
      }
      setError(response.ok
        ? action === "stop" && !importedClipRestorationConfirmed
          ? "A native imported-clip preview is still active. Retry Stop."
          : null
        : response.message);
      const eventMessage = crossedEventStatus(response);
      if (eventMessage) setStatus(eventMessage);
      else if (action !== "scrub") setStatus(response.message);
    } catch (cause) {
      if (epoch === transportEpoch.current) setError(errorText(cause, "Animation preview is unavailable."));
    }
  }

  function keyCurrent() {
    if (!selectedId || !currentProperty || !currentProperty.animatable) return;
    void runEdit(() => client.animationKey(
      selectedId,
      currentProperty.component,
      currentProperty.property,
      displayTick,
      interpolation,
    ), (result) => {
      if (!result.trackId || !result.keyId) return;
      const identity = keySelectionId(result.trackId, result.keyId);
      animationEditorStore.getState().setSelection(workspaceKey, activeContext, [result.trackId], [identity], identity);
    });
  }

  function selectedKeyLocations(extra?: { track: AnimationTrackInfo; key: AnimationKeyInfo }) {
    const selectedIds = new Set(contextView.selectedKeyIds);
    if (extra && !selectedIds.has(keySelectionId(extra.track.id, extra.key.id))) {
      return [extra];
    }
    return contextTracks.flatMap((track) => track.keys
      .filter((key) => selectedIds.has(keySelectionId(track.id, key.id)))
      .map((key) => ({ track, key })));
  }

  function deleteSelectedKey(extra?: { track: AnimationTrackInfo; key: AnimationKeyInfo }) {
    const locations = selectedKeyLocations(extra);
    if (locations.length === 0) return;
    if (locations.some(({ track }) => track.locked)) {
      setStatus("Unlock every selected track before deleting its keys.");
      return;
    }
    const touchedTrackIds = [...new Set(locations.map(({ track }) => track.id))];
    const deletes: AnimationKeyDeleteInfo[] = locations.map(({ track, key }) => ({ targetId: track.targetId, trackId: track.id, keyId: key.id }));
    void runEdit(() => client.animationDeleteKeys(selectedId, deletes), (result) => {
      if (result.ok) animationEditorStore.getState().setSelection(workspaceKey, activeContext, touchedTrackIds, []);
    });
  }

  function updateSelectedKey(update: Omit<AnimationKeyUpdateInfo, "keyId">) {
    if (!selectedKeyLocation || selectedKeyLocation.track.locked) return;
    const { track, key } = selectedKeyLocation;
    void runEdit(() => client.animationUpdateKeys(track.targetId, track.id, [{ keyId: key.id, ...update }]));
  }

  function changeInterpolation(track: AnimationTrackInfo, next: AnimationInterpolation) {
    if (track.locked) return;
    void runEdit(() => client.animationSetInterpolation(track.targetId, track.id, next));
  }

  function changeTrackEnabled(track: AnimationTrackInfo) {
    if (track.locked) return;
    void runEdit(() => client.animationSetTrackEnabled(track.targetId, track.id, !track.enabled));
  }

  function changeTrackLocked(track: AnimationTrackInfo) {
    void runEdit(() => client.animationSetTrackLocked(track.targetId, track.id, !track.locked));
  }

  function selectKey(track: AnimationTrackInfo, key: AnimationKeyInfo, event?: {
    ctrlKey?: boolean;
    metaKey?: boolean;
    shiftKey?: boolean;
    stopPropagation?: () => void;
  }) {
    event?.stopPropagation?.();
    const additive = Boolean(event?.ctrlKey || event?.metaKey);
    const selected = contextView.selectedKeyIds;
    const identity = keySelectionId(track.id, key.id);
    let nextKeyIds: readonly string[] = [identity];
    let nextAnchor: string | null = identity;

    if (event?.shiftKey) {
      const ordered = [...track.keys].sort((left, right) => left.tick - right.tick || left.id.localeCompare(right.id));
      const anchorIndex = ordered.findIndex((item) => keySelectionId(track.id, item.id) === contextView.selectionAnchorKeyId);
      const keyIndex = ordered.findIndex((item) => item.id === key.id);
      if (anchorIndex >= 0 && keyIndex >= 0) {
        const start = Math.min(anchorIndex, keyIndex);
        const end = Math.max(anchorIndex, keyIndex);
        const rangeIds = ordered.slice(start, end + 1).map((item) => keySelectionId(track.id, item.id));
        nextKeyIds = additive ? [...new Set([...selected, ...rangeIds])] : rangeIds;
        nextAnchor = contextView.selectionAnchorKeyId;
      }
    } else if (additive) {
      const removing = selected.includes(identity);
      nextKeyIds = removing ? selected.filter((id) => id !== identity) : [...selected, identity];
      nextAnchor = removing ? contextView.selectionAnchorKeyId : identity;
    }

    const nextKeySet = new Set(nextKeyIds);
    const nextTrackIds = contextTracks
      .filter((candidate) => candidate.keys.some((candidateKey) => nextKeySet.has(keySelectionId(candidate.id, candidateKey.id))))
      .map((candidate) => candidate.id);
    animationEditorStore.getState().setSelection(workspaceKey, activeContext, nextTrackIds, nextKeyIds, nextAnchor);
    void transport("scrub", key.tick);
  }

  function addMarker() {
    if (!selectedId) return;
    const name = draftString(contextView.drafts[MARKER_NAME_DRAFT]);
    void runEdit(() => client.animationAddMarker(selectedId, name, displayTick), () => {
      animationEditorStore.getState().removeDraft(workspaceKey, activeContext, MARKER_NAME_DRAFT);
    });
  }

  function addEvent() {
    if (!selectedId) return;
    const name = draftString(contextView.drafts[EVENT_NAME_DRAFT]);
    const payloadText = draftString(contextView.drafts[EVENT_PAYLOAD_DRAFT]).trim();
    let payload: Json | undefined;
    if (payloadText) {
      try {
        payload = JSON.parse(payloadText) as Json;
      } catch {
        payload = payloadText;
      }
    }
    void runEdit(() => client.animationAddEvent(selectedId, name, displayTick, payload), () => {
      const store = animationEditorStore.getState();
      store.removeDraft(workspaceKey, activeContext, EVENT_NAME_DRAFT);
      store.removeDraft(workspaceKey, activeContext, EVENT_PAYLOAD_DRAFT);
    });
  }

  const duration = Math.max(1, model.durationTick);
  const timelineWidth = Math.max(620, Math.min(12_000, (duration / Math.max(1, model.ticksPerSecond)) * 160 * contextView.zoom));
  const activeReadiness = contextReadiness(model, activeContext);
  const contextTabs = ANIMATION_CONTEXTS.map((context) => {
    const readiness = contextReadiness(model, context);
    return {
      id: context,
      label: CONTEXT_LABELS[context],
      badge: readiness.state === "ready" ? readiness.tracks || readiness.properties : readiness.state.replace("_", " "),
      tooltip: readiness.reason,
    };
  });

  function snapTick(tick: number): number {
    const snap = contextView.snap;
    if (!snap.enabled) return Math.max(0, Math.min(duration, Math.round(tick)));
    const unit = snap.grid === "seconds"
      ? model.ticksPerSecond
      : snap.grid === "frames"
        ? model.ticksPerSecond / 60
        : 1;
    const step = Math.max(1, unit * snap.increment);
    let snapped = Math.round(tick / step) * step;
    const candidates = [
      ...(snap.toKeys ? contextTracks.flatMap((track) => track.keys.map((key) => key.tick)) : []),
      ...(snap.toMarkers ? model.markers.map((marker) => marker.tick) : []),
    ];
    const nearest = candidates.sort((left, right) => Math.abs(left - tick) - Math.abs(right - tick))[0];
    const magnetTolerance = Math.max(1, (duration / Math.max(1, timelineWidth)) * 8);
    if (nearest !== undefined && Math.abs(nearest - tick) <= magnetTolerance) snapped = nearest;
    return Math.max(0, Math.min(duration, Math.round(snapped)));
  }

  function scrubLane(event: ReactMouseEvent<HTMLDivElement>) {
    if (event.target !== event.currentTarget) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0) return;
    const tick = ((event.clientX - bounds.left) / bounds.width) * duration;
    void transport("scrub", snapTick(tick));
  }

  function onWorkspaceKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (isInteractiveTarget(event.target)) {
      // Keep W/E/R/F, Space, Delete, and arrow keys owned by the focused editor control, not App's viewport.
      event.stopPropagation();
      return;
    }
    if (["w", "e", "r", "f"].includes(event.key.toLocaleLowerCase())) {
      // Once the timeline itself owns focus, viewport tool shortcuts must remain isolated too.
      event.stopPropagation();
    } else if (event.key === " ") {
      event.preventDefault();
      event.stopPropagation();
      void transport(model.playing ? "pause" : "play");
    } else if ((event.key === "Delete" || event.key === "Backspace") && selectedKey) {
      event.preventDefault();
      event.stopPropagation();
      deleteSelectedKey();
    } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
      event.preventDefault();
      event.stopPropagation();
      const direction = event.key === "ArrowRight" ? 1 : -1;
      const frame = Math.max(1, Math.round(model.ticksPerSecond / 60));
      void transport("scrub", Math.max(0, Math.min(duration, displayTick + direction * frame)));
    }
  }

  return (
    <div
      ref={workspaceRef}
      data-testid="animation-workspace"
      className="animation-workspace-root"
      role="region"
      aria-label="Animation editor"
      tabIndex={0}
      onKeyDown={onWorkspaceKeyDown}
      style={{ display: "grid", gridTemplateRows: "auto auto minmax(0, 1fr)", height: "100%", minHeight: 300, color: color.text.primary, font: font.ui }}
    >
      <div style={{ display: "flex", alignItems: "center", minWidth: 0, borderBottom: `1px solid ${color.border.subtle}`, background: color.bg.panel }}>
        <div style={{ flex: "1 1 auto", minWidth: 0 }}>
          <DockTabs
            id="animation-contexts"
            ariaLabel="Animation context"
            tabs={contextTabs}
            activeId={activeContext}
            onChange={(id) => animationEditorStore.getState().setActiveContext(workspaceKey, id as AnimationContext)}
          />
        </div>
        <DockTabs
          id="animation-surfaces"
          ariaLabel="Animation authoring surface"
          tabs={[
            { id: "timeline", label: "Timeline" },
            { id: "curves", label: "Curves", tooltip: "Numeric key values and cubic tangents" },
            { id: "graph", label: "Graph", tooltip: "Blend, layer, state-machine, and runtime-debug graph" },
          ]}
          activeId={activeSurface}
          onChange={(id) => {
            const surface = id as "timeline" | "curves" | "graph";
            animationEditorStore.getState().setActiveSurface(graphWorkspaceKey, surface);
            if (surface !== "graph") animationEditorStore.getState().setView(workspaceKey, activeContext, surface);
          }}
        />
        <span style={{ padding: `0 ${space.sm}px`, color: color.text.muted, fontSize: fontSize.meta, whiteSpace: "nowrap" }}>
          One sequence · {model.sequenceName} · rev {model.revision || "legacy"}
        </span>
      </div>

      <div style={{ display: "flex", alignItems: "center", gap: space.sm, padding: space.sm, borderBottom: `1px solid ${color.border.subtle}`, background: color.bg.raised, overflowX: "auto" }}>
        <Button compact aria-label={model.playing ? "Pause animation" : "Play animation"} data-testid="animation-play" onClick={() => void transport(model.playing ? "pause" : "play")}>
          {model.playing ? "Pause" : "Play"}
        </Button>
        <Button compact variant="ghost" data-testid="animation-stop" onClick={stopAnimation}>Stop</Button>
        <span style={{ font: font.mono, fontSize: fontSize.body, minWidth: 122 }} data-testid="animation-time">
          {timeLabel(displayTick, model.ticksPerSecond, contextView.timeDisplay)} · {displayTick}t
        </span>
        <input
          aria-label="Animation timeline"
          data-testid="animation-scrub"
          type="range"
          min={0}
          max={duration}
          step={Math.max(1, Math.round(model.ticksPerSecond / 120))}
          value={Math.min(displayTick, duration)}
          onChange={(event) => void transport("scrub", snapTick(Number(event.target.value)))}
          style={{ flex: "1 1 160px", minWidth: 120 }}
        />
        <select
          aria-label="Loop policy"
          value={model.loopPolicy}
          disabled={clipPreviewId !== null || clipPreviewPending}
          title={clipPreviewId !== null || clipPreviewPending
            ? "Unsaved imported-clip previews use a bounded one-shot lease. Stop preview to change the authored loop policy."
            : undefined}
          onChange={(event) => void transport(model.playing ? "play" : "scrub", model.playing ? undefined : displayTick, event.target.value as AnimationLoopPolicy)}
          style={selectStyle}
        >
          <option value="once">Once</option><option value="loop">Loop</option><option value="ping_pong">Ping-pong</option>
        </select>
        <Badge tone={model.playing ? "success" : "neutral"}>{model.playing ? "previewing" : "stopped"}</Badge>
        <span style={{ display: "inline-flex", alignItems: "center", gap: space.xs, color: color.text.muted, fontSize: fontSize.micro }}>
          <ShortcutBadge keys="Space" /> play · <ShortcutBadge keys={["←", "→"]} ariaLabel="Left or Right arrow" /> frame
        </span>
      </div>

      {activeSurface === "graph" ? (
        <AnimationGraphEditor client={client} sequenceId={model.sequenceId || "main"} workspaceKey={graphWorkspaceKey} />
      ) : (
      <div className={`animation-workspace-layout${contextView.inspector.open ? " animation-workspace-layout--inspector" : ""}`} style={{ gap: space.sm, padding: space.sm, minHeight: 0, overflow: "auto" }}>
        <section className="animation-workspace-tools" style={{ ...card, padding: space.md, overflow: "auto" }} aria-label={`${CONTEXT_LABELS[activeContext]} animation tools`}>
          <Header title={`${CONTEXT_LABELS[activeContext]} channels`} detail={model.selectedName ?? "No entity selected"} />
          <div data-testid="animation-context-readiness" data-state={activeReadiness.state} style={{ padding: space.sm, marginBottom: space.sm, borderRadius: radius.md, background: color.bg.inset }}>
            <div style={{ display: "flex", justifyContent: "space-between", gap: space.sm }}>
              <span style={{ fontSize: fontSize.meta }}>{activeReadiness.properties} properties · {activeReadiness.tracks} tracks</span>
              <Badge tone={bindingTone(activeReadiness.state)}>{activeReadiness.state.replace("_", " ")}</Badge>
            </div>
            <div style={{ marginTop: space.xs, color: color.text.muted, fontSize: fontSize.micro, lineHeight: 1.35 }}>{activeReadiness.reason}</div>
            {activeReadiness.action && <div style={{ marginTop: space.xs, color: color.info.text, fontSize: fontSize.micro }}>Next: {activeReadiness.action}</div>}
          </div>
          {activeContext !== "3d" && (
            <ContextPreview context={activeContext} values={previewValues} selectedName={model.selectedName} readiness={activeReadiness} />
          )}
          {!selectedId ? (
            <Hint>Select an object to expose context-compatible channels.</Hint>
          ) : (
            <>
              <label style={{ display: "grid", gap: space.xs, color: color.text.secondary, fontSize: fontSize.meta }}>
                Property
                <select
                  data-testid="animation-property"
                  aria-describedby="animation-property-status"
                  value={currentProperty ? propertyId(currentProperty) : ""}
                  onChange={(event) => animationEditorStore.getState().setDraft(workspaceKey, activeContext, PROPERTY_DRAFT, event.target.value)}
                  style={{ ...selectStyle, width: "100%" }}
                >
                  {!contextProperties.some((property) => property.animatable) && <option value="">No verified {CONTEXT_LABELS[activeContext]} playback properties</option>}
                  {contextProperties.map((property) => (
                    <option key={propertyId(property)} value={propertyId(property)} title={property.bindingReason || property.reason || undefined}>
                      {property.label}{property.animatable ? "" : " — unavailable"}
                    </option>
                  ))}
                </select>
              </label>
              <div style={{ display: "flex", justifyContent: "space-between", gap: space.sm, marginTop: space.sm, fontSize: fontSize.meta }}>
                <span style={{ color: color.text.muted }} title="Preview is render-only and never rewrites this value.">Authored value</span>
                <code>{currentProperty ? valueLabel(currentProperty.value) : "—"}</code>
              </div>
              {currentProperty && (
                <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", gap: space.sm, marginTop: space.xs }}>
                  <Badge tone={bindingTone(bindingState(currentProperty))}>{bindingState(currentProperty).replace("_", " ")}</Badge>
                  <span style={{ color: color.text.muted, fontSize: fontSize.micro }}>{runtimeSinkLabel(currentProperty.runtimeSink)}</span>
                </div>
              )}
              <label style={{ display: "grid", gap: space.xs, marginTop: space.sm, color: color.text.secondary, fontSize: fontSize.meta }}>
                New track interpolation
                <select
                  value={interpolation}
                  disabled={!currentProperty?.animatable || currentProperty.valueKind === "bool" || currentProperty.valueKind === "string"}
                  onChange={(event) => animationEditorStore.getState().setDraft(workspaceKey, activeContext, INTERPOLATION_DRAFT, event.target.value)}
                  style={selectStyle}
                >
                  <option value="step">Step</option><option value="linear">Linear</option><option value="cubic">Cubic</option>
                </select>
              </label>
              <Button data-testid="animation-add-key" style={{ width: "100%", marginTop: space.md }} disabled={busy || !currentProperty?.animatable} onClick={keyCurrent}>
                Add key at {displayTick}t
              </Button>
              <div id="animation-property-status">
                {!currentProperty && <Hint>Only channels with a verified adapter can be keyed; unavailable typed fields stay visible and explained.</Hint>}
                {(currentProperty?.bindingReason || currentProperty?.reason) && <Hint>{currentProperty.bindingReason || currentProperty.reason}</Hint>}
              </div>
            </>
          )}

          <div style={{ marginTop: space.md, paddingTop: space.sm, borderTop: `1px solid ${color.border.subtle}` }}>
            <Header title="Timeline annotations" detail="Shared by all contexts" />
            <div style={{ display: "grid", gridTemplateColumns: "1fr auto", gap: space.xs }}>
              <input
                className="mtk-input"
                data-testid="animation-marker-name"
                aria-label="New marker name"
                placeholder="Marker name"
                value={draftString(contextView.drafts[MARKER_NAME_DRAFT])}
                onChange={(event) => animationEditorStore.getState().setDraft(workspaceKey, activeContext, MARKER_NAME_DRAFT, event.target.value)}
              />
              <Button compact data-testid="animation-add-marker" disabled={!selectedId || busy} onClick={addMarker}>+ Marker</Button>
              <input
                className="mtk-input"
                data-testid="animation-event-name"
                aria-label="New event name"
                placeholder="Event name"
                value={draftString(contextView.drafts[EVENT_NAME_DRAFT])}
                onChange={(event) => animationEditorStore.getState().setDraft(workspaceKey, activeContext, EVENT_NAME_DRAFT, event.target.value)}
              />
              <Button compact data-testid="animation-add-event" disabled={!selectedId || busy} onClick={addEvent}>+ Event</Button>
            </div>
            <input
              className="mtk-input"
              data-testid="animation-event-payload"
              aria-label="New event payload"
              placeholder='Optional payload, e.g. {"sound":"step"}'
              value={draftString(contextView.drafts[EVENT_PAYLOAD_DRAFT])}
              onChange={(event) => animationEditorStore.getState().setDraft(workspaceKey, activeContext, EVENT_PAYLOAD_DRAFT, event.target.value)}
              style={{ width: "100%", marginTop: space.xs }}
            />
          </div>
          {error && <div role="alert" style={{ marginTop: space.sm, color: color.danger.text, fontSize: fontSize.meta }}>{error}</div>}
        </section>

        <section className="animation-workspace-timeline" style={{ ...card, display: "grid", gridTemplateRows: "auto minmax(0, 1fr)", minWidth: 0, overflow: "hidden" }} aria-label="Animation timeline editor">
          <div style={{ display: "flex", alignItems: "center", gap: space.sm, padding: space.sm, borderBottom: `1px solid ${color.border.subtle}`, background: color.bg.raised, overflowX: "auto" }}>
            <input
              type="search"
              className="mtk-input"
              data-testid="animation-search"
              aria-label="Search animation tracks"
              placeholder={`Filter ${CONTEXT_LABELS[activeContext]} tracks…`}
              value={contextView.search}
              onChange={(event) => animationEditorStore.getState().setSearch(workspaceKey, activeContext, event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Escape" && contextView.search) animationEditorStore.getState().setSearch(workspaceKey, activeContext, "");
              }}
              style={{ flex: "1 1 130px", minWidth: 100 }}
            />
            <Button
              compact
              variant="ghost"
              active={contextView.snap.enabled}
              aria-pressed={contextView.snap.enabled}
              data-testid="animation-snap"
              onClick={() => animationEditorStore.getState().setSnap(workspaceKey, activeContext, { enabled: !contextView.snap.enabled })}
              title="Snap timeline clicks to frames, nearby keys, and markers"
            >
              Snap {contextView.snap.enabled ? "on" : "off"}
            </Button>
            <label style={{ display: "flex", alignItems: "center", gap: space.xs, color: color.text.muted, fontSize: fontSize.micro }}>
              Zoom
              <input
                type="range"
                min={Math.log2(MIN_ANIMATION_ZOOM)}
                max={Math.log2(MAX_ANIMATION_ZOOM)}
                step={0.25}
                aria-label="Timeline zoom"
                data-testid="animation-zoom"
                value={Math.log2(contextView.zoom)}
                onChange={(event) => animationEditorStore.getState().setZoom(workspaceKey, activeContext, 2 ** Number(event.target.value))}
              />
              <span>{contextView.zoom < 1 ? contextView.zoom.toFixed(2) : contextView.zoom.toFixed(1)}×</span>
            </label>
            <select
              aria-label="Timeline time display"
              value={contextView.timeDisplay}
              onChange={(event) => animationEditorStore.getState().setTimeDisplay(workspaceKey, activeContext, event.target.value as "frames" | "seconds" | "ticks")}
              style={selectStyle}
            >
              <option value="frames">Frames</option><option value="seconds">Seconds</option><option value="ticks">Ticks</option>
            </select>
            <Button compact variant="ghost" onClick={() => animationEditorStore.getState().setInspectorOpen(workspaceKey, activeContext, !contextView.inspector.open)}>
              {contextView.inspector.open ? "Hide" : "Show"} inspector
            </Button>
          </div>

          {contextView.view === "curves" ? (
            <CurveEditor
              track={selectedTrack}
              selectedKeyId={selectedKeyId}
              onSelect={selectKey}
              onDelete={(track, key) => deleteSelectedKey({ track, key })}
            />
          ) : (
            <div
              ref={timelineRef}
              className="mtk-scroll"
              data-testid="animation-timeline-viewport"
              onScroll={(event) => {
                const x = event.currentTarget.scrollLeft;
                const y = event.currentTarget.scrollTop;
                if (scrollCommitTimer.current !== null) window.clearTimeout(scrollCommitTimer.current);
                pendingScrollCommit.current = () => animationEditorStore.getState().setScroll(workspaceKey, activeContext, { x, y });
                scrollCommitTimer.current = window.setTimeout(() => {
                  pendingScrollCommit.current?.();
                  pendingScrollCommit.current = null;
                  scrollCommitTimer.current = null;
                }, 120);
              }}
              style={{ minHeight: 0, overflow: "auto", background: color.bg.inset }}
            >
              <div style={{ width: 178 + timelineWidth, minHeight: "100%" }}>
                <Ruler
                  duration={duration}
                  currentTick={displayTick}
                  ticksPerSecond={model.ticksPerSecond}
                  display={contextView.timeDisplay}
                  width={timelineWidth}
                  onScrub={(tick) => void transport("scrub", snapTick(tick))}
                />
                <AnnotationLane
                  label="Markers"
                  duration={duration}
                  width={timelineWidth}
                  currentTick={displayTick}
                  onLaneClick={scrubLane}
                >
                  {model.markers.map((marker) => (
                    <span key={marker.id} style={{ position: "absolute", left: `${(marker.tick / duration) * 100}%`, top: 4, display: "inline-flex", transform: "translateX(-8px)", zIndex: 2 }}>
                      <button
                        type="button"
                        data-testid="animation-marker"
                        aria-label={`${marker.name} marker at ${marker.tick} ticks`}
                        title={`${marker.name} · ${timeLabel(marker.tick, model.ticksPerSecond, contextView.timeDisplay)}`}
                        onClick={() => void transport("scrub", marker.tick)}
                        style={{ width: 16, height: 20, padding: 0, border: 0, color: markerColor(marker), background: "transparent", cursor: "pointer" }}
                      >◆</button>
                      <button
                        type="button"
                        aria-label={`Delete marker ${marker.name}`}
                        disabled={busy}
                        onClick={() => void runEdit(() => client.animationDeleteMarker(marker.ownerId, marker.id))}
                        style={{ width: 14, height: 16, padding: 0, border: 0, color: color.text.muted, background: color.bg.raised, cursor: "pointer", fontSize: 10 }}
                      >×</button>
                    </span>
                  ))}
                </AnnotationLane>
                <AnnotationLane label="Events" duration={duration} width={timelineWidth} currentTick={displayTick} onLaneClick={scrubLane}>
                  {model.events.map((event) => (
                    <span key={event.id} style={{ position: "absolute", left: `${(event.tick / duration) * 100}%`, top: 5, display: "inline-flex", transform: "translateX(-8px)", zIndex: 2 }}>
                      <button
                        type="button"
                        data-testid="animation-event"
                        aria-label={`${event.name} event at ${event.tick} ticks`}
                        title={`${event.name} · ${valueLabel(event.payload)}`}
                        onClick={() => void transport("scrub", event.tick)}
                        style={{ minWidth: 18, height: 18, padding: "0 3px", border: `1px solid ${color.info.border}`, borderRadius: radius.sm, color: color.info.text, background: color.info.bg, cursor: "pointer", fontSize: 9 }}
                      >E</button>
                      <button
                        type="button"
                        aria-label={`Delete event ${event.name}`}
                        disabled={busy}
                        onClick={() => void runEdit(() => client.animationDeleteEvent(event.ownerId, event.id))}
                        style={{ width: 14, height: 16, padding: 0, border: 0, color: color.text.muted, background: color.bg.raised, cursor: "pointer", fontSize: 10 }}
                      >×</button>
                    </span>
                  ))}
                </AnnotationLane>

                {visibleTracks.length === 0 ? (
                  <div style={{ width: 178 + timelineWidth, padding: space.lg }}>
                    <EmptyPanelState
                      compact
                      icon="◇"
                      title={contextTracks.length === 0 ? `No ${CONTEXT_LABELS[activeContext]} tracks yet` : "No matching tracks"}
                      description={contextTracks.length === 0 ? "Choose a compatible property and add a key at the playhead." : `Clear “${contextView.search}” to show all context tracks.`}
                      primaryAction={contextView.search ? <Button compact onClick={() => animationEditorStore.getState().setSearch(workspaceKey, activeContext, "")}>Clear filter</Button> : undefined}
                    />
                  </div>
                ) : visibleTracks.map((track) => {
                  const active = selectedTrack?.id === track.id;
                  const state = bindingState(track);
                  return (
                    <div
                      key={track.id}
                      data-testid="animation-track"
                      data-context={inferredContext(track)}
                      data-enabled={track.enabled}
                      data-locked={track.locked}
                      data-binding-state={state}
                      style={{ display: "grid", gridTemplateColumns: `178px ${timelineWidth}px`, minHeight: 48, opacity: track.enabled ? 1 : 0.52, borderTop: `1px solid ${state === "invalid" ? color.danger.border : color.border.subtle}` }}
                    >
                      <div style={{ position: "sticky", left: 0, zIndex: 4, display: "grid", gridTemplateColumns: "1fr auto", gap: space.xs, padding: space.sm, outline: active ? `1px solid ${color.accent.base}` : "none", background: active ? color.accent.subtle : color.bg.panel }}>
                        <button
                          type="button"
                          aria-pressed={active}
                          aria-label={`Select track ${track.name}`}
                          onClick={() => animationEditorStore.getState().setSelection(workspaceKey, activeContext, [track.id], [])}
                          style={{ minWidth: 0, padding: 0, border: 0, textAlign: "left", color: "inherit", background: "transparent", cursor: "pointer" }}
                        >
                          <span title={`${track.targetName} / ${track.component}.${track.property}`} style={{ display: "block", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", fontSize: fontSize.body }}>{track.name}</span>
                          <span style={{ display: "flex", alignItems: "center", gap: space.xs, marginTop: 2 }}>
                            <Badge tone={bindingTone(state)}>{state.replace("_", " ")}</Badge>
                            <span style={{ color: color.text.muted, fontSize: fontSize.micro }}>{track.keys.length} keys</span>
                          </span>
                        </button>
                        <div style={{ display: "flex", alignItems: "center", gap: 2 }}>
                          <Button
                            compact
                            icon
                            variant="ghost"
                            disabled={track.locked || busy}
                            data-testid="animation-track-enabled"
                            aria-label={`${track.enabled ? "Mute" : "Unmute"} ${track.name}`}
                            title={track.enabled ? "Mute track without deleting keys" : "Unmute track"}
                            onClick={(event) => { event.stopPropagation(); changeTrackEnabled(track); }}
                          >{track.enabled ? "M" : "m"}</Button>
                          <Button
                            compact
                            icon
                            variant="ghost"
                            active={track.locked}
                            data-testid="animation-track-locked"
                            aria-label={`${track.locked ? "Unlock" : "Lock"} ${track.name}`}
                            title={track.locked ? "Unlock track editing" : "Protect track from editing"}
                            onClick={(event) => { event.stopPropagation(); changeTrackLocked(track); }}
                          >{track.locked ? "▣" : "□"}</Button>
                        </div>
                      </div>
                      <div onClick={scrubLane} style={{ position: "relative", minHeight: 47, backgroundImage: `linear-gradient(to right, ${color.border.subtle} 1px, transparent 1px)`, backgroundSize: `${Math.max(20, timelineWidth / 10)}px 100%`, cursor: "crosshair" }}>
                        <Playhead />
                        {track.keys.map((key) => {
                          const identity = keySelectionId(track.id, key.id);
                          const keyActive = contextView.selectedKeyIds.includes(identity);
                          return (
                            <button
                              key={key.id}
                              type="button"
                              aria-label={`${track.name} key at ${key.seconds.toFixed(2)} seconds`}
                              aria-pressed={keyActive}
                              data-testid="animation-key"
                              data-selected={keyActive || undefined}
                              title={`${key.tick}t · ${valueLabel(key.value)}${track.locked ? " · locked" : ""}`}
                              onClick={(event) => selectKey(track, key, event)}
                              onKeyDown={(event) => {
                                if ((event.key === "Delete" || event.key === "Backspace") && !track.locked) {
                                  event.preventDefault();
                                  event.stopPropagation();
                                  deleteSelectedKey({ track, key });
                                }
                              }}
                              style={{ position: "absolute", left: `calc(${(key.tick / duration) * 100}% - 12px)`, top: 11, display: "grid", placeItems: "center", width: 24, height: 24, padding: 0, border: 0, background: "transparent", cursor: "pointer", zIndex: 3 }}
                            >
                              <span aria-hidden="true" style={{ width: 12, height: 12, transform: "rotate(45deg)", border: `1px solid ${keyActive ? color.accent.base : state === "invalid" ? color.danger.border : color.border.strong}`, background: keyActive ? color.accent.solid : track.locked ? color.bg.inset : color.bg.raised }} />
                            </button>
                          );
                        })}
                      </div>
                    </div>
                  );
                })}
              </div>
            </div>
          )}
        </section>

        {contextView.inspector.open && (
          <aside className="animation-workspace-inspector" style={{ ...card, overflow: "auto" }} aria-label="Animation inspector">
            <DisclosureSection
              title="Key and track"
              summary={selectedKey ? `${selectedKey.tick}t` : selectedTrack ? selectedTrack.name : "Nothing selected"}
              open={disclosureOpen(contextView.inspector.disclosures, "selection", true)}
              onOpenChange={(open) => animationEditorStore.getState().setInspectorDisclosure(workspaceKey, activeContext, "selection", open)}
            >
              <div style={{ padding: space.md }}>
                {!selectedTrack ? <Hint>Select a track or key to edit it.</Hint> : (
                  <>
                    <div style={{ display: "flex", justifyContent: "space-between", gap: space.sm, alignItems: "center" }}>
                      <strong style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{selectedTrack.name}</strong>
                      <Badge tone={bindingTone(bindingState(selectedTrack))}>{bindingState(selectedTrack).replace("_", " ")}</Badge>
                    </div>
                    <div style={{ marginTop: space.xs, color: color.text.muted, fontSize: fontSize.meta }}>{selectedTrack.targetName} · {selectedTrack.component}.{selectedTrack.property}</div>
                    {selectedTrack.bindingReason && <Hint>{selectedTrack.bindingReason}</Hint>}
                    <label style={{ display: "grid", gap: space.xs, marginTop: space.sm, color: color.text.secondary, fontSize: fontSize.meta }}>
                      Interpolation
                      <select value={selectedTrack.interpolation} disabled={selectedTrack.locked} onChange={(event) => changeInterpolation(selectedTrack, event.target.value as AnimationInterpolation)} style={selectStyle}>
                        <option value="step">Step</option><option value="linear">Linear</option><option value="cubic">Cubic</option>
                      </select>
                    </label>
                    {selectedKey && (
                      <div style={{ display: "grid", gridTemplateColumns: "1fr auto", gap: space.sm, alignItems: "center", marginTop: space.sm }}>
                        <span style={{ color: color.text.secondary, fontSize: fontSize.meta }}>Exact tick</span>
                        <NumericField
                          integer
                          min={0}
                          max={duration}
                          value={selectedKey.tick}
                          disabled={selectedTrack.locked || busy}
                          ariaLabel="Selected key exact tick"
                          data-testid="animation-key-tick"
                          onCommit={(tick) => updateSelectedKey({ tick: Math.round(tick) })}
                        />
                        <span style={{ color: color.text.secondary, fontSize: fontSize.meta }}>Value</span>
                        {typeof selectedKey.value === "number" ? (
                          <NumericField
                            value={selectedKey.value}
                            disabled={selectedTrack.locked || busy}
                            ariaLabel="Selected key value"
                            data-testid="animation-key-value"
                            onCommit={(value) => updateSelectedKey({ value })}
                          />
                        ) : <code style={{ maxWidth: 120, overflow: "hidden", textOverflow: "ellipsis" }}>{valueLabel(selectedKey.value)}</code>}
                        {selectedTrack.interpolation === "cubic" && typeof selectedKey.value === "number" && (
                          <>
                            <span style={{ color: color.text.secondary, fontSize: fontSize.meta }}>In tangent</span>
                            <NumericField
                              value={typeof selectedKey.inTangent === "number" ? selectedKey.inTangent : 0}
                              disabled={selectedTrack.locked || busy}
                              ariaLabel="Selected key incoming tangent"
                              data-testid="animation-key-in-tangent"
                              onCommit={(inTangent) => updateSelectedKey({ inTangent })}
                            />
                            <span style={{ color: color.text.secondary, fontSize: fontSize.meta }}>Out tangent</span>
                            <NumericField
                              value={typeof selectedKey.outTangent === "number" ? selectedKey.outTangent : 0}
                              disabled={selectedTrack.locked || busy}
                              ariaLabel="Selected key outgoing tangent"
                              data-testid="animation-key-out-tangent"
                              onCommit={(outTangent) => updateSelectedKey({ outTangent })}
                            />
                          </>
                        )}
                      </div>
                    )}
                    {selectedKey && (
                      <Button compact variant="danger" data-testid="animation-delete-key" disabled={busy || selectedTrack.locked} onClick={() => deleteSelectedKey()} style={{ width: "100%", marginTop: space.md }}>
                        Delete key
                      </Button>
                    )}
                    {selectedTrack.locked && <Hint>Unlock this track before editing keys or interpolation.</Hint>}
                  </>
                )}
              </div>
            </DisclosureSection>

            <DisclosureSection
              title="Asset readiness"
              summary={model.asset?.qualityGrade ?? "No asset"}
              open={disclosureOpen(contextView.inspector.disclosures, "asset", false)}
              onOpenChange={(open) => animationEditorStore.getState().setInspectorDisclosure(workspaceKey, activeContext, "asset", open)}
            >
              <div style={{ padding: space.md }}>
                {!model.asset ? <Hint>Select an entity to inspect animation readiness.</Hint> : (
                  <>
                    <div style={{ display: "flex", justifyContent: "space-between", gap: space.sm }}><strong>{model.asset.displayName}</strong><Badge tone="accent">{model.asset.qualityGrade}</Badge></div>
                    <div style={{ margin: `${space.xs}px 0`, color: color.text.muted, fontSize: fontSize.meta }}>{model.asset.source} · {model.asset.provenance}</div>
                    {model.asset.logicalId && (
                      <div data-testid="animation-asset-lifecycle" style={{ display: "grid", gridTemplateColumns: "auto minmax(0, 1fr)", gap: `${space.xs}px ${space.sm}px`, margin: `${space.sm}px 0`, padding: space.sm, borderRadius: radius.md, background: color.bg.inset, fontSize: fontSize.micro }}>
                        <span style={{ color: color.text.muted }}>Logical asset</span><code title={model.asset.logicalId} style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{model.asset.logicalId}</code>
                        <span style={{ color: color.text.muted }}>Revision</span><code title={model.asset.revisionId ?? undefined} style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{model.asset.revisionId ?? "unknown"}</code>
                        <span style={{ color: color.text.muted }}>Import</span><span>{model.asset.importState?.replaceAll("_", " ") ?? "unknown"}</span>
                        <span style={{ color: color.text.muted }}>Source</span><span>{model.asset.sourceLocation?.replaceAll("_", " ") ?? "unknown"}{model.asset.watchesSource ? " · watched" : ""}</span>
                        <span style={{ color: color.text.muted }}>References</span><span>{model.asset.dependencyCount} dependencies · {model.asset.reimportDiagnostics} retained diagnostics</span>
                      </div>
                    )}
                    {model.asset.clips.length > 0 && (
                      <section aria-label="Imported animation clips" style={{ margin: `${space.md}px 0` }}>
                        <div style={{ marginBottom: space.xs, color: color.text.secondary, fontSize: fontSize.meta, fontWeight: 600 }}>Imported clips</div>
                        {model.asset.clips.map((clip) => {
                          const settingUp = clipSetupId === clip.clipId;
                           const sourceTargetIds = clipSourceTargetIds(clip);
                           const repairing = settingUp
                             && clipDraftFence.current?.clipId === clip.clipId
                             && clipDraftFence.current.mode === "repair";
                           const repairChanges = repairing
                             ? (clipDraftFence.current?.repairChanges ?? [])
                             : (clip.repairChanges ?? []);
                           const instanceOptions = clip.instances ?? [];
                           const requiresExplicitMapping = repairing || sourceTargetIds.length > 1 || clip.sourceTargets.length > 1;
                          const mappingValidation = settingUp
                            ? clipMappingValidation(clip)
                            : { complete: false, message: null, bySource: {} as Record<string, string> };
                          const draftStale = settingUp && clipDraftIsStale(clip);
                          const firstUnresolvedSourceId = sourceTargetIds.find((sourceTargetId) => Boolean(mappingValidation.bySource[sourceTargetId])) ?? null;
                           const tone = clip.readiness === "ready"
                             ? "success"
                             : clip.readiness === "setup_available" || clip.readiness === "explicit_mapping_required" || clip.readiness === "repair_required"
                               ? "warn"
                               : "neutral";
                          return (
                            <div
                              key={clip.clipId}
                              data-testid="animation-imported-clip"
                              data-readiness={clip.readiness}
                              style={{ marginBottom: space.sm, padding: space.sm, border: `1px solid ${color.border.subtle}`, borderRadius: radius.md, background: color.bg.inset }}
                            >
                              <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.sm }}>
                                <strong style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>{clip.name}</strong>
                                <Badge tone={tone}>{clip.readiness.replaceAll("_", " ")}</Badge>
                              </div>
                              <div style={{ marginTop: space.xs, color: color.text.muted, fontSize: fontSize.micro }}>
                                {clip.sourceTargets.length} source node{clip.sourceTargets.length === 1 ? "" : "s"} / {timeLabel(clip.durationTick, model.ticksPerSecond, "seconds")}
                              </div>
                              <div style={{ marginTop: space.xs, color: color.text.secondary, fontSize: fontSize.meta }}>{clip.reason}</div>
                              {clip.readiness === "repair_required" && repairChanges.length > 0 && instanceOptions.length === 0 && (
                                <div style={{ marginTop: space.xs, color: color.warn.text, fontSize: fontSize.micro }}>
                                  {repairChanges.length} source change{repairChanges.length === 1 ? "" : "s"} to review
                                </div>
                              )}
                              {instanceOptions.length > 0 && (
                                <div aria-label="Persisted clip instances" style={{ display: "grid", gap: space.xs, marginTop: space.sm }}>
                                  {instanceOptions.map((instance) => (
                                    <section
                                      key={instance.instanceId}
                                      data-testid="animation-clip-instance-option"
                                      data-readiness={instance.readiness}
                                      style={{ padding: space.sm, border: `1px solid ${color.border.subtle}`, borderRadius: radius.md, background: color.bg.panel }}
                                    >
                                      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.sm }}>
                                        <strong>{instance.name}</strong>
                                        <Badge tone={instance.readiness === "ready" ? "success" : "warn"}>
                                          {instance.readiness.replaceAll("_", " ")}
                                        </Badge>
                                      </div>
                                      <code style={{ display: "block", marginTop: space.xs, color: color.text.muted, fontSize: fontSize.micro }}>{instance.instanceId}</code>
                                      <div style={{ marginTop: space.xs, color: color.text.secondary, fontSize: fontSize.micro }}>{instance.reason}</div>
                                      {instance.readiness === "ready" && (
                                        <div data-testid="animation-clip-ready" style={{ marginTop: space.xs, color: color.success.text, fontSize: fontSize.meta }}>
                                          Ready for Graph / <code>{instance.instanceId}</code>
                                        </div>
                                      )}
                                      <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs, marginTop: space.sm }}>
                                        {instance.readiness === "repair_required" && (
                                          <Button
                                            compact
                                            data-testid={`animation-clip-repair-instance-${instance.instanceId}`}
                                            disabled={busy}
                                            aria-haspopup="dialog"
                                            onClick={(event) => openClipSetup(event, {
                                              ...clip,
                                              readiness: "repair_required",
                                              instanceId: instance.instanceId,
                                              targetMappings: instance.targetMappings,
                                              repairChanges: instance.repairChanges,
                                            }, "repair")}
                                          >
                                            Repair instance
                                          </Button>
                                        )}
                                        {clipDiscardConfirmId === instance.instanceId ? (
                                          <>
                                            <span role="alert" style={{ alignSelf: "center", color: color.danger.text, fontSize: fontSize.micro }}>Discard this persisted instance?</span>
                                            <Button compact variant="ghost" disabled={busy} onClick={() => setClipDiscardConfirmId(null)}>Cancel</Button>
                                            <Button
                                              compact
                                              variant="danger"
                                              data-testid={`animation-clip-discard-confirm-${instance.instanceId}`}
                                              disabled={busy}
                                              onClick={() => void discardClipInstance(instance.instanceId)}
                                            >
                                              Confirm discard
                                            </Button>
                                          </>
                                        ) : (
                                          <Button
                                            compact
                                            variant="ghost"
                                            data-testid={`animation-clip-discard-${instance.instanceId}`}
                                            disabled={busy}
                                            onClick={() => setClipDiscardConfirmId(instance.instanceId)}
                                          >
                                            Discard
                                          </Button>
                                        )}
                                      </div>
                                    </section>
                                  ))}
                                  <Button
                                    compact
                                    data-testid={`animation-clip-setup-another-${clip.clipId}`}
                                    disabled={busy}
                                    aria-haspopup="dialog"
                                    aria-expanded={settingUp}
                                    onClick={(event) => openClipSetup(event, { ...clip, instanceId: null, targetMappings: [], repairChanges: [] }, "create")}
                                    style={{ width: "100%" }}
                                  >
                                    Set up another instance
                                  </Button>
                                </div>
                              )}
                              {instanceOptions.length === 0 && (clip.readiness === "setup_available" || clip.readiness === "explicit_mapping_required" || clip.readiness === "repair_required") && (
                                <Button
                                  compact
                                  data-testid={clip.readiness === "repair_required"
                                    ? `animation-clip-repair-${clip.clipId}`
                                    : clip.readiness === "explicit_mapping_required"
                                      ? `animation-clip-map-${clip.clipId}`
                                      : `animation-clip-setup-${clip.clipId}`}
                                  disabled={busy}
                                  aria-haspopup="dialog"
                                  aria-expanded={settingUp}
                                  onClick={(event) => openClipSetup(event, clip)}
                                  style={{ width: "100%", marginTop: space.sm }}
                                >
                                  {clip.readiness === "repair_required"
                                    ? "Repair mapping"
                                    : clip.readiness === "explicit_mapping_required"
                                      ? "Map targets"
                                      : "Set up clip"}
                                </Button>
                              )}
                              {instanceOptions.length === 0 && clip.readiness === "ready" && clip.instanceId && (
                                <>
                                  <div data-testid="animation-clip-ready" style={{ marginTop: space.sm, color: color.success.text, fontSize: fontSize.meta }}>
                                    Ready for Graph / <code>{clip.instanceId}</code>
                                  </div>
                                  <Button
                                    compact
                                    data-testid={`animation-clip-setup-another-${clip.clipId}`}
                                    disabled={busy}
                                    aria-haspopup="dialog"
                                    aria-expanded={settingUp}
                                    onClick={(event) => openClipSetup(event, clip, "create")}
                                    style={{ width: "100%", marginTop: space.sm }}
                                  >
                                    Set up another instance
                                  </Button>
                                </>
                              )}
                              {settingUp && (
                                <Modal
                                  open
                                  onClose={requestCloseClipSetup}
                                  initialFocusRef={requiresExplicitMapping ? clipFirstUnresolvedTarget : clipPreviewButton}
                                  ariaLabelledBy={`animation-clip-mapper-heading-${clip.clipId}`}
                                  ariaDescribedBy={`animation-clip-mapper-description-${clip.clipId}`}
                                >
                                  <div
                                    data-testid="animation-clip-setup-dialog"
                                    style={{
                                      width: "min(720px, calc(100vw - 32px))",
                                      maxHeight: "calc(100vh - 32px)",
                                      overflow: "auto",
                                      padding: space.lg,
                                      border: `1px solid ${color.border.strong}`,
                                      borderRadius: radius.xl,
                                      background: color.bg.raised,
                                      color: color.text.primary,
                                      boxShadow: "0 22px 72px rgba(0, 0, 0, 0.48)",
                                    }}
                                  >
                                    <h2
                                      id={`animation-clip-mapper-heading-${clip.clipId}`}
                                      style={{ margin: 0, fontFamily: font.ui, fontSize: fontSize.heading }}
                                    >
                                      {repairing
                                        ? `Repair mapping for ${clip.name}`
                                        : requiresExplicitMapping
                                          ? `Map targets for ${clip.name}`
                                          : `Set up ${clip.name}`}
                                    </h2>
                                    <div
                                      id={`animation-clip-mapper-description-${clip.clipId}`}
                                      style={{ marginTop: space.xs, color: color.text.secondary, fontSize: fontSize.meta }}
                                    >
                                      {clip.name} · {timeLabel(clip.durationTick, model.ticksPerSecond, "seconds")}. Each imported source node keeps its typed channels and needs one live Transform target.
                                    </div>
                                    {draftStale && (
                                      <div data-testid="animation-clip-draft-stale" role="alert" style={{ marginTop: space.sm, padding: space.sm, border: `1px solid ${color.danger.border}`, borderRadius: radius.md, color: color.danger.text, background: color.danger.bg, fontSize: fontSize.meta }}>
                                        The imported revision or clip-instance set changed while this mapper was open. Close and reopen it to review the current source evidence.
                                      </div>
                                    )}
                                    {error && (
                                      <div data-testid="animation-clip-error" role="alert" style={{ marginTop: space.sm, padding: space.sm, borderRadius: radius.md, color: color.danger.text, background: color.danger.bg, fontSize: fontSize.meta }}>
                                        {error}
                                      </div>
                                    )}
                                    {repairing && repairChanges.length > 0 && (
                                      <section
                                        data-testid="animation-clip-repair-changes"
                                        aria-label="Source changes requiring review"
                                        style={{ marginTop: space.sm, padding: space.sm, border: `1px solid ${color.warn.border}`, borderRadius: radius.md, color: color.warn.text, background: color.warn.bg, fontSize: fontSize.meta }}
                                      >
                                        <strong>What changed in the imported clip</strong>
                                        <ul style={{ margin: `${space.xs}px 0 0`, paddingLeft: space.lg }}>
                                          {repairChanges.map((change) => <li key={change}>{change}</li>)}
                                        </ul>
                                      </section>
                                    )}

                                    {requiresExplicitMapping ? (
                                      <div style={{ marginTop: space.md }}>
                                        <div style={{ display: "flex", justifyContent: "flex-end", marginBottom: space.sm }}>
                                          <Button
                                            compact
                                            data-testid="animation-clip-suggest-exact"
                                            disabled={busy || draftStale}
                                            onClick={() => void suggestExactClipTargets(clip)}
                                          >
                                            Suggest exact names
                                          </Button>
                                        </div>
                                        <div style={{ display: "grid", gap: space.sm }}>
                                         {sourceTargetIds.map((sourceTargetId, index) => {
                                           const sourceBindings = clipSourceBindings(clip, sourceTargetId);
                                           const sourceLabel = clipSourceTargetLabel(clip, sourceTargetId, index);
                                           const issue = mappingValidation.bySource[sourceTargetId] ?? null;
                                           const issueId = `animation-clip-target-${clip.clipId}-${index}-error`;
                                           return (
                                            <section
                                              key={sourceTargetId}
                                              data-testid="animation-clip-source-mapping"
                                              data-state={issue ? "invalid" : "ready"}
                                              style={{ padding: space.sm, border: `1px solid ${issue ? color.danger.border : color.border.subtle}`, borderRadius: radius.md, background: color.bg.inset }}
                                            >
                                              <label
                                                htmlFor={`animation-clip-target-${clip.clipId}-${index}`}
                                                style={{ display: "block", fontSize: fontSize.meta, fontWeight: 600 }}
                                              >
                                                {sourceLabel}
                                              </label>
                                              <select
                                                ref={sourceTargetId === firstUnresolvedSourceId ? clipFirstUnresolvedTarget : undefined}
                                                id={`animation-clip-target-${clip.clipId}-${index}`}
                                                 aria-label={`Target for ${sourceLabel}`}
                                                 aria-invalid={Boolean(issue)}
                                                 aria-describedby={issue ? issueId : undefined}
                                                data-testid={`animation-clip-target-${index}`}
                                                value={clipTargetMappings[sourceTargetId] ?? ""}
                                                onChange={(event) => void updateClipTargetMapping(sourceTargetId, event.currentTarget.value)}
                                                disabled={busy}
                                                style={{ ...selectStyle, width: "100%", marginTop: space.xs }}
                                              >
                                                <option value="">Choose a live Transform entity…</option>
                                                {liveTransformTargets.map((target) => (
                                                  <option key={target.id} value={target.id}>{target.name} · {target.id}</option>
                                                ))}
                                              </select>
                                              <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs, marginTop: space.sm }}>
                                                {(sourceBindings.length > 0
                                                  ? sourceBindings.map((binding) => {
                                                      const destinationProperty = binding.component === "Transform" && binding.property === "scale" && binding.valueKind === "vec3"
                                                        ? "scale3"
                                                        : binding.property;
                                                      return `${binding.component}.${binding.property} · ${binding.valueKind} → ${binding.component}.${destinationProperty} · ${binding.valueKind}`;
                                                    })
                                                  : clip.channels
                                                ).map((channel) => (
                                                  <code key={channel} style={{ padding: `2px ${space.xs}px`, borderRadius: radius.sm, background: color.bg.panel, color: color.text.secondary, fontSize: fontSize.micro }}>
                                                    {channel}
                                                  </code>
                                                ))}
                                              </div>
                                              {issue && <div id={issueId} role="alert" style={{ marginTop: space.xs, color: color.danger.text, fontSize: fontSize.meta }}>{issue}</div>}
                                            </section>
                                          );
                                        })}
                                        </div>
                                      </div>
                                    ) : (
                                      <div style={{ marginTop: space.md, padding: space.sm, borderRadius: radius.md, background: color.bg.inset, fontSize: fontSize.meta }}>
                                        <div><strong>Source</strong>: {clip.sourceTargets.join(", ")}</div>
                                        <div style={{ marginTop: space.xs }}><strong>Channels</strong></div>
                                        <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs, marginTop: space.xs }}>
                                          {(clipSourceBindings(clip).length
                                            ? clipSourceBindings(clip).map((binding) => {
                                                const destinationProperty = binding.component === "Transform" && binding.property === "scale" && binding.valueKind === "vec3"
                                                  ? "scale3"
                                                  : binding.property;
                                                return `${binding.component}.${binding.property} · ${binding.valueKind} → ${binding.component}.${destinationProperty} · ${binding.valueKind}`;
                                              })
                                            : clip.channels
                                          ).map((channel) => (
                                            <code key={channel} style={{ padding: `2px ${space.xs}px`, borderRadius: radius.sm, background: color.bg.panel, color: color.text.secondary, fontSize: fontSize.micro }}>
                                              {channel}
                                            </code>
                                          ))}
                                        </div>
                                        <div style={{ marginTop: space.xs }}><strong>Target</strong>: {model.selectedName ?? selectedId}</div>
                                      </div>
                                    )}

                                    {mappingValidation.message && (
                                      <div data-testid="animation-clip-mapping-status" role="status" style={{ marginTop: space.sm, color: color.warn.text, fontSize: fontSize.meta }}>
                                        {mappingValidation.message}
                                      </div>
                                    )}
                                    <Hint>
                                      Preview is render-only and changes no history. {repairing
                                        ? "Repair keeps the persisted instance identity while replacing only its validated target mapping."
                                        : "Confirm creates one revision-pinned instance without modifying the imported asset."}
                                    </Hint>
                                    {clipPreviewId === clip.clipId && (
                                      <section
                                        aria-label="Imported clip audition transport"
                                        data-testid="animation-clip-audition-transport"
                                        style={{ display: "grid", gap: space.sm, marginTop: space.md, padding: space.sm, border: `1px solid ${color.accent.border}`, borderRadius: radius.md, background: color.bg.inset }}
                                      >
                                        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.sm }}>
                                          <strong style={{ fontSize: fontSize.meta }}>Render-only audition</strong>
                                          <span data-testid="animation-clip-audition-time" style={{ font: font.mono, fontSize: fontSize.micro }}>
                                            {timeLabel(displayTick, model.ticksPerSecond, contextView.timeDisplay)} · {displayTick}t
                                          </span>
                                        </div>
                                        <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
                                          <Button
                                            compact
                                            data-testid="animation-clip-audition-play"
                                            onClick={() => void transport(model.playing ? "pause" : "play")}
                                          >
                                            {model.playing ? "Pause audition" : "Resume audition"}
                                          </Button>
                                          <input
                                            aria-label="Imported clip audition timeline"
                                            data-testid="animation-clip-audition-scrub"
                                            type="range"
                                            min={0}
                                            max={duration}
                                            step={Math.max(1, Math.round(model.ticksPerSecond / 120))}
                                            value={Math.min(displayTick, duration)}
                                            onChange={(event) => void transport("scrub", snapTick(Number(event.target.value)))}
                                            style={{ flex: "1 1 180px", minWidth: 120 }}
                                          />
                                        </div>
                                        <span role="status" style={{ color: color.text.muted, fontSize: fontSize.micro }}>
                                          Pause or scrub here while setup stays open. Stop preview restores authored transforms.
                                        </span>
                                      </section>
                                    )}
                                    <div style={{ display: "flex", flexWrap: "wrap", justifyContent: "flex-end", gap: space.xs, marginTop: space.md }}>
                                      <Button compact disabled={busy && !clipPreviewPending} onClick={requestCloseClipSetup}>
                                        Cancel
                                      </Button>
                                      <span ref={(node) => { clipPreviewButton.current = node?.querySelector("button") ?? null; }}>
                                        <Button
                                          compact
                                          data-testid="animation-clip-preview"
                                          disabled={busy || draftStale || !mappingValidation.complete}
                                          active={clipPreviewId === clip.clipId}
                                          onClick={() => {
                                            if (clipPreviewId === clip.clipId) void stopClipPreview();
                                            else void previewClipInstance(clip);
                                          }}
                                        >
                                          {clipPreviewId === clip.clipId ? "Stop preview" : "Preview"}
                                        </Button>
                                      </span>
                                      <Button
                                        compact
                                        variant="primary"
                                        data-testid="animation-clip-confirm"
                                        disabled={busy || draftStale || !mappingValidation.complete}
                                        onClick={() => void saveClipInstance(clip)}
                                      >
                                        {repairing ? "Repair mapping" : "Confirm mapping"}
                                      </Button>
                                    </div>
                                  </div>
                                </Modal>
                              )}
                            </div>
                          );
                        })}
                      </section>
                    )}
                    {model.asset.capabilities.map((fact) => (
                      <div key={fact.capability} data-testid="animation-capability" data-state={fact.state} style={{ padding: `${space.sm}px 0`, borderTop: `1px solid ${color.border.subtle}` }}>
                        <div style={{ display: "flex", justifyContent: "space-between", gap: space.sm }}><span>{fact.capability}</span><Badge tone={capabilityTone(fact.state)}>{fact.state}</Badge></div>
                        <div style={{ marginTop: space.xs, color: color.text.muted, fontSize: fontSize.meta }}>{fact.reason}</div>
                        {fact.action && <div style={{ marginTop: space.xs, color: color.info.text, fontSize: fontSize.meta }}>Next: {fact.action}</div>}
                      </div>
                    ))}
                    {model.asset.suggestions.map((suggestion) => <Hint key={suggestion}>{suggestion}</Hint>)}
                  </>
                )}
              </div>
            </DisclosureSection>

            <DisclosureSection
              title="Diagnostics"
              summary={model.issues.length ? `${model.issues.length} issue${model.issues.length === 1 ? "" : "s"}` : "All clear"}
              open={disclosureOpen(contextView.inspector.disclosures, "diagnostics", model.issues.length > 0)}
              onOpenChange={(open) => animationEditorStore.getState().setInspectorDisclosure(workspaceKey, activeContext, "diagnostics", open)}
            >
              <div style={{ padding: space.md }}>
                {model.issues.length === 0 ? <Hint>No animation validation issues in this sequence.</Hint> : model.issues.map((issue) => (
                  <div key={`${issue.code}:${issue.message}`} role={issue.severity === "error" ? "alert" : undefined} data-severity={issue.severity} style={{ marginBottom: space.sm, padding: space.sm, borderRadius: radius.md, background: issue.severity === "error" ? color.danger.bg : color.warn.bg, fontSize: fontSize.meta }}>
                    <strong>{issue.code}</strong> · {issue.message}{issue.fix ? <div style={{ marginTop: space.xs }}>Recovery: {issue.fix}</div> : null}
                  </div>
                ))}
              </div>
            </DisclosureSection>
          </aside>
        )}
      </div>
      )}
    </div>
  );
}

function Ruler({
  duration,
  currentTick,
  ticksPerSecond,
  display,
  width,
  onScrub,
}: {
  duration: number;
  currentTick: number;
  ticksPerSecond: number;
  display: "frames" | "seconds" | "ticks";
  width: number;
  onScrub: (tick: number) => void;
}) {
  const ticks = Array.from({ length: 11 }, (_, index) => Math.round((duration * index) / 10));
  return (
    <div style={{ display: "grid", gridTemplateColumns: `178px ${width}px`, height: 32, position: "sticky", top: 0, zIndex: 7, borderBottom: `1px solid ${color.border.default}`, background: color.bg.raised }}>
      <div style={{ position: "sticky", left: 0, zIndex: 8, display: "flex", alignItems: "center", padding: `0 ${space.sm}px`, background: color.bg.raised, fontSize: fontSize.meta, color: color.text.secondary }}>Tracks</div>
      <div
        data-testid="animation-ruler"
        onClick={(event) => {
          const bounds = event.currentTarget.getBoundingClientRect();
          if (bounds.width > 0) onScrub(((event.clientX - bounds.left) / bounds.width) * duration);
        }}
        style={{ position: "relative", cursor: "crosshair" }}
      >
        <Playhead tick={currentTick} duration={duration} />
        {ticks.map((tick) => (
          <span key={tick} style={{ position: "absolute", left: `${(tick / duration) * 100}%`, top: 5, transform: "translateX(-50%)", color: color.text.muted, font: font.mono, fontSize: fontSize.micro }}>
            {timeLabel(tick, ticksPerSecond, display)}
          </span>
        ))}
      </div>
    </div>
  );
}

function AnnotationLane({
  label,
  duration,
  width,
  currentTick,
  onLaneClick,
  children,
}: {
  label: string;
  duration: number;
  width: number;
  currentTick: number;
  onLaneClick: (event: ReactMouseEvent<HTMLDivElement>) => void;
  children: React.ReactNode;
}) {
  return (
    <div style={{ display: "grid", gridTemplateColumns: `178px ${width}px`, minHeight: 30, borderBottom: `1px solid ${color.border.subtle}` }}>
      <div style={{ position: "sticky", left: 0, zIndex: 5, padding: `${space.xs}px ${space.sm}px`, color: color.text.secondary, background: color.bg.panel, fontSize: fontSize.meta }}>{label}</div>
      <div onClick={onLaneClick} style={{ position: "relative", minHeight: 29, cursor: "crosshair" }}>
        <Playhead tick={currentTick} duration={duration} />
        {children}
      </div>
    </div>
  );
}

function Playhead({ tick, duration }: { tick?: number; duration?: number }) {
  const left = tick === undefined || duration === undefined ? "var(--animation-playhead, 0%)" : `${(tick / duration) * 100}%`;
  return <div aria-hidden="true" style={{ position: "absolute", left, top: 0, bottom: 0, width: 1, background: color.accent.base, boxShadow: `0 0 4px ${color.accent.base}`, pointerEvents: "none", zIndex: 2 }} />;
}

function CurveEditor({
  track,
  selectedKeyId,
  onSelect,
  onDelete,
}: {
  track: AnimationTrackInfo | null;
  selectedKeyId: string | null;
  onSelect: (track: AnimationTrackInfo, key: AnimationKeyInfo) => void;
  onDelete: (track: AnimationTrackInfo, key: AnimationKeyInfo) => void;
}) {
  if (!track) return <EmptyPanelState icon="⌁" title="Select a numeric track" description="The curve view shares the same keys and playhead as the timeline." />;
  const numeric = track.keys.filter((key): key is AnimationKeyInfo & { value: number } => typeof key.value === "number");
  if (numeric.length !== track.keys.length || numeric.length === 0) {
    return <EmptyPanelState icon="⌁" title="This track has no numeric curve" description="Boolean, text, sprite-frame, and structured channels use the timeline and Step interpolation." />;
  }
  const ordered = [...numeric].sort((left, right) => left.tick - right.tick || left.id.localeCompare(right.id));
  const minTick = Math.min(...ordered.map((key) => key.tick));
  const maxTick = Math.max(...ordered.map((key) => key.tick));
  const tickSpan = Math.max(1, maxTick - minTick);
  const sampled: Array<{ tick: number; value: number }> = [];
  if (track.interpolation === "step") {
    ordered.forEach((key, index) => {
      sampled.push({ tick: key.tick, value: key.value });
      const next = ordered[index + 1];
      if (next) sampled.push({ tick: next.tick, value: key.value });
    });
  } else if (track.interpolation === "linear") {
    sampled.push(...ordered.map((key) => ({ tick: key.tick, value: key.value })));
  } else {
    const samplingTrack = track.enabled ? track : { ...track, enabled: true };
    ordered.slice(0, -1).forEach((key, index) => {
      const next = ordered[index + 1];
      const subdivisions = Math.max(8, Math.min(32, Math.ceil((next.tick - key.tick) / Math.max(1, tickSpan / 24))));
      for (let step = 0; step < subdivisions; step += 1) {
        const tick = key.tick + ((next.tick - key.tick) * step) / subdivisions;
        const value = sampleTrack(samplingTrack, tick);
        if (typeof value === "number") sampled.push({ tick, value });
      }
    });
    const last = ordered[ordered.length - 1];
    sampled.push({ tick: last.tick, value: last.value });
  }
  const sampledValues = sampled.map((item) => item.value);
  const rawMinValue = Math.min(...sampledValues, ...ordered.map((key) => key.value));
  const rawMaxValue = Math.max(...sampledValues, ...ordered.map((key) => key.value));
  const padding = rawMaxValue === rawMinValue ? Math.max(1, Math.abs(rawMinValue) * 0.1) : (rawMaxValue - rawMinValue) * 0.08;
  const minValue = rawMinValue - padding;
  const maxValue = rawMaxValue + padding;
  const valueSpan = maxValue - minValue;
  const point = (item: { tick: number; value: number }) => ({
    x: 40 + ((item.tick - minTick) / tickSpan) * 720,
    y: 220 - ((item.value - minValue) / valueSpan) * 180,
  });
  const path = sampled.map((item, index) => `${index === 0 ? "M" : "L"} ${point(item).x} ${point(item).y}`).join(" ");
  const selectedKey = ordered.find((key) => keySelectionId(track.id, key.id) === selectedKeyId) ?? null;
  const tangentHandles = selectedKey && track.interpolation === "cubic"
    ? ([
        ["in", selectedKey.inTangent, -1],
        ["out", selectedKey.outTangent, 1],
      ] as const).flatMap(([kind, tangent, direction]) => {
        if (typeof tangent !== "number") return [];
        const deltaTick = Math.max(1, tickSpan * 0.08);
        return [{ kind, point: point({ tick: selectedKey.tick + direction * deltaTick, value: selectedKey.value + direction * tangent * deltaTick }) }];
      })
    : [];
  return (
    <div data-testid="animation-curves" className="mtk-scroll" style={{ minHeight: 0, overflow: "auto", padding: space.md, background: color.bg.inset }}>
      <div style={{ display: "flex", justifyContent: "space-between", gap: space.sm, marginBottom: space.sm }}>
        <div><strong>{track.name}</strong><div style={{ color: color.text.muted, fontSize: fontSize.meta }}>{track.interpolation} · {numeric.length} numeric keys</div></div>
        <div style={{ color: color.text.muted, fontSize: fontSize.meta }}>Edit exact values and cubic tangents in the inspector.</div>
      </div>
      <svg viewBox="0 0 800 260" role="group" aria-label={`${track.name} animation curve editor`} style={{ minWidth: 620, width: "100%", height: "calc(100% - 45px)", minHeight: 190, border: `1px solid ${color.border.subtle}`, borderRadius: radius.md, background: color.bg.panel }}>
        <path data-testid="animation-curve-path" data-interpolation={track.interpolation} d={path} fill="none" stroke={color.accent.base} strokeWidth={2} />
        {selectedKey && tangentHandles.map((handle) => {
          const origin = point(selectedKey);
          return (
            <g key={handle.kind} aria-hidden="true">
              <line x1={origin.x} y1={origin.y} x2={handle.point.x} y2={Math.max(12, Math.min(248, handle.point.y))} stroke={color.warn.text} strokeWidth={1} />
              <circle cx={handle.point.x} cy={Math.max(12, Math.min(248, handle.point.y))} r={4} fill={color.bg.raised} stroke={color.warn.text} />
            </g>
          );
        })}
        {ordered.map((key) => {
          const position = point(key);
          const selected = keySelectionId(track.id, key.id) === selectedKeyId;
          return (
            <g
              key={key.id}
              role="button"
              tabIndex={0}
              aria-pressed={selected}
              aria-label={`${track.name} curve key at ${key.tick} ticks`}
              onClick={() => onSelect(track, key)}
              onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                  event.preventDefault();
                  event.stopPropagation();
                  onSelect(track, key);
                } else if ((event.key === "Delete" || event.key === "Backspace") && !track.locked) {
                  event.preventDefault();
                  event.stopPropagation();
                  onDelete(track, key);
                }
              }}
              style={{ cursor: "pointer" }}
            >
              <circle cx={position.x} cy={position.y} r={14} fill="transparent" />
              <circle cx={position.x} cy={position.y} r={selected ? 7 : 5} fill={selected ? color.accent.solid : color.bg.raised} stroke={color.accent.base} strokeWidth={2} pointerEvents="none" />
            </g>
          );
        })}
      </svg>
    </div>
  );
}

function Header({ title, detail }: { title: string; detail: string }) {
  return <div style={{ marginBottom: space.sm }}><div style={{ fontSize: fontSize.label, fontWeight: 650 }}>{title}</div><div style={{ color: color.text.muted, fontSize: fontSize.meta }}>{detail}</div></div>;
}

function Hint({ children }: { children: React.ReactNode }) {
  return <div style={{ padding: space.sm, marginTop: space.xs, borderRadius: radius.md, background: color.bg.inset, color: color.text.muted, fontSize: fontSize.meta, lineHeight: 1.45 }}>{children}</div>;
}
