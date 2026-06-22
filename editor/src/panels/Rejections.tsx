//! Rejection surface — the north-star "every 'no' explained". When the core rejects an optimistic
//! edit (e.g. an incompatible bind), the store reverts the optimistic effect and records the reason;
//! this panel shows it so the user always sees WHY a bind failed.

import { projectionStore, useRejections } from "../store/projection";

export function Rejections() {
  const rejections = useRejections();
  if (rejections.length === 0) return null;
  return (
    // Stable `#reject` id — the "every 'no' explained" surface the prompt-40 E2E reads (ADR-010).
    <div id="reject" data-testid="reject" style={{ position: "fixed", right: 12, bottom: 12, maxWidth: 360, zIndex: 20 }}>
      {rejections.map((r) => (
        <div
          key={r.clientOpId}
          style={{
            background: "rgba(120,20,20,0.92)",
            color: "#ffe8e8",
            border: "1px solid #f87171",
            borderRadius: 6,
            padding: "8px 10px",
            marginTop: 6,
            font: "12px ui-monospace, monospace",
          }}
        >
          {/* generic — the surface handles bind AND field-edit rejections (the reason explains which) */}
          <strong>rejected</strong> — {r.reason}
          <button
            onClick={() => projectionStore.getState().dismissRejection(r.clientOpId)}
            style={{ marginLeft: 8, background: "transparent", color: "#ffd", border: "none", cursor: "pointer" }}
          >
            ✕
          </button>
        </div>
      ))}
    </div>
  );
}
