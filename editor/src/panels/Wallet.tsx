//! Wallet (M10.10 / C4·C7) — the token-balance surface ONLY. The AI-edit "rustier" action MOVED out of
//! here to `AiEditPanel` (it used to float over + clip the balance/Top-up — C4). The balance is the
//! centralized `wallet` store (so a spend anywhere — generate, AI-edit, marketplace — updates it here),
//! read from `wallet_info` on mount. "Top up" is HONEST: a sandbox **dev grant** labelled "+100 dev
//! tokens" (not an implied purchase), and the change is VISIBLE (a toast + a brief flash) — never a silent
//! balance mutation (C7). Keeps the scaffold's stable `#walletBal`/`#topup` ids (prompt-40).

import { useEffect, useRef, useState } from "react";
import { useBalance, setBalance } from "../store/wallet";
import { setStatus } from "../store/ui";
import { pushToast } from "../store/toasts";
import type { EditorClient } from "../transport/session";

export function Wallet({ client }: { client: EditorClient }) {
  const balance = useBalance();
  const [flash, setFlash] = useState(false);
  const flashTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

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

  // Clear a pending flash timer on unmount (test hygiene — no setState after teardown).
  useEffect(() => () => {
    if (flashTimer.current) clearTimeout(flashTimer.current);
  }, []);

  async function onTopUp() {
    try {
      const r = await client.topUp();
      setBalance(r.balance);
      setFlash(true);
      flashTimer.current = setTimeout(() => setFlash(false), 600);
      // HONEST: a sandbox dev grant, not a purchase; the change is loud (toast + status), never silent.
      pushToast(`+100 dev tokens (sandbox grant — no purchase) · ${r.balance} total`, "cost");
      setStatus(`+100 dev tokens · ${r.balance} total`);
    } catch (e) {
      console.error("top_up failed", e);
      pushToast("top up failed — please try again", "error");
    }
  }

  return (
    <div
      id="wallet"
      data-testid="wallet"
      style={{ padding: 12, fontSize: 13, color: "#fbbf24", display: "flex", alignItems: "center", gap: 6, whiteSpace: "nowrap", minWidth: 0 }}
    >
      <span>
        ⊞{" "}
        <span id="walletBal" data-testid="balance" style={{ transition: "color .2s", color: flash ? "#fff" : "#fbbf24" }}>
          {balance ?? "…"}
        </span>{" "}
        tokens
      </span>
      <button
        id="topup"
        data-testid="topup"
        onClick={onTopUp}
        title="Sandbox dev grant — no real purchase (ADR-004/018)"
        style={{ padding: "2px 8px", fontSize: 11, background: "#4a3a1f", color: "#fbbf24", border: "1px solid #5a4a2f", borderRadius: 4, cursor: "pointer" }}
      >
        +100 dev tokens
      </button>
    </div>
  );
}
