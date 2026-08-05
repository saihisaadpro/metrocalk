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
import { MenuPopup, PopupMenuGroup, PopupMenuItem } from "../theme/workspace";
import { color, elevation, font, fontSize, radius, space as sp, z } from "../theme/tokens";
import type { EditorClient, ViewportRenderProfile } from "../transport/session";

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

export interface ViewportToolbarProps {
  client: EditorClient;
  /** Standalone embeds may keep gizmo modes here; the full editor owns them in the primary tool rail. */
  showTransformTools?: boolean;
}

export function ViewportToolbar({ client, showTransformTools = true }: ViewportToolbarProps) {
  const [mode, setMode] = useState<Mode>("translate");
  const [hasSel, setHasSel] = useState(false);
  const [space, setSpace] = useState("world");
  const [pivot, setPivot] = useState("origin");
  const [cam, setCam] = useState<number[] | null>(null);
  const [snapOn, setSnapOn] = useState(true);
  const [renderProfile, setRenderProfile] = useState<ViewportRenderProfile>("cinematic");
  const timer = useRef<number | null>(null);
  const mounted = useRef(false);
  const refreshInFlight = useRef<Promise<void> | null>(null);
  const refreshAgain = useRef(false);

  function refresh(): Promise<void> {
    if (refreshInFlight.current) {
      refreshAgain.current = true;
      return refreshInFlight.current;
    }
    const request = (async () => {
      do {
        refreshAgain.current = false;
        try {
          const [[m, sel, , spaceVal, pv], camera, profile] = await Promise.all([
            client.gizmoDebug(),
            client.cameraDebug(),
            client.renderProfileDebug(),
          ]);
          if (!mounted.current) return;
          setMode(m as Mode);
          setHasSel(sel);
          setSpace(spaceVal);
          setPivot(pv);
          setCam(camera);
          setRenderProfile(profile);
        } catch {
          /* live-only (the dev MockCore returns inert defaults) — never throw in the UI */
        }
      } while (refreshAgain.current);
    })();
    const settled = request.finally(() => {
      refreshInFlight.current = null;
    });
    refreshInFlight.current = settled;
    return settled;
  }

  useEffect(() => {
    mounted.current = true;
    let disposed = false;
    // A slow, self-scheduled chrome poll keeps the toolbar in sync with W/E/R without overlapping requests.
    // NEVER per-frame — the viewport hot path stays native (invariant 4).
    const poll = async () => {
      await refresh();
      if (!disposed) timer.current = window.setTimeout(() => void poll(), 500);
    };
    void poll();
    return () => {
      disposed = true;
      mounted.current = false;
      refreshAgain.current = false;
      if (timer.current != null) window.clearTimeout(timer.current);
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
    client.setSnap(next); // native accepts the user-facing state: true means snapping on
  };
  const toggleRenderProfile = async () => {
    const next: ViewportRenderProfile = renderProfile === "cinematic" ? "cad" : "cinematic";
    const applied = await client.setRenderProfile(next);
    setRenderProfile(applied);
    setStatus(applied === "cad" ? "CAD color inspection" : "cinematic viewport");
    void refresh();
  };
  // The active state reads LIVE from the render-thread gizmo/camera state (the `is-active` accent), never a
  // static highlight — the accepted-tier "surface live truth" bar (M14.1).
  const view = viewLabel(cam);
  const viewName = view === "persp" ? "Perspective" : view[0].toUpperCase() + view.slice(1);
  const cameraGridStyle = { display: "grid", gridTemplateColumns: "repeat(2, minmax(0, 1fr))", gap: sp.xs } as const;

  return (
    <div
      id="vptoolbar"
      data-testid="vptoolbar"
      role="toolbar"
      aria-label="Viewport tools"
      // Toolbar interactions must NOT fall through to the viewport (pick/orbit/context-menu).
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      onContextMenu={(e) => e.stopPropagation()}
      style={{
        position: "absolute",
        top: sp.sm,
        left: "50%",
        transform: "translateX(-50%)",
        display: "flex",
        gap: sp.xxs,
        alignItems: "center",
        padding: `${sp.xs}px ${sp.sm}px`,
        background: color.bg.raised,
        border: `1px solid ${color.border.subtle}`,
        borderRadius: radius.xxl,
        boxShadow: elevation.e1,
        zIndex: z.chrome,
        pointerEvents: "auto",
      }}
    >
      {/* The shipped M9 gizmo — mode / space / pivot (the drag is native, 0-IPC). */}
      <MenuPopup
        id="viewport-transform-popup"
        label="Transform settings"
        trigger={`Transform · ${space === "world" ? "World" : "Local"}`}
        triggerProps={{
          id: "vpTransform",
          "data-testid": "vpTransform",
          variant: "toggle",
          compact: true,
          title: "Transform mode, space, pivot, and snapping",
        }}
      >
        {(close) => (
          <div id="vpTransformMenu" style={{ display: "grid", gap: sp.md }}>
            {showTransformTools && (
              <PopupMenuGroup label="Mode">
                <PopupMenuItem
                  id="vpMove"
                  data-testid="vpMove"
                  label="Move"
                  meta="W"
                  role="menuitemradio"
                  aria-checked={mode === "translate"}
                  variant="toggle"
                  active={mode === "translate"}
                  onSelect={() => setGizmoMode("translate")}
                  onRequestClose={close}
                />
                <PopupMenuItem
                  id="vpRotate"
                  data-testid="vpRotate"
                  label="Rotate"
                  meta="E"
                  role="menuitemradio"
                  aria-checked={mode === "rotate"}
                  variant="toggle"
                  active={mode === "rotate"}
                  onSelect={() => setGizmoMode("rotate")}
                  onRequestClose={close}
                />
                <PopupMenuItem
                  id="vpScale"
                  data-testid="vpScale"
                  label="Scale"
                  meta="R"
                  role="menuitemradio"
                  aria-checked={mode === "scale"}
                  variant="toggle"
                  active={mode === "scale"}
                  onSelect={() => setGizmoMode("scale")}
                  onRequestClose={close}
                />
              </PopupMenuGroup>
            )}
            <PopupMenuGroup label="Transform context">
              <PopupMenuItem
                id="vpSpace"
                data-testid="vpSpace"
                label="Space"
                description="Toggle world or local axes"
                meta={space === "world" ? "World" : "Local"}
                onSelect={() => client.gizmoSpaceToggle().then((next) => setSpace(next))}
                onRequestClose={close}
              />
              <PopupMenuItem
                id="vpPivot"
                data-testid="vpPivot"
                label="Pivot"
                description="Toggle origin or selection centre"
                meta={pivot === "origin" ? "Origin" : "Centre"}
                onSelect={() => client.gizmoPivotToggle().then((next) => setPivot(next))}
                onRequestClose={close}
              />
              <PopupMenuItem
                id="vpSnap"
                data-testid="vpSnap"
                label="Snapping"
                description="Magnetic grid and angle snapping"
                meta={snapOn ? "On" : "Off"}
                role="menuitemcheckbox"
                aria-checked={snapOn}
                variant="toggle"
                active={snapOn}
                onSelect={toggleSnap}
                onRequestClose={close}
              />
            </PopupMenuGroup>
          </div>
        )}
      </MenuPopup>
      <MenuPopup
        id="viewport-view-popup"
        label="View and framing"
        trigger={`View · ${viewName}`}
        triggerProps={{
          id: "vpView",
          "data-testid": "vpView",
          variant: "toggle",
          compact: true,
          title: "Framing, camera presets, and render profile",
        }}
      >
        {(close) => (
          <div id="vpViewMenu" style={{ display: "grid", gap: sp.md }}>
            <PopupMenuGroup label="Framing">
              <PopupMenuItem
                id="vpFrameSel"
                data-testid="vpFrameSel"
                label="Frame selected"
                description="Center the camera on the current object"
                meta="F"
                disabled={!hasSel}
                disabledReason="Select an object to frame it"
                onSelect={frameSelected}
                onRequestClose={close}
              />
              <PopupMenuItem
                id="vpFrameAll"
                data-testid="vpFrameAll"
                label="Frame all"
                description="Fit every visible object in the viewport"
                onSelect={() => client.frameAll()}
                onRequestClose={close}
              />
            </PopupMenuGroup>
            <PopupMenuGroup label="Camera">
              <div style={cameraGridStyle}>
                {[
                  ["vpTop", "Top", "top"],
                  ["vpFront", "Front", "front"],
                  ["vpSide", "Side", "side"],
                  ["vpPersp", "Perspective", "persp"],
                ].map(([id, label, presetName]) => (
                  <PopupMenuItem
                    key={id}
                    id={id}
                    data-testid={id}
                    label={label}
                    role="menuitemradio"
                    aria-checked={view === presetName}
                    variant="toggle"
                    active={view === presetName}
                    onSelect={() => preset(presetName)}
                    onRequestClose={close}
                  />
                ))}
                {/* The orientation readout carries the orientation cube's role; a true 3D cube remains a
                    render-fidelity follow-up. */}
                <span
                  id="vpOrient"
                  data-testid="vpOrient"
                  data-view={view}
                  aria-label={`Current camera view: ${viewName}`}
                  style={{
                    gridColumn: "1 / -1",
                    color: color.accent.base,
                    font: font.mono,
                    fontSize: fontSize.micro,
                    padding: `0 ${sp.xs}px`,
                  }}
                >
                  ▣ {viewName}
                </span>
              </div>
            </PopupMenuGroup>
            <PopupMenuGroup label="Rendering">
              <PopupMenuItem
                id="vpRenderProfile"
                data-testid="vpRenderProfile"
                role="menuitemcheckbox"
                aria-checked={renderProfile === "cad"}
                variant="toggle"
                active={renderProfile === "cad"}
                label="Render profile"
                description={renderProfile === "cad"
                  ? "Color-faithful materials and technical silhouettes"
                  : "Filmic contrast for game presentation"}
                meta={renderProfile === "cad" ? "CAD" : "Film"}
                onSelect={toggleRenderProfile}
                onRequestClose={close}
              />
            </PopupMenuGroup>
          </div>
        )}
      </MenuPopup>
    </div>
  );
}
