//! M12.4 (ADR-048) — the **AI Compose** panel: a natural-language sentence → a **reviewable** Composition
//! proposal → applied through the SAME validated commit pipeline a human/plugin uses. The AI is a **guest**:
//! it only *proposes*; the engine validates + commits (one undoable transaction) or refuses with a plain-
//! language reason (ADR-016). Close the loop: the user reads the proposed patches BEFORE applying, every "no"
//! is explained, and the result is one Ctrl-Z away. The shipped headless live path is the `metrocalk-mcp`
//! server (an external MCP client like Claude); this is the in-editor seam beside it.

import { useState } from "react";
import { useStore } from "zustand";
import { projectionStore } from "../store/projection";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";
import type { ComposeOp, ComposeProposal, Composition } from "../transport/protocol";

const box: React.CSSProperties = { font: "12px ui-monospace, monospace", padding: 10 };
const ctrl: React.CSSProperties = { font: "11px ui-monospace, monospace", padding: "1px 3px" };

/** A one-line, plain-language summary of a proposed op (so the user reviews WHAT will change, not raw JSON). */
function describeOp(op: ComposeOp): string {
  switch (op.op) {
    case "setField":
      return `set ${op.component}.${op.field} on ${op.entity}`;
    case "authorRule":
      return `author rule "${op.rule.name}" (When ${op.rule.event})`;
    case "authorStateMachine":
      return `author state machine "${op.machine.name}"`;
    default:
      return "unknown op";
  }
}

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
        // The proposal was pre-validated, but the scene can change between review + apply — explain, don't crash.
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
    <div id="compose" data-testid="compose-panel" style={{ ...box, borderTop: "1px solid #2a2d35" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: 6 }}>
        <b>AI Compose</b>
        <span style={{ color: "#888", fontSize: 10 }}>{selectedName ? `acts on: ${selectedName}` : "select an entity"}</span>
      </div>

      <textarea
        data-testid="compose-sentence"
        placeholder='e.g. "when an enemy dies and kills reach 4, set it on fire"'
        style={{ ...ctrl, width: "100%", height: 38, resize: "vertical", boxSizing: "border-box" }}
        value={sentence}
        onChange={(e) => setSentence(e.target.value)}
      />
      <div style={{ marginTop: 6 }}>
        <button
          data-testid="compose-propose"
          style={{ ...ctrl, fontWeight: 700 }}
          disabled={busy || sentence.trim().length === 0}
          onClick={() => void propose()}
        >
          {busy ? "thinking…" : "Propose"}
        </button>
      </div>

      {proposal && !proposal.ok && (
        <div data-testid="compose-error" style={{ color: "#f88", margin: "8px 0" }}>
          {proposal.error}
        </div>
      )}

      {proposal && proposal.ok && proposal.composition && (
        <div
          data-testid="compose-proposal"
          style={{ border: "1px solid #3a5", borderRadius: 4, padding: 8, marginTop: 8, background: "#0c1a0c" }}
        >
          <div style={{ marginBottom: 4 }}>
            Proposed <b data-testid="compose-opcount">{proposal.ops}</b> patch{proposal.ops === 1 ? "" : "es"} — review before applying:
          </div>
          <ul style={{ margin: "4px 0 8px 16px", padding: 0 }}>
            {proposal.composition.ops.map((op, i) => (
              <li key={i} data-testid="compose-op">
                {describeOp(op)}
              </li>
            ))}
          </ul>
          <button
            data-testid="compose-apply"
            style={{ ...ctrl, fontWeight: 700 }}
            disabled={busy}
            onClick={() => proposal.composition && void apply(proposal.composition)}
          >
            Apply
          </button>{" "}
          <button data-testid="compose-discard" style={ctrl} disabled={busy} onClick={() => setProposal(null)}>
            Discard
          </button>
        </div>
      )}
    </div>
  );
}
