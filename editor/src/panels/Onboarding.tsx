//! First-run onboarding — a short, skippable, non-blocking on-ramp. Visibility can be controlled by the
//! workspace, and both calls to action expose callbacks so the card can launch a real guided workflow.

import { useState } from "react";
import { Button } from "../theme/primitives";
import { DisclosureSection } from "../theme/workspace";
import { color, elevation, space, z } from "../theme/tokens";

const FLAG = "mtk.onboarded.v1";

/** Has this person already put the first-run card away?
 *
 *  Exported because the card is a STAGE OCCUPANT, not only a card: it is `position: absolute; bottom`
 *  inside the viewport, so anything else that wants the bottom of the stage has to know whether it is
 *  there. ADR-193's frame-guide badge is the first such thing, and it found out the way these are
 *  always found out - an `.exe` composite with two pills painted over each other. */
export function onboardingDismissed(): boolean {
  try {
    return localStorage.getItem(FLAG) === "1";
  } catch {
    return false;
  }
}

const readDismissed = onboardingDismissed;

export interface OnboardingProps {
  /** Workspace-level progressive disclosure. Defaults to true so existing integrations remain compatible. */
  show?: boolean;
  /** Runs after the primary action records completion and closes the card. */
  onStart?: () => void;
  /** Runs after Skip records completion and closes the card. */
  onSkip?: () => void;
}

export function Onboarding({ show = true, onStart, onSkip }: OnboardingProps = {}) {
  const [dismissed, setDismissed] = useState(readDismissed);

  if (!show || dismissed) return null;

  function complete(callback?: () => void) {
    try {
      localStorage.setItem(FLAG, "1");
    } catch {
      // Storage may be unavailable in private mode; local state still dismisses for this session.
    }
    setDismissed(true);
    // The card is a stage occupant, and what yields to it has to learn that it has gone. Announced on
    // `window` rather than through a store, because `dismissed` is this component's own state and a
    // store would be a second place it lives - which is how two answers to "is the card up" start.
    try {
      window.dispatchEvent(new CustomEvent("mtk:onboarding-dismissed"));
    } catch {
      // A DOM that cannot dispatch is a DOM with no stage to occupy.
    }
    callback?.();
  }

  return (
    <DisclosureSection
      id="onboarding"
      data-testid="onboarding"
      title="Make your first thing"
      summary="Place · bind · Play · Save"
      defaultOpen={false}
      tone="card"
      density="compact"
      headingLevel={2}
      landmark={false}
      role="region"
      aria-live="polite"
      aria-labelledby="onboarding-title"
      aria-describedby="onboarding-summary"
      actions={(
        <>
          <Button id="onboardSkip" data-testid="onboardSkip" type="button" variant="ghost" compact onClick={() => complete(onSkip)}>
            Skip
          </Button>
          <Button id="onboardStart" data-testid="onboardStart" type="button" variant="primary" compact onClick={() => complete(onStart)}>
            Start
          </Button>
        </>
      )}
      // Overlaid on the STAGE, inside it — the same anchoring `PlayBadge` uses, and for the same
      // reason: what the card talks about (place · bind · Play · Save) happens on the stage. It was
      // `fixed` and centred on the WINDOW, which is a position that knows nothing about the docks:
      // at 1000 px a 520 px card centred on the window overlapped the left dock by 192 px and took
      // the clicks of whatever was under it. Percentages here are of the stage, so the card yields
      // with the stage instead of ignoring it, and the grid stays the only thing that decides where
      // the stage is.
      // No `stopPropagation`: `stageInput.ts`'s `onStageSurface` is the one place that decides
      // whether a pointer belongs to the stage or to chrome floating over it, and a per-overlay
      // guard is the idiom that module superseded. See `PlayBadge` for the measurement.
      style={{
        position: "absolute",
        left: "50%",
        bottom: space.lg,
        transform: "translateX(-50%)",
        zIndex: z.menu,
        boxSizing: "border-box",
        width: `min(520px, calc(100% - ${space.xxl}px))`,
        maxHeight: `calc(100% - ${space.xxxl}px)`,
        overflowX: "hidden",
        overflowY: "auto",
        margin: 0,
        boxShadow: elevation.e3,
      }}
    >
      <p style={{ margin: `0 0 ${space.md}px`, color: color.text.muted, lineHeight: 1.5 }}>
        A one-minute, skippable path through the real editor loop.
      </p>
      <ol style={{ margin: `0 0 0 ${space.xl}px`, padding: 0, color: color.text.secondary, lineHeight: 1.65 }}>
        <li><strong>Place an asset</strong> from Create or import a local file.</li>
        <li><strong>Bind by intent</strong> from the highlighted compatible choices.</li>
        <li><strong>Press Play</strong> to test safely, then Stop to resume editing.</li>
        <li><strong>Save</strong> from File and reopen the project whenever you return.</li>
      </ol>
    </DisclosureSection>
  );
}
