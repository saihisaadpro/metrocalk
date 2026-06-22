//! ToastHost (M10.10) — renders the transient toasts (`store/toasts`) over the **stage**, top-center, so
//! confirmations land next to where the user acted rather than only in the footer gutter (C11 / C5). Each
//! toast auto-dismisses after `TOAST_TTL_MS` (the timer lives here, not the store) and is click-to-dismiss.
//! Stable hooks (`#toastHost`, `data-testid="toast"`, `data-kind`) for the review flow + Vitest.

import { useEffect } from "react";
import { toastStore, useToasts, TOAST_TTL_MS, type Toast } from "../store/toasts";

const BG: Record<Toast["kind"], string> = {
  info: "#1c2433",
  cost: "#3a2f16",
  success: "#163322",
  error: "#3a1f1f",
};
const BORDER: Record<Toast["kind"], string> = {
  info: "#2a3550",
  cost: "#5a4a2f",
  success: "#2f6a45",
  error: "#6a3f2f",
};
const FG: Record<Toast["kind"], string> = {
  info: "#cde",
  cost: "#fbbf24",
  success: "#7fe39a",
  error: "#fcd",
};

function ToastRow({ toast }: { toast: Toast }) {
  useEffect(() => {
    const t = setTimeout(() => toastStore.getState().dismiss(toast.id), TOAST_TTL_MS);
    return () => clearTimeout(t);
  }, [toast.id]);
  return (
    <div
      className="mtk-toast"
      data-testid="toast"
      data-kind={toast.kind}
      onClick={() => toastStore.getState().dismiss(toast.id)}
      style={{
        pointerEvents: "auto",
        background: BG[toast.kind],
        color: FG[toast.kind],
        border: `1px solid ${BORDER[toast.kind]}`,
        borderRadius: 6,
        padding: "6px 12px",
        fontSize: 12,
        font: "12px ui-monospace, monospace",
        boxShadow: "0 6px 18px #0007",
        cursor: "pointer",
        maxWidth: 420,
      }}
    >
      {toast.text}
    </div>
  );
}

export function ToastHost({ top = 14 }: { top?: number }) {
  const toasts = useToasts();
  if (toasts.length === 0) return null;
  return (
    <div
      id="toastHost"
      data-testid="toastHost"
      style={{
        position: "absolute",
        top,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: 150,
        display: "flex",
        flexDirection: "column",
        gap: 6,
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
