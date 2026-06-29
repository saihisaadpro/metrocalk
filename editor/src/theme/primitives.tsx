//! Reusable UI primitives (M14.1 / ADR-057) — the small shared control set every editor surface builds on,
//! so a button/panel/field looks and behaves the same everywhere (and a restyle is one edit, not 28). The
//! interactive states that inline styles can't express (hover/pressed/disabled/focus-ring) live in the
//! `mtk-*` classes in `theme/global.css`; these components just pick the right class + forward the stable
//! `id`/`data-testid` the prompt-40 e2e + Vitest key on. Non-colour layout values come from `theme/tokens`.

import type { ButtonHTMLAttributes, CSSProperties, ReactNode, InputHTMLAttributes } from "react";
import { color, radius, space, font, fontSize, text } from "./tokens";

/** A data-* / id passthrough the card/icon primitives accept for the stable e2e/Vitest hooks. */
type DataAttrs = { id?: string; title?: string; "data-testid"?: string; "data-id"?: string; "data-source"?: string; "data-kind"?: string };

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "toggle";

export interface ButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "className"> {
  variant?: ButtonVariant;
  /** Toggle-on state (drives `.is-active` → the accent fill, so live tool/snap/space state is unmistakable). */
  active?: boolean;
  /** Tighter padding/size for dense toolbars. */
  compact?: boolean;
  /** Icon-only sizing. */
  icon?: boolean;
  children?: ReactNode;
}

/** The one button. Variants: primary · secondary · ghost · danger · toggle (+ `compact`/`icon`/`active`).
 *  Real hover/pressed/disabled/focus states come from the `.mtk-btn*` classes (global.css). */
export function Button({ variant = "secondary", active = false, compact = false, icon = false, children, style, ...rest }: ButtonProps) {
  const cls = [
    "mtk-btn",
    `mtk-btn--${variant}`,
    compact && "mtk-btn--compact",
    icon && "mtk-btn--icon",
    variant === "toggle" && active && "is-active",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button className={cls} style={style} {...rest}>
      {children}
    </button>
  );
}

/** A coherent panel region: opaque panel background + a hairline border, laid out as a flex column. The
 *  opaque background is deliberate — panels paint their own bg so only the viewport stays a transparent hole
 *  for the wgpu composite (ADR-008). */
export function Panel({ children, style, scroll = false, ...rest }: { children: ReactNode; style?: CSSProperties; scroll?: boolean } & { "data-testid"?: string; id?: string }) {
  return (
    <div
      className={scroll ? "mtk-scroll" : undefined}
      style={{
        display: "flex",
        flexDirection: "column",
        background: color.bg.panel,
        overflow: scroll ? "auto" : "hidden",
        minHeight: 0,
        ...style,
      }}
      {...rest}
    >
      {children}
    </div>
  );
}

/** A panel's title bar — an uppercased section label, with an optional right-aligned action slot. */
export function PanelHeader({ title, right, style }: { title: ReactNode; right?: ReactNode; style?: CSSProperties }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: space.sm,
        padding: `${space.sm}px ${space.lg}px`,
        borderBottom: `1px solid ${color.border.subtle}`,
        background: color.bg.panel,
        ...style,
      }}
    >
      <span style={text.panelTitle}>{title}</span>
      {right}
    </div>
  );
}

/** A lighter in-panel section label (denser than a PanelHeader; for grouping inside a panel). */
export function SectionHeader({ children, style }: { children: ReactNode; style?: CSSProperties }) {
  return (
    <div style={{ ...text.sectionTitle, padding: `${space.xs}px ${space.lg}px`, ...style }}>{children}</div>
  );
}

/** A scrollable region with a styled scrollbar (never raw browser scrollbars). */
export function ScrollArea({ children, style, ...rest }: { children: ReactNode; style?: CSSProperties } & { "data-testid"?: string; id?: string }) {
  return (
    <div className="mtk-scroll" style={{ overflow: "auto", minHeight: 0, ...style }} {...rest}>
      {children}
    </div>
  );
}

/** A styled numeric field (integrated dark, mono — consumed by the M14.3 inspector; created here so the
 *  primitive layer is complete). */
export function NumericField({ style, ...rest }: Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "className">) {
  return <input type="number" className="mtk-input mtk-input--mono" style={{ width: 72, ...style }} {...rest} />;
}

/** A styled text field (integrated dark) — the shared input the command bar + forms use. */
export function TextField({ style, mono = false, ...rest }: Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "className"> & { mono?: boolean }) {
  return <input type="text" className={mono ? "mtk-input mtk-input--mono" : "mtk-input"} style={style} {...rest} />;
}

/** A small, neutral pill/badge (for live readouts — view label, counts). Not a button. The `title`
 *  carries the plain-language explanation (a requirer's needed cap, a price) — never colour-alone. */
export function Badge({ children, tone = "neutral", style, title }: { children: ReactNode; tone?: "neutral" | "accent" | "warn" | "success"; style?: CSSProperties; title?: string }) {
  const tones: Record<string, CSSProperties> = {
    neutral: { background: color.bg.inset, color: color.text.secondary, borderColor: color.border.default },
    accent: { background: color.accent.subtle, color: color.accent.base, borderColor: color.accent.border },
    warn: { background: color.warn.bg, color: color.warn.text, borderColor: color.warn.border },
    success: { background: color.success.bg, color: color.success.text, borderColor: color.success.border },
  };
  return (
    <span
      title={title}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: space.xs,
        padding: `1px ${space.sm}px`,
        borderRadius: radius.sm,
        border: "1px solid",
        font: font.mono,
        fontSize: fontSize.micro,
        whiteSpace: "nowrap",
        ...tones[tone],
        ...style,
      }}
    >
      {children}
    </span>
  );
}

/** The semantic kind of an entity/asset → a glyph + a deterministic dark-theme hue. Keys off a stable
 *  `kind` string the caller derives from the **real** projection (the relational summary / salient
 *  component) or a catalog item's source/category — never a styled string a test would couple to. */
const ICON_KINDS: Record<string, { glyph: string; hue: number }> = {
  mesh: { glyph: "◆", hue: 210 },
  group: { glyph: "▣", hue: 220 },
  light: { glyph: "☼", hue: 45 },
  camera: { glyph: "◉", hue: 190 },
  requirer: { glyph: "◇", hue: 150 }, // hollow = a needed binding not yet filled
  physics: { glyph: "◍", hue: 270 },
  rule: { glyph: "λ", hue: 30 },
  audio: { glyph: "♪", hue: 330 },
  marketplace: { glyph: "⬡", hue: 265 },
  generated: { glyph: "✦", hue: 285 },
  imported: { glyph: "▤", hue: 175 },
  local: { glyph: "◆", hue: 210 },
  default: { glyph: "◻", hue: 215 },
};

/** A styled type-icon — the graceful fallback when a live thumbnail isn't available (over budget / offline /
 *  the dev/browser build / not yet rendered). A framed, hue-tinted glyph so the panel still reads at a glance.
 *  The `data-kind` is the structured signal a test keys on. */
export function TypeIcon({ kind, size = 40, style }: { kind: string; size?: number; style?: CSSProperties }) {
  const k = ICON_KINDS[kind] ?? ICON_KINDS.default;
  return (
    <span
      data-testid="type-icon"
      data-kind={kind}
      aria-hidden
      style={{
        display: "inline-flex",
        alignItems: "center",
        justifyContent: "center",
        width: size,
        height: size,
        flex: "none",
        fontFamily: font.mono,
        fontSize: Math.round(size * 0.5),
        lineHeight: 1,
        color: `hsl(${k.hue} 55% 70%)`,
        background: `hsl(${k.hue} 32% 16%)`,
        border: `1px solid hsl(${k.hue} 40% 30%)`,
        borderRadius: radius.md,
        ...style,
      }}
    >
      {k.glyph}
    </span>
  );
}

/** One card surface — asset/component cards (M14.2 / ADR-058). Real hover/selected/unavailable/warning
 *  states come from the `.mtk-card` classes (global.css); the metadata layout is the caller's. Renders a
 *  `<button>` so it's keyboard-reachable; `disabled`/`tone:"unavailable"` explains *why it can't* via `title`. */
export function Card({
  selected = false,
  tone = "default",
  disabled = false,
  onClick,
  children,
  style,
  ...rest
}: {
  selected?: boolean;
  tone?: "default" | "warn" | "unavailable";
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
  style?: CSSProperties;
} & DataAttrs) {
  const cls = [
    "mtk-card",
    selected && "is-selected",
    tone === "warn" && "is-warn",
    (tone === "unavailable" || disabled) && "is-unavailable",
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type="button" className={cls} onClick={disabled ? undefined : onClick} disabled={disabled} style={style} {...rest}>
      {children}
    </button>
  );
}
