//! StatusBar — the transient status line (the scaffold's bottom-left `#status`). Subscribes ONLY to
//! the ephemeral UI/status store (not the projection store, invariant 1): status is chrome, not
//! projected core state. Any action (`bound HealthBar`, `topped up`, …) flows here via `setStatus`;
//! an empty status renders a neutral placeholder so the bar never collapses to nothing.
//!
//! Keeps the vanilla scaffold's stable `#status` id (plus a `data-testid`) so the acceptance
//! page-object re-greens by selector-swap, not a spec rewrite.

import { useStatus } from "../store/ui";

const PLACEHOLDER = "ready";

export function StatusBar() {
  const status = useStatus();
  const text = status.length > 0 ? status : PLACEHOLDER;
  const idle = status.length === 0;

  return (
    <div
      id="status"
      data-testid="status"
      style={{
        padding: "4px 10px",
        fontSize: 12,
        fontFamily: "monospace",
        color: idle ? "#667" : "#cde",
        borderTop: "1px solid #222",
        whiteSpace: "nowrap",
        overflow: "hidden",
        textOverflow: "ellipsis",
      }}
    >
      {text}
    </div>
  );
}
