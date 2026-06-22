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
  /** The CSS `grid-template-columns` — the MIDDLE (stage) is always the flex `1fr` with a min-width. */
  gridColumns: string;
}

/** The stage's protected minimum width (px) — it never shrinks below this; the panels yield first. */
export const STAGE_MIN = 360;
/** Below this width the side panels collapse to icon rails (desktop windowed / split-screen use). */
export const COLLAPSE_BELOW = 900;
/** Below this width the open panels shrink to their compact widths (still open). */
export const COMPACT_BELOW = 1200;
/** The collapsed icon-rail width (px). */
export const RAIL_W = 44;

export function panelLayout(width: number): PanelLayout {
  if (width < COLLAPSE_BELOW) {
    return {
      left: RAIL_W,
      right: RAIL_W,
      collapsed: true,
      gridColumns: `${RAIL_W}px minmax(${STAGE_MIN}px, 1fr) ${RAIL_W}px`,
    };
  }
  if (width < COMPACT_BELOW) {
    return { left: 220, right: 260, collapsed: false, gridColumns: `220px minmax(${STAGE_MIN}px, 1fr) 260px` };
  }
  return { left: 260, right: 320, collapsed: false, gridColumns: `260px minmax(${STAGE_MIN}px, 1fr) 320px` };
}
