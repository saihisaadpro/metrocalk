//! Requirers — the quick-pick list of the entities that REQUIRE a capability (the scaffold's
//! `#requirers`; a HealthBar "requires Health"). These are the rare, bindable starting points for
//! north-star test #1: they're a needle in a 5k-entity haystack, so surface them directly. One click
//! selects the requirer → the Reveal (bind-by-intent) panel populates with its compatible targets.
//!
//! A requirer is identified from the projection by the real `/core` requirer MARKER component —
//! **`HealthBar`** (the scaffold's proven signal; capabilities like `requires Health` are ECS pairs, NOT
//! projected as components, so a requirer can't be found by a "needs X" field). Derived from
//! `useEntityOrder()` + `projectionStore.getState().displayed[id]` (the read-model is the single source of
//! truth, invariant 1; this panel holds NO state of its own). The `.cand` / `data-id` hooks mirror the
//! vanilla scaffold's stable signals, so the prompt-40 acceptance page-object keys on the same selectors.

import { projectionStore, useEntityOrder } from "../store/projection";

export function Requirers() {
  // Subscribe to the entity order so a (re)load re-renders this list; read the components from the
  // `displayed` read-model to keep the filter on the optimistic-overlay projection (invariant 1).
  const order = useEntityOrder();
  const displayed = projectionStore.getState().displayed;

  const requirers = order
    .map((id) => displayed[id])
    .filter((e): e is NonNullable<typeof e> => !!e && "HealthBar" in e.components)
    .slice(0, 60); // rare in the scene; cap the quick-pick list (the scaffold's bound)

  return (
    <div id="requirers" data-testid="requirers" style={{ padding: 12, fontSize: 13 }}>
      {requirers.length === 0 ? (
        <div style={{ color: "#888" }}>none found</div>
      ) : (
        requirers.map((e) => (
          <div
            key={e.id}
            className="cand"
            data-testid="requirer"
            data-id={e.id}
            onClick={() => projectionStore.getState().select(e.id)}
            title="Click to see the compatible targets this can bind to."
            style={{
              padding: "4px 6px",
              margin: "2px 0",
              cursor: "pointer",
              color: "#cde",
              borderLeft: "3px solid #34d399",
              borderRadius: 4,
            }}
          >
            {e.name}
            <span style={{ opacity: 0.55, fontSize: 11 }}> · needs a binding</span>
          </div>
        ))
      )}
    </div>
  );
}
