//! DescribeBar — describe-to-create (north-star #2), the front door. The bar owns its FULL outcome in one
//! place: a local/marketplace **match → place + select** the result; **no match → an explicit inline panel
//! under the field** with actionable controls — ［ ✦ Generate with AI · ~N tokens ］ · ［ Browse the asset
//! library ］ · ［ Build manually ］ — never a passive, button-less footer line. Generate → a progress state
//! → place + select the generated result (the M6 placeholder-first stream-in, ADR-017). The commit is
//! **disabled while the field is empty** (C5 — no enabled-inert CTA), and every outcome surfaces a toast AT
//! THE GESTURE (C11), not only the status gutter.
//!
//! Implement this feature in accordance with the Engine UI/UX Architecture Constitution.
//!
//! WHERE IT WAS, AND WHY THAT WAS A DEFECT RATHER THAN A PLACEMENT. This is the engine's second north star,
//! and reaching it took: open the Engines rail → Build → wait for a lazy chunk → scroll past Shape Studio
//! and Other tools → **open a collapsed disclosure** headed `Describe`, summarised *"Optional assisted
//! creation"*. Three clicks, a scroll, and a label telling you not to bother, for the gesture the product is
//! named after. Nothing measured that: `check-ui-constitution` scored this file **1**, because a control
//! that is beautifully built and unreachable is indistinguishable, to a counter of raw controls, from one
//! that is not there at all.
//!
//! Both full-shell reference sheets put the same act in the same place — a lifted composer at the
//! bottom-centre **of the stage**, always present, over the picture. It lives there now, as a child of the
//! one stage-footer anchor (see `.mtk-stage-footer`), and it draws itself out of the shared `Composer`
//! anatomy rather than nine inline style objects and three hand-built boxes.
//!
//! THE LEADING `+` IS THE OTHER HALF OF THE SAME QUESTION. "Describe it", "import it" and "browse for it"
//! are three answers to *get something into my scene*, and only one of them was on the stage. The reference
//! composer opens a small menu from a leading `+` for exactly this; the three rows here are the product's
//! real doors, not invented ones.
//!
//! WHAT IS DELIBERATELY NOT COPIED FROM THE REFERENCE. Its composer carries a model picker and a microphone.
//! This engine has neither — AI is a guest, and there is no model to choose — so the trailing slot carries
//! the thing that IS true and that a user needs before committing: which tier will answer, and what it will
//! cost. Substrate truth in the reference's shape.
//!
//! Keeps the stable `#describe`/`#describeBtn`/`#genBtn` hooks the acceptance gates drive.

import { useEffect, useState } from "react";
import { projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import { GENERATE_COST } from "../transport/protocol";
import { Icon } from "../theme/icons";
import { Badge, Button } from "../theme/primitives";
import { Callout } from "../theme/callout";
import { MenuPopup, PopupMenuItem } from "../theme/workspace";
import {
  Composer,
  ComposerField,
  ComposerHint,
  ComposerOutcome,
  ComposerRow,
  ComposerSubmit,
} from "../theme/composer";
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

export interface DescribeBarProps {
  client: EditorClient;
  /** The stage form (a lifted card over the picture) or the panel form (framed, no elevation). */
  form?: "floating" | "inline";
  /** The `+` menu's three doors. Each is optional; a door with no handler is not offered rather than
   *  offered-and-inert, which is the one thing `<ux_quality>` 6 rules out. */
  onImport?: () => void;
  onBrowseAssets?: () => void;
  onDrawShape?: () => void;
}

export function DescribeBar({ client, form = "inline", onImport, onBrowseAssets, onDrawShape }: DescribeBarProps) {
  const [query, setQuery] = useState("");
  const [panel, setPanel] = useState<Panel>(null);
  const [preview, setPreview] = useState<Preview>(null);
  const empty = query.trim().length === 0;
  const busy = panel?.kind === "generating";

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
    if (!q) return; // empty guard (the commit is also disabled) — never a silent inert CTA (C5)
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

  /** Browse the asset library (the *browse* door to creation). The library lives in a workspace that may
   *  not be open — the composer is on the stage now — so this OPENS it first and then aims the caret at
   *  its search. Focusing an id that is not mounted was a no-op that looked like a dead control. */
  function browseLibrary(q: string) {
    setPanel(null);
    // OPEN IT, then aim at its search. The old version only aimed: it called `focus()` on `#assetSearch`
    // from a composer that could be anywhere, and `focus()` on an id that is not mounted is a no-op that
    // looks exactly like a working control. The library lives in a LAZILY-loaded workspace, so the focus
    // lands only when it happens to be open already — which is why the toast, not the caret, is the
    // durable pointer here.
    onBrowseAssets?.();
    (document.getElementById("assetSearch") as HTMLInputElement | null)?.focus();
    pushToast(`Browse the asset library for "${q}"`, "info");
    setStatus(`browsing the asset library for "${q}"`);
  }

  const submitReason = empty ? "Describe something first" : busy ? "A generation is already running" : undefined;

  return (
    <Composer
      id="describeComposer"
      data-testid="describebar"
      form={form}
      aria-label="Describe something to create"
    >
      <ComposerRow>
        {/* No doors wired → no trigger. A `+` that opens an empty menu is the inert surface
            `<ux_quality>` 6 rules out, and it is what a panel embedding this composer without the
            shell's handlers would otherwise get. */}
        {(onImport || onBrowseAssets || onDrawShape) && (
        <MenuPopup
          id="describeAdd"
          label="Add something to the scene"
          placement="top-start"
          trigger={<Icon name="plus" size="md" />}
          triggerProps={{
            id: "describeAddBtn",
            "data-testid": "describeAddBtn",
            variant: "ghost",
            icon: true,
            className: "mtk-composer__lead",
            "aria-label": "Add something to the scene",
          }}
        >
          {(close) => (
            <>
              {onImport && (
                <PopupMenuItem
                  data-testid="describeAddImport"
                  leading={<Icon name="import" size="md" />}
                  label="Import a file…"
                  description="A model, drawing or assembly from disk"
                  onSelect={onImport}
                  onRequestClose={close}
                />
              )}
              {onBrowseAssets && (
                <PopupMenuItem
                  data-testid="describeAddBrowse"
                  leading={<Icon name="assets" size="md" />}
                  label="Browse the asset library"
                  description="Everything already available to this project"
                  onSelect={onBrowseAssets}
                  onRequestClose={close}
                />
              )}
              {onDrawShape && (
                <PopupMenuItem
                  data-testid="describeAddDraw"
                  leading={<Icon name="pipe" size="md" />}
                  label="Draw it in the viewport"
                  description="Procedural geometry you route by hand"
                  onSelect={onDrawShape}
                  onRequestClose={close}
                />
              )}
            </>
          )}
        </MenuPopup>
        )}
        <ComposerField
          id="describe"
          data-testid="describe"
          value={query}
          placeholder="Describe something to create…"
          aria-label="Describe something to create"
          disabled={busy}
          onChange={(e) => {
            setQuery(e.target.value);
            // typing invalidates a stale offer/refusal so feedback never goes stale (C11)
            setPanel((p) => (p && p.kind !== "generating" ? null : p));
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
          }}
        />
        <ComposerSubmit
          id="describeBtn"
          data-testid="describeBtn"
          label="Create it"
          disabled={empty || busy}
          disabledReason={submitReason}
          onClick={() => void submit()}
        />
      </ComposerRow>

      {/* Accepted-tier registry-aware preview: WHAT will be created + its real cost, before commit. Suppressed
          while an offer/generating/refusal panel is showing (that panel is the more specific surface). */}
      {!empty && !panel && preview && (
        <ComposerHint data-testid="describePreview">
          {preview.kind === "match" ? (
            <>
              <Icon name="check" size="sm" title="Found in the catalogue" />
              <span>
                Will place <strong>{preview.label}</strong> · {preview.source}
              </span>
              <Badge
                data-testid="previewCost"
                tone={preview.price ? "warn" : "success"}
                title={preview.price ? "Buying this from the marketplace spends tokens" : "Already available to this project"}
              >
                {preview.price ? `−${preview.price} tokens` : "free"}
              </Badge>
            </>
          ) : (
            <>
              <Icon name="sparkle" size="sm" title="Nothing matches — this will be generated" />
              <span>No match — will generate</span>
              <Badge data-testid="previewCost" tone="warn" title="Generating spends tokens">
                ~{GENERATE_COST} tokens
              </Badge>
            </>
          )}
        </ComposerHint>
      )}

      {panel != null && (
        <ComposerOutcome>
          {panel.kind === "offer" && (
            <Callout tone="info" data-testid="describePanel">
              No match for “{panel.query}”. Generate it with AI, browse the asset library, or build it yourself.
              <div className="mtk-composer__actions">
                <Button
                  id="genBtn"
                  data-testid="genBtn"
                  variant="primary"
                  compact
                  title={`Generate a new asset with AI — costs about ${GENERATE_COST} tokens`}
                  onClick={() => void runGenerate(panel.query)}
                >
                  <Icon name="sparkle" size="sm" /> Generate with AI · ~{GENERATE_COST} tokens
                </Button>
                <Button id="browseMarket" data-testid="browseMarket" variant="secondary" compact onClick={() => browseLibrary(panel.query)}>
                  Browse asset library
                </Button>
                <Button
                  id="buildManual"
                  data-testid="buildManual"
                  variant="secondary"
                  compact
                  onClick={() => {
                    setPanel(null);
                    setStatus("build it manually — add an asset or components in the inspector");
                  }}
                >
                  Build manually
                </Button>
              </div>
            </Callout>
          )}

          {panel.kind === "generating" && (
            <Callout tone="info" data-testid="describePanel" icon={<span className="mtk-spinner" aria-hidden />}>
              <span data-testid="genProgress">Generating “{panel.query}” … a placeholder drops in, the mesh streams in.</span>
            </Callout>
          )}

          {panel.kind === "refusal" && (
            <Callout tone="danger" data-testid="describePanel">
              {panel.message}
              <div className="mtk-composer__actions">
                <Button data-testid="describePanelDismiss" variant="secondary" compact onClick={() => setPanel(null)}>
                  Dismiss
                </Button>
              </div>
            </Callout>
          )}
        </ComposerOutcome>
      )}
    </Composer>
  );
}
