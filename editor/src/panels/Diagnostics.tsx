//! Diagnostics (M14.3 / ADR-059) — "every 'no' explained" as a first-class, **structured, actionable**
//! surface, keyed off the **real** registry/relational projection (ADR-058 `rel.needsBinding/requires` + the
//! reveal's `required`/`greyed`-with-reason) — the C6 fix (today 0 requirers surface against the real
//! `/core`). It separates **actionable** diagnostics (an unmet requirement → a one-click **fix**: bind the
//! best-ranked compatible source, or an explained "no source") from **informational** ones (why other
//! targets can't bind — grouped + collapsible). Tests key off the structured `data-severity`/`data-kind`
//! model + the fix affordance, never the rendered prose.

import { useEffect, useState } from "react";
import { useSelectedId, useSummary, useEdges } from "../store/projection";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Button, Badge } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";
import type { RevealResponse } from "../transport/protocol";

const EMPTY: RevealResponse = { required: [], compatible: [], greyed: [], bound: [] };
const meta: React.CSSProperties = { font: font.ui, fontSize: fontSize.meta, color: color.text.muted };

export function Diagnostics({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const summary = useSummary(id ?? "");
  const edges = useEdges();
  const [reveal, setReveal] = useState<RevealResponse>(EMPTY);
  const [showWhy, setShowWhy] = useState(false);

  // Re-fetch the reveal when this entity's OUTGOING edges change (a bind/undo) — so the actionable fix +
  // the "needs binding" diagnostic update live. Same debounced, off-frame command the Reveal picker uses.
  const edgeSig = id
    ? Object.values(edges).filter((e) => e.from === id).map((e) => `${e.id}:${e.status}`).sort().join(",")
    : "";
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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id, client, edgeSig]);

  if (!id) return null;
  const rel = summary?.rel;
  const needs = !!rel?.needsBinding;
  const caps = (rel?.requires?.length ? rel.requires : reveal.required).join(", ") || "a capability";
  const top = reveal.compatible[0];
  const greyed = reveal.greyed;

  const title = (
    <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.sm, font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase", color: color.text.secondary }}>
      <span>Diagnostics</span>
      {needs && <Badge tone="warn">1</Badge>}
    </div>
  );

  // No diagnostics → an honest "all clear" (never a blank pane that reads as broken).
  if (!needs && greyed.length === 0) {
    return (
      <div id="diagnostics" data-testid="diagnostics" style={{ padding: space.lg }}>
        {title}
        <div style={{ ...meta, display: "flex", alignItems: "center", gap: space.sm }} data-testid="diag-clear">
          <Badge tone="success">✓</Badge> No issues — this object is fully wired.
        </div>
      </div>
    );
  }

  return (
    <div id="diagnostics" data-testid="diagnostics" style={{ padding: space.lg }}>
      {title}
      {needs && (
        <div
          data-testid="diag-row"
          data-severity="error"
          data-kind="needs-binding"
          style={{ display: "flex", alignItems: "center", gap: space.sm, padding: space.sm, marginBottom: space.xs, border: `1px solid ${color.warn.border}`, borderRadius: radius.md, background: color.warn.bg }}
        >
          <Badge tone="warn">needs binding</Badge>
          <span style={{ flex: 1, minWidth: 0, font: font.ui, fontSize: fontSize.body, color: color.text.primary }} title={`This object needs a source of ${caps} — bind it to one.`}>
            Needs a <strong>{caps}</strong> source
          </span>
          {top ? (
            <Button
              data-testid="diag-fix"
              variant="primary"
              compact
              onClick={() => {
                client.bind(id, "tracks", top.id);
                setStatus(`tracking ${top.name}`);
                pushToast(`bound · now tracking ${top.name}`, "success");
              }}
              title={`Bind to ${top.name} — the best-ranked compatible source (match ${top.affinity})`}
            >
              Bind to {top.name}
            </Button>
          ) : (
            <Badge tone="neutral" title="No compatible source exists in the scene yet — add a provider of this capability.">no source</Badge>
          )}
        </div>
      )}
      {greyed.length > 0 && (
        <div data-testid="diag-greyed">
          <button type="button" className="mtk-group-head" style={{ borderRadius: radius.md }} aria-expanded={showWhy} onClick={() => setShowWhy((s) => !s)}>
            <span className={"mtk-group-caret" + (showWhy ? " is-open" : "")}>▸</span>
            Why {greyed.length} other{greyed.length > 1 ? "s" : ""} can’t bind
          </button>
          {showWhy && (
            <div style={{ padding: `${space.xs}px ${space.sm}px` }}>
              {greyed.map((g) => (
                <div key={g.id} data-testid="diag-greyed-row" data-severity="info" style={{ ...meta, padding: "1px 0" }} title={g.reason}>
                  <span style={{ color: color.text.secondary }}>{g.name}</span> — {g.reason}
                </div>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
