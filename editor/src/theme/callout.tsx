//! **The one inline note**, in its own module — and the module boundary is a BUDGET decision, exactly
//! like `fields.tsx`'s own (ADR-147).
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! `Callout` lived in `theme/fields.tsx`, whose whole argument is that a task-surface vocabulary
//! declared beside `primitives.tsx` lands in the ENTRY chunk whether or not a first-paint surface uses
//! it. That argument held right up until a first-paint surface needed ONE of the eight: the stage
//! composer states its outcomes as callouts, and importing the note dragged the checkbox, the field
//! grid, the metric grid and the progress bar into the entry with it — measured at 10,688 bytes, of
//! which the composer itself is a fraction. Splitting the note out costs nothing anyone can measure and
//! keeps the other seven where ADR-147 put them.
//!
//! `fields.tsx` re-exports it, so every existing consumer is unchanged and there is still exactly one
//! `Callout` in the engine.

import type { AriaRole, CSSProperties, ReactNode } from "react";
import { Icon } from "./icons";

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

