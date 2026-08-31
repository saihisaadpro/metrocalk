//! **What an environment reply MEANS**, in one place, because two surfaces now read the same reply.
//!
//! The panel (`panels/LookSection`) and the command palette both call `import_environment`, and the
//! reply has three outcomes that must never be collapsed into two:
//!
//! * **cancelled** — the person dismissed the file dialog. Nothing happened and nothing should be said.
//!   Drawing this as a failure is the honest-state rule read backwards; drawing it as a success is
//!   worse.
//! * **refused** — a real reason: unreadable file, not a Radiance panorama, over the size bound. The
//!   engine already wrote the sentence; showing anything else here would be a second opinion.
//! * **applied** — the summary the engine wrote, which names the file and its size.
//!
//! Written as a pure function with no React and no store so both callers can hold it, and so the rule
//! can be tested without mounting anything.

import type { EnvironmentReply } from "../transport/protocol";

/** How a surface should report an environment reply. `null` = say nothing at all. */
export interface EnvironmentOutcome {
  tone: "success" | "error";
  message: string;
}

/** The one reading of an [`EnvironmentReply`]. `null` for a dismissed dialog. */
export function environmentOutcome(reply: EnvironmentReply): EnvironmentOutcome | null {
  if (reply.cancelled) return null;
  if (reply.reason) return { tone: "error", message: reply.reason };
  return {
    tone: "success",
    // A reply that applied with no sentence still has a name, and "" is not a report. The engine
    // always writes one on import; the fallback covers a reset, whose message is short but present.
    message: reply.message || `Lighting from ${reply.label}`,
  };
}
