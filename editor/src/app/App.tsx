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
import { setStatus } from "../store/ui";
import { Modal, Popover } from "../theme/Popover";
import { Button } from "../theme/primitives";
import { DockRail } from "../theme/workspace";
import { color, elevation, font, fontSize, motion, radius, space, z } from "../theme/tokens";
import { STAGE_MIN, type DockForm, dockForm, dockGridColumns, panelLayout } from "./layout";
import { usePlayerDrive } from "./usePlayerDrive";
import { ViewportToolbar } from "../panels/ViewportToolbar";
import { Rejections } from "../panels/Rejections";
import { StatusBar } from "../panels/StatusBar";
import { ContextMenu } from "../panels/ContextMenu";
import { ToastHost } from "../panels/ToastHost";
import { EmptyState } from "../panels/EmptyState";
import { Onboarding } from "../panels/Onboarding";
import { FocusBanner } from "../panels/FocusBanner";
import { CommandPalette, type EditorCommand } from "../panels/CommandPalette";
import { ViewportToolRail, type ViewportTool } from "../panels/ViewportToolRail";
import type { PipeForgeStatus } from "../transport/protocol";
import { EditorHeader } from "./EditorHeader";
import { LeftDock, InspectorDock, type LeftWorkspace, type InspectorWorkspace } from "./EditorDocks";
import { EngineRail, ENGINES, engineById, type EngineId } from "./EngineRail";
import { BottomDock, type BottomWorkspace } from "./BottomDock";
import { normalizeSurfacePoint } from "./viewportCoordinates";

// Pipe Forge's graph/fitting/catalog workbench is substantial and used only when its viewport tool is
// active. Keep the default editor entry lean; the compact loading state occupies the same overlay region.
const PipeForge = lazy(() => import("../panels/PipeForge").then((module) => ({ default: module.PipeForge })));

// The collapsed side rails carry only what that dock actually holds. The sub-engines are NOT repeated
// here — they live on the Engines rail, which is always visible, and listing them twice in different
// orders is exactly the confusion this layout removes.
const LEFT_RAIL_ITEMS = [
  { id: "scene", label: "Scene", icon: "▱" },
  { id: "build", label: "Build", icon: "+" },
  { id: "terrain", label: "Terrain", icon: "◭" },
] as const;
const RIGHT_RAIL_ITEMS = [
  { id: "properties", label: "Properties", icon: "⚙" },
  { id: "relations", label: "Relations", icon: "⌘" },
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
      <span>{paused ? "⏸ PAUSED" : "● PLAYING"}</span>
      <span style={{ color: color.text.muted }}>— Esc or</span>
      <Button data-testid="stageStop" variant="danger" compact onClick={onStop}>
        ⏹ Stop
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
        setStatus("⏹ stopped");
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
  // the object list. The dock therefore keeps a FULLY OPAQUE background and dims only its contents.
  const dockSurface: React.CSSProperties = { overflow: "hidden", background: "var(--mtk-bg-panel)" };
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
      onImport={() => void client.importAssetDialog().then((id) => {
        selectCreated(id);
        if (id) openEngine("model");
      })}
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
        onOpenLeftDock={() => setDrawer("left")}
        onOpenRightDock={() => setDrawer("right")}
      />
      <div style={{ flex: 1, display: "grid", gridTemplateColumns: dockGridColumns(layout, leftDockCollapsed, rightDockCollapsed, vw), minHeight: 0 }}>
        {/* ENGINES — the one index of what this editor can do. Always the first column (except in the
            phone-width overlay layout, where the whole shell is a single column). */}
        {!layout.overlay && (
          // Same reason as the docks: the rail dims over an opaque backing rather than dimming its own
          // background, or the viewport shows through it during Play.
          <div style={{ display: "flex", ...dockSurface }}>
            <EngineRail
              active={engine}
              compact={layout.collapsed}
              badges={{ scene: order.length || undefined, gameplay: playing ? "live" : undefined }}
              onChange={openEngine}
              style={chromeDim}
            />
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
          <div style={{ borderRight: "1px solid var(--mtk-border-subtle)", ...dockSurface }}>
            <div style={dockContent}>{leftPanel}</div>
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
            must not also cover the "● PLAYING" badge, which `<ux_quality>` 5 requires to be
            unmistakable on the stage itself. */}
        <div
          ref={stageColumn}
          className="mtk-stage-column"
          style={{ "--mtk-stage-min": `${STAGE_MIN}px`, "--mtk-z-dock-sheet": z.drawer } as React.CSSProperties}
        >
        <div
          id="viewport"
          data-testid="viewport"
          role="region"
          tabIndex={-1}
          aria-label="3D viewport"
          onPointerDown={(e) => {
            if (e.button === 2) {
              rightDrag.current = { x: e.clientX, y: e.clientY, moved: false };
              client.dragStart(); // native orbit begins; the render loop polls the cursor (0 IPC/frame)
              return;
            }
            if (e.button === 0) {
              if (pipeActive) return; // drawing owns the click; do not start a gizmo drag underneath it
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
          onPointerMove={(e) => {
            const rd = rightDrag.current;
            if (rd && (Math.abs(e.clientX - rd.x) > 6 || Math.abs(e.clientY - rd.y) > 6)) rd.moved = true;
          }}
          onPointerUp={(e) => {
            if (e.button === 2 && rightDrag.current) client.dragEnd();
            if (e.button === 0 && gizmoHit.current) client.gizmoDragEnd(); // commit the gizmo move (one tx)
          }}
          onWheel={(e) => client.zoom(e.deltaY * 0.04)}
          onContextMenu={(e) => {
            e.preventDefault();
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
            color: color.text.faint,
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
              anyway — it comes back with the stage. */}
          <Onboarding show={!sceneEmpty && !playing && !stageSheet} onStart={() => openEngine("build")} />
          {sceneEmpty && !playing && !stageSheet && (
            <EmptyState
              onDrawPipe={() => setActiveTool("pipe")}
              onBrowseAssets={() => openEngine("build")}
              onImport={() => void client.importAssetDialog().then((id) => {
                selectCreated(id);
                if (id) openEngine("model");
              })}
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
          <div style={{ borderLeft: "1px solid var(--mtk-border-subtle)", ...dockSurface }}>
            <div style={dockContent}>{rightPanel}</div>
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
            style={{
              position: "absolute",
              top: 0,
              bottom: 0,
              ...(drawer === "left" ? { left: 0, borderRight: "1px solid var(--mtk-border-subtle)" } : { right: 0, borderLeft: "1px solid var(--mtk-border-subtle)" }),
              width: "min(340px, calc(100vw - 36px))",
              background: "var(--mtk-bg-inset)",
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
        // Portaled + edge-aware (Popover): the right-click menu can no longer be clipped by a panel's
        // `overflow` or open off-screen near a viewport edge (it clamps/flips into view).
        <Popover open anchorPoint={{ x: ctx.x, y: ctx.y }} onClose={() => setCtx(null)}>
          <ContextMenu
            client={client}
            id={ctx.id}
            onClose={() => setCtx(null)}
            onFocus={(id, dist) => setFocused({ id, dist })}
          />
        </Popover>
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
      <CommandPalette
        open={commandsOpen}
        onClose={() => setCommandsOpen(false)}
        commands={commands}
        onCommandError={(error, command) => {
          console.error(`command ${command.id} failed`, error);
          setStatus(`${command.label} could not be completed`);
        }}
      />
      <StatusBar />
      <Rejections />
    </div>
  );
}
