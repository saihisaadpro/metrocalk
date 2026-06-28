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

pub mod caps;
pub mod catalog;
pub mod entity_id;
pub mod marketplace;
pub mod merge;
pub mod pipeline;
pub mod producer;
pub mod project;
pub mod registry;
pub mod resolve;
pub mod rules;
pub mod state_machine;
pub mod stdlib;
pub mod taxonomy;
pub mod undo;
pub mod variant;

pub use catalog::{CatalogItem, CatalogSearch, SearchSeam, Source};
pub use entity_id::{EntityId, IdGenerator};
pub use marketplace::{
    CapDecl, LocalCatalog, MarketplaceEntry, MarketplaceIndex, MarketplaceMatch,
};
pub use merge::MergeReport;
pub use pipeline::{CapRole, CapabilityResolver, Engine, FieldValue, Op, PipelineError};
pub use producer::ProducerHook;
pub use project::{ProjectError, FORMAT_VERSION};
pub use registry::{
    ActionMeta, Builder, ComponentMeta, EventMeta, FieldSpec, FieldType, Registry, RegistryError,
};
pub use resolve::{resolve, resolve_local, Match, NextTier, Resolution, Resolved};
pub use rules::{
    propose_mirror, validate_rule, Action, CompareOp, Condition, RuleData, RuleError, RuleId,
};
pub use state_machine::{
    validate_state_machine, StateMachine, StateMachineError, StateMachineId, StateMachineReport,
    Transition, ENTER_STATE_ACTION,
};
pub use taxonomy::{bucket_of, is_standard_category, Category, STD_CATEGORIES};
pub use variant::{Composition, CompositionNode, ResolvedNode, Variant, VariantOp};

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
