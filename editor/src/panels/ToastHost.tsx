//! ToastHost (M10.10) — renders the transient toasts (`store/toasts`) over the **stage**, top-center, so
//! confirmations land next to where the user acted rather than only in the footer gutter (C11 / C5). Each
//! toast auto-dismisses after `TOAST_TTL_MS` (the timer lives here, not the store) and is click-to-dismiss.
//! Stable hooks (`#toastHost`, `data-testid="toast"`, `data-kind`) for the review flow + Vitest.

import { useEffect, useState } from "react";
import { toastStore, useToasts, TOAST_TTL_MS, type Toast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, elevation, font, fontSize, radius, space, z } from "../theme/tokens";

// Semantic kinds → the design-system colour roles (theme/global.css vars).
const TONE: Record<Toast["kind"], { background: string; border: string; foreground: string }> = {
  info: { background: color.info.bg, border: color.info.border, foreground: color.info.text },
  cost: { background: color.warn.bg, border: color.warn.border, foreground: color.warn.text },
  success: { background: color.success.bg, border: color.success.border, foreground: color.success.text },
  error: { background: color.danger.bg, border: color.danger.border, foreground: color.danger.text },
};

function ToastRow({ toast }: { toast: Toast }) {
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    if (paused) return;
    const t = setTimeout(() => toastStore.getState().dismiss(toast.id), TOAST_TTL_MS);
    return () => clearTimeout(t);
  }, [paused, toast.id]);

  const tone = TONE[toast.kind];
  const live = toast.kind === "error" ? "assertive" : "polite";

  return (
    <div
      className="mtk-toast mtk-anim-toast"
      data-testid="toast"
      data-kind={toast.kind}
      role={toast.kind === "error" ? "alert" : "status"}
      aria-live={live}
      aria-atomic="true"
      onMouseEnter={() => setPaused(true)}
      onMouseLeave={() => setPaused(false)}
      onFocusCapture={() => setPaused(true)}
      onBlurCapture={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setPaused(false);
      }}
      style={{
        // Passive feedback must never intercept viewport/toolbar gestures. Only the explicitly interactive
        // dismiss control opts back into pointer events; focusing or hovering that control still pauses TTL.
        pointerEvents: "none",
        display: "flex",
        alignItems: "center",
        gap: space.sm,
        background: tone.background,
        color: tone.foreground,
        border: `1px solid ${tone.border}`,
        borderRadius: radius.lg,
        padding: `${space.sm}px ${space.sm}px ${space.sm}px ${space.lg}px`,
        fontSize: fontSize.body,
        fontFamily: font.ui,
        boxShadow: elevation.e2,
        width: "max-content",
        maxWidth: `calc(100vw - ${space.xxl * 2}px)`,
      }}
    >
      <span style={{ minWidth: 0, overflowWrap: "anywhere" }}>{toast.text}</span>
      <Button
        type="button"
        variant="ghost"
        compact
        icon
        data-testid="toast-dismiss"
        aria-label={`Dismiss notification: ${toast.text}`}
        title="Dismiss notification"
        onClick={() => toastStore.getState().dismiss(toast.id)}
        style={{ flex: "none", color: tone.foreground, pointerEvents: "auto" }}
      >
        <Icon name="close" size="sm" />
      </Button>
    </div>
  );
}

export function ToastHost({ top = 58 }: { top?: number }) {
  const toasts = useToasts();
  if (toasts.length === 0) return null;
  return (
    <div
      id="toastHost"
      data-testid="toastHost"
      role="region"
      aria-label="Notifications"
      style={{
        position: "absolute",
        top,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: z.toast,
        display: "flex",
        flexDirection: "column",
        gap: space.sm,
        alignItems: "center",
        pointerEvents: "none",
      }}
    >
      {toasts.map((t) => (
        <ToastRow key={t.id} toast={t} />
      ))}
    </div>
  );
}
