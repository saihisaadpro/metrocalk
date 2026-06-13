//! `metrocalk-core` — the deterministic engine core.
//!
//! Builds on the `metrocalk-ecs` wrapper crate (the [`World`](metrocalk_ecs::World) query trait;
//! the one place Flecs/`unsafe` live — ADR-001/006). This crate holds:
//! - the **component metadata registry** ([`registry`]) — JSON-Schema-fed, capability-aware; the
//!   data layer behind the compatibility query and (later) describe-to-create (M1.3, real);
//! - the **standard component library** ([`stdlib`]) — real example components;
//! - (later) the single transactional ECS↔Loro commit pipeline + merge-validation + engine-side
//!   undo stack (M1–2, invariant 3) and the wgpu renderer (hot path stays Rust-side, invariant 4).
//!
//! Native-only — it depends on Flecs through the wrapper; the browser uses the pure-Rust query
//! backend over the Loro projection (ADR-006).

pub mod registry;
pub mod stdlib;

pub use registry::{Builder, ComponentMeta, FieldSpec, FieldType, Registry, RegistryError};

/// Engine identifier — one constant shared by downstream crates, logs, and file headers.
pub const ENGINE_NAME: &str = "metrocalk";

#[cfg(test)]
mod tests {
    use super::ENGINE_NAME;

    #[test]
    fn engine_name_is_stable() {
        assert_eq!(ENGINE_NAME, "metrocalk");
    }
}
