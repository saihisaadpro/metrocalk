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

/** z-layer scale — one ladder so overlays never fight (matches the existing App/menu/drawer/toast order). */
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
