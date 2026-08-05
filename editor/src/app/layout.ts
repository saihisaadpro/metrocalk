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
  if (width < COMPACT_BELOW) {
    return { left: 240, right: 300, collapsed: false, overlay: false, gridColumns: `240px minmax(${STAGE_MIN}px, 1fr) 300px` };
  }
  return { left: 280, right: 340, collapsed: false, overlay: false, gridColumns: `280px minmax(${STAGE_MIN}px, 1fr) 340px` };
}

/** Compose the realized grid after user-controlled dock collapse. Responsive rails still win below their
 * breakpoint; at larger sizes each dock can independently yield without changing the workspace context. */
export function dockGridColumns(layout: PanelLayout, leftCollapsed: boolean, rightCollapsed: boolean): string {
  if (layout.overlay) return "minmax(0, 1fr)";
  const left = layout.collapsed || leftCollapsed ? RAIL_W : layout.left;
  const right = layout.collapsed || rightCollapsed ? RAIL_W : layout.right;
  return `${left}px minmax(${STAGE_MIN}px, 1fr) ${right}px`;
}
