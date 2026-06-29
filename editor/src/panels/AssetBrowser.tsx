//! Asset browser (M10.2 / ADR-031) — a searchable, categorized library over the **ONE M3.4 catalog**
//! (registry + marketplace + imported assets), the bring-your-own-content on-ramp's home. It does NOT
//! fork the search/category logic: it calls the shell's `catalog` (grouped by category, ADR-019) +
//! `catalog_search` (the tiered resolver, ranked + a no-match generate seam), and **place-into-scene** is
//! `add_item` → one undoable, persisted, pre-componentized entity (deliverable 4). Reuse, not duplication.
//!
//! **M14.2 (ADR-058) — cards to the accepted tier.** Each card surfaces the **real registry truth**: the
//! **source-tier** (local · marketplace · AI-generated, visually distinct icon + badge) and the **capability
//! state** (what it *provides* / what it *needs* to attach), read from the real catalog entry — not a static
//! swatch. A catalog item isn't in the scene yet, so it carries the styled **type-icon** (the live per-entity
//! RTT thumbnail is for placed scene entities — the hierarchy); the M7 provenance panel is M14.3.

import { useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { walletStore, setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import { Card, TypeIcon, Badge } from "../theme/primitives";
import { color, font, fontSize, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";
import type { CatalogItem } from "../transport/protocol";

const sourceTone = (src: string): "neutral" | "accent" =>
  src === "marketplace" || src === "generated" ? "accent" : "neutral";

export function AssetBrowser({ client }: { client: EditorClient }) {
  const [groups, setGroups] = useState<Record<string, CatalogItem[]>>({});
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<CatalogItem[] | null>(null); // null = browse mode
  const [seam, setSeam] = useState<string | null>(null);

  // Load the one catalog on mount (grouped by category).
  useEffect(() => {
    let live = true;
    client
      .catalog()
      .then((g) => {
        if (live) setGroups(g);
      })
      .catch(() => {
        if (live) setGroups({});
      });
    return () => {
      live = false;
    };
  }, [client]);

  async function runSearch(q: string) {
    setQuery(q);
    if (!q.trim()) {
      setResults(null);
      setSeam(null);
      return;
    }
    const r = await client.catalogSearch(q);
    setResults(r.items);
    setSeam(r.items.length === 0 ? (r.seam ?? "generate") : null);
  }

  async function place(item: CatalogItem) {
    const before = walletStore.getState().balance;
    const r = await client.addItem(item.id, item.source);
    if (r.balance != null) setBalance(r.balance); // a marketplace buy debits — keep the wallet legible
    if (r.created) {
      // place + SELECT the result so it's visible/inspectable (C11 — feedback at the gesture). The cost shown
      // is the ACTUAL debit (balance delta), never a catalog-price guess that could disagree with the charge.
      projectionStore.getState().select(r.created);
      const spent = before != null && r.balance != null ? before - r.balance : null;
      const cost = spent && spent > 0 ? ` · −${spent} tokens` : "";
      pushToast(`Added ${item.label} · ${item.source}${cost}`, "success");
      setStatus(`added ${item.label} · ${item.source}`);
    } else if (r.seam) {
      pushToast(`${item.label}: ${r.seam}`, "error");
      setStatus(`${item.label}: ${r.seam}`);
    }
  }

  const card = (item: CatalogItem) => (
    <Card
      key={`${item.source}:${item.id}`}
      data-testid="asset-item"
      data-id={item.id}
      data-source={item.source}
      onClick={() => void place(item)}
      title={item.requires.length ? `attaches to ${item.requires.join(", ")}` : item.label}
      style={{ marginBottom: space.xxs }}
    >
      <TypeIcon kind={item.source} size={26} />
      <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: 1 }}>
        <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", font: font.ui, fontSize: fontSize.body, color: color.text.primary }}>
          {item.label}
        </span>
        <div style={{ display: "flex", gap: space.xs, alignItems: "center", overflow: "hidden", whiteSpace: "nowrap" }}>
          <Badge tone={sourceTone(item.source)}>{item.source}</Badge>
          {item.provides.length > 0 && (
            <Badge tone="success" title={`provides ${item.provides.join(", ")}`}>+{item.provides[0]}</Badge>
          )}
          {item.requires.length > 0 && (
            <Badge tone="neutral" title={`needs ${item.requires.join(", ")}`}>↳{item.requires[0]}</Badge>
          )}
        </div>
      </div>
      {item.price != null && (
        <Badge tone="warn" title={`${item.price} tokens`} style={{ color: color.token }}>
          ⊞{item.price}
        </Badge>
      )}
    </Card>
  );

  return (
    <div id="assetbrowser" data-testid="assetbrowser" style={{ padding: `${space.sm}px ${space.lg}px` }}>
      <input
        className="mtk-input"
        id="assetSearch"
        data-testid="asset-search"
        value={query}
        placeholder="Search assets (or describe to generate)…"
        onChange={(e) => void runSearch(e.target.value)}
        style={{ width: "100%", boxSizing: "border-box", marginBottom: space.sm }}
      />
      {results !== null ? (
        <div data-testid="asset-results">
          {results.map(card)}
          {seam && (
            <div data-testid="asset-seam" style={{ color: color.accent.base, padding: `${space.xs}px ${space.sm}px`, fontSize: fontSize.body }}>
              no catalog match — {seam} “{query}”?
            </div>
          )}
        </div>
      ) : (
        Object.entries(groups).map(([category, items]) => (
          <div key={category} data-testid="asset-category" data-category={category} style={{ marginBottom: space.sm }}>
            <div style={{ font: font.ui, fontSize: fontSize.meta, fontWeight: 600, color: color.text.muted, margin: `${space.xs}px 0 ${space.xxs}px` }}>{category}</div>
            {items.map(card)}
          </div>
        ))
      )}
    </div>
  );
}
