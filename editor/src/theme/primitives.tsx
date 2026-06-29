//! Reusable UI primitives (M14.1 / ADR-057) — the small shared control set every editor surface builds on,
//! so a button/panel/field looks and behaves the same everywhere (and a restyle is one edit, not 28). The
//! interactive states that inline styles can't express (hover/pressed/disabled/focus-ring) live in the
//! `mtk-*` classes in `theme/global.css`; these components just pick the right class + forward the stable
//! `id`/`data-testid` the prompt-40 e2e + Vitest key on. Non-colour layout values come from `theme/tokens`.

import { useEffect, useRef, useState } from "react";
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

export interface NumericFieldProps {
  /** The authoritative (committed) value — the field resyncs to it when not being edited/scrubbed. */
  value: number;
  /** Commit a value as a transaction: at pointer-up of a scrub (ONE undo step, not N), on Enter/blur of a
   *  typed value, and on each keyboard nudge. The inspector wires this to `client.setField` (ADR-010). */
  onCommit: (v: number) => void;
  /** Live during a scrub-drag — local visual feedback only (NO IPC, NOT a transaction). */
  onScrub?: (v: number) => void;
  /** Nudge/scrub base step (1 for integers, 0.1 for floats by default). */
  step?: number;
  integer?: boolean;
  min?: number;
  max?: number;
  disabled?: boolean;
  /** Externally-marked invalid/unbound/default state (a red ring — never colour-alone, paired with a title). */
  invalid?: boolean;
  /** Value units per drag pixel (defaults to `step`); Shift = ×10 (coarse), Alt = ×0.1 (fine). */
  scrubSpeed?: number;
  ariaLabel?: string;
  title?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/** The M14.1 styled numeric field, upgraded to a real number control (M14.3 / ADR-059): **drag-to-scrub**
 *  (pointer-drag, modifier-scaled), **keyboard nudge** (Arrow ↑/↓, Shift ×10), and **type-to-set**. Each
 *  *commit* is a transaction (`onCommit`) — a whole scrub-drag coalesces into ONE undo step (committed at
 *  pointer-up, not per-move); a typed value commits on Enter/blur; invalid input reverts (no silent zeroing).
 *  Local feedback during the drag streams no IPC. `data-scrubbing` is the structured test signal. */
export function NumericField({
  value,
  onCommit,
  onScrub,
  step,
  integer = false,
  min,
  max,
  disabled = false,
  invalid = false,
  scrubSpeed,
  ariaLabel,
  title,
  style,
  ...rest
}: NumericFieldProps) {
  const testid = (rest as { "data-testid"?: string })["data-testid"];
  const effStep = step ?? (integer ? 1 : 0.1); // integers nudge/scrub by 1, floats by 0.1, unless overridden
  const fmt = (n: number): string => (integer ? String(Math.round(n)) : String(n));
  const clampSnap = (n: number): number => {
    let v = integer ? Math.round(n) : n;
    if (min != null) v = Math.max(min, v);
    if (max != null) v = Math.min(max, v);
    return v;
  };
  const [textVal, setTextVal] = useState(() => fmt(value));
  const [scrubbing, setScrubbing] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const editing = useRef(false); // focused OR scrubbing — don't resync the field out from under the user
  const skipBlurCommit = useRef(false); // a scrub already committed → the trailing blur must NOT re-commit
  const cleanup = useRef<(() => void) | null>(null);
  // Resync to the authoritative value when not actively editing/scrubbing (an external delta / undo / reselect).
  useEffect(() => {
    if (!editing.current) setTextVal(fmt(value));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value, scrubbing]);
  // Remove any in-flight drag listeners if we unmount mid-scrub (the inspector swaps on reselect).
  useEffect(() => () => cleanup.current?.(), []);

  const parsed = textVal.trim() === "" ? null : Number(textVal);
  const validText = parsed !== null && Number.isFinite(parsed) && (!integer || Number.isInteger(parsed));

  // Drag-to-scrub via window listeners (the standard drag pattern — the cursor can leave the field; mouse
  // events carry coordinates reliably across environments). A whole drag commits ONCE at mouse-up.
  function onMouseDown(e: React.MouseEvent) {
    if (disabled || e.button !== 0) return;
    const startX = e.clientX;
    const startVal = value;
    let moved = false;
    let lastVal = startVal;
    const onMove = (ev: MouseEvent) => {
      const dx = ev.clientX - startX;
      if (!moved && Math.abs(dx) < 3) return; // a click, not a scrub (movement threshold)
      if (!moved) {
        moved = true;
        editing.current = true;
        setScrubbing(true);
      }
      ev.preventDefault(); // suppress text selection while scrubbing
      const speed = (scrubSpeed ?? effStep) * (ev.shiftKey ? 10 : ev.altKey ? 0.1 : 1);
      lastVal = clampSnap(startVal + dx * speed);
      setTextVal(fmt(lastVal));
      onScrub?.(lastVal);
    };
    const onUp = () => {
      cleanup.current = null;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      if (moved) {
        editing.current = false;
        setScrubbing(false);
        skipBlurCommit.current = true; // the trailing blur (the field kept focus) must not re-commit
        inputRef.current?.blur();
        onCommit(lastVal); // ONE coalesced transaction for the whole drag (one undo step)
      }
    };
    cleanup.current = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }

  function commitTyped() {
    if (skipBlurCommit.current) {
      skipBlurCommit.current = false;
      return; // a scrub already committed this — don't double-commit on the trailing blur
    }
    if (validText && parsed !== null) onCommit(clampSnap(parsed));
    else setTextVal(fmt(value)); // invalid → revert to the committed value (no silent zeroing)
  }

  return (
    <input
      ref={inputRef}
      type="text"
      inputMode={integer ? "numeric" : "decimal"}
      role="spinbutton"
      aria-label={ariaLabel}
      aria-valuenow={value}
      disabled={disabled}
      title={title ?? (textVal.trim() !== "" && !validText ? `Enter a ${integer ? "whole number" : "number"} — not applied` : undefined)}
      className={"mtk-input mtk-input--mono mtk-numfield" + (invalid || (!validText && textVal.trim() !== "") ? " is-invalid" : "")}
      data-testid={testid}
      data-scrubbing={scrubbing ? "1" : "0"}
      value={textVal}
      onMouseDown={onMouseDown}
      onFocus={() => {
        editing.current = true;
      }}
      onBlur={() => {
        editing.current = false;
        commitTyped();
      }}
      onChange={(e) => setTextVal(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "ArrowUp" || e.key === "ArrowDown") {
          e.preventDefault();
          const mult = e.shiftKey ? 10 : 1;
          onCommit(clampSnap(value + (e.key === "ArrowUp" ? effStep : -effStep) * mult));
        } else if (e.key === "Enter") {
          (e.target as HTMLInputElement).blur(); // commit + release → next Ctrl-Z is a SCENE undo
        }
      }}
      style={{ width: 80, cursor: disabled ? "not-allowed" : "ew-resize", ...style }}
    />
  );
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
