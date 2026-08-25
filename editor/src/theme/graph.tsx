//! Shared graph visual contract — **the one graph framework** the constitution asks for.
//!
//! WHY THIS FILE GREW A UI. Its predecessor owned three functions returning inline styles, and the
//! constitution's Graph Editors section asks for something much larger: *"Every graph editor should
//! inherit from one graph framework. Shared: zoom, pan, selection, marquee, alignment, colour rules,
//! connection rules, search, mini map, navigation. No subsystem creates its own graph behaviour."*
//! What actually existed was one editor (`AnimationGraphEditor`) that implemented all of that by hand
//! in 1,551 lines, and two — `BindingGraph` and `StateGraph` — that implemented **none** of it and
//! handed React Flow a `data.label`, letting the vendored stylesheet draw a centred sentence in a
//! stock white box. Neither had ever appeared in a capture, so the gap had been invisible since the
//! day it was written down.
//!
//! So the split is: **this module owns the canvas, the node card, the port, the edge and the chrome**
//! (zoom · fit · mini map · legend · search slot · empty state); **domain adapters own topology and
//! transactions**. A subsystem that wants a graph brings nodes, edges and a layout intent — never a
//! `<ReactFlow>` of its own, and never a colour.
//!
//! POSITIONS ARE PART OF THE DESIGN, NOT AN AFTERTHOUGHT. Both thin editors positioned their nodes
//! with `(i % 6) * 150` — arithmetic over an arbitrary index, which put all six nodes of a real bind
//! neighbourhood in one row with the selected one adrift below it, five edges crossing through four
//! other nodes, and the whole thing overflowing its own pane by 98px after `fitView`. A picture whose
//! layout carries no meaning is not "the relationships, immediately understandable"; it is a pile.
//! [`columnLayout`] and [`rankByDistance`] are here so a graph's shape states its semantics — what
//! feeds this, what this feeds — and so no subsystem invents a third way to place a node.

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type ReactNode,
  type RefObject,
} from "react";
import {
  Background,
  BackgroundVariant,
  BaseEdge,
  EdgeLabelRenderer,
  Handle,
  MiniMap,
  Position,
  ReactFlow,
  ReactFlowProvider,
  getBezierPath,
  useReactFlow,
  type Edge as RfEdge,
  type EdgeProps,
  type Node as RfNode,
  type NodeChange,
  type NodeProps,
  type ReactFlowProps,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { color, elevation, font, fontSize, lineHeight, radius, space, text } from "./tokens";
import { TypeIcon } from "./primitives";

export type GraphNodeEmphasis = "default" | "selected" | "initial" | "live" | "blocked";
export type GraphEdgeState = "default" | "selected" | "confirmed" | "pending" | "rejected" | "active" | "disabled";

export const graphTheme = {
  canvas: color.bg.canvas,
  grid: color.border.default,
  node: color.bg.raised,
  nodeText: color.text.primary,
  edge: color.text.muted,
  selection: color.accent.base,
  initial: color.info.text,
  live: color.success.text,
  warning: color.warn.text,
  danger: color.danger.text,
} as const;

/** The emphasis ring, as ONE table rather than a nested ternary per property.
 *
 *  Selection used to be a `2px solid` border, which is the heaviest treatment in the whole editor
 *  applied to its most common state — and it reflows the card, because a border occupies layout while
 *  the 1px unselected one does not, so selecting a node moved it. A ring is a `box-shadow`: it costs
 *  no layout, it reads as a soft halo rather than an outline, and it is the same language `:focus-vis`
 *  ible` already speaks everywhere else in this stylesheet. */
const EMPHASIS: Record<GraphNodeEmphasis, { border: string; ring: string; chip?: string }> = {
  default: { border: color.border.subtle, ring: "none" },
  selected: { border: color.accent.border, ring: `0 0 0 3px ${color.accent.subtle}` },
  initial: { border: color.info.border, ring: `0 0 0 3px ${color.info.bg}`, chip: "start" },
  live: { border: color.success.border, ring: `0 0 0 3px ${color.success.bg}`, chip: "live" },
  blocked: { border: color.danger.border, ring: `0 0 0 3px ${color.danger.bg}` },
};

/** Kept as the inline-style form for `AnimationGraphEditor`, which draws its own node body and only
 *  wants the shell. New graphs use [`GraphNodeCard`], which is this shell plus the contents. */
export function graphNodeStyle(emphasis: GraphNodeEmphasis = "default"): CSSProperties {
  const e = EMPHASIS[emphasis];
  return {
    minWidth: 116,
    padding: `${space.sm}px ${space.md}px`,
    border: `1px solid ${e.border}`,
    borderRadius: radius.xl,
    background: color.bg.raised,
    color: color.text.primary,
    boxShadow: e.ring === "none" ? elevation.e1 : `${e.ring}, ${elevation.e2}`,
    fontFamily: font.ui,
    fontSize: fontSize.body,
    lineHeight: lineHeight.compact,
    opacity: emphasis === "blocked" ? 0.64 : 1,
  };
}

export function graphEdgeStyle(state: GraphEdgeState = "default"): CSSProperties {
  const stroke =
    state === "confirmed" || state === "active"
      ? color.success.text
      : state === "selected"
        ? color.accent.base
        : state === "pending"
          ? color.warn.text
          : state === "rejected"
            ? color.danger.text
            : state === "disabled"
              ? color.text.faint
              : color.text.muted;

  return {
    stroke,
    strokeWidth: state === "active" ? 2 : 1.5,
    strokeDasharray: state === "pending" ? "5 4" : undefined,
    opacity: state === "disabled" ? 0.58 : 1,
  };
}

/** THE RELATION PILL, AND WHY IT IS NOT AN SVG LABEL.
 *
 *  React Flow's built-in `label` draws a `<text>` and an optional `<rect>` INSIDE the edge's own `<g>`,
 *  in the same SVG as every other edge — so paint order is document order, and any edge declared after
 *  a label is drawn ON TOP OF IT. That is not a hypothetical: the door state machine has one back edge
 *  (`Closing → Closed`) that spans the whole graph horizontally, and it drew a line straight through
 *  the middle of `open_requested`, three columns away, striking the word out. An opaque background
 *  fixes label-against-label and does nothing at all for label-against-wire.
 *
 *  `EdgeLabelRenderer` portals into `.react-flow__edgelabel-renderer`, a div React Flow mounts AFTER
 *  the edges' `<svg>` and BEFORE the nodes — which is exactly the right z-order for this object: above
 *  every wire, below every card. It is also HTML, so the pill is styled by the stylesheet with the
 *  rest of the design system instead of by six SVG presentation props, and it inherits the same
 *  `is-dimmed` fade its edge does when a path is being traced. */
function GraphRelationEdge({
  id,
  sourceX,
  sourceY,
  sourcePosition,
  targetX,
  targetY,
  targetPosition,
  style,
  label,
  data,
}: EdgeProps) {
  const [path, labelX, labelY] = getBezierPath({
    sourceX,
    sourceY,
    sourcePosition,
    targetX,
    targetY,
    targetPosition,
  });
  return (
    <>
      <BaseEdge id={id} path={path} style={style} />
      {label != null && (
        <EdgeLabelRenderer>
          <div
            // The pill lives in a DIFFERENT DOM layer from its wire, so the `is-dimmed` class the
            // tracing view puts on the edge cannot reach it — a faded wire with a full-strength label
            // still shouting its relation is worse than no fade at all. The flag travels in the edge's
            // own `data`, which is the only channel the two halves share.
            className={data?.dimmed ? "mtk-graph-edge-label is-dimmed" : "mtk-graph-edge-label"}
            // The pill is `pointer-events: none` in the stylesheet: HTML over the canvas otherwise
            // swallows the drag that pans the graph, and a canvas with dead patches on it is a worse
            // canvas than one with no labels.
            data-edge={id}
            style={{
              transform: `translate(-50%, -50%) translate(${labelX}px, ${labelY}px)`,
              color: style?.stroke,
            }}
          >
            {label}
          </div>
        </EdgeLabelRenderer>
      )}
    </>
  );
}

/** One edge type, registered once — the same argument as `GRAPH_NODE_TYPES`. */
export const GRAPH_EDGE_TYPES = { mtkRelation: GraphRelationEdge } as const;

export interface GraphEdgeSpec {
  id: string;
  source: string;
  target: string;
  state?: GraphEdgeState;
  /** The relation this wire carries. Omitted draws an unlabelled wire — which is the right answer for
   *  a fan of edges that all say the same thing (see `BindingGraph`'s `LABEL_FAN_MAX`). */
  label?: string;
  /** Marching ants. Reserved for something genuinely IN FLIGHT — a bind the core has not answered yet
   *  — never for "this one is important", because motion on a static diagram is the "never
   *  distracting" clause of the constitution being broken. */
  animated?: boolean;
}

/** The one place a graph edge is built. A subsystem that assembled its own object would be free to
 *  forget the type, the style or the dim class, which is how the two thin editors ended up with
 *  React Flow's vendored appearance in the first place. */
export function graphEdge({ id, source, target, state = "default", label, animated }: GraphEdgeSpec): RfEdge {
  return {
    id,
    source,
    target,
    type: "mtkRelation",
    label,
    animated: animated ?? false,
    style: graphEdgeStyle(state),
  };
}

// ── layout ────────────────────────────────────────────────────────────────────────────────────────

/** The card's width, in ONE place, because two things need it and they must not disagree.
 *
 *  A state name is one word and a CAD part name is nine, so one width cannot serve both: 208px
 *  holding "Locked" is a column of whitespace, and 148px holding "Hydraulic Power Unit — Skid
 *  Mounted, 210 bar" is four clamped lines. Two sizes, one card — never a second card component.
 *
 *  It lives here rather than in `global.css` because [`columnLayout`] has to subtract it from the
 *  column gap to leave room for the relation pill that rides on the edge between two columns. A
 *  width stated once in a stylesheet and once in a layout function is two numbers that drift until
 *  a label lands underneath a card — which is exactly what the first capture of this panel showed:
 *  `ControlSignal` reading `ontrolSigna` with the rest behind the selected node. */
export const GRAPH_CARD_WIDTH = 208;
export const GRAPH_CARD_WIDTH_COMPACT = 148;

/** A one-line card's height, used ONLY for the frame between mount and the browser's first
 *  measurement. It is deliberately an estimate and deliberately not used for anything permanent: the
 *  real height depends on how many lines a title wraps to, which is a question only layout can
 *  answer, and `onNodesChange` replaces this the moment it does. */
export const GRAPH_CARD_HEIGHT_ESTIMATE = 44;

/** How much clear horizontal run an edge needs between two cards for its relation label to sit in.
 *
 *  Measured against the worst real case rather than guessed: `ControlSignal` at 11px mono renders a
 *  ~118px pill, and the first attempt at 132 left 7px of visible edge either side — which is only
 *  fine until two columns sit at the same height and the connection between them is a straight
 *  horizontal line whose entire visible length is then the label. The wire's colour IS its status,
 *  so a wire you cannot see has lost the one thing the legend explains. 168 leaves ~25px of edge on
 *  each side of the widest pill, at every alignment. */
export const GRAPH_LABEL_RUN = 168;

export interface ColumnLayoutOptions {
  /** Horizontal distance between column ORIGINS (cards are positioned by their top-left). The
   *  default is the card plus a clear run for the edge label — the two things that decide whether a
   *  relation pill is readable — rather than a round number that looked right once. */
  columnGap?: number;
  /** Vertical distance between two rows' centres, INCLUDING the card. */
  rowGap?: number;
}

/** Deterministic left-to-right placement: `columns[i]` is one vertical stack, each stack centred on a
 *  shared axis so a one-node column sits level with the middle of a five-node one.
 *
 *  Centring is the part that matters and the part `(i % 6) * 150` cannot express. A graph reads as a
 *  flow when the thing everything points at is on the axis everything else is arranged around; it
 *  reads as a list when the columns are all top-aligned and the eye has to find the subject. */
export function columnLayout(
  columns: string[][],
  { columnGap = GRAPH_CARD_WIDTH + GRAPH_LABEL_RUN, rowGap = 108 }: ColumnLayoutOptions = {},
): Record<string, { x: number; y: number }> {
  const tallest = Math.max(1, ...columns.map((c) => c.length));
  const positions: Record<string, { x: number; y: number }> = {};
  columns.forEach((column, ci) => {
    const offset = ((tallest - column.length) * rowGap) / 2;
    column.forEach((id, ri) => {
      positions[id] = { x: ci * columnGap, y: offset + ri * rowGap };
    });
  });
  return positions;
}

/** Breadth-first rank from a set of roots over directed edges — the column index for a graph whose
 *  shape is "how far is this from the start", which is every state machine and every dependency
 *  chain. Nodes unreachable from a root are appended in a final column rather than dropped: a node
 *  the layout cannot place is still a node the user authored, and silently omitting it is the class
 *  of defect this whole harness exists to catch. */
export function rankByDistance(
  ids: readonly string[],
  edges: readonly { from: string; to: string }[],
  roots: readonly string[],
): string[][] {
  const out = new Map<string, string[]>();
  for (const e of edges) (out.get(e.from) ?? out.set(e.from, []).get(e.from)!).push(e.to);

  const rank = new Map<string, number>();
  let frontier = roots.filter((r) => ids.includes(r));
  let depth = 0;
  for (const r of frontier) rank.set(r, 0);
  while (frontier.length > 0) {
    depth += 1;
    const next: string[] = [];
    for (const id of frontier) {
      for (const to of out.get(id) ?? []) {
        if (rank.has(to)) continue;
        rank.set(to, depth);
        next.push(to);
      }
    }
    frontier = next;
  }

  const unreached = ids.filter((id) => !rank.has(id));
  const maxRank = Math.max(-1, ...rank.values());
  for (const id of unreached) rank.set(id, maxRank + 1);

  const columns: string[][] = [];
  for (const id of ids) {
    const r = rank.get(id) ?? 0;
    (columns[r] ??= []).push(id);
  }
  return columns.filter((c) => c != null && c.length > 0);
}

// ── the node card ─────────────────────────────────────────────────────────────────────────────────

/** What a graph node SAYS, in the one vocabulary every graph in the engine uses.
 *
 *  A stock React Flow node carries one string. The reference screens carry four things — a small-caps
 *  role above the name, the name, an icon that says what kind of thing it is, and a status chip —
 *  and the difference is the whole reason the binding graph read as anonymous boxes. */
export interface GraphCardData extends Record<string, unknown> {
  title: string;
  /** The small-caps role line above the title (`"provides PowerSource"`, `"state"`). */
  eyebrow?: string;
  /** A second, quieter line under the title — mono, for ids/values/counts. */
  meta?: string;
  /** `TypeIcon` kind; omitted means no icon. */
  kind?: string;
  emphasis?: GraphNodeEmphasis;
  /** Ports. A node with neither draws no handles at all, which is right for a leaf in a read-only
   *  neighbourhood view — a handle the user cannot drag from is an affordance that lies. */
  targetPort?: boolean;
  sourcePort?: boolean;
  /** Faded because something ELSE is being traced. Not a state of the node — a state of the view. */
  dimmed?: boolean;
  /** The narrow card, for graphs whose titles are single words (a state machine). One card, two
   *  widths — never a second card component, and never a per-graph `width` in an inline style. */
  compact?: boolean;
}

export type GraphCardNode = RfNode<GraphCardData, "mtkCard">;

const CHIP_TONE: Record<string, { fg: string; bg: string; bd: string }> = {
  start: { fg: color.info.text, bg: color.info.bg, bd: color.info.border },
  live: { fg: color.success.text, bg: color.success.bg, bd: color.success.border },
};

/** The shared node renderer. Registered once as `mtkCard`, consumed by every graph. */
export function GraphNodeCard({ data, selected }: NodeProps<GraphCardNode>) {
  const emphasis: GraphNodeEmphasis = selected ? "selected" : (data.emphasis ?? "default");
  const e = EMPHASIS[emphasis];
  const chip = e.chip ? CHIP_TONE[e.chip] : undefined;
  return (
    <div
      className="mtk-graph-card"
      data-emphasis={emphasis}
      data-dimmed={data.dimmed ? "true" : "false"}
      data-compact={data.compact ? "true" : "false"}
      style={{
        width: data.compact ? GRAPH_CARD_WIDTH_COMPACT : GRAPH_CARD_WIDTH,
        borderColor: e.border,
        boxShadow: e.ring === "none" ? elevation.e1 : `${e.ring}, ${elevation.e2}`,
      }}
    >
      {data.targetPort && <Handle type="target" position={Position.Left} className="mtk-graph-port" />}
      <div className="mtk-graph-card__head">
        {data.kind != null && <TypeIcon kind={data.kind} size={22} style={{ borderRadius: radius.md }} />}
        <div className="mtk-graph-card__text">
          {data.eyebrow != null && <div className="mtk-graph-card__eyebrow" style={text.eyebrow}>{data.eyebrow}</div>}
          <div className="mtk-graph-card__title">{data.title}</div>
          {data.meta != null && <div className="mtk-graph-card__meta">{data.meta}</div>}
        </div>
        {chip != null && (
          <span
            className="mtk-graph-card__chip"
            style={{ color: chip.fg, background: chip.bg, borderColor: chip.bd }}
          >
            {e.chip}
          </span>
        )}
      </div>
      {data.sourcePort && <Handle type="source" position={Position.Right} className="mtk-graph-port" />}
    </div>
  );
}

/** The registry every graph passes to `nodeTypes`. Frozen at module scope on purpose: React Flow
 *  re-creates every node when this object's identity changes, and an inline `{{ mtkCard: … }}` in a
 *  render body changes identity on every render. */
export const GRAPH_NODE_TYPES = { mtkCard: GraphNodeCard } as const;

// ── the canvas ────────────────────────────────────────────────────────────────────────────────────

/** THE GUTTER THE CHROME LIVES IN, MEASURED — because the alternative was caught in a capture.
 *
 *  The toolbar, the zoom pill, the legend and the mini map FLOAT over the canvas: that is the design
 *  language of the references, where a control panel sits above the artwork rather than beside it.
 *  `fitView` knows nothing about them, so it fills the whole pane — and on the first graph big enough
 *  to reach the corners, the search field landed on top of `Feeder 01 — mesh` and the mini map on top
 *  of `Station 07 readout`. R3 named the first one exactly ("one of them is taking the other's clicks,
 *  and which depends on paint order"); the second was found by reading the PNG.
 *
 *  So the fit reserves a band on each side, MEASURED from the chrome that is actually mounted rather
 *  than stated as a constant beside a stylesheet that would then drift from it. A graph with no legend
 *  and no mini map gets its whole canvas back; one with both keeps clear of them at every zoom. */
const GRAPH_GUTTER = 14;

/** The zoom below which a card stops being readable. See `graphFitDecision`. `minZoom` on
 *  `<ReactFlow>` stays lower, because a user who chooses to zoom out to see the shape is making a
 *  different request from the one the fit answers. */
export const GRAPH_FIT_MIN_ZOOM = 0.75;

/** THE CHROME MOVED OUT OF THE CANVAS, AND THAT IS WHY THIS FUNCTION IS NOW THREE LINES.
 *
 *  The first version of this surface floated the search field, the zoom pill and the legend over the
 *  graph as React Flow `Panel`s, and reserved a band for each in `fitView`'s padding. Two things went
 *  wrong with that, both caught in captures. At 300px — the Inspector track this graph actually ships
 *  in — the top-left and top-right panels simply landed on each other (30px of the `+` button, under
 *  the search field), which no amount of fit padding can help because neither of them is on the
 *  canvas. And whenever the fit hit its legibility floor, the content stopped obeying the reserve and
 *  slid under the chrome anyway, because a reserve is a request to `fitView` and not a boundary.
 *
 *  A row is a boundary. The head and foot are grid tracks OUTSIDE the pane now, so the canvas box IS
 *  the space the graph gets, at every width, with nothing to measure and nothing to agree with. What
 *  is left floating is the mini map alone — the one overlay whose whole job is to sit over the graph
 *  and say where you are in it — so this reserves its band and a gutter, and nothing else.
 *
 *  THE UNIT IS NOT OPTIONAL: React Flow's `Padding` takes a bare number as a RATIO (`0.18` is 18%),
 *  so a measured `60` asks for 6000% and fits the graph to a speck. Ask in `px`. */
function chromePadding(surface: HTMLElement | null) {
  // Measured, not `GRAPH_MINIMAP_SIZE.height` restated — the element is right there, and a reserve
  // that reads the constant would keep agreeing with it after the stylesheet stopped.
  const mini = surface?.querySelector(".react-flow__minimap");
  const bottom = (mini ? mini.getBoundingClientRect().height : 0) + GRAPH_GUTTER;
  // The template-literal type `${number}px` is what React Flow's `Padding` accepts; a plain `string`
  // is not assignable to it, and `as const` on a template literal with an interpolation does not
  // narrow. Declaring the return shape here keeps the unit inside the type system rather than in a
  // comment nobody reads at the call site.
  const px = (n: number): `${number}px` => `${Math.round(n)}px`;
  return { top: px(GRAPH_GUTTER), bottom: px(bottom), left: px(GRAPH_GUTTER), right: px(GRAPH_GUTTER) };
}

/** The zoom/fit pill. Custom rather than React Flow's `<Controls>` because that component ships its
 *  own geometry and its own icons, and the engine already has both — a `Controls` restyled through
 *  four `!important`-adjacent overrides is a second button implementation wearing the first one's
 *  colours. The labels are words and ASCII, not media glyphs: `glyph-coverage.json` exists because a
 *  Unicode media-control character rendered as an empty box on the one machine that took a
 *  screenshot, and the minus sign here is U+2212, which is declared. */
function GraphControls({ ariaLabel, surface }: { ariaLabel: string; surface: RefObject<HTMLDivElement | null> }) {
  const flow = useReactFlow();
  return (
    <div className="mtk-graph-controls" role="group" aria-label={ariaLabel}>
      <button type="button" onClick={() => void flow.zoomIn({ duration: 160 })} aria-label="Zoom in" title="Zoom in">
        +
      </button>
      <button type="button" onClick={() => void flow.zoomOut({ duration: 160 })} aria-label="Zoom out" title="Zoom out">
        −
      </button>
      <button
        type="button"
        onClick={() =>
          void flow.fitView({
            duration: 220,
            padding: chromePadding(surface.current),
            minZoom: GRAPH_FIT_MIN_ZOOM,
          })
        }
        aria-label="Fit graph to view"
        title="Fit graph to view"
      >
        Fit
      </button>
    </div>
  );
}

/** The mini map's own box. Small on purpose: React Flow's default is 200x150, and in the 726x330
 *  canvas a docked graph actually gets, that is 46% of the height reserved for a thumbnail — which
 *  pushed the graph itself out of the top of its own pane. */
export const GRAPH_MINIMAP_SIZE = { width: 132, height: 96 } as const;

/** The canvas below which a mini map is not navigation, it is furniture. Three times its own box on
 *  each axis: any less and the thumbnail is competing with the thing it is a thumbnail of. */
const GRAPH_MINIMAP_MIN_CANVAS = { width: 320, height: 240 };

export interface GraphFitInput {
  /** The bounding box of every node, in flow units. */
  bounds: { width: number; height: number };
  /** The canvas element's own box, in CSS px. */
  canvas: { width: number; height: number };
  nodeCount: number;
  minimapFrom: number;
}

export interface GraphFitDecision {
  /** The zoom `fitView` would choose with no floor — below the floor means it cannot show everything. */
  wanted: number;
  /** `wanted` clamped to the legibility floor: what the graph is actually drawn at. */
  zoom: number;
  /** The fit could not show the whole graph at a readable size. */
  clamped: boolean;
  /** Whether the mini map is drawn — and therefore how much bottom gutter the fit must reserve. */
  minimap: boolean;
}

/** THE ONE RULE THAT TIES LEGIBILITY TO NAVIGATION, extracted so it can be tested.
 *
 *  Every capture in this harness is taken at deviceScaleFactor 2, which is exactly how a 17-node graph
 *  fitted at 0.39 read as "fine" in a PNG and would have shipped with 5px type. So the fit stops at a
 *  readable zoom — and the moment it does, the graph no longer shows everything, which is precisely
 *  when the user needs to be told there is more of it. The mini map is that telling. Tying the two
 *  together in one function is what stops them drifting into "a floor that hides content silently" and
 *  "a thumbnail that appears at an arbitrary node count".
 *
 *  A mini map still needs somewhere to live: below `GRAPH_MINIMAP_MIN_CANVAS` it would take a third of
 *  the pane to say something the Fit button already says, so in a small dock the answer is Fit and
 *  panning. That is a real limit, and it is stated rather than discovered. */
export function graphFitDecision({ bounds, canvas, nodeCount, minimapFrom }: GraphFitInput): GraphFitDecision {
  const roomy =
    canvas.width >= GRAPH_MINIMAP_MIN_CANVAS.width && canvas.height >= GRAPH_MINIMAP_MIN_CANVAS.height;
  const free = (extraBottom: number) => ({
    w: canvas.width - GRAPH_GUTTER * 2,
    h: canvas.height - GRAPH_GUTTER * 2 - extraBottom,
  });
  const fitTo = (f: { w: number; h: number }) =>
    Math.min(f.w / Math.max(1, bounds.width), f.h / Math.max(1, bounds.height));

  // Decided WITHOUT the mini map's gutter, then applied WITH it. Asking the other way round is
  // circular: the reserve exists because the thumbnail is shown, and the thumbnail is shown because
  // the graph did not fit — measure the graph against the pane it would have had.
  const wanted = fitTo(free(0));
  const clamped = wanted < GRAPH_FIT_MIN_ZOOM;
  const minimap = roomy && (nodeCount >= minimapFrom || clamped);
  const zoom = Math.max(wanted, GRAPH_FIT_MIN_ZOOM);
  return { wanted, zoom, clamped, minimap };
}

/** THE FLOOR UNDER "FIT", AND THE REASON IT IS NOT OPTIONAL.
 *
 *  `fitView` will shrink a graph as far as it takes to get everything on screen, and every capture in
 *  this harness is taken at deviceScaleFactor 2 — which is exactly how a 17-node graph fitted at 0.39
 *  read as "fine" in a PNG and would have shipped with 5px type. A graph nobody can read has not been
 *  framed, it has been disposed of.
 *
 *  So the fit stops at a zoom where the card is still legible, and when that means the graph no longer
 *  fits, the surface says so by showing the mini map — see `clamped` below. That is the honest trade
 *  and it is the one every real graph editor makes: legibility first, navigation second, "all of it at
 *  once" only when both can be had. `minZoom` on `<ReactFlow>` stays lower, because a user who chooses
 *  to zoom out to see the shape is making a different request from the one the fit answers. */
/** Re-fits once the chrome exists and the cards have been measured, and reports whether the fit hit
 *  its floor. Rendered as a child of `<ReactFlow>` because `useReactFlow` needs its provider, and it
 *  draws nothing: it is the one place that turns "the graph changed" into "the graph is framed", so
 *  the Fit button and the automatic fit cannot disagree about what framing means. */
function GraphAutoFit({
  surface,
  nodes,
  signature,
  reserve,
  minimapFrom,
  onDecide,
}: {
  surface: RefObject<HTMLDivElement | null>;
  nodes: GraphCardNode[];
  signature: string;
  reserve: boolean;
  minimapFrom: number;
  onDecide: (decision: GraphFitDecision) => void;
}) {
  const flow = useReactFlow();
  // A LAYOUT EFFECT, AND `clamped` COMPUTED RATHER THAN OBSERVED. The first version awaited
  // `fitView()`'s promise and then read `getZoom()` back, which made settling a CHAIN — fit, resolve,
  // set state, mount the mini map, re-fit — that took longer than the 250 ms the capture harness waits
  // before it asserts. The gate duly caught it: `unclipped` reported five cards clipped by 6 px at an
  // intermediate zoom of 0.837, while a probe with a 1.5 s wait measured the settled 0.75. A graph that
  // is only correct after a second is a graph that visibly jumps on a slow machine, so this is a real
  // defect and not a harness timing quirk.
  //
  // The zoom the fit WILL choose is a division, so there is nothing to wait for: bounds over the box
  // that is left once the chrome has its bands. One pass, before paint, no promise.
  useLayoutEffect(() => {
    // The CANVAS box, not the surface box: the head and foot are rows outside it now, so the space the
    // graph actually gets is this element and nothing has to be subtracted from it.
    const box = surface.current?.querySelector(".mtk-graph-surface__canvas")?.getBoundingClientRect();
    if (box && nodes.length > 0) {
      onDecide(
        graphFitDecision({
          bounds: flow.getNodesBounds(nodes),
          canvas: { width: box.width, height: box.height },
          nodeCount: nodes.length,
          minimapFrom,
        }),
      );
    }
    void flow.fitView({ padding: chromePadding(surface.current), minZoom: GRAPH_FIT_MIN_ZOOM });
    // `reserve` is in the dependency list because showing the mini map CHANGES the bottom gutter, so
    // the fit that decided to show it is no longer the right fit. It converges in one step: the extra
    // reserve can only make the fit smaller, and smaller is still clamped.
  }, [flow, surface, nodes, signature, reserve, minimapFrom, onDecide]);
  return null;
}

export interface GraphLegendItem {
  label: string;
  /** The edge state whose stroke this swatch shows — so the legend cannot drift from the edges it
   *  explains: both read `graphEdgeStyle`. */
  state: GraphEdgeState;
}

export interface GraphSurfaceProps {
  nodes: GraphCardNode[];
  edges: RfEdge[];
  /** Announced by the canvas and used to name its controls — every graph needs one and no graph
   *  should have to remember three separate strings for it. */
  label: string;
  "data-testid"?: string;
  /** Free slot at the top-left — a search field, a filter row, a breadcrumb. */
  toolbar?: ReactNode;
  /** Colour rules the user cannot be expected to guess. Rendered bottom-left. */
  legend?: GraphLegendItem[];
  /** The mini map appears once a graph is big enough to get lost in — OR as soon as the fit hits its
   *  legibility floor and stops showing everything, which is the moment it stops being decoration and
   *  becomes the only thing telling the user there is more graph off to the side. A number, not a
   *  boolean, so the count threshold is one decision in one place instead of a different
   *  `nodes.length > n` per editor. */
  minimapFrom?: number;
  height?: number | string;
  minHeight?: number;
  onNodeHover?: (id: string | null) => void;
  flowProps?: Partial<ReactFlowProps>;
}

/** The canvas every graph editor sits in: background, viewport behaviour, chrome and slots.
 *
 *  Everything a subsystem could get subtly wrong on its own — the dot grid's colour and gap, the fit
 *  padding, the zoom limits, whether the attribution badge shows, what a mini map looks like, where
 *  the legend goes, whether the chrome covers the graph — is decided once, here. */
export function GraphSurface({
  nodes,
  edges,
  label,
  toolbar,
  legend,
  minimapFrom = 12,
  height = "100%",
  minHeight = 240,
  onNodeHover,
  flowProps,
  ...rest
}: GraphSurfaceProps) {
  const surface = useRef<HTMLDivElement>(null);
  const onEnter = useCallback(
    (_: unknown, node: { id: string }) => onNodeHover?.(node.id),
    [onNodeHover],
  );
  const onLeave = useCallback(() => onNodeHover?.(null), [onNodeHover]);

  /** THE MINI MAP WAS EMPTY, AND THE REASON IS A REAL CONTRACT, NOT A CSS SLIP.
   *
   *  React Flow's mini map draws each node by reading `node.internals.userNode` and skipping any node
   *  that fails `nodeHasDimensions` — which checks `measured ?? width ?? initialWidth` on the USER
   *  node, the array the caller passes in. In a fully controlled flow with no `onNodesChange`, the
   *  measured size is written into the library's internal lookup (so the canvas lays out correctly and
   *  `fitView` frames correctly) and is NEVER written back to the caller's node. Every node therefore
   *  fails the check, the mini map renders its mask and nothing else, and what ships is a blank white
   *  card floating over the graph — visible in `progress/graph-framework/before/binding-graph-dense`.
   *
   *  `AnimationGraphEditor` sidesteps this by declaring `initialWidth`/`initialHeight` constants beside
   *  its node component: a truthful estimate, and a card's geometry stated twice. This measures
   *  instead. `onNodesChange` is where React Flow reports the dimensions it observed; keeping them and
   *  stamping them back onto the nodes closes the loop with the browser's own numbers, so the mini map
   *  is drawing what is on screen rather than what someone typed. The equality guard is what stops it
   *  from being a render loop: an unchanged measurement returns the same state object. */
  const [measured, setMeasured] = useState<Record<string, { width: number; height: number }>>({});
  const onNodesChange = useCallback((changes: NodeChange[]) => {
    const seen: Record<string, { width: number; height: number }> = {};
    let any = false;
    for (const c of changes) {
      if (c.type === "dimensions" && c.dimensions) {
        seen[c.id] = { width: c.dimensions.width, height: c.dimensions.height };
        any = true;
      }
    }
    if (!any) return;
    setMeasured((prev) => {
      let changed = false;
      const next = { ...prev };
      for (const [id, d] of Object.entries(seen)) {
        if (prev[id]?.width !== d.width || prev[id]?.height !== d.height) {
          next[id] = d;
          changed = true;
        }
      }
      return changed ? next : prev;
    });
  }, []);

  /** Width comes from the constant the card itself is drawn with — one number, two readers, never two
   *  numbers. Height is the part that genuinely depends on how many lines a title wraps to, so it is
   *  the part that waits for the browser; until it arrives, `GRAPH_CARD_HEIGHT_ESTIMATE` keeps the node
   *  dimensioned so the mini map has something to draw and the first fit is not wild. */
  const sized = useMemo(
    () =>
      nodes.map((n) => ({
        ...n,
        width: n.data.compact ? GRAPH_CARD_WIDTH_COMPACT : GRAPH_CARD_WIDTH,
        height: measured[n.id]?.height ?? GRAPH_CARD_HEIGHT_ESTIMATE,
      })),
    [nodes, measured],
  );

  /** What the automatic fit re-runs on. Node identity churns whenever a hover dims a card, and
   *  re-fitting then would move the graph under the pointer — so the signature is the SHAPE (which
   *  nodes, where, how big), not the array. */
  const signature = useMemo(
    () =>
      sized
        .map((n) => `${n.id}:${Math.round(n.position.x)},${Math.round(n.position.y)},${n.width ?? 0}x${n.height ?? 0}`)
        .join("|") + `#${legend != null}${toolbar != null}`,
    [sized, legend, toolbar],
  );

  const [minimap, setMinimap] = useState(false);
  const onDecide = useCallback((d: GraphFitDecision) => setMinimap(d.minimap), []);
  // Reset when the graph itself changes: "this graph did not fit" is a fact about a shape, and
  // carrying it across a new selection would leave a mini map hanging over a graph of three nodes.
  useEffect(() => setMinimap(false), [signature]);

  return (
    <div className="mtk-graph-surface" ref={surface} style={{ height, minHeight }} {...rest}>
      {/* The provider, not the `<ReactFlow>` element, is what `useReactFlow` needs — which is what lets
          the zoom pill live in a row beside the canvas instead of floating on top of it. */}
      <ReactFlowProvider>
        <div className="mtk-graph-surface__head">
          {toolbar != null && <div className="mtk-graph-toolbar">{toolbar}</div>}
          <GraphControls ariaLabel={`${label} view controls`} surface={surface} />
        </div>
        <div className="mtk-graph-surface__canvas">
          <ReactFlow
            nodes={sized}
            edges={edges}
            nodeTypes={GRAPH_NODE_TYPES}
            edgeTypes={GRAPH_EDGE_TYPES}
            onNodesChange={onNodesChange}
            fitView
            minZoom={0.2}
            maxZoom={2.5}
            nodesDraggable={false}
            nodesConnectable={false}
            proOptions={{ hideAttribution: true }}
            onNodeMouseEnter={onNodeHover ? onEnter : undefined}
            onNodeMouseLeave={onNodeHover ? onLeave : undefined}
            aria-label={label}
            {...flowProps}
          >
            <Background variant={BackgroundVariant.Dots} gap={18} size={1} color={graphTheme.grid} />
            <GraphAutoFit
              surface={surface}
              nodes={sized}
              signature={signature}
              reserve={minimap}
              minimapFrom={minimapFrom}
              onDecide={onDecide}
            />
            {minimap && (
              <MiniMap
                pannable
                zoomable
                aria-label={`${label} mini map`}
                nodeColor={MINIMAP_NODE}
                style={GRAPH_MINIMAP_SIZE}
              />
            )}
          </ReactFlow>
        </div>
        {legend != null && legend.length > 0 && (
          <div className="mtk-graph-legend">
            {legend.map((item) => (
              <span key={item.label} className="mtk-graph-legend__item">
                <svg width="18" height="8" aria-hidden="true">
                  <line x1="1" y1="4" x2="17" y2="4" style={graphEdgeStyle(item.state)} />
                </svg>
                {item.label}
              </span>
            ))}
          </div>
        )}
      </ReactFlowProvider>
    </div>
  );
}

/** A stable function identity: React Flow memoizes the mini map's node list on its props, and an
 *  inline arrow re-renders every rect on every parent render. */
const MINIMAP_NODE = () => color.border.strong;

/** Filter-as-you-type over a graph's nodes, shared so "search" means the same thing in every editor.
 *
 *  Returns the matching id set (or `null` for "no query" — which is NOT the same as "everything
 *  matched", because a query that matches nothing must be able to say so). Matching is on the title
 *  and the eyebrow, case-insensitively, because a user searching a bind graph types either the name
 *  of a thing or the name of the relation and should not have to know which field they are in. */
export function useGraphSearch(nodes: readonly GraphCardNode[]) {
  const [query, setQuery] = useState("");
  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (q === "") return null;
    const hit = new Set<string>();
    for (const n of nodes) {
      const hay = `${n.data.title} ${n.data.eyebrow ?? ""} ${n.data.meta ?? ""}`.toLowerCase();
      if (hay.includes(q)) hit.add(n.id);
    }
    return hit;
  }, [nodes, query]);
  return { query, setQuery, matches };
}
