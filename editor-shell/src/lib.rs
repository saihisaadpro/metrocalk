//! `metrocalk-editor-shell` — the Rust side of the M2.6 desktop editor (the M2 convergence).
//!
//! This lib is the **bridge** between the real [`metrocalk_core::Engine`] (the authoritative core —
//! commit pipeline, `World`, and Loro; **no MockCore**) and the M2.5 editor's `ProjectionDelta` and
//! `EditTx` JSON contract carried over the M2.4 transport. The Tauri app (`src-tauri/`: a transparent
//! WebView2 over the wgpu viewport per ADR-008, the M2.2 renderer, picking/camera in Rust) builds on
//! this and lives outside the workspace.
//!
//! Status: M2.6 in progress. The edit round-trip spine ([`bridge`]) is real + tested here; the live
//! shell wiring (viewport, composite, picking, the residual measurements, the M2 gate) is the
//! remaining convergence work — see `progress/M2.md`.

pub mod actions;
pub mod ai;
pub mod blobstore;
pub mod bom;
pub mod bridge;
pub mod capscene;
pub mod compose_ai;
pub mod csg_intent;
pub mod feature_history;
pub mod generate;
pub mod metering;
pub mod pdm;
pub mod persist;
pub mod physics_intent;
pub mod play_rules;
pub mod plugin_host;
pub mod project;
pub mod reveal;
pub mod transform_solver;
pub mod wallet;

pub use actions::{actions_for, Action, ActionItem};
pub use ai::{apply_ai_patch, AiPatch, PatchOp};
pub use bom::{rollup as bom_rollup, Bom, BomLine};
pub use bridge::{
    apply_edit, enrich_relational, project_entity, project_full, EditIntent, EditTx,
    ProjectionDelta, ProjectionOp, RelSummary,
};
pub use capscene::{
    add_kind, apply_marketplace_entry, bind, describe_create, duplicate_entity, instantiate,
    place_generation_placeholder, place_mesh, positions, remove_entity, seed, CapResolver,
    CapScene, MeshCatalog, SeedIndex, MESH_FIELD, TRACKS,
};
pub use compose_ai::{ComposeAiError, Composer, DemoComposer, RemoteComposer};
pub use feature_history::{
    eval_variables, rebuild, rebuild_reproduces, validate_feature_op, validate_history,
    Configuration, Dim, Expr, FeatureError, FeatureHistory, FeatureId, FeatureOp, Rebuilt,
};
pub use generate::{
    FakeGenerator, GenError, GenRequest, MeshGenerator, MeterAction, RemoteGenerator, StubMeter,
    TokenMeter,
};
pub use metering::{ai_edit_material, buy_marketplace, material_patch, Outcome};
pub use pdm::{
    approval_delta, branch_from, merge_eco, release, state_identity, verify as verify_revision,
    EcoOutcome, PdmError,
};
pub use persist::{Log, Record};
pub use play_rules::{build_recording, PlaySession};
pub use project::{atomic_write, open_into, save, OpenError};
pub use reveal::{required_caps, reveal, why_not, Candidate, Context, Rels, Reveal, WhyNot};
pub use wallet::Wallet;
