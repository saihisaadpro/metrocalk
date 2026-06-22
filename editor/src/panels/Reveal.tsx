//! Bind-by-intent reveal (north-star #1, M10.1 parity) — select an entity → its ranked compatible bind
//! targets appear (proximity·affinity, ADR-011), incompatible ones are GREYED **with the reason** ("every
//! 'no' explained", ADR-016), existing bindings show as "tracking …". One click on a candidate binds it
//! (optimistic; the authoritative edge streams back over the Channel). Reads the reveal from the shell's
//! `reveal_targets` command via the `EditorClient`; subscribes only to `selectedId`.
//!
//! The `.cand` / `.boundrow` / `disabled` class hooks + `data-id` mirror the vanilla scaffold's stable
//! signals, so the prompt-40 acceptance page-object re-greens by selector-swap, not a spec rewrite.

import { useEffect, useState } from "react";
import { useSelectedId } from "../store/projection";
import type { EditorClient } from "../transport/session";
import type { RevealResponse } from "../transport/protocol";

const EMPTY: RevealResponse = { required: [], compatible: [], greyed: [], bound: [] };

export function Reveal({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const [reveal, setReveal] = useState<RevealResponse>(EMPTY);

  useEffect(() => {
    if (!id) {
      setReveal(EMPTY);
      return;
    }
    let live = true;
    client
      .revealTargets(id)
      .then((r) => {
        if (live) setReveal(r);
      })
      .catch(() => {
        if (live) setReveal(EMPTY);
      });
    return () => {
      live = false;
    };
  }, [id, client]);

  if (!id) {
    return <div style={{ padding: 12, color: "#888" }}>Select an entity to see compatible bind targets.</div>;
  }

  const empty = reveal.compatible.length === 0 && reveal.greyed.length === 0 && reveal.bound.length === 0;

  return (
    <div id="reveal" data-testid="reveal" style={{ padding: 12, fontSize: 13 }}>
      {reveal.required.length > 0 && (
        <div style={{ opacity: 0.7, marginBottom: 6 }}>requires {reveal.required.join(", ")}</div>
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
          onClick={() => client.bind(id, "tracks", c.id)}
          style={{ display: "block", width: "100%", textAlign: "left", margin: "2px 0", padding: "4px 6px", background: "#1c2030", color: "#cde", border: "1px solid #2a3550", borderRadius: 4, cursor: "pointer" }}
        >
          {c.name} <span style={{ opacity: 0.5 }}>· affinity {c.affinity}</span>
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
