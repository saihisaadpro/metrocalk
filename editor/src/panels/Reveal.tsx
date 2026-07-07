//! Bind-by-intent reveal (north-star #1, M10.1 parity) — select an entity → its ranked compatible bind
//! targets appear (proximity·affinity, ADR-011), incompatible ones are GREYED **with the reason** ("every
//! 'no' explained", ADR-016), existing bindings show as "tracking …". One click on a candidate binds it
//! (optimistic; the authoritative edge streams back over the Channel). Reads the reveal from the shell's
//! `reveal_targets` command via the `EditorClient`; subscribes only to `selectedId`.
//!
//! The `.cand` / `.boundrow` / `disabled` class hooks + `data-id` mirror the vanilla scaffold's stable
//! signals, so the prompt-40 acceptance page-object re-greens by selector-swap, not a spec rewrite.

import { useSelectedId } from "../store/projection";
import { useReveal } from "../store/reveal";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";

export function Reveal({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  // The reveal (targets + greyed-with-reason + bound) comes from a SHARED, deduplicated cache keyed on
  // `(id, edgeSig)` — see store/reveal.ts. The Diagnostics panel reads the same key, so a select now
  // costs ONE `reveal_targets` round-trip, not two (perf audit F2). The key still includes each outgoing
  // edge's STATUS so an optimistic bind's `pending → confirmed` flip re-fetches and moves the target into
  // "tracking".
  const reveal = useReveal(client);

  if (!id) {
    return <div style={{ padding: 12, color: "#888" }}>Select an entity to see compatible bind targets.</div>;
  }

  const empty = reveal.compatible.length === 0 && reveal.greyed.length === 0 && reveal.bound.length === 0;

  return (
    <div id="reveal" data-testid="reveal" style={{ padding: 12, fontSize: 13 }}>
      {reveal.required.length > 0 && (
        <div
          style={{ opacity: 0.7, marginBottom: 6 }}
          title={`This object needs a source of ${reveal.required.join(", ")} — bind it to one of the matches below.`}
        >
          Needs {reveal.required.join(", ")} — pick a match to bind
        </div>
      )}
      {reveal.bound.length > 0 && (
        <div style={{ marginBottom: 8 }}>
          <div style={{ opacity: 0.6, fontSize: 11 }}>tracking</div>
          {reveal.bound.map((b) => (
            <div key={b.id} className="boundrow" data-testid="bound">
              {b.name} <span style={{ opacity: 0.5 }}>· {b.kind}</span>
            </div>
          ))}
        </div>
      )}
      {reveal.compatible.map((c) => (
        <button
          key={c.id}
          className="cand"
          data-testid="candidate"
          data-id={c.id}
          onClick={() => {
            // Feedback AT THE GESTURE (C11): bind-by-intent (north-star #1) was silent — the candidate
            // only moved to "tracking" after the authoritative round-trip, with no toast/status. Confirm
            // optimistically; the bound row + edge follow.
            client.bind(id, "tracks", c.id);
            setStatus(`tracking ${c.name}`);
            pushToast(`bound · now tracking ${c.name}`, "success");
          }}
          title={`Click to bind — this object will track ${c.name} (match ${c.affinity} of 100)`}
          style={{ display: "block", width: "100%", textAlign: "left", margin: "2px 0", padding: "4px 6px", background: "#1c2030", color: "#cde", border: "1px solid #2a3550", borderRadius: 4, cursor: "pointer" }}
        >
          {c.name}{" "}
          <span style={{ opacity: 0.5 }} title="How well this target fits (higher = better match)">
            · match {c.affinity}
          </span>
        </button>
      ))}
      {reveal.greyed.map((g) => (
        <div
          key={g.id}
          className="cand disabled"
          data-testid="greyed"
          data-id={g.id}
          style={{ margin: "2px 0", padding: "4px 6px", color: "#667", border: "1px solid #222", borderRadius: 4 }}
        >
          {g.name} <span style={{ opacity: 0.75 }}>— {g.reason}</span>
        </div>
      ))}
      {empty && <div style={{ color: "#888" }}>no compatible targets</div>}
    </div>
  );
}
