//! `metrocalk-core` — the deterministic engine core.
//!
//! Builds on the `metrocalk-ecs` wrapper crate (the [`World`](metrocalk_ecs::World) query trait;
//! the one place Flecs/`unsafe` live — ADR-001/006). This crate holds:
//! - the **component metadata registry** ([`registry`]) — JSON-Schema-fed, capability-aware; the
//!   data layer behind the compatibility query and (later) describe-to-create (M1.3, real);
//! - the **standard component library** ([`stdlib`]) — real example components;
//! - the **commit pipeline** ([`pipeline`]) — the single transactional path for all scene
//!   mutations (invariant 3); ECS authoritative, Loro mirrored via deltas (invariant 2);
//! - the **engine-side undo/redo stack** ([`undo`]) — in-memory inverse-op stack (F2);
//! - the **merge-validation layer** ([`merge`]) — detects + repairs 8 invalid-state classes (F1);
//! - **peer-namespaced entity IDs** ([`entity_id`]) — no collision under concurrent create (F3).
//!
//! Native-only — it depends on Flecs through the wrapper; the browser uses the pure-Rust query
//! backend over the Loro projection (ADR-006).

pub mod entity_id;
pub mod merge;
pub mod pipeline;
pub mod producer;
pub mod registry;
pub mod stdlib;
pub mod undo;

pub use entity_id::{EntityId, IdGenerator};
pub use merge::MergeReport;
pub use pipeline::{Engine, FieldValue, Op, PipelineError};
pub use producer::ProducerHook;
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
