//! The shared layer above [`DeltaTransport`] — written once, reused by every impl.
//!
//! Responsibilities: frame-tick **coalescing**, **batch-id/ACK** tracking, **outbox-collapse
//! backpressure** (at most one `DocUpdate` in flight; while it's unacked the gap accumulates and the
//! next send is ONE update spanning it — never N queued frames), **fragmentation/reassembly**, and a
//! **reconciliation hook**. None of this touches Loro: it drives a [`DeltaSource`] (export) and a
//! [`DeltaSink`] (apply), both byte-only, implemented by `/core` on `Engine`.

use crate::frame::{self, FrameKind, FRAGMENT_THRESHOLD};
use crate::{DeltaTransport, OnRecv};
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

// ── source / sink (byte-only; /core implements these on `Engine`) ──────────────

/// Exports outbound deltas. `/core` implements this on `Engine` via Loro `export(update, from)`.
pub trait DeltaSource {
    /// The document's current version vector, opaque bytes.
    fn version(&self) -> Vec<u8>;
    /// All updates since the given version vector (empty `vv` ⇒ full catch-up from empty). Opaque
    /// Loro `update` bytes — coalesces every commit since `vv` into one blob.
    fn updates_since(&self, vv: &[u8]) -> Vec<u8>;
}

/// Applies inbound deltas. `/core` implements this on `Engine` via Loro `import` + merge-validation.
pub trait DeltaSink {
    /// Apply one `update` blob.
    ///
    /// # Errors
    /// Returns [`ApplyError`] if the payload is not a valid update for this document.
    fn apply(&mut self, update: &[u8]) -> Result<ApplyOutcome, ApplyError>;
    /// The sink's current version vector (for handshake/resync).
    fn version(&self) -> Vec<u8>;
}

/// Result of applying an inbound delta.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// `true` if merge-validation repaired an invariant while applying — the **correction** case;
    /// `false` is the clean optimistic-echo **no-op** (predicted == authoritative).
    pub repaired: bool,
}

#[derive(Debug)]
pub struct ApplyError(pub String);

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "delta apply failed: {}", self.0)
    }
}
impl std::error::Error for ApplyError {}

// ── reconciliation hook (upgrade-ready; SEPARATE from the engine-side user-undo stack) ─────────

/// Hook for Wait-for-Ack reconciliation. M2 single-process uses [`InstantAck`] (the core acks
/// instantly, so nothing reconciles). The Phase-2 upgrade is a **drop-in** swap to a reconciler that
/// does undo→apply→redo on a `repaired` correction — and it stays strictly separate from the
/// engine-side inverse-op *user* undo stack (M1.6 / ADR-002 F2): this never touches `Engine::undo`.
pub trait Reconciler {
    /// A local batch was sent and is now unacknowledged.
    fn on_local_pending(&mut self, batch_id: u64);
    /// A local batch was acknowledged by the peer (echo received).
    fn on_acked(&mut self, batch_id: u64);
    /// A remote delta was applied; `repaired` distinguishes a correction from a clean no-op.
    fn on_remote(&mut self, repaired: bool);
}

/// M2 default: the single-process core acks instantly, so there is nothing to reconcile.
#[derive(Default)]
pub struct InstantAck;
impl Reconciler for InstantAck {
    fn on_local_pending(&mut self, _batch_id: u64) {}
    fn on_acked(&mut self, _batch_id: u64) {}
    fn on_remote(&mut self, _repaired: bool) {}
}

// ── stats (for tests + benches) ────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SessionStats {
    /// `DocUpdate` frames sent — the backpressure-collapse assertion counts these.
    pub doc_updates_sent: u64,
    pub acks_sent: u64,
    pub acks_received: u64,
    pub blobs_applied: u64,
    pub corrections: u64,
    pub decode_errors: u64,
    pub apply_errors: u64,
    pub fragments_sent: u64,
    pub fragments_dropped: u64,
}

// ── session ────────────────────────────────────────────────────────────────────

struct InFlight {
    batch_id: u64,
    to_version: Vec<u8>,
}

/// Drives one [`DeltaTransport`] with the full protocol logic. Generic over the transport; the
/// source/sink are passed per call so a session can be send-only, recv-only, or both.
pub struct DeltaSession<T: DeltaTransport> {
    transport: T,
    room_id: u32,
    peer_id: u64,
    remote_peer_id: Option<u64>,

    next_batch_id: u64,
    next_msg_id: u64,
    in_flight: Option<InFlight>,
    /// The peer's acknowledged version — outbound exports are taken `from` here, so an unacked gap
    /// re-exports as ONE spanning update rather than N queued frames.
    last_acked_vv: Vec<u8>,
    unacked: BTreeSet<u64>,

    inbox: Arc<Mutex<VecDeque<Vec<u8>>>>,
    reassembler: Reassembler,
    reconciler: Box<dyn Reconciler + Send>,
    stats: SessionStats,
}

impl<T: DeltaTransport> DeltaSession<T> {
    /// Wrap a transport. Installs the inbox callback. `peer_id` is this end's Loro PeerID (ADR-002
    /// F3), carried in the handshake.
    pub fn new(
        mut transport: T,
        room_id: u32,
        peer_id: u64,
        reconciler: Box<dyn Reconciler + Send>,
    ) -> Self {
        let inbox: Arc<Mutex<VecDeque<Vec<u8>>>> = Arc::new(Mutex::new(VecDeque::new()));
        let inbox_cb = inbox.clone();
        let cb: OnRecv = Box::new(move |bytes: &[u8]| {
            inbox_cb.lock().unwrap().push_back(bytes.to_vec());
        });
        transport.set_on_recv(cb);
        Self {
            transport,
            room_id,
            peer_id,
            remote_peer_id: None,
            next_batch_id: 1,
            next_msg_id: 1,
            in_flight: None,
            last_acked_vv: Vec::new(),
            unacked: BTreeSet::new(),
            inbox,
            reassembler: Reassembler::default(),
            reconciler,
            stats: SessionStats::default(),
        }
    }

    pub fn stats(&self) -> SessionStats {
        self.stats
    }
    pub fn is_in_flight(&self) -> bool {
        self.in_flight.is_some()
    }
    pub fn unacked_count(&self) -> usize {
        self.unacked.len()
    }
    pub fn remote_peer_id(&self) -> Option<u64> {
        self.remote_peer_id
    }
    pub fn connection_state(&self) -> crate::ConnectionState {
        self.transport.connection_state()
    }
    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    /// Send the handshake: this end's PeerID + the version we already hold (so the peer exports only
    /// what we're missing — also the reconnect-resync path: send a stale vv → peer ships the gap).
    ///
    /// # Errors
    /// Propagates the transport's send error.
    pub fn send_handshake(&mut self, known_vv: &[u8]) -> Result<(), T::Error> {
        let f = frame::encode_handshake(self.room_id, self.peer_id, known_vv);
        self.transport.send(&f)
    }

    /// Frame-tick coalescer + outbox-collapse backpressure. Call once per frame (or per commit).
    /// If a `DocUpdate` is already in flight this returns immediately (collapse — the gap
    /// accumulates); otherwise it exports ONE update spanning everything since the last ACK.
    ///
    /// # Errors
    /// Propagates the transport's send error.
    pub fn tick<S: DeltaSource>(&mut self, source: &S) -> Result<(), T::Error> {
        if self.in_flight.is_some() {
            return Ok(()); // backpressure-collapse: at most one in flight
        }
        let cur = source.version();
        let update = source.updates_since(&self.last_acked_vv);
        if update.is_empty() {
            return Ok(()); // nothing new since the peer's acked version
        }
        let batch_id = self.next_batch_id;
        self.next_batch_id += 1;
        let frame = frame::encode_doc_update(self.room_id, batch_id, &[&update]);
        self.in_flight = Some(InFlight {
            batch_id,
            to_version: cur,
        });
        self.unacked.insert(batch_id);
        self.reconciler.on_local_pending(batch_id);
        self.stats.doc_updates_sent += 1;
        self.send_framed(&frame)
    }

    /// Drain inbound frames, applying deltas to `sink` and acking them, processing acks, reassembling
    /// fragments. `now_ms` is a monotonic clock for fragment timeouts (passed in, not read, so this
    /// crate needs no `std::time` and stays wasm-portable / seed-testable).
    ///
    /// # Errors
    /// Propagates the transport's send error (e.g. while sending an ACK).
    pub fn pump<K: DeltaSink>(&mut self, sink: &mut K, now_ms: u64) -> Result<(), T::Error> {
        let frames: Vec<Vec<u8>> = {
            let mut q = self.inbox.lock().unwrap();
            q.drain(..).collect()
        };
        for raw in frames {
            self.handle_frame(&raw, sink, now_ms)?;
        }
        Ok(())
    }

    /// Drop fragment reassemblies older than `timeout_ms`; returns the dropped `msg_id`s. The caller
    /// recovers by re-exporting from `last_acked` (idempotent), so no explicit resend frame is
    /// needed — out-of-order/loss tolerance is Loro's version-vector causality at work.
    pub fn sweep_fragments(&mut self, now_ms: u64, timeout_ms: u64) -> Vec<u64> {
        let dropped = self.reassembler.sweep(now_ms, timeout_ms);
        self.stats.fragments_dropped += dropped.len() as u64;
        dropped
    }

    // ── internals ────────────────────────────────────────────────────────

    fn send_framed(&mut self, frame: &[u8]) -> Result<(), T::Error> {
        if frame.len() <= FRAGMENT_THRESHOLD {
            return self.transport.send(frame);
        }
        let msg_id = self.next_msg_id;
        self.next_msg_id += 1;
        for part in frame::fragment(self.room_id, msg_id, frame) {
            self.transport.send(&part)?;
            self.stats.fragments_sent += 1;
        }
        Ok(())
    }

    fn handle_frame<K: DeltaSink>(
        &mut self,
        raw: &[u8],
        sink: &mut K,
        now_ms: u64,
    ) -> Result<(), T::Error> {
        let Ok(header) = frame::decode(raw) else {
            self.stats.decode_errors += 1;
            return Ok(()); // a malformed frame never kills the session
        };
        match header.kind {
            FrameKind::DocUpdate => {
                let Ok(du) = frame::decode_doc_update(header.payload) else {
                    self.stats.decode_errors += 1;
                    return Ok(());
                };
                let mut repaired = false;
                for blob in &du.blobs {
                    match sink.apply(blob) {
                        Ok(o) => {
                            self.stats.blobs_applied += 1;
                            repaired |= o.repaired;
                        }
                        Err(_) => self.stats.apply_errors += 1,
                    }
                }
                if repaired {
                    self.stats.corrections += 1;
                }
                self.reconciler.on_remote(repaired);
                let ack = frame::encode_ack(self.room_id, du.batch_id);
                self.stats.acks_sent += 1;
                self.transport.send(&ack)?;
            }
            FrameKind::Ack => {
                if let Ok(bid) = frame::decode_ack(header.payload) {
                    self.stats.acks_received += 1;
                    if let Some(inf) = &self.in_flight {
                        if inf.batch_id == bid {
                            self.last_acked_vv = inf.to_version.clone();
                            self.unacked.remove(&bid);
                            self.in_flight = None;
                            self.reconciler.on_acked(bid);
                        }
                    } else {
                        self.unacked.remove(&bid);
                    }
                }
            }
            FrameKind::Handshake => {
                if let Ok(hs) = frame::decode_handshake(header.payload) {
                    self.remote_peer_id = Some(hs.peer_id);
                    // The peer tells us what it already holds → our outbound resync point. Also the
                    // reconnect path: a reconnecting peer's stale vv makes our next tick ship the gap.
                    self.last_acked_vv = hs.known_vv;
                    self.in_flight = None;
                }
            }
            FrameKind::Fragment => {
                if let Ok(part) = frame::decode_fragment(header.payload) {
                    if let Some(inner) = self.reassembler.push(&part, now_ms) {
                        self.handle_frame(&inner, sink, now_ms)?;
                    }
                } else {
                    self.stats.decode_errors += 1;
                }
            }
            FrameKind::Ping => {
                let pong = frame::encode_pong(self.room_id);
                self.transport.send(&pong)?;
            }
            // Pong: liveness only. Ephemeral: reserved for Phase-2 collab — ignored in M2.
            FrameKind::Pong | FrameKind::Ephemeral => {}
        }
        Ok(())
    }
}

// ── fragment reassembly ─────────────────────────────────────────────────────────

#[derive(Default)]
struct Reassembler {
    parts: HashMap<u64, Partial>,
}

struct Partial {
    total: usize,
    received: usize,
    buf: Vec<u8>,
    started_ms: u64,
}

impl Reassembler {
    /// Add a fragment; returns the reassembled inner frame once all bytes have arrived.
    fn push(&mut self, part: &frame::FragmentPart, now_ms: u64) -> Option<Vec<u8>> {
        let entry = self.parts.entry(part.msg_id).or_insert_with(|| Partial {
            total: part.total_len,
            received: 0,
            buf: vec![0u8; part.total_len],
            started_ms: now_ms,
        });
        let end = part.offset.saturating_add(part.chunk.len());
        if end > entry.total {
            return None; // malformed fragment; ignore
        }
        entry.buf[part.offset..end].copy_from_slice(&part.chunk);
        entry.received += part.chunk.len();
        if entry.received >= entry.total {
            return self.parts.remove(&part.msg_id).map(|p| p.buf);
        }
        None
    }

    fn sweep(&mut self, now_ms: u64, timeout_ms: u64) -> Vec<u64> {
        let stale: Vec<u64> = self
            .parts
            .iter()
            .filter(|(_, p)| now_ms.saturating_sub(p.started_ms) > timeout_ms)
            .map(|(id, _)| *id)
            .collect();
        for id in &stale {
            self.parts.remove(id);
        }
        stale
    }
}
