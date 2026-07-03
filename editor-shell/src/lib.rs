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
pub mod cad_import;
pub mod cad_intent;
pub mod capscene;
pub mod compose_ai;
pub mod constraint_intent;
pub mod cosim;
pub mod csg_intent;
pub mod feature_history;
pub mod generate;
pub mod generative;
pub mod metering;
pub mod pdm;
pub mod persist;
pub mod physics_intent;
pub mod play_rules;
pub mod plugin_host;
pub mod pmi;
pub mod pmi_step;
pub mod project;
pub mod reveal;
pub mod sdf_intent;
pub mod transform_solver;
pub mod wallet;

pub use actions::{actions_for, Action, ActionItem};
pub use ai::{apply_ai_patch, AiPatch, PatchOp};
pub use bom::{rollup as bom_rollup, Bom, BomLine};
pub use bridge::{
    apply_edit, enrich_relational, project_entity, project_full, EditIntent, EditTx,
    ProjectionDelta, ProjectionOp, RelSummary,
};
pub use cad_import::{
    changed_count, import_cad, land_import, read_cad, reimport_diff, CadImportError, CadLanding,
    CAD_PART,
};
pub use cad_intent::{import_step, StepImport};
pub use capscene::{
    add_kind, apply_marketplace_entry, bind, describe_create, duplicate_entity, instantiate,
    place_generation_placeholder, place_mesh, positions, remove_entity, seed, CapResolver,
    CapScene, MeshCatalog, SeedIndex, MESH_FIELD, TRACKS,
};
pub use compose_ai::{ComposeAiError, Composer, DemoComposer, RemoteComposer};
pub use constraint_intent::{
    explain_conflict, propose_constraints, sketch_point_meta, solve_and_land, witness_from_doc,
    ConstraintCertificate, ConstraintProposal, SolveLanding, SKETCH_POINT,
};
pub use cosim::{co_simulate, land_cosim, CoSimRun, CoSimSchedule, CoSimStep, FmiSolver};
pub use feature_history::{
    eval_variables, rebuild, rebuild_reproduces, validate_feature_op, validate_history,
    Configuration, Dim, Expr, FeatureError, FeatureHistory, FeatureId, FeatureOp, Rebuilt,
};
pub use generate::{
    FakeGenerator, GenError, GenRequest, MeshGenerator, MeterAction, RemoteGenerator, StubMeter,
    TokenMeter,
};
pub use generative::{
    apply_optimized_design, bake_design, baked_mesh_is_watertight, design_certificate,
    design_component_meta, optimize, parse_spec, place_design_seed, propose_design,
    CandidateOrigin, Design, DesignCandidate, GenerativeRun, GradientSource, LoadSpec, Material,
    Objective, PreciceFmiSolver, RomBeamSolver, Solver, SolverError, SpecError, StructuralResult,
    DESIGN_COMPONENT,
};
pub use metering::{ai_edit_material, buy_marketplace, material_patch, Outcome};
pub use pdm::{
    approval_delta, branch_from, merge_eco, release, state_identity, verify as verify_revision,
    EcoOutcome, PdmError,
};
pub use persist::{Log, Record};
pub use play_rules::{build_recording, PlaySession};
pub use pmi::{
    ai_adjust_tolerance, attach_fcf, fcf_component_meta, fcfs_on, is_cad_feature, read_fcf,
    validate_fcf, Characteristic, Contribution, Contributor, Fcf, Fix, McResult, PmiError, Stackup,
    StackupAnalysis, StackupCertificate, Standard, FCF_COMPONENT,
};
pub use pmi_step::{
    collect_semantic_fcfs, export_step as export_step_pmi, import_step_text, measure_fidelity,
    reimport_with_pmi, scene_with_pmi, FidelityRow, RoundTripFidelity, SemanticFcf,
};
pub use project::{atomic_write, open_into, save, OpenError};
pub use reveal::{required_caps, reveal, why_not, Candidate, Context, Rels, Reveal, WhyNot};
pub use sdf_intent::{bake as bake_sdf, bake_auto as bake_sdf_auto, SdfBakeError};
pub use wallet::Wallet;
