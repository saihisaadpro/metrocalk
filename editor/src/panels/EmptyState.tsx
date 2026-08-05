//! EmptyState (M10.10 / C10) — the first-run / empty-project state shown over the stage when the scene has
//! no entities: a true empty state with ONE clear next step ("Describe your first object, or drag in an
//! asset"), never a blank canvas and never the 5k perf fixture. The CTA focuses the describe field so the
//! front door is one click away.

import { Button } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";

export interface EmptyStateProps {
  onBrowseAssets?: () => void;
  onDrawPipe?: () => void;
  onImport?: () => void;
}

export function EmptyState({ onBrowseAssets, onDrawPipe, onImport }: EmptyStateProps = {}) {
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
        gap: space.lg,
        textAlign: "center",
        color: color.text.secondary,
        font: font.ui,
        fontSize: fontSize.label,
        pointerEvents: "none",
      }}
    >
      <div aria-hidden style={{ fontSize: 40, color: color.text.faint, lineHeight: 1 }}>✦</div>
      <div style={{ fontSize: fontSize.heading, color: color.text.primary, fontWeight: 650 }}>Start with something tangible</div>
      <div style={{ maxWidth: 430, color: color.text.muted, lineHeight: 1.5 }}>Draw procedural geometry, choose a local asset, or import your own file. Every result remains editable and undoable.</div>
      <div style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", gap: space.sm, pointerEvents: "auto", marginTop: space.xs }}>
        <Button data-testid="emptyPipe" variant="primary" onClick={onDrawPipe}>⌁ Draw a pipe</Button>
        <Button data-testid="emptyAssets" variant="secondary" onClick={onBrowseAssets}>Browse assets</Button>
        <Button data-testid="emptyImport" variant="secondary" onClick={onImport}>Import file…</Button>
      </div>
    </div>
  );
}
