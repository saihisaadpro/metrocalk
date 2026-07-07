//! Shared, deduplicated reveal cache (perf audit F2 / RC-5). The Reveal picker and the Diagnostics
//! panel both display the same `reveal_targets` result for the selected entity; before this each fired
//! its OWN blocking `reveal_targets` IPC on every select (and every outgoing-edge change) — two
//! round-trips per interaction on the tao-thread-coupled command path. Here a single fetch per
//! `(selectedId, edgeSig)` key fans out to every subscriber; a second panel asking for the same key
//! reuses the in-flight/last result instead of issuing a duplicate round-trip.
//!
//! Keyed on `id + edgeSig` for the SAME reason the panels were (see Reveal.tsx): an optimistic bind
//! flips an edge `pending → confirmed`, which must re-fetch so the target moves into "tracking". The
//! key changing is the re-fetch trigger; the key matching is the de-dupe.

import { useEffect, useRef } from "react";
import { createStore } from "zustand/vanilla";
import { useStore } from "zustand";
import { useSelectedId, useEdges } from "./projection";
import type { EditorClient } from "../transport/session";
import type { RevealResponse } from "../transport/protocol";

const EMPTY: RevealResponse = { required: [], compatible: [], greyed: [], bound: [] };

interface RevealCache {
  key: string;
  /** The key an IPC is currently in flight for — concurrent same-key requests share it (one round-trip). */
  inFlight: string | null;
  value: RevealResponse;
  /**
   * Fetch the reveal for `(id, key)`. A same-key re-render de-dupes against the cached result; `force`
   * (a component MOUNT) re-fetches even on a cache hit — stale-while-revalidate: the cached list shows
   * instantly, the fresh one lands when the IPC resolves. Without the mount refresh, a scene change that
   * does NOT touch the selection's outgoing edges (a provider entity added/deleted) left a reopened
   * panel showing the stale cached list indefinitely.
   */
  fetch(client: EditorClient, id: string, key: string, force?: boolean): void;
}

// A sentinel initial key so the very first real request (even the empty "no selection" key) claims and
// runs exactly once.
const revealCache = createStore<RevealCache>((set, get) => ({
  key: " uninitialized",
  inFlight: null,
  value: EMPTY,
  fetch(client, id, key, force = false) {
    const s = get();
    if (s.inFlight === key) return; // a fetch for this exact key is in flight — share it (one IPC)
    if (!force && key === s.key) return; // fresh-enough for a re-render with an unchanged key
    if (!id) {
      set({ key, inFlight: null, value: EMPTY });
      return;
    }
    // Claim the key so a peer panel's effect de-dupes; keep the last value visible until the new one
    // resolves (no flicker — matches the panels' prior behaviour).
    set({ key, inFlight: key });
    client
      .revealTargets(id)
      .then((r) => {
        if (get().inFlight === key) set({ value: r, inFlight: null });
      })
      .catch(() => {
        if (get().inFlight === key) set({ value: EMPTY, inFlight: null });
      });
  },
}));

/** The reveal for the current selection — shared across panels, one IPC per `(id, edgeSig)`. */
export function useReveal(client: EditorClient): RevealResponse {
  const id = useSelectedId();
  const edges = useEdges();
  const key = id
    ? id +
      "|" +
      Object.values(edges)
        .filter((e) => e.from === id)
        .map((e) => `${e.id}:${e.status}`)
        .sort()
        .join(",")
    : "";
  // First effect run per component INSTANCE = a mount (the drawer reopened, a layout change) → force a
  // revalidating fetch even when the key matches the cache; later runs are key changes (the normal path).
  const mounted = useRef(false);
  useEffect(() => {
    const force = !mounted.current;
    mounted.current = true;
    revealCache.getState().fetch(client, id ?? "", key, force);
  }, [client, id, key]);
  return useStore(revealCache, (s) => s.value);
}
