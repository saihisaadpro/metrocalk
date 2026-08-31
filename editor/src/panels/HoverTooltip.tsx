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
import { entityLabel } from "../store/selectionText";
import { PopoverSurface } from "../theme/Popover";
import { color, fontSize, space } from "../theme/tokens";

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
    <PopoverSurface
      id="tooltip"
      data-testid="tooltip"
      role="tooltip"
      style={{
        maxWidth: 280,
        color: color.text.secondary,
        fontSize: fontSize.meta,
        pointerEvents: "none",
      }}
    >
      {/* ONE NAMER. `selectionText` is where the editor decides what to call something, "because it is
          said in four" places and they drift. This surface was the fifth, and it disagreed with the
          rest the moment it became reachable: measured on the packaged `.exe`, hovering the object the
          outliner calls `Empty 6` and the object menu calls `Empty 6` printed **`1_14`** here — the
          engine's own `entity_details.name`, which for an unnamed entity is its Loro key. A raw key in
          user copy is exactly `<ux_quality>` 4. The engine's name is kept as the fallback for the case
          the projection cannot answer (an entity not in the current projection at all), where it is
          strictly better than nothing. */}
      <div data-testid="tooltip-name" style={{ color: color.text.primary, fontWeight: 650, marginBottom: space.xs }}>
        {entityLabel(id) === id ? details.name : entityLabel(id)}
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
    </PopoverSurface>
  );
}

function Section({ label, items, testid }: { label: string; items: string[]; testid: string }) {
  return (
    <div data-testid={testid} style={{ marginTop: space.xxs }}>
      <span style={{ color: color.text.muted, marginRight: space.sm }}>{label}</span>
      <span>{items.join(", ")}</span>
    </div>
  );
}
