//! The shot cards — the one gesture that puts a camera move into a scene.
//!
//! Shared rather than duplicated because there are now two places a shot is added from: the Gameplay
//! panel's compact Cinematics block (300px, where a cutscene is authored beside its roles and its
//! effects) and the Cutscene timeline in the Animate dock (where it is edited against a clock). The
//! card grid is the same object in both, down to the `shot-${kind}` handles the tests and the `.exe`
//! E2E key on, and two copies of it would drift the first time a card's refusal wording changed.

import { Icon } from "../theme/icons";
import { Button } from "../theme/primitives";
import { color, space, text } from "../theme/tokens";
import type { ShotSpec } from "../transport/protocol";

export interface ShotCatalogueProps {
  specs: ShotSpec[];
  /** Columns the grid may use. The narrow dock gets two; the wide timeline gets as many as fit. */
  minColumn?: number;
  disabled?: boolean;
  /** Why the whole grid is refusing, in the user's words — never a bare dark button. */
  disabledReason?: string;
  onPick: (kind: string) => void;
}

export function ShotCatalogue({
  specs,
  minColumn,
  disabled = false,
  disabledReason,
  onPick,
}: ShotCatalogueProps) {
  // AN EMPTY CATALOGUE MUST NOT RENDER AN EMPTY GROUP. `cinema_catalog` answers with nothing wherever
  // the cinematics sub-engine is absent — the browser build is exactly that case — and both call sites
  // then drew a labelled group containing no cards, under a pacing row that sets the pacing of a
  // cutscene there is no way to add. Say why instead. The guard lives here rather than in either
  // caller because both callers had it wrong for the same reason.
  if (specs.length === 0) {
    return (
      <div data-testid="cinema-unavailable" style={{ ...text.itemLabel, color: color.text.muted }}>
        No shots are available here — the cinematics sub-engine runs in the desktop editor.
      </div>
    );
  }

  return (
    <div
      role="group"
      aria-label="Add a shot"
      data-testid="shot-catalogue"
      style={{
        display: "grid",
        gridTemplateColumns: `repeat(auto-fill, minmax(${minColumn ?? 120}px, 1fr))`,
        gap: space.xs,
      }}
    >
      {specs.map((spec) => (
        <Button
          key={spec.kind}
          data-testid={`shot-${spec.kind}`}
          variant="secondary"
          compact
          disabled={disabled}
          disabledReason={disabledReason}
          title={disabled ? disabledReason : `${spec.blurb}. Adds: ${spec.adds} — one Ctrl-Z removes it`}
          onClick={() => onPick(spec.kind)}
        >
          <Icon name={spec.kind} size="md" fallback="camera" /> {spec.label}
        </Button>
      ))}
    </div>
  );
}

export default ShotCatalogue;
