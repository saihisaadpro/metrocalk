//! First-run onboarding — a short, skippable, non-blocking on-ramp. Visibility can be controlled by the
//! workspace, and both calls to action expose callbacks so the card can launch a real guided workflow.

import { useState } from "react";
import { Button } from "../theme/primitives";
import { DisclosureSection } from "../theme/workspace";
import { color, elevation, space, z } from "../theme/tokens";

const FLAG = "mtk.onboarded.v1";

function readDismissed(): boolean {
  try {
    return localStorage.getItem(FLAG) === "1";
  } catch {
    return false;
  }
}

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
      style={{
        position: "fixed",
        left: "50%",
        bottom: "calc(var(--mtk-status-bar-height) + var(--mtk-bottom-bar-height) + var(--mtk-space-4))",
        transform: "translateX(-50%)",
        zIndex: z.menu,
        boxSizing: "border-box",
        width: `min(520px, calc(100vw - ${space.xxl}px))`,
        maxHeight: `calc(100vh - ${space.xxxl}px)`,
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
