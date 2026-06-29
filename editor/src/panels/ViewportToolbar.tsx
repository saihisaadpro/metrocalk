//! ViewportToolbar (M10.7 / ADR-037) — the viewport's authoring controls, overlaid on the stage. Surfaces
//! the **shipped native M9 gizmo** (mode W/E/R · world/local space · origin/center pivot — the drag stays
//! native + 0-IPC, this is just the toolbar) and the **camera/framing ergonomics** every editor has
//! (frame-selected · frame-all · view presets top/front/side/persp + an orientation readout · snap toggle).
//!
//! **Single-source gizmo state (no desync):** the toolbar POLLS `gizmo_debug` (the one authoritative gizmo
//! state, owned by the render thread) on a slow chrome interval — never per-frame (invariant 4) — and
//! refreshes immediately after a toolbar action, so the W/E/R keyboard shortcuts and the toolbar can't drift
//! apart. Stable `#vp*` ids for the prompt-40 gate.

import { useEffect, useRef, useState } from "react";
import { setStatus } from "../store/ui";
import { Button } from "../theme/primitives";
import { color, font, fontSize, radius, space as sp, z } from "../theme/tokens";
import type { EditorClient } from "../transport/session";

type Mode = "translate" | "rotate" | "scale";

/** A compact view label from the camera's [orbit, elevation] (the orientation readout). */
function viewLabel(cam: number[] | null): string {
  if (!cam) return "persp";
  const [orbit, elevation] = cam;
  if (elevation > 1.2) return "top";
  if (Math.abs(elevation) < 0.15) {
    if (Math.abs(Math.abs(orbit) - Math.PI / 2) < 0.2) return "front";
    if (Math.abs(orbit) < 0.2) return "side";
  }
  return "persp";
}

export function ViewportToolbar({ client }: { client: EditorClient }) {
  const [mode, setMode] = useState<Mode>("translate");
  const [hasSel, setHasSel] = useState(false);
  const [space, setSpace] = useState("world");
  const [pivot, setPivot] = useState("origin");
  const [cam, setCam] = useState<number[] | null>(null);
  const [snapOn, setSnapOn] = useState(true);
  const timer = useRef<number | null>(null);

  async function refresh() {
    try {
      const [m, sel, , spaceVal, pv] = await client.gizmoDebug();
      setMode(m as Mode);
      setHasSel(sel);
      setSpace(spaceVal);
      setPivot(pv);
      setCam(await client.cameraDebug());
    } catch {
      /* live-only (the dev MockCore returns inert defaults) — never throw in the UI */
    }
  }

  useEffect(() => {
    void refresh();
    // A slow chrome poll keeps the toolbar in sync with the W/E/R keys (the gizmo state is single-source on
    // the render thread). NEVER per-frame — the viewport hot path stays native (invariant 4).
    timer.current = window.setInterval(() => void refresh(), 500);
    return () => {
      if (timer.current != null) window.clearInterval(timer.current);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client]);

  const setGizmoMode = (m: Mode) => {
    client.gizmoMode(m);
    setMode(m);
  };
  const frameSelected = async () => {
    const sel = await client.gizmoSelected().catch(() => null);
    if (sel) {
      client.focusEntity(sel);
      setStatus("framed the selection");
      void refresh();
    } else {
      setStatus("select something to frame (F)");
    }
  };
  const preset = (p: string) => {
    client.viewPreset(p);
    setStatus(`view: ${p}`);
    void refresh();
  };
  const toggleSnap = () => {
    const next = !snapOn;
    setSnapOn(next);
    client.setSnap(!next); // setSnap(on)=false ⇒ snapping ON; track the user-facing "snap on"
  };

  // The active state reads LIVE from the render-thread gizmo/camera state (the `is-active` accent), never a
  // static highlight — the accepted-tier "surface live truth" bar (M14.1).
  const btn = (id: string, label: string, on: boolean, onClick: () => void, title?: string, enabled = true) => (
    <Button id={id} data-testid={id} variant="toggle" active={on} compact disabled={!enabled} title={title} onClick={onClick}>
      {label}
    </Button>
  );

  const sep = <span style={{ width: 1, alignSelf: "stretch", background: color.border.default, margin: `0 ${sp.xxs}px` }} />;
  const view = viewLabel(cam);

  return (
    <div
      id="vptoolbar"
      data-testid="vptoolbar"
      // Toolbar interactions must NOT fall through to the viewport (pick/orbit/context-menu).
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
      style={{
        position: "absolute",
        top: sp.sm,
        left: sp.sm,
        display: "flex",
        gap: sp.xxs,
        alignItems: "center",
        padding: `${sp.xs}px ${sp.sm}px`,
        background: color.bg.raised,
        border: `1px solid ${color.border.default}`,
        borderRadius: radius.lg,
        boxShadow: "0 2px 8px #0006",
        zIndex: z.chrome,
        pointerEvents: "auto",
      }}
    >
      {/* The shipped M9 gizmo — mode / space / pivot (the drag is native, 0-IPC). */}
      {btn("vpMove", "Move", mode === "translate", () => setGizmoMode("translate"), "Translate (W)")}
      {btn("vpRotate", "Rotate", mode === "rotate", () => setGizmoMode("rotate"), "Rotate (E)")}
      {btn("vpScale", "Scale", mode === "scale", () => setGizmoMode("scale"), "Scale (R)")}
      {sep}
      {btn("vpSpace", space === "world" ? "World" : "Local", false, () => {
        void client.gizmoSpaceToggle().then((s) => setSpace(s));
      }, "Toggle world/local")}
      {btn("vpPivot", pivot === "origin" ? "Pivot ⊙" : "Pivot ◉", false, () => {
        void client.gizmoPivotToggle().then((p) => setPivot(p));
      }, "Toggle origin/center pivot")}
      {sep}
      {/* Camera & framing. */}
      {btn("vpFrameSel", "Frame ⊙", false, () => void frameSelected(), "Frame selected (F)", hasSel)}
      {btn("vpFrameAll", "Frame all", false, () => client.frameAll(), "Frame the whole scene")}
      {sep}
      {btn("vpTop", "Top", view === "top", () => preset("top"))}
      {btn("vpFront", "Front", view === "front", () => preset("front"))}
      {btn("vpSide", "Side", view === "side", () => preset("side"))}
      {btn("vpPersp", "Persp", view === "persp", () => preset("persp"))}
      {/* The orientation readout (the orientation cube's role — clickable presets above; a true 3D cube is
          a render-fidelity follow-up). */}
      <span id="vpOrient" data-testid="vpOrient" data-view={view} style={{ color: color.accent.base, font: font.mono, fontSize: fontSize.micro, padding: `0 ${sp.xs}px` }}>
        ▣ {view}
      </span>
      {sep}
      {btn("vpSnap", snapOn ? "Snap ✓" : "Snap ✗", snapOn, toggleSnap, "Magnetic snap (grid/angle)")}
    </div>
  );
}
