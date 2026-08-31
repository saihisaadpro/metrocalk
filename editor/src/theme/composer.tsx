//! **The composer** — the one surface in this engine where a user types a sentence and the engine acts
//! on it.
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! WHY IT IS AN ANATOMY AND NOT A PANEL. Two surfaces already asked the same question in two different
//! shapes: `DescribeBar` ("describe something to create" — north-star #2) drew a `<input class="mtk-input">`
//! between a sparkle and a `Create` button, and `TerrainPanel`'s describe box drew a `TextArea` above a
//! status line and a `Build it` button. Same act, same three parts — a field, a reading of what will
//! happen, and the commit — stated twice with different paddings, different type sizes and different
//! places for the reading. The references put that act in ONE shape (`docs/UI/material - texture
//! editor.jpeg`, `docs/UI/collision - event screens.jpeg`): a lifted card holding a leading action, the
//! field, a live readout, and a **round** commit button. This module is that shape, once.
//!
//! THE SUBMIT IS ROUND ON PURPOSE, AND IT IS THE ONLY ROUND CONTROL IN THE ENGINE. Every other button
//! here is a rounded rectangle, so the circle is not decoration — it is what tells you, before you have
//! read a word, that this row commits a sentence rather than toggling a tool. Both reference sheets draw
//! it the same way and it is the one thing they agree on across two otherwise unrelated screens.
//!
//! `form="floating"` is the stage form — a lifted card over the picture. `form="inline"` is the panel
//! form: the same anatomy with no shadow, so a dock column does not get a card inside a card.

import { forwardRef, type InputHTMLAttributes, type ReactNode, type TextareaHTMLAttributes } from "react";
import { Icon } from "./icons";
import { Button, type ButtonProps } from "./primitives";

export interface ComposerProps {
  children: ReactNode;
  /** `floating` lifts the card over the stage; `inline` drops the elevation for a panel column. */
  form?: "floating" | "inline";
  id?: string;
  className?: string;
  "data-testid"?: string;
  "aria-label"?: string;
}

/** The composer surface. A `group`, not a `form`: submitting is a command, not a page navigation, and a
 *  real `<form>` inside the editor shell would reload the app on a stray Enter in any nested field. */
export function Composer({ children, form = "floating", id, className, ...rest }: ComposerProps) {
  return (
    <section
      id={id}
      role="group"
      className={["mtk-composer", `mtk-composer--${form}`, className].filter(Boolean).join(" ")}
      {...rest}
    >
      {children}
    </section>
  );
}

/** The control row: leading action · field · readout · submit. Everything else in a composer stacks
 *  UNDER this row, which is what keeps the commit in the same place whatever the outcome is saying. */
export function ComposerRow({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={["mtk-composer__row", className].filter(Boolean).join(" ")}>{children}</div>;
}

type FieldOwn = { className?: string; "data-testid"?: string };

/** The single-line field. It carries no border and no well of its own — the composer card IS the field's
 *  frame, and a second frame inside it is the "card inside a card" the constitution names. */
export const ComposerField = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement> & FieldOwn>(
  function ComposerField({ className, ...rest }, ref) {
    return <input ref={ref} type="text" className={["mtk-composer__field", className].filter(Boolean).join(" ")} {...rest} />;
  },
);

/** The multi-line field, for a description that is a paragraph rather than a phrase. Same frame, same
 *  type, so the two forms of the same act do not look like two different controls. */
export const ComposerTextArea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement> & FieldOwn>(
  function ComposerTextArea({ className, rows = 2, ...rest }, ref) {
    return (
      <textarea
        ref={ref}
        rows={rows}
        className={["mtk-composer__field", "mtk-composer__field--multiline", className].filter(Boolean).join(" ")}
        {...rest}
      />
    );
  },
);

/** The reading: what the sentence currently in the field WILL do, and what it will cost, before it is
 *  committed. Quiet by default — it orients, it does not compete with the field it explains. */
export function ComposerHint({
  children,
  id,
  className,
  ...rest
}: { children: ReactNode; id?: string; className?: string } & { "data-testid"?: string; role?: "status" }) {
  return (
    <p id={id} className={["mtk-composer__hint", className].filter(Boolean).join(" ")} {...rest}>
      {children}
    </p>
  );
}

export interface ComposerSubmitProps extends Omit<ButtonProps, "children" | "variant" | "icon"> {
  /** The accessible name. An icon-only control has no text, so this is not optional. */
  label: string;
  /** Overrides the default send mark (e.g. a sparkle where the act is a generation). */
  icon?: ReactNode;
}

/** The round commit. `aria-label` carries the name unconditionally, which — unlike `title` — leaves
 *  `disabledReason` free to speak when the button refuses (the `Button` trap: an unconditional `title`
 *  permanently outranks the reason). */
export const ComposerSubmit = forwardRef<HTMLButtonElement, ComposerSubmitProps>(function ComposerSubmit(
  { label, icon, className, ...rest },
  ref,
) {
  return (
    <Button
      ref={ref}
      variant="primary"
      icon
      aria-label={label}
      className={["mtk-composer__submit", className].filter(Boolean).join(" ")}
      {...rest}
    >
      {icon ?? <Icon name="arrow-up" size="md" />}
    </Button>
  );
});

/** Where an outcome lands: the offer, the progress, the refusal. A region rather than a bare stack so a
 *  reply that arrives while the user is elsewhere is announced, and so every composer puts its answer in
 *  the same place relative to the field that produced it. */
export function ComposerOutcome({ children, ...rest }: { children: ReactNode } & { "data-testid"?: string }) {
  return (
    <div className="mtk-composer__outcome" role="status" aria-live="polite" {...rest}>
      {children}
    </div>
  );
}
