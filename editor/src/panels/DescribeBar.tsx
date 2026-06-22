//! DescribeBar — describe-to-create (north-star #2), the front door (M10.10 / C1). The bar owns its FULL
//! outcome in one place: a local/marketplace **match → place + select** the result; **no match → an explicit
//! inline panel under the field** with actionable controls — ［ ✦ Generate with AI · ~N tokens ］ ·
//! ［ Browse marketplace ］ · ［ Build manually ］ — never a passive, button-less footer line ("Generate?",
//! "last resort"). Generate → a progress state → place + select the generated result (the M6 placeholder-
//! first stream-in, ADR-017). Create is **disabled while the field is empty** (C5 — no enabled-inert CTA),
//! and every outcome surfaces a toast AT THE GESTURE (C11), not only the status gutter.
//!
//! Keeps the scaffold's stable `#describe`/`#describeBtn` ids AND restores the `#genBtn` the prompt-40
//! acceptance gate drives (the React port had dropped it — the literal C1 bug).

import { useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import { GENERATE_COST } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** The inline outcome panel under the field: the no-match generate offer, the in-progress generate state,
 *  or an explained refusal (a marketplace/generation buy refused when broke — every "no" explained). */
type Panel =
  | null
  | { kind: "offer"; query: string }
  | { kind: "generating"; query: string }
  | { kind: "refusal"; message: string };

export function DescribeBar({ client }: { client: EditorClient }) {
  const [query, setQuery] = useState("");
  const [panel, setPanel] = useState<Panel>(null);
  const empty = query.trim().length === 0;

  async function submit() {
    const q = query.trim();
    if (!q) return; // empty guard (Create is also disabled) — never a silent inert CTA (C5)
    setPanel(null);
    const r = await client.describe(q);
    if (r.balance != null) setBalance(r.balance);

    // MATCH → place + select the result, right where the action started (the loop closes here).
    if (r.created) {
      const cost = r.source === "marketplace" && r.price != null ? ` · −${r.price} tokens` : "";
      projectionStore.getState().select(r.created);
      setQuery("");
      pushToast(`Created ${r.kind ?? "object"} · ${r.source ?? "local"}${cost}`, "success");
      setStatus(`created ${r.kind ?? "entity"} · ${r.created} · ${r.source ?? "local"}`);
      return;
    }

    // A marketplace buy refused (broke) → the explained seam (verbatim), no scene change.
    if (r.seam && r.seam.startsWith("insufficient")) {
      setPanel({ kind: "refusal", message: r.seam });
      setStatus(r.seam);
      return;
    }

    // NO MATCH anywhere → the explicit, actionable generate offer (C1) — NOT a passive footer line.
    setPanel({ kind: "offer", query: q });
    setStatus(`no local or marketplace match for "${q}" — Generate with AI? (~${GENERATE_COST} tokens)`);
  }

  /** The opt-in, METERED tier-3 generate (M6/ADR-017): a deliberate, priced click → a progress state →
   *  place + select the result (the real mesh streams in over the projection Channel on the `.exe`). */
  async function runGenerate(q: string) {
    setPanel({ kind: "generating", query: q });
    setStatus(`generating "${q}" … (~${GENERATE_COST} tokens)`);
    pushToast(`Generating "${q}" … ~${GENERATE_COST} tokens`, "cost");
    const r = await client.generate(q);
    if (r.balance != null) setBalance(r.balance);
    if (r.created) {
      projectionStore.getState().select(r.created);
      setQuery("");
      setPanel(null);
      const cost = r.cost != null ? ` · −${r.cost} tokens` : "";
      const left = r.balance != null ? ` · ${r.balance} left` : "";
      pushToast(`Generated · placed${cost}${left}`, "success");
      setStatus(`generated · ${r.created}${cost}`);
      return;
    }
    // unavailable / refused-when-broke → explain inline + a toast; no silent debit (the reserve was released).
    const msg = r.seam ?? "generation unavailable";
    setPanel({ kind: "refusal", message: msg });
    pushToast(msg, "error");
    setStatus(msg);
  }

  /** Browse the asset library (the *browse* door to creation) — focus the asset search with the query. */
  function browseMarketplace(q: string) {
    setPanel(null);
    const el = document.getElementById("assetSearch") as HTMLInputElement | null;
    el?.focus();
    pushToast(`Browse the asset library (left panel) for "${q}"`, "info");
    setStatus(`browsing the asset library for "${q}"`);
  }

  const btn = (bg: string, fg: string, border: string): React.CSSProperties => ({
    background: bg,
    color: fg,
    border: `1px solid ${border}`,
    borderRadius: 4,
    padding: "4px 10px",
    cursor: "pointer",
    font: "12px ui-monospace, monospace",
  });

  return (
    <div data-testid="describebar" style={{ padding: "6px 12px" }}>
      <div style={{ display: "flex", gap: 6 }}>
        <input
          id="describe"
          data-testid="describe"
          value={query}
          placeholder="Describe something to create — e.g. “a glowing health bar”"
          onChange={(e) => {
            setQuery(e.target.value);
            // typing invalidates a stale offer/refusal so feedback never goes stale (C11)
            setPanel((p) => (p && p.kind !== "generating" ? null : p));
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
          }}
          style={{ flex: 1, background: "#11131a", color: "#cfe", border: "1px solid #333", borderRadius: 3, padding: "2px 6px" }}
        />
        <button
          id="describeBtn"
          data-testid="describeBtn"
          disabled={empty}
          title={empty ? "Describe something first" : undefined}
          onClick={() => void submit()}
          style={{
            background: empty ? "#23262e" : "#2a4365",
            color: empty ? "#667" : "#fff",
            border: "none",
            borderRadius: 4,
            padding: "3px 10px",
            cursor: empty ? "not-allowed" : "pointer",
          }}
        >
          Create
        </button>
      </div>

      {panel?.kind === "offer" && (
        <div
          data-testid="describePanel"
          style={{ marginTop: 6, padding: "8px 10px", background: "#171b27", border: "1px solid #2a3550", borderRadius: 6, fontSize: 12 }}
        >
          <div style={{ marginBottom: 6, color: "#cdd" }}>
            No match for “{panel.query}”. Generate it with AI, browse the asset library, or build it yourself.
          </div>
          <div style={{ display: "flex", gap: 8, flexWrap: "wrap" }}>
            <button
              id="genBtn"
              data-testid="genBtn"
              title={`Generate a new asset with AI — costs about ${GENERATE_COST} tokens`}
              onClick={() => void runGenerate(panel.query)}
              style={btn("#3a2f5a", "#e8dcff", "#5a4a8f")}
            >
              ✦ Generate with AI · ~{GENERATE_COST} tokens
            </button>
            <button
              id="browseMarket"
              data-testid="browseMarket"
              onClick={() => browseMarketplace(panel.query)}
              style={btn("#1c2030", "#cde", "#2a3550")}
            >
              Browse asset library
            </button>
            <button
              id="buildManual"
              data-testid="buildManual"
              onClick={() => {
                setPanel(null);
                setStatus("build it manually — add an asset or components in the inspector");
              }}
              style={btn("#1c2030", "#cde", "#2a3550")}
            >
              Build manually
            </button>
          </div>
        </div>
      )}

      {panel?.kind === "generating" && (
        <div
          data-testid="describePanel"
          style={{ marginTop: 6, padding: "8px 10px", background: "#221b33", border: "1px solid #5a4a8f", borderRadius: 6, fontSize: 12, color: "#e8dcff" }}
        >
          <span data-testid="genProgress">Generating “{panel.query}” … a placeholder drops in, the mesh streams in.</span>
        </div>
      )}

      {panel?.kind === "refusal" && (
        <div
          data-testid="describePanel"
          style={{ marginTop: 6, padding: "8px 10px", background: "#2a1b1b", border: "1px solid #6a3f2f", borderRadius: 6, fontSize: 12, color: "#fcd", display: "flex", alignItems: "center", justifyContent: "space-between", gap: 8 }}
        >
          <span>{panel.message}</span>
          <button data-testid="describePanelDismiss" onClick={() => setPanel(null)} style={btn("#1c2030", "#cde", "#2a3550")}>
            Dismiss
          </button>
        </div>
      )}
    </div>
  );
}
