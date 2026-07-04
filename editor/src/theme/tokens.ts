//! Design tokens (M14.1 / ADR-057) — the typed token layer every editor surface consumes, so components
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
    hover: "var(--mtk-bg-hover)",
    active: "var(--mtk-bg-active)",
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
  micro: 10,
  meta: 11,
  body: 12,
  label: 13,
  title: 14,
  heading: 16,
  display: 20,
} as const;

/** Spacing scale (px) — a 2/4 base rhythm. */
export const space = {
  none: 0,
  xxs: 2,
  xs: 4,
  sm: 6,
  md: 8,
  lg: 12,
  xl: 16,
  xxl: 24,
} as const;

/** Corner radii (px). */
export const radius = {
  sm: 3,
  md: 4,
  lg: 6,
  xl: 8,
  pill: 999,
} as const;

/** Elevation (box-shadows) for raised surfaces. */
export const elevation = {
  e1: "0 2px 8px #0006",
  e2: "0 6px 18px #0007",
  e3: "0 8px 24px #0009",
  e4: "0 4px 16px #0008",
} as const;

/** Motion presets (restrained — a serious tool, not a marketing page). */
export const motion = {
  fast: "120ms ease",
  base: "180ms ease",
  slow: "240ms ease",
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
 *  - `menu`    100 — anchored floating menus / dropdowns / context menus / popovers (the default for `Popover`).
 *  - `overlay` 110 — a full-screen scrim/backdrop behind a drawer or modal.
 *  - `drawer`  120 — a slide-in side drawer (sits above its own scrim).
 *  - `badge`   140 — persistent stage badges (e.g. "● PLAYING").
 *  - `toast`   150 — transient notifications (toasts) — above menus so a toast is never hidden by a menu.
 *  - `guard`   200 — blocking modal dialogs / confirmations (the top; must sit over everything, the default
 *                    for `Modal`).
 */
export const z = {
  base: 0,
  chrome: 5,
  sticky: 10,
  menu: 100,
  overlay: 110,
  drawer: 120,
  badge: 140,
  toast: 150,
  guard: 200,
} as const;

/** Text roles — ready-to-spread `CSSProperties` for the common copy kinds (panel title · section title ·
 *  item label · metadata · value · warning · disabled). Keeps text styling consistent without re-deriving. */
export const text = {
  panelTitle: { font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase" as const, color: color.text.secondary },
  sectionTitle: { font: font.ui, fontSize: fontSize.meta, fontWeight: 600, color: color.text.secondary },
  itemLabel: { font: font.ui, fontSize: fontSize.body, color: color.text.primary },
  metadata: { font: font.mono, fontSize: fontSize.meta, color: color.text.muted },
  value: { font: font.mono, fontSize: fontSize.body, color: color.text.primary },
  warning: { font: font.ui, fontSize: fontSize.body, color: color.warn.text },
  disabled: { color: color.text.faint },
} as const;
