//! The **wallet** store (M10.10) — the user's token balance, the SINGLE source for every surface that
//! shows or spends it (the top-bar wallet · the AI-edit panel · the describe-bar generate). Centralizing
//! it means a spend anywhere updates the balance everywhere, and — paired with a toast on every change —
//! a spend is always VISIBLE (C7: never silently mutate a balance). The authoritative balance is the
//! shell's (M7 / ADR-018); this mirrors the value every econ/generate response carries back.

import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";

interface WalletState {
  /** The token balance, or `null` before the first `wallet_info` read. */
  balance: number | null;
  setBalance(n: number | null): void;
  reset(): void;
}

export const walletStore = createStore<WalletState>((set) => ({
  balance: null,
  setBalance: (balance) => set({ balance }),
  reset: () => set({ balance: null }),
}));

/** Mirror the authoritative balance from any econ/generate response (the one chokepoint). */
export const setBalance = (n: number | null): void => walletStore.getState().setBalance(n);

/** Subscribe to the token balance (null until the first `wallet_info` read). */
export const useBalance = (): number | null => useStore(walletStore, (s) => s.balance);
