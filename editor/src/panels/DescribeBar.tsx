//! DescribeBar — describe-to-create (north-star #2). Free text → the core resolves it across the
//! tiers (local → marketplace → generate seam, ADR-004): on Create (or Enter) we call
//! `client.describe(query)` and surface a STABLE status derived from the structured result —
//! created+kind ("created HealthBar · local") for a local/marketplace hit, the explained generate
//! seam ("no local or marketplace match — Generate?") when nothing matched, or the refusal seam
//! ("insufficient …") when a marketplace buy is refused broke. When `created` is non-null we also
//! `select(created)` so its attach targets reveal in the bind panel (the same flow the scaffold's
//! `doDescribe` ran). Reads nothing from the store; drives the projection store's `select` + the
//! ephemeral status line via `setStatus`.
//!
//! Keeps the vanilla scaffold's stable `#describe` / `#describeBtn` ids (plus `data-testid`s) so the
//! prompt-40 acceptance page-object re-greens by selector-swap, not a spec rewrite.

import { useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";
import type { DescribeResponse } from "../transport/protocol";

/** Build the STABLE status line from the structured describe result (no free-form interpolation of
 *  the query into the success message — the kind/source/seam are the load-bearing, assertable bits). */
function describeStatus(query: string, r: DescribeResponse): string {
  if (r.created) {
    const kind = r.kind ?? "entity";
    if (r.source === "marketplace") {
      const econ = r.price != null ? ` · −${r.price} tokens (creator keeps ~70%)` : "";
      const left = r.balance != null ? ` · ⊞ ${r.balance} left` : "";
      return `marketplace: bought ${kind} · ${r.created}${econ}${left}`;
    }
    return `created ${kind} · ${r.created} · local (free)`;
  }
  if (r.seam && r.seam.startsWith("insufficient")) {
    // Marketplace buy refused (broke) — surface the honest "top up?" seam verbatim, no scene change.
    return r.seam;
  }
  if (r.seam === "generate") {
    // Tier 3, opt-in: nothing local or on the marketplace — the explained generate seam.
    return `no local or marketplace match for "${query}" — Generate? (≈10 tokens, last resort)`;
  }
  return `no match for "${query}"`;
}

export function DescribeBar({ client }: { client: EditorClient }) {
  const [query, setQuery] = useState("");

  async function submit() {
    const q = query.trim();
    if (!q) return;
    const r = await client.describe(q);
    setStatus(describeStatus(q, r));
    // A hit drops a pre-componentized entity (one undoable tx, echoed over the Channel); focus it so
    // its compatible bind targets reveal — the same select the scaffold ran after a describe hit.
    if (r.created) projectionStore.getState().select(r.created);
  }

  return (
    <div data-testid="describebar" style={{ display: "flex", gap: 6, padding: "6px 12px" }}>
      <input
        id="describe"
        data-testid="describe"
        value={query}
        placeholder="or describe: e.g. health bar"
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void submit();
        }}
        style={{ flex: 1, background: "#11131a", color: "#cfe", border: "1px solid #333", borderRadius: 3, padding: "2px 6px" }}
      />
      <button
        id="describeBtn"
        data-testid="describeBtn"
        onClick={() => void submit()}
        style={{ background: "#2a4365", color: "#fff", border: "none", borderRadius: 4, padding: "3px 10px", cursor: "pointer" }}
      >
        Create
      </button>
    </div>
  );
}
