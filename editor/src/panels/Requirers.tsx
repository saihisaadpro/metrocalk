//! Requirers — the quick-pick list of entities that still NEED a binding (the scaffold's `#requirers`; the
//! rare, bindable starting points for north-star test #1 — a needle in a 5k-entity haystack, so surface
//! them directly). One click selects the requirer → the Reveal (bind-by-intent) panel populates.
//!
//! **M14.2 (ADR-058) — the C6 closure.** A requirer is identified from the **projected relational summary**
//! `rel.needsBinding` — keyed off the REAL `/core` `(Requires, cap)` ECS pairs (not the brittle `HealthBar`
//! component-name filter, which missed every other requirer kind and false-positived an already-bound one).
//! The summary is the single source of truth (invariant 1; this panel holds NO state of its own), so a
//! successful bind flips `needsBinding` and the row leaves this list live. The `.cand` / `data-id` hooks
//! mirror the vanilla scaffold's stable signals so the prompt-40 page-object keys on the same selectors.
//!
//! **ADR-170 — two homes, one definition.** It answers "what is waiting for me?", which is a question with
//! no selection in it, so it now also fills the Inspector's no-selection state (`InspectorEmpty`) as well as
//! the left dock's `Needs attention` popover. `hideWhenEmpty` is what makes that safe: a panel that is only
//! ever shown when it has something to say must be able to render nothing, and the popover — which the user
//! opened deliberately and is owed an answer — must still say "none found".

import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Thumbnail } from "../theme/Thumbnail";
import { Badge, Card } from "../theme/primitives";
import { color, font, fontSize, space, text } from "../theme/tokens";

export interface RequirersProps {
  /** Render nothing at all when no entity is waiting — for a surface this list is a GUEST on. */
  hideWhenEmpty?: boolean;
}

/** THE REQUIRER RULE, STATED ONCE. A caller that has to decide whether to draw a heading, a hairline or
 *  a whole section around this list needs the count *before* it renders — and the moment it re-derives
 *  `rel.needsBinding` itself there are two answers to "what is waiting?" that can disagree. */
export function useRequirers() {
  // Subscribe to the summary map so a relational FLIP (a bind → needsBinding false) updates the list live.
  // Reads structured `rel.needsBinding` — the authoritative requirer signal off the projection (C6).
  const summaries = useStore(projectionStore, (s) => s.summaries);
  return Object.values(summaries)
    .filter((s) => s.rel?.needsBinding)
    .slice(0, 60); // rare in the scene; cap the quick-pick list (the scaffold's bound)
}

export function Requirers({ hideWhenEmpty = false }: RequirersProps = {}) {
  const requirers = useRequirers();

  if (requirers.length === 0 && hideWhenEmpty) return null;

  return (
    <div id="requirers" data-testid="requirers" style={{ padding: `${space.md}px ${space.lg}px` }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.sm, ...text.eyebrow }}>
        <span>Needs binding</span>
        {requirers.length > 0 && <Badge tone="accent">{requirers.length}</Badge>}
      </div>
      {requirers.length === 0 ? (
        <div style={{ color: color.text.muted, fontSize: fontSize.body }}>none found</div>
      ) : (
        requirers.map((s) => (
          <Card
            key={s.id}
            className="cand"
            data-testid="requirer"
            data-id={s.id}
            onClick={() => {
              projectionStore.getState().select(s.id);
              setStatus(`selected ${s.name} — see its compatible bind targets`);
            }}
            title={`Requires ${s.rel?.requires.join(", ") || "a capability"} — click to see the compatible targets this can bind to.`}
            style={{ marginBottom: space.xxs }}
          >
            <Thumbnail id={s.id} kind="requirer" size={20} />
            <span style={{ flex: 1, minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
              {s.name}
            </span>
            <Badge tone="accent">needs {s.rel?.requires[0] ?? "binding"}</Badge>
          </Card>
        ))
      )}
    </div>
  );
}
