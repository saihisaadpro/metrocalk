//! Shape Studio — the Build engine's creation surface: parametric shapes, draw-to-3D, combine and meld.
//!
//! Implemented in accordance with the Engine UI/UX Architecture Constitution. The contract (ADR-035)
//! shapes every interaction here: a click that creates something ends with the thing placed AND
//! selected plus a toast at the gesture ("· Ctrl-Z to undo"); a refusal is a plain-language sentence
//! shown where the user acted, never a silent no-op; build facts (triangles, milliseconds) are legible
//! on every outcome. All engine work is discrete commands over `EditorClient` — nothing here streams
//! per-frame IPC (invariant 4), and every command is one undoable engine transaction.

import { useEffect, useMemo, useRef, useState } from "react";
import type { CSSProperties, PointerEvent as ReactPointerEvent } from "react";
import { projectionStore, useFieldValue, useMultiSelect, useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button, NumericField } from "../theme/primitives";
import { DisclosureSection } from "../theme/workspace";
import { color, fontSize, radius, space, text } from "../theme/tokens";
import type { ShapeParamSpec, ShapeRecipeSource, ShapeReply, ShapeSpec } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The drawing canvas maps SVG units to metres at 1u = 0.1 m: a 100×70 canvas is a 10 m × 7 m sheet. */
const CANVAS_W = 100;
const CANVAS_H = 70;
const UNIT_M = 0.1;

/** Snap grid pitch: 2.5 canvas units = 0.25 m — fine enough for real work, coarse enough to land
 *  exactly where you aim. Toggleable; freehand strokes are never snapped (it would corrugate them). */
const SNAP_U = 2.5;

/** Draw presets — one click fills the canvas with a shape worth extruding (and gives tests a
 *  deterministic outline). Canvas coordinates. */
const DRAW_PRESETS: { id: string; label: string; points: [number, number][] }[] = [
  {
    id: "star",
    label: "Star",
    points: Array.from({ length: 10 }, (_, i) => {
      const angle = -Math.PI / 2 + (i * Math.PI) / 5;
      const r = i % 2 === 0 ? 28 : 12;
      return [50 + r * Math.cos(angle), 35 + r * Math.sin(angle)] as [number, number];
    }),
  },
  {
    id: "arrow",
    label: "Arrow",
    points: [
      [20, 28], [58, 28], [58, 16], [84, 35], [58, 54], [58, 42], [20, 42],
    ],
  },
  {
    id: "circle",
    label: "Circle",
    points: Array.from({ length: 24 }, (_, i) => {
      const angle = (i * Math.PI * 2) / 24;
      return [50 + 26 * Math.cos(angle), 35 + 26 * Math.sin(angle)] as [number, number];
    }),
  },
  {
    id: "gear",
    label: "Gear",
    // Nine teeth, four outline points per tooth: outer flank · outer flank · root · root.
    points: Array.from({ length: 36 }, (_, i) => {
      const tooth = Math.floor(i / 4);
      const step = (Math.PI * 2) / 9;
      const base = -Math.PI / 2 + tooth * step;
      const w = step / 4;
      const table: [number, number][] = [
        [28, base],
        [28, base + w],
        [20, base + 2 * w],
        [20, base + 3 * w],
      ];
      const [r, angle] = table[i % 4];
      return [50 + r * Math.cos(angle), 35 + r * Math.sin(angle)] as [number, number];
    }),
  },
];

/** Local parameter specs for the drawn kinds (the catalog covers only the parametric shapes). */
const DRAWN_PARAMS: Record<string, ShapeParamSpec[]> = {
  extrude: [
    { key: "height", label: "Height", min: 0.02, max: 50, step: 0.1, default: 1, integer: false, unit: "m" },
    { key: "taper", label: "Taper", min: 0.05, max: 1, step: 0.05, default: 1, integer: false, unit: "" },
  ],
  revolve: [
    { key: "segments", label: "Smoothness", min: 8, max: 128, step: 4, default: 48, integer: true, unit: "" },
  ],
};

/** Ramer–Douglas–Peucker: thin a freehand stroke down to its shape-defining corners so the engine
 *  gets a clean outline instead of hundreds of jittery samples. */
function simplifyStroke(points: [number, number][], eps: number): [number, number][] {
  if (points.length <= 3) return points;
  const keep = new Array<boolean>(points.length).fill(false);
  keep[0] = true;
  keep[points.length - 1] = true;
  const stack: [number, number][] = [[0, points.length - 1]];
  while (stack.length > 0) {
    const [a, b] = stack.pop() as [number, number];
    const [ax, ay] = points[a];
    const [bx, by] = points[b];
    const dx = bx - ax;
    const dy = by - ay;
    const len2 = dx * dx + dy * dy;
    let maxD = 0;
    let maxI = -1;
    for (let i = a + 1; i < b; i++) {
      const [px, py] = points[i];
      let d: number;
      if (len2 === 0) {
        d = Math.hypot(px - ax, py - ay);
      } else {
        const t = Math.max(0, Math.min(1, ((px - ax) * dx + (py - ay) * dy) / len2));
        d = Math.hypot(px - (ax + t * dx), py - (ay + t * dy));
      }
      if (d > maxD) {
        maxD = d;
        maxI = i;
      }
    }
    if (maxD > eps && maxI > 0) {
      keep[maxI] = true;
      stack.push([a, maxI], [maxI, b]);
    }
  }
  return points.filter((_, i) => keep[i]);
}

const COMBINED_KINDS = new Set(["union", "carve", "intersect", "meld"]);

const hintStyle: CSSProperties = {
  fontSize: fontSize.meta,
  color: color.text.muted,
  padding: `${space.xs}px 0`,
};

export function ShapeStudio({ client }: { client: EditorClient }) {
  const [specs, setSpecs] = useState<ShapeSpec[]>([]);
  const [busy, setBusy] = useState(false);
  const [refusal, setRefusal] = useState<string | null>(null);
  // Bumped on every refusal: parameter fields key on it, so a refused value is thrown away and the
  // field resyncs to the document's real value instead of re-submitting the refusal on blur.
  const [refusalNonce, setRefusalNonce] = useState(0);
  const selected = useSelectedId();
  const multi = useMultiSelect();

  useEffect(() => {
    let live = true;
    client
      .shapeCatalog()
      .then((list) => live && setSpecs(list))
      .catch((e: unknown) => console.error("shape_catalog failed", e));
    return () => {
      live = false;
    };
  }, [client]);

  /** Every Shape Studio outcome lands here: place + select on success, an explained refusal otherwise. */
  async function run(action: () => Promise<ShapeReply>, verb: string) {
    setBusy(true);
    setRefusal(null);
    try {
      const reply = await action();
      if (reply.created) {
        projectionStore.getState().select(reply.created);
        void client.gizmoSelect(reply.created).catch((e: unknown) => console.error("gizmoSelect failed", e));
        pushToast(`${reply.message} · Ctrl-Z to undo`, "success");
        setStatus(`${reply.message} (${Math.max(1, Math.round(reply.ms))} ms)`);
        return reply;
      }
      const why = reply.reason ?? "the engine refused without a reason";
      setRefusal(why);
      setRefusalNonce((n) => n + 1);
      pushToast(why, "error");
      setStatus(`${verb} refused: ${why}`);
      return reply;
    } catch (e) {
      console.error(`${verb} failed`, e);
      setRefusal(`${verb} failed — please try again`);
      pushToast(`${verb} failed — please try again`, "error");
      return null;
    } finally {
      setBusy(false);
    }
  }

  return (
    <div data-testid="shape-studio">
      <div className="mtk-dock-section-heading">Shapes</div>
      <div
        role="group"
        aria-label="Create a shape"
        style={{
          display: "grid",
          gridTemplateColumns: "repeat(3, 1fr)",
          gap: space.xs,
          padding: `0 0 ${space.sm}px`,
        }}
      >
        {specs.map((spec) => (
          <Button
            key={spec.kind}
            data-testid={`shape-card-${spec.kind}`}
            variant="secondary"
            disabled={busy}
            title={`${spec.blurb} — lands in the scene, selected, one Ctrl-Z to undo`}
            onClick={() => void run(() => client.shapeSpawn(spec.kind), `Create ${spec.label}`)}
            style={{ flexDirection: "column", gap: 2, paddingTop: space.sm, paddingBottom: space.sm, minHeight: 52 }}
          >
            <Icon name={spec.kind} size="xl" fallback="shape" />
            <span style={{ fontSize: fontSize.meta }}>{spec.label}</span>
          </Button>
        ))}
        {specs.length === 0 && (
          <div style={{ ...hintStyle, gridColumn: "1 / -1" }}>Shape catalog loading…</div>
        )}
      </div>

      <DrawSection client={client} busy={busy} run={run} />
      <CombineSection client={client} busy={busy} run={run} multi={multi} />
      <SelectedShapeSection
        client={client}
        busy={busy}
        run={run}
        selected={selected}
        specs={specs}
        resyncKey={refusalNonce}
      />

      {refusal && (
        <div
          data-testid="shape-refusal"
          role="status"
          style={{
            margin: `${space.sm}px 0`,
            padding: `${space.sm}px ${space.md}px`,
            background: color.danger.bg,
            border: `1px solid ${color.danger.border}`,
            borderRadius: radius.md,
            fontSize: fontSize.meta,
            color: color.danger.text,
          }}
        >
          {refusal}
        </div>
      )}
    </div>
  );
}

/** Draw a flat outline, then raise (extrude) or spin (revolve) it into a real solid. */
function DrawSection({
  client,
  busy,
  run,
}: {
  client: EditorClient;
  busy: boolean;
  run: (action: () => Promise<ShapeReply>, verb: string) => Promise<ShapeReply | null>;
}) {
  const [points, setPoints] = useState<[number, number][]>([]);
  const [mode, setMode] = useState<"extrude" | "revolve">("extrude");
  const [height, setHeight] = useState(1);
  const [segments, setSegments] = useState(48);
  const [taper, setTaper] = useState(1);
  const [snap, setSnap] = useState(true);
  // Pointer interaction state (refs — per-move re-renders come from setPoints alone).
  const drag = useRef<{ kind: "idle" } | { kind: "maybe"; start: [number, number] } | { kind: "stroke" } | { kind: "point"; index: number }>({ kind: "idle" });

  /** Event → canvas coordinates (view units), NaN-guarded for zero-layout environments. */
  function canvasPoint(e: ReactPointerEvent<SVGSVGElement>): [number, number] | null {
    const rect = e.currentTarget.getBoundingClientRect();
    const x = ((e.clientX - rect.left) / Math.max(1, rect.width)) * CANVAS_W;
    const y = ((e.clientY - rect.top) / Math.max(1, rect.height)) * CANVAS_H;
    if (!Number.isFinite(x) || !Number.isFinite(y)) return null;
    return [x, y];
  }

  const snapped = ([x, y]: [number, number]): [number, number] =>
    snap
      ? [Math.round(x / SNAP_U) * SNAP_U, Math.round(y / SNAP_U) * SNAP_U]
      : [Math.round(x * 10) / 10, Math.round(y * 10) / 10];

  /** Three gestures, one surface: press ON a point drags that point (precision editing); press-and-
   *  move on empty canvas draws a FREEHAND stroke (simplified to its corners on release); press-and-
   *  release without moving adds a single snapped point. */
  function onDown(e: ReactPointerEvent<SVGSVGElement>) {
    if (e.button !== 0 || e.isPrimary === false) return;
    const p = canvasPoint(e);
    if (!p) return;
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      /* pointer capture is unavailable under test DOMs */
    }
    const hit = points.findIndex(([x, y]) => Math.hypot(x - p[0], y - p[1]) < 2.2);
    drag.current = hit >= 0 ? { kind: "point", index: hit } : { kind: "maybe", start: p };
  }

  function onMove(e: ReactPointerEvent<SVGSVGElement>) {
    const state = drag.current;
    if (state.kind === "idle") return;
    const p = canvasPoint(e);
    if (!p) return;
    if (state.kind === "point") {
      const moved = snapped(p);
      setPoints((current) => current.map((q, i) => (i === state.index ? moved : q)));
      return;
    }
    if (state.kind === "maybe") {
      if (Math.hypot(p[0] - state.start[0], p[1] - state.start[1]) < 1.8) return;
      drag.current = { kind: "stroke" };
      setPoints((current) => [...current, state.start]);
    }
    // Freehand sampling: unsnapped (snapping corrugates a stroke), thinned by distance.
    setPoints((current) => {
      const last = current[current.length - 1];
      if (last && Math.hypot(p[0] - last[0], p[1] - last[1]) < 1.6) return current;
      return [...current, [Math.round(p[0] * 10) / 10, Math.round(p[1] * 10) / 10]];
    });
  }

  function onUp(e: ReactPointerEvent<SVGSVGElement>) {
    const state = drag.current;
    drag.current = { kind: "idle" };
    if (state.kind === "maybe") {
      // No movement: a plain click adds one snapped point.
      setPoints((current) => [...current, snapped(state.start)]);
    } else if (state.kind === "stroke") {
      // Release: thin the stroke to its shape-defining corners (ε = 1 canvas unit = 10 cm).
      setPoints((current) => simplifyStroke(current, 1.0).slice(0, 128));
    }
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      /* symmetric with the capture guard */
    }
  }

  /** Canvas → metres. Extrude: a ground plan around the canvas centre. Revolve: x is the distance
   *  from the axis (the canvas's left edge), y runs upward. */
  const toProfile = (): [number, number][] =>
    mode === "extrude"
      ? points.map(([x, y]) => [(x - CANVAS_W / 2) * UNIT_M, (y - CANVAS_H / 2) * UNIT_M])
      : points.map(([x, y]) => [x * UNIT_M, (CANVAS_H - y) * UNIT_M]);

  const canCreate = points.length >= 3;
  const path = points.map(([x, y], i) => `${i === 0 ? "M" : "L"}${x},${y}`).join(" ");

  /** The precision readout: real metres, live, from the same mapping the engine will receive. */
  const dims = useMemo(() => {
    if (points.length === 0) return null;
    const profile =
      mode === "extrude"
        ? points.map(([x, y]) => [(x - CANVAS_W / 2) * UNIT_M, (y - CANVAS_H / 2) * UNIT_M])
        : points.map(([x, y]) => [x * UNIT_M, (CANVAS_H - y) * UNIT_M]);
    const xs = profile.map((p) => p[0]);
    const ys = profile.map((p) => p[1]);
    const w = Math.max(...xs) - Math.min(...xs);
    const h = Math.max(...ys) - Math.min(...ys);
    const fmt = (v: number) => (Math.round(v * 100) / 100).toFixed(2);
    return mode === "extrude"
      ? `${fmt(w)} × ${fmt(h)} m · ${points.length} points`
      : `out to ${fmt(Math.max(...xs))} m · ${fmt(h)} m tall · ${points.length} points`;
  }, [points, mode]);

  async function create() {
    const reply = await run(
      () => client.shapeDraw(mode, toProfile(), height, segments, taper),
      mode === "extrude" ? "Raise drawing" : "Spin drawing",
    );
    if (reply?.created) setPoints([]);
  }

  return (
    <DisclosureSection title="Draw a shape" summary="Sketch an outline, get a solid" defaultOpen storageKey="build-draw">
      <div style={{ display: "grid", gap: space.sm }}>
        <div style={{ display: "flex", gap: space.xs, alignItems: "center", flexWrap: "wrap" }}>
          <Button
            data-testid="draw-mode-extrude"
            variant="toggle"
            compact
            active={mode === "extrude"}
            title="Draw the shape as seen from above; it rises straight up"
            onClick={() => setMode("extrude")}
          >
            <Icon name="extrude" size="sm" /> Raise
          </Button>
          <Button
            data-testid="draw-mode-revolve"
            variant="toggle"
            compact
            active={mode === "revolve"}
            title="Draw a side profile; it spins around the left edge into a round solid"
            onClick={() => setMode("revolve")}
          >
            <Icon name="revolve" size="sm" /> Spin
          </Button>
          <span style={{ flex: 1 }} />
        </div>
        {/* Presets on their own row — sharing the mode row made the last one wrap and dangle. */}
        <div style={{ display: "flex", gap: space.xs, alignItems: "center", flexWrap: "wrap" }}>
          <span style={{ fontSize: fontSize.meta, color: color.text.muted }}>Try:</span>
          {DRAW_PRESETS.map((preset) => (
            <Button
              key={preset.id}
              data-testid={`draw-preset-${preset.id}`}
              variant="ghost"
              compact
              title={`Fill the canvas with a ${preset.label.toLowerCase()} outline`}
              onClick={() => setPoints(preset.points.map((p) => [...p] as [number, number]))}
            >
              {preset.label}
            </Button>
          ))}
        </div>
        <svg
          data-testid="draw-canvas"
          role="application"
          aria-label={`Drawing canvas — click to add points, drag to draw freehand, drag a point to move it (${points.length} so far)`}
          viewBox={`0 0 ${CANVAS_W} ${CANVAS_H}`}
          onPointerDown={onDown}
          onPointerMove={onMove}
          onPointerUp={onUp}
          style={{
            width: "100%",
            aspectRatio: `${CANVAS_W} / ${CANVAS_H}`,
            background: color.bg.inset,
            border: `1px solid ${color.border.default}`,
            borderRadius: radius.md,
            cursor: "crosshair",
            touchAction: "none",
            display: "block",
          }}
        >
          {/* Grid: one line per metre. */}
          {Array.from({ length: 9 }, (_, i) => (i + 1) * 10).map((x) => (
            <line key={`v${x}`} x1={x} y1={0} x2={x} y2={CANVAS_H} stroke={color.border.subtle} strokeWidth={0.3} />
          ))}
          {Array.from({ length: 6 }, (_, i) => (i + 1) * 10).map((y) => (
            <line key={`h${y}`} x1={0} y1={y} x2={CANVAS_W} y2={y} stroke={color.border.subtle} strokeWidth={0.3} />
          ))}
          {mode === "revolve" && (
            /* The spin axis — the drawing revolves around this line. */
            <line x1={0.6} y1={0} x2={0.6} y2={CANVAS_H} stroke={color.accent.base} strokeWidth={0.8} strokeDasharray="2 1.5" />
          )}
          {points.length > 0 && (
            <path
              d={`${path}${points.length >= 3 ? " Z" : ""}`}
              fill={points.length >= 3 ? color.accent.subtle : "none"}
              stroke={color.accent.base}
              strokeWidth={0.7}
              strokeLinejoin="round"
            />
          )}
          {points.map(([x, y], i) => (
            <circle key={i} cx={x} cy={y} r={1.1} fill={color.accent.base} />
          ))}
        </svg>
        {/* The precision readout: what the engine will actually receive, in metres, live. */}
        <div
          data-testid="draw-dims"
          style={{ display: "flex", gap: space.sm, alignItems: "center", fontSize: fontSize.meta, color: color.text.muted, minHeight: 18 }}
        >
          <span style={{ flex: 1 }}>{dims ?? "Click to place points · drag to draw freehand"}</span>
          <Button
            data-testid="draw-snap"
            variant="toggle"
            compact
            active={snap}
            title="Snap placed and dragged points to a 0.25 m grid (freehand strokes stay free)"
            onClick={() => setSnap((s) => !s)}
          >
            <Icon name="grid" size="sm" /> Snap
          </Button>
        </div>
        <div style={{ display: "flex", gap: space.sm, alignItems: "center", flexWrap: "wrap" }}>
          {mode === "extrude" ? (
            <>
              <label style={{ display: "flex", gap: space.xs, alignItems: "center", fontSize: fontSize.meta, color: color.text.secondary }}>
                Height
                <NumericField
                  data-testid="draw-height"
                  ariaLabel="Extrude height in metres"
                  value={height}
                  min={0.02}
                  max={50}
                  step={0.1}
                  onCommit={setHeight}
                  style={{ width: 64 }}
                />
                <span style={{ color: color.text.muted }}>m</span>
              </label>
              <label
                title="1 keeps the walls straight; smaller pinches the top toward the centre"
                style={{ display: "flex", gap: space.xs, alignItems: "center", fontSize: fontSize.meta, color: color.text.secondary }}
              >
                Taper
                <NumericField
                  data-testid="draw-taper"
                  ariaLabel="Extrude taper (1 = straight walls)"
                  title="1 keeps the walls straight; smaller pinches the top toward the centre"
                  value={taper}
                  min={0.05}
                  max={1}
                  step={0.05}
                  onCommit={setTaper}
                  style={{ width: 56 }}
                />
              </label>
            </>
          ) : (
            <label style={{ display: "flex", gap: space.xs, alignItems: "center", fontSize: fontSize.meta, color: color.text.secondary }}>
              Smoothness
              <NumericField
                data-testid="draw-segments"
                ariaLabel="Revolve smoothness (segments)"
                value={segments}
                integer
                min={8}
                max={128}
                step={4}
                onCommit={setSegments}
                style={{ width: 64 }}
              />
            </label>
          )}
          <span style={{ flex: 1 }} />
          <Button
            data-testid="draw-undo"
            variant="ghost"
            compact
            disabled={points.length === 0}
            title={points.length === 0 ? "No points yet" : "Remove the last point"}
            onClick={() => setPoints((current) => current.slice(0, -1))}
          >
            Undo point
          </Button>
          <Button
            data-testid="draw-clear"
            variant="ghost"
            compact
            disabled={points.length === 0}
            title={points.length === 0 ? "No points yet" : "Clear the drawing"}
            onClick={() => setPoints([])}
          >
            Clear
          </Button>
          <Button
            data-testid="draw-create"
            variant="primary"
            compact
            disabled={busy || !canCreate}
            title={canCreate ? "Turn the outline into a solid — one Ctrl-Z to undo" : "Draw at least three points first"}
            onClick={() => void create()}
          >
            <Icon name={mode === "extrude" ? "extrude" : "revolve"} size="sm" />{mode === "extrude" ? "Raise it" : "Spin it"}
          </Button>
        </div>
      </div>
    </DisclosureSection>
  );
}

/** Combine exactly two selected objects: exact booleans, or the smooth SDF meld. */
function CombineSection({
  client,
  busy,
  run,
  multi,
}: {
  client: EditorClient;
  busy: boolean;
  run: (action: () => Promise<ShapeReply>, verb: string) => Promise<ShapeReply | null>;
  multi: string[];
}) {
  const [blend, setBlend] = useState(0.25);
  const two = multi.length === 2;
  const [a, b] = two ? multi : [null, null];
  const why = two ? undefined : "Select two objects first — click one, then Ctrl-click another";

  return (
    <DisclosureSection title="Combine" summary="Two objects become one" defaultOpen storageKey="build-combine">
      {!two && <div data-testid="combine-hint" style={hintStyle}>{why}</div>}
      <div style={{ display: "flex", gap: space.xs, flexWrap: "wrap", alignItems: "center" }}>
        <Button
          data-testid="combine-union"
          variant="secondary"
          compact
          disabled={busy || !two}
          title={why ?? "Join the two objects into one solid — one Ctrl-Z brings both back"}
          onClick={() => a && b && void run(() => client.shapeCombine(a, b, "union"), "Join")}
        >
          <Icon name="union" size="sm" /> Join
        </Button>
        <Button
          data-testid="combine-carve"
          variant="secondary"
          compact
          disabled={busy || !two}
          title={why ?? "Carve the second object out of the first"}
          onClick={() => a && b && void run(() => client.shapeCombine(a, b, "carve"), "Carve")}
        >
          <Icon name="subtract" size="sm" /> Carve
        </Button>
        <Button
          data-testid="combine-intersect"
          variant="secondary"
          compact
          disabled={busy || !two}
          title={why ?? "Keep only where the two objects overlap"}
          onClick={() => a && b && void run(() => client.shapeCombine(a, b, "intersect"), "Overlap")}
        >
          <Icon name="intersect" size="sm" /> Overlap
        </Button>
        <span style={{ flex: 1 }} />
        <Button
          data-testid="combine-meld"
          variant="primary"
          compact
          disabled={busy || !two}
          title={why ?? "Melt the two shapes together with a smooth blend (spheres, boxes and cylinders)"}
          onClick={() => a && b && void run(() => client.shapeMeld(a, b, blend), "Meld")}
        >
          <Icon name="meld" size="sm" /> Meld
        </Button>
        <label
          title={why ?? "How far the meld melts the two shapes into each other"}
          style={{ display: "flex", gap: space.xs, alignItems: "center", fontSize: fontSize.meta, color: color.text.secondary }}
        >
          Blend
          <NumericField
            data-testid="meld-blend"
            ariaLabel="Meld blend radius in metres"
            title={why ?? "How far the meld melts the two shapes into each other"}
            value={blend}
            min={0.05}
            max={2}
            step={0.05}
            onCommit={setBlend}
            disabled={busy || !two}
            style={{ width: 56 }}
          />
          <span style={{ color: color.text.muted }}>m</span>
        </label>
      </div>
    </DisclosureSection>
  );
}

/** Live parameters of the selected shape — each commit is one engine re-bake, one Ctrl-Z. */
function SelectedShapeSection({
  client,
  busy,
  run,
  selected,
  specs,
  resyncKey,
}: {
  client: EditorClient;
  busy: boolean;
  run: (action: () => Promise<ShapeReply>, verb: string) => Promise<ShapeReply | null>;
  selected: string | null;
  specs: ShapeSpec[];
  /** Bumped on refusal — remounts the fields so a refused value never lingers or re-submits. */
  resyncKey: number;
}) {
  const sourceValue = useFieldValue(selected ?? "", "ShapeRecipe", "source");
  const recipe = useMemo<ShapeRecipeSource | null>(() => {
    if (typeof sourceValue !== "string" || sourceValue.length === 0) return null;
    try {
      return JSON.parse(sourceValue) as ShapeRecipeSource;
    } catch {
      return null;
    }
  }, [sourceValue]);

  if (!selected || !recipe) return null;

  const params: ShapeParamSpec[] =
    specs.find((spec) => spec.kind === recipe.kind)?.params ?? DRAWN_PARAMS[recipe.kind] ?? [];

  return (
    <DisclosureSection title="Selected shape" summary="Live parameters — edits re-bake in place" defaultOpen storageKey="build-selected-shape">
      <div data-testid="shape-params" style={{ display: "grid", gap: space.xs }}>
        <div style={{ ...text.eyebrow, color: color.text.muted }}>{recipe.kind}</div>
        {COMBINED_KINDS.has(recipe.kind) ? (
          <div style={hintStyle}>
            A combined shape has no editable parameters — press Ctrl-Z to get its source shapes back.
          </div>
        ) : (
          params.map((param) => {
            const current = recipe.params?.[param.key] ?? param.default;
            return (
              <label
                key={`${param.key}:${resyncKey}`}
                style={{
                  display: "flex",
                  gap: space.sm,
                  alignItems: "center",
                  justifyContent: "space-between",
                  fontSize: fontSize.meta,
                  color: color.text.secondary,
                }}
              >
                {param.label}
                <span style={{ display: "inline-flex", gap: space.xs, alignItems: "center" }}>
                  <NumericField
                    data-testid={`shape-param-${param.key}`}
                    ariaLabel={`${param.label} of the selected shape`}
                    value={current}
                    min={param.min}
                    max={param.max}
                    step={param.step}
                    integer={param.integer}
                    disabled={busy}
                    onCommit={(v) => {
                      // An unchanged commit (focus + blur, or a scrub back to the start) must not
                      // burn an undo step on a re-bake of identical geometry.
                      if (v === current) return;
                      void run(() => client.shapeUpdate(selected, { [param.key]: v }), `Set ${param.label}`);
                    }}
                    style={{ width: 72 }}
                  />
                  {param.unit && <span style={{ color: color.text.muted }}>{param.unit}</span>}
                </span>
              </label>
            );
          })
        )}
      </div>
    </DisclosureSection>
  );
}

export default ShapeStudio;
