//! Asset browser (M10.2 / ADR-031) — a searchable, categorized library over the **ONE M3.4 catalog**
//! (registry + marketplace + imported assets), the bring-your-own-content on-ramp's home. It does NOT
//! fork the search/category logic: it calls the shell's `catalog` (grouped by category, ADR-019) +
//! `catalog_search` (the tiered resolver, ranked + a no-match generate seam), and **place-into-scene** is
//! `add_item` → one undoable, persisted, pre-componentized entity (deliverable 4). Reuse, not duplication.
//!
//! Thumbnails are a placeholder swatch here (the live wgpu thumbnail render is the local-GUI item); the
//! list is keyed for virtualization on the min-spec profile.

import { useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { walletStore, setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";
import type { CatalogItem } from "../transport/protocol";

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
      // place + SELECT the result so it's visible/inspectable (C11 — feedback at the gesture, no silent
      // "added X" with nothing to show). The cost shown is the ACTUAL debit (balance delta), never a
      // catalog-price guess that could disagree with the real charge.
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
    <button
      key={`${item.source}:${item.id}`}
      className="asset-item"
      data-testid="asset-item"
      data-id={item.id}
      data-source={item.source}
      onClick={() => void place(item)}
      title={item.requires.length ? `attaches to ${item.requires.join(", ")}` : item.label}
      style={{ display: "flex", alignItems: "center", gap: 8, width: "100%", textAlign: "left", margin: "2px 0", padding: "4px 6px", background: "#171b27", color: "#cde", border: "1px solid #2a3550", borderRadius: 4, cursor: "pointer" }}
    >
      <span data-testid="asset-thumb" style={{ width: 22, height: 22, borderRadius: 3, background: "#2a3550", flex: "none" }} />
      <span style={{ flex: 1, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{item.label}</span>
      {item.price != null && <span style={{ opacity: 0.6, fontSize: 11 }}>⊞{item.price}</span>}
      <span style={{ opacity: 0.45, fontSize: 10 }}>{item.source}</span>
    </button>
  );

  return (
    <div id="assetbrowser" data-testid="assetbrowser" style={{ padding: "6px 12px", fontSize: 13 }}>
      <input
        id="assetSearch"
        data-testid="asset-search"
        value={query}
        placeholder="Search assets (or describe to generate)…"
        onChange={(e) => void runSearch(e.target.value)}
        style={{ width: "100%", boxSizing: "border-box", marginBottom: 6, background: "#11131a", color: "#cfe", border: "1px solid #333", borderRadius: 3, padding: "3px 6px" }}
      />
      {results !== null ? (
        <div data-testid="asset-results">
          {results.map(card)}
          {seam && (
            <div data-testid="asset-seam" style={{ color: "#c9a7ff", padding: "4px 6px" }}>
              no catalog match — {seam} “{query}”?
            </div>
          )}
        </div>
      ) : (
        Object.entries(groups).map(([category, items]) => (
          <div key={category} data-testid="asset-category" data-category={category} style={{ marginBottom: 6 }}>
            <div style={{ opacity: 0.55, fontSize: 11, margin: "4px 0 2px" }}>{category}</div>
            {items.map(card)}
          </div>
        ))
      )}
    </div>
  );
}
