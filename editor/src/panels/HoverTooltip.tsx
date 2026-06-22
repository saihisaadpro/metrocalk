//! HoverTooltip (M3.3) — a read-only inspector that surfaces an entity's shape on hover: its name, its
//! component list, and its capability contract (provides / requires / boundTo). Hover MUST be inert — it
//! never selects, never mutates — so this reads `entityDetails` through the `EditorClient` and renders only.
//! When there's nothing under the cursor (`id == null`) or the lookup yields nothing (`details == null`),
//! the surface collapses to nothing (returns null) so an absent tooltip leaves no stray DOM.
//!
//! The `id="tooltip"` / `data-testid="tooltip"` root mirrors the vanilla scaffold's stable signal, and each
//! capability section is omitted when empty so the tooltip stays terse for sparse entities.

import { useEffect, useState } from "react";
import type { EditorClient } from "../transport/session";
import type { EntityDetails } from "../transport/protocol";

export function HoverTooltip({ client, id }: { client: EditorClient; id: string | null }) {
  const [details, setDetails] = useState<EntityDetails | null>(null);

  useEffect(() => {
    if (!id) {
      setDetails(null);
      return;
    }
    let live = true;
    client
      .entityDetails(id)
      .then((d) => {
        if (live) setDetails(d);
      })
      .catch(() => {
        if (live) setDetails(null);
      });
    return () => {
      live = false;
    };
  }, [id, client]);

  // Nothing under the cursor, or the lookup found nothing → render nothing at all.
  if (!id || !details) return null;

  return (
    <div
      id="tooltip"
      data-testid="tooltip"
      role="tooltip"
      style={{
        padding: "8px 10px",
        fontSize: 12,
        background: "#14161f",
        color: "#cde",
        border: "1px solid #2a3550",
        borderRadius: 6,
        maxWidth: 280,
        pointerEvents: "none",
      }}
    >
      <div data-testid="tooltip-name" style={{ fontWeight: 600, marginBottom: 4 }}>
        {details.name}
      </div>
      {details.components.length > 0 && (
        <Section label="components" items={details.components} testid="tooltip-components" />
      )}
      {details.provides.length > 0 && (
        <Section label="provides" items={details.provides} testid="tooltip-provides" />
      )}
      {details.requires.length > 0 && (
        <Section label="requires" items={details.requires} testid="tooltip-requires" />
      )}
      {details.boundTo.length > 0 && (
        <Section label="tracking" items={details.boundTo} testid="tooltip-boundto" />
      )}
    </div>
  );
}

function Section({ label, items, testid }: { label: string; items: string[]; testid: string }) {
  return (
    <div data-testid={testid} style={{ marginTop: 2 }}>
      <span style={{ opacity: 0.55, marginRight: 6 }}>{label}</span>
      <span>{items.join(", ")}</span>
    </div>
  );
}
