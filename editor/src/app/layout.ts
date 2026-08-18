//! Responsive panel layout (M10.10 / C8) — the **stage is layout-priority**: the side panels yield (and
//! collapse to icon rails below a breakpoint) so the viewport never collapses first. A PURE function of
//! the window width, so the stage-priority rule is unit-testable without real layout (jsdom has none): the
//! middle (stage) track is always `minmax(STAGE_MIN, 1fr)` — the flex region with a protected floor —
//! while the side tracks are fixed px that shrink (and collapse to rails) as width drops.

export interface PanelLayout {
  /** Left panel width in px (the rail width when `collapsed`). */
  left: number;
  /** Right panel width in px (the rail width when `collapsed`). */
  right: number;
  /** Below the breakpoint the side panels collapse to icon rails so the stage keeps the space. */
  collapsed: boolean;
  /** On phone-width windows even the rails yield; the header opens both docks as focus-managed overlays. */
  overlay: boolean;
  /** The CSS `grid-template-columns` — the MIDDLE (stage) is always the flex `1fr` with a min-width. */
  gridColumns: string;
}

/** The stage's protected minimum width (px) — it never shrinks below this; the panels yield first. */
export const STAGE_MIN = 320;
/** Below this width the side panels collapse to icon rails (desktop windowed / split-screen use). */
export const COLLAPSE_BELOW = 980;
/** Below this width the shell becomes a one-column stage with modal dock drawers. */
export const OVERLAY_BELOW = 620;
/** Below this width the open panels shrink to their compact widths (still open). */
export const COMPACT_BELOW = 1200;
/** The collapsed icon-rail width (px). */
export const RAIL_W = 44;
/** The Engines rail width (px) — the always-present index of sub-engines. */
export const ENGINE_RAIL_W = 132;
/** The Engines rail when the window is too narrow to afford labels. */
export const ENGINE_RAIL_W_COMPACT = 56;

export function panelLayout(width: number): PanelLayout {
  if (width < OVERLAY_BELOW) {
    return {
      left: 0,
      right: 0,
      collapsed: true,
      overlay: true,
      gridColumns: "minmax(0, 1fr)",
    };
  }
  if (width < COLLAPSE_BELOW) {
    return {
      left: RAIL_W,
      right: RAIL_W,
      collapsed: true,
      overlay: false,
      gridColumns: `${RAIL_W}px minmax(${STAGE_MIN}px, 1fr) ${RAIL_W}px`,
    };
  }
  // The LEFT column is the sub-engine you are working in, so it gets the width; the right column is a
  // read-out of the selection and needs less. Before the Engines rail the left column was only an outliner
  // and the ratio was the other way round — which left the terrain workspace overflowing its 280px dock the
  // moment it moved out of the Inspector.
  if (width < COMPACT_BELOW) {
    return { left: 300, right: 260, collapsed: false, overlay: false, gridColumns: `300px minmax(${STAGE_MIN}px, 1fr) 260px` };
  }
  return { left: 340, right: 300, collapsed: false, overlay: false, gridColumns: `340px minmax(${STAGE_MIN}px, 1fr) 300px` };
}

/** Compose the realized grid after user-controlled dock collapse. Responsive rails still win below their
 * breakpoint; at larger sizes each dock can independently yield without changing the workspace context.
 *
 * `width` is the window width, and it is REQUIRED rather than optional because the whole point of this
 * function is that the four tracks have to fit inside something. Without it the tracks were composed from
 * a table and never added up: at 1000 px with the Inspector pinned open the template read
 * `132px 300px minmax(320px, 1fr) 260px` — 1012 px of tracks in a 1000 px window — and the browser paid
 * for it by painting the Inspector 12 px off the right edge of the screen, unreachable and unscrollable.
 * The unit test was green the whole time and was right to be: it compares the string this function
 * returns, and the string was exactly the intended one. Adding it up is the part nothing was doing.
 *
 * The overflow is resolved the way the product rule says it must be — **the panels yield, the stage does
 * not** — and in the priority the layout already states: the right dock is a read-out of the selection and
 * gives first, the left dock is the sub-engine you are working in and gives second, and neither ever
 * shrinks past its rail width, because a dock narrower than its own rail is not a dock. */
export function dockGridColumns(
  layout: PanelLayout,
  leftCollapsed: boolean,
  rightCollapsed: boolean,
  width: number,
): string {
  // The Engines rail is ALWAYS the first track, even in the collapsed layouts: it is the index of what the
  // editor can do, and an index you have to open a drawer to reach is not an index. Only in the phone-width
  // overlay layout — where the whole shell is one column — does it fold into the header drawers.
  const engines = layout.collapsed ? ENGINE_RAIL_W_COMPACT : ENGINE_RAIL_W;
  if (layout.overlay) return "minmax(0, 1fr)";
  let left = layout.collapsed || leftCollapsed ? RAIL_W : layout.left;
  let right = layout.collapsed || rightCollapsed ? RAIL_W : layout.right;

  let over = engines + left + STAGE_MIN + right - width;
  if (over > 0) {
    const give = Math.min(over, right - RAIL_W);
    right -= give;
    over -= give;
  }
  if (over > 0) {
    left -= Math.min(over, left - RAIL_W);
  }
  // If even two rails and the floor do not fit, there is nothing left to take from the panels and the
  // stage absorbs the remainder — but that is below OVERLAY_BELOW, where this branch is unreachable.
  return `${engines}px ${left}px minmax(${STAGE_MIN}px, 1fr) ${right}px`;
}
