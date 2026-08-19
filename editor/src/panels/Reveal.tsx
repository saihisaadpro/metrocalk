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
import { Button, Surface } from "../theme/primitives";
import { color, fontSize, space } from "../theme/tokens";
import { EmptyPanelState } from "../theme/workspace";
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
    return (
      <EmptyPanelState
        compact
        icon="⌘"
        title="Select an object to find compatible targets"
        description="Ranked matches and clear incompatibility reasons will appear here."
      />
    );
  }

  const empty = reveal.compatible.length === 0 && reveal.greyed.length === 0 && reveal.bound.length === 0;

  return (
    <div id="reveal" data-testid="reveal" style={{ padding: space.lg, fontSize: fontSize.body }}>
      {reveal.required.length > 0 && (
        <div
          style={{ color: color.text.secondary, marginBottom: space.sm, lineHeight: 1.45 }}
          title={`This object needs a source of ${reveal.required.join(", ")} — bind it to one of the matches below.`}
        >
          Needs {reveal.required.join(", ")} — pick a match to bind
        </div>
      )}
      {reveal.bound.length > 0 && (
        <div style={{ marginBottom: space.md }}>
          <div style={{ color: color.text.muted, fontSize: fontSize.micro, marginBottom: space.xs }}>Tracking</div>
          {reveal.bound.map((b) => (
            <Surface key={b.id} tone="inset" className="boundrow" data-testid="bound" style={{ padding: space.md, marginBottom: space.xs }}>
              {b.name} <span style={{ color: color.text.muted }}>· {b.kind}</span>
            </Surface>
          ))}
        </div>
      )}
      {reveal.compatible.map((c) => (
        <Button
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
          style={{ display: "flex", width: "100%", justifyContent: "flex-start", marginBottom: space.xs, textAlign: "left" }}
        >
          {/* The name is DATA — a CAD import names entities "Overhead Crane Assembly Rev C — Long
              Travel Girder" — and `.mtk-btn` is `white-space: nowrap`, which is right for an authored
              label and wrong for one that arrives from a file. Left as a bare text node the row grew
              to its content and pushed the score 148 px outside a 320 px drawer, where the user could
              neither read it nor scroll to it. So the NAME yields (it truncates, and the button's
              `title` still carries it in full) and the SCORE never does: it is the ranking this whole
              list exists to express, and a candidate list without its ranking is just a list. */}
          <span style={{ flex: "1 1 auto", minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>
            {c.name}
          </span>
          <span
            style={{ flex: "0 0 auto", color: color.text.muted }}
            title="How well this target fits (higher = better match)"
          >
            · match {c.affinity}
          </span>
        </Button>
      ))}
      {reveal.greyed.map((g) => (
        <Surface
          key={g.id}
          tone="inset"
          className="cand disabled"
          data-testid="greyed"
          data-id={g.id}
          style={{ marginBottom: space.xs, padding: space.md, color: color.text.muted }}
        >
          {/* The reason is the WHOLE POINT of a greyed row — north-star #1 is "every no explained",
              and a no whose explanation is dimmed is a no. `opacity: 0.75` cost this sentence
              5.00:1 → 3.07:1 on the inset surface, which is under AA even after the token repair:
              a ratio the palette cannot predict, because the palette never sees the compositing.
              The row is already de-emphasised by its surface and its position under the matches. */}
          {g.name} <span>— {g.reason}</span>
        </Surface>
      ))}
      {empty && <div style={{ color: color.text.muted }}>No compatible targets</div>}
    </div>
  );
}
