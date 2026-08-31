//! AiEditPanel (M10.10 C3·C4 → M14.3 / ADR-059 → ADR-164) — the AI-edit suggestion as a first-class
//! **validated-patch** surface. Off the top-bar wallet, inline near the SELECTED ENTITY (the right pane),
//! in PLAIN language ("Add weathered-metal look", not the "rustier" in-joke). The spend is LEGIBLE +
//! DELIBERATE: the **real token cost** shows up-front, a click opens a confirm with an explicit
//! **before → after** (the entity's current material → the chosen one), and only Apply charges
//! (debit-on-success, the M7 ledger); the result is VISIBLE (the material change lands in the inspector +
//! a toast). A refusal-when-broke is EXPLAINED and leaves the balance untouched (M7 / ADR-016/017 — the
//! patch is a **validated, undoable transaction**). Keeps the `#rustier`/`#rustierApply` ids (prompt-40).
//!
//! **ADR-164 — WHAT THIS PANEL IS NO LONGER.** It used to carry a six-button material palette, and every
//! button on it spent two tokens to write a string that `client.setField` writes for nothing. That
//! palette is now `MaterialPanel`, free and deterministic, and this is what was always left over: the
//! ONE action here that genuinely needs a model, priced, deliberate, and optional. The invariant it
//! restores is the project's own — the deterministic core works with no LLM, and the AI is a guest.
//!
//! Its own layout is gone with it. Twenty inline style objects and a private `textMeta` have become the
//! shared `Callout`, `Badge` and `Button`, so the one assisted row in the Inspector is built out of the
//! same parts as every unassisted one.

import { useState } from "react";
import { useSelectedId, useDisplayedEntity, projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import { Icon } from "../theme/icons";
import { Button, Badge } from "../theme/primitives";
import { Callout } from "../theme/fields";
import { materialPresetFor } from "../theme/materials";
import type { EditorClient } from "../transport/session";

const AI_EDIT_COST = 2;
/** The preset the suggestion applies. Named here so the confirm's "after" and the request agree. */
const SUGGESTED = "rusty";

export function AiEditPanel({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const entity = useDisplayedEntity(selectedId ?? "");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  // Nothing selected → nothing to edit (the AI-edit only makes sense on an entity).
  if (!selectedId) return null;

  // THE SAME REFUSAL THE FREE PICKER STATES, AND FOR THE SAME REASON. `ai_edit` applies its patch
  // through `apply_ai_patch`, which needs `MeshRenderer` to exist — so on an object without one this
  // button offers to spend two tokens on an edit that cannot land. It was live in every such case
  // before ADR-164, and the `material-no-mesh` capture is what showed it: a greyed grid of free
  // swatches with an enabled priced button underneath. `<ux_quality>` 6 — an enabled control does
  // something or says why it can't.
  const currentMaterial = entity?.components.MeshRenderer?.material;
  const shadeable = entity?.components.MeshRenderer !== undefined;
  const refusal = shadeable ? undefined : "This object has no mesh to shade — add a MeshRenderer first.";
  // The BEFORE, in the same words the picker above uses for the same value — "Rust", not "rusty" — so
  // the confirm and the swatch cannot describe one material two ways.
  const preset = materialPresetFor(currentMaterial);
  const before = preset?.label ?? (typeof currentMaterial === "string" && currentMaterial ? currentMaterial : "default");

  async function apply(material = SUGGESTED, label = "Weathered-metal look") {
    if (!selectedId || busy) return;
    const target = selectedId; // capture: the selection may change during the await (don't mis-attribute)
    setBusy(true);
    try {
      const r = await client.aiEdit(target, material);
      if (r.ok) {
        // Debit-on-success: the new balance is authoritative; surface the charge AND the result. Only claim
        // the visible per-entity result when the selection hasn't moved (the balance update is global).
        setBalance(r.balance);
        const cost = r.cost ?? AI_EDIT_COST;
        const onTarget = projectionStore.getState().selectedId === target;
        pushToast(`${label} applied · −${cost} tokens · ${r.balance} left`, "success");
        setStatus(onTarget ? `${label.toLowerCase()} · −${cost} tokens` : `applied · −${cost} tokens`);
      } else {
        // Refuse-when-broke, EXPLAINED: surface the reason, leave the balance untouched (no charge).
        const msg = r.message ?? "refused";
        pushToast(msg, "error");
        setStatus(msg);
      }
    } catch (e) {
      // A failed AI-edit must not strand the panel or leak an unhandled rejection (the clean-console gate).
      console.error("ai_edit failed", e);
      pushToast("AI-edit failed — please try again", "error");
    } finally {
      setBusy(false);
      setConfirming(false);
    }
  }

  return (
    <div id="aiEdit" data-testid="aiEdit" className="mtk-ai-edit">
      <div className="mtk-ai-edit__head">
        <span className="mtk-ai-edit__label">AI suggestion</span>
        <Badge tone="accent"><Icon name="sparkle" size="sm" /> validated patch</Badge>
      </div>
      {!confirming ? (
        <>
          <Button
            id="rustier"
            data-testid="rustier"
            variant="secondary"
            className="mtk-ai-edit__trigger"
            disabled={!shadeable}
            disabledReason={refusal}
            onClick={() => setConfirming(true)}
            // CONDITIONAL on the same predicate as `disabled`. `Button` resolves
            // `title ?? (refusing && disabledReason)`, so an unconditional title permanently blocks
            // the refusal from ever being spoken.
            title={shadeable ? "Use AI to restyle the selected object — an undoable, validated patch (about 2 tokens)" : undefined}
          >
            <Icon name="sparkle" size="sm" /> Add weathered-metal look · ~{AI_EDIT_COST} tokens
          </Button>
          {/* Only when it can act. The refusal is already on screen once — the picker states it above
              this row — and the button carries it as its own accessible description, so repeating the
              sentence a third time in a 234px column is noise rather than help. */}
          {shadeable && (
            <p className="mtk-ai-edit__note">
              Changes this object’s material to a weathered metal finish — applied as an undoable patch.
            </p>
          )}
        </>
      ) : (
        <Callout tone="info" data-testid="rustierConfirm" icon={<Icon name="sparkle" size="sm" />}>
          <div className="mtk-ai-edit__question">Apply the weathered-metal look for ~{AI_EDIT_COST} tokens?</div>
          {/* The explicit before → after (C3/C7 — show what changes). */}
          <div className="mtk-ai-edit__diff">
            <span className="mtk-ai-edit__label">Material</span>
            <Badge tone="neutral">{before}</Badge>
            <Icon name="arrow-right" size="sm" />
            <Badge tone="accent">weathered metal</Badge>
          </div>
          <div className="mtk-ai-edit__actions">
            <Button data-testid="rustierCancel" variant="secondary" compact onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button id="rustierApply" data-testid="rustierApply" variant="primary" compact disabled={busy} onClick={() => void apply()}>
              {busy ? "Applying…" : `Apply · ~${AI_EDIT_COST} tokens`}
            </Button>
          </div>
        </Callout>
      )}
    </div>
  );
}
