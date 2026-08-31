//! EmptyState (M10.10 / C10) — the first-run / empty-project state shown over the stage when the scene has
//! no entities: a true empty state with ONE clear next step ("Describe your first object, or drag in an
//! asset"), never a blank canvas and never the 5k perf fixture. The CTA focuses the describe field so the
//! front door is one click away.

import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, elevation, font, fontSize, lineHeight, radius, space } from "../theme/tokens";

export interface EmptyStateProps {
  /** Aims the caret at the stage composer — the CTA this file's own header has always described. */
  onDescribe?: () => void;
  onBrowseAssets?: () => void;
  onDrawPipe?: () => void;
  onImport?: () => void;
}

export function EmptyState({ onDescribe, onBrowseAssets, onDrawPipe, onImport }: EmptyStateProps = {}) {
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
        textAlign: "center",
        color: color.text.secondary,
        font: font.ui,
        fontSize: fontSize.label,
        pointerEvents: "none",
      }}
    >
      {/* A resting surface, not bare copy over the stage. The viewport behind this can be anything — a pale
          sky, a dark scene, a busy grid — so text tuned for a panel is a coin toss out there. The card makes
          the first thing a new author reads legible regardless of what is being rendered. */}
      <div
        data-testid="emptyStateCard"
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: space.lg,
          padding: `${space.xxl}px ${space.xxxl}px ${space.xxl}px`,
          maxWidth: 460,
          background: color.bg.panel,
          border: `1px solid ${color.border.subtle}`,
          borderRadius: radius.xxl,
          boxShadow: elevation.e3,
        }}
      >
        <Icon name="sparkle" size={34} style={{ color: color.accent.base, opacity: 0.55 }} />
        <div style={{ fontSize: fontSize.heading, color: color.text.primary, fontWeight: 650, letterSpacing: "-0.2px" }}>Start with something tangible</div>
        <div style={{ color: color.text.muted, lineHeight: lineHeight.body }}>Draw procedural geometry, choose a local asset, or import your own file. Every result remains editable and undoable.</div>
        <div style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", gap: space.sm, pointerEvents: "auto", marginTop: space.xs }}>
          {/* THE HEADER OF THIS FILE HAS ALWAYS SAID "the CTA focuses the describe field", AND NO CTA
              DID. It could not: the describe field was inside a collapsed disclosure in a workspace
              this card cannot open. It is on the stage now (`.mtk-stage-footer`), directly below this
              card, so the promise is finally keepable — and it leads, because describing is north
              star #2 and the other three are the ways you reach for something that already exists. */}
          {onDescribe && (
            <Button data-testid="emptyDescribe" variant="primary" onClick={onDescribe}><Icon name="sparkle" size="sm" /> Describe it</Button>
          )}
          <Button data-testid="emptyPipe" variant="secondary" onClick={onDrawPipe}><Icon name="pipe" size="sm" /> Draw a pipe</Button>
          <Button data-testid="emptyAssets" variant="secondary" onClick={onBrowseAssets}>Browse assets</Button>
          <Button data-testid="emptyImport" variant="secondary" onClick={onImport}>Import file…</Button>
        </div>
      </div>
    </div>
  );
}
