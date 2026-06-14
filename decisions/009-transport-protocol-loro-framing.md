# ADR-009: Transport protocol = Loro Syncing Protocol v1 framing + opaque Loro-update payload

**Date:** 2026-06-14 · **Status:** Accepted — landed M2.4 (`/transport` + `/core` producer hook) · **Supersedes:** the "encoding undecided" note in the M1 `transport` trait sketch

## Context

Invariant 2 requires deltas-only across every boundary (core↔UI, client↔server). M2.4 had to pick a
*wire format* for those deltas and a transport abstraction with three impls (Tauri Channel, local
WebSocket, in-process WASM). Two failure modes to avoid: (a) inventing a bespoke per-op delta codec —
months of causality/ordering/idempotency work we'd own forever; (b) shipping bare Loro bytes with no
envelope — no room id, no ACK, no fragmentation, no versioning, and a hard coupling of the wire to
one library's output with no negotiation seam.

Research (verified June 2026) pointed at the **Loro Syncing Protocol v1**: a small envelope (magic +
room + message-type + payload; `DocUpdate` batches update blobs + a batch id for ACK; fragments for
large payloads; explicit `proto_version`; app-level ping/pong), carrying **Loro's own
`export({mode:"update", from: vv})` bytes as the opaque payload**.

## Decision

Adopt **Loro Protocol v1 framing** with the **Loro `update` blob as the opaque payload**. The framing
is ported into `/transport` (no `loro` dependency there — payloads are `&[u8]`, so the CI
`loro`-outside-`/core` grep stays green). The `/core` producer hook is the only place that turns a
committed transaction into update bytes (`export`) and applies inbound ones (`import` + merge
validation).

Concretely:
- **Envelope:** fixed 16-byte header — magic `frame_kind` (`%LOR` doc delta · `%ACK` · `%HSK`
  handshake · `%FRG` fragment · `%EPH` ephemeral, **reserved** for Phase-2 collab · `%PNG`/`%PNT`
  ping/pong) · `proto_version` · flags · room id · payload length.
- **`DeltaTransport` trait** is byte-only (`send` / `set_on_recv` / `connection_state`); ALL protocol
  logic — envelope codec, batch-id/ACK, **outbox-collapse backpressure**, fragmentation/reassembly,
  reconciliation hook — lives ONCE in a shared `DeltaSession` above the trait.
- **Coalescing = version-vector diff.** Each tick exports `updates_since(last_acked_vv)` — one blob
  spanning every commit since the peer's last ACK. This *is* the outbox-collapse: at most one
  `DocUpdate` in flight; while unacked the gap accumulates and the next send is ONE spanning update,
  never N queued frames.
- **Reconciliation** is Wait-for-Ack with an "unacknowledged" marker, behind a `Reconciler` hook.
  M2 single-process uses an instant-ack no-op; the Phase-2 undo→apply→redo server reconciliation is a
  drop-in swap, kept strictly separate from the engine-side *user* undo stack (M1.6 / ADR-002 F2).

## Consequences

- **Causality for free:** Loro's version vectors give ordering, idempotency, and out-of-order
  tolerance — verified: apply-update2-before-update1 converges; re-import is a no-op; reconnect from a
  stale vv pulls exactly the gap in one update.
- **Same wire serves Phase-2 collab unchanged** — the server is just another peer; `%EPH` is already
  reserved for presence/cursors.
- **Coupling is contained** by the envelope: the payload is opaque, so a future format change (or a
  second doc engine) is a payload-type negotiation via `proto_version` + a new `frame_kind`, not a
  rewrite. `/transport` never names a Loro type.
- **Numbers (M2.4):** envelope encode+decode of a 256 B coalesced delta — p50 0.7 µs / p99 ≤1.3 µs
  (negligible vs the 16.6 ms budget). End-to-end Tauri **Channel** latency (re-confirm M2.1's ~3.4 ms
  over WebView2) + the Windows initial-snapshot-load cliff are **deferred to M2.6** when the real
  shell is wired (the Channel impl + the envelope micro-bench stand alone now). Local WebSocket
  round-trips (loopback) but its measured latency here is poll-loop-bound, not the wire — see
  `transport/README.md`.

## Revisit when

- The Windows snapshot-load benchmark (M2.6) shows the Channel large-payload path breaches budget →
  route initial catch-up over WebSocket/fragments (the protocol already supports it).
- Phase-2 collab introduces concurrent remote ops → swap the `Reconciler` for the server-reconcile
  implementation and exercise the correction path end-to-end.
