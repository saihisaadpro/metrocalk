//! Shared timeline + transport contract — **the one timeline framework** the constitution asks for.
//!
//! *"Every timeline in every subsystem should share the same implementation. Animation · Sequencer ·
//! Cutscenes · VFX · Audio · Physics · Procedural systems · Simulation. All timelines should inherit
//! from one timeline framework."* — `docs/UI/prompt.txt`, and gate **4** of §13's migration
//! programme, the last of the six still open.
//!
//! WHAT WAS ACTUALLY THERE. The CSS half of this had already been written — `.mtk-timeline*` in
//! `global.css`, with `--mtk-track-header` declared once so the ruler and the lanes cannot disagree
//! about where the header column ends. The REACT half had not: `Ruler`, `AnnotationLane` and
//! `Playhead` were **private functions at the bottom of `AnimationWorkspace.tsx`**, a 2,783-line
//! panel, so no other subsystem could reach them. That is a framework only in the sense that a
//! stylesheet is a framework: the moment a second surface needs a scrubber it writes its own, and
//! `PhysicsPanel` had — a `<label>`, a bare `Slider`, a mono `frame 12/300` and a paragraph of
//! helper prose, with no step control, no readout rhythm and no playhead anywhere. Two subsystems,
//! two transports, nothing comparing them.
//!
//! So the split is the same one [`graph.tsx`] draws: **this module owns the ruler, the lane, the
//! head, the playhead, the key, the chip, the transport and the curve canvas**; domain adapters own
//! ticks, keys, and transactions. A subsystem that wants a timeline brings a duration and rows —
//! never a `position: absolute` of its own, and never a colour.
//!
//! THREE DEFECTS THE FIRST CAPTURE OF A POPULATED TIMELINE CONTAINED, all of them geometry this
//! module now owns rather than leaves to the caller:
//!
//!   1. **the last ruler label was cut in half.** Ticks are `translateX(-50%)`, which is right for
//!      every label with room either side and wrong for the one at `left: 100%` — half of it lands
//!      outside the track. `--first` already existed for the mirror case (and had been written as
//!      `:first-child`, which matched the PLAYHEAD, so it had never applied to a tick at all);
//!      `--last` did not, and `2.` was the whole rightmost label in the shipped capture.
//!   2. **the playhead painted THROUGH a ruler label.** The line is `z-index: 3` and a tick is
//!      `z-index: auto`, so a positioned element with a stacking index wins over its positioned
//!      siblings regardless of DOM order — the accent bar struck out the "1" of "1.20s" at the one
//!      moment the reader most wants to know what time it is. Labels now sit above the line and
//!      knock out the ruler behind their own glyphs, which is what an NLE ruler does.
//!   3. **a badge on every row is not a badge.** Four track rows carried four coloured pills, three
//!      of them saying `ready` — "accent colours only where interaction requires attention", and
//!      nothing about a healthy track requires any. [[TimelineTrackHead]] takes `attention`, and a
//!      row that is fine says so by having nothing to say. The state is still on the row as
//!      `data-binding-state` for the gates and in the name's `title` for a person.
//!
//! COLOUR IS NEVER THE ONLY CHANNEL (§10). A muted lane is hatched as well as tinted, an invalid row
//! is ruled as well as tinted, the playhead is a line as well as accented, and every icon-only
//! control here takes a required `label` that becomes both `aria-label` and `title`.

import type { CSSProperties, MouseEvent as ReactMouseEvent, ReactNode, Ref, UIEvent } from "react";
import { Icon } from "./icons";
import { Button, Toolbar, ToolbarGroup, type ButtonProps } from "./primitives";

/** A data-* / id passthrough for the stable e2e/Vitest hooks, as the other primitives take. */
type DataAttrs = {
  id?: string;
  title?: string;
  "data-testid"?: string;
  "data-context"?: string;
  "data-enabled"?: boolean;
  "data-locked"?: boolean;
  "data-binding-state"?: string;
  "data-selected"?: boolean | undefined;
  "data-readiness"?: string;
};

/** One labelled division of the ruler. `value` is in the caller's own time unit; this module never
 *  interprets it beyond dividing it by the duration. */
export interface TimelineTickMark {
  value: number;
  label: string;
}

/**
 * Where a time sits along a lane, as a CSS percentage.
 *
 * A duration of zero is not an error the caller has to pre-empt — an empty sequence is an ordinary
 * state and every consumer had written its own `Math.max(1, …)` guard before reaching for this.
 */
export function timelineOffset(value: number, duration: number): string {
  const span = duration > 0 ? duration : 1;
  const ratio = Math.min(1, Math.max(0, value / span));
  return `${ratio * 100}%`;
}

/**
 * The time a pointer event lands on, in the caller's unit, or `null` when the element has no width
 * yet.
 *
 * WHY THIS IS SHARED. Both timelines had this arithmetic inline and they had written it differently:
 * the ruler divided by `bounds.width` and the lanes by the same, but neither clamped, so a click on
 * the 1px right border scrubbed PAST the end of the sequence and the transport then reconciled the
 * playhead backwards a frame. Clamping in one place is the whole reason a geometry helper exists.
 */
export function timelineTickAt(
  event: { clientX: number; currentTarget: { getBoundingClientRect(): DOMRect } },
  duration: number,
): number | null {
  const bounds = event.currentTarget.getBoundingClientRect();
  if (bounds.width <= 0) return null;
  const ratio = (event.clientX - bounds.left) / bounds.width;
  return Math.min(duration, Math.max(0, ratio * duration));
}

/**
 * Evenly spaced ruler divisions.
 *
 * Ten divisions is the number `.mtk-timeline__lane--gridded`'s `background-size: 10%` draws, so the
 * graticule under the lanes and the numbers above them are one decision rather than two that agree
 * by luck. A caller that wants a different count must change both, which is the point.
 */
export const TIMELINE_DIVISIONS = 10;

export function timelineTicks(duration: number, label: (value: number) => string): TimelineTickMark[] {
  // NOT ROUNDED. This used to be `Math.round((duration * index) / TIMELINE_DIVISIONS)` and it was
  // wrong twice over on any duration that is not a multiple of ten. It is a POSITION as well as a
  // number: at a 6.5s duration the last division rounded to 7 and the label sat past the end of its
  // own lane, and three pairs of divisions rounded to the SAME value — which the ruler then used as
  // its React key, so it rendered duplicate keys and dropped divisions. The only reason no consumer
  // saw it is that the first one measures in 60000ths of a second, where every tenth is an integer.
  // Formatting belongs to the caller's `label`, which is the function that knows the unit.
  return Array.from({ length: TIMELINE_DIVISIONS + 1 }, (_, index) => {
    const value = (duration * index) / TIMELINE_DIVISIONS;
    return { value, label: label(value) };
  });
}

// ── The surface ───────────────────────────────────────────────────────────────────────────────────

export interface TimelineSurfaceProps extends DataAttrs {
  /** The lane width in px. The header column is added by the STYLESHEET, from the same
   *  `--mtk-track-header` the rows lay themselves out with, so the strip's scroll width and the
   *  column it must leave room for are one statement instead of two — the 178 used to be written as
   *  a literal here and in three other places, and nothing compared them. */
  laneWidth: number;
  className?: string;
  style?: CSSProperties;
  onScroll?: (event: UIEvent<HTMLDivElement>) => void;
  scrollRef?: Ref<HTMLDivElement>;
  children: ReactNode;
  /** Rendered as a direct child of the SCROLLER, not of the content strip — a percentage width
   *  resolves against the visible box there, and `sticky` keeps it in place while the lanes pan.
   *  Inside the strip it would centre itself in the scroll width and drift off screen. */
  footer?: ReactNode;
}

/** A TIMELINE SHORTER THAN THE RULER PLUS ONE LANE IS NOT A TIMELINE — the smallest thing that still
 *  shows *when* something happens and *that* something does. The scroller was `minHeight: 0` ("be
 *  exactly as small as the box you are in"), and in the bottom dock at a 640px window that box is
 *  **9px** holding 262px of timeline: a scroll container that can show neither its scrollbar nor one
 *  row of itself. It was green in the layout gate since ADR-125 because R6's bar was "shows literally
 *  nothing", and 9px is not nothing.
 *
 *  The floor lives in `.mtk-timeline` as `calc(var(--mtk-ruler-height) + var(--mtk-lane-height))`,
 *  beside the two heights it is made of, so it cannot drift from them. It was briefly stated here as
 *  well — three numbers in TypeScript mirroring three in CSS, which is the same two-statements-one-
 *  contract this module removed from the header width four lines up. */

export function TimelineSurface({
  laneWidth,
  className,
  style,
  onScroll,
  scrollRef,
  children,
  footer,
  ...rest
}: TimelineSurfaceProps) {
  return (
    <div
      ref={scrollRef}
      className={["mtk-scroll", "mtk-timeline", className].filter(Boolean).join(" ")}
      onScroll={onScroll}
      style={{ overflow: "auto", ...style }}
      {...rest}
    >
      <div className="mtk-timeline__strip" style={{ ["--mtk-timeline-width" as string]: `${laneWidth}px` }}>
        {children}
      </div>
      {footer}
    </div>
  );
}

/** The sticky, full-visible-width host an empty state goes in. See [[TimelineSurfaceProps.footer]]. */
export function TimelineEmpty({ children }: { children: ReactNode }) {
  return <div className="mtk-timeline__empty">{children}</div>;
}

// ── Rows ──────────────────────────────────────────────────────────────────────────────────────────

export interface TimelineRowProps extends DataAttrs {
  /** `track` is the taller row whose head carries controls; `lane` is a single-purpose annotation
   *  strip (markers, events, a recorded-frame band). */
  variant?: "lane" | "track";
  muted?: boolean;
  invalid?: boolean;
  children: ReactNode;
}

export function TimelineRow({
  variant = "lane",
  muted = false,
  invalid = false,
  children,
  ...rest
}: TimelineRowProps) {
  const cls = [
    "mtk-timeline__row",
    variant === "track" && "mtk-timeline__row--track",
    invalid && "is-invalid",
    muted && "is-muted",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls} {...rest}>
      {children}
    </div>
  );
}

/** The sticky header cell of a `lane` row: one quiet word naming what the strip holds. */
export function TimelineHead({ children }: { children: ReactNode }) {
  return <div className="mtk-timeline__head">{children}</div>;
}

export interface TimelineTrackHeadProps {
  name: string;
  /** The full identity behind the (ellipsised) name, plus anything a badge is no longer saying. */
  title?: string;
  selected?: boolean;
  onSelect?: () => void;
  /** A count, a unit, a duration — the one piece of metadata that is stated nowhere else. */
  meta?: ReactNode;
  /**
   * The badge, and ONLY when the row needs attention.
   *
   * See the header note: three of four rows in the first populated capture carried a green `ready`
   * pill, which is four accents competing inside 178px to say that nothing is wrong. A row that is
   * fine carries no badge; the state stays on the row as a `data-` attribute and in `title`.
   */
  attention?: ReactNode;
  /** Icon-only toggles — mute, lock — at the end of the metadata line. */
  actions?: ReactNode;
}

export function TimelineTrackHead({
  name,
  title,
  selected = false,
  onSelect,
  meta,
  attention,
  actions,
}: TimelineTrackHeadProps) {
  return (
    <div className={`mtk-timeline__head${selected ? " is-selected" : ""}`}>
      <Button
        variant="ghost"
        className="mtk-timeline__track-select"
        aria-pressed={selected}
        aria-label={`Select track ${name}`}
        title={title}
        onClick={onSelect}
      >
        <span className="mtk-timeline__track-name">{name}</span>
      </Button>
      <div className="mtk-timeline__track-meta">
        {attention}
        {meta !== undefined && <span className="mtk-timeline__track-keys">{meta}</span>}
        {actions !== undefined && <div className="mtk-timeline__track-actions">{actions}</div>}
      </div>
    </div>
  );
}

export interface TimelineLaneProps extends DataAttrs {
  /** The zebra fill that lets the eye follow one row across a wide strip. */
  alternate?: boolean;
  onClick?: (event: ReactMouseEvent<HTMLDivElement>) => void;
  children?: ReactNode;
}

/** A lane always carries the graticule: the ten divisions the ruler labels, so a key lines up with a
 *  number. It was on the track rows and NOT on the marker/event rows, which is why an event at 1.6 s
 *  had nothing under it to sight against. */
export function TimelineLane({ alternate = false, onClick, children, ...rest }: TimelineLaneProps) {
  const cls = [
    "mtk-timeline__lane",
    "mtk-timeline__lane--gridded",
    alternate && "mtk-timeline__lane--odd",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls} onClick={onClick} {...rest}>
      {children}
    </div>
  );
}

// ── The playhead ──────────────────────────────────────────────────────────────────────────────────

/**
 * A line you can see and aim at.
 *
 * With no `tick`, it reads `--animation-playhead` off an ancestor — the 30 Hz custom-property path
 * that moves the head during playback **without re-rendering React**, which is invariant 4 (the hot
 * path never crosses the JS boundary) expressed in one CSS variable. Passing a tick is for the rows
 * that are already re-rendering anyway.
 */
export function TimelinePlayhead({
  tick,
  duration,
  handle = false,
}: {
  tick?: number;
  duration?: number;
  handle?: boolean;
}) {
  const left =
    tick === undefined || duration === undefined
      ? "var(--animation-playhead, 0%)"
      : timelineOffset(tick, duration);
  return (
    <div
      aria-hidden="true"
      className={`mtk-timeline__playhead${handle ? " mtk-timeline__playhead--handled" : ""}`}
      style={{ left }}
    />
  );
}

// ── The ruler ─────────────────────────────────────────────────────────────────────────────────────

export interface TimelineRulerProps {
  /** The head cell's word. "Tracks" in an animation timeline, "Frames" in a recording. */
  label: ReactNode;
  duration: number;
  currentTick: number;
  ticks: readonly TimelineTickMark[];
  onScrub?: (tick: number) => void;
  "data-testid"?: string;
}

export function TimelineRuler({
  label,
  duration,
  currentTick,
  ticks,
  onScrub,
  ...rest
}: TimelineRulerProps) {
  const last = ticks.length - 1;
  return (
    <div className="mtk-timeline__row mtk-timeline__ruler">
      <div className="mtk-timeline__head">{label}</div>
      <div
        className="mtk-timeline__ruler-track"
        onClick={(event) => {
          if (!onScrub) return;
          const tick = timelineTickAt(event, duration);
          if (tick !== null) onScrub(tick);
        }}
        {...rest}
      >
        <TimelinePlayhead tick={currentTick} duration={duration} handle />
        {ticks.map((tick, index) => (
          <span
            // The DIVISION is the identity, not the value it lands on: two divisions of a short
            // sequence can share a rendered number, and keying on the number silently dropped one of
            // them. `timelineTicks` no longer rounds, but a caller may still hand over a list with
            // repeats, and a key that depends on the data is a key that can collide.
            key={index}
            className={[
              "mtk-timeline__tick",
              index === 0 && "mtk-timeline__tick--first",
              index === last && last > 0 && "mtk-timeline__tick--last",
            ]
              .filter(Boolean)
              .join(" ")}
            style={{ left: timelineOffset(tick.value, duration) }}
          >
            {tick.label}
          </span>
        ))}
      </div>
    </div>
  );
}

// ── Things ON a lane ──────────────────────────────────────────────────────────────────────────────

export interface TimelineKeyProps extends Omit<ButtonProps, "children" | "variant" | "className"> {
  tick: number;
  duration: number;
  /** The accent fill. Everything else is a hollow diamond. */
  selected?: boolean;
  /** A key on a track whose binding is broken: ruled in danger, never only tinted. */
  invalid?: boolean;
  /** A locked track's keys are filled with the inset surface, so "you cannot move this" is visible
   *  on the key itself rather than only on the lock button 178px away. */
  locked?: boolean;
}

/**
 * The diamond.
 *
 * `clamp()` on the offset is what keeps the first and last key inside the lane instead of half
 * outside it — the same edge case the ruler's `--first`/`--last` handle for labels, and the reason
 * both live in this module rather than in whichever panel hit it first.
 */
export function TimelineKey({ tick, duration, selected = false, invalid = false, locked = false, ...rest }: TimelineKeyProps) {
  return (
    <Button
      variant="ghost"
      className="mtk-timeline__key"
      data-selected={selected || undefined}
      style={{
        left: `clamp(0px, calc(${timelineOffset(tick, duration)} - 12px), calc(100% - 24px))`,
      }}
      {...rest}
    >
      <span
        aria-hidden="true"
        className={[
          "mtk-timeline__key-mark",
          selected && "is-selected",
          invalid && "is-invalid",
          locked && "is-locked",
        ]
          .filter(Boolean)
          .join(" ")}
      />
    </Button>
  );
}

export interface TimelineChipProps {
  tick: number;
  duration: number;
  children: ReactNode;
  /** The chip's own action — jump the playhead here. */
  onClick?: () => void;
  label: string;
  title?: string;
  /** The authored colour of a marker, when it has one. Passed through rather than chosen here: it is
   *  document data, not a design decision. */
  markColor?: string;
  tone?: "marker" | "event";
  /** The trailing remove control, when the row is editable. */
  onRemove?: () => void;
  removeLabel?: string;
  removeDisabled?: boolean;
  "data-testid"?: string;
}

/**
 * A marker or an event, anchored at a time.
 *
 * WHY THE REMOVE CONTROL IS PART OF THE CHIP. Both lanes had built this by hand as two absolutely
 * positioned siblings inside a `translateX(-8px)` span, and the `×` was a bare 14×16 `<button>` with
 * `border: 0` — below every target-size floor in §10 and outside the button primitive entirely. As
 * one component the pair is a single anchored object, the remove control is a real `Button`, and the
 * hit areas cannot overlap because they are laid out, not positioned.
 */
export function TimelineChip({
  tick,
  duration,
  children,
  onClick,
  label,
  title,
  markColor,
  tone = "event",
  onRemove,
  removeLabel,
  removeDisabled = false,
  ...rest
}: TimelineChipProps) {
  return (
    <span className="mtk-timeline__chip" style={{ left: timelineOffset(tick, duration) }}>
      <Button
        variant="ghost"
        className={`mtk-timeline__chip-mark mtk-timeline__chip-mark--${tone}`}
        aria-label={label}
        title={title ?? label}
        style={markColor ? { color: markColor } : undefined}
        onClick={onClick}
        {...rest}
      >
        {children}
      </Button>
      {onRemove !== undefined && (
        <Button
          variant="ghost"
          icon
          compact
          className="mtk-timeline__chip-remove"
          aria-label={removeLabel ?? `Remove ${label}`}
          title={removeLabel ?? `Remove ${label}`}
          disabled={removeDisabled}
          disabledReason="Another edit is still in flight — this will be available in a moment."
          onClick={onRemove}
        >
          <Icon name="close" size="sm" />
        </Button>
      )}
    </span>
  );
}

export interface TimelineClipProps extends DataAttrs {
  /** Where the clip starts, in the caller's own time unit. */
  start: number;
  /** How long it lasts, same unit. */
  length: number;
  /** The whole lane's span, same unit. */
  duration: number;
  /** What it is, painted inside the bar. */
  label: string;
  /** The full sentence behind the (ellipsised) label. */
  title?: string;
  selected?: boolean;
  /** Ruled in danger, never only tinted: a clip the sequence has a warning about. */
  invalid?: boolean;
  /** The bar under the playhead. Reads as live, and is a different thing from selected. */
  live?: boolean;
  onClick?: () => void;
  /** A duration, a count — the one number that belongs ON the bar. */
  meta?: ReactNode;
}

/**
 * A SPAN on a lane — a thing that lasts, as opposed to [[TimelineKey]] and [[TimelineChip]], which
 * are things that happen at an instant.
 *
 * WHY THIS WAS MISSING. Both of this module's first consumers annotate a moment: a keyframe is a
 * time, a marker is a time, a recorded frame is a time. Nothing in the editor had ever drawn a
 * DURATION, so a cutscene — whose entire content is four lengths in a row — could only ever be a
 * bulleted list of sentences, and the one number that decides what a cut feels like had nowhere to
 * be. The geometry is the same `timelineOffset` every other mark uses, plus a width; putting it here
 * rather than in the panel is what keeps a shot bar and a keyframe diamond agreeing about where 4.2s
 * is.
 *
 * `min-width` is in the stylesheet beside the lane height it is a fraction of. A 0.2s shot inside a
 * four-minute sequence is a bar a fraction of a pixel wide, and a control you cannot hit is not a
 * control; below that floor adjacent bars overlap, which is the honest picture of shots too short to
 * tell apart.
 */
export function TimelineClip({
  start,
  length,
  duration,
  label,
  title,
  selected = false,
  invalid = false,
  live = false,
  onClick,
  meta,
  ...rest
}: TimelineClipProps) {
  const span = duration > 0 ? duration : 1;
  const width = Math.max(0, Math.min(1, length / span)) * 100;
  return (
    <Button
      variant="ghost"
      className={[
        "mtk-timeline__clip",
        invalid && "is-invalid",
        live && "is-live",
      ]
        .filter(Boolean)
        .join(" ")}
      data-selected={selected || undefined}
      aria-pressed={selected}
      title={title ?? label}
      style={{ left: timelineOffset(start, duration), width: `${width}%` }}
      onClick={onClick}
      {...rest}
    >
      <span className="mtk-timeline__clip-label">{label}</span>
      {meta !== undefined && <span className="mtk-timeline__clip-meta">{meta}</span>}
    </Button>
  );
}

// ── The transport ─────────────────────────────────────────────────────────────────────────────────

export interface TransportProps extends DataAttrs {
  "aria-label": string;
  children: ReactNode;
  style?: CSSProperties;
}

/**
 * The transport row: what time it is · the transport · where you are in the clip · how it repeats.
 *
 * It is a [[Toolbar]] with one extra class, deliberately — the wrap policy, the group gap and the
 * attached-segment radius are already solved there, and a transport that re-solved them would drift
 * from every other row in the engine the first time one of them changed. What this adds is the
 * floating pill: a panel surface, `radius.xl`, elevation 1, and margin instead of a bottom rule.
 */
export function Transport({ children, style, ...rest }: TransportProps) {
  return (
    <Toolbar tight raised={false} divided={false} className="mtk-transport" role="group" style={style} {...rest}>
      {children}
    </Toolbar>
  );
}

/** The attached run of step/play/stop buttons — one segmented surface, not four loose squares. */
export function TransportButtons({ children, ...rest }: { children: ReactNode; "aria-label": string }) {
  return (
    <ToolbarGroup attached {...rest}>
      {children}
    </ToolbarGroup>
  );
}

/** The scrub track. Takes the row's leftover width and never less than 140px of it. */
export function TransportScrub({ children }: { children: ReactNode }) {
  return (
    <ToolbarGroup grow={140} aria-label="Scrub" className="mtk-transport__scrub">
      {children}
    </ToolbarGroup>
  );
}

// ── The curve canvas ──────────────────────────────────────────────────────────────────────────────

export interface CurveCanvasProps {
  "aria-label": string;
  /** What the vertical axis measures, in the reader's words. The references label both axes. */
  valueAxis: string;
  timeAxis: string;
  /** The plotted content, in the 0–800 × 0–260 user space the graticule is drawn in. */
  children: ReactNode;
  /** Shown at the top-left of the card: what curve this is. */
  caption?: ReactNode;
  /** Shown at the top-right: how to change it. */
  hint?: ReactNode;
}

/** The plot's user space and the inset the graticule is drawn inside it. The inset is symmetric on
 *  purpose: the shipped curve reserved 20px above the plot and 40px below, so the card had a band of
 *  dead white along its bottom edge that read as the grid having failed to reach it. */
export const CURVE_VIEWBOX = { width: 800, height: 260, left: 40, top: 20, right: 760, bottom: 240 } as const;

/**
 * The easing/value curve surface.
 *
 * WHAT IT ADDS OVER THE `<svg>` IT REPLACES: a graticule and two named axes. The shipped curve was a
 * blue polyline and some circles on an unruled white rectangle — which is a picture of a shape, not
 * a reading of a value, and the references make exactly this distinction (their easing panel labels
 * `acceleration` against `time` over a light grid). Without the grid there is no way to see that two
 * keys are level, and without the axis words there is no way to know which way is "more".
 */
export function CurveCanvas({ valueAxis, timeAxis, children, caption, hint, ...rest }: CurveCanvasProps) {
  const { width, height, left, top, right, bottom } = CURVE_VIEWBOX;
  const columns = Array.from({ length: TIMELINE_DIVISIONS + 1 }, (_, i) => left + ((right - left) * i) / TIMELINE_DIVISIONS);
  const rows = Array.from({ length: 5 }, (_, i) => top + ((bottom - top) * i) / 4);
  return (
    <div className="mtk-curve" data-testid="animation-curves">
      {(caption !== undefined || hint !== undefined) && (
        <div className="mtk-curve__caption">
          <div>{caption}</div>
          <div className="mtk-curve__hint">{hint}</div>
        </div>
      )}
      <div className="mtk-curve__plot">
        <span className="mtk-curve__axis mtk-curve__axis--value">{valueAxis}</span>
        <svg viewBox={`0 0 ${width} ${height}`} role="group" className="mtk-curve__svg" {...rest}>
          <g aria-hidden="true" className="mtk-curve__grid">
            {columns.map((x) => (
              <line key={`c${x}`} x1={x} y1={top} x2={x} y2={bottom} />
            ))}
            {rows.map((y) => (
              <line key={`r${y}`} x1={left} y1={y} x2={right} y2={y} />
            ))}
          </g>
          {children}
        </svg>
        <span className="mtk-curve__axis mtk-curve__axis--time">{timeAxis}</span>
      </div>
    </div>
  );
}

/** The one place a curve's own geometry is stated, so the plot, the key hit areas and the tangent
 *  handles cannot disagree about where a value lands. */
export function curvePoint(
  item: { tick: number; value: number },
  bounds: { minTick: number; tickSpan: number; minValue: number; valueSpan: number },
): { x: number; y: number } {
  const { left, right, top, bottom } = CURVE_VIEWBOX;
  return {
    x: left + ((item.tick - bounds.minTick) / bounds.tickSpan) * (right - left),
    y: bottom - ((item.value - bounds.minValue) / bounds.valueSpan) * (bottom - top),
  };
}

