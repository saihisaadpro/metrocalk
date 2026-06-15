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

pub mod bridge;
pub mod capscene;
pub mod reveal;

pub use bridge::{apply_edit, project_full, EditIntent, EditTx, ProjectionDelta, ProjectionOp};
pub use capscene::{bind, positions, seed, CapScene, SeedIndex, TRACKS};
pub use reveal::{required_caps, reveal, why_not, Candidate, Context, Rels, Reveal, WhyNot};
