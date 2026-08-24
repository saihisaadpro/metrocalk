//! StatusBar — the transient status line (the scaffold's bottom-left `#status`). Subscribes ONLY to
//! the ephemeral UI/status store (not the projection store, invariant 1): status is chrome, not
//! projected core state. Any action (`bound HealthBar`, `topped up`, …) flows here via `setStatus`;
//! an empty status renders a neutral placeholder so the bar never collapses to nothing.
//!
//! Keeps the vanilla scaffold's stable `#status` id (plus a `data-testid`) so the acceptance
//! page-object re-greens by selector-swap, not a spec rewrite.

import { useStatus } from "../store/ui";
import { color, font, fontSize, space } from "../theme/tokens";

const PLACEHOLDER = "ready";

export function StatusBar() {
  const status = useStatus();
  const text = status.length > 0 ? status : PLACEHOLDER;
  const idle = status.length === 0;

  return (
    <div
      id="status"
      data-testid="status"
      role="status"
      aria-live="polite"
      aria-atomic="true"
      title={text}
      style={{
        minHeight: 24,
        boxSizing: "border-box",
        padding: `${space.xs}px ${space.lg}px`,
        fontSize: fontSize.meta,
        // The UI face, not the mono one. A status line is a SENTENCE about what just happened —
        // "bound HealthBar", "stopped" — and setting a sentence in a code face was the shell's one
        // remaining piece of terminal styling: it read as log output rather than as the editor
        // speaking. `font.mono` is for ids, values and diagnostics, which this is not.
        fontFamily: font.ui,
        lineHeight: "16px",
        color: idle ? color.text.muted : color.text.secondary,
        // The GROUND, the same one the header and the dock tracks paint, so the chrome is one
        // continuous surface with the panels floating on it. Opaque, as it must be — the .exe root is
        // transparent for the wgpu composite (ADR-008), so every non-stage surface paints its own.
        background: color.bg.base,
        // No top rule: the ground does not need a line drawn across it to prove it is the ground.
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis",
      }}
    >
      {text}
    </div>
  );
}
