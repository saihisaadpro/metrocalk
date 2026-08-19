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

/** Where the bottom dock lives when it is open: a track BELOW the stage, or a sheet OVER it. */
export type DockForm = "docked" | "sheet";

/** THE VERTICAL TWIN OF `dockGridColumns`, AND OF THE ≤620px SIDE DRAWERS.
 *
 * ADR-126 made the dock yield vertically — `max-height: calc(100% - var(--mtk-stage-min))` — and closed
 * by naming what yielding could not answer: **below a certain window height the stage's floor and a
 * usable workspace cannot both hold**, and it left the dock shrinking through that regime "which is
 * honest but is not an answer". Measured at HEAD it was not honest either. The dock reports `is-open`,
 * its tab lights up, and the workspace inside it is **37px of 188 at a 480px window, 1px at 440px, and
 * 1px all the way down to 300px** — a scroll container too short to render a scrollbar, so nothing on
 * screen says the workspace is there. Every layout invariant is silent: R6 wants `clientHeight < 1` and
 * the box is exactly 1, R7 exempts a scroller that has not disabled its scrollbar, and this one has not
 * — it simply has nowhere to draw it. The user clicks a workspace and the editor does nothing, with no
 * reason given: `<ux_quality>` 6, an enabled control that neither acts nor explains.
 *
 * The answer is not new, which is the argument for it. **On the horizontal axis this shell already
 * solves exactly this problem**: below `OVERLAY_BELOW` the side docks stop being tracks beside the stage
 * and become drawers over it, because a dock squeezed past its own minimum is a dock that has been
 * deleted without saying so. Rotate that rule 90° and it reads: when the dock cannot fit BELOW the
 * stage, it floats OVER it. The stage then keeps its whole column and its floor — a sheet the user
 * opened and can close is not the stage collapsing — and the workspace gets a real content box with a
 * real scrollbar. Refusing to open was the other candidate and is worse: it deletes seven workspaces
 * because a window is short, which is the "seven authored, five reachable" defect of ADR-126 with a
 * bigger number.
 *
 * The three inputs are MEASURED, not restated. `barH` and `contentMin` are read from
 * `--mtk-bottom-bar-height` and `--mtk-dock-content-min` on the live element, because both are real
 * variables — the bar is 48px under `(pointer: coarse)` — and a copy of them here would be a second
 * statement of a number that already moves on its own, which is the class of defect ADR-119 through
 * ADR-126 are all about. `stageHeight` is the stage column's measured `clientHeight`, for the reason
 * ADR-125 gave: adding the tracks up against the window is the part nothing was doing.
 *
 * AND A MEASUREMENT OF ZERO IS NOT A SHORT WINDOW — IT IS NO MEASUREMENT. The first version returned
 * `"sheet"` for a zero-height column, which is the *degraded* form, and every environment without
 * layout gets that answer: jsdom, the first render before the observer has run, a hidden tab. The
 * vitest suite found it within a minute — `pipe-forge` vanished from an unrelated end-to-end test,
 * because a shell that believes it is 0px tall floats its dock and withdraws the stage's overlays.
 * That is `absence is not assent` (ADR-118) in a layout function: an unknown must not read as the
 * answer that changes behaviour. The docked form is what the shell does when nothing is wrong, so it
 * is what an unanswered question returns, and the observer corrects it on the first real layout.
 *
 * Pure, so the threshold is unit-testable in jsdom, which has no layout to measure. */
export function dockForm(stageHeight: number, barH: number, contentMin: number): DockForm {
  if (!(stageHeight > 0)) return "docked"; // not measured yet (and NaN, which compares false either way)
  return stageHeight >= STAGE_MIN + barH + contentMin ? "docked" : "sheet";
}
