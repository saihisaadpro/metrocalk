//! AiEditPanel (M10.10 C3·C4 → M14.3 / ADR-059) — the AI-edit suggestion as a first-class **validated-patch**
//! surface. Off the top-bar wallet, inline near the SELECTED ENTITY (the right pane), in PLAIN language
//! ("Add weathered-metal look", not the "rustier" in-joke). The spend is LEGIBLE + DELIBERATE: the **real
//! token cost** shows up-front, a click opens a confirm with an explicit **before → after** (the entity's
//! current material → the chosen one), and only Apply charges (debit-on-success, the M7 ledger); the result
//! is VISIBLE (the material change lands in the inspector + a toast). A refusal-when-broke is EXPLAINED and
//! leaves the balance untouched (M7 / ADR-016/017 — the patch is a **validated, undoable transaction**).
//! Keeps the `#rustier`/`#rustierApply` ids (prompt-40). Restyled with the M14.1 primitives (one accent —
//! never the purple-SaaS the owner rejected).

import { useState } from "react";
import { useSelectedId, useFieldValue, projectionStore } from "../store/projection";
import { setStatus } from "../store/ui";
import { setBalance } from "../store/wallet";
import { pushToast } from "../store/toasts";
import { Button, Badge } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
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
  const currentMaterial = useFieldValue(selectedId ?? "", "MeshRenderer", "material");
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);

  // Nothing selected → nothing to edit (the AI-edit only makes sense on an entity).
  if (!selectedId) return null;

  const before = typeof currentMaterial === "string" && currentMaterial ? currentMaterial : "default";

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
    <div
      id="aiEdit"
      data-testid="aiEdit"
      style={{ padding: space.lg, fontSize: fontSize.body, borderTop: `1px solid ${color.border.subtle}` }}
    >
      <div style={{ display: "flex", alignItems: "center", gap: space.sm, marginBottom: space.sm }}>
        <span style={{ ...textMeta }}>AI suggestion</span>
        <Badge tone="accent">✦ validated patch</Badge>
      </div>
      {!confirming ? (
        <>
          <Button
            id="rustier"
            data-testid="rustier"
            variant="secondary"
            onClick={() => setConfirming(true)}
            title="Use AI to restyle the selected object — an undoable, validated patch (about 2 tokens)"
            style={{ width: "100%", justifyContent: "flex-start", color: color.accent.base, borderColor: color.accent.border, background: color.accent.subtle }}
          >
            ✦ Add weathered-metal look · ~{AI_EDIT_COST} tokens
          </Button>
          <div style={{ ...textMeta, marginTop: space.xs }}>Changes this object’s material to a weathered metal finish — applied as an undoable patch.</div>
        </>
      ) : (
        <div
          data-testid="rustierConfirm"
          style={{ padding: space.md, background: color.accent.subtle, border: `1px solid ${color.accent.border}`, borderRadius: radius.lg }}
        >
          <div style={{ color: color.text.primary, marginBottom: space.sm }}>
            Apply the weathered-metal look for ~{AI_EDIT_COST} tokens?
          </div>
          {/* The explicit before → after (C3/C7 — show what changes). */}
          <div style={{ display: "flex", alignItems: "center", gap: space.sm, marginBottom: space.md, ...textMeta }}>
            <span>Material</span>
            <Badge tone="neutral">{before}</Badge>
            <span aria-hidden>→</span>
            <Badge tone="accent">weathered metal</Badge>
          </div>
          <div style={{ display: "flex", gap: space.sm, justifyContent: "flex-end" }}>
            <Button data-testid="rustierCancel" variant="secondary" compact onClick={() => setConfirming(false)}>
              Cancel
            </Button>
            <Button id="rustierApply" data-testid="rustierApply" variant="primary" compact disabled={busy} onClick={() => void apply()}>
              {busy ? "Applying…" : `Apply · ~${AI_EDIT_COST} tokens`}
            </Button>
          </div>
        </div>
      )}
      {/* M11.2 material palette — a deliberate, labelled pick (the cost is stated); each applies the same
          metered, validated AI-edit with the chosen PBR preset, with a before/after toast. */}
      <div id="materialPalette" data-testid="materialPalette" style={{ marginTop: space.md }}>
        <div style={{ ...textMeta, marginBottom: space.xs }}>Materials · ~{AI_EDIT_COST} tokens each</div>
        <div style={{ display: "flex", flexWrap: "wrap", gap: space.xs }}>
          {MATERIALS.map((m) => (
            <Button
              key={m.preset}
              data-testid={`material-${m.preset}`}
              variant="secondary"
              compact
              disabled={busy}
              onClick={() => void apply(m.preset, `${m.label} material`)}
              title={`Give this object a ${m.label.toLowerCase()} PBR finish (${before} → ${m.label.toLowerCase()}) — an undoable patch, about ${AI_EDIT_COST} tokens`}
            >
              {m.label}
            </Button>
          ))}
        </div>
      </div>
    </div>
  );
}

const textMeta: React.CSSProperties = { font: font.ui, fontSize: fontSize.meta, color: color.text.muted };
