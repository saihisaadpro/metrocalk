//! `metrocalk-transport` — the one protocol trait carried across every boundary
//! (core to UI, client to server). Three concrete impls land in M2+ (in-process WASM call,
//! Tauri channels, WebSocket); this is the trait sketch only — no impls, no encoding decided.
//!
//! Invariant 2 (deltas only): every boundary carries *changes*, never full-state snapshots. The
//! trait is shaped so the only thing you can move is a [`Delta`] — there is deliberately no
//! `send_state` / `snapshot` method, so "ship the whole world" is never the easy path.

/// An ordered change on a boundary. The payload — a revision counter plus a binary / JSON-Patch
/// encoding — is defined alongside the commit pipeline in M1-2; the invariant fixed now is that a
/// `Delta` represents a *change*, never a snapshot.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Delta;

/// Moves [`Delta`]s across one boundary. Implementations must not coalesce into full-state
/// transfers (invariant 2).
pub trait Transport {
    /// Transport-specific error type.
    type Error;

    /// Send one delta toward the peer.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying channel fails to accept the delta.
    fn send_delta(&mut self, delta: &Delta) -> Result<(), Self::Error>;

    /// Take the deltas received from the peer since the last call.
    ///
    /// # Errors
    /// Returns [`Self::Error`] if the underlying channel fails while draining.
    fn drain_incoming(&mut self) -> Result<Vec<Delta>, Self::Error>;
}
