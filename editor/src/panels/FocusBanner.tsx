//! Focus-mode banner (M3.3 / M10.1 React parity). When the user Focuses an entity (context menu), the
//! camera frames it and this banner appears, carrying the framed camera distance (read back from the live
//! `focus_debug` command) as `data-dist`. Click it — or press Esc (App handles the key) — to exit focus.
//!
//! Mirrors the scaffold's `#focusbanner` DOM contract exactly (`data-dist` numeric, `data-focused="true"`,
//! conditionally rendered so `display` is never "none" while focused) so the prompt-40 page-object greens
//! by selector-swap, not a rewrite.

export function FocusBanner({ id, dist, onClear }: { id: string; dist: number; onClear: () => void }) {
  return (
    <div
      id="focusbanner"
      data-testid="focusbanner"
      data-dist={String(dist)}
      data-focused="true"
      onClick={onClear}
      title="Click or press Esc to exit focus"
      style={{
        position: "fixed",
        top: 12,
        left: "50%",
        transform: "translateX(-50%)",
        zIndex: 65,
        display: "flex",
        alignItems: "center",
        gap: 8,
        padding: "4px 14px",
        borderRadius: 999,
        background: "#10203aee",
        border: "1px solid #3a6ea5",
        color: "#9ecbff",
        font: "12px ui-monospace, monospace",
        boxShadow: "0 4px 16px #0008",
        cursor: "pointer",
      }}
    >
      🔍 Focused: {id} <span style={{ opacity: 0.6 }}>· click or Esc to exit</span>
    </div>
  );
}
