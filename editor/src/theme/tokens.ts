//! Design tokens (ADR-090 light-first Engine UI/UX Architecture Constitution; extends ADR-035) — the
//! typed token layer every editor surface consumes, so components
//! carry **no magic hex / magic numbers** (the ADR-035 contract, made systematic). Colours reference the
//! CSS custom properties defined in `theme/global.css` (the single source of truth — `var(--mtk-*)`), so a
//! palette change is one edit and there's no hex duplicated across TS + CSS. The non-colour scales
//! (space/radii/type/elevation/motion/z) live here as plain values. Inline styles read these; pseudo-state
//! styling (hover/active/focus/scrollbars) lives in `global.css` (the `mtk-*` classes + the `Button`/`Panel`
//! primitives). jsdom doesn't load the CSS, but tests key off structured signals, never resolved colour.

/** Colour roles — `var(--mtk-*)` references (resolved from `theme/global.css`). */
export const color = {
  bg: {
    base: "var(--mtk-bg-base)",
    panel: "var(--mtk-bg-panel)",
    raised: "var(--mtk-bg-raised)",
    inset: "var(--mtk-bg-inset)",
    input: "var(--mtk-bg-input)",
    canvas: "var(--mtk-bg-canvas)",
    hover: "var(--mtk-bg-hover)",
    active: "var(--mtk-bg-active)",
    scrim: "var(--mtk-bg-scrim)",
  },
  border: {
    subtle: "var(--mtk-border-subtle)",
    default: "var(--mtk-border)",
    strong: "var(--mtk-border-strong)",
  },
  text: {
    primary: "var(--mtk-text)",
    secondary: "var(--mtk-text-secondary)",
    muted: "var(--mtk-text-muted)",
    faint: "var(--mtk-text-faint)",
  },
  accent: {
    base: "var(--mtk-accent)",
    solid: "var(--mtk-accent-solid)",
    solidHover: "var(--mtk-accent-solid-hover)",
    border: "var(--mtk-accent-border)",
    subtle: "var(--mtk-accent-subtle)",
    ring: "var(--mtk-ring)",
    onSolid: "var(--mtk-on-accent)",
  },
  overlay: {
    scrim: "var(--mtk-overlay-scrim)",
  },
  success: { text: "var(--mtk-success)", solid: "var(--mtk-success-solid)", border: "var(--mtk-success-border)", bg: "var(--mtk-success-bg)" },
  warn: { text: "var(--mtk-warn)", solid: "var(--mtk-warn-solid)", border: "var(--mtk-warn-border)", bg: "var(--mtk-warn-bg)" },
  danger: { text: "var(--mtk-danger)", solid: "var(--mtk-danger-solid)", border: "var(--mtk-danger-border)", bg: "var(--mtk-danger-bg)" },
  info: { text: "var(--mtk-info)", border: "var(--mtk-info-border)", bg: "var(--mtk-info-bg)" },
  token: "var(--mtk-token)",
} as const;

/** Font stacks (a strong technical UI sans + a compact mono for ids/values/tokens/diagnostics). */
export const font = {
  ui: "var(--mtk-font-ui)",
  mono: "var(--mtk-font-mono)",
} as const;

/** Type scale (px). */
export const fontSize = {
  micro: 11,
  meta: 12,
  body: 13,
  label: 13,
  title: 15,
  heading: 18,
  display: 22,
  hero: 28,
} as const;

/** Line-height roles keep dense technical data legible without making the workbench feel loose. */
export const lineHeight = {
  compact: 1.2,
  body: 1.45,
  relaxed: 1.6,
} as const;

/** Spacing scale (px) — a 2/4 base rhythm with room for calm, high-level composition. */
export const space = {
  none: 0,
  xxs: 2,
  xs: 4,
  sm: 6,
  md: 8,
  lg: 12,
  xl: 16,
  xxl: 24,
  xxxl: 32,
  huge: 40,
} as const;

/** Corner radii (px). Restrained at control scale, softer for floating and feature surfaces. */
export const radius = {
  none: 0,
  sm: 4,
  md: 6,
  lg: 8,
  xl: 12,
  xxl: 16,
  pill: 999,
} as const;

/** Elevation for a light workbench: broad ambient shadows plus a restrained key shadow. */
export const elevation = {
  e0: "none",
  e1: "0 1px 2px rgba(21, 34, 50, 0.08), 0 2px 6px rgba(21, 34, 50, 0.05)",
  e2: "0 4px 12px rgba(21, 34, 50, 0.10), 0 1px 3px rgba(21, 34, 50, 0.08)",
  e3: "0 12px 32px rgba(21, 34, 50, 0.14), 0 3px 8px rgba(21, 34, 50, 0.08)",
  e4: "0 20px 56px rgba(21, 34, 50, 0.18), 0 5px 14px rgba(21, 34, 50, 0.10)",
  inset: "inset 0 1px 2px rgba(21, 34, 50, 0.06)",
} as const;

/** Motion presets (restrained — a serious tool, not a marketing page). */
export const motion = {
  instant: "80ms cubic-bezier(0.2, 0, 0, 1)",
  fast: "120ms cubic-bezier(0.2, 0, 0, 1)",
  base: "180ms cubic-bezier(0.2, 0, 0, 1)",
  slow: "240ms cubic-bezier(0.2, 0, 0, 1)",
  duration: {
    instant: 80,
    fast: 120,
    base: 180,
    slow: 240,
  },
  easing: {
    standard: "cubic-bezier(0.2, 0, 0, 1)",
    enter: "cubic-bezier(0, 0, 0, 1)",
    exit: "cubic-bezier(0.3, 0, 1, 1)",
  },
} as const;

/** Shared control geometry. Coarse-pointer overrides live in `global.css`, not individual controls. */
export const control = {
  height: {
    compact: 30,
    default: 34,
    comfortable: 40,
    touch: 44,
  },
  inlinePadding: {
    compact: 8,
    default: 12,
    comfortable: 16,
  },
  icon: {
    sm: 14,
    md: 16,
    lg: 20,
    xl: 24,
  },
} as const;

/**
 * **The app's z-layer ladder — one source of truth so overlays never fight.**
 *
 * THE RULE (read this before adding any floating UI): a raised `z-index` does NOT let an element escape an
 * ancestor's `overflow: hidden` (it gets CLIPPED) or an ancestor stacking context (it gets BURIED). So any UI
 * that floats above the layout — dropdown, popover, context menu, modal, tooltip, toast — MUST be rendered
 * through a PORTAL to `document.body`, never as a `position: absolute` child of a normal panel. Use the shared
 * primitives in [`theme/Popover.tsx`] (`Popover` for anchored menus/context menus, `Modal` for centered
 * dialogs); they portal, are edge-aware, and dismiss on Escape / outside-click. Do NOT hand-roll a floating
 * `position: absolute` panel inside a toolbar/panel row — that is exactly the bug that hid the File menu behind
 * the header (the header row is `overflow: hidden`).
 *
 * The ladder (low → high). Pick a token, never a raw number:
 *  - `base`    0   — normal document flow.
 *  - `chrome`  5   — in-viewport chrome pinned over the stage (viewport toolbar, empty-state).
 *  - `sticky`  10  — sticky headers within a scroll region.
 *  - `overlay` 110 — a full-screen scrim/backdrop behind a drawer or modal.
 *  - `drawer`  120 — a slide-in side drawer (sits above its own scrim).
 *  - `menu`    130 — anchored menus/popovers, including those invoked from inside a drawer.
 *  - `badge`   140 — persistent stage badges (e.g. "● PLAYING").
 *  - `toast`   150 — transient notifications (toasts) — above menus so a toast is never hidden by a menu.
 *  - `guard`   200 — blocking modal dialogs / confirmations (the top; must sit over everything, the default
 *                    for `Modal`).
 */
export const z = {
  base: 0,
  chrome: 5,
  sticky: 10,
  overlay: 110,
  drawer: 120,
  menu: 130,
  badge: 140,
  toast: 150,
  guard: 200,
} as const;

/** Text roles — ready-to-spread `CSSProperties` for the common copy kinds (panel title · section title ·
 *  item label · metadata · value · warning · disabled). Keeps text styling consistent without re-deriving. */
export const text = {
  panelTitle: { font: font.ui, fontSize: fontSize.meta, fontWeight: 650, lineHeight: lineHeight.compact, letterSpacing: 0.15, color: color.text.primary },
  sectionTitle: { font: font.ui, fontSize: fontSize.meta, fontWeight: 650, lineHeight: lineHeight.compact, color: color.text.secondary },
  itemLabel: { font: font.ui, fontSize: fontSize.body, lineHeight: lineHeight.body, color: color.text.primary },
  metadata: { font: font.mono, fontSize: fontSize.meta, lineHeight: lineHeight.body, color: color.text.muted },
  value: { font: font.mono, fontSize: fontSize.body, lineHeight: lineHeight.body, color: color.text.primary },
  warning: { font: font.ui, fontSize: fontSize.body, lineHeight: lineHeight.body, color: color.warn.text },
  disabled: { color: color.text.faint },
} as const;
