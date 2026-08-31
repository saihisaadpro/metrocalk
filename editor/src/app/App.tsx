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
import { projectionStore, useDisplayedEntity, useEntityOrder, useMultiSelect, useSelectedId } from "../store/projection";
import { thumbnailStore, startThumbnailPump } from "../store/thumbnails";
import { playStore, usePlaying, usePaused } from "../store/play";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
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
import { ImportDropOverlay } from "./ImportDropOverlay";
import { FocusBanner } from "../panels/FocusBanner";
import type { EditorCommand } from "../panels/CommandPalette";
import { ViewportToolRail, type ViewportTool } from "../panels/ViewportToolRail";
import type { PipeForgeStatus } from "../transport/protocol";
import { EditorHeader } from "./EditorHeader";
import { LeftDock, InspectorDock, type LeftWorkspace, type InspectorWorkspace } from "./EditorDocks";
import { EngineRail, ENGINES, engineById, type EngineId } from "./EngineRail";
import { BottomDock, type BottomWorkspace } from "./BottomDock";
import { onStageSurface } from "./stageInput";
import { normalizeSurfacePoint } from "./viewportCoordinates";
import { isMarqueeDrag, marqueeBox, marqueeMode, marqueeResult } from "./marquee";
import { StageMarquee } from "./StageMarquee";
import { selectionSentence, entityLabel } from "../store/selectionText";
import { deleteSelection } from "./deleteSelection";
import { selectAllWith, selectionCommands } from "./selectionCommands";
import { stateSelection } from "./stateSelection";
import { requestObjectSearch } from "../store/find";

// WHAT MAY BE DEFERRED, AND WHY THE LIST IS SHORT. A chunk that loads on demand is absent until the
// gesture that needs it — so the only safe candidates are surfaces a user REACHES FOR: Pipe Forge
// (its viewport tool), the command palette (Ctrl/Cmd+K), the entity menu (right-click). Each carries
// a named loading state, because a gesture that produces nothing is a gesture the user repeats.
//
// `Onboarding` and `ImportDropOverlay` were on this list and are deliberately NOT any more — see
// `scripts/first-paint.json` and ADR-130. The bundle budget only ever said "smaller", and a rule that
// only says smaller, followed exactly, deletes the product; the counter-rule is declared, not inferred.
const PipeForge = lazy(() => import("../panels/PipeForge").then((module) => ({ default: module.PipeForge })));
const CommandPalette = lazy(() => import("../panels/CommandPalette").then((module) => ({ default: module.CommandPalette })));
const ContextMenu = lazy(() => import("../panels/ContextMenu").then((module) => ({ default: module.ContextMenu })));

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
      // A DOM overlay ON the stage must not also DRIVE the stage. The viewport's own handlers pick,
      // orbit and start gizmo drags on pointerdown/click, and this badge is inside it — so without
      // this, pressing ⏹ Stop also fires a pick at the badge's coordinates. Found while moving the
      // onboarding card onto the stage; the exposure is the same one and is fixed in both places.
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
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

function effectiveViewportWidth(): number {
  if (typeof window === "undefined" || typeof document === "undefined") return 1440;
  const cssZoom = Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("zoom"));
  return window.innerWidth / (Number.isFinite(cssZoom) && cssZoom > 0 ? cssZoom : 1);
}

export function App() {
  const client = useEditorSession();
  usePlayerDrive(client); // arrows / WASD move the Player role while Play runs
  const native = isTauri(); // inside the packaged .exe the viewport is the real wgpu region (composite)
  // The M3.3 right-click context menu, opened for an entity at a cursor position.
  // The right-click menu acts on the SELECTION (ADR-183), so what is stored here is the set, not the
  // one id under the cursor. The primary is the last element, matching the projection store.
  const [ctx, setCtx] = useState<{ ids: string[]; x: number; y: number } | null>(null);
  // M3.3 focus mode — the framed entity + its camera distance (read from `focus_debug`); drives the banner.
  const [focused, setFocused] = useState<{ id: string; dist: number } | null>(null);
  // Tracks a right-press for the orbit-vs-context-menu movement threshold (the scaffold's disambiguation).
  const rightDrag = useRef<{ x: number; y: number; moved: boolean } | null>(null);
  // M9 gizmo handle-drag: set by a left-press that HIT a gizmo handle (so the click doesn't re-pick + the
  // release commits). A ref (not state) so the click/up guards read it synchronously off the hot path.
  const gizmoHit = useRef(false);
  // Box selection. The PRESS is a ref (read synchronously in the move handler, off the hot path) and only
  // the drawn rectangle is state — so a press that turns out to be a click costs no render at all, and a
  // real marquee re-renders one absolutely-positioned div and nothing else.
  const marqueePress = useRef<{ x: number; y: number } | null>(null);
  const [marqueeDrag, setMarqueeDrag] = useState<{
    start: { x: number; y: number };
    current: { x: number; y: number };
    /** The stage's top-left in client coordinates, read ONCE when the box starts. The rectangle is an
     *  absolutely-positioned child of the stage, so it needs stage-relative pixels; measuring the
     *  element on every move would put a layout read on a pointer handler for a number that cannot
     *  change while a pointer is captured. */
    origin: { left: number; top: number };
  } | null>(null);
  // A completed marquee must not also arrive as a pick: `click` fires after `pointerup`, and without
  // this the release of a box that selected fourteen objects would immediately re-select the one under
  // the cursor. Consumed by the click it suppresses — and CLEARED AT THE NEXT PRESS, which is what
  // bounds it to one gesture. Relying on the click alone was wrong and the live `.exe` run proved it:
  // a drag whose release does not produce a click on the stage (the pointer left the window, or a
  // synthetic sequence) left the flag standing, and the user's NEXT click on the stage was silently
  // eaten. One lost click is indistinguishable from a dead viewport.
  const marqueeConsumedClick = useRef(false);
  const [pipeStatus, setPipeStatus] = useState<PipeForgeStatus | null>(null);
  const pipeActive = pipeStatus?.active === true;
  const [pipeBusy, setPipeBusy] = useState(false);
  const [activeTool, setActiveTool] = useState<ViewportTool>("select");
  const [leftWorkspace, setLeftWorkspace] = useState<LeftWorkspace>("scene");
  // Which sub-engine the author is in. One piece of state for the whole rail, so "where am I?" always has
  // exactly one answer — the thing the old three-dock arrangement could not provide.
  const [engine, setEngine] = useState<EngineId>("scene");
  const [inspectorWorkspace, setInspectorWorkspace] = useState<InspectorWorkspace>("properties");
  const [bottomWorkspace, setBottomWorkspace] = useState<BottomWorkspace>("asset");
  const [bottomOpen, setBottomOpen] = useState(false);
  const [commandsOpen, setCommandsOpen] = useState(false);
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
  const multiSelect = useMultiSelect();
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

  /**
   * Ctrl/Cmd-F — the find chord, landing in the one box that searches the scene.
   *
   * The Scene workspace is opened FIRST because the outliner is a `hidden` tabpanel everywhere else,
   * and focus into a `display:none` subtree is focus nobody can see. The request travels as store
   * state rather than a ref, so the shell owns the keyboard and the panel owns its field — neither
   * reaches into the other.
   */
  function findObjects() {
    openEngine("scene");
    requestObjectSearch();
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
    setActiveTool(tool);
    if (tool === "move" || tool === "rotate" || tool === "scale") {
      client.gizmoMode(tool === "move" ? "translate" : tool);
    }
  }

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
      // **UNDO AND REDO ARE CHORDS, AND A BUTTON DOES NOT OWN A CHORD.** The guard above exists for the
      // BARE-LETTER shortcuts — W/E/R/F must not switch viewport tools while a control has the
      // keyboard — and applying it to Ctrl-Z as well made the editor refuse the one promise it prints
      // out loud. Found live in the packaged `.exe`: delete a selection from the Actions menu, focus
      // returns to the trigger BUTTON, the toast and the status line both say "recoverable with
      // Ctrl-Z", and Ctrl-Z does nothing at all until you click somewhere else first. A control that
      // states a way out and then refuses it is worse than one that never offered.
      //
      // What genuinely owns Ctrl-Z is a TEXT-EDITING context, because that is where the browser's own
      // undo stack lives and stealing it would lose typing. Nothing else.
      const editingText = !!el && !!el.closest("input, textarea, [contenteditable='true']");
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setCommandsOpen(true);
        return;
      }
      // Ctrl/Cmd-F — find objects. The one chord every person already knows for "where is it", and it
      // was unbound: the scene search box was reachable only by opening the Scene workspace and
      // clicking into it. Guarded on `editingText` because a text field owns the browser's own find.
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "f") {
        if (editingText) return;
        e.preventDefault();
        findObjects();
        return;
      }
      // Ctrl/Cmd-A — select every object. Guarded on `editingText` for the same reason Ctrl-Z is: a
      // text field owns select-all-text, and nothing else owns this chord.
      if ((e.ctrlKey || e.metaKey) && !e.shiftKey && e.key.toLowerCase() === "a") {
        if (editingText) return;
        e.preventDefault();
        selectAll();
        return;
      }
      // **DELETE / BACKSPACE — the most-pressed key in any editor, and it was not bound at all**
      // (ADR-183). The only two routes to deleting anything were a row inside a popup menu in the left
      // dock and a right-click row that destroyed one object; a person who selected 378 bolts and
      // pressed Delete got silence. Guarded on `editingText` rather than the wider `editing`, for the
      // same reason Ctrl-Z is: a text field owns Delete over its own characters, and nothing else does
      // — including a focused BUTTON, which is exactly where focus lands after the Actions menu runs.
      if (e.key === "Delete" || e.key === "Backspace") {
        if (editingText) return;
        if (playing || pipeActive) return;
        const ids = projectionStore.getState().multiSelect;
        if (!ids.length) return;
        e.preventDefault();
        void deleteSelection(client, ids).then((outcome) => {
          setStatus(outcome.sentence);
          pushToast(outcome.ok ? outcome.sentence : "couldn't delete the selection", outcome.ok ? "info" : "error");
        });
        return;
      }
      const redoGesture =
        (e.ctrlKey || e.metaKey) &&
        (e.key.toLowerCase() === "y" || (e.shiftKey && e.key.toLowerCase() === "z"));
      if (redoGesture) {
        if (editingText) return;
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
        if (editingText) return;
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
        if (pipeActive && !pipeBusy) {
          void client.pipeForgeCancel().then((status) => {
            setPipeStatus(status);
            setStatus(status.message);
          });
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
  }, [client, ctx, drawer, playing, focused, pipeActive, pipeBusy]);

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

  // SELECTING WITHOUT A GESTURE (ADR-176). ADR-158 gave the stage the four gestures the engine
  // understood; these are the verbs no gesture can express. Both halves of the selection, always —
  // the store the Inspector reads AND the engine model the picture is outlined from — through the
  // same `selectEntities` seam the outliner uses.
  const applySelection = (ids: string[], sentence: string) => stateSelection(client, ids, sentence);
  const selectAll = selectAllWith({ apply: applySelection });

  const importAsset = () => {
    void client.importAssetDialog().then((id) => {
      selectCreated(id);
      if (id) openEngine("model");
    });
  };

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
    { id: "tool-pipe", label: "Draw pipe", category: "Create", description: "Author and bake a routed PBR asset in the viewport", keywords: ["pipe forge", "procedural"], disabled: playing, disabledReason: "Stop Play before authoring", execute: () => chooseTool("pipe") },
    { id: "create-entity", label: "Create empty entity", category: "Create", description: "Add a named object at the origin", execute: async () => selectCreated(await client.createEntity(0, 1, 0, "Entity")) },
    { id: "create-light", label: "Add point light", category: "Create", description: "Add a warm point light above the origin", execute: async () => selectCreated(await client.addLight("point", 0, 4, 0, 1, 0.96, 0.9, 60)) },
    { id: "import-asset", label: "Import asset…", category: "Create", description: "Choose a supported 3D, image, or CAD file", execute: async () => selectCreated(await client.importAssetDialog()) },
    // From the one exported list, so the rows a `shots` capture photographs are the rows that ship.
    ...selectionCommands({
      apply: applySelection,
      say: setStatus,
      find: findObjects,
      hasSelection: multiSelect.length > 0,
      sceneEmpty,
    }),
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
      onImport={importAsset}
      onContextMenu={(ids, x, y) => {
        if (!playing) setCtx({ ids, x, y });
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
              // A new press begins a new gesture, so nothing the LAST one left behind may still be
              // suppressing this one's click.
              marqueeConsumedClick.current = false;
              if (pipeActive) return; // drawing owns the click; do not start a gizmo drag underneath it
              // M9 gizmo handle-grab: only when an entity is selected; if a handle is HIT the render loop
              // drags it natively (0 IPC/frame, like orbit) and the release commits. A miss falls through to
              // the normal pick. The hit flag resolves async, so a WebDriver synthetic click (which fires
              // immediately) still picks normally — the suppression is for real human-timed drags.
              gizmoHit.current = false;
              // A left-press on bare stage is the start of a box selection until it turns out to be a
              // click. Recorded even when a gizmo probe is in flight: if the probe comes back HIT, the
              // press is withdrawn below, because dragging a handle and dragging a box are the same
              // gesture until the engine answers which one it was.
              if (!playing) marqueePress.current = { x: e.clientX, y: e.clientY };
              if (projectionStore.getState().selectedId) {
                const { x: nx, y: ny } = normalizeSurfacePoint(e.clientX, e.clientY);
                void client
                  .gizmoPickDrag(nx, ny, e.ctrlKey || e.metaKey)
                  .then((hit) => {
                    gizmoHit.current = hit;
                    if (hit) {
                      marqueePress.current = null;
                      setMarqueeDrag(null);
                    }
                  })
                  .catch(() => {});
              }
            }
          }}
          onClick={(e) => {
            if (!onStageSurface(e)) return;
            // A drag that drew a box is not a pick either. `click` fires after `pointerup`, so without
            // this the release would re-select whatever is under the cursor and throw the box away.
            if (marqueeConsumedClick.current) {
              marqueeConsumedClick.current = false;
              return;
            }
            // A left-press that grabbed a gizmo handle is a DRAG, not a pick — don't re-select.
            if (gizmoHit.current && !pipeActive) {
              gizmoHit.current = false;
              return;
            }
            gizmoHit.current = false;
            // Left-click pick on the click event (fires reliably under WebDriver `element.click()`, unlike a
            // synthesized pointerdown). Pick at normalized FULL-SURFACE coordinates (the native camera rays
            // across the whole wgpu/WebView surface, while DOM docks only crop its visible area).
            const { x: nx, y: ny } = normalizeSurfacePoint(e.clientX, e.clientY);
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
            // What the keyboard was doing is part of the gesture, and the engine has understood these
            // three since picking was rebuilt — the front end simply never sent them. Shift extends,
            // Ctrl/Cmd toggles, Alt takes the NEXT object under the cursor so the part behind the part
            // you can see is reachable without hiding anything.
            const mods = { extend: e.shiftKey, toggle: e.ctrlKey || e.metaKey, cycle: e.altKey };
            const modified = mods.extend || mods.toggle;
            void client
              .viewportPick(nx, ny, mods)
              .then(async (picked) => {
                if (modified) {
                  // A toggle that DESELECTED still hit something, so the hit cannot say what is
                  // selected now. Read the set back rather than predicting it — the one extra round
                  // trip happens on a modified click and never on a plain one.
                  const ids = await client.selectionIds().catch(() => null);
                  if (ids) {
                    projectionStore.getState().setSelection(ids);
                    setStatus(selectionSentence(ids.length));
                    return;
                  }
                }
                if (picked) {
                  projectionStore.getState().select(picked);
                  setStatus(entityLabel(picked));
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
            const press = marqueePress.current;
            if (!press || gizmoHit.current || pipeActive) return;
            const current = { x: e.clientX, y: e.clientY };
            if (!marqueeDrag && !isMarqueeDrag(press, current)) return;
            // Capture on the FIRST move that qualifies, not on the press: a plain click must not
            // redirect events away from the overlays it might have landed on, and a real box drag must
            // keep receiving moves after the cursor leaves the window — otherwise letting go outside
            // the stage strands a rectangle on screen with no release to clear it.
            // `try` because pointer capture is not universal: jsdom has no implementation at all, and a
            // synthetic event carries no live pointer for a real browser to capture. Neither is a reason
            // to abandon the box — capture is an improvement to a drag that already works without it.
            if (!marqueeDrag) {
              try {
                e.currentTarget.setPointerCapture(e.pointerId);
              } catch {
                /* the drag still tracks through the element's own move events */
              }
            }
            const origin = marqueeDrag?.origin ?? (({ left, top }) => ({ left, top }))(e.currentTarget.getBoundingClientRect());
            setMarqueeDrag({ start: press, current, origin });
          }}
          onPointerUp={(e) => {
            if (e.button === 2 && rightDrag.current) client.dragEnd();
            if (e.button === 0 && gizmoHit.current) client.gizmoDragEnd(); // commit the gizmo move (one tx)
            const drag = marqueeDrag;
            marqueePress.current = null;
            if (!drag || e.button !== 0) {
              setMarqueeDrag(null);
              return;
            }
            setMarqueeDrag(null);
            marqueeConsumedClick.current = true;
            try {
              if (e.currentTarget.hasPointerCapture?.(e.pointerId)) e.currentTarget.releasePointerCapture(e.pointerId);
            } catch {
              /* nothing was captured */
            }
            // The corners go over IN THE ORDER THEY WERE DRAGGED. The engine reads the policy from the
            // direction (`ScreenRect::from_drag`), so normalizing them here would silently make every
            // marquee an enclose — and the caption on screen would be lying about what just happened.
            const mode = marqueeMode(drag.start.x, drag.current.x);
            const extend = e.shiftKey || e.ctrlKey || e.metaKey;
            const from = normalizeSurfacePoint(drag.start.x, drag.start.y);
            const to = normalizeSurfacePoint(drag.current.x, drag.current.y);
            void client
              .viewportPickRegion(from.x, from.y, to.x, to.y, extend)
              .then((ids) => {
                projectionStore.getState().setSelection(ids);
                setStatus(selectionSentence(ids.length, ids));
                // At the gesture, not only in the gutter (`<ux_quality>` 2): the box is gone by the
                // time the answer arrives, and a count that only appears in the status bar is a count
                // nobody looking at the stage will read.
                pushToast(marqueeResult(ids.length, mode, extend), ids.length ? "success" : "info");
              })
              .catch((err) => console.error("viewport_pick_region failed", err));
          }}
          onPointerCancel={() => {
            // A cancelled pointer (the OS took it, a touch was interrupted) must not leave a rectangle
            // painted over the stage with no release coming to clear it.
            marqueePress.current = null;
            setMarqueeDrag(null);
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
            // THE WHOLE SELECTION, not the primary. `selectedId` is one of what may be 378 outlined
            // objects; a menu built from it offered `Delete` over a picture of 378 and removed one.
            const { multiSelect, selectedId } = projectionStore.getState();
            const sel = multiSelect.length ? multiSelect : selectedId ? [selectedId] : [];
            if (sel.length) setCtx({ ids: sel, x: e.clientX, y: e.clientY });
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
          <Onboarding show={!sceneEmpty && !playing && !stageSheet} onStart={() => openEngine("build")} />
          {sceneEmpty && !playing && !stageSheet && (
            <EmptyState
              onDrawPipe={() => setActiveTool("pipe")}
              onBrowseAssets={() => openEngine("build")}
              onImport={importAsset}
            />
          )}
          {marqueeDrag && (
            <StageMarquee
              box={marqueeBox(marqueeDrag.start, marqueeDrag.current)}
              origin={marqueeDrag.origin}
              mode={marqueeMode(marqueeDrag.start.x, marqueeDrag.current.x)}
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
              ids={ctx.ids}
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
      <StatusBar />
      <Rejections />
    </div>
  );
}
