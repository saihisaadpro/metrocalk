//! Reusable UI primitives (M14.1 / ADR-057) — the small shared control set every editor surface builds on,
//! so a button/panel/field looks and behaves the same everywhere (and a restyle is one edit, not 28). The
//! interactive states that inline styles can't express (hover/pressed/disabled/focus-ring) live in the
//! `mtk-*` classes in `theme/global.css`; these components just pick the right class + forward the stable
//! `id`/`data-testid` the prompt-40 e2e + Vitest key on. Non-colour layout values come from `theme/tokens`.

import { forwardRef, useEffect, useRef, useState } from "react";
import type {
  ButtonHTMLAttributes,
  CSSProperties,
  InputHTMLAttributes,
  LabelHTMLAttributes,
  AriaRole,
  ReactNode,
  SelectHTMLAttributes,
  TextareaHTMLAttributes,
} from "react";
import { Icon } from "./icons";
import { color, radius, space, font, fontSize, text } from "./tokens";

/** A data-* / id passthrough the card/icon primitives accept for the stable e2e/Vitest hooks. */
type DataAttrs = { id?: string; title?: string; "data-testid"?: string; "data-id"?: string; "data-source"?: string; "data-kind"?: string };

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "toggle";
export type ControlSize = "compact" | "default" | "comfortable";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** `data-*` passthrough. JSX special-cases these on intrinsic elements, but a `ButtonProps` *object*
   *  (e.g. `triggerProps`) is excess-property-checked — so the e2e/Vitest hooks need declaring here. */
  [dataAttr: `data-${string}`]: unknown;
  variant?: ButtonVariant;
  /** Toggle-on state (drives `.is-active` → the accent fill, so live tool/snap/space state is unmistakable). */
  active?: boolean;
  /** Tighter padding/size for dense toolbars. */
  compact?: boolean;
  /** Named target size. Prefer this over one-off height/padding values. */
  size?: ControlSize;
  /** Icon-only sizing. */
  icon?: boolean;
  /** **WHY THIS CONTROL IS REFUSING, in the user's words** — rendered as the button's `title`, which is
   *  its accessible description.
   *
   *  WHY IT LIVES ON THE PRIMITIVE. `disabledReason` was already this repository's convention in six
   *  places — `PopupMenuItem`, `ContextMenu`, `CommandPalette`, `AuthoringToolbar`, `ViewportToolbar`,
   *  `AssetLabPanel` — and in none of them is it optional-by-accident: three of the six also PAINT it.
   *  The one thing missing was the shared `Button` itself, which is what a panel reaches for when it
   *  is not building a menu. So every surface that used the primitive directly had no path to state a
   *  reason and, measured across all 26 shot scenes, three of them did not: `+ Marker`, `+ Event` and
   *  Terrain's `Build it` went dark with no sentence anywhere. A convention that the most-used control
   *  cannot express is a convention that is only followed where it was invented.
   *
   *  It does NOT imply `disabled` — the two stay orthogonal, exactly as `PopupMenuItem` has them, so a
   *  reason can be computed once and handed over beside the boolean that consumed it. The gate that
   *  makes it non-optional is R9 in `scripts/shots/shoot.mjs`; this prop is only what makes obeying it
   *  the shortest path. */
  disabledReason?: string;
  children?: ReactNode;
}

/** The one button. Variants: primary · secondary · ghost · danger · toggle (+ `compact`/`icon`/`active`).
 *  Real hover/pressed/disabled/focus states come from the `.mtk-btn*` classes (global.css). */
export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button({
  variant = "secondary",
  active = false,
  compact = false,
  size = "default",
  icon = false,
  disabledReason,
  title,
  type = "button",
  role,
  "aria-checked": ariaChecked,
  "aria-pressed": ariaPressed,
  "aria-selected": ariaSelected,
  className,
  children,
  style,
  ...rest
}, ref) {
  const resolvedSize = compact ? "compact" : size;
  const exposesAnotherSelectionPattern =
    role != null || ariaChecked != null || ariaSelected != null;
  const cls = [
    "mtk-btn",
    `mtk-btn--${variant}`,
    `mtk-btn--${resolvedSize}`,
    icon && "mtk-btn--icon",
    variant === "toggle" && active && "is-active",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  // The reason only speaks while the control is actually refusing. Left on an enabled button it would
  // be a tooltip claiming an unavailability that is over — the stale-status-line bug `<ux_quality>` 2
  // names by hand — and an explicit `title` still wins, because a caller that wrote one meant it.
  // BOTH SPELLINGS OF "OFF", because the codebase uses both and for good reasons: `disabled` for a
  // plain control, `aria-disabled` where the item must stay focusable so a keyboard user can reach the
  // explanation at all (`ContextMenu`, `PopupMenuItem`). A reason that only attached to one of the two
  // would be missing from exactly the surfaces that went to the trouble of staying reachable.
  const refusing = rest.disabled === true || rest["aria-disabled"] === true || rest["aria-disabled"] === "true";
  const resolvedTitle = title ?? (refusing && disabledReason ? disabledReason : undefined);
  return (
    <button
      ref={ref}
      type={type}
      role={role}
      title={resolvedTitle}
      aria-checked={ariaChecked}
      aria-pressed={ariaPressed ?? (variant === "toggle" && !exposesAnotherSelectionPattern ? active : undefined)}
      aria-selected={ariaSelected}
      className={cls}
      style={style}
      {...rest}
    >
      {children}
    </button>
  );
});

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

/** A lighter in-panel section label (denser than a PanelHeader; for grouping inside a panel).
 *
 *  IT USED TO CARRY ITS OWN 12px INLINE PADDING, and every one of its ten call sites is inside a
 *  container that already insets its content — a `DisclosureSection` body, a padded `ScrollArea`, a
 *  padded column. So the heading always started 12px to the RIGHT of the rows it labelled, which is
 *  the arrangement that makes a heading read as belonging to something else. Two things could have
 *  been done about it: pass `padding: 0` at ten call sites, which is one value invented ten times and
 *  the constitution's root-cause rule verbatim; or decide it once here. The label now states only its
 *  own vertical rhythm and takes its indent from whatever it is inside, like every other block does. */
export function SectionHeader({ children, style }: { children: ReactNode; style?: CSSProperties }) {
  return (
    <div style={{ ...text.sectionTitle, paddingBlock: space.xs, ...style }}>{children}</div>
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
  /** DOM id, so a shared `PropertyRow` label can point at the control it names (accessibility §10). */
  id?: string;
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
  id,
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
      id={id}
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

/**
 * A styled multi-line text field — the same input contract as [`TextField`], for prose.
 *
 * Shares the `mtk-input` class so focus, density and disabled states cannot drift from the single-line field.
 * `resize: vertical` because a description can be longer than its box, and taking that away would make the
 * author scroll a three-line window instead of opening it up.
 */
export function TextArea({
  style,
  mono = false,
  rows = 3,
  ...rest
}: Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "className"> & { mono?: boolean }) {
  return (
    <textarea
      className={mono ? "mtk-input mtk-input--mono" : "mtk-input"}
      rows={rows}
      style={{ resize: "vertical", lineHeight: 1.45, ...style }}
      {...rest}
    />
  );
}

/** A small, neutral pill/badge (for live readouts — view label, counts). Not a button. The `title`
 *  carries the plain-language explanation (a requirer's needed cap, a price) — never colour-alone. */
/** Search is a first-class editor interaction with one shared focus, density and clearing contract. */
export const SearchField = forwardRef<HTMLInputElement, Omit<InputHTMLAttributes<HTMLInputElement>, "type">>(
  function SearchField({ className, style, ...rest }, ref) {
    return (
      <input
        ref={ref}
        type="search"
        className={["mtk-input", "mtk-search", className].filter(Boolean).join(" ")}
        style={style}
        {...rest}
      />
    );
  },
);

/** Shared native select styling preserves platform semantics while removing subsystem-specific controls. */
export function SelectField({
  className,
  children,
  style,
  ...rest
}: SelectHTMLAttributes<HTMLSelectElement>) {
  return (
    <select
      className={["mtk-input", "mtk-select", className].filter(Boolean).join(" ")}
      style={style}
      {...rest}
    >
      {children}
    </select>
  );
}

/** Multiline input from the same control family as text, numeric, search and select fields. */
export function TextAreaField({
  className,
  style,
  ...rest
}: TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={["mtk-input", "mtk-textarea", className].filter(Boolean).join(" ")}
      style={style}
      {...rest}
    />
  );
}

export interface PropertyRowProps {
  label: ReactNode;
  children: ReactNode;
  help?: ReactNode;
  actions?: ReactNode;
  /** The value's unit, in its own column (ADR-136) — `m`, `°`, `kg`, `m/s`.
   *
   *  A COLUMN, NOT A SUFFIX, and the first capture of the populated inspector is why. Rendered inside
   *  the control cell it takes width from the input, so a `kg` row's box is narrower than a `×` row's
   *  and the sheet's right edge goes ragged — visible in `inspector-real-vocabulary` before this, and
   *  invisible to every assertion on that scene, because a ragged edge is a set of boxes that are each
   *  exactly where their own row put them. */
  unit?: ReactNode;
  /** Required control id: a visible inspector label must always name the value it edits. */
  htmlFor: string;
  className?: string;
  style?: CSSProperties;
  labelProps?: Omit<LabelHTMLAttributes<HTMLLabelElement>, "htmlFor">;
  "data-testid"?: string;
}

/**
 * Shared inspector anatomy: readable label, value control, contextual actions and concise guidance.
 * Value state and transactions stay with the authoritative subsystem store.
 */
export function PropertyRow({
  label,
  children,
  help,
  actions,
  unit,
  htmlFor,
  className,
  style,
  labelProps,
  ...rest
}: PropertyRowProps) {
  return (
    <div className={["mtk-property-row", className].filter(Boolean).join(" ")} style={style} {...rest}>
      <label className="mtk-property-row__label" htmlFor={htmlFor} {...labelProps}>
        {label}
      </label>
      <div className="mtk-property-row__control">{children}</div>
      {/* Both trailing cells are ALWAYS emitted, empty when there is nothing to put in them. A cell
          that appears only when it has content is a cell that changes the row's geometry when it does
          — which is how four of nine Transform rows ended up with a narrower input than the other
          five, purely because their value differed from its default and so offered a reset. */}
      <div className="mtk-property-row__unit" aria-hidden={unit == null || undefined} data-testid={unit != null ? "prop-unit" : undefined}>
        {unit}
      </div>
      <div className="mtk-property-row__actions">{actions}</div>
      {help != null && <div className="mtk-property-row__help">{help}</div>}
    </div>
  );
}

export interface SurfaceProps {
  children: ReactNode;
  tone?: "panel" | "floating" | "inset";
  className?: string;
  style?: CSSProperties;
  role?: AriaRole;
  id?: string;
  "data-testid"?: string;
}

/** Token-owned visual surface for panels, floating tools and inset wells. */
export function Surface({
  children,
  tone = "panel",
  className,
  ...rest
}: SurfaceProps) {
  return (
    <div className={["mtk-surface", `mtk-surface--${tone}`, className].filter(Boolean).join(" ")} {...rest}>
      {children}
    </div>
  );
}

export function Badge({ children, tone = "neutral", style, title }: { children: ReactNode; tone?: "neutral" | "accent" | "warn" | "success"; style?: CSSProperties; title?: string }) {
  const tones: Record<string, CSSProperties> = {
    neutral: { background: color.bg.inset, color: color.text.secondary, borderColor: color.border.default },
    accent: { background: color.accent.subtle, color: color.accent.base, borderColor: color.accent.border },
    warn: { background: color.warn.bg, color: color.warn.text, borderColor: color.warn.border },
    success: { background: color.success.bg, color: color.success.text, borderColor: color.success.border },
  };
  return (
    <span
      className="mtk-badge"
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

/** THE FIVE TRANSPORT ICONS AND THE INLINE RESET NOW LIVE IN [`theme/icons.tsx`] — WITH THE OTHER NINETY.
 *
 *  They were drawn here first, and for a good reason: the transport used the Unicode media-control
 *  characters, `animation-timeline-tracks.png` photographed four buttons of which three were empty
 *  boxes, and inline SVG has no font dependency at all. What that fix could not do from inside this
 *  file was generalise — so `▤` stayed on the Scene tab, `⬡` on Model, `⌕` in the search field, and
 *  thirty-five colour emoji kept arriving from the Rust catalogs, none of which is a coverage bug and
 *  every one of which is the same design bug. `Icon` is the general answer: one named vocabulary, one
 *  grid, one stroke weight, one size token. `TransportIcon`/`RevertIcon` are `<Icon name="play" />`
 *  and `<Icon name="revert" />` now; keeping two icon components would have been the very duplication
 *  the set exists to end.
 */

/** The semantic kind of an entity/asset → a deterministic hue. The MARK comes from the one icon set
 *  under the same name (`mesh`, `light`, `camera`, …), which is why this table no longer carries a
 *  character: a `kind` and an icon name are the same vocabulary, and stating it twice is how `◆` ended
 *  up meaning both `mesh` and `local` while `◇` meant `requirer` here and `logic` on the rail.
 *
 *  Keys off a stable `kind` string the caller derives from the **real** projection (the relational
 *  summary / salient component) or a catalog item's source/category — never a styled string a test
 *  would couple to. */
const ICON_KIND_HUES: Record<string, number> = {
  mesh: 210,
  group: 220,
  light: 45,
  camera: 190,
  requirer: 150, // the dashed outline = a needed binding not yet filled
  character: 15,
  physics: 270,
  rule: 30,
  audio: 330,
  marketplace: 265,
  generated: 285,
  imported: 175,
  local: 210,
  default: 215,
};

/** A styled type-icon — the graceful fallback when a live thumbnail isn't available (over budget / offline /
 *  the dev/browser build / not yet rendered). A framed, hue-tinted mark so the panel still reads at a glance.
 *  The `data-kind` is the structured signal a test keys on; `data-icon` (inside) is the drawing's. */
export function TypeIcon({ kind, size = 40, style }: { kind: string; size?: number; style?: CSSProperties }) {
  const hue = ICON_KIND_HUES[kind] ?? ICON_KIND_HUES.default;
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
        color: `hsl(${hue} 46% 34%)`,
        background: `hsl(${hue} 58% 95%)`,
        border: `1px solid hsl(${hue} 34% 82%)`,
        borderRadius: radius.md,
        ...style,
      }}
    >
      <Icon name={kind} fallback="shape" size={Math.round(size * 0.52)} />
    </span>
  );
}

/** One card surface — asset/component cards (M14.2 / ADR-058). Real hover/selected/unavailable/warning
 *  states come from the `.mtk-card` classes (global.css); the metadata layout is the caller's. Renders a
 *  `<button>` so it's keyboard-reachable; `disabled`/`tone:"unavailable"` explains *why it can't* via `title`.
 *
 *  IT HAD NO CALLERS AND A HAND-WRITTEN TWIN. `Requirers` — the quick-pick list this component's own ADR
 *  was written for — spelt `<button type="button" className="cand mtk-card">` out by hand, because the
 *  shared version could not carry the `cand` hook the prompt-40 page object keys on. That is the whole
 *  reason `className` is here: a shared control that cannot take the caller's stable hook is a shared
 *  control the caller cannot use, and the copy that replaces it is where the states then drift. */
export function Card({
  selected = false,
  tone = "default",
  disabled = false,
  onClick,
  children,
  className,
  style,
  ...rest
}: {
  selected?: boolean;
  tone?: "default" | "warn" | "unavailable";
  disabled?: boolean;
  onClick?: () => void;
  children: ReactNode;
  /** Caller-owned hooks/modifiers, composed AFTER the state classes so a stable selector survives. */
  className?: string;
  style?: CSSProperties;
} & DataAttrs) {
  const cls = [
    "mtk-card",
    selected && "is-selected",
    tone === "warn" && "is-warn",
    (tone === "unavailable" || disabled) && "is-unavailable",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <button type="button" className={cls} onClick={disabled ? undefined : onClick} disabled={disabled} style={style} {...rest}>
      {children}
    </button>
  );
}

// ── Slider ────────────────────────────────────────────────────────────────────────────────────────

export interface SliderProps
  extends Omit<InputHTMLAttributes<HTMLInputElement>, "type" | "value" | "min" | "max"> {
  value: number;
  min: number;
  max: number;
}

/**
 * The one range control. Every `type="range"` in the engine was a RAW BROWSER CONTROL before this —
 * a hairline track and an OS thumb sitting in toolbars whose every other control is a rounded 30px
 * surface — because a slider primitive did not exist and `global.css` said only `accent-color`.
 *
 * WHY THE FILL IS COMPUTED HERE. WebKit has no `::-moz-range-progress`, so the filled part of the
 * track is a hard-stopped gradient whose stop this component sets as `--mtk-slider-fill` from the
 * value it is already re-rendering with. Firefox ignores the variable and uses its own pseudo-element.
 * That is the whole reason `value`/`min`/`max` are required rather than optional passthroughs: a
 * slider that does not know its own extent cannot paint how far along it is, and an uncontrolled
 * range is exactly the control that then looks empty at 90%.
 */
export function Slider({ value, min, max, className, style, ...rest }: SliderProps) {
  const span = max - min;
  const ratio = span > 0 ? (value - min) / span : 0;
  const fill = `${Math.max(0, Math.min(1, ratio)) * 100}%`;
  return (
    <input
      type="range"
      className={["mtk-slider", className].filter(Boolean).join(" ")}
      value={value}
      min={min}
      max={max}
      style={{ ...style, ["--mtk-slider-fill" as string]: fill }}
      {...rest}
    />
  );
}

/**
 * A slider with the two things a bare bar never has: what it controls, and what it currently reads.
 * `<ux_quality>` 4 (plain language) and the constitution's "unit labels" are both about this — a
 * zoom bar with no number beside it can be dragged but not aimed.
 *
 * The label is the control's accessible name via `aria-label`, so the visible text and the
 * screen-reader text cannot drift apart; a caller that wants a different accessible name passes
 * `ariaLabel` and takes responsibility for the difference.
 */
export function SliderField({
  label,
  valueLabel,
  ariaLabel,
  style,
  ...rest
}: SliderProps & { label: ReactNode; valueLabel?: ReactNode; ariaLabel?: string }) {
  return (
    <span className="mtk-slider-field" style={style}>
      {label !== null && label !== undefined && label !== "" && (
        <span className="mtk-slider-field__label" aria-hidden="true">{label}</span>
      )}
      <Slider aria-label={ariaLabel ?? (typeof label === "string" ? label : undefined)} {...rest} />
      {valueLabel !== undefined && <span className="mtk-slider-field__value">{valueLabel}</span>}
    </span>
  );
}

// ── Toolbar ───────────────────────────────────────────────────────────────────────────────────────

/**
 * The shared row rhythm. Twenty-five hand-rolled `display: flex; align-items: center; gap:` rows
 * existed across the panels with nothing in common between them, which is how one subsystem's
 * toolbar ends up with a different padding, a different divider and a different overflow answer from
 * its neighbour's — and how two controls end up touching.
 *
 * The overflow policy is WRAP, deliberately, and it is the reason this is a component and not a
 * convention: the pattern it replaces is `overflow-x: auto` on a strip with no visible scrollbar,
 * which puts the last controls off screen with nothing on screen to say they exist. A row that grows
 * a second line is legible; a row that silently loses its right-hand end is not.
 */
export function Toolbar({
  tight = false,
  raised = true,
  divided = true,
  className,
  children,
  style,
  ...rest
}: {
  tight?: boolean;
  raised?: boolean;
  divided?: boolean;
  className?: string;
  children: ReactNode;
  style?: CSSProperties;
  role?: AriaRole;
  "aria-label"?: string;
} & DataAttrs) {
  const cls = [
    "mtk-toolbar",
    tight && "mtk-toolbar--tight",
    raised && "mtk-toolbar--raised",
    divided && "mtk-toolbar--divided",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  return (
    <div className={cls} style={style} {...rest}>
      {children}
    </div>
  );
}

/** Controls that belong together, at the control gap rather than the toolbar gap. `attached` fuses
 *  them into one segmented surface (a transport, a mode switch) so the group reads as a single thing. */
export function ToolbarGroup({
  attached = false,
  grow = false,
  className,
  children,
  style,
  ...rest
}: {
  attached?: boolean;
  /** Take the row's leftover width. A number sets the flex basis the group prefers before it does. */
  grow?: boolean | number;
  className?: string;
  children: ReactNode;
  style?: CSSProperties;
  role?: AriaRole;
  "aria-label"?: string;
} & DataAttrs) {
  const cls = [
    "mtk-toolbar__group",
    attached && "mtk-toolbar__group--attached",
    grow !== false && "mtk-toolbar__group--grow",
    className,
  ]
    .filter(Boolean)
    .join(" ");
  const basis = typeof grow === "number" ? { ["--mtk-toolbar-grow" as string]: `${grow}px` } : undefined;
  return (
    <div className={cls} style={{ ...basis, ...style }} {...rest}>
      {children}
    </div>
  );
}

/** A hairline between groups — reached for only when space alone has stopped separating them. */
export function ToolbarSeparator() {
  return <span className="mtk-toolbar__sep" aria-hidden="true" />;
}

/** Pushes what follows to the far end of the row. */
export function ToolbarSpacer() {
  return <span className="mtk-toolbar__spacer" aria-hidden="true" />;
}

/** A read-out is DATA, not a control: mono, tabular-figures, and never the thing that grows. A
 *  timecode that reflows as it counts is the reason the animate transport row jittered. */
export function ReadOut({
  children,
  unit,
  title,
  ...rest
}: { children: ReactNode; unit?: ReactNode; title?: string } & DataAttrs) {
  return (
    <span className="mtk-readout" title={title} {...rest}>
      <span className="mtk-readout__value">{children}</span>
      {unit !== undefined && <span className="mtk-readout__unit">{unit}</span>}
    </span>
  );
}
