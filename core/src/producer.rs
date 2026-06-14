//! The `/core` producer hook — the seam where a committed transaction becomes a coalesced delta on
//! the wire (invariant 2). This is the one place that bridges the Loro-backed [`Engine`] to the
//! Loro-free [`metrocalk_transport`] layer: it implements the transport's byte-only [`DeltaSource`]
//! / [`DeltaSink`] on `Engine` (here, where `loro` is allowed), and bundles a [`DeltaSession`] with
//! the engine's PeerID handshake.
//!
//! **Coalescing strategy.** Rather than buffering Loro `subscribe_local_update` callbacks, we coalesce
//! by **version-vector diff**: each tick exports `updates_since(last_acked_vv)` — one blob spanning
//! every commit since the peer's last ACK. This *is* the outbox-collapse: while a `DocUpdate` is in
//! flight the gap simply accumulates, and the next export covers it in one frame (never N queued).
//! Same observable result as a subscribe-buffer, with less state and a built-in backpressure path.
//!
//! **Separation of concerns.** Transport reconciliation (Wait-for-Ack, [`metrocalk_transport::
//! Reconciler`]) is kept strictly apart from the engine-side *user* undo stack (M1.6 / ADR-002 F2):
//! nothing here touches [`Engine::undo`]. The Phase-2 server-reconciliation (undo→apply→redo on a
//! correction) drops in by swapping the session's reconciler.

use crate::Engine;
use metrocalk_ecs::World;
use metrocalk_transport::{
    ApplyError, ApplyOutcome, DeltaSession, DeltaSink, DeltaSource, DeltaTransport, InstantAck,
};

// ── Engine as a byte-only delta source/sink (the Loro boundary) ────────────────

impl<W: World> DeltaSource for Engine<W> {
    fn version(&self) -> Vec<u8> {
        self.version_vector()
    }

    fn updates_since(&self, vv: &[u8]) -> Vec<u8> {
        // Empty vv ⇒ the peer has nothing ⇒ full catch-up (still a delta-from-empty, fragmented
        // when large — never a snapshot method on the wire). Otherwise: the delta since their vv.
        if vv.is_empty() {
            self.export_updates()
        } else {
            self.export_updates_since(vv)
        }
    }
}

impl<W: World> DeltaSink for Engine<W> {
    fn apply(&mut self, update: &[u8]) -> Result<ApplyOutcome, ApplyError> {
        // import + merge-validation (detect/repair the 8 invalid-state classes) + ECS rebuild.
        // `repaired` is the optimistic-echo signal: false = clean no-op, true = correction.
        let report = self.merge(update).map_err(|e| ApplyError(e.to_string()))?;
        Ok(ApplyOutcome {
            repaired: report.total_repairs > 0,
        })
    }

    fn version(&self) -> Vec<u8> {
        self.version_vector()
    }
}

// ── producer hook ──────────────────────────────────────────────────────────────

/// Bundles a [`DeltaSession`] with an [`Engine`] for the deltas-only wire. Drive it per frame:
/// [`on_tick`](Self::on_tick) coalesces + sends outbound deltas; [`pump`](Self::pump) applies inbound
/// deltas (and acks). Construct with [`attach`](Self::attach), which performs the PeerID handshake.
pub struct ProducerHook<T: DeltaTransport> {
    session: DeltaSession<T>,
    /// Monotonic frame counter handed to the session as the fragment-timeout clock (no `std::time`
    /// in the transport layer — keeps it wasm-portable and seed-testable).
    frame_clock: u64,
}

impl<T: DeltaTransport> ProducerHook<T> {
    /// Attach `transport` to drive deltas out of `engine`. Establishes peer identity — the engine's
    /// Loro PeerID (ADR-002 F3) — in the handshake before any delta flows. `known_vv` is what THIS
    /// end already holds (empty toward a fresh peer; a stale vv drives reconnect-resync).
    ///
    /// # Errors
    /// Propagates the transport's send error from the handshake frame.
    pub fn attach<W: World>(
        engine: &Engine<W>,
        transport: T,
        room_id: u32,
        known_vv: &[u8],
    ) -> Result<Self, T::Error> {
        let mut session =
            DeltaSession::new(transport, room_id, engine.peer_id(), Box::new(InstantAck));
        session.send_handshake(known_vv)?;
        Ok(Self {
            session,
            frame_clock: 0,
        })
    }

    /// Coalesce + send one frame's worth of deltas. Call once per frame tick (or after a commit).
    /// No-op while a `DocUpdate` is in flight (backpressure-collapse).
    ///
    /// # Errors
    /// Propagates the transport's send error.
    pub fn on_tick<W: World>(&mut self, engine: &Engine<W>) -> Result<(), T::Error> {
        self.session.tick(engine)
    }

    /// Drain inbound frames, applying remote deltas to `engine` and acking them.
    ///
    /// # Errors
    /// Propagates the transport's send error (e.g. while sending an ACK).
    pub fn pump<W: World>(&mut self, engine: &mut Engine<W>) -> Result<(), T::Error> {
        let now = self.frame_clock;
        self.frame_clock += 1;
        self.session.pump(engine, now)
    }

    /// The underlying session (stats, in-flight state, connection state).
    pub fn session(&self) -> &DeltaSession<T> {
        &self.session
    }

    /// Mutable session access (e.g. to send a fresh handshake on reconnect, or sweep fragments).
    pub fn session_mut(&mut self) -> &mut DeltaSession<T> {
        &mut self.session
    }
}
