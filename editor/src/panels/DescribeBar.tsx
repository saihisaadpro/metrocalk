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

import { useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import { GENERATE_COST } from "../transport/protocol";
import { Button } from "../theme/primitives";
import { color, elevation, fontSize, radius, space } from "../theme/tokens";
import type { EditorClient } from "../transport/session";

/** The accepted-tier "registry-aware" preview (M14.1) — what the typed query WILL create, read live from the
 *  real catalog (registry + marketplace + imported) + the ledger cost, BEFORE the user commits. Substrate
 *  truth, not decoration: a match shows the real item + its real price; a no-match shows the generate cost. */
type Preview =
  | null
  | { kind: "match"; label: string; source: string; price: number | null }
  | { kind: "generate" };

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
  const [preview, setPreview] = useState<Preview>(null);
  const empty = query.trim().length === 0;

  // Registry-aware preview (accepted-tier): debounced, non-mutating `catalog_search` over the typed query →
  // show WHAT will be created + its real cost BEFORE commit. Local state only (never `setStatus` — the
  // status line stays the action's, and the "empty query → no status churn" contract holds); a slow,
  // off-hot-path read (discrete, debounced — invariant 4); best-effort (a failed read shows nothing).
  useEffect(() => {
    const q = query.trim();
    if (!q || panel?.kind === "generating") {
      setPreview(null);
      return;
    }
    let live = true;
    const t = setTimeout(() => {
      void client
        .catalogSearch(q)
        .then((r) => {
          if (!live) return;
          const top = r.items[0];
          if (top) setPreview({ kind: "match", label: top.label, source: top.source, price: top.price ?? null });
          else setPreview({ kind: "generate" });
        })
        .catch(() => live && setPreview(null));
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [query, client, panel?.kind]);

  async function submit() {
    const q = query.trim();
    if (!q) return; // empty guard (Create is also disabled) — never a silent inert CTA (C5)
    setPanel(null);
    setPreview(null);
    try {
      const r = await client.describe(q);
      if (r.balance != null) setBalance(r.balance);

      // MATCH → place + select the result, right where the action started (the loop closes here). The
      // status carries the stable TIER tag the prompt-40 E2E keys on (`local:` / `marketplace:` · `bought` ·
      // `tokens`); the toast is the friendly UX (M10.10) — both, not footer-only.
      if (r.created) {
        projectionStore.getState().select(r.created);
        setQuery("");
        const kind = r.kind ?? "entity";
        if (r.source === "marketplace") {
          const cost = r.price != null ? ` · −${r.price} tokens` : "";
          const left = r.balance != null ? ` · ${r.balance} left` : "";
          setStatus(`marketplace: bought ${kind} · ${r.created}${cost}${left}`);
          pushToast(`Bought ${r.kind ?? "object"} · marketplace${cost}`, "success");
        } else {
          setStatus(`local: created ${kind} · ${r.created} (free)`);
          pushToast(`Created ${r.kind ?? "object"} · local`, "success");
        }
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
    } catch (e) {
      console.error("describe failed", e);
      pushToast("create failed — please try again", "error");
      setStatus("create failed");
    }
  }

  /** The opt-in, METERED tier-3 generate (M6/ADR-017): a deliberate, priced click → a progress state →
   *  place + select the result (the real mesh streams in over the projection Channel on the `.exe`). */
  async function runGenerate(q: string) {
    setPanel({ kind: "generating", query: q });
    setStatus(`generating "${q}" … (~${GENERATE_COST} tokens)`);
    pushToast(`Generating "${q}" … ~${GENERATE_COST} tokens`, "cost");
    let r: Awaited<ReturnType<typeof client.generate>>;
    try {
      r = await client.generate(q);
    } catch (e) {
      // A failed generation must clear the progress state + explain — never strand "Generating…".
      console.error("generate failed", e);
      setPanel({ kind: "refusal", message: "generation failed — please try again" });
      pushToast("generation failed", "error");
      setStatus("generation failed");
      return;
    }
    if (r.balance != null) setBalance(r.balance);
    if (r.created) {
      projectionStore.getState().select(r.created);
      void client.gizmoSelect(r.created).catch((e) => console.error("gizmoSelect failed (engine selection may be out of sync)", e)); // set the ENGINE selection too (gizmo/inspector track it)
      // The wallet shows the charge AT THE GESTURE: `generate` reserves a hold up front and returns the
      // AVAILABLE balance (settled − the hold), so `setBalance(r.balance)` above already reflects the −cost.
      // No client poll — the legible-cost contract is met by the response, not a post-hoc read.
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

  return (
    <div data-testid="describebar" style={{ padding: `${space.sm}px ${space.lg}px` }}>
      <div style={{ display: "flex", gap: space.sm, alignItems: "center" }}>
        <span aria-hidden style={{ color: color.accent.base, fontSize: fontSize.title, lineHeight: 1 }}>✦</span>
        <input
          id="describe"
          data-testid="describe"
          className="mtk-input"
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
          style={{ flex: 1 }}
        />
        <Button
          id="describeBtn"
          data-testid="describeBtn"
          variant="primary"
          disabled={empty}
          title={empty ? "Describe something first" : undefined}
          onClick={() => void submit()}
        >
          Create
        </Button>
      </div>

      {/* Accepted-tier registry-aware preview: WHAT will be created + its real cost, before commit. Suppressed
          while an offer/generating/refusal panel is showing (that panel is the more specific surface). */}
      {!empty && !panel && preview && (
        <div
          data-testid="describePreview"
          style={{ marginTop: space.xs, fontSize: fontSize.meta, color: color.text.muted, display: "flex", alignItems: "center", gap: space.sm }}
        >
          {preview.kind === "match" ? (
            <>
              <span style={{ color: color.success.text }}>✓ will place</span>
              <span style={{ color: color.text.secondary }}>{preview.label}</span>
              <span style={{ color: color.text.faint }}>· {preview.source}</span>
              <span data-testid="previewCost" style={{ color: preview.price ? color.token : color.success.text }}>
                · {preview.price ? `−${preview.price} tokens` : "free"}
              </span>
            </>
          ) : (
            <>
              <span style={{ color: color.accent.base }}>✦ no match — will generate</span>
              <span data-testid="previewCost" style={{ color: color.token }}>· ~{GENERATE_COST} tokens</span>
            </>
          )}
        </div>
      )}

      {panel?.kind === "offer" && (
        <div
          data-testid="describePanel"
          style={{ marginTop: space.sm, padding: `${space.md}px ${space.lg}px`, background: color.bg.raised, border: `1px solid ${color.border.default}`, borderRadius: radius.lg, fontSize: fontSize.body, boxShadow: elevation.e1 }}
        >
          <div style={{ marginBottom: space.sm, color: color.text.secondary }}>
            No match for “{panel.query}”. Generate it with AI, browse the asset library, or build it yourself.
          </div>
          <div style={{ display: "flex", gap: space.md, flexWrap: "wrap" }}>
            <Button
              id="genBtn"
              data-testid="genBtn"
              variant="primary"
              title={`Generate a new asset with AI — costs about ${GENERATE_COST} tokens`}
              onClick={() => void runGenerate(panel.query)}
            >
              ✦ Generate with AI · ~{GENERATE_COST} tokens
            </Button>
            <Button id="browseMarket" data-testid="browseMarket" variant="secondary" onClick={() => browseMarketplace(panel.query)}>
              Browse asset library
            </Button>
            <Button
              id="buildManual"
              data-testid="buildManual"
              variant="secondary"
              onClick={() => {
                setPanel(null);
                setStatus("build it manually — add an asset or components in the inspector");
              }}
            >
              Build manually
            </Button>
          </div>
        </div>
      )}

      {panel?.kind === "generating" && (
        <div
          data-testid="describePanel"
          style={{ marginTop: space.sm, padding: `${space.md}px ${space.lg}px`, background: color.accent.subtle, border: `1px solid ${color.accent.border}`, borderRadius: radius.lg, fontSize: fontSize.body, color: color.text.primary, display: "flex", alignItems: "center", gap: space.md }}
        >
          <span className="mtk-spinner" aria-hidden />
          <span data-testid="genProgress">Generating “{panel.query}” … a placeholder drops in, the mesh streams in.</span>
        </div>
      )}

      {panel?.kind === "refusal" && (
        <div
          data-testid="describePanel"
          style={{ marginTop: space.sm, padding: `${space.md}px ${space.lg}px`, background: color.danger.bg, border: `1px solid ${color.danger.border}`, borderRadius: radius.lg, fontSize: fontSize.body, color: color.danger.text, display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.md }}
        >
          <span>{panel.message}</span>
          <Button data-testid="describePanelDismiss" variant="secondary" compact onClick={() => setPanel(null)}>
            Dismiss
          </Button>
        </div>
      )}
    </div>
  );
}
