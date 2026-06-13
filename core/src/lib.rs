//! `metrocalk-core` — the deterministic engine core.
//!
//! Charter (filled in across M1, nothing here yet but the skeleton + a smoke test):
//! - the semantic-ECS wrapper API over Flecs (M1.2) — the one crate allowed `unsafe`, behind its
//!   own lint config, so Flecs types never leak past it (ADR-001, invariant 5);
//! - the component metadata registry, JSON-Schema-fed (M1.3);
//! - the single transactional ECS to Loro commit pipeline + merge-validation layer (M1-2,
//!   invariant 3) and an engine-side inverse-op undo stack (spike-1 finding F2);
//! - the wgpu renderer (hot path stays Rust-side, invariant 4).
//!
//! Native is authoritative via Flecs; the browser query backend is pure-Rust over the Loro
//! projection (ADR-006), so this crate is native-only — the wasm build never compiles it.

/// Engine identifier — one constant shared by downstream crates, logs, and file headers.
pub const ENGINE_NAME: &str = "metrocalk";

#[cfg(test)]
mod tests {
    use super::ENGINE_NAME;

    #[test]
    fn engine_name_is_stable() {
        // Proves the build/test harness end-to-end; real tests arrive with real code in M1.2+.
        assert_eq!(ENGINE_NAME, "metrocalk");
    }
}
