//! Deterministic animation pose/property graphs.
//!
//! Authored graphs are versioned data. Compilation resolves sequence references, validates the
//! acyclic pose DAG independently from the cyclic state machine, and produces an immutable plan.
//! Runtime instances own parameters, clocks, reusable buffers and bounded event/trace output only.

mod authoring;
mod compile;
mod runtime;

pub use authoring::*;
pub use compile::*;
pub use runtime::*;

/// Current authored animation-graph schema version.
pub const ANIMATION_GRAPH_SCHEMA_VERSION: u32 = 2;

#[cfg(test)]
mod tests;
