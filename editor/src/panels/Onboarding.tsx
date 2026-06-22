//! First-run onboarding (M10.5) — a short, skippable, NON-blocking "make your first thing" on-ramp.
//!
//! Shows ONCE (a localStorage flag → no nagging: dismiss it and it never returns), as a slim bottom-centre
//! card that never covers the side panels / top toolbar / viewport-centre the user (or the acceptance gate)
//! clicks — it guides without trapping. The copy is cross-checked against the can/can't doc: it promises
//! ONLY what the M10 creative loop actually ships (place · describe · bind · Play/Stop · Save/reopen) and
//! deliberately names NOTHING that isn't built yet (scripting → M12, materials/lighting/audio → M11). An
//! honest on-ramp, not a feature brochure.

import { useState } from "react";

const FLAG = "mtk.onboarded.v1";

function read(): boolean {
  try {
    return localStorage.getItem(FLAG) === "1";
  } catch {
    return false; // private mode / no storage → just show it (still dismissable in-session)
  }
}

export function Onboarding() {
  const [dismissed, setDismissed] = useState(read);
  if (dismissed) return null;

  const close = () => {
    try {
      localStorage.setItem(FLAG, "1"); // remember — no nagging on the next launch
    } catch {
      /* storage unavailable — at least dismiss for this session */
    }
    setDismissed(true);
  };

  return (
    <div
      id="onboarding"
      data-testid="onboarding"
      role="dialog"
      aria-label="Make your first thing"
      style={{
        position: "fixed",
        left: "50%",
        bottom: 18,
        transform: "translateX(-50%)",
        zIndex: 40,
        width: 540,
        maxWidth: "calc(100vw - 32px)",
        background: "rgba(16,20,30,0.97)",
        border: "1px solid #2a3550",
        borderRadius: 10,
        padding: "14px 16px",
        color: "#cde",
        font: "12px ui-monospace, monospace",
        boxShadow: "0 8px 30px rgba(0,0,0,0.45)",
      }}
    >
      <div style={{ display: "flex", alignItems: "baseline", justifyContent: "space-between", marginBottom: 8 }}>
        <span style={{ fontWeight: 600, fontSize: 13, color: "#9ecbff" }}>Make your first thing</span>
        <span style={{ opacity: 0.55 }}>a 1-minute tour — skippable</span>
      </div>
      <ol style={{ margin: "0 0 10px 18px", padding: 0, lineHeight: 1.7 }}>
        <li>
          <strong>Place an asset</strong> — open <strong>Assets</strong> and pick one, or describe one in plain
          words (e.g. “health bar”).
        </li>
        <li>
          <strong>Bind by intent</strong> — click an object that needs something, then pick a highlighted match
          to wire it up.
        </li>
        <li>
          <strong>Press Play</strong> to test it live, then <strong>Stop</strong> to return to editing — nothing
          you tried is kept.
        </li>
        <li>
          <strong>Save</strong> from the <strong>File</strong> menu and reopen it anytime — your scene comes back
          exactly as you left it.
        </li>
      </ol>
      <div style={{ display: "flex", gap: 8, justifyContent: "flex-end", alignItems: "center" }}>
        <button
          id="onboardSkip"
          data-testid="onboardSkip"
          onClick={close}
          style={{ background: "transparent", color: "#9ab", border: "1px solid #2a3550", borderRadius: 5, padding: "5px 12px", cursor: "pointer" }}
        >
          Skip
        </button>
        <button
          id="onboardStart"
          data-testid="onboardStart"
          onClick={close}
          style={{ background: "#1d4ed8", color: "#fff", border: "none", borderRadius: 5, padding: "5px 14px", cursor: "pointer" }}
        >
          Got it — let’s go
        </button>
      </div>
    </div>
  );
}
