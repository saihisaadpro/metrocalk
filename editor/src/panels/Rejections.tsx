//! Rejection surface — the north-star "every 'no' explained". When the core rejects an optimistic
//! edit, this panel restores the authoritative state and explains why the change could not be applied.

import { projectionStore, useRejections } from "../store/projection";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, z } from "../theme/tokens";

export function Rejections() {
  const rejections = useRejections();
  if (rejections.length === 0) return null;

  return (
    // Stable `#reject` id — the "every 'no' explained" surface the acceptance flow reads (ADR-010).
    <section
      id="reject"
      data-testid="reject"
      aria-label="Rejected changes"
      style={{
        position: "fixed",
        right: space.lg,
        bottom: "calc(var(--mtk-status-bar-height) + var(--mtk-bottom-bar-height) + var(--mtk-space-4))",
        width: `min(360px, calc(100vw - ${space.xxl}px))`,
        zIndex: z.toast,
      }}
    >
      {rejections.map((rejection) => (
        <div
          key={rejection.clientOpId}
          role="alert"
          aria-live="assertive"
          aria-atomic="true"
          style={{
            display: "flex",
            alignItems: "flex-start",
            gap: space.sm,
            marginTop: space.sm,
            padding: `${space.md}px ${space.sm}px ${space.md}px ${space.lg}px`,
            color: color.danger.text,
            background: color.danger.bg,
            border: `1px solid ${color.danger.border}`,
            borderRadius: radius.lg,
            boxShadow: elevation.e2,
            fontFamily: font.ui,
            fontSize: fontSize.body,
          }}
        >
          <span style={{ flex: 1, minWidth: 0, lineHeight: 1.45, overflowWrap: "anywhere" }}>
            <strong>Rejected:</strong> {rejection.reason}
          </span>
          <Button
            type="button"
            variant="ghost"
            compact
            icon
            onClick={() => projectionStore.getState().dismissRejection(rejection.clientOpId)}
            aria-label={`Dismiss rejection: ${rejection.reason}`}
            title="Dismiss rejection"
            style={{ flex: "none", color: color.danger.text }}
          >
            <Icon name="close" size="sm" />
          </Button>
        </div>
      ))}
    </section>
  );
}
