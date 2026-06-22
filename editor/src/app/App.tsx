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
import { playStore, usePlaying, usePaused } from "../store/play";
import { setStatus } from "../store/ui";
import { panelLayout } from "./layout";
import { Hierarchy } from "../panels/Hierarchy";
import { Rejections } from "../panels/Rejections";
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
import { Inspector } from "../inspector/Inspector";
import { BindingGraph } from "../graph/BindingGraph";

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
        top: 12,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: 140,
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "4px 12px",
        borderRadius: 999,
        background: paused ? "#3a3416ee" : "#143a22ee",
        border: `1px solid ${paused ? "#fbbf24" : "#2f9e54"}`,
        color: paused ? "#fbbf24" : "#7fe39a",
        font: "12px ui-monospace, monospace",
        boxShadow: "0 4px 16px #0008",
      }}
    >
      <span>{paused ? "⏸ PAUSED" : "● PLAYING"}</span>
      <span style={{ opacity: 0.7 }}>— Esc or</span>
      <button
        data-testid="stageStop"
        onClick={onStop}
        style={{ background: "#5a2f1f", color: "#fcd", border: "1px solid #6a3f2f", borderRadius: 4, padding: "1px 8px", cursor: "pointer", font: "11px ui-monospace, monospace" }}
      >
        ⏹ Stop
      </button>
    </div>
  );
}

/** A collapsed side panel = a thin icon rail (C8): the stage keeps the space; one click opens the panel
 *  as an overlay drawer. */
function Rail({ side, label, onOpen }: { side: "left" | "right"; label: string; onOpen: () => void }) {
  const border = side === "left" ? { borderRight: "1px solid #2a2d35" } : { borderLeft: "1px solid #2a2d35" };
  return (
    <div style={{ ...border, display: "flex", alignItems: "center", justifyContent: "center", height: "100%" }}>
      <button
        data-testid={`rail-${side}`}
        onClick={onOpen}
        title={`Open ${label} (window is narrow — the stage keeps priority)`}
        style={{ background: "transparent", color: "#9aa0aa", border: "none", cursor: "pointer", writingMode: "vertical-rl", padding: "8px 2px", font: "11px ui-monospace, monospace", letterSpacing: 1 }}
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
  // Tracks a right-press for the orbit-vs-context-menu movement threshold (the scaffold's disambiguation).
  const rightDrag = useRef<{ x: number; y: number; moved: boolean } | null>(null);

  const playing = usePlaying();
  const paused = usePaused();
  const order = useEntityOrder();
  const sceneEmpty = order.length === 0;

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
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
        e.preventDefault();
        client.undo();
        setStatus("undo");
      }
      if (e.key === "Escape") {
        if (ctx) {
          setCtx(null);
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
  }, [client, ctx, drawer, playing]);

  function stopPlay() {
    void client.stop().then((info) => {
      playStore.getState().refresh(info);
      setStatus("⏹ stopped");
    });
  }

  // Edit chrome is de-emphasised while playing (the mode switch is felt in peripheral vision; edits are
  // also gated off on the shell).
  const chromeDim: React.CSSProperties = playing
    ? { opacity: 0.45, pointerEvents: "none", transition: "opacity .2s" }
    : { transition: "opacity .2s" };

  const leftPanel = (
    <div style={{ overflow: "hidden", display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={{ borderBottom: "1px solid #2a2d35", maxHeight: 200, overflowY: "auto" }}>
        <AssetBrowser client={client} />
      </div>
      <div style={{ borderBottom: "1px solid #2a2d35", maxHeight: 140, overflowY: "auto" }}>
        <Requirers />
      </div>
      <div style={{ flex: 1, overflow: "hidden", minHeight: 0 }}>
        <Hierarchy />
      </div>
    </div>
  );
  const rightPanel = (
    <div style={{ overflowY: "auto", display: "flex", flexDirection: "column", height: "100%" }}>
      <Inspector client={client} />
      <AiEditPanel client={client} />
      <div style={{ borderTop: "1px solid #2a2d35" }}>
        <Reveal client={client} />
      </div>
      <div style={{ borderTop: "1px solid #2a2d35", flex: 1, minHeight: 220 }}>
        <BindingGraph />
      </div>
    </div>
  );

  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column", background: "#0a0a0f", color: "#e8e8e8" }}>
      <div style={{ height: 40, display: "flex", alignItems: "center", gap: 12, padding: "0 12px", background: "#14161c", borderBottom: "1px solid #2a2d35", font: "13px ui-monospace, monospace", overflow: "hidden", minWidth: 0 }}>
        <strong>metrocalk</strong>
        <FileMenu client={client} />
        <PlayControls client={client} />
        <div style={{ marginLeft: "auto", minWidth: 0 }}>
          <Wallet client={client} />
        </div>
      </div>
      <div style={{ borderBottom: "1px solid #2a2d35", background: "#101218" }}>
        <DescribeBar client={client} />
      </div>
      <div style={{ flex: 1, display: "grid", gridTemplateColumns: layout.gridColumns, minHeight: 0 }}>
        {/* LEFT — full panel, or a collapsed icon rail (the stage keeps priority on resize). */}
        {layout.collapsed ? (
          <Rail side="left" label="Scene" onOpen={() => setDrawer("left")} />
        ) : (
          <div style={{ borderRight: "1px solid #2a2d35", overflow: "hidden", ...chromeDim }}>{leftPanel}</div>
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
            } else if (e.button === 0) {
              void client.viewportPick().then((picked) => {
                if (picked) projectionStore.getState().select(picked);
              });
            }
          }}
          onPointerMove={(e) => {
            const rd = rightDrag.current;
            if (rd && (Math.abs(e.clientX - rd.x) > 6 || Math.abs(e.clientY - rd.y) > 6)) rd.moved = true;
          }}
          onPointerUp={(e) => {
            if (e.button === 2 && rightDrag.current) client.dragEnd();
          }}
          onWheel={(e) => client.zoom(e.deltaY * 0.04)}
          onContextMenu={(e) => {
            e.preventDefault();
            // a right-DRAG was an orbit, not a menu request — suppress the menu (movement threshold)
            const orbited = rightDrag.current?.moved;
            rightDrag.current = null;
            if (orbited) return;
            const sel = projectionStore.getState().selectedId;
            if (sel) setCtx({ id: sel, x: e.clientX, y: e.clientY });
          }}
          style={{
            position: "relative",
            background: native ? "transparent" : "#0d0f15", // transparent → wgpu composites through (.exe)
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            color: "#444",
            font: "12px ui-monospace, monospace",
            outline: playing ? `3px solid ${paused ? "#fbbf24" : "#2f9e54"}` : "none",
            outlineOffset: -3,
            boxShadow: playing ? `inset 0 0 60px ${paused ? "#fbbf2433" : "#2f9e5444"}` : "none",
            transition: "outline-color .2s, box-shadow .2s",
          }}
        >
          {!native && "native wgpu viewport — drag to orbit · scroll to zoom · click to select (live in the .exe)"}
          {playing && <PlayBadge paused={paused} onStop={stopPlay} />}
          {sceneEmpty && !playing && <EmptyState />}
          <ToastHost top={playing ? 52 : 14} />
        </div>

        {/* RIGHT — full panel, or a collapsed icon rail. */}
        {layout.collapsed ? (
          <Rail side="right" label="Inspector" onOpen={() => setDrawer("right")} />
        ) : (
          <div style={{ borderLeft: "1px solid #2a2d35", overflow: "hidden", ...chromeDim }}>{rightPanel}</div>
        )}
      </div>

      {/* Collapsed-panel overlay drawer (opened from a rail). */}
      {drawer && (
        <>
          <div onClick={() => setDrawer(null)} style={{ position: "fixed", inset: 0, zIndex: 110, background: "#0006" }} />
          <div
            data-testid={`drawer-${drawer}`}
            style={{
              position: "fixed",
              top: 40,
              bottom: 0,
              ...(drawer === "left" ? { left: 0, borderRight: "1px solid #2a2d35" } : { right: 0, borderLeft: "1px solid #2a2d35" }),
              width: 300,
              zIndex: 120,
              background: "#0d0f15",
              overflow: "auto",
              boxShadow: "0 0 30px #000a",
            }}
          >
            {drawer === "left" ? leftPanel : rightPanel}
          </div>
        </>
      )}

      {ctx && (
        <>
          <div onClick={() => setCtx(null)} style={{ position: "fixed", inset: 0, zIndex: 90 }} />
          <div style={{ position: "fixed", left: ctx.x, top: ctx.y, zIndex: 100 }}>
            <ContextMenu client={client} id={ctx.id} onClose={() => setCtx(null)} />
          </div>
        </>
      )}
      <StatusBar />
      <Rejections />
    </div>
  );
}
