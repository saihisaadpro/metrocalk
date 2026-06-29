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

import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { Thumbnail } from "../theme/Thumbnail";
import { Badge } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";

export function Requirers() {
  // Subscribe to the summary map so a relational FLIP (a bind → needsBinding false) updates the list live.
  // Reads structured `rel.needsBinding` — the authoritative requirer signal off the projection (C6).
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const requirers = Object.values(summaries)
    .filter((s) => s.rel?.needsBinding)
    .slice(0, 60); // rare in the scene; cap the quick-pick list (the scaffold's bound)

  return (
    <div id="requirers" data-testid="requirers" style={{ padding: `${space.md}px ${space.lg}px` }}>
      <div style={{ display: "flex", alignItems: "baseline", gap: space.sm, marginBottom: space.sm, font: font.ui, fontSize: fontSize.meta, fontWeight: 600, letterSpacing: 0.4, textTransform: "uppercase", color: color.text.secondary }}>
        <span>Needs binding</span>
        {requirers.length > 0 && <Badge tone="accent">{requirers.length}</Badge>}
      </div>
      {requirers.length === 0 ? (
        <div style={{ color: color.text.muted, fontSize: fontSize.body }}>none found</div>
      ) : (
        requirers.map((s) => (
          <button
            key={s.id}
            type="button"
            className="cand mtk-card"
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
          </button>
        ))
      )}
    </div>
  );
}
