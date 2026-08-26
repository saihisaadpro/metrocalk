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
pub mod animation_bindings;
pub mod animation_clip_intent;
pub mod animation_graph_intent;
pub mod animation_intent;
pub mod asset_lab;
pub mod blobstore;
pub mod bom;
pub mod bridge;
pub mod cad_import;
pub mod cad_intent;
pub mod capscene;
pub mod cinema_intent;
pub mod companion_intent;
pub mod compose_ai;
pub mod condition_intent;
pub mod constraint_intent;
pub mod cosim;
pub mod csg_intent;
pub mod feature_history;
pub mod formats;
pub mod generate;
pub mod generative;
pub mod kinematics;
pub mod match_cook;
pub mod metering;
pub mod pdm;
pub mod persist;
pub mod physics_intent;
pub mod pipe_forge;
pub mod play_rules;
pub mod plugin_host;
pub mod pmi;
pub mod pmi_step;
pub mod project;
pub mod reimport;
pub mod reveal;
pub mod role_intent;
pub mod scene_capture;
pub mod sdf_intent;
pub mod semantic;
pub mod shape_forge;
pub mod step_export;
pub mod terrain_intent;
pub mod transform_solver;
pub mod vfx_intent;
pub mod wallet;

pub use actions::{actions_for, Action, ActionItem};
pub use ai::{apply_ai_patch, AiPatch, PatchOp};
pub use animation_bindings::{
    built_in_animation_binding_descriptors, standard_animation_binding_registry,
    AnimationBindingRegistry,
};
pub use animation_clip_intent::{
    authored_animation_clip_instance_revision, delete_animation_clip_instance,
    load_animation_clip_instances, save_animation_clip_instance,
    validate_animation_clip_instance_document, AnimationClipInstanceDocument,
    AnimationClipInstanceIntentError, AnimationClipInstanceIssue,
    AnimationClipInstanceIssueSeverity, AnimationClipInstanceLoad, ANIMATION_CLIP_INSTANCE,
    ANIMATION_CLIP_INSTANCE_SCHEMA_VERSION,
};
pub use animation_graph_intent::{
    adapt_animation_graph_document, authored_animation_graph_revision, delete_animation_graph,
    load_animation_graph, save_animation_graph, validate_animation_graph_document,
    AdaptedAnimationGraph, AnimationGraphCondition, AnimationGraphConditionOperator,
    AnimationGraphCurve, AnimationGraphDiagnostic, AnimationGraphDiagnosticSeverity,
    AnimationGraphDocument, AnimationGraphEdge, AnimationGraphEdgeProvenance,
    AnimationGraphIntentError, AnimationGraphInterruption, AnimationGraphLoad, AnimationGraphNode,
    AnimationGraphNodeKind, AnimationGraphParameter, AnimationGraphParameterKind,
    AnimationGraphParameterRoute, AnimationGraphPosition, AnimationGraphState,
    AnimationGraphStateMachine, AnimationGraphTransition, AnimationGraphValue, ANIMATION_GRAPH,
    ANIMATION_GRAPH_SCHEMA_VERSION,
};
pub use animation_intent::{
    add_event as add_animation_event, add_marker as add_animation_marker, author_spin_ops,
    authored_animation_revision, delete_event as delete_animation_event,
    delete_key as delete_animation_key, delete_keys as delete_animation_keys,
    delete_marker as delete_animation_marker, key_property as key_animation_property,
    load_animation_document, property_catalog as animation_property_catalog,
    set_track_enabled as set_animation_track_enabled,
    set_track_interpolation as set_animation_interpolation,
    set_track_locked as set_animation_track_locked, update_keys as update_animation_keys,
    AnimatableProperty, AnimationDocument, AnimationIntentError, AnimationKeyDelete,
    AnimationKeyReceipt, AnimationKeyUpdate, AnimationStorageIssue, ANIMATION_TRACK,
    DEFAULT_TICKS_PER_SECOND, MAIN_SEQUENCE_ID,
};
pub use asset_lab::{
    audit_asset, cleanup_asset, optimize_asset, AssetAuditReport, AssetChangeReport, AssetLabError,
    AuditOptions, Availability, AvailabilityState, BoundsReport, ByteEstimate, CapabilityAudit,
    CleanupChanges, CleanupConfig, CleanupResult, LodCandidateReport, MaterialAudit, NormalAudit,
    NormalRepairMode, OptimizationConfig, OptimizationPreset, OptimizationResult, TextureAudit,
    TextureDescriptor, UvAudit, UvGenerationMode, UvOverlapSignal, UvOverlapState,
    DEFAULT_TOPOLOGY_WELD_THRESHOLD, DEFAULT_UV_PAIR_BUDGET,
};
pub use bom::{rollup as bom_rollup, Bom, BomLine};
pub use bridge::{
    apply_edit, enrich_relational, project_entity, project_full, EditIntent, EditTx,
    ProjectionDelta, ProjectionOp, RelSummary,
};
pub use cad_import::{
    active_cad_source_members, active_cad_source_roots, bake_basis_into_mesh, basis_is_rigid,
    cad_source_content_hash, cad_source_owner_ops, cad_source_root_ops, cad_source_selection,
    cad_source_selection_op, cad_z_up_to_editor_mesh, cad_z_up_to_editor_transform, changed_count,
    import_cad, is_cad_file, land_import, land_import_from_source, load_persisted_cad_meshes,
    load_persisted_cad_meshes_for, persist_cad_mesh, read_cad, read_cad_with_companion,
    reimport_diff, CadImportError, CadLanding, CadSourceIdentity, CAD_IMPORT_OWNER,
    CAD_IMPORT_SOURCE, CAD_PART,
};
pub use cad_intent::{import_step, StepImport};
pub use capscene::{
    add_kind, apply_marketplace_entry, bind, describe_create, duplicate_entity, instantiate,
    place_generation_placeholder, place_mesh, positions, remove_entity, seed, CapResolver,
    CapScene, MeshCatalog, SeedIndex, MESH_FIELD, TRACKS,
};
pub use cinema_intent::{
    add_shot_ops, cutscene_of, describe_shot, framing_catalog, move_shot_ops, remove_shot_ops,
    reply_for as cinema_reply, reply_with_names as cinema_reply_named, set_mood_ops,
    set_shot_framing_ops, set_shot_seconds_ops, shot_specs, CinemaError, CinemaReply,
    FramingCatalog, FramingEdit, FramingOption, ShotRow, ShotSpec, CINEMA_COMPONENT,
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
pub use formats::{
    explain_unsupported, format_catalog, import_extensions, spec_for_extension, Carries, Direction,
    Fidelity, FormatSpec,
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
pub use kinematics::{
    encode_track, joint_of, joint_of_with_component, joint_pose, joint_source, parse_track,
    set_joint_ops, track_end, track_value, try_author_joint_tracks_ops, try_set_joint_ops,
    validate_joint_spec, validate_joint_time, validate_joint_value, Joint, JointTrackAuthoring,
    JointTrackKey, JointValidationResult, JOINT, JOINT_TRACK, KINEMATIC_JOINT, LEGACY_JOINT,
};
pub use metering::{ai_edit_material, buy_marketplace, material_patch, Outcome};
pub use step_export::{
    export_parts as export_step_parts, scene_from_parts, StepExportError, StepExportReport,
    StepPart, MAX_EXPORT_TRIANGLES,
};
pub use vfx_intent::{
    add_effect_ops, describe_layer, effect_specs, remove_effect_ops, stack_of, vfx_reply,
    EffectSpec, Trigger as VfxTrigger, VfxError, VfxReply, VFX_COMPONENT,
};
// The project-owned triangle-mesh type (what `bake_basis_into_mesh` takes/returns and `persist_cad_mesh`
// stores) — re-exported so the app shell can NAME it (`RegOut`) without a direct `metrocalk-csg` dep.
pub use condition_intent::{
    add_clause_ops, build_clause, clauses_of, condition_specs, describe_clause,
    expand_subject_rules, remove_clause_ops, ClauseRequest, Clauses, ConditionError, ConditionSpec,
    CONDITION_COMPONENT,
};
pub use metrocalk_csg::TriMesh;
pub use pdm::{
    approval_delta, branch_from, merge_eco, release, state_identity, verify as verify_revision,
    EcoOutcome, PdmError,
};
pub use persist::{Log, Record};
pub use pipe_forge::{
    bake_pipe, decode_pipe_artifact, land_pipe_asset, rebake_pipe_in_place, replace_pipe_asset,
    PipeBakeReport, PipeBuild, PipeFittingKind, PipeFittingPlacement, PipeForgeError,
    PipeForgeOptions, PipeForgeStatus, PipeKit, PipeQuality, PipeRecipe, PipeRouteEdge,
    PipeRouteGraph, PipeRouteHandle, PipeRouteNode, PipeToolSession, UserFittingCatalogEntry,
    PIPE_ARTIFACT_MAGIC, PIPE_RECIPE_VERSION,
};
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
pub use reimport::{
    capture_overrides, match_scene_against, plan_rebind, rebind_ops, reimport_identity_of,
    reimport_over_scene, set_reimport_id_ops, Adjudication, OrphanedOverride, OverrideSet,
    RebindOutcome, ReimportDiffEntry, ReimportSession, REIMPORT_ID,
};
pub use reveal::{required_caps, reveal, why_not, Candidate, Context, Rels, Reveal, WhyNot};
pub use role_intent::{
    assign_role, clear_role, find_score, role_of, role_specs, roster as role_roster, BlockedInfo,
    CompanionRow, HealthInfo, RoleError, RoleOutcome, RoleReply, RoleRow, RoleSpec, RoleStatusInfo,
    DEFAULT_TOUCH_RADIUS, DEFEAT_RULE_ID, PICKUP_RULE_ID, ROLE_COMPONENT, SCORE_NAME,
};
pub use scene_capture::{capture_scene, CapturedMesh, SceneCaptureError};
pub use sdf_intent::{bake as bake_sdf, bake_auto as bake_sdf_auto, SdfBakeError};
pub use shape_forge::{
    bake_meld, bake_mesh_result, bake_shape, build_shape, combine_kind, combine_label,
    combine_meshes, decode_shape_artifact, land_combined_asset, land_shape_asset, meld_field,
    placeholder_cube, rebuild_from_source, recentre, replace_shape_asset, shape_material,
    shape_specs, spawn_spot, transform_mesh_world, validate_param_keys, ShapeBuild, ShapeError,
    ShapeLanding, ShapeRecipe, ShapeReply, ShapeSpec, SHAPE_COMPONENT,
};
// M15.11 (ADR-081) — the AI semantic pass landed on the substrate: features as validated typed component
// data (child entities, one undoable commit), the confidence-gated adjudication hold, semantic search,
// the defeaturing recommender, and the kernel-verified feature-informed collider planner.
pub use semantic::{
    all_cad_parts, class_ops, collider_ops, defeature_ops, feature_component_meta, feature_fields,
    parse_semantic, part_class_component_meta, plan_defeature, plan_feature_collider,
    plan_semantic_land, resolve_semantic, semantic_act_ops, validate_fields, ColliderPlan,
    DefeaturePlan, DefeatureRow, HeldRecognition, SemanticIntent, SemanticLanding, SemanticMatches,
    SemanticNoun, SemanticVerb, AUTO_COMMIT_CONFIDENCE, CAD_FEATURE, PART_CLASS,
};
pub use wallet::Wallet;
