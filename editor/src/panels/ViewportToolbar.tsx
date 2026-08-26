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
import { useStore } from "zustand";
import { cameraStore } from "../store/cameras";
import { useProjectSessionId } from "../store/project";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Icon } from "../theme/icons";
import { MenuPopup, PopupMenuGroup, PopupMenuItem } from "../theme/workspace";
import { color, elevation, font, fontSize, radius, space as sp, z } from "../theme/tokens";
import {
  focalLengthMm,
  freeLook,
  lookThrough,
  refreshCameras,
  saveCurrentView,
} from "./cameraActions";
import type { EditorClient, ViewportRenderProfile } from "../transport/session";

type Mode = "translate" | "rotate" | "scale";

/** How many saved cameras this menu lists before it says how many it is not listing.
 *
 *  A menu is a short list you scan, not a browser: the Cameras group already pushed the Rendering
 *  group below the popover's fold on an 800px window, and an uncapped list would push it off a
 *  taller one too. The cap is stated on screen rather than applied silently — a list that quietly
 *  stops is indistinguishable from a scene that only has six cameras. */
const MENU_CAMERAS = 6;

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
  // Read from the shared store rather than a local list: the Inspector's camera section edits the same
  // cameras, and two lists refreshed on two schedules is two answers to "which one am I inside".
  const cameras = useStore(cameraStore, (s) => s.cameras);
  const lookingThroughId = useStore(cameraStore, (s) => s.lookingThroughId);
  const projectSession = useProjectSessionId();
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

  // The camera list is read ONCE on mount, not on the 500 ms chrome poll: authored cameras change only
  // when someone authors one, and every gesture that changes them refreshes the store itself
  // (`cameraActions`). Polling a document read twice a second to watch for an edit only this UI can make
  // is the per-frame-IPC habit at a slower clock.
  useEffect(() => {
    void refreshCameras(client);
  }, [client, projectSession]);

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
            {/* ORIENTATION, not "Camera" — these are the editor view presets, and the readout below
                them stands in for an orientation cube. It was called Camera until a group that is
                really about cameras landed directly beneath it, and two adjacent headings one letter
                apart meaning different things is worse than either name alone. */}
            <PopupMenuGroup label="Orientation">
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
                  <Icon name="view" size="sm" /> {viewName}
                </span>
              </div>
            </PopupMenuGroup>
            {/* SAVED CAMERAS. Creating one lives here, at the viewport, because "save this view" is a
                fact about where you are standing — the Inspector's camera section owns everything you
                can do to a camera you have selected, and both call the same `cameraActions`.
                The rows are `menuitemradio` because they are exclusive: you are looking through one
                camera or through none, and Free look is the "none" option rather than a fourth verb. */}
            <PopupMenuGroup label="Cameras">
              <PopupMenuItem
                id="vpSaveCamera"
                data-testid="vpSaveCamera"
                label="Save this view as a camera"
                description="Keeps where you are standing and what you are looking at, so you can come back to it"
                onSelect={() => {
                  void saveCurrentView(client).then((created) => {
                    // Select it, so the Inspector's camera section is already open on the thing that
                    // was just made — the loop closed at the gesture rather than leaving the author to
                    // go and find their new camera in the hierarchy.
                    if (created) projectionStore.getState().select(created.id);
                  });
                }}
                onRequestClose={close}
              />
              {cameras.length === 0 ? (
                <PopupMenuItem
                  id="vpNoCameras"
                  data-testid="vpNoCameras"
                  label="No cameras yet"
                  description="Frame a view you like, then save it here"
                  disabled
                  disabledReason="This scene has no saved cameras"
                  onSelect={() => undefined}
                />
              ) : (
                cameras.slice(0, MENU_CAMERAS).map((camera) => (
                  <PopupMenuItem
                    key={camera.id}
                    id={`vpCamera-${camera.id}`}
                    data-testid="vpCamera"
                    label={camera.name}
                    description={
                      camera.lookAt
                        ? `${focalLengthMm(camera.fovDeg)} mm lens${camera.active ? " · Play renders from this one" : ""}`
                        : "Saved before cameras could aim — it follows the editor's view"
                    }
                    meta={lookingThroughId === camera.id ? "Looking through" : undefined}
                    role="menuitemradio"
                    aria-checked={lookingThroughId === camera.id}
                    variant="toggle"
                    active={lookingThroughId === camera.id}
                    onSelect={() => {
                      projectionStore.getState().select(camera.id);
                      void lookThrough(client, camera);
                    }}
                    onRequestClose={close}
                  />
                ))
              )}
              {cameras.length > MENU_CAMERAS && (
                <PopupMenuItem
                  id="vpMoreCameras"
                  data-testid="vpMoreCameras"
                  label={`${cameras.length - MENU_CAMERAS} more not shown here`}
                  description="Pick any of them in the outliner to look through it"
                  disabled
                  disabledReason="This menu lists the most recent cameras; the outliner lists them all"
                  onSelect={() => undefined}
                />
              )}
              {lookingThroughId !== null && (
                <PopupMenuItem
                  id="vpFreeLook"
                  data-testid="vpFreeLook"
                  label="Free look"
                  description="Back to the editor's own camera"
                  role="menuitemradio"
                  aria-checked={false}
                  onSelect={() => void freeLook(client)}
                  onRequestClose={close}
                />
              )}
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
