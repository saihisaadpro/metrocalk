# `metrocalk-transport` — the deltas-only wire (M2.4)

Invariant 2 made real: every boundary (core↔UI, client↔server) carries **changes, never snapshots**.
Adopts the **Loro Syncing Protocol v1** framing with **Loro's own `export(update, from)` bytes as the
opaque payload** — so ordering, idempotency, and out-of-order tolerance come from Loro's version-vector
causality for free, and the same wire serves Phase-2 collab unchanged. See
[ADR-009](../decisions/009-transport-protocol-loro-framing.md).

**This crate links no `loro`/Flecs** — payloads are `&[u8]`; the Loro `export`/`import` lives in the
`/core` producer hook (`core/src/producer.rs`). The CI `loro`-outside-`/core` grep stays green.

## Layers

```
frame.rs     Loro-Protocol-v1 envelope: kinds, batch id, fragments, ping/pong. Pure codec.
DeltaTransport (lib.rs)   byte-only trait: send(&[u8]) · set_on_recv(cb) · connection_state()
session.rs   DeltaSession — ALL protocol logic, written once above the trait:
             coalescing · batch-id/ACK · outbox-collapse backpressure · fragments · reconcile hook
impls.rs     in_process_pair() · ChannelTransport (Tauri) · [ws] WebSocketTransport
core/producer.rs (in /core)   DeltaSource/DeltaSink on Engine + ProducerHook + PeerID handshake
```

## Frame layout

Fixed 16-byte header + kind-specific payload. The 4 magic bytes **are** the frame kind (ASCII, so a
hex dump is readable):

```
offset  bytes  field
0       4      magic / frame_kind:  %LOR doc-delta · %ACK · %HSK handshake · %FRG fragment
                                    %EPH ephemeral (RESERVED, Phase-2) · %PNG/%PNT ping/pong
4       1      proto_version (= 1)
5       1      flags (reserved)
6       4      room_id (u32 LE)
10      4      payload_len (u32 LE)
14      2      reserved
16      ..     payload
```

- **`%LOR` DocUpdate** payload: `batch_id` (u64) · `n_blobs` (u32) · `[len(u32) + blob]…`. Each blob
  is a Loro `update`. A frame-tick coalescer normally emits ONE blob spanning the tick.
- **`%ACK`**: the 8-byte `batch_id` being acknowledged.
- **`%HSK`**: `peer_id` (u64, the Loro PeerID — ADR-002 F3) + the sender's known version vector
  (drives resync: a stale vv makes the peer ship exactly the gap).
- **`%FRG`**: `msg_id · total_len · offset · chunk_len · chunk` — wraps an oversized inner frame
  (split at 256 KiB; the initial catch-up is the case that needs it).

## Backpressure: outbox-collapse

At most **one `DocUpdate` in flight**. While it's unacked the gap simply accumulates; the next send
exports `updates_since(last_acked_vv)` — **one update spanning the whole gap**, never N queued frames.
This falls straight out of coalescing-by-version-vector-diff (no separate queue to drain).

## How to run

```
# unit/integration tests (no networking dep): 10 tests
cargo test -p metrocalk-transport
# include the WebSocket impl + its loopback round-trip:
cargo test -p metrocalk-transport --features ws

# end-to-end with two real Loro engines (convergence, out-of-order, idempotent, reconnect, echo):
cargo test -p metrocalk-core --test transport_sync

# micro-benches (run twice per discipline; --nocapture prints the numbers):
cargo test -p metrocalk-transport --release channel_envelope -- --nocapture
cargo test -p metrocalk-transport --release --features ws websocket -- --nocapture
```

(`cargo` is not on PATH in this environment — prepend the rustup bin; see the repo memory.)

## Measured (M2.4)

Env: Win11 26200 · i9-13900H · rustc 1.92.0 · `tungstenite` 0.24 (no-TLS, loopback) · 2 runs each.

| path | metric | result |
|---|---|---|
| Channel envelope encode+decode (256 B coalesced delta) | p50 / p99 | **0.7 µs / 0.9–1.3 µs** (≈0.001 ms — negligible vs 16.6 ms) |
| WebSocket loopback round-trip (256 B) | p50 / p99 | 1.5–3.4 ms / 2.0–3.8 ms — **poll-loop-bound** (1 ms reader sleep), an upper bound, NOT the wire |

**Deferred to M2.6** (needs the real Tauri shell, which is on the unmerged `m2.1` branch): the
end-to-end Tauri **Channel** latency (re-confirm M2.1's ~3.4 ms p99 over WebView2) and the **Windows
initial-snapshot-load** cliff (route startup over WS/fragments if it breaches budget). The Channel
impl + the envelope micro-bench above stand alone now.

## Honest boundaries

- The WS latency above is dominated by the foundation's **polling reader** (sleeps 1 ms between
  non-blocking reads), so it over-reports and swings run-to-run; the authoritative WS IPC figure is
  M2.1's 1.3 ms. A future event-driven reader removes the poll floor.
- The **correction** reconciliation path (`repaired == true`) is surfaced (`ApplyOutcome.repaired`,
  `Reconciler::on_remote`) but not exercised end-to-end here — forcing a merge-repair is covered by
  `core/tests/merge.rs`. The Phase-2 undo→apply→redo server reconcile is a drop-in `Reconciler`.
- No real collab server (Phase 2) and no concurrent remote ops yet — single-process, instant-ack.
