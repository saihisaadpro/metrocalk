//! Editor shell — wires the projection store to a (mock) core over the in-process transport and lays
//! out the panels. The viewport is a placeholder rect that demonstrates the **input-ownership
//! contract**: pointer events over it are deferred to the native wgpu layer (invariant 4), wired for
//! real in M2.6. Everything else here is UI chrome owned by React.
//!
//! M10.10 UX hardening: the **stage is layout-priority** — side panels collapse to icon rails below a
//! breakpoint so the viewport never collapses first (C8); **Play is unmistakable on the stage** — a
//! coloured border + an overlaid "● PLAYING — Esc / ⏹ to stop" badge + de-emphasised edit chrome (C2);
//! feedback lands as **toasts over the stage** (C11); a fresh scene shows a real **empty state** (C10).

import { useEffect, useRef, useState } from "react";
import { createSession, isTauri, type EditorClient } from "../transport/session";
import { projectionStore, useEntityOrder } from "../store/projection";
import { thumbnailStore, startThumbnailPump } from "../store/thumbnails";
import { playStore, usePlaying, usePaused } from "../store/play";
import { setStatus } from "../store/ui";
import { Popover } from "../theme/Popover";
import { Button } from "../theme/primitives";
import { color, font, fontSize, radius, space, z } from "../theme/tokens";
import { panelLayout } from "./layout";
import { Hierarchy } from "../panels/Hierarchy";
import { AuthoringToolbar } from "../panels/AuthoringToolbar";
import { ViewportToolbar } from "../panels/ViewportToolbar";
import { Rejections } from "../panels/Rejections";
import { Onboarding } from "../panels/Onboarding";
import { Reveal } from "../panels/Reveal";
import { DescribeBar } from "../panels/DescribeBar";
import { Wallet } from "../panels/Wallet";
import { Requirers } from "../panels/Requirers";
import { StatusBar } from "../panels/StatusBar";
import { ContextMenu } from "../panels/ContextMenu";
import { AssetBrowser } from "../panels/AssetBrowser";
import { FileMenu } from "../panels/FileMenu";
import { PlayControls } from "../panels/PlayControls";
import { ToastHost } from "../panels/ToastHost";
import { EmptyState } from "../panels/EmptyState";
import { AiEditPanel } from "../panels/AiEditPanel";
import { ImportReport } from "../panels/ImportReport";
import { ReimportPanel } from "../panels/ReimportPanel";
import { JointPanel } from "../panels/JointPanel";
import { Diagnostics } from "../panels/Diagnostics";
import { Inspector } from "../inspector/Inspector";
import { BindingGraph } from "../graph/BindingGraph";
import { PhysicsPanel } from "../panels/PhysicsPanel";
import { RulesPanel } from "../panels/RulesPanel";
import { ComposePanel } from "../panels/ComposePanel";
import { StateGraphPanel } from "../panels/StateGraphPanel";
import { RuleDebugPanel } from "../panels/RuleDebugPanel";
import { TransformPanel } from "../panels/TransformPanel";
import { FocusBanner } from "../panels/FocusBanner";

/** Build the editor session once: the REAL Tauri shell transport inside the packaged `.exe` (the live
 *  `/core` over the `connect` Channel), else the in-process MockCore for `npm run dev` / tests. */
function useEditorSession(): EditorClient {
  const ref = useRef<EditorClient | null>(null);
  if (!ref.current) {
    ref.current = createSession();
  }
  return ref.current;
}

/** The persistent "● PLAYING" badge overlaid ON the stage (not only the toolbar) — Play must be
 *  unmistakable where the user is looking (C2). Stop is always one click away here too. */
function PlayBadge({ paused, onStop }: { paused: boolean; onStop: () => void }) {
  return (
    <div
      id="playStageBadge"
      data-testid="playStageBadge"
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
        boxShadow: "0 4px 16px #0008",
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

/** A collapsed side panel = a thin icon rail (C8): the stage keeps the space; one click opens the panel
 *  as an overlay drawer. */
function Rail({ side, label, onOpen }: { side: "left" | "right"; label: string; onOpen: () => void }) {
  const border = side === "left" ? { borderRight: `1px solid ${color.border.subtle}` } : { borderLeft: `1px solid ${color.border.subtle}` };
  return (
    <div style={{ ...border, display: "flex", alignItems: "center", justifyContent: "center", height: "100%", background: color.bg.panel }}>
      <button
        data-testid={`rail-${side}`}
        className="mtk-btn mtk-btn--ghost"
        onClick={onOpen}
        title={`Open ${label} (window is narrow — the stage keeps priority)`}
        style={{ color: color.text.secondary, writingMode: "vertical-rl", padding: `${space.md}px 2px`, fontSize: fontSize.meta, letterSpacing: 1 }}
      >
        ☰ {label}
      </button>
    </div>
  );
}

export function App() {
  const client = useEditorSession();
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

  const playing = usePlaying();
  const paused = usePaused();
  const order = useEntityOrder();
  const sceneEmpty = order.length === 0;

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

  // Responsive layout — the stage is layout-priority; panels collapse to rails below a breakpoint (C8).
  const [vw, setVw] = useState(() => (typeof window !== "undefined" ? window.innerWidth : 1440));
  useEffect(() => {
    const onResize = () => setVw(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);
  const layout = panelLayout(vw);
  // Which collapsed panel is currently opened as an overlay drawer.
  const [drawer, setDrawer] = useState<"left" | "right" | null>(null);
  useEffect(() => {
    if (!layout.collapsed) setDrawer(null); // widening back out closes the drawer
  }, [layout.collapsed]);

  // Ctrl-Z / ⌘-Z → undo; Escape closes the context menu, then a drawer, then STOPS Play (the badge says
  // "Esc … to stop"). A discrete event — never the per-frame hot path (invariant 4).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement as HTMLElement | null;
      const editing = !!el && (el.tagName === "INPUT" || el.tagName === "TEXTAREA" || el.isContentEditable);
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
        // Don't hijack TEXT undo while the user is typing in a field — only undo the SCENE otherwise.
        if (editing) return;
        e.preventDefault();
        // Honest feedback: only say "undo" when a transaction was actually reverted (the shell reports it),
        // else "nothing to undo" — never claim a revert on an empty history.
        void client.undo().then((did) => setStatus(did ? "undo" : "nothing to undo"));
      }
      // M9 gizmo mode — the universal W/E/R game-editor shortcut (sticky tool state; guarded off text fields
      // + modifier chords so it never fires while editing or during Ctrl-Z). A discrete command, not per-frame.
      const k = e.key.toLowerCase();
      if ((k === "w" || k === "e" || k === "r") && !e.ctrlKey && !e.metaKey && !e.altKey && !editing) {
        client.gizmoMode(k === "w" ? "translate" : k === "e" ? "rotate" : "scale");
        setStatus(k === "w" ? "move (W)" : k === "e" ? "rotate (E)" : "scale (R)");
      }
      if (e.key === "Escape") {
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
  }, [client, ctx, drawer, playing, focused]);

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
    ? { opacity: 0.45, pointerEvents: "none", transition: "opacity .2s" }
    : { transition: "opacity .2s" };

  const leftPanel = (
    <div style={{ overflow: "hidden", display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={{ borderBottom: "1px solid var(--mtk-border-subtle)", maxHeight: 200, overflowY: "auto" }}>
        <AssetBrowser client={client} />
      </div>
      <div style={{ borderBottom: "1px solid var(--mtk-border-subtle)", maxHeight: 140, overflowY: "auto" }}>
        <Requirers />
      </div>
      <AuthoringToolbar client={client} />
      <div style={{ flex: 1, overflow: "hidden", minHeight: 0 }}>
        <Hierarchy
          client={client}
          onContextMenu={(id, x, y) => {
            // No context actions while Playing (editing is gated off in Play — mirrors the viewport menu).
            if (playing) return;
            setCtx({ id, x, y });
          }}
        />
      </div>
    </div>
  );
  const rightPanel = (
    <div style={{ overflowY: "auto", display: "flex", flexDirection: "column", height: "100%" }}>
      <Inspector client={client} />
      <AiEditPanel client={client} />
      {/* M15.9 — rig + animate the selected part as a mechanism joint (real axis, undoable, scrubbable). */}
      <div style={{ borderTop: "1px solid var(--mtk-border-subtle)" }}>
        <JointPanel client={client} />
      </div>
      {/* M15.10 — the re-import diff + adjudication (renders only after a re-import over an existing CAD scene). */}
      <div style={{ borderTop: "1px solid var(--mtk-border-subtle)" }}>
        <ReimportPanel client={client} />
      </div>
      {/* M15.7 — the never-silent CAD import report (renders only when the scene has imported CAD). */}
      <div style={{ borderTop: "1px solid var(--mtk-border-subtle)" }}>
        <ImportReport client={client} />
      </div>
      <div style={{ borderTop: "1px solid var(--mtk-border-subtle)" }}>
        <Diagnostics client={client} />
      </div>
      <div style={{ borderTop: "1px solid var(--mtk-border-subtle)" }}>
        <Reveal client={client} />
      </div>
      <TransformPanel client={client} />
      <PhysicsPanel client={client} />
      <RulesPanel client={client} />
      <ComposePanel client={client} />
      <StateGraphPanel client={client} />
      <RuleDebugPanel client={client} />
      <div style={{ borderTop: "1px solid var(--mtk-border-subtle)", flex: 1, minHeight: 220 }}>
        <BindingGraph />
      </div>
    </div>
  );

  return (
    // Root is the chrome backdrop. In the .exe (`native`) it is **transparent** so the native wgpu scene
    // composites up through the transparent viewport hole (ADR-008) — the panels below paint their OWN
    // opaque background so only the viewport stays a hole. (A `#0a0a0f` root here would occlude the wgpu
    // layer even behind the transparent viewport div — the bug that left the .exe viewport black.)
    <div style={{ height: "100vh", display: "flex", flexDirection: "column", background: native ? "transparent" : color.bg.base, color: color.text.primary, font: font.ui }}>
      <div style={{ height: 40, display: "flex", alignItems: "center", gap: space.lg, padding: `0 ${space.lg}px`, background: color.bg.raised, borderBottom: `1px solid ${color.border.subtle}`, font: font.ui, fontSize: fontSize.label, overflow: "hidden", minWidth: 0 }}>
        <strong style={{ letterSpacing: 0.3, color: color.text.primary }}>
          metrocalk<span style={{ color: color.accent.base }}>.</span>
        </strong>
        <FileMenu client={client} />
        <PlayControls client={client} />
        <div style={{ marginLeft: "auto", minWidth: 0 }}>
          <Wallet client={client} />
        </div>
      </div>
      <div style={{ borderBottom: `1px solid ${color.border.subtle}`, background: color.bg.panel }}>
        <DescribeBar client={client} />
      </div>
      <div style={{ flex: 1, display: "grid", gridTemplateColumns: layout.gridColumns, minHeight: 0 }}>
        {/* LEFT — full panel, or a collapsed icon rail (the stage keeps priority on resize). */}
        {layout.collapsed ? (
          <Rail side="left" label="Scene" onOpen={() => setDrawer("left")} />
        ) : (
          <div style={{ borderRight: "1px solid var(--mtk-border-subtle)", overflow: "hidden", background: "var(--mtk-bg-panel)", ...chromeDim }}>{leftPanel}</div>
        )}

        {/* viewport: native-owned (invariant 4). Inside the `.exe` it is **transparent** so the native wgpu
            scene composites through (ADR-008); the per-frame orbit/zoom runs in the native render loop (the
            JS only fires drag_start/drag_end/zoom/pick — never per frame). Left-click → native pick → select;
            right-DRAG → orbit (suppress the menu); right-CLICK → the M3.3 context menu. */}
        <div
          id="viewport"
          data-testid="viewport"
          onPointerDown={(e) => {
            if (e.button === 2) {
              rightDrag.current = { x: e.clientX, y: e.clientY, moved: false };
              client.dragStart(); // native orbit begins; the render loop polls the cursor (0 IPC/frame)
              return;
            }
            if (e.button === 0) {
              // M9 gizmo handle-grab: only when an entity is selected; if a handle is HIT the render loop
              // drags it natively (0 IPC/frame, like orbit) and the release commits. A miss falls through to
              // the normal pick. The hit flag resolves async, so a WebDriver synthetic click (which fires
              // immediately) still picks normally — the suppression is for real human-timed drags.
              gizmoHit.current = false;
              if (projectionStore.getState().selectedId) {
                const r = e.currentTarget.getBoundingClientRect();
                const nx = (e.clientX - r.left) / Math.max(1, r.width);
                const ny = (e.clientY - r.top) / Math.max(1, r.height);
                void client
                  .gizmoPickDrag(nx, ny, e.ctrlKey || e.metaKey)
                  .then((hit) => (gizmoHit.current = hit))
                  .catch(() => {});
              }
            }
          }}
          onClick={(e) => {
            // A left-press that grabbed a gizmo handle is a DRAG, not a pick — don't re-select.
            if (gizmoHit.current) {
              gizmoHit.current = false;
              return;
            }
            // Left-click pick on the click event (fires reliably under WebDriver `element.click()`, unlike a
            // synthesized pointerdown). Pick at the click's NORMALIZED viewport coords (the command rays the
            // camera — no OS-cursor dependency) → select + a stable "picked"/"nothing here" status.
            const r = e.currentTarget.getBoundingClientRect();
            const nx = (e.clientX - r.left) / Math.max(1, r.width);
            const ny = (e.clientY - r.top) / Math.max(1, r.height);
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
            if (playing) {
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
            background: native ? "transparent" : "var(--mtk-bg-inset)", // transparent → wgpu composites through (.exe)
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: color.text.faint,
            font: font.mono,
            fontSize: fontSize.body,
            outline: playing ? `3px solid ${paused ? "var(--mtk-warn-border)" : "var(--mtk-success-border)"}` : "none",
            outlineOffset: -3,
            boxShadow: playing ? `inset 0 0 60px ${paused ? "#fbbf2433" : "#2f9e5444"}` : "none",
            transition: "outline-color .2s, box-shadow .2s",
          }}
        >
          {!native && "native wgpu viewport — drag to orbit · scroll to zoom · click to select (live in the .exe)"}
          {!playing && <ViewportToolbar client={client} />}
          {playing && <PlayBadge paused={paused} onStop={stopPlay} />}
          {sceneEmpty && !playing && <EmptyState />}
          <ToastHost top={playing ? 52 : 14} />
        </div>

        {/* RIGHT — full panel, or a collapsed icon rail. */}
        {layout.collapsed ? (
          <Rail side="right" label="Inspector" onOpen={() => setDrawer("right")} />
        ) : (
          <div style={{ borderLeft: "1px solid var(--mtk-border-subtle)", overflow: "hidden", background: "var(--mtk-bg-panel)", ...chromeDim }}>{rightPanel}</div>
        )}
      </div>

      {/* Collapsed-panel overlay drawer (opened from a rail). */}
      {drawer && (
        <>
          <div onClick={() => setDrawer(null)} style={{ position: "fixed", inset: 0, zIndex: z.overlay, background: "#0006" }} />
          <div
            data-testid={`drawer-${drawer}`}
            style={{
              position: "fixed",
              top: 40,
              bottom: 0,
              ...(drawer === "left" ? { left: 0, borderRight: "1px solid var(--mtk-border-subtle)" } : { right: 0, borderLeft: "1px solid var(--mtk-border-subtle)" }),
              width: 300,
              zIndex: z.drawer,
              background: "var(--mtk-bg-inset)",
              overflow: "auto",
              boxShadow: "0 0 30px #000a",
            }}
          >
            {drawer === "left" ? leftPanel : rightPanel}
          </div>
        </>
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
      <StatusBar />
      <Rejections />
      <Onboarding />
    </div>
  );
}
