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
pub mod bridge;
pub mod capscene;
pub mod generate;
pub mod metering;
pub mod persist;
pub mod reveal;
pub mod wallet;

pub use actions::{actions_for, Action, ActionItem};
pub use ai::{apply_ai_patch, AiPatch, PatchOp};
pub use bridge::{
    apply_edit, project_entity, project_full, EditIntent, EditTx, ProjectionDelta, ProjectionOp,
};
pub use capscene::{
    apply_marketplace_entry, bind, describe_create, duplicate_entity, instantiate,
    place_generation_placeholder, place_mesh, positions, remove_entity, seed, CapScene,
    MeshCatalog, SeedIndex, MESH_FIELD, TRACKS,
};
pub use generate::{
    FakeGenerator, GenError, GenRequest, MeshGenerator, MeterAction, RemoteGenerator, StubMeter,
    TokenMeter,
};
pub use metering::{ai_edit_rustier, buy_marketplace, rustier_patch, Outcome};
pub use persist::{Log, Record};
pub use reveal::{required_caps, reveal, why_not, Candidate, Context, Rels, Reveal, WhyNot};
pub use wallet::Wallet;
