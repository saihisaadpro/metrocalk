//! Focus-mode banner. Focus is a live editor state, so it is announced politely and the visible banner is
//! a native button: pointer, keyboard, and assistive-technology users all get the same clear exit action.
//!
//! It names a SUBJECT, not an id (ADR-194). It used to print the raw loro key — `Focused: 1_4a3f` over a
//! scene whose outliner row said `Weld Gun` — which is the exact thing `selectionText.ts` exists to stop
//! (`<ux_quality>` 4, no engine-internal ids in user copy). And focus can now be a SET, which an id
//! cannot express at all: the caller resolves the subject through the one place the editor decides what
//! to call something, and hands the finished clause here.

import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, z } from "../theme/tokens";

export function FocusBanner({ subject, dist, onClear }: { subject: string; dist: number; onClear: () => void }) {
  return (
    <div
      role="status"
      aria-live="polite"
      aria-atomic="true"
      style={{
        position: "fixed",
        top: "calc(var(--mtk-header-height) + var(--mtk-space-4))",
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: z.badge,
        maxWidth: `calc(100vw - ${space.xxl * 2}px)`,
      }}
    >
      <Button
        id="focusbanner"
        data-testid="focusbanner"
        data-dist={String(dist)}
        data-focused="true"
        type="button"
        variant="secondary"
        onClick={onClear}
        aria-label={`Focused on ${subject}. Exit focus`}
        title="Exit focus (Esc)"
        style={{
          display: "flex",
          minWidth: 0,
          maxWidth: "100%",
          gap: space.md,
          padding: `${space.xs}px ${space.lg}px`,
          borderRadius: radius.pill,
          background: color.info.bg,
          borderColor: color.info.border,
          color: color.info.text,
          fontFamily: font.mono,
          fontSize: fontSize.body,
          boxShadow: elevation.e4,
        }}
      >
        <Icon name="detail" size="md" />
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          Focused: {subject}
        </span>
        <span aria-hidden="true" style={{ color: color.text.muted }}>
          · Esc to exit
        </span>
      </Button>
    </div>
  );
}
