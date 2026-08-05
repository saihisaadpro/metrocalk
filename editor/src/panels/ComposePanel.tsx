/**
 * Review-first composition builder.
 *
 * A plain-language request produces a proposal that the user can inspect before anything reaches the scene.
 * Apply sends the exact reviewed composition through the normal validated, undoable commit path.
 */

import { useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import { Badge, Button } from "../theme/primitives";
import { color, font, fontSize, radius, space } from "../theme/tokens";
import { ShortcutBadge } from "../theme/workspace";
import type { ComposeOp, ComposeProposal, Composition } from "../transport/protocol";
import type { EditorClient } from "../transport/session";

/** A concise, reviewable summary of an operation. Raw patch JSON stays out of the primary workflow. */
function describeOp(op: ComposeOp): string {
  switch (op.op) {
    case "setField":
      return `Set ${op.component}.${op.field} on ${op.entity}`;
    case "authorRule":
      return `author rule "${op.rule.name}" (when ${op.rule.event})`;
    case "authorStateMachine":
      return `Author state machine "${op.machine.name}"`;
    default:
      return "Unknown operation";
  }
}

const helperText: React.CSSProperties = {
  margin: 0,
  color: color.text.muted,
  font: font.ui,
  fontSize: fontSize.meta,
  lineHeight: 1.45,
};

export function ComposePanel({ client }: { client: EditorClient }) {
  const selectedId = useStore(projectionStore, (s) => s.selectedId);
  const summaries = useStore(projectionStore, (s) => s.summaries);
  const selectedName = selectedId ? (summaries[selectedId]?.name ?? selectedId) : null;

  const [sentence, setSentence] = useState("");
  const [proposal, setProposal] = useState<ComposeProposal | null>(null);
  const [busy, setBusy] = useState(false);

  async function propose() {
    setBusy(true);
    setProposal(null);
    try {
      const p = await client.proposeComposition(sentence, selectedId);
      setProposal(p);
    } catch {
      setProposal({ ok: false, composition: null, ops: 0, error: "could not reach the composer — please try again" });
    } finally {
      setBusy(false);
    }
  }

  async function apply(composition: Composition) {
    setBusy(true);
    try {
      const r = await client.compose(composition);
      if (r.error || !r.ok) {
        setProposal({ ok: false, composition: null, ops: 0, error: r.error ?? "the composition was rejected" });
        return;
      }
      pushToast(`Applied ${r.applied} patch${r.applied === 1 ? "" : "es"} · ${r.rules} rule${r.rules === 1 ? "" : "s"} · Ctrl-Z to undo`, "success");
      setProposal(null);
      setSentence("");
    } catch {
      setProposal({ ok: false, composition: null, ops: 0, error: "could not apply the composition — please try again" });
    } finally {
      setBusy(false);
    }
  }

  return (
    <section
      id="compose"
      data-testid="compose-panel"
      className="mtk-scroll"
      aria-label="Scene composition controls"
      aria-busy={busy || undefined}
      style={{ minHeight: 0, overflowY: "auto", padding: space.md, background: color.bg.panel }}
    >
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: space.sm, marginBottom: space.md }}>
        <span style={helperText}>Preview-first scene changes</span>
        <Badge tone={selectedName ? "accent" : "neutral"}>{selectedName ? `Target: ${selectedName}` : "Scene-wide"}</Badge>
      </div>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (!busy && sentence.trim()) void propose();
        }}
        style={{ display: "flex", flexDirection: "column", gap: space.sm }}
      >
        <div>
          <label
            htmlFor="composeSentence"
            style={{ display: "block", marginBottom: space.xs, color: color.text.secondary, font: font.ui, fontSize: fontSize.meta, fontWeight: 600 }}
          >
            What should happen?
          </label>
          <textarea
            id="composeSentence"
            data-testid="compose-sentence"
            className="mtk-input"
            placeholder='Example: "When an enemy dies and kills reach 4, set it on fire"'
            aria-describedby="compose-help compose-target"
            style={{ boxSizing: "border-box", width: "100%", minHeight: 76, resize: "vertical", lineHeight: 1.45 }}
            value={sentence}
            disabled={busy}
            onChange={(e) => setSentence(e.target.value)}
          />
          <div id="compose-help" style={{ ...helperText, marginTop: space.xs }}>
            Be specific about the trigger, condition, and result. Nothing changes until you approve the preview.
          </div>
          <div id="compose-target" style={{ ...helperText, marginTop: space.xs, color: selectedName ? color.accent.base : color.text.muted }}>
            {selectedName
              ? `The current selection, “${selectedName}”, will be supplied as context.`
              : "No entity is selected. Requests that need a target will explain what to select."}
          </div>
        </div>

        <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: space.sm }}>
          <Button
            data-testid="compose-propose"
            type="submit"
            variant="primary"
            disabled={busy || sentence.trim().length === 0}
            title={sentence.trim().length === 0 ? "Enter a scene request before preparing a proposal." : "Prepare a reviewable list of scene changes."}
          >
            {busy ? (
              <>
                <span className="mtk-spinner" aria-hidden="true" /> Preparing preview…
              </>
            ) : (
              "Preview changes"
            )}
          </Button>
          {!busy && !proposal && <span style={helperText}>A preview never edits the scene.</span>}
        </div>
      </form>

      {busy && (
        <div role="status" aria-live="polite" style={{ ...helperText, marginTop: space.md }}>
          Preparing and validating the requested changes…
        </div>
      )}

      {proposal && !proposal.ok && (
        <div
          data-testid="compose-error"
          role="alert"
          style={{
            marginTop: space.md,
            padding: space.md,
            border: `1px solid ${color.danger.border}`,
            borderRadius: radius.lg,
            background: color.danger.bg,
            color: color.danger.text,
            font: font.ui,
            fontSize: fontSize.body,
            lineHeight: 1.45,
          }}
        >
          <div style={{ marginBottom: space.xs, fontWeight: 600 }}>A preview could not be created</div>
          <div>{proposal.error}</div>
          <Button type="button" variant="ghost" compact style={{ marginTop: space.sm }} onClick={() => setProposal(null)}>
            Dismiss
          </Button>
        </div>
      )}

      {proposal && proposal.ok && proposal.composition && (
        <section
          data-testid="compose-proposal"
          aria-labelledby="compose-proposal-title"
          style={{
            marginTop: space.lg,
            overflow: "hidden",
            border: `1px solid ${color.success.border}`,
            borderRadius: radius.lg,
            background: color.success.bg,
          }}
        >
          <div style={{ padding: space.md, borderBottom: `1px solid ${color.success.border}` }}>
            <div id="compose-proposal-title" style={{ color: color.text.primary, font: font.ui, fontSize: fontSize.body, fontWeight: 600 }}>
              Review proposed changes
            </div>
            <div style={{ ...helperText, marginTop: space.xs, color: color.success.text }}>
              <b data-testid="compose-opcount">{proposal.ops}</b> patch{proposal.ops === 1 ? "" : "es"} ready. The scene is still unchanged.
            </div>
          </div>

          <ol style={{ display: "flex", flexDirection: "column", gap: space.xs, margin: 0, padding: space.md, listStylePosition: "inside" }}>
            {proposal.composition.ops.map((op, i) => (
              <li
                key={i}
                data-testid="compose-op"
                style={{
                  padding: space.sm,
                  border: `1px solid ${color.border.subtle}`,
                  borderRadius: radius.md,
                  background: color.bg.raised,
                  color: color.text.secondary,
                  font: font.ui,
                  fontSize: fontSize.body,
                  lineHeight: 1.4,
                }}
              >
                {describeOp(op)}
              </li>
            ))}
          </ol>

          <div style={{ display: "flex", flexWrap: "wrap", gap: space.sm, padding: space.md, borderTop: `1px solid ${color.success.border}` }}>
            <Button
              data-testid="compose-apply"
              type="button"
              variant="primary"
              disabled={busy}
              onClick={() => proposal.composition && void apply(proposal.composition)}
            >
              Apply reviewed changes
            </Button>
            <Button data-testid="compose-discard" type="button" disabled={busy} onClick={() => setProposal(null)}>
              Discard preview
            </Button>
          </div>
        </section>
      )}
      <div style={{ ...helperText, marginTop: space.lg, paddingTop: space.md, borderTop: `1px solid ${color.border.subtle}` }}>
        Changes use the standard commit path and can be undone with <ShortcutBadge keys={["Ctrl", "Z"]} ariaLabel="Control plus Z" />
      </div>
    </section>
  );
}
