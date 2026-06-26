//! AiEditPanel (M10.10 / C3·C4) — the AI-edit suggestion, RELOCATED off the top-bar wallet (where it
//! floated over + clipped the balance) to an inline panel near the SELECTED ENTITY (the right pane), in
//! PLAIN language ("Add weathered-metal look", not the "rustier" in-joke). The spend is LEGIBLE +
//! DELIBERATE: the price shows up-front, a click opens a one-line before/after CONFIRM, and only Apply
//! charges (debit-on-success); the result is VISIBLE (the material change lands in the inspector + a
//! toast). A refusal-when-broke is EXPLAINED and leaves the balance untouched (M7 / ADR-016). Keeps the
//! `#rustier` id (prompt-40) on the trigger; `#rustierApply` on the confirm.

import { useState } from "react";
import { useSelectedId, projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";

const AI_EDIT_COST = 2;

// The M11.2 (ADR-041) PBR material presets — a small palette of named looks, each assigned through the same
// metered, schema-validated AI-edit (apply_ai_patch → MeshRenderer.material → a per-entity render override).
const MATERIALS: { preset: string; label: string }[] = [
  { preset: "metal", label: "Metal" },
  { preset: "chrome", label: "Chrome" },
  { preset: "gold", label: "Gold" },
  { preset: "copper", label: "Copper" },
  { preset: "rusty", label: "Rust" },
  { preset: "plastic", label: "Plastic" },
];

export function AiEditPanel({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  // Nothing selected → nothing to edit (the AI-edit only makes sense on an entity).
  if (!selectedId) return null;

  async function apply(material = "rusty", label = "Weathered-metal look") {
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
    <div id="aiEdit" data-testid="aiEdit" style={{ padding: 12, fontSize: 13, borderTop: "1px solid #2a2d35" }}>
      <div style={{ opacity: 0.6, fontSize: 11, marginBottom: 6 }}>AI suggestion</div>
      {!confirming ? (
        <>
          <button
            id="rustier"
            data-testid="rustier"
            onClick={() => setConfirming(true)}
            title="Use AI to restyle the selected object — costs about 2 tokens"
            style={{ display: "block", width: "100%", textAlign: "left", padding: "6px 8px", background: "#2b2233", color: "#e8dcff", border: "1px solid #4a3a5f", borderRadius: 4, cursor: "pointer" }}
          >
            ✦ Add weathered-metal look · ~{AI_EDIT_COST} tokens
          </button>
          <div style={{ opacity: 0.6, fontSize: 11, marginTop: 4 }}>Changes this object’s material to a weathered metal finish.</div>
        </>
      ) : (
        <div data-testid="rustierConfirm" style={{ padding: "8px 10px", background: "#221b33", border: "1px solid #5a4a8f", borderRadius: 6 }}>
          <div style={{ marginBottom: 8, color: "#e8dcff" }}>
            Apply the weathered-metal look for ~{AI_EDIT_COST} tokens? Material → weathered metal.
          </div>
          <div style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
            <button
              data-testid="rustierCancel"
              onClick={() => setConfirming(false)}
              style={{ padding: "4px 12px", background: "#1b1e26", color: "#e8e8e8", border: "1px solid #2a2d35", borderRadius: 4, cursor: "pointer" }}
            >
              Cancel
            </button>
            <button
              id="rustierApply"
              data-testid="rustierApply"
              disabled={busy}
              onClick={() => void apply()}
              style={{ padding: "4px 12px", background: "#3a2f5a", color: "#e8dcff", border: "1px solid #5a4a8f", borderRadius: 4, cursor: busy ? "default" : "pointer" }}
            >
              {busy ? "Applying…" : `Apply · ~${AI_EDIT_COST} tokens`}
            </button>
          </div>
        </div>
      )}
      {/* M11.2 material palette — a deliberate, labelled pick (the cost is stated); each applies the same
          metered AI-edit with the chosen PBR preset. */}
      <div id="materialPalette" data-testid="materialPalette" style={{ marginTop: 10 }}>
        <div style={{ opacity: 0.6, fontSize: 11, marginBottom: 4 }}>Materials · ~{AI_EDIT_COST} tokens each</div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
          {MATERIALS.map((m) => (
            <button
              key={m.preset}
              data-testid={`material-${m.preset}`}
              disabled={busy}
              onClick={() => void apply(m.preset, `${m.label} material`)}
              title={`Give this object a ${m.label.toLowerCase()} PBR finish — about ${AI_EDIT_COST} tokens`}
              style={{ padding: "3px 9px", background: "#1b2233", color: "#cfe", border: "1px solid #2a3550", borderRadius: 4, cursor: busy ? "default" : "pointer", fontSize: 12 }}
            >
              {m.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
