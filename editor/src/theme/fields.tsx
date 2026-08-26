//! The settings-sheet family (ADR-147) — the field, the checkbox, the note, the measurement.
//!
//! WHY A SECOND THEME MODULE AND NOT MORE OF `primitives.tsx`. `primitives.tsx` is the always-loaded
//! control set: the shell imports `Button`, `Panel` and the field family on first paint, so everything
//! declared beside them is in the ENTRY chunk whether the surface using it has been opened or not.
//! These eight are a TASK-SURFACE vocabulary — a settings sheet, its evidence, its notes — and today
//! their only consumer is a lazily-loaded workspace. Declared here they land in that workspace's own
//! chunk, and the first-paint budget pays for them the day a first-paint surface actually uses one.
//! Measured: +4,290 bytes on the entry chunk when they lived in `primitives.tsx`, 0 from here.
//!
//! It is still ONE component library. `theme/` is already eight modules — workspace, graph, timeline,
//! assets, icons, Popover, Thumbnail, primitives — split by role, not by subsystem, and no panel is
//! allowed to declare a control of its own in either case.

import { useId } from "react";
import type { AriaRole, CSSProperties, ReactNode } from "react";
import { Icon } from "./icons";

/** A data-* / id passthrough for the stable e2e/Vitest hooks, mirroring `primitives.tsx`. */
type DataAttrs = { id?: string; title?: string; "data-testid"?: string; "data-id"?: string };

export interface FieldGridProps {
  children: ReactNode;
  /** Column floor in px. The column COUNT is then a consequence of the width, never a breakpoint. */
  minColumn?: number;
  className?: string;
  style?: CSSProperties;
  role?: AriaRole;
  "aria-label"?: string;
}

/** As many field columns as fit, down to one. */
export function FieldGrid({ children, minColumn, className, style, ...rest }: FieldGridProps & DataAttrs) {
  return (
    <div
      className={["mtk-field-grid", className].filter(Boolean).join(" ")}
      style={minColumn == null ? style : { ["--mtk-field-min" as string]: `${minColumn}px`, ...style }}
      {...rest}
    >
      {children}
    </div>
  );
}

export interface FieldProps {
  label: ReactNode;
  children: ReactNode;
  /** Concise plain-language guidance, under the control. Always readable prose, never engine jargon. */
  help?: ReactNode;
  /** The value's unit, beside the control — `px`, `m`, `px/unit`. */
  unit?: ReactNode;
  /** Required so the visible label always names the control it edits. */
  htmlFor: string;
  /** Two columns, or the grid's whole width, for a control that needs the room. */
  span?: "one" | "wide" | "full";
  /** Greys the label and help with the control, so a disabled field reads as one disabled thing. */
  disabled?: boolean;
  className?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/** Label above, control, help below — the field shape a settings sheet is made of. */
export function Field({
  label,
  children,
  help,
  unit,
  htmlFor,
  span = "one",
  disabled = false,
  className,
  style,
  ...rest
}: FieldProps) {
  return (
    <div
      className={["mtk-field", span === "wide" && "mtk-field--wide", span === "full" && "mtk-field--full", className]
        .filter(Boolean)
        .join(" ")}
      data-disabled={disabled || undefined}
      style={style}
      {...rest}
    >
      <label className="mtk-field__label" htmlFor={htmlFor}>{label}</label>
      <div className="mtk-field__control">
        {children}
        {unit != null && <span className="mtk-field__unit">{unit}</span>}
      </div>
      {help != null && <p className="mtk-field__help" id={`${htmlFor}-help`}>{help}</p>}
    </div>
  );
}

export interface CheckboxProps {
  label: ReactNode;
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** One line of plain-language consequence, under the label. */
  description?: ReactNode;
  disabled?: boolean;
  /** Why it is refusing, in the user's words — surfaced as the row's `title` while disabled. */
  disabledReason?: string;
  id?: string;
  className?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/**
 * The one checkbox. Three files drew their own before this — the Model panel, the animation graph and
 * the inspector — and the constitution names "toggle" among the controls that must all come from one
 * component family.
 *
 * `appearance: none` for the reason `.mtk-select` takes it: `accent-color` yields the OS widget, whose
 * shape, radius and tick belong to the desktop theme and match nothing else in the editor. The native
 * `<input type="checkbox">` underneath is kept, so keyboard, screen-reader and form semantics are the
 * platform's and not a re-implementation.
 */
export function Checkbox({
  label,
  checked,
  onChange,
  description,
  disabled = false,
  disabledReason,
  id,
  className,
  style,
  ...rest
}: CheckboxProps) {
  const generatedId = useId();
  const inputId = id ?? `mtk-check-${generatedId}`;
  const descriptionId = description != null ? `${inputId}-description` : undefined;
  return (
    <label
      className={["mtk-check", className].filter(Boolean).join(" ")}
      htmlFor={inputId}
      data-disabled={disabled || undefined}
      title={disabled ? disabledReason : undefined}
      style={style}
      {...rest}
    >
      <input
        id={inputId}
        type="checkbox"
        className="mtk-check__box"
        checked={checked}
        disabled={disabled}
        // ON THE INPUT, not only on the label around it. A reason lives where the refusal does: an
        // assistive technology reading the checkbox reaches the input, and the shots harness's R9
        // check walks UP from the refusing element through inactive ancestors only — a `<label>` is
        // not itself disabled, so a title parked there is a reason the control never gives.
        title={disabled ? disabledReason : undefined}
        aria-describedby={descriptionId}
        onChange={(event) => onChange(event.target.checked)}
      />
      <span className="mtk-check__label">{label}</span>
      {description != null && <span className="mtk-check__description" id={descriptionId}>{description}</span>}
    </label>
  );
}

export type CalloutTone = "neutral" | "info" | "success" | "warn" | "danger";

const CALLOUT_ICON: Record<CalloutTone, string> = {
  neutral: "info",
  info: "info",
  success: "check",
  warn: "warning",
  danger: "error",
};

export interface CalloutProps {
  children: ReactNode;
  tone?: CalloutTone;
  title?: ReactNode;
  /** Overrides the tone's default mark. */
  icon?: ReactNode;
  role?: AriaRole;
  id?: string;
  className?: string;
  style?: CSSProperties;
  "data-testid"?: string;
}

/**
 * The one inline note. Guidance, a warning, an all-clear and a "why this is refusing" were four
 * bespoke boxes inside a single panel, each with its own border colour, padding and type size.
 *
 * The mark is never optional: colour alone must not carry the meaning, which is the same rule
 * `Badge`'s `title` and the palette gate's contrast floor exist to keep.
 */
export function Callout({ children, tone = "neutral", title, icon, role, className, style, ...rest }: CalloutProps) {
  return (
    <div
      className={["mtk-callout", `mtk-callout--${tone}`, className].filter(Boolean).join(" ")}
      role={role}
      style={style}
      {...rest}
    >
      <span className="mtk-callout__icon" aria-hidden="true">
        {icon ?? <Icon name={CALLOUT_ICON[tone]} size="sm" />}
      </span>
      <div className="mtk-callout__body">
        {title != null && <strong className="mtk-callout__title">{title}</strong>}
        {children}
      </div>
    </div>
  );
}

/** A measurement grid: as many tiles as fit, down to one. */
export function MetricGrid({ children, minColumn, className, style, ...rest }: {
  children: ReactNode;
  minColumn?: number;
  className?: string;
  style?: CSSProperties;
} & DataAttrs) {
  return (
    <div
      className={["mtk-metric-grid", className].filter(Boolean).join(" ")}
      style={minColumn == null ? style : { ["--mtk-metric-min" as string]: `${minColumn}px`, ...style }}
      {...rest}
    >
      {children}
    </div>
  );
}

/**
 * One measurement, and — when an operation has produced one — the measurement it became.
 *
 * No border: six numbers in six hairline boxes is six rectangles competing with the numbers inside
 * them. The value is mono and tabular so a column of them stays aligned while it changes, which is
 * the same reason `ReadOut` is.
 */
export function Metric({ label, value, after, unit, description, ...rest }: {
  label: ReactNode;
  value: ReactNode;
  /** The value after the operation — rendered as `before → after`, with the result marked as such. */
  after?: ReactNode;
  unit?: ReactNode;
  description?: string;
} & DataAttrs) {
  const suffix = unit != null ? <> {unit}</> : null;
  return (
    <div className="mtk-metric" title={description} {...rest}>
      <span className="mtk-metric__label">{label}</span>
      <span className="mtk-metric__values">
        <span className="mtk-metric__value">{value}{suffix}</span>
        {after != null && (
          <>
            <span className="mtk-metric__arrow" aria-hidden="true"><Icon name="arrow-right" size="sm" /></span>
            <span className="mtk-metric__value mtk-metric__value--after">{after}{suffix}</span>
            <VisuallyHidden> after</VisuallyHidden>
          </>
        )}
      </span>
    </div>
  );
}

/** A determinate progress bar, drawn once. Indeterminate when `value` is omitted. */
export function ProgressBar({ value, label, className, style }: {
  value?: number;
  label: string;
  className?: string;
  style?: CSSProperties;
}) {
  return (
    <progress
      className={["mtk-progress", className].filter(Boolean).join(" ")}
      max={1}
      value={value}
      aria-label={label}
      style={style}
    />
  );
}

/** Text for assistive technology only. */
export function VisuallyHidden({ children }: { children: ReactNode }) {
  return <span className="mtk-visually-hidden">{children}</span>;
}
