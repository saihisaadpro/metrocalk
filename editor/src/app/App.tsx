//! Editor shell — wires the projection store to a (mock) core over the in-process transport and lays
//! out the panels. The viewport is a placeholder rect that demonstrates the **input-ownership
//! contract**: pointer events over it are deferred to the native wgpu layer (invariant 4), wired for
//! real in M2.6. Everything else here is UI chrome owned by React.
//!
//! M10.10 UX hardening: the **stage is layout-priority** — side panels collapse to icon rails below a
//! breakpoint so the viewport never collapses first (C8); **Play is unmistakable on the stage** — a
//! coloured border + an overlaid "● PLAYING — Esc / ⏹ to stop" badge + de-emphasised edit chrome (C2);
//! feedback lands as **toasts over the stage** (C11); a fresh scene shows a real **empty state** (C10).

import { lazy, Suspense, useEffect, useRef, useState } from "react";
import { createSession, isTauri, type EditorClient } from "../transport/session";
import { projectionStore, useDisplayedEntity, useEntityOrder, useSelectedId } from "../store/projection";
import { thumbnailStore, startThumbnailPump } from "../store/thumbnails";
import { playStore, usePlaying, usePaused } from "../store/play";
import { cinemaPreviewStore, useCinemaPreview } from "../store/cinemaPreview";
import { subjectAimStore, useSubjectAim, type AimRung } from "../store/subjectAim";
import { highlightKey, stageHighlightStore, useStageHighlight } from "../store/stageHighlight";
import { setStatus } from "../store/ui";
import { Modal, Popover } from "../theme/Popover";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { DockRail } from "../theme/workspace";
import { color, elevation, font, fontSize, motion, radius, space, z } from "../theme/tokens";
import { STAGE_MIN, type DockForm, dockForm, dockGridColumns, panelLayout } from "./layout";
import { usePlayerDrive } from "./usePlayerDrive";
import { ViewportToolbar } from "../panels/ViewportToolbar";
import { Rejections } from "../panels/Rejections";
import { StatusBar } from "../panels/StatusBar";
import { ToastHost } from "../panels/ToastHost";
import { EmptyState } from "../panels/EmptyState";
import { Onboarding } from "../panels/Onboarding";
// NOT deferred, for the reason stated beside `Onboarding` below: this is north-star #2's control and it
// is on the stage from the first frame. A front door that arrives after the first paint is a front door
// the user has already walked past.
import { DescribeBar } from "../panels/DescribeBar";
import { ImportDropOverlay } from "./ImportDropOverlay";
import { FocusBanner } from "../panels/FocusBanner";
import { SubjectAimBadge } from "../panels/SubjectAimBadge";
import type { EditorCommand } from "../panels/CommandPalette";
import { ViewportToolRail, type ViewportTool } from "../panels/ViewportToolRail";
import type { GroundSketchState, PipeForgeStatus } from "../transport/protocol";
import { EditorHeader } from "./EditorHeader";
import { LeftDock, InspectorDock, type LeftWorkspace, type InspectorWorkspace } from "./EditorDocks";
import { EngineRail, ENGINES, engineById, type EngineId } from "./EngineRail";
import { BottomDock, type AnimateWorkspace, type BottomWorkspace } from "./BottomDock";
import { onStageSurface } from "./stageInput";
import { normalizeSurfacePoint } from "./viewportCoordinates";

// WHAT MAY BE DEFERRED, AND WHY THE LIST IS SHORT. A chunk that loads on demand is absent until the
// gesture that needs it — so the only safe candidates are surfaces a user REACHES FOR: Pipe Forge
// (its viewport tool), the command palette (Ctrl/Cmd+K), the entity menu (right-click). Each carries
// a named loading state, because a gesture that produces nothing is a gesture the user repeats.
//
// `Onboarding` and `ImportDropOverlay` were on this list and are deliberately NOT any more — see
// `scripts/first-paint.json` and ADR-130. The bundle budget only ever said "smaller", and a rule that
// only says smaller, followed exactly, deletes the product; the counter-rule is declared, not inferred.
const PipeForge = lazy(() => import("../panels/PipeForge").then((module) => ({ default: module.PipeForge })));
const GroundSketch = lazy(() => import("../panels/GroundSketch").then((module) => ({ default: module.GroundSketch })));
const CommandPalette = lazy(() => import("../panels/CommandPalette").then((module) => ({ default: module.CommandPalette })));
const ContextMenu = lazy(() => import("../panels/ContextMenu").then((module) => ({ default: module.ContextMenu })));
// The export dialog reads the format catalogue and holds the fidelity ledger; a session that never
// exports should not pay for either at boot (ADR-174).
const ExportDialog = lazy(() => import("../panels/ExportDialog").then((module) => ({ default: module.ExportDialog })));
const ImportDialog = lazy(() => import("../panels/ImportDialog").then((module) => ({ default: module.ImportDialog })));

// The collapsed side rails carry only what that dock actually holds. The sub-engines are NOT repeated
// here — they live on the Engines rail, which is always visible, and listing them twice in different
// orders is exactly the confusion this layout removes.
const LEFT_RAIL_ITEMS = [
  { id: "scene", label: "Scene", icon: <Icon name="scene" size="lg" /> },
  { id: "build", label: "Build", icon: <Icon name="build" size="lg" /> },
  { id: "terrain", label: "Terrain", icon: <Icon name="terrain" size="lg" /> },
] as const;
const RIGHT_RAIL_ITEMS = [
  { id: "properties", label: "Properties", icon: <Icon name="properties" size="lg" /> },
  { id: "relations", label: "Relations", icon: <Icon name="relations" size="lg" /> },
] as const;

/** Build the editor session once: the REAL Tauri shell transport inside the packaged `.exe` (the live
 *  `/core` over the `connect` Channel), else the in-process MockCore for `npm run dev` / tests. */
function useEditorSession(): EditorClient {
  const ref = useRef<EditorClient | null>(null);
  if (!ref.current) {
    ref.current = createSession();
  }
  return ref.current;
}

const SHELL_LAYOUT_STORAGE_PREFIX = "metrocalk:shell-layout:v1:";

/** How long the cursor rests before the stage is asked what is under it, while aiming a shot. The
 *  same 140ms the subject picker's search waits, and for the same reason: the read behind it counts
 *  DRAWN PARTS for every rung of the chain, which walks every published instance in the scene. */
const AIM_HOVER_MS = 140;

function useRememberedBoolean(key: string, fallback: boolean) {
  const [value, setValue] = useState(() => {
    try {
      const stored = window.localStorage.getItem(`${SHELL_LAYOUT_STORAGE_PREFIX}${key}`);
      return stored == null ? fallback : stored === "true";
    } catch {
      return fallback;
    }
  });
  useEffect(() => {
    try {
      window.localStorage.setItem(`${SHELL_LAYOUT_STORAGE_PREFIX}${key}`, String(value));
    } catch {
      // A locked-down WebView still keeps the preference for this session.
    }
  }, [key, value]);
  return [value, setValue] as const;
}

/** The persistent "● PLAYING" badge overlaid ON the stage (not only the toolbar) — Play must be
 *  unmistakable where the user is looking (C2). Stop is always one click away here too. */
function PlayBadge({ paused, onStop }: { paused: boolean; onStop: () => void }) {
  return (
    <div
      id="playStageBadge"
      data-testid="playStageBadge"
      // A DOM overlay ON the stage must not also DRIVE the stage — and that is decided ONCE, at the
      // seam, by `stageInput.ts`'s `onStageSurface`. The two `stopPropagation` handlers that used to
      // sit here were the superseded per-overlay idiom, kept alive by a comment claiming they were
      // load-bearing; they were not, and the claim is what made the next author copy them onto a new
      // badge. Measured both ways: removing them changes no assertion, and defeating `onStageSurface`
      // turns all three `StageOverlays.test.tsx` cases red — with them present, only one went red.
      style={{
        position: "absolute",
        top: space.lg,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: z.badge,
        display: "flex",
        alignItems: "center",
        gap: space.md,
        padding: `${space.xs}px ${space.lg}px`,
        borderRadius: radius.pill,
        background: paused ? color.warn.bg : color.success.bg,
        border: `1px solid ${paused ? color.warn.border : color.success.border}`,
        color: paused ? color.warn.text : color.success.text,
        font: font.mono,
        fontSize: fontSize.body,
        boxShadow: elevation.e2,
      }}
    >
      <span data-state={paused ? "paused" : "playing"} style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
        {paused ? <Icon name="pause" size={12} /> : <span aria-hidden className="mtk-live-dot" />}
        {paused ? "PAUSED" : "PLAYING"}
      </span>
      <span style={{ color: color.text.muted }}>— Esc or</span>
      <Button data-testid="stageStop" variant="danger" compact onClick={onStop}>
        <Icon name="stop" size="sm" />
        Stop
      </Button>
    </div>
  );
}

/** The "◉ PREVIEW" badge overlaid ON the stage while the cutscene camera holds the viewport.
 *
 *  The same rule `PlayBadge` exists for: a mode that changes what the viewport MEANS has to be
 *  unmistakable where the user is looking, and the way out has to be one click from there. A held
 *  preview is easy to miss — the picture is a good picture, it just is not the author's camera, and
 *  orbiting will not move it. Without this the only control that could release it lives in the
 *  bottom dock, which the author may have closed. */
function PreviewBadge({
  shotIndex,
  shots,
  subjectName,
  blending,
  onExit,
}: {
  shotIndex: number | null;
  shots: number;
  subjectName: string;
  blending: boolean;
  onExit: () => void;
}) {
  return (
    <div
      id="cinemaPreviewBadge"
      data-testid="cinemaPreviewBadge"
      // NO `stopPropagation` here, deliberately. A DOM overlay on the stage must not also DRIVE the
      // stage, and `stageInput.ts` decides that once for every overlay from the browser's own hit
      // test — a per-overlay guard is the superseded idiom whose whole failure mode was five
      // overlays stopping five different subsets of six events. Proven, not assumed: adding the two
      // handlers back changes no assertion in `StageOverlays.test.tsx`, and defeating
      // `onStageSurface` turns it red.
      style={{
        position: "absolute",
        top: space.lg,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: z.badge,
        display: "flex",
        alignItems: "center",
        gap: space.md,
        padding: `${space.xs}px ${space.lg}px`,
        borderRadius: radius.pill,
        background: color.info.bg,
        border: `1px solid ${color.info.border}`,
        color: color.info.text,
        font: font.mono,
        fontSize: fontSize.body,
        boxShadow: elevation.e2,
      }}
    >
      <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
        <Icon name="camera" size={12} />
        PREVIEW
      </span>
      <span data-testid="cinemaPreviewBadgeShot" style={{ color: color.text.secondary }}>
        {shotIndex === null
          ? subjectName
          : `${blending ? "transition into shot" : "shot"} ${shotIndex + 1} of ${shots} · ${subjectName}`}
      </span>
      <Button data-testid="stageExitPreview" variant="secondary" compact onClick={onExit}>
        Exit
      </Button>
    </div>
  );
}

function effectiveViewportWidth(): number {
  if (typeof window === "undefined" || typeof document === "undefined") return 1440;
  const cssZoom = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("zoom"));
  return window.innerWidth / (Number.isFinite(cssZoom) && cssZoom > 0 ? cssZoom : 1);
}

export function App() {
  const client = useEditorSession();
  usePlayerDrive(client); // arrows / WASD move the Player role while Play runs
  const native = isTauri(); // inside the packaged .exe the viewport is the real wgpu region (composite)
  // Read here rather than inside the badge so the badge stays a pure presentation component that a
  // test can render with any state, including the ones the store cannot reach on its own.
  const cinemaPreview = useCinemaPreview();
  // The M3.3 right-click context menu, opened for an entity at a cursor position.
  const [ctx, setCtx] = useState<{ id: string; x: number; y: number } | null>(null);
  // M3.3 focus mode — the framed entity + its camera distance (read from `focus_debug`); drives the banner.
  const [focused, setFocused] = useState<{ id: string; dist: number } | null>(null);
  // Tracks a right-press for the orbit-vs-context-menu movement threshold (the scaffold's disambiguation).
  const rightDrag = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  // M9 gizmo handle-drag: set by a left-press that HIT a gizmo handle (so the click doesn't re-pick + the
  // release commits). A ref (not state) so the click/up guards read it synchronously off the hot path.
  const gizmoHit = useRef(false);
  const [pipeStatus, setPipeStatus] = useState<PipeForgeStatus | null>(null);
  const pipeActive = pipeStatus?.active === true;
  // The ground sketch is armed by the TOOL, not by a session the engine owns: the corners survive a
  // trip to another tool on purpose (a half-drawn outline is work), so "is it armed" is the rail's
  // answer and the read-model is only what the outline currently measures.
  const [sketch, setSketch] = useState<GroundSketchState | null>(null);
  const [sketchBusy, setSketchBusy] = useState(false);
  // AIMING A SHOT BY POINTING AT THE THING (`store/subjectAim`). Started from a shot's Frames picker
  // in the bottom dock, lived here: while it is on, a left click on the stage names an object for a
  // shot to film instead of selecting it — which it MUST NOT do, because the Cutscene panel is bound
  // to the selection and selecting the object would switch which cutscene is on screen.
  const aim = useSubjectAim();
  const aimActive = aim.active;
  // The ladder for an object, kept for the length of one aim. A sweep back and forth over the same
  // two parts is one read each, not one per settle — and the read is a scene-wide count of drawn
  // parts, which on the imported production line walks 15,711 instances.
  const aimLadder = useRef<Map<string, AimRung[]>>(new Map());
  const aimHoverTimer = useRef<number | null>(null);
  // Replies can land out of order once the cursor has moved on. Only the newest is allowed to speak.
  const aimHoverSeq = useRef(0);
  const aimHoverAt = useRef<string | null>(null);
  const [pipeBusy, setPipeBusy] = useState(false);
  const [activeTool, setActiveTool] = useState<ViewportTool>("select");
  const [leftWorkspace, setLeftWorkspace] = useState<LeftWorkspace>("scene");
  // Which sub-engine the author is in. One piece of state for the whole rail, so "where am I?" always has
  // exactly one answer — the thing the old three-dock arrangement could not provide.
  const [engine, setEngine] = useState<EngineId>("scene");
  const [inspectorWorkspace, setInspectorWorkspace] = useState<InspectorWorkspace>("properties");
  const [bottomWorkspace, setBottomWorkspace] = useState<BottomWorkspace>("asset");
  // Which of Animate's two timelines is showing. Lifted here so `openCutscene` can land on the one it
  // names — see `BottomDockProps.animate`.
  const [animateWorkspace, setAnimateWorkspace] = useState<AnimateWorkspace>("properties");
  const [bottomOpen, setBottomOpen] = useState(false);
  const [commandsOpen, setCommandsOpen] = useState(false);
  const [exportOpen, setExportOpen] = useState(false);
  const [importOpen, setImportOpen] = useState(false);
  const [vw, setVw] = useState(effectiveViewportWidth);
  const [leftDockCollapsed, setLeftDockCollapsed] = useRememberedBoolean("left-collapsed", false);
  const [rightDockCollapsed, setRightDockCollapsed] = useRememberedBoolean("right-collapsed", vw < 1200);
  const [toolRailMinimized, setToolRailMinimized] = useRememberedBoolean("tool-rail-minimized", true);
  const [dockFlyout, setDockFlyout] = useState<"left" | "right" | null>(null);
  const dockFlyoutAnchor = useRef<HTMLElement | null>(null);

  const playing = usePlaying();
  const paused = usePaused();
  const order = useEntityOrder();
  const selectedId = useSelectedId();
  const selectedEntity = useDisplayedEntity(selectedId ?? "");
  const editablePipeId = selectedId && selectedEntity?.components.PipeRecipe ? selectedId : null;
  const sceneEmpty = order.length === 0;

  useEffect(() => {
    if (!playing || !focused) return;
    client.unfocus();
    setFocused(null);
  }, [client, focused, playing]);

  // Emit a stable "connected · N entities" status the FIRST time the projection streams in (the scaffold's
  // connect signal the prompt-40 black-box E2E keys on — an intentional, stable token, not cosmetic copy).
  const connectedRef = useRef(false);
  useEffect(() => {
    if (!connectedRef.current && order.length > 0) {
      connectedRef.current = true;
      setStatus(`connected · ${order.length} entities`);
    }
  }, [order.length]);

  // Wire the live-thumbnail store (M14.2 / ADR-058): give it the renderer seam, pick the min-spec budget on
  // a low-core machine (the Rust command also caps resolution by MTK_PROFILE), and start the off-frame drain
  // pump (the dirty backlog refreshes without waiting on a scroll). Cleaned up on unmount.
  useEffect(() => {
    const t = thumbnailStore.getState();
    t.setClient(client);
    const cores = typeof navigator !== "undefined" ? (navigator.hardwareConcurrency ?? 8) : 8;
    t.setMinSpec(cores <= 4);
    const stop = startThumbnailPump();
    return () => {
      stop();
      thumbnailStore.getState().setClient(null);
    };
  }, [client]);

  // Reconnect the overlay to native render-only tool state after a WebView refresh. The editable route
  // lives in Rust; React is only its controller/read-model.
  useEffect(() => {
    void client.pipeForgeStatus().then(setPipeStatus).catch(() => {});
  }, [client]);

  // Native project/play transitions can cancel a render-only route without remounting the WebView. While a
  // route is live, reconcile the small authoritative status at chrome cadence so the UI cannot keep claiming
  // a stale drawing session after New/Open/Play. This is never a viewport-frame poll.
  useEffect(() => {
    if (!pipeActive) return;
    const timer = window.setInterval(() => {
      void client.pipeForgeStatus().then((status) => {
        setPipeStatus(status);
        if (!status.active) setPipeBusy(false);
      }).catch(() => {});
    }, 750);
    return () => window.clearInterval(timer);
  }, [client, pipeActive]);

  // Responsive layout — the stage is layout-priority; panels collapse to rails below a breakpoint (C8).
  useEffect(() => {
    const onResize = () => setVw(effectiveViewportWidth());
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  const layout = panelLayout(vw);
  // THE SAME QUESTION ON THE OTHER AXIS, AND IT HAS TO BE MEASURED. `panelLayout` reads the window
  // width because the side tracks are the only thing between the window's two edges; the stage column
  // is NOT the window's height — the header and the status bar take 81px of it — and reconstructing
  // that from constants is the second-statement-of-a-number defect this shell has been repairing for
  // four ADRs. So the column is observed, and the two dock minimums are read off the live element
  // rather than copied into TypeScript: `--mtk-bottom-bar-height` is genuinely 48px under
  // `(pointer: coarse)`, so a copy here would be wrong on every touch device.
  //
  // A ResizeObserver and not a `resize` listener: the column's height changes when the *chrome* does
  // (the header collapses its own rows at 760px), not only when the window does. It fires on layout
  // changes only — never per frame — so the hot path (invariant 4) is untouched.
  const stageColumn = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const [dockShape, setDockShape] = useState<DockForm>("docked");
  useEffect(() => {
    const el = stageColumn.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const read = () => {
      const cs = getComputedStyle(el);
      const px = (name: string) => Number.parseFloat(cs.getPropertyValue(name)) || 0;
      setDockShape(dockForm(el.clientHeight, px("--mtk-bottom-bar-height"), px("--mtk-dock-content-min")));
    };
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // TELL THE RENDERER WHERE THE 3D IS ACTUALLY VISIBLE.
  //
  // The wgpu surface is the whole window; this UI is composited on top of it and the viewport is a
  // transparent hole. The renderer could not see that, so `frame_all` and `focus_entity` fitted and
  // CENTRED their subject on the window -- and with a dock open on each side the window centre is not
  // inside the hole at all. In the production factory captures the whole imported assembly sat against
  // the viewport's left edge with part of the framed area behind panels.
  //
  // Same discipline as the dock observer above: a ResizeObserver on the element itself, so it fires on
  // layout changes (a dock collapsing, the drawer opening) and never per frame. Fractions rather than
  // pixels, so DPI scaling and the WebView's own zoom cannot make the two sides disagree.
  useEffect(() => {
    const el = viewportRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    let last = "";
    const report = () => {
      const rect = el.getBoundingClientRect();
      const w = window.innerWidth;
      const h = window.innerHeight;
      if (!(w > 0 && h > 0 && rect.width > 0 && rect.height > 0)) return;
      const next = { x: rect.left / w, y: rect.top / h, width: rect.width / w, height: rect.height / h };
      // Only on a real change: an observer that fires during an animated collapse would otherwise
      // send a command per frame of the transition.
      const key = `${next.x.toFixed(4)}|${next.y.toFixed(4)}|${next.width.toFixed(4)}|${next.height.toFixed(4)}`;
      if (key === last) return;
      last = key;
      client.reportViewportRect(next);
    };
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    window.addEventListener("resize", report);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", report);
    };
  }, [client]);
  // Which collapsed panel is currently opened as an overlay drawer.
  const [drawer, setDrawer] = useState<"left" | "right" | null>(null);
  // The stage is under a sheet: the dock is in its floating form AND open. Read by the stage's own
  // overlays, which withdraw rather than sit underneath it (see the tool rail below).
  const stageSheet = dockShape === "sheet" && bottomOpen;
  // The stage is the drawing surface only while it IS the stage: in Play, or under a dock sheet, the
  // tool rail is withdrawn and a click means whatever that mode means.
  const drawActive = activeTool === "draw" && !playing && !stageSheet;
  useEffect(() => {
    if (!layout.collapsed) setDrawer(null); // widening back out closes the responsive drawer
    if (layout.collapsed || layout.overlay) setDockFlyout(null);
  }, [layout.collapsed, layout.overlay]);

  // Crossing into the compact shell persists both docks as rails. Widening therefore grows the stage
  // instead of unexpectedly restoring panels and shrinking it; users pin back only the context they need.
  useEffect(() => {
    if (layout.collapsed) {
      setLeftDockCollapsed(true);
      setRightDockCollapsed(true);
      return;
    }
    if (!layout.collapsed && vw < 1200) setRightDockCollapsed(true);
  }, [layout.collapsed, setLeftDockCollapsed, setRightDockCollapsed, vw]);

  function openLeft(workspace: LeftWorkspace) {
    setLeftWorkspace(workspace);
    if (layout.collapsed) setDrawer("left");
    else if (leftDockCollapsed) setLeftDockCollapsed(false);
    setDockFlyout(null);
  }

  function openInspector(workspace: InspectorWorkspace = "properties") {
    setInspectorWorkspace(workspace);
    if (layout.collapsed) setDrawer("right");
    else if (rightDockCollapsed) setRightDockCollapsed(false);
    setDockFlyout(null);
  }

  function openBottom(workspace: BottomWorkspace) {
    setBottomWorkspace(workspace);
    setBottomOpen(true);
  }

  /** The Cinematics block's deep link: the Animate dock, open, on the Cutscene timeline. */
  function openCutscene() {
    setAnimateWorkspace("cutscene");
    openEngine("animate");
  }

  /**
   * The one router. Every way of reaching a sub-engine — the rail, the command palette, a deep link from
   * another panel — goes through here, so the active engine and the surface showing it can never disagree.
   */
  function openEngine(id: EngineId) {
    setEngine(id);
    const def = engineById(id);
    if (def.surface === "side") {
      openLeft(id as LeftWorkspace);
    } else {
      // The bottom-dock engines keep their existing ids; only the rail's vocabulary is new.
      openBottom((id === "model" ? "asset" : id === "animate" ? "animation" : id) as BottomWorkspace);
    }
  }

  function activateDockRail(side: "left" | "right", workspace: string, anchor: HTMLButtonElement) {
    if (side === "left") setLeftWorkspace(workspace as LeftWorkspace);
    else setInspectorWorkspace(workspace as InspectorWorkspace);
    if (layout.collapsed) {
      setDrawer(side);
      return;
    }
    dockFlyoutAnchor.current = anchor;
    setDockFlyout(side);
  }

  function chooseTool(tool: ViewportTool) {
    if (pipeBusy) return;
    if (pipeActive && tool !== "pipe") {
      void client.pipeForgeCancel().then((status) => {
        setPipeStatus(status);
        setStatus(status.message);
      });
    }
    // Arming is the tool's job, and DISARMING keeps the corners: an author who steps away to move
    // something and comes back has not thrown their outline away. `Start over` is how you do that,
    // and it says so.
    if (tool === "draw") {
      void client
        .sketchTool(true, sketch?.gridM, sketch?.angleSnap ?? true)
        .then((next) => {
          setSketch(next);
          setStatus(next.message);
        })
        .catch((err) => console.error("sketch_tool failed", err));
    } else if (activeTool === "draw") {
      void client
        .sketchTool(false)
        .then(setSketch)
        .catch((err) => console.error("sketch_tool failed", err));
    }
    setActiveTool(tool);
    if (tool === "move" || tool === "rotate" || tool === "scale") {
      client.gizmoMode(tool === "move" ? "translate" : tool);
    }
  }

  // WHAT THE CURSOR IS OVER, ON HOVER-SETTLE - never per frame (invariant 4). Two reads, and the
  // second is skipped whenever the peek names the same object as last time, or one already read
  // during this aim: `viewport_peek` is a ray against the scene BVH, but the chain read counts drawn
  // parts for every rung, which on the imported production line walks 15,711 instances.
  const aimHover = (clientX: number, clientY: number) => {
    if (aimHoverTimer.current !== null) window.clearTimeout(aimHoverTimer.current);
    const { x, y } = normalizeSurfacePoint(clientX, clientY);
    aimHoverTimer.current = window.setTimeout(() => {
      const seq = ++aimHoverSeq.current;
      const aiming = () => seq === aimHoverSeq.current && subjectAimStore.getState().active;
      if (!aiming()) return;
      subjectAimStore.getState().look(true);
      void client
        .viewportPeek(x, y)
        .then(async (id) => {
          if (!aiming()) return;
          if (!id) {
            aimHoverAt.current = null;
            subjectAimStore.getState().hover([]);
            // Over empty space is an ANSWER, not a missing one: the cue goes out with the rungs, so
            // the picture and the badge never disagree about whether the cursor is on something.
            stageHighlightStore.getState().show("stage", []);
            return;
          }
          // The stage lights the LEAF, because that is what a click on the stage takes. A rung the
          // pointer is on overrides it (`show("rung", …)`), which is what makes the ladder legible:
          // the badge offers `Box · 1 part` in `Assembly Hall · 7 parts`, and now the seven are
          // visible before the shot is committed to them.
          stageHighlightStore.getState().show("stage", [id]);
          if (aimHoverAt.current === id) {
            subjectAimStore.getState().look(false);
            return;
          }
          aimHoverAt.current = id;
          const cached = aimLadder.current.get(id);
          if (cached) {
            subjectAimStore.getState().hover(cached);
            return;
          }
          const chain = await client.cinemaSubjectChain(id);
          if (!aiming()) return;
          // The rungs are the ENGINE's rows, in the engine's order, under the engine's headings -
          // read, never re-ranked, so the badge cannot offer a chain the scene does not have.
          const rungs: AimRung[] = chain.candidates.map((row) => ({
            id: row.id,
            name: row.name,
            parts: row.parts,
            group: row.group,
          }));
          aimLadder.current.set(id, rungs);
          subjectAimStore.getState().hover(rungs);
        })
        .catch(() => subjectAimStore.getState().look(false));
    }, AIM_HOVER_MS);
  };

  // The pending hover dies with the mode: a settle that lands after a Cancel would otherwise repaint
  // a badge that is no longer on screen, and the ladder cache belongs to one aim, not to the session.
  useEffect(() => {
    if (aimActive) return;
    if (aimHoverTimer.current !== null) window.clearTimeout(aimHoverTimer.current);
    aimHoverTimer.current = null;
    aimHoverSeq.current += 1;
    aimHoverAt.current = null;
    aimLadder.current.clear();
    // The cue belongs to the gesture. A highlight surviving the mode would leave the stage claiming
    // the cursor is over something in a viewport that has gone back to selecting.
    stageHighlightStore.getState().reset();
  }, [aimActive]);

  // THE ONE PLACE THE STAGE HIGHLIGHT CROSSES THE BOUNDARY. Every surface that points at something
  // writes to `store/stageHighlight`; this sends the result once, when the ANSWER changes — not per
  // frame, not per mouse move, not once per surface (invariant 4). `highlightKey` is what makes the
  // dependency the answer rather than the array identity.
  const stageHighlight = useStageHighlight();
  const stageHighlightKey = highlightKey(stageHighlight);
  useEffect(() => {
    void client.viewportHover(stageHighlight.ids);
    // The cleanup does NOT clear: it runs on every change of the key, and clearing there would send
    // `[]` between every two hovers — twice the IPC, and a visible flicker between two rungs.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, stageHighlightKey]);

  // The stage is not lit while Play owns the camera: the cue is editor chrome, and a shot must not
  // contain it — the same rule the cinema preview applies to the selection outline.
  useEffect(() => {
    if (playing) stageHighlightStore.getState().reset();
  }, [playing]);

  // Ctrl-Z / ⌘-Z → undo; Escape closes the context menu, then a drawer, then STOPS Play (the badge says
  // "Esc … to stop"). A discrete event — never the per-frame hot path (invariant 4).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = (e.target instanceof HTMLElement ? e.target : document.activeElement) as HTMLElement | null;
      // Timeline selects, buttons, sliders, listboxes and editable inspectors own their keystrokes. Treat
      // every interactive command scope as editing so W/E/R/F cannot unexpectedly switch viewport tools.
      const editing = !!el && !!el.closest(
        "input, textarea, select, button, [contenteditable='true'], [role='button'], [role='slider'], [role='listbox'], [role='menu'], [data-command-scope]",
      );
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setCommandsOpen(true);
        return;
      }
      const redoGesture =
        (e.ctrlKey || e.metaKey) &&
        (e.key.toLowerCase() === "y" || (e.shiftKey && e.key.toLowerCase() === "z"));
      if (redoGesture) {
        if (editing) return;
        e.preventDefault();
        if (pipeActive) {
          setStatus("Finish or cancel the active route before redoing scene changes");
          return;
        }
        void client.redo().then((did) => setStatus(did ? "redo" : "nothing to redo"));
        return;
      }
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "z") {
        // Don't hijack TEXT undo while the user is typing in a field — only undo the SCENE otherwise.
        if (editing) return;
        e.preventDefault();
        if (pipeActive && !pipeBusy) {
          void client.pipeForgeUndo().then((status) => {
            setPipeStatus(status);
            setStatus(status.message);
          });
          return;
        }
        // Honest feedback: only say "undo" when a transaction was actually reverted (the shell reports it),
        // else "nothing to undo" — never claim a revert on an empty history.
        void client.undo().then((did) => setStatus(did ? "undo" : "nothing to undo"));
      }
      // M9 gizmo mode — the universal W/E/R game-editor shortcut (sticky tool state; guarded off text fields
      // + modifier chords so it never fires while editing or during Ctrl-Z). A discrete command, not per-frame.
      const k = e.key.toLowerCase();
      if ((k === "w" || k === "e" || k === "r") && !e.ctrlKey && !e.metaKey && !e.altKey && !editing && !pipeActive && !pipeBusy) {
        chooseTool(k === "w" ? "move" : k === "e" ? "rotate" : "scale");
        setStatus(k === "w" ? "move (W)" : k === "e" ? "rotate (E)" : "scale (R)");
      }
      // D for draw, beside W/E/R: the ground sketch is a primary tool, not a panel setting.
      if (k === "d" && !e.ctrlKey && !e.metaKey && !e.altKey && !editing && !pipeActive && !pipeBusy) {
        chooseTool(activeTool === "draw" ? "select" : "draw");
        setStatus(activeTool === "draw" ? "select (D)" : "Draw on the ground (D) — click to place a corner");
      }
      if (k === "f" && !e.ctrlKey && !e.metaKey && !e.altKey && !editing && !pipeActive && !pipeBusy) {
        e.preventDefault();
        void client.gizmoSelected().then((id) => {
          if (id) {
            client.focusEntity(id);
            setStatus("Framed the selection (F)");
          } else {
            setStatus("Select something to frame");
          }
        });
      }
      if (e.key === "Escape") {
        // FIRST, because it is the most recently entered mode and the badge promises it by name. An
        // aim is also the only one of these the user is holding the mouse for.
        if (subjectAimStore.getState().active) {
          subjectAimStore.getState().cancel();
          setStatus("Aiming cancelled — the shot still frames what it did");
          return;
        }
        if (pipeActive && !pipeBusy) {
          void client.pipeForgeCancel().then((status) => {
            setPipeStatus(status);
            setStatus(status.message);
          });
          return;
        }
        if (drawActive) {
          // Leaves the TOOL, not the work: the corners are still there when the tool comes back, and
          // `Start over` is the control that throws them away.
          chooseTool("select");
          setStatus("Left the drawing tool — the outline is kept");
          return;
        }
        if (ctx) {
          setCtx(null);
          return;
        }
        if (focused) {
          // Exit focus mode: restore the camera + drop the dim flag, clear the banner, emit the stable status.
          client.unfocus();
          setFocused(null);
          setStatus("focus cleared");
          return;
        }
        if (drawer) {
          setDrawer(null);
          return;
        }
        if (playing) stopPlay();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, ctx, drawer, playing, focused, pipeActive, pipeBusy, activeTool, drawActive]);

  function stopPlay() {
    void client
      .stop()
      .then((info) => {
        playStore.getState().refresh(info);
        setStatus("stopped");
      })
      .catch((e) => console.error("stop failed", e));
  }

  // Edit chrome is de-emphasised while playing (the mode switch is felt in peripheral vision; edits are
  // also gated off on the shell).
  const chromeDim: React.CSSProperties = playing
    ? { opacity: 0.45, pointerEvents: "none", transition: `opacity ${motion.base}` }
    : { transition: `opacity ${motion.base}` };

  // `opacity` composites the element AND ITS OWN BACKGROUND, which is fine in a browser sitting on an
  // opaque body and wrong in the packaged app: the native root is deliberately transparent so the wgpu
  // viewport can show through, so a 45%-opaque dock let the 3D scene bleed into the side panels the
  // moment Play started. Seen in an OS capture of a Play session — a crate and its shadow drawn across
  // the object list. The dock therefore keeps a FULLY OPAQUE background and dims only its contents:
  // `.mtk-shell-track` (the ground) and `.mtk-shell-card` (the panel) both paint, and `chromeDim`
  // below is applied to the CONTENT, never to either surface.
  // A BLOCK CONTAINER DOES NOT CONSTRAIN THE PANEL INSIDE IT, AND THE PANEL WAS COUNTING ON IT.
  // `WorkspacePanel` is `display: flex; flex-direction: column` whose body is `flex: 1 1 auto;
  // min-height: 0` — an arrangement that only means anything if the panel itself has been given a
  // height. In a `display: block` box it takes its CONTENT height instead, and `overflow: hidden`
  // here then cuts it with no scrollbar and no mark. Measured in `shell-build` at a 1000×900 window:
  // the Build workspace laid out at **1274px inside an 819px track** and **16 controls were entirely
  // below the cut** — `describebar`, `describe`, `describeBtn` (describe-to-create, north-star test
  // #2), the whole `assetbrowser` with its search and categories, `create-import`, `create-pipe`,
  // `combine-meld`. Not truncated: absent, with nothing on screen saying so. The scene asserted
  // `authbar` was PRESENT, which it was, in the DOM, 400px below the bottom of its own panel.
  //
  // One word fixes it, because the panel was always written to fill a flex column. Paired with
  // `.mtk-dock-panel.mtk-scroll { overflow: auto }`, which is the other half: the JSX has said
  // `mtk-scroll` on those panels since they were written, and `.mtk-scroll` is a scrollbar SKIN that
  // has never set `overflow` — a class asking for behaviour no rule provided.
  const dockContent: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    height: "100%",
    minHeight: 0,
    overflow: "hidden",
    ...chromeDim,
  };

  const selectCreated = (id: string | null) => {
    if (!id) return;
    projectionStore.getState().select(id);
    void client.gizmoSelect(id);
  };

  // ADR-178 — every way into an import opens the SAME dialog. It used to call the native picker
  // straight from four places (the menu, the palette, the empty state, "import another" after a
  // drop), which is four callers each writing their own idea of what happens afterwards.
  const importAsset = () => setImportOpen(true);

  const commands: EditorCommand[] = [
    // Generated from the SAME list the rail renders, so the palette can never drift out of sync with it.
    ...ENGINES.map((e) => ({
      id: `workspace-${e.id}`,
      label: `Open ${e.label}`,
      category: "Workspaces",
      description: e.blurb,
      execute: () => openEngine(e.id),
    })),
    { id: "workspace-properties", label: "Open Properties", category: "Workspaces", description: "Inspect the selected object", execute: () => openInspector("properties") },
    { id: "workspace-relations", label: "Open Relations", category: "Workspaces", description: "Inspect bindings and graph relationships", execute: () => openInspector("relations") },
    { id: "workspace-problems", label: "Open Problems", category: "Workspaces", description: "Selection diagnostics and actionable issues", execute: () => openBottom("problems") },
    { id: "workspace-runtime", label: "Open Runtime", category: "Workspaces", description: "Live rule decisions during Play", execute: () => openBottom("runtime") },
    { id: "tool-select", label: "Select tool", category: "Viewport tools", description: "Select objects in the viewport", execute: () => chooseTool("select") },
    { id: "tool-move", label: "Move tool", category: "Viewport tools", shortcut: "W", disabled: !selectedId, disabledReason: "Select an object first", execute: () => chooseTool("move") },
    { id: "tool-rotate", label: "Rotate tool", category: "Viewport tools", shortcut: "E", disabled: !selectedId, disabledReason: "Select an object first", execute: () => chooseTool("rotate") },
    { id: "tool-scale", label: "Scale tool", category: "Viewport tools", shortcut: "R", disabled: !selectedId, disabledReason: "Select an object first", execute: () => chooseTool("scale") },
    { id: "tool-draw", label: "Draw on the ground", category: "Create", description: "Trace an outline in the viewport at real size, then raise it into a solid", keywords: ["sketch", "outline", "footprint", "extrude", "plan", "building", "floor"], shortcut: "D", disabled: playing, disabledReason: "Stop Play before drawing", execute: () => chooseTool("draw") },
    { id: "tool-pipe", label: "Draw pipe", category: "Create", description: "Author and bake a routed PBR asset in the viewport", keywords: ["pipe forge", "procedural"], disabled: playing, disabledReason: "Stop Play before authoring", execute: () => chooseTool("pipe") },
    { id: "create-entity", label: "Create empty entity", category: "Create", description: "Add a named object at the origin", execute: async () => selectCreated(await client.createEntity(0, 1, 0, "Entity")) },
    { id: "create-light", label: "Add point light", category: "Create", description: "Add a warm point light above the origin", execute: async () => selectCreated(await client.addLight("point", 0, 4, 0, 1, 0.96, 0.9, 60)) },
    { id: "import-asset", label: "Import a file…", category: "File", description: "See what this build reads, then choose a file", keywords: ["fbx", "gltf", "glb", "obj", "step", "3dxml", "usd", "import"], execute: () => setImportOpen(true) },
    // The palette had no File category at all, so the one command with a real cost attached — writing
    // the scene out — was reachable only by finding the menu it lived in.
    { id: "file-export", label: "Export scene…", category: "File", description: "Choose a format, see what it carries, and write the scene", keywords: ["glb", "gltf", "usd", "usda", "step", "save as", "write"], execute: () => setExportOpen(true) },
    { id: "view-frame-all", label: "Frame all", category: "View", description: "Fit the whole scene in the viewport", execute: () => client.frameAll() },
    { id: "view-top", label: "Top view", category: "View", execute: () => client.viewPreset("top") },
    { id: "view-front", label: "Front view", category: "View", execute: () => client.viewPreset("front") },
    { id: "view-side", label: "Side view", category: "View", execute: () => client.viewPreset("side") },
    { id: "view-perspective", label: "Perspective view", category: "View", execute: () => client.viewPreset("persp") },
    { id: "edit-undo", label: "Undo", category: "Edit", shortcut: ["Ctrl", "Z"], execute: async () => setStatus((await client.undo()) ? "Undo complete" : "Nothing to undo") },
    { id: "edit-redo", label: "Redo", category: "Edit", shortcut: ["Ctrl", "Shift", "Z"], execute: async () => setStatus((await client.redo()) ? "Redo complete" : "Nothing to redo") },
    {
      id: playing ? "play-stop" : "play-start",
      label: playing ? "Stop Play" : "Start Play",
      category: "Run",
      description: playing ? "Restore the pre-Play editing state" : "Test the scene in a disposable runtime session",
      execute: async () => {
        const info = playing ? await client.stop() : await client.play();
        playStore.getState().refresh(info);
        setStatus(playing ? "Stopped" : "Playing");
      },
    },
  ];

  const leftPanel = (
    <LeftDock
      client={client}
      active={leftWorkspace}
      onStartPipe={() => {
        setActiveTool("pipe");
        setDrawer(null);
        setDockFlyout(null);
      }}
      onStartDraw={() => {
        // Through `chooseTool`, not `setActiveTool`: arming the native tool is part of what choosing
        // it MEANS, and a second way in that skipped it would arm the panel and not the stage.
        chooseTool("draw");
        setDrawer(null);
        setDockFlyout(null);
      }}
      onImport={importAsset}
      onOpenCutscene={openCutscene}
      onContextMenu={(id, x, y) => {
        if (!playing) setCtx({ id, x, y });
      }}
      onCollapse={!layout.collapsed && !leftDockCollapsed ? () => {
        setLeftDockCollapsed(true);
        setDockFlyout(null);
      } : undefined}
      onPin={!layout.collapsed && leftDockCollapsed ? () => {
        setLeftDockCollapsed(false);
        setDockFlyout(null);
      } : undefined}
    />
  );
  const rightPanel = (
    <InspectorDock
      client={client}
      active={inspectorWorkspace}
      onChange={setInspectorWorkspace}
      onJumpTo={(target) => openEngine(target)}
      onCollapse={!layout.collapsed && !rightDockCollapsed ? () => {
        setRightDockCollapsed(true);
        setDockFlyout(null);
      } : undefined}
      onPin={!layout.collapsed && rightDockCollapsed ? () => {
        setRightDockCollapsed(false);
        setDockFlyout(null);
      } : undefined}
    />
  );

  return (
    // Root is the chrome backdrop. In the .exe (`native`) it is **transparent** so the native wgpu scene
    // composites up through the transparent viewport hole (ADR-008) — the panels below paint their OWN
    // opaque background so only the viewport stays a hole. (Any opaque root here would occlude the wgpu
    // layer even behind the transparent viewport div — the bug that left the .exe viewport black.)
    <div className="mtk-editor-root" style={{ height: "100vh", display: "flex", flexDirection: "column", background: native ? "transparent" : color.bg.base, color: color.text.primary, font: font.ui }}>
      <EditorHeader
        client={client}
        compact={layout.collapsed}
        onOpenCommands={() => setCommandsOpen(true)}
        onExport={() => setExportOpen(true)}
        onImport={() => setImportOpen(true)}
        onOpenLeftDock={() => setDrawer("left")}
        onOpenRightDock={() => setDrawer("right")}
      />
      <div style={{ flex: 1, display: "grid", gridTemplateColumns: dockGridColumns(layout, leftDockCollapsed, rightDockCollapsed, vw), minHeight: 0 }}>
        {/* ENGINES — the one index of what this editor can do. Always the first column (except in the
            phone-width overlay layout, where the whole shell is a single column). */}
        {!layout.overlay && (
          // Same reason as the docks: the rail dims over an opaque backing rather than dimming its own
          // background, or the viewport shows through it during Play.
          <div className="mtk-shell-track mtk-shell-track--engines">
            {/* The CARD is the opaque surface and the RAIL is the content, in that order and never
                merged: `chromeDim` is an `opacity`, and an opacity composites the element together
                with its own background. Put it on the painted surface and Play makes the panel 45%
                transparent over a deliberately transparent native root — the crate-through-the-
                object-list bleed the comment on `chromeDim` describes. Dim the contents; never the
                surface. */}
            <div className="mtk-shell-card">
              <EngineRail
                active={engine}
                compact={layout.collapsed}
                badges={{ scene: order.length || undefined, gameplay: playing ? "live" : undefined }}
                onChange={openEngine}
                style={chromeDim}
              />
            </div>
          </div>
        )}

        {/* LEFT — full panel, or a collapsed icon rail (the stage keeps priority on resize). */}
        {!layout.overlay && (layout.collapsed || leftDockCollapsed ? (
          <DockRail
            side="left"
            label="Scene and creation"
            items={LEFT_RAIL_ITEMS}
            activeId={leftWorkspace}
            popupOpen={dockFlyout === "left" || drawer === "left"}
            onActivate={(id, anchor) => activateDockRail("left", id, anchor)}
            data-testid="rail-left"
          />
        ) : (
          <div className="mtk-shell-track mtk-shell-track--left">
            <div className="mtk-shell-card">
              <div style={dockContent}>{leftPanel}</div>
            </div>
          </div>
        ))}

        {/* viewport: native-owned (invariant 4). Inside the `.exe` it is **transparent** so the native wgpu
            scene composites through (ADR-008); the per-frame orbit/zoom runs in the native render loop (the
            JS only fires drag_start/drag_end/zoom/pick — never per frame). Left-click → native pick → select;
            right-DRAG → orbit (suppress the menu); right-CLICK → the M3.3 context menu. */}
        {/* The stage's protected floor, published to CSS from the ONE place it is written down. The
            bottom dock's `max-height` is `calc(100% - var(--mtk-stage-min))` against this element, so
            the dock yields before the stage does on the vertical axis exactly as `dockGridColumns`
            makes the side docks yield on the horizontal one. Set here rather than in the stylesheet
            because `STAGE_MIN` is also what the shots harness imports to measure the floor: a number
            written down twice is a number that only moves in one place. */}
        {/* `--mtk-z-dock-sheet` is published here for the same reason `--mtk-stage-min` is: `z` in
            `tokens.ts` is the one ordering of this shell's layers, and a literal in the stylesheet
            would be a second one that drifts. The sheet is the bottom dock's drawer, so it takes the
            drawer's layer — BELOW `z.badge`, deliberately: a sheet that covers the stage in Play mode
            must not also cover the live PLAYING badge, which `<ux_quality>` 5 requires to be
            unmistakable on the stage itself. */}
        <div
          ref={stageColumn}
          className="mtk-stage-column"
          style={{ "--mtk-stage-min": `${STAGE_MIN}px`, "--mtk-z-dock-sheet": z.drawer } as React.CSSProperties}
        >
        <div
          ref={viewportRef}
          id="viewport"
          data-testid="viewport"
          role="region"
          tabIndex={-1}
          aria-label="3D viewport"
          onPointerDown={(e) => {
            // The stage surface, or a control floating over it? An INITIATING gesture that landed on
            // chrome did not land on the stage — see `stageInput.ts` for why this is the seam's
            // question and not each overlay's. Without it, a right-press on the "Import file…" button
            // starts a native ORBIT underneath it (measured: 3 of 3 empty-state buttons, in Chromium).
            if (!onStageSurface(e)) return;
            if (e.button === 2) {
              rightDrag.current = { x: e.clientX, y: e.clientY, moved: false };
              client.dragStart(); // native orbit begins; the render loop polls the cursor (0 IPC/frame)
              return;
            }
            if (e.button === 0) {
              if (aimActive) return; // aiming owns the click; do not grab a gizmo handle underneath it
              if (pipeActive) return; // drawing owns the click; do not start a gizmo drag underneath it
              if (drawActive) return; // the ground sketch owns the click for the same reason
              // M9 gizmo handle-grab: only when an entity is selected; if a handle is HIT the render loop
              // drags it natively (0 IPC/frame, like orbit) and the release commits. A miss falls through to
              // the normal pick. The hit flag resolves async, so a WebDriver synthetic click (which fires
              // immediately) still picks normally — the suppression is for real human-timed drags.
              gizmoHit.current = false;
              if (projectionStore.getState().selectedId) {
                const { x: nx, y: ny } = normalizeSurfacePoint(e.clientX, e.clientY);
                void client
                  .gizmoPickDrag(nx, ny, e.ctrlKey || e.metaKey)
                  .then((hit) => (gizmoHit.current = hit))
                  .catch(() => {});
              }
            }
          }}
          onClick={(e) => {
            if (!onStageSurface(e)) return;
            // A left-press that grabbed a gizmo handle is a DRAG, not a pick — don't re-select.
            if (gizmoHit.current && !pipeActive && !drawActive) {
              gizmoHit.current = false;
              return;
            }
            gizmoHit.current = false;
            // Left-click pick on the click event (fires reliably under WebDriver `element.click()`, unlike a
            // synthesized pointerdown). Pick at normalized FULL-SURFACE coordinates (the native camera rays
            // across the whole wgpu/WebView surface, while DOM docks only crop its visible area).
            const { x: nx, y: ny } = normalizeSurfacePoint(e.clientX, e.clientY);
            // AIMING TAKES THE CLICK, AND PEEKS RATHER THAN PICKS. `viewport_peek` names what is
            // under the cursor without touching the selection - which is the whole reason this can
            // be a click at all: the Cutscene panel is bound to the selection, so selecting the
            // object being named would switch which cutscene is on screen and lose the shot.
            if (aimActive) {
              void client
                .viewportPeek(nx, ny)
                .then((id) => {
                  if (!subjectAimStore.getState().active) return;
                  if (!id) {
                    setStatus("Nothing there — click the object this shot should film");
                    return;
                  }
                  const rungs = aimLadder.current.get(id);
                  subjectAimStore.getState().pick(id, rungs?.[0]?.name ?? id);
                })
                .catch((err) => console.error("viewport_peek failed", err));
              return;
            }
            if (drawActive) {
              // The point is the one the render thread already snapped and DREW — read, not re-derived
              // from these coordinates, so the corner the author saw is the corner they get.
              if (sketchBusy) return;
              void client
                .sketchPoint()
                .then((next) => {
                  setSketch(next);
                  setStatus(next.message);
                })
                .catch((err) => console.error("sketch_point failed", err));
              return;
            }
            if (pipeActive) {
              if (pipeBusy) return;
              void client
                .pipeForgePoint(nx, ny)
                .then((status) => {
                  setPipeStatus(status);
                  setStatus(status.message);
                })
                .catch((err) => console.error("pipe_forge_point failed", err));
              return;
            }
            void client
              .viewportPick(nx, ny)
              .then((picked) => {
                if (picked) {
                  projectionStore.getState().select(picked);
                  setStatus(`picked ${picked}`);
                } else {
                  setStatus("nothing here");
                }
              })
              .catch((err) => console.error("viewport_pick failed", err));
          }}
          // NO `onStageSurface` ON THESE TWO, DELIBERATELY. They COMPLETE a gesture rather than start
          // one: a right-drag that began on bare stage and is released with the cursor over the tool
          // rail must still reach `drag_end`, or the native orbit never stops. Gating them on the
          // target is precisely the bug the old per-overlay idiom shipped — `PipeForge` stopped
          // `pointerup` along with everything else, so an orbit released over its panel left the
          // camera spinning.
          onPointerMove={(e) => {
            const rd = rightDrag.current;
            if (rd && (Math.abs(e.clientX - rd.x) > 6 || Math.abs(e.clientY - rd.y) > 6)) rd.moved = true;
            // The one INITIATING read on this handler, so it takes the stage-surface gate the other
            // two lines deliberately do not: moving the cursor OFF the stage and onto the aim badge
            // must leave the ladder standing - the rungs are what the pointer is travelling to.
            if (aimActive && !rd && onStageSurface(e)) aimHover(e.clientX, e.clientY);
          }}
          onPointerUp={(e) => {
            if (e.button === 2 && rightDrag.current) client.dragEnd();
            if (e.button === 0 && gizmoHit.current) client.gizmoDragEnd(); // commit the gizmo move (one tx)
          }}
          onWheel={(e) => {
            // A wheel over a panel floating on the stage scrolls THAT PANEL. Before this line every
            // one of the 35 controls the shell paints inside the viewport turned a scroll into a
            // camera zoom — including the first-run card, which is itself `overflow-y: auto` and so
            // was being scrolled and zoomed at once.
            if (!onStageSurface(e)) return;
            client.zoom(e.deltaY * 0.04);
          }}
          onContextMenu={(e) => {
            // `preventDefault` is UNGATED on purpose, and it is the one asymmetry here. Suppressing
            // the browser's own menu is a decision about the stage REGION — the whole rectangle is
            // the engine's surface, and a native "Reload / Save image as…" menu over any part of it
            // is wrong. Which menu OPENS instead is a decision about the TARGET, and that is gated
            // below. The old idiom conflated the two and answered differently per overlay: the
            // viewport toolbar swallowed the event entirely (browser menu on right-click), the tool
            // rail too, and the empty-state card let the ENGINE's entity menu open over itself.
            e.preventDefault();
            if (!onStageSurface(e)) {
              rightDrag.current = null;
              return;
            }
            // No context actions while Playing — editing is gated off in Play (and it would let a user open
            // Focus mid-Play, where Esc would then clear focus instead of stopping Play, contradicting the
            // on-stage badge's "Esc to stop"). The badge's promise stays honest.
            if (playing || pipeActive) {
              rightDrag.current = null;
              return;
            }
            // a right-DRAG was an orbit, not a menu request — suppress the menu (movement threshold)
            const orbited = rightDrag.current?.moved;
            rightDrag.current = null;
            if (orbited) return;
            const sel = projectionStore.getState().selectedId;
            if (sel) setCtx({ id: sel, x: e.clientX, y: e.clientY });
          }}
          style={{
            position: "relative",
            flex: "1 1 auto",
            minHeight: 0,
            overflow: "hidden",
            background: native ? "transparent" : "var(--mtk-bg-inset)", // transparent → wgpu composites through (.exe)
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: color.text.muted,
            font: font.mono,
            fontSize: fontSize.body,
            outline: playing ? `3px solid ${paused ? "var(--mtk-warn-border)" : "var(--mtk-success-border)"}` : "none",
            outlineOffset: -3,
            boxShadow: playing ? `inset 0 0 60px ${paused ? color.warn.bg : color.success.bg}` : "none",
            transition: `outline-color ${motion.base}, box-shadow ${motion.base}`,
          }}
        >
          {!native && "native wgpu viewport — drag to orbit · scroll to zoom · click to select (live in the .exe)"}
          {/* A COVERED CONTROL IS WORSE THAN AN ABSENT ONE. The sheet floats over the stage, and the
              stage's own controls are absolutely positioned inside the viewport underneath it — so
              without this they are still in the DOM, still focusable, still counted by every gate,
              and completely unreachable. R3 caught it in the run meant to prove the sheet worked:
              `vpSelect`/`vpMove` sharing pixels with the dock's `Model` tab. Measured across the
              whole regime, it is not an edge: at 520 three of five transform tools are entirely
              covered, at 480 four of five, and at 400 **all five plus the viewport toolbar** — the
              rail wants y 104..341 and even at the top of the regime there are only 228px of stage
              above the sheet. There is no height at which both fit, which is what makes withdrawing
              them a rule rather than a tuning.
              The shell already states this contract one mode over: `!playing` removes exactly these
              two, because in Play the stage is not the surface you are authoring on. A sheet the user
              opened over the stage is the same statement, and it is reversible by the same click that
              made it — the tools come back when the sheet closes. */}
          {!playing && !stageSheet && (
            <ViewportToolRail
              data-testid="viewport-tool-rail"
              activeTool={activeTool}
              onToolChange={chooseTool}
              minimized={toolRailMinimized}
              onMinimizedChange={setToolRailMinimized}
              availability={{
                move: { disabled: !selectedId || pipeActive, reason: pipeActive ? "Finish or cancel the pipe route first" : "Select an object first" },
                rotate: { disabled: !selectedId || pipeActive, reason: pipeActive ? "Finish or cancel the pipe route first" : "Select an object first" },
                scale: { disabled: !selectedId || pipeActive, reason: pipeActive ? "Finish or cancel the pipe route first" : "Select an object first" },
                pipe: { disabled: pipeBusy, reason: "The current asset is baking" },
              }}
            />
          )}
          {!playing && !stageSheet && <ViewportToolbar client={client} showTransformTools={false} />}
          {drawActive && (
            <Suspense
              fallback={
                <div role="status" aria-live="polite" style={{ position: "absolute", top: 76, left: toolRailMinimized ? 54 : 138, zIndex: z.chrome, width: 220, padding: space.md, borderRadius: radius.lg, color: color.text.secondary, background: color.bg.raised, border: `1px solid ${color.border.default}`, boxShadow: elevation.e2 }}>
                  Loading the drawing tools…
                </div>
              }
            >
              <GroundSketch
                client={client}
                state={sketch}
                onState={setSketch}
                onPendingChange={setSketchBusy}
                // NEVER WIDER THAN THE STAGE. The panel floats over the surface it is drawing on, so
                // its width is capped by what is there — `<ux_quality>` 5, and the reason the minimized
                // form already said this: a fixed 288 on a 508px stage is a tool covering its own work.
                style={{ left: toolRailMinimized ? 54 : 138, width: toolRailMinimized ? "min(288px, calc(100% - 62px))" : "min(288px, calc(100% - 146px))" }}
              />
            </Suspense>
          )}
          {!playing && !stageSheet && activeTool === "pipe" && (
            <Suspense
              fallback={
                <div role="status" aria-live="polite" style={{ position: "absolute", top: 76, left: toolRailMinimized ? 54 : 138, zIndex: z.chrome, width: 220, padding: space.md, borderRadius: radius.lg, color: color.text.secondary, background: color.bg.raised, border: `1px solid ${color.border.default}`, boxShadow: elevation.e2 }}>
                  Loading Pipe Forge…
                </div>
              }
            >
              <PipeForge
                client={client}
                status={pipeStatus}
                editableEntityId={editablePipeId}
                onPendingChange={setPipeBusy}
                style={{ left: toolRailMinimized ? 54 : 138, width: toolRailMinimized ? "min(336px, calc(100% - 62px))" : 336 }}
                onStatus={(status) => {
                  setPipeStatus(status);
                  setStatus(status.message);
                }}
                onBaked={(report) => {
                  setStatus(report.message);
                  if (report.entityId) {
                    projectionStore.getState().select(report.entityId);
                    void client.gizmoSelect(report.entityId);
                    setInspectorWorkspace("properties");
                    openEngine("model");
                  }
                  void client.pipeForgeStatus().then(setPipeStatus);
                }}
              />
            </Suspense>
          )}
          {playing && <PlayBadge paused={paused} onStop={stopPlay} />}
          {/* Bottom-centre, and it coexists with the preview badge on purpose: re-aiming a shot
              WHILE previewing it is the loop this closes - point at something else, and the frame
              on the stage is already the new one by the time the badge has gone. */}
          {aimActive && (
            <SubjectAimBadge
              shotIndex={aim.shotIndex}
              shots={aim.shots}
              rungs={aim.rungs}
              looking={aim.looking}
              onPick={(rung) => subjectAimStore.getState().pick(rung.id, rung.name)}
              // LEAVING A RUNG HANDS THE STAGE BACK, it does not go dark: the cursor is still over
              // the leaf the badge's first rung names, and a cue that blanked between two rungs
              // would flicker exactly while the author was comparing them. The store deliberately
              // keeps no stack — what the stage is over is `aim.rungs[0]`, and it is the caller
              // that knows that, not a store remembering a peek that may have gone stale.
              onPreview={(rung) => {
                const back = aim.rungs[0];
                stageHighlightStore
                  .getState()
                  .show(rung ? "rung" : "stage", rung ? [rung.id] : back ? [back.id] : []);
              }}
              onCancel={() => {
                subjectAimStore.getState().cancel();
                setStatus("Aiming cancelled — the shot still frames what it did");
              }}
            />
          )}
          {/* Never both: Play takes the camera itself, and the shell hands a held preview back
              before it does, so a badge for each would be two claims about one viewport. */}
          {!playing && cinemaPreview.active && (
            <PreviewBadge
              shotIndex={cinemaPreview.shotIndex}
              shots={cinemaPreview.shots}
              subjectName={cinemaPreview.subjectName}
              blending={cinemaPreview.blending}
              onExit={() => {
                const target = cinemaPreview.entity;
                cinemaPreviewStore.getState().reset();
                if (target) void client.cinemaPreview(target, 0, false);
              }}
            />
          )}
          {/* NOT deferred, and the comment that said it could be was WRONG about its own code. This
              component owns the `subscribeNativeImportLifecycle` effect — the OS-drop listener IS this
              mount, not something beside it — so behind a `lazy` with a `null` fallback the shell is
              deaf to a drop until the chunk resolves, and a file dropped in that window lands on a
              stage that never answers. There is no second listener to catch it and no error to read:
              the user's drop is simply gone. `<ux_quality>` 6 — no inert surfaces. */}
          <ImportDropOverlay
            onEntityImported={(rootId) => {
              selectCreated(rootId);
              openEngine("model");
            }}
            onOpenImportReport={() => openBottom("import")}
            onOpenFormats={() => openBottom("formats")}
            onImportAnother={importAsset}
          />
          {/* The first-run card is ON THE STAGE, inside it, not floating over the window. It used to
              be `position: fixed; left: 50%` — centred on the WINDOW — and a 520 px card centred on a
              1000 px window starts at x=240, which is 192 px inside a left dock that ends at 432. It
              sat on top of the Build workspace's Combine section and the Terrain presets and took
              their clicks (`<ux_quality>` 4: no control overlaps another). Centred on the STAGE it
              cannot reach a dock at any width, because the grid — one source of truth — decides where
              the stage is and the card simply lives there, exactly as `PlayBadge` does.
              `!stageSheet` for the same reason the tool rail has it, and it was found the same way
              the sheet's first defect was — by LOOKING at the capture. Every assertion passed and the
              card was painted straight across the Model workspace's description, because it is
              `z.menu` (130) against the sheet's `z.drawer` (120). R3 was silent and right to be: it
              compares CONTROLS, and what the card was covering here is prose. A first-run card
              inviting you to start on a stage you have just covered up is the wrong invitation
              anyway — it comes back with the stage.
              NOT deferred either: this is the first thing a first-run user is shown, and a `lazy` with
              a `null` fallback means the invitation ARRIVES LATE — a blank stage, then a card popping
              in — on exactly the machines product principle 3 targets, where the extra request costs
              most. A surface whose entire job is to be there when you first look cannot be the surface
              that loads last. */}
          {/* AND IT YIELDS TO AN AIM, for the same reason it yields to Play and to the dock sheet: a
              mode that owns the stage owns what is painted on it. The first-run card is anchored
              `bottom: space.lg, left: 50%` — the SAME anchor the aim badge takes — so with both up
              the badge sits on the card's headline, which is `<ux_quality>` 4's overlap exactly.
              Caught on the `.exe` capture, where a fresh project had brought the card back. */}
          {/* ONE ANCHOR AT THE BOTTOM OF THE STAGE, and everything that wants to be there is a child of
              it in reading order. Three surfaces had each written `position: absolute; left: 50%;
              bottom: …` for themselves, which is not a layout — it is three elements agreeing to occupy
              the same pixels, and this repository has twice paid for the disagreement (the aim badge on
              the first-run card's headline; the card itself centred on the WINDOW and 192px inside the
              left dock). A stack cannot collide with itself. */}
          {!playing && !stageSheet && (
            <div className="mtk-stage-footer" data-testid="stage-footer">
              {/* AND IT YIELDS TO DRAWING FOR THE SAME REASON IT YIELDS TO AN AIM. A card advising an
                  author how to fill an empty scene, painted over the outline they are filling it with,
                  is advice covering its own outcome — measured on the live `.exe`, where it sat across
                  the middle of a 35 x 56 m footprint being drawn. */}
              <Onboarding show={!sceneEmpty && !aimActive && !drawActive} onStart={() => openEngine("build")} />
              {/* NORTH STAR #2, ON THE STAGE. It used to be three clicks and a scroll inside a
                  collapsed disclosure headed "Optional assisted creation" — see `DescribeBar`'s own
                  header for why that was a defect and not a placement. It yields to an aim for the same
                  reason the first-run card does: a mode that owns the stage owns what is painted on it,
                  and the aim badge takes this anchor. */}
              {!aimActive && (
                <DescribeBar
                  client={client}
                  form="floating"
                  onImport={importAsset}
                  onBrowseAssets={() => openEngine("build")}
                  // "Draw it in the viewport" now means the general gesture it has always named. It
                  // armed PIPE FORGE, because a pipe router was the only thing that could draw in the
                  // viewport when the label was written — a promise the product has since kept.
                  onDrawShape={() => chooseTool("draw")}
                />
              )}
            </div>
          )}
          {sceneEmpty && !playing && !stageSheet && !drawActive && (
            <EmptyState
              onDescribe={() => document.getElementById("describe")?.focus()}
              onDrawPipe={() => setActiveTool("pipe")}
              onBrowseAssets={() => openEngine("build")}
              onImport={importAsset}
            />
          )}
          {/* Keep transient feedback below the authoring toolbar; passive toasts must not cover tools. */}
          <ToastHost top={playing ? 52 : 58} />
        </div>

        <BottomDock
          client={client}
          active={bottomWorkspace}
          open={bottomOpen}
          form={dockShape}
          playing={playing}
          animate={animateWorkspace}
          onAnimateChange={setAnimateWorkspace}
          onChange={(w) => {
            setBottomWorkspace(w);
            // Keep the rail in step. Switching workspace from the dock's own strip and leaving the rail
            // pointing somewhere else would put the editor back in the state this layout removed: two
            // controls, two answers to "where am I?".
            // Mapped through `engineById`, never cast. Three of the dock's workspaces have no engine —
            // Problems and Runtime are diagnostics, and Formats is a build-capability report, not a
            // place you author — and the cast that used to stand here sent `"formats"` into `setEngine`
            // as an `EngineId` it is not. The rail then lit NOTHING, which is precisely the "two
            // controls, two answers to where am I" state this layout exists to remove.
            const asEngine =
              w === "asset" ? "model" : w === "animation" ? "animate" : w;
            if (ENGINES.some((e) => e.id === asEngine)) setEngine(asEngine as EngineId);
          }}
          onOpenChange={setBottomOpen}
        />
        </div>

        {/* RIGHT — full panel, or a collapsed icon rail. */}
        {!layout.overlay && (layout.collapsed || rightDockCollapsed ? (
          <DockRail
            side="right"
            label="Inspector"
            items={RIGHT_RAIL_ITEMS}
            activeId={inspectorWorkspace}
            popupOpen={dockFlyout === "right" || drawer === "right"}
            onActivate={(id, anchor) => activateDockRail("right", id, anchor)}
            data-testid="rail-right"
          />
        ) : (
          <div className="mtk-shell-track mtk-shell-track--right">
            <div className="mtk-shell-card">
              <div style={dockContent}>{rightPanel}</div>
            </div>
          </div>
        ))}
      </div>

      {/* User-collapsed desktop docks reopen as light, anchored working panels. They preserve the stage and
          can be pinned back into the grid from their shared panel header. */}
      {dockFlyout && !layout.collapsed && !layout.overlay && (
        <Popover
          open
          anchor={dockFlyoutAnchor}
          returnFocus={dockFlyoutAnchor}
          placement={dockFlyout === "left" ? "bottom-start" : "bottom-end"}
          role="dialog"
          ariaLabel={dockFlyout === "left" ? "Scene and creation quick panel" : "Inspector quick panel"}
          zIndex={z.drawer}
          onClose={() => setDockFlyout(null)}
        >
          <div className="mtk-dock-flyout" data-testid={`dock-flyout-${dockFlyout}`}>
            {dockFlyout === "left" ? leftPanel : rightPanel}
          </div>
        </Popover>
      )}

      {/* Collapsed-panel overlay drawer (opened from a rail). */}
      {drawer && (
        <Modal open onClose={() => setDrawer(null)} zIndex={z.drawer} ariaLabel={drawer === "left" ? "Scene and asset workspace" : "Inspector workspace"}>
          <aside
            data-testid={`drawer-${drawer}`}
            // The ≤620px form of the same dock, so it says the same thing about itself: a panel
            // surface with a soft lift, rounded on the edge that faces the workspace and square on
            // the one flush with the window. It used to be an inset well behind a 1px hairline —
            // the vocabulary the docked form has now dropped, and the only place it survived.
            style={{
              position: "absolute",
              top: 0,
              bottom: 0,
              ...(drawer === "left"
                ? { left: 0, borderRadius: "0 var(--mtk-shell-radius) var(--mtk-shell-radius) 0" }
                : { right: 0, borderRadius: "var(--mtk-shell-radius) 0 0 var(--mtk-shell-radius)" }),
              width: "min(340px, calc(100vw - 36px))",
              background: "var(--mtk-bg-panel)",
              display: "flex",
              flexDirection: "column",
              overflow: "hidden",
              boxShadow: elevation.e3,
            }}
          >
            <div style={{ display: "flex", justifyContent: "flex-end", padding: space.xs, borderBottom: `1px solid ${color.border.subtle}`, background: color.bg.raised }}>
              <Button variant="ghost" compact icon aria-label={`Close ${drawer === "left" ? "Scene" : "Inspector"} workspace`} onClick={() => setDrawer(null)}>×</Button>
            </div>
            {/* Same as `dockContent`: the drawer is the ≤620px form of the same panel, and a block
                box would let it lay out at content height and be cut by this `overflow: hidden`. */}
            <div style={{ display: "flex", flexDirection: "column", flex: 1, minHeight: 0, overflow: "hidden", ...chromeDim }}>{drawer === "left" ? leftPanel : rightPanel}</div>
          </aside>
        </Modal>
      )}

      {ctx && (
        <Suspense fallback={null}>
          {/* Portaled + edge-aware (Popover): the right-click menu can no longer be clipped by a panel's
              `overflow` or open off-screen near a viewport edge (it clamps/flips into view). */}
          <Popover open anchorPoint={{ x: ctx.x, y: ctx.y }} onClose={() => setCtx(null)}>
            <ContextMenu
              client={client}
              id={ctx.id}
              onClose={() => setCtx(null)}
              onFocus={(id, dist) => setFocused({ id, dist })}
            />
          </Popover>
        </Suspense>
      )}
      {focused && (
        <FocusBanner
          id={focused.id}
          dist={focused.dist}
          onClear={() => {
            client.unfocus();
            setFocused(null);
            setStatus("focus cleared");
          }}
        />
      )}
      {commandsOpen && (
        <Suspense
          fallback={(
            <Modal open onClose={() => setCommandsOpen(false)} ariaLabel="Command palette loading">
              <div className="mtk-workspace-state" role="status" aria-live="polite">
                <span className="mtk-spinner" aria-hidden="true" />
                <span>Loading command palette…</span>
              </div>
            </Modal>
          )}
        >
          <CommandPalette
            open
            onClose={() => setCommandsOpen(false)}
            commands={commands}
            onCommandError={(error, command) => {
              console.error(`command ${command.id} failed`, error);
              setStatus(`${command.label} could not be completed`);
            }}
          />
        </Suspense>
      )}
      {importOpen && (
        <Suspense
          fallback={(
            <Modal open onClose={() => setImportOpen(false)} ariaLabel="Import dialog loading">
              <div className="mtk-workspace-state" role="status" aria-live="polite">
                <span className="mtk-spinner" aria-hidden="true" />
                <span>Loading import…</span>
              </div>
            </Modal>
          )}
        >
          <ImportDialog open client={client} onClose={() => setImportOpen(false)} />
        </Suspense>
      )}
      {exportOpen && (
        <Suspense
          fallback={(
            <Modal open onClose={() => setExportOpen(false)} ariaLabel="Export dialog loading">
              <div className="mtk-workspace-state" role="status" aria-live="polite">
                <span className="mtk-spinner" aria-hidden="true" />
                <span>Loading export…</span>
              </div>
            </Modal>
          )}
        >
          <ExportDialog open client={client} onClose={() => setExportOpen(false)} />
        </Suspense>
      )}
      <StatusBar />
      <Rejections />
    </div>
  );
}
