//! EmptyState (M10.10 / C10) — the first-run / empty-project state shown over the stage when the scene has
//! no entities: a true empty state with ONE clear next step ("Describe your first object, or drag in an
//! asset"), never a blank canvas and never the 5k perf fixture. The CTA focuses the describe field so the
//! front door is one click away.

export function EmptyState() {
  return (
    <div
      id="emptyState"
      data-testid="emptyState"
      style={{
        position: "absolute",
        inset: 0,
        display: "flex",
        flexDirection: "column",
        alignItems: "center",
        justifyContent: "center",
        gap: 10,
        textAlign: "center",
        color: "#9aa0aa",
        font: "13px ui-monospace, monospace",
        pointerEvents: "none",
      }}
    >
      <div style={{ fontSize: 15, color: "#cfd2d6" }}>Your scene is empty</div>
      <div style={{ maxWidth: 360 }}>Describe your first object above, or drag an asset in from the library.</div>
      <button
        data-testid="emptyDescribe"
        onClick={() => (document.getElementById("describe") as HTMLInputElement | null)?.focus()}
        style={{
          pointerEvents: "auto",
          background: "#2a4365",
          color: "#fff",
          border: "none",
          borderRadius: 4,
          padding: "5px 12px",
          cursor: "pointer",
          font: "12px ui-monospace, monospace",
        }}
      >
        Describe your first object
      </button>
    </div>
  );
}
