//! EmptyState (M10.10 / C10) — the first-run / empty-project state shown over the stage when the scene has
//! no entities: a true empty state with ONE clear next step ("Describe your first object, or drag in an
//! asset"), never a blank canvas and never the 5k perf fixture. The CTA focuses the describe field so the
//! front door is one click away.

import { Button } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";

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
        gap: space.lg,
        textAlign: "center",
        color: color.text.secondary,
        font: font.ui,
        fontSize: fontSize.label,
        pointerEvents: "none",
      }}
    >
      <div aria-hidden style={{ fontSize: 40, color: color.text.faint, lineHeight: 1 }}>✦</div>
      <div style={{ fontSize: fontSize.heading, color: color.text.primary, fontWeight: 600 }}>Your scene is empty</div>
      <div style={{ maxWidth: 380, color: color.text.muted }}>Describe your first object above, or drag an asset in from the library.</div>
      <Button
        data-testid="emptyDescribe"
        variant="primary"
        style={{ pointerEvents: "auto", marginTop: space.xs }}
        onClick={() => (document.getElementById("describe") as HTMLInputElement | null)?.focus()}
      >
        ✦ Describe your first object
      </Button>
      <div style={{ fontSize: fontSize.meta, color: color.text.faint, marginTop: space.xs }}>or drop a .glb / .fbx / .png anywhere on the stage</div>
    </div>
  );
}
