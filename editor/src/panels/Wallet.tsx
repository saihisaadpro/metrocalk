//! Wallet + AI-edit (M7) — the token-economy surface. On mount the balance is read from the shell's
//! `wallet_info` command via the `EditorClient`; "Top up" grants the sandbox stipend (`top_up`,
//! ADR-004/018 — no real money) and "Make it rustier" runs the schema-validated AI patch (`ai_edit`,
//! debit-on-success). Refuse-when-broke is EXPLAINED: when `ai_edit` returns `!ok` the surfaced
//! `message` lands in the status line and the displayed balance is left UNCHANGED (the charge never
//! happened). The AI-edit button only appears when an entity is selected — there's nothing to edit
//! otherwise.
//!
//! The `#walletBal` / `#topup` / `#rustier` id hooks + `data-testid`s mirror the vanilla scaffold's
//! stable signals, so the prompt-40 acceptance page-object re-greens by selector-swap, not a rewrite.

import { useEffect, useState } from "react";
import { useSelectedId } from "../store/projection";
import { setStatus } from "../store/ui";
import type { EditorClient } from "../transport/session";

export function Wallet({ client }: { client: EditorClient }) {
  const selectedId = useSelectedId();
  const [balance, setBalance] = useState<number | null>(null);

  // Read the authoritative balance on mount (and if the client identity changes).
  useEffect(() => {
    let live = true;
    client
      .walletInfo()
      .then((r) => {
        if (live) setBalance(r.balance);
      })
      .catch(() => {
        /* leave the placeholder — a failed read must not crash the chrome */
      });
    return () => {
      live = false;
    };
  }, [client]);

  async function onTopUp() {
    const r = await client.topUp();
    setBalance(r.balance);
    setStatus(`topped up · ${r.balance} tokens`);
  }

  async function onRustier() {
    if (!selectedId) return;
    const r = await client.aiEdit(selectedId);
    if (r.ok) {
      // Debit-on-success: the new balance is authoritative; surface the charge.
      setBalance(r.balance);
      setStatus(`rustier · −${r.cost}`);
    } else {
      // Refuse-when-broke, EXPLAINED: surface the reason, leave the balance untouched (no charge).
      setStatus(r.message ?? "refused");
    }
  }

  return (
    <div id="wallet" data-testid="wallet" style={{ padding: 12, fontSize: 13, color: "#fbbf24" }}>
      <div>
        ⊞ <span id="walletBal" data-testid="balance">{balance ?? "…"}</span> tokens
        <button
          id="topup"
          data-testid="topup"
          onClick={onTopUp}
          style={{ margin: "0 0 0 6px", padding: "2px 8px", fontSize: 11, background: "#4a3a1f", color: "#fbbf24", border: "1px solid #5a4a2f", borderRadius: 4, cursor: "pointer" }}
        >
          Top up +100
        </button>
      </div>
      {selectedId && (
        <button
          id="rustier"
          data-testid="rustier"
          onClick={onRustier}
          style={{ margin: "8px 0 0", padding: "4px 8px", background: "#5a2f1f", color: "#fcd", border: "1px solid #6a3f2f", borderRadius: 4, cursor: "pointer" }}
        >
          ✦ Make it rustier (≈2 tokens)
        </button>
      )}
    </div>
  );
}
