//! Diagnostics (M14.3 / ADR-059) — "every 'no' explained" as a first-class, **structured, actionable**
//! surface, keyed off the **real** registry/relational projection (ADR-058 `rel.needsBinding/requires` + the
//! reveal's `required`/`greyed`-with-reason) — the C6 fix (today 0 requirers surface against the real
//! `/core`). It separates **actionable** diagnostics (an unmet requirement → a one-click **fix**: bind the
//! best-ranked compatible source, or an explained "no source") from **informational** ones (why other
//! targets can't bind — grouped + collapsible). Tests key off the structured `data-severity`/`data-kind`
//! model + the fix affordance, never the rendered prose.

import { useState } from "react";
import { useSelectedId, useSummary } from "../store/projection";
import { useReveal } from "../store/reveal";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button, Badge } from "../theme/primitives";
import { color, font, fontSize, radius, space, text } from "../theme/tokens";
import type { EditorClient } from "../transport/session";

const meta: React.CSSProperties = { font: font.ui, fontSize: fontSize.meta, color: color.text.muted };

export function Diagnostics({ client }: { client: EditorClient }) {
  const id = useSelectedId();
  const summary = useSummary(id ?? "");
  // Shared, deduplicated reveal (perf audit F2) — the Reveal picker reads the same `(id, edgeSig)` key,
  // so the actionable fix + "needs binding" diagnostic update live off ONE round-trip, not a second.
  const reveal = useReveal(client);
  const [showWhy, setShowWhy] = useState(false);

  if (!id) return null;
  const rel = summary?.rel;
  const needs = !!rel?.needsBinding;
  const caps = (rel?.requires?.length ? rel.requires : reveal.required).join(", ") || "a capability";
  const top = reveal.compatible[0];
  const greyed = reveal.greyed;

  const title = (
    <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.sm, ...text.eyebrow }}>
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
          <Badge tone="success"><Icon name="check" size="sm" /></Badge> No issues — this object is fully wired.
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
          // WRAPS, because three things that must all stay readable do not fit on one 320 px line.
          // Without it this row had no third outcome: the badge is fixed, the button would not shrink,
          // so the only give was the message — and `flex: 1` (basis 0%) plus `minWidth: 0` said it was
          // content at ZERO. It measured 0 px wide and painted its sentence straight through the
          // button. The message now claims its own content width and the ACTION is what moves to a
          // second line, which is the right thing to give up: a diagnostic that cannot be read is not
          // a diagnostic, and a button on its own line is still a button.
          style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: space.sm, padding: space.sm, marginBottom: space.xs, border: `1px solid ${color.warn.border}`, borderRadius: radius.md, background: color.warn.bg }}
        >
          <Badge tone="warn">needs binding</Badge>
          {/* `0 1 auto`, not `1 1 auto`: the message takes its content width and yields under
              pressure, but never GROWS into the free space. `flex-grow: 1` was the first repair here
              and it swallowed the whole row, so the button wrapped to its own line at EVERY width —
              a fix for 320 px silently becoming the layout at 1440. The `same_line` assertion on the
              wide scene is what caught that; the number in `looking_for` had said "one line" while
              the capture showed two. */}
          <span style={{ flex: "0 1 auto", font: font.ui, fontSize: fontSize.body, color: color.text.primary }} title={`This object needs a source of ${caps} — bind it to one.`}>
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
              // `.mtk-btn` is `white-space: nowrap` and `min-width: auto`, so a data-derived name in
              // the label makes this button un-shrinkable: with a CAD name it was painted 204 px
              // outside a 320 px drawer — the entire one-click fix, off-screen and unclickable, while
              // the row it belongs to still looked fine. `minWidth: 0` lets the row take the width
              // back; the span below decides what is given up, and it is never the verb.
              // A BOUNDED basis, not `auto`. Under `flex-wrap` the browser assigns items to lines by
              // their hypothetical size before it shrinks anything, so a button whose base size is
              // its 430 px CAD label never fits on a shared line and wraps at EVERY width — which is
              // what the wide scene caught. 12rem is what it asks for; it grows into whatever is left
              // and truncates when it cannot have it, so whether this row wraps is decided by the
              // available width and not by how long one entity happens to be called.
              style={{ flex: "1 1 12rem", minWidth: 0 }}
            >
              {/* Two flex items on purpose, and no `&nbsp;` between them. The verb is its own
                  un-shrinkable item so truncation can never eat it — "Bind t…" is not a control —
                  while the name, which is data, takes the ellipsis. The separator is `.mtk-btn`'s
                  own `gap: 6px`: an `&nbsp;` here ADDS to that rather than replacing it, and the
                  capture showed the ~9 px result. Spacing is the button system's contract; stating
                  it a second time inline is the same two-statements-of-one-contract shape the
                  repository keeps paying for. */}
              Bind to
              <span style={{ minWidth: 0, overflow: "hidden", textOverflow: "ellipsis" }}>{top.name}</span>
            </Button>
          ) : (
            <Badge tone="neutral" title="No compatible source exists in the scene yet — add a provider of this capability.">no source</Badge>
          )}
        </div>
      )}
      {greyed.length > 0 && (
        <div data-testid="diag-greyed">
          <button type="button" className="mtk-group-head" style={{ borderRadius: radius.md }} aria-expanded={showWhy} onClick={() => setShowWhy((s) => !s)}>
            <span className={"mtk-group-caret" + (showWhy ? " is-open" : "")}><Icon name="chevron-right" size="sm" /></span>
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
