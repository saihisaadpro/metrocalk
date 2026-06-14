//! `metrocalk-transport` — the deltas-only wire (invariant 2). Real as of M2.4.
//!
//! Three layers, bottom-up:
//! 1. [`frame`] — the **Loro Syncing Protocol v1** envelope (kinds, batch id, fragments). No Loro dep.
//! 2. [`DeltaTransport`] — a byte-oriented trait (`send` / `set_on_recv` / `connection_state`) with
//!    three impls (in-process, Tauri Channel, local WebSocket). The transport only moves bytes.
//! 3. [`session::DeltaSession`] — the shared logic written ONCE above the trait: frame-tick
//!    coalescing, batch-id/ACK, **outbox-collapse backpressure**, fragmentation/reassembly, and the
//!    reconciliation hook. It drives a [`DeltaSource`] (export) + [`DeltaSink`] (apply), both of
//!    which are byte-only so this crate links no Loro/Flecs — `/core` implements them on `Engine`.
//!
//! Invariant 2 is structural: the only thing you can move is a *delta* (a Loro `update` blob inside a
//! `DocUpdate`); there is no `send_snapshot`. The initial catch-up is still a delta-from-empty,
//! fragmented when large.

pub mod frame;
pub mod impls;
pub mod session;

pub use session::{
    ApplyError, ApplyOutcome, DeltaSession, DeltaSink, DeltaSource, InstantAck, Reconciler,
    SessionStats,
};

/// Liveness of a transport's underlying channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Connecting,
    Connected,
    Disconnected,
}

/// A received-frame callback. Installed by the session; invoked by the transport's recv source
/// (the WebSocket reader thread, the in-process peer, or the Tauri `invoke` handler).
pub type OnRecv = Box<dyn FnMut(&[u8]) + Send>;

/// Moves opaque enveloped frames across one boundary. Deliberately byte-only and snapshot-free:
/// all protocol logic (envelope, ACK, backpressure, fragments) lives in [`session::DeltaSession`]
/// above this trait, so each impl is just a byte pump.
pub trait DeltaTransport {
    /// Transport-specific send error.
    type Error: std::error::Error + Send + 'static;

    /// Send one already-enveloped frame toward the peer.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying channel rejects the bytes.
    fn send(&mut self, frame: &[u8]) -> Result<(), Self::Error>;

    /// Install the received-frame callback. The session installs one that enqueues into its inbox.
    fn set_on_recv(&mut self, cb: OnRecv);

    /// Current liveness of the channel.
    fn connection_state(&self) -> ConnectionState;
}
