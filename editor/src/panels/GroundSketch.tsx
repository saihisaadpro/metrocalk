//! Ground Sketch — draw an outline **on the ground, in the viewport, at world scale**, then raise it.
//!
//! Implemented in accordance with the Engine UI/UX Architecture Constitution. The stage is the drawing
//! surface; this panel is the *precision* half beside it — what the outline measures, what it snaps to,
//! and the two numbers (a typed length, a height) that turn a rough gesture into an exact solid.
//!
//! WHY THE NUMBERS LIVE HERE AND THE LINE LIVES THERE. The rubber band, the snap marker and the
//! closing edge follow the cursor at frame rate, natively, on the render thread — a JS mouse-move
//! would put an IPC round trip between the hand and the line (invariant 4). What crosses the boundary
//! is a click, and a 10 Hz poll of the read-model so the metres beside the drawing stay honest. That
//! is a readout, not the hot path.
//!
//! Every command answers with the WHOLE read-model, so a click closes its own loop (`<ux_quality>` 1):
//! nothing here asks a second time what it just did, and no control reports a result the engine did
//! not send.

import { useEffect, useRef, useState } from "react";
import type { CSSProperties } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Badge, Button, NumericField, SelectField } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, text, z } from "../theme/tokens";
import type { GroundSketchState } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** Snap pitches offered, metres. `0` is freehand — named "Off" rather than "0 m", which reads as a
 *  broken value. Chosen to span the jobs: a doorway, a room, a building, a city block. */
const GRIDS: { m: number; label: string }[] = [
  { m: 0, label: "Off" },
  { m: 0.1, label: "10 cm" },
  { m: 0.25, label: "25 cm" },
  { m: 0.5, label: "50 cm" },
  { m: 1, label: "1 m" },
  { m: 5, label: "5 m" },
];

/** How often the readout re-reads the outline while the tool is armed. Fast enough that the metres
 *  feel live, slow enough that it is nowhere near the per-frame path the invariant protects. */
const POLL_MS = 100;

const fmt = (v: number) => (Number.isFinite(v) ? v.toFixed(2) : "0.00");

export interface GroundSketchProps {
  client: EditorClient;
  state: GroundSketchState | null;
  onState: (state: GroundSketchState) => void;
  /** Raised while a command is in flight, so the stage can ignore clicks that would race it. */
  onPendingChange?: (pending: boolean) => void;
  style?: CSSProperties;
}

export function GroundSketch({ client, state, onState, onPendingChange, style }: GroundSketchProps) {
  const [height, setHeight] = useState(3);
  const [length, setLength] = useState(4);
  const [busy, setBusy] = useState(false);
  const pendingSignal = useRef(onPendingChange);
  pendingSignal.current = onPendingChange;

  // The live readout. Only while armed, and cleared on unmount — a poll that outlives its panel is a
  // background IPC nobody asked for.
  useEffect(() => {
    let live = true;
    const timer = window.setInterval(() => {
      client
        .sketchState()
        .then((s) => live && onState(s))
        .catch(() => {});
    }, POLL_MS);
    return () => {
      live = false;
      window.clearInterval(timer);
    };
  }, [client, onState]);

  async function run<T>(action: () => Promise<T>, apply: (result: T) => void, verb: string) {
    setBusy(true);
    pendingSignal.current?.(true);
    try {
      apply(await action());
    } catch (e) {
      console.error(`${verb} failed`, e);
      pushToast(`${verb} failed — please try again`, "error");
    } finally {
      setBusy(false);
      pendingSignal.current?.(false);
    }
  }

  async function raise() {
    await run(
      () => client.sketchCommit(height),
      (reply) => {
        if (reply.created) {
          projectionStore.getState().select(reply.created);
          void client.gizmoSelect(reply.created).catch((e: unknown) => console.error("gizmoSelect failed", e));
          pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
          setStatus(`${reply.message} (${Math.max(1, Math.round(reply.ms))} ms)`);
          void client.sketchState().then(onState).catch(() => {});
        } else {
          const why = reply.reason ?? "the engine refused without a reason";
          pushToast(why, "error");
          setStatus(`Raise refused: ${why}`);
        }
      },
      "Raise outline",
    );
  }

  const corners = state?.points.length ?? 0;
  const canBuild = state?.canBuild === true;
  const aiming = state?.cursor != null;
  // The refusal sentence a disabled control shows. Stated once so the height field, the Raise button
  // and the typed-length button cannot disagree about why they are off.
  //
  // EVERY `title` BELOW IS CONDITIONAL, AND THAT IS NOT A STYLE CHOICE. `Button` resolves
  // `title ?? (refusing && disabledReason)` — a caller who wrote a title meant it — so an
  // UNCONDITIONAL title permanently hides the reason, and the control goes dark saying only what it
  // would have done if it worked. That is the exact trap the terrain panel shipped eleven times
  // (`progress/terrain-way-in`); the first version of this panel shipped it four more.
  const cannotRaise = canBuild
    ? undefined
    : corners === 0
      ? "Click on the ground to place the first corner"
      : `An outline needs three corners that enclose something — ${corners} so far`;

  return (
    <section
      data-testid="ground-sketch"
      aria-label="Ground sketch"
      onPointerDown={(e) => e.stopPropagation()}
      style={{
        position: "absolute",
        top: space.xxl + space.xl + space.xs,
        left: space.sm,
        zIndex: z.chrome,
        width: 288,
        maxWidth: "calc(100% - 16px)",
        maxHeight: "calc(100% - 104px)",
        overflowX: "hidden",
        overflowY: "auto",
        overscrollBehavior: "contain",
        pointerEvents: "auto",
        color: color.text.primary,
        background: color.bg.raised,
        border: `1px solid ${corners > 0 ? color.accent.border : color.border.default}`,
        borderRadius: radius.xl,
        boxShadow: elevation.e2,
        font: font.ui,
        fontSize: fontSize.body,
        ...style,
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
          gap: space.sm,
          padding: `${space.sm}px ${space.lg}px`,
          background: color.bg.raised,
          borderBottom: `1px solid ${color.border.subtle}`,
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: space.sm }}>
          <Icon name="draw" size="lg" style={{ color: color.accent.base }} />
          <span style={text.panelTitle}>Draw on the ground</span>
        </div>
        <Badge tone={state?.closed ? "success" : corners > 0 ? "accent" : "neutral"}>
          {state?.closed ? "Closed" : corners > 0 ? "Drawing" : "Ready"}
        </Badge>
      </header>

      <div style={{ padding: space.lg, display: "grid", gap: space.md }}>
        {/* The measurement, in the size a measurement deserves. This is the thing the author is
            actually watching while they draw, so it is the largest thing in the panel. */}
        <div
          data-testid="ground-sketch-dims"
          role="status"
          aria-live="polite"
          style={{
            padding: `${space.sm}px ${space.md}px`,
            borderRadius: radius.lg,
            background: color.bg.inset,
            border: `1px solid ${color.border.subtle}`,
          }}
        >
          <div style={{ font: font.mono, fontSize: fontSize.display, color: color.text.primary }}>
            {fmt(state?.widthM ?? 0)} × {fmt(state?.depthM ?? 0)} m
          </div>
          <div style={{ fontSize: fontSize.meta, color: color.text.muted, marginTop: space.xxs }}>
            {corners} {corners === 1 ? "corner" : "corners"} · {fmt(state?.areaM2 ?? 0)} m² ·{" "}
            {fmt(state?.perimeterM ?? 0)} m around
          </div>
          <div
            data-testid="ground-sketch-live"
            style={{ fontSize: fontSize.meta, color: aiming ? color.accent.base : color.text.muted, marginTop: space.xxs }}
          >
            {aiming
              ? `next ${fmt(state?.segmentM ?? 0)} m · ${state?.snap ?? ""}`
              : "point at the ground to aim"}
          </div>
        </div>

        {/* Precision, and it is two controls rather than a settings page: what the corners land on,
            and whether the angles between them are square. */}
        <div style={{ display: "grid", gap: space.xs }}>
          <label
            htmlFor="ground-sketch-grid"
            style={{ fontSize: fontSize.meta, color: color.text.secondary, display: "flex", alignItems: "center", gap: space.xs }}
          >
            <Icon name="grid" size="sm" /> Corners land on
          </label>
          <SelectField
            id="ground-sketch-grid"
            data-testid="ground-sketch-grid"
            aria-label="What corners land on"
            value={String(state?.gridM ?? 0.25)}
            disabled={busy}
            title="The spacing every corner snaps to. Off draws freehand."
            onChange={(e) =>
              void run(
                () => client.sketchTool(true, Number(e.target.value), state?.angleSnap ?? true),
                onState,
                "Change the grid",
              )
            }
          >
            {GRIDS.map((g) => (
              <option key={g.m} value={String(g.m)}>
                {g.label}
              </option>
            ))}
          </SelectField>
          <Button
            data-testid="ground-sketch-angle"
            variant="toggle"
            compact
            active={state?.angleSnap === true}
            aria-pressed={state?.angleSnap === true}
            disabled={busy}
            title="Straighten a wall onto the nearest 15° when the cursor is close to it — the fastest way to a square corner"
            onClick={() =>
              void run(
                () => client.sketchTool(true, state?.gridM, !(state?.angleSnap ?? true)),
                onState,
                "Toggle square corners",
              )
            }
          >
            <Icon name="snap" size="sm" /> Straight angles
          </Button>
        </div>

        {/* The typed dimension. Aim, type, and the corner lands at exactly that distance — the half of
            precision drawing a mouse cannot do. */}
        <div style={{ display: "grid", gap: space.xs }}>
          <label htmlFor="ground-sketch-length" style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
            Exact length from the last corner
          </label>
          <div style={{ display: "flex", gap: space.xs, alignItems: "center" }}>
            <NumericField
              id="ground-sketch-length"
              data-testid="ground-sketch-length"
              value={length}
              min={0.01}
              max={5000}
              step={0.25}
              disabled={busy}
              ariaLabel="Exact length in metres"
              onCommit={setLength}
              style={{ flex: 1 }}
            />
            <Button
              data-testid="ground-sketch-place"
              variant="secondary"
              compact
              disabled={busy || corners === 0}
              disabledReason={corners === 0 ? "Click the first corner first — a length is measured from it" : undefined}
              title={corners === 0 ? undefined : "Place the next corner exactly this far away, in the direction you are aiming"}
              onClick={() => void run(() => client.sketchPointExact(length), onState, "Place a measured corner")}
            >
              Place
            </Button>
          </div>
        </div>

        {/* Raising it: the height, and the one button that turns the drawing into a thing. */}
        <div style={{ display: "grid", gap: space.xs }}>
          <label htmlFor="ground-sketch-height" style={{ fontSize: fontSize.meta, color: color.text.secondary }}>
            Height
          </label>
          <NumericField
            id="ground-sketch-height"
            data-testid="ground-sketch-height"
            value={height}
            min={0.02}
            max={50}
            step={0.25}
            disabled={busy}
            ariaLabel="Height in metres"
            onCommit={setHeight}
          />
          <Button
            data-testid="ground-sketch-raise"
            variant="primary"
            disabled={busy || !canBuild}
            disabledReason={cannotRaise}
            title={canBuild ? "Raise the outline into a solid, standing where you drew it — one Ctrl-Z to undo" : undefined}
            onClick={() => void raise()}
            style={{ width: "100%" }}
          >
            <Icon name="extrude" size="sm" /> Raise into a solid
          </Button>
        </div>

        <div style={{ display: "flex", gap: space.xs }}>
          <Button
            data-testid="ground-sketch-undo"
            variant="ghost"
            compact
            disabled={busy || (corners === 0 && !state?.closed)}
            disabledReason={corners === 0 && !state?.closed ? "There is no corner to take back" : undefined}
            title={corners === 0 && !state?.closed ? undefined : "Take back the last corner"}
            onClick={() => void run(() => client.sketchUndo(), onState, "Undo a corner")}
            style={{ flex: 1 }}
          >
            <Icon name="undo" size="sm" /> Last corner
          </Button>
          <Button
            data-testid="ground-sketch-clear"
            variant="ghost"
            compact
            disabled={busy || corners === 0}
            disabledReason={corners === 0 ? "There is nothing drawn to clear" : undefined}
            title={corners === 0 ? undefined : "Throw the whole outline away and start again"}
            onClick={() => void run(() => client.sketchClear(), onState, "Clear the outline")}
            style={{ flex: 1 }}
          >
            <Icon name="close" size="sm" /> Start over
          </Button>
        </div>

        <div
          data-testid="ground-sketch-message"
          role="status"
          aria-live="polite"
          style={{ fontSize: fontSize.meta, color: color.text.muted }}
        >
          {state?.message ?? "Click on the ground to place the first corner."}
        </div>
      </div>
    </section>
  );
}

export default GroundSketch;
