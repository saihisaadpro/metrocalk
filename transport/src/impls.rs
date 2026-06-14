//! The three [`DeltaTransport`] impls. Each is a thin byte pump — all protocol logic lives in
//! [`crate::session`]. (a) [`InProcessTransport`] (browser, zero-copy sync loopback — the browser is
//! Loro-authoritative so "send" is a direct callback), (b) [`ChannelTransport`] (desktop default,
//! raw bytes over a Tauri `Channel<InvokeResponseBody>` — the channel is injected by the shell at
//! M2.6), (c) [`WebSocketTransport`] (fallback + collab foundation; native, behind the `ws` feature).

use crate::{ConnectionState, DeltaTransport, OnRecv};
use std::sync::{Arc, Mutex};

#[cfg(feature = "ws")]
mod ws;
#[cfg(feature = "ws")]
pub use ws::{WebSocketTransport, WsServer};

/// Shared transport error. In-process never fails; Channel surfaces the injected sink's error;
/// WebSocket surfaces I/O / protocol errors.
#[derive(Debug)]
pub enum TransportError {
    /// The channel is closed / the peer is gone.
    Closed,
    /// Underlying I/O failure (WebSocket).
    Io(std::io::Error),
    /// Protocol/library error text (WebSocket).
    Protocol(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Closed => write!(f, "transport closed"),
            TransportError::Io(e) => write!(f, "transport io: {e}"),
            TransportError::Protocol(s) => write!(f, "transport protocol: {s}"),
        }
    }
}
impl std::error::Error for TransportError {}

type CbSlot = Arc<Mutex<Option<OnRecv>>>;

// ── (a) in-process: zero-copy synchronous loopback ────────────────────────────

/// One end of an in-process pair. `send` synchronously invokes the peer's installed callback — no
/// serialization queue, no thread. Models the browser build where the Loro doc is local and the
/// "wire" is a function call (ADR-006: browser is Loro-authoritative).
pub struct InProcessTransport {
    my_cb: CbSlot,
    peer_cb: CbSlot,
}

/// Create a connected in-process pair (e.g. core ↔ in-browser consumer).
#[must_use]
pub fn in_process_pair() -> (InProcessTransport, InProcessTransport) {
    let a: CbSlot = Arc::new(Mutex::new(None));
    let b: CbSlot = Arc::new(Mutex::new(None));
    (
        InProcessTransport {
            my_cb: a.clone(),
            peer_cb: b.clone(),
        },
        InProcessTransport {
            my_cb: b,
            peer_cb: a,
        },
    )
}

impl DeltaTransport for InProcessTransport {
    type Error = TransportError;
    fn send(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        if let Some(cb) = self.peer_cb.lock().unwrap().as_mut() {
            cb(frame);
        }
        // No installed peer callback yet ⇒ pre-handshake; dropping is correct (idempotent re-export
        // recovers). Never an error for the in-process loopback.
        Ok(())
    }
    fn set_on_recv(&mut self, cb: OnRecv) {
        *self.my_cb.lock().unwrap() = Some(cb);
    }
    fn connection_state(&self) -> ConnectionState {
        ConnectionState::Connected
    }
}

// ── (b) Tauri Channel: raw bytes (NOT JSON) over Channel<InvokeResponseBody> ───────────────────

type ByteSink = Box<dyn FnMut(&[u8]) -> Result<(), TransportError> + Send>;

/// Desktop-default transport. Outbound frames go through the injected `sink` — at M2.6 the shell
/// wires that to `channel.send(InvokeResponseBody::Raw(bytes))` (raw bytes, **not** JSON, to avoid
/// the base64/JSON tax on the 60 Hz delta path). Inbound frames (JS→Rust `invoke` raw-binary) are
/// delivered via [`ChannelTransport::deliver`].
///
/// The end-to-end Windows Channel benchmark (re-confirm M2.1's ~3.4 ms + the snapshot-load cliff)
/// lands at M2.6 when the real Tauri shell is wired — see RESULTS/README. Here the impl + the
/// envelope/encode micro-bench stand alone (decision: build impl, defer e2e).
pub struct ChannelTransport {
    sink: ByteSink,
    inbound: CbSlot,
    state: ConnectionState,
}

impl ChannelTransport {
    /// Build with the outbound byte sink (the shell passes the Tauri `Channel.send` closure).
    #[must_use]
    pub fn new(sink: ByteSink) -> Self {
        Self {
            sink,
            inbound: Arc::new(Mutex::new(None)),
            state: ConnectionState::Connected,
        }
    }
    /// Deliver an inbound frame (called by the shell's JS→Rust `invoke` handler).
    pub fn deliver(&self, frame: &[u8]) {
        if let Some(cb) = self.inbound.lock().unwrap().as_mut() {
            cb(frame);
        }
    }
    /// Handle the shell can hold to deliver inbound frames from another thread.
    #[must_use]
    pub fn inbound_handle(&self) -> CbSlot {
        self.inbound.clone()
    }
}

impl DeltaTransport for ChannelTransport {
    type Error = TransportError;
    fn send(&mut self, frame: &[u8]) -> Result<(), Self::Error> {
        (self.sink)(frame)
    }
    fn set_on_recv(&mut self, cb: OnRecv) {
        *self.inbound.lock().unwrap() = Some(cb);
    }
    fn connection_state(&self) -> ConnectionState {
        self.state
    }
}
