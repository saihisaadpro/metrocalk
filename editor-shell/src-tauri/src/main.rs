#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! M2.6 + M3.1 desktop editor shell — the convergence, now with binding-by-intent live.
//!
//! A transparent WebView2 (the editor UI) over a native wgpu viewport (M2.2 instanced scene) on one
//! HWND (ADR-008, single-window, OS-composited). The **real** `/core` Engine drives both: it lives on
//! a dedicated thread (Flecs is `!Send`, so it can't sit in Tauri's `Send+Sync` managed state —
//! M2.1's finding), fed editor `EditTx`s over `invoke` and pushing `ProjectionDelta`s back over a
//! Tauri `Channel` (the desktop binding of the M2.4 transport contract). Camera + picking stay in Rust
//! (invariant 4); only the committed delta crosses (inv. 2).
//!
//! M3.1: the scene now carries the stdlib capability web (HealthBar requires Health, …). Clicking an
//! entity runs the reveal engine (`editor-shell::reveal`, ADR-011) on the **engine's own world** and
//! returns ranked compatible targets + every "no" explained; a candidate click binds in one undoable
//! transaction (north-star test #1).

mod ibl;
mod render;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use metrocalk_assets::{
    detect, AssetId, AssetStore, Detected, FbxImporter, GltfImporter, ImageImporter, KtxImporter,
    MeshGpu, MeshSource, ObjImporter,
};
use metrocalk_core::catalog::{CatalogItem, CatalogSearch};
use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_economy::{HoldId, SandboxProvider, GENERATE_TOKENS};
use metrocalk_ecs::{Entity, FlecsWorld, World};
use metrocalk_editor_shell::compose_ai::Composer;
use metrocalk_editor_shell::generate::{GenRequest, MeshGenerator};
use metrocalk_editor_shell::physics_intent::{self, MeshMetrics, PhysicsWarning};
use metrocalk_editor_shell::project as mtk_project;
use metrocalk_editor_shell::reveal::{required_caps, reveal, why_not_with_required, Context};
use metrocalk_editor_shell::transform_solver::{
    Constraint, ConstraintIntent, SnapKind, SnapTarget,
};
use metrocalk_editor_shell::{
    actions_for, ai_edit_material, apply_ai_patch, apply_edit, buy_marketplace, capscene,
    enrich_relational, project_entity, project_full, transform_solver, ActionItem, AiPatch,
    CapScene, EditIntent, EditTx, Log, MeshCatalog, Outcome, PatchOp, ProjectionDelta,
    ProjectionOp, Record, Wallet,
};
use metrocalk_gizmo::{
    Gizmo, GizmoMode, GizmoPivot, GizmoSpace, Handle, Ray, Transform as GizmoTransform,
};
use metrocalk_interchange::{Interchange, UrdfInterchange, UsdInterchange};
use metrocalk_physics::{
    explain_contact, BodyDesc, BodyHandle, BodyKind, ColliderDesc, ColliderShape, Contact,
    Fidelity, Physics, RapierPhysics, Recording, Replay,
};
use render::{Instance, SceneState, Shared};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

// C10: a real first-run must open onto a SMALL, navigable scene — never the 5,000-entity stress wall. The
// seed still forces entity 0 to a HealthBar near the origin (+ a few Health providers), so bind-by-intent
// (north-star #1) is demonstrable out of the box. The 5k stress fixture stays one `MTK_SCENE_N=5000` away
// (the perf/acceptance/base/reload e2e configs pin it; the dev MockCore already does this via `sampleScene`).
const SAMPLE_N: usize = 16;

/// Send a projection delta to the WebView, LOGGING a send failure instead of swallowing it (audit F1).
/// A dead channel (webview closed mid-op) used to drop every confirm/undo/create/bind delta silently →
/// the UI desynced from the engine (stuck-pending edits, dead-looking undo, ghost entities) with zero
/// trace. `$ch` is the bound `&Channel` from `if let Some(ch) = &channel`.
macro_rules! send_proj {
    ($ch:expr, $d:expr) => {
        if let Err(e) = $ch.send($d) {
            eprintln!("[shell] projection channel send failed (UI may be out of sync): {e}");
        }
    };
}

/// M14.2 (ADR-058) — a full re-projection ENRICHED with the live relational summary (the C6 closure): each
/// entity carries `requires/provides/bound/needsBinding` + a `kind`, keyed off the real `(Requires/Provides,
/// cap)` ECS pairs + `bindings()`, so the hierarchy/Requirers surface the scene's real binding/requirer truth
/// against the **live `/core`** (retiring the brittle `HealthBar` name filter). A read/render projection
/// computed off the discrete projection path (connect / undo / open / sim-restart) — NEVER per-frame (inv. 4),
/// and never authored into the doc (zero determinism impact, like the M11.3 lights / M8 sim projection).
fn proj_full(engine: &Engine<FlecsWorld>, scene: &CapScene) -> ProjectionDelta {
    let mut d = project_full(engine);
    enrich_relational(&mut d, engine, scene.rels, &scene.cap_name);
    d
}

/// The checked-in demo assets — **embedded** so the packaged app has no runtime file dependency, while
/// the importer still runs on real glTF bytes (provenance: `assets/examples/gen_fixtures.rs`).
const HEALTHBAR_GLB: &[u8] = include_bytes!("../../assets/healthbar.glb");
const PROP_GLB: &[u8] = include_bytes!("../../assets/prop.glb");
/// The M8.2 physics test mesh — a ball; a spawned RigidBody renders as this (see `spawn_physics_body`).
const SPHERE_GLB: &[u8] = include_bytes!("../../assets/sphere.glb");
/// The spawned ball's collider radius (world meters). The render mesh is normalized separately.
const BALL_RADIUS: f32 = 0.45;

/// The runtime asset state the engine thread owns — the import store's results turned into render data,
/// loaded once at startup. `catalog` maps a resolved component kind → its asset handle (describe-to-
/// create); `handle_to_slot` + `scales` turn an entity's `MeshRenderer.mesh` handle into a viewport
/// render slot + a normalized scale; `meshes` is the slot-indexed packed geometry handed to the
/// viewport. The asset *store* itself is dropped after packing — nothing here borrows from it.
struct AssetsRuntime {
    catalog: MeshCatalog,
    /// Logical asset name (a marketplace entry's `asset` field, e.g. `"prop"`) → content-addressed
    /// handle — how a marketplace entry's mesh is resolved at apply time.
    asset_by_name: HashMap<String, String>,
    handle_to_slot: HashMap<String, usize>,
    scales: Vec<f32>,
    meshes: Vec<MeshGpu>,
    /// The ball mesh handle (M8.2) — a spawned physics body renders as this.
    sphere: String,
    /// M11.5 (ADR-044) — per-asset provenance, keyed by the content-address handle (riding the store, not
    /// rebuilding it). Populated for IMPORTED assets (built-in catalog meshes have none). Carries the
    /// identity record + the perceptual hash used for near-duplicate hints.
    provenance: HashMap<String, metrocalk_assets::Provenance>,
}

/// Re-import ONE persisted asset blob into the store on boot, routed by MAGIC — mirrors `import_any`'s
/// routing (ADR-040) so a handle saved in the `.mtk` re-resolves after reload. Returns whether a mesh
/// asset was registered. **FBX + KTX2 are included** because the binary builds `metrocalk-assets` with the
/// `fbx`+`ktx2` features (Cargo.toml): omitting them silently dropped a reopened FBX/KTX2 import to the
/// placeholder — the M11.1 reload hole that contradicted ADR-040's "survives reload". The handle is the
/// content address of the SOURCE bytes (`AssetStore::import` → `AssetId::of_bytes`), the same value the
/// live import command saved, so the doc's `MeshRenderer.mesh` re-resolves. Audio/unrecognized blobs are
/// not mesh assets → `false`. The match is exhaustive (no `_`) so a future `Detected` variant must be
/// triaged here, not silently dropped again.
fn reimport_persisted_blob(store: &mut AssetStore, gltf: &GltfImporter, bytes: &[u8]) -> bool {
    let stored = match detect(bytes) {
        Some(Detected::Gltf) => store.import(gltf, bytes),
        Some(Detected::Obj) => store.import(&ObjImporter::new(), bytes),
        Some(Detected::Image) => store.import(&ImageImporter::new(), bytes),
        Some(Detected::Fbx) => store.import(&FbxImporter::new(), bytes),
        Some(Detected::Ktx2) => store.import(&KtxImporter::new(), bytes),
        Some(Detected::Audio) | None => return false, // not a mesh asset
    };
    stored.is_ok()
}

/// Import the embedded fixtures into a content-addressed store, build the kind→handle catalog, and pack
/// each asset to GPU-ready geometry + a normalized render scale. Import is the one-shot heavy op
/// (measured here, never frame-budget-gated). Slot order is per-run (the handle in the doc is the
/// stable id; the slot is a transient render index), so a reload re-resolves handles correctly.
fn load_assets() -> AssetsRuntime {
    let importer = GltfImporter::new();
    let mut store = AssetStore::new();
    let t0 = std::time::Instant::now();
    let healthbar = store
        .import(&importer, HEALTHBAR_GLB)
        .expect("import healthbar.glb");
    let prop = store.import(&importer, PROP_GLB).expect("import prop.glb");
    let sphere = store
        .import(&importer, SPHERE_GLB)
        .expect("import sphere.glb");

    // M11.1 — re-import any PERSISTED asset bytes (generated meshes / user File→Import) from the
    // content-addressed sidecar dir, so a handle saved in the `.mtk` doc re-resolves after reload (closes
    // the M6 residual — a generated mesh no longer dangles on reopen). Each blob round-trips through the
    // MAGIC router, content-addressed → the same handle; the meshes loop below packs it like any fixture.
    // A corrupt blob is skipped by `load_all` (content/name mismatch), never trusted.
    let blob_dir = sidecar("metrocalk-assets");
    let mut blobs_loaded = 0usize;
    for (_id, bytes) in metrocalk_editor_shell::blobstore::load_all(&blob_dir) {
        if reimport_persisted_blob(&mut store, &importer, &bytes) {
            blobs_loaded += 1;
        }
    }
    let import_ms = t0.elapsed().as_secs_f64() * 1000.0;
    if blobs_loaded > 0 {
        eprintln!("[shell] re-imported {blobs_loaded} persisted asset blob(s) from {blob_dir:?}");
    }

    // kind → asset handle: a resolved HealthBar renders as the bar mesh; a resolved MeshRenderer as the
    // prop. A kind absent here has no mesh → the honest placeholder-cube fallback.
    let catalog: MeshCatalog = [
        ("HealthBar", healthbar.as_str()),
        ("MeshRenderer", prop.as_str()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();

    // Logical asset name → handle, for marketplace entries (their `asset` field is a logical name).
    let asset_by_name: HashMap<String, String> =
        [("healthbar", healthbar.as_str()), ("prop", prop.as_str())]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

    // M11.5 (ADR-044) — label the three built-in fixtures so a boot-time provenance record reads honestly;
    // anything else in the store is a user import restored from the content-addressed sidecar (its original
    // file name isn't carried by the blob — a provenance sidecar is the named seam).
    let builtins: HashMap<&str, &str> = [
        (healthbar.as_str(), "healthbar.glb (built-in)"),
        (prop.as_str(), "prop.glb (built-in)"),
        (sphere.as_str(), "sphere.glb (built-in)"),
    ]
    .into_iter()
    .collect();

    let mut meshes = Vec::new();
    let mut handle_to_slot = HashMap::new();
    let mut scales = Vec::new();
    let mut provenance: HashMap<String, metrocalk_assets::Provenance> = HashMap::new();
    for (id, asset) in store.iter() {
        let slot = meshes.len();
        meshes.push(MeshGpu::from_asset(asset));
        let ext = asset.bounds().max_extent();
        scales.push(if ext > 0.0 { 0.9 / ext } else { 1.0 });
        handle_to_slot.insert(id.as_str().to_string(), slot);
        // Record provenance for every boot-loaded asset (recomputing the perceptual hash from its primary
        // texture) so near-duplicate hints + the inspector field survive a reload, not just in-session imports.
        let phash = asset
            .textures
            .first()
            .map_or(0, metrocalk_assets::perceptual_hash);
        let source = builtins
            .get(id.as_str())
            .copied()
            .unwrap_or("(restored on reload)");
        provenance.insert(
            id.as_str().to_string(),
            metrocalk_assets::Provenance::imported(source, id.as_str().to_string(), phash),
        );
    }
    // M15.7 (ADR-077) — restore the DERIVED CAD render meshes (the `mtkcad:` handles a saved doc's
    // MeshRenderer fields carry) from the cad-mesh sidecar, so a reopened project renders its imported
    // CAD parts instead of silently degrading them to placeholder cubes. Boot cost is deserialize + GPU
    // pack of the ~dozens of UNIQUE meshes — never a re-parse of the multi-hundred-MB source container.
    let cad_restored =
        metrocalk_editor_shell::load_persisted_cad_meshes(&sidecar("metrocalk-cad-meshes"));
    if !cad_restored.is_empty() {
        eprintln!(
            "[shell] restored {} persisted CAD mesh(es) from the cad-mesh sidecar",
            cad_restored.len()
        );
    }
    for (handle, asset) in cad_restored {
        let slot = meshes.len();
        meshes.push(MeshGpu::from_asset(&asset));
        scales.push(1.0);
        handle_to_slot.insert(handle, slot);
    }
    eprintln!(
        "[shell] imported {} mesh assets ({} verts total) in {import_ms:.3} ms (one-shot)",
        meshes.len(),
        meshes.iter().map(MeshGpu::vertex_count).sum::<usize>()
    );
    AssetsRuntime {
        catalog,
        asset_by_name,
        handle_to_slot,
        scales,
        meshes,
        sphere: sphere.as_str().to_string(),
        provenance,
    }
}

/// A ranked compatible target the selection can bind to (north-star test #1).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Candidate {
    id: String,
    name: String,
    distance: f32,
    affinity: u32,
}

/// An incompatible target the UI greys, with the registry-derived reason ("every 'no' explained").
#[derive(Serialize, Clone)]
struct Greyed {
    id: String,
    name: String,
    reason: String,
}

/// An existing outgoing binding of the selection (so the panel shows "tracking …" after a bind / reload).
#[derive(Serialize, Clone)]
struct Bound {
    id: String,
    name: String,
    kind: String,
}

/// The reveal result handed to the UI for a selected entity.
#[derive(Serialize, Clone, Default)]
struct RevealResponse {
    required: Vec<String>,
    compatible: Vec<Candidate>,
    greyed: Vec<Greyed>,
    bound: Vec<Bound>,
}

/// The describe-to-create result: the created entity + kind, which tier it came from, and — for a
/// marketplace hit — the inert economy seam (token price). On no match anywhere, the generate `seam`.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct DescribeResponse {
    created: Option<String>,
    kind: Option<String>,
    /// `"local"` or `"marketplace"` — which tier resolved it (the happy-path source).
    source: Option<String>,
    /// Token price of a marketplace entry — **actively charged** to the user's wallet when the entry is
    /// bought in the describe flow (M7: debit + ~70% accrued to the creator). No real money moves
    /// (ADR-004/018); shown so the UI reports the cost + the remaining balance.
    price: Option<u32>,
    /// The seam tier when nothing matched anywhere (`"generate"`) — a documented stub.
    seam: Option<String>,
    /// The user's token balance after a marketplace buy (M7), for the wallet UI.
    balance: Option<u32>,
}

/// M9.4 — one ranked snap candidate for the UI/E2E: the snap-graph node + the explained "why this"
/// (the reveal/rank/explain pattern applied to space, ranked by the shared ADR-011 `intent_order`).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SnapHit {
    id: String,
    kind: String,
    x: f32,
    y: f32,
    z: f32,
    distance: f32,
    why: String,
}

/// M9.4 — the outcome of applying a constraint / placement sentence: `ok` + an explained `reason` (every
/// "no" explained, ADR-016) + the compiled `intents` (the editable-before-commit list, for a sentence).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct SolveResult {
    ok: bool,
    reason: Option<String>,
    intents: Vec<String>,
}

/// M10.3 (ADR-033) — the project document state the File menu reads: the current `.mtk` path (or `null`
/// for an untitled project), whether there are **unsaved changes** (the guard's signal), the recent
/// projects, and an explained `error` from the last file op (open/save) — never a crash. Mirrors the
/// React `ProjectInfo`.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ProjectInfoResp {
    path: Option<String>,
    dirty: bool,
    recents: Vec<String>,
    error: Option<String>,
}

/// M10.4 (ADR-034) — Play-mode state for the editor's runtime controls: `playing` = the scene is
/// running (Play entered, not yet Stopped); `paused` = running but the sim is frozen. Both false ⇒
/// Stopped (authoring). Mirrors the React `PlayInfo`.
#[derive(Serialize, Clone, Copy, Default)]
#[serde(rename_all = "camelCase")]
struct PlayInfo {
    playing: bool,
    paused: bool,
}

/// Commands to the engine thread (which owns the `!Send` Engine).
enum EngineCmd {
    Connect(Channel<ProjectionDelta>),
    Edit(EditTx),
    /// Undo the last transaction; reply whether anything was actually reverted (so the UI can be honest —
    /// "undo" vs "nothing to undo" — instead of always claiming a revert on an empty history).
    Undo {
        reply: mpsc::Sender<bool>,
    },
    /// Compute the reveal for a selected entity and reply (request/response — a read).
    Reveal {
        id: String,
        reply: Sender<RevealResponse>,
    },
    /// Bind the selection to a chosen compatible target (one undoable transaction).
    Bind {
        from: String,
        to: String,
    },
    /// Describe-to-create (M3.2): resolve a free-text query + instantiate the top local match.
    Describe {
        query: String,
        reply: Sender<DescribeResponse>,
    },
    /// The action model for an entity (M3.3) — valid actions + every-"no"-explained (a read).
    Actions {
        id: String,
        reply: Sender<Vec<ActionItem>>,
    },
    /// Remove an entity + its edges (M3.3) — one undoable transaction.
    Remove {
        id: String,
    },
    /// Duplicate an entity (M3.3) — one undoable transaction; replies the new id.
    Duplicate {
        id: String,
        reply: Sender<Option<String>>,
    },
    /// Entity details for the hover tooltip (M3.3) — a read.
    Details {
        id: String,
        reply: Sender<Option<EntityDetails>>,
    },
    /// M11.5 (ADR-044) — the selected entity's asset provenance (identity/AI-flag/near-dup) — a read.
    AssetProvenance {
        id: String,
        reply: Sender<Option<ProvenanceInfo>>,
    },
    /// M12.1 (ADR-045) — list all authored rules (the editor Rule list) — a read.
    ListRules {
        reply: Sender<Vec<RuleSummary>>,
    },
    /// M12.1 (ADR-045) — author (or replace, if `id` is given) a rule: registry-validate (Blocked+
    /// explained), commit one undoable `SetRule`, and reply the new id + the offered mirror rule.
    AuthorRule {
        rule: metrocalk_core::RuleData,
        id: Option<String>,
        reply: Sender<AuthorRuleResult>,
    },
    /// M12.1 (ADR-045) — remove a rule (one undoable `RemoveRule`). Replies success.
    DeleteRule {
        id: String,
        reply: Sender<bool>,
    },
    /// M12.2 (ADR-046) — list all authored state machines (the editor state-graph view) — a read. Returns
    /// each machine in full (states + transitions, so the React Flow graph can render) + its live current
    /// state (the M12.5 seam).
    ListStateMachines {
        reply: Sender<Vec<StateMachineInfo>>,
    },
    /// M12.2 (ADR-046) — author (or replace, if `id` is given) a state machine: validate (Blocked +
    /// explained, no-dangling), commit one undoable `SetStateMachine`, and reply the new id + the
    /// unreachable-states warning.
    AuthorStateMachine {
        sm: metrocalk_core::StateMachine,
        id: Option<String>,
        reply: Sender<AuthorStateMachineResult>,
    },
    /// M12.2 (ADR-046) — remove a state machine (one undoable `RemoveStateMachine`). Replies success.
    DeleteStateMachine {
        id: String,
        reply: Sender<bool>,
    },
    /// M12.3 (ADR-047) — run a sandboxed WASM plugin `name` with `input` (the honest-ceiling escape): the
    /// plugin computes an effect that lands as an **undoable transaction** through the commit pipeline (or is
    /// rejected/contained). Echoes the resulting projection delta to the viewport + persists a replay record.
    RunPlugin {
        name: String,
        input: String,
        reply: Sender<RunPluginResult>,
    },
    /// M12.4 (ADR-048) — turn a natural-language `sentence` into a **reviewable** [`Composition`] PROPOSAL
    /// (the in-app AI compose seam): the composer proposes; the engine **validates** it against the live
    /// scene (so even the preview is pre-checked) and replies the JSON composition + an explained-if-not
    /// reason. Nothing is applied — the user reviews, then calls `Compose`. `target` = the selected entity.
    ProposeComposition {
        sentence: String,
        target: Option<String>,
        reply: Sender<ComposeProposal>,
    },
    /// M12.4 (ADR-048) — apply a reviewed `composition` (the validated op-set: SetField / AuthorRule /
    /// AuthorStateMachine) through the ONE commit pipeline as a **single undoable transaction**, or reject it
    /// whole with a plain-language reason (nothing applied). The SAME validated path a human / plugin uses —
    /// the AI is never a raw mutation. Echoes the projection + persists a `Compose` replay record on success.
    Compose {
        composition: metrocalk_core::compose::Composition,
        reply: Sender<ComposeResult>,
    },
    // ── M12.5 (ADR-049) Rules in Play + the live truth-state debugger ──────────────────────────────────
    /// Fire a live gameplay **event** into the running Rules (only in Play) — the When-channel: the event is
    /// recorded into the Play recording + one tick advances, so a later scrub replays it deterministically
    /// (the M8.4 input channel for logic). `selected` is the clicked entity, so the reply carries its fresh
    /// truth-state. A render/runtime **projection** — never the ECS/Loro doc (ADR-021/034).
    FireRuleEvent {
        event: String,
        subject: Option<String>,
        selected: Option<String>,
        reply: Sender<RuleDebugInfo>,
    },
    /// The **live truth-state** for the clicked entity + the decision history (the "debug by looking" read,
    /// test #5 box 3) — a non-mutating projection over the runtime state. `id = None` ⇒ history only.
    RuleDebug {
        id: Option<String>,
        reply: Sender<RuleDebugInfo>,
    },
    /// **Scrub** the decision history to `frame` over the M8.4 replay channel (rewind = rebuild-from-recording,
    /// then replay forward — deterministic-by-rebuild) and reply the truth-state at that frame (test #5 box 4).
    RuleScrub {
        frame: u64,
        selected: Option<String>,
        reply: Sender<RuleDebugInfo>,
    },
    // ── M10.6 scene-authoring verbs (ADR-036) — each one undoable transaction over the Movable Tree +
    // override pipeline. reparent reuses `ReparentPart`; delete=deactivate is distinct from `Remove`. ──
    /// Create an empty named entity at a position → reply its id (selected by the caller).
    CreateEntity {
        x: f32,
        y: f32,
        z: f32,
        name: String,
        reply: Sender<Option<String>>,
    },
    /// M11.3 — author a Light entity (Directional/Point/Spot), one undoable commit. Replies its id.
    AddLight {
        kind: String,
        pos: [f32; 3],
        color: [f32; 3],
        intensity: f32,
        reply: Sender<Option<String>>,
    },
    /// M11.3 — a NON-MUTATING lighting read for the acceptance gate (a stable signal, like `PartDebug`):
    /// (count of AUTHORED light entities = doc truth, render light count incl. the synthesized default key
    /// light when empty, shadow-caster index or -1, caster kind 0=dir/1=point/2=spot or -1).
    LightingDebug {
        reply: Sender<(usize, usize, i64, i64)>,
    },
    /// M11.4 — author a scene Camera entity (Transform pos + Camera{fov,near,far,active}), one undoable
    /// commit. Replies its id.
    AddCamera {
        pos: [f32; 3],
        fov: f32,
        active: bool,
        reply: Sender<Option<String>>,
    },
    /// M11.4 — LOOK THROUGH the active scene camera (`on`) or back to the editor fly-cam (`!on`): snapshots
    /// the active Camera entity's view into the render override (a projection, never Loro). Replies whether
    /// an active camera was found (when `on`).
    LookThrough {
        on: bool,
        reply: Sender<bool>,
    },
    /// M11.4 — a non-mutating camera read for the gate: (count of authored Camera entities, an active one
    /// present, the active fov in degrees or -1).
    CameraDebug {
        reply: Sender<(usize, bool, f32)>,
    },
    /// Rename an entity (`__meta__.name`) → reply applied.
    RenameEntity {
        id: String,
        name: String,
        reply: Sender<bool>,
    },
    /// Group a selection under a new parent node → reply the group id.
    GroupEntities {
        ids: Vec<String>,
        name: String,
        reply: Sender<Option<String>>,
    },
    /// Ungroup — dissolve a group (children to its parent, delete the group) → reply applied.
    UngroupEntity {
        id: String,
        reply: Sender<bool>,
    },
    /// Multi-edit — set one numeric field on N entities as ONE batched, atomic, undoable tx → reply applied.
    MultiEdit {
        ids: Vec<String>,
        component: String,
        field: String,
        value: f64,
        reply: Sender<bool>,
    },
    /// Delete = deactivate (M10.6, non-destructive; frees dependents) → reply applied.
    DeleteDeactivate {
        id: String,
        reply: Sender<bool>,
    },
    /// Copy a sub-tree to the clipboard (a read → fills the thread clipboard).
    CopySubtree {
        id: String,
    },
    /// Cut = copy + delete(deactivate) → reply applied.
    CutSubtree {
        id: String,
        reply: Sender<bool>,
    },
    /// Paste the clipboard under fresh ids → reply the new root id.
    PasteClipboard {
        reply: Sender<Option<String>>,
    },
    /// M11.1 (ADR-040) — import a user asset file from disk (FBX/glTF/OBJ/PNG/… via the MAGIC router):
    /// register its GPU mesh, place an entity carrying the handle, persist the bytes (survives reload) →
    /// reply the new entity id (or `None` on an unsupported/malformed file).
    ImportAsset {
        path: String,
        reply: Sender<Option<String>>,
    },
    /// Generation (M6, tier 3): drop a grey placeholder + kick off async text-to-3D; reply the placeholder.
    Generate {
        query: String,
        reply: Sender<GenerateResponse>,
    },
    /// A generation worker finished — import the bytes + stream the real mesh into the placeholder, on
    /// the engine thread (the !Send engine owns the world + the asset store).
    GenerateComplete {
        placeholder: String,
        prompt: String,
        bytes: Vec<u8>,
    },
    /// A generation worker failed (provider error / panic) — release the reservation (refund) and keep
    /// the honest grey placeholder (M7); never charged for a failure.
    GenerateFailed {
        placeholder: String,
        reason: String,
    },
    /// A live AI-edit (M7 + M11.2) — assign a named PBR `material` preset: a schema-validated patch metered
    /// at the edit rate (debit-on-success). Replies the economy outcome.
    AiEdit {
        id: String,
        material: String,
        reply: Sender<EconResponse>,
    },
    /// A sandbox token top-up (M7) — $10 ≈ 100 tokens via the payment seam (no real money).
    TopUp {
        reply: Sender<EconResponse>,
    },
    /// The user's token balance (M7) — a read for the wallet UI.
    WalletInfo {
        reply: Sender<EconResponse>,
    },
    /// The browsable "+ Add" catalog (M3.4), grouped by category bucket — a read.
    Catalog {
        reply: Sender<BTreeMap<String, Vec<CatalogItem>>>,
    },
    /// A catalog search (M3.4) — reuses the tiered resolver; a read.
    CatalogSearch {
        query: String,
        reply: Sender<CatalogSearch>,
    },
    /// Add a chosen catalog item (M3.4) — a free local instantiate or a metered marketplace buy; the
    /// **same** instantiate path as describe-to-create.
    Add {
        id: String,
        source: String,
        reply: Sender<AddResponse>,
    },
    /// Spawn a physics body (M8.2) — one undoable ECS setup commit, mirrored into the sim, rendered as
    /// the ball; replies its id. Starts the sim running.
    SpawnBody {
        pos: [f32; 3],
        reply: Sender<Option<String>>,
    },
    /// Physics introspection (M8.2) — `(body count, lowest body y, contact count)`. A read of the sim +
    /// the read-only diagnostic seam; lets the E2E confirm a dropped ball actually fell + landed.
    PhysicsDebug {
        reply: Sender<(usize, f64, usize)>,
    },
    /// A single body's CURRENT SIM position `[x,y,z]` — the render-side transform the sim integrates. The
    /// sim is render-only (ADR-021), so a body's motion (a shove/impulse) NEVER reaches the authored
    /// `Transform` that `read_transform` reads; this reads the sim source `physics_debug` aggregates, so a
    /// test can verify a body actually MOVED. `[0,0,0]` if the id isn't a live sim body.
    BodySimPosition {
        id: String,
        reply: Sender<[f64; 3]>,
    },
    /// M8.3 — make a dead mesh entity a correct dynamic body (one undoable commit + mirror into the sim);
    /// replies whether it applied.
    MakeDynamic {
        id: String,
        reply: Sender<bool>,
    },
    /// M11.1 — make an existing (imported) mesh a STATIC physics obstacle (fixed body + hull collider).
    MakeStatic {
        id: String,
        reply: Sender<bool>,
    },
    /// M8.3 — run the collider-intelligence catalogue for an entity (a read); replies the warnings.
    PhysicsCheck {
        id: String,
        reply: Sender<Vec<PhysicsWarning>>,
    },
    /// M8.3 — apply a one-click physics fix (`add-collider`/`use-hull`/`fix-mass`/`fix-scale`) as one
    /// undoable commit + re-mirror; replies whether it applied.
    PhysicsFix {
        id: String,
        action: String,
        reply: Sender<bool>,
    },
    /// Play/pause the deterministic sim (M8.2) — setup stays editable while paused.
    SetSimRunning(bool),
    /// M8.4 — scrub the sim timeline to `frame` over the **sim-replay channel** (deterministic replay of
    /// the recorded setup + inputs; a rewind rebuilds the world, sidestepping #910). Pauses + projects the
    /// scrubbed frame. Replies the timeline state so the slider stays in sync.
    SimScrub {
        frame: u64,
        reply: Sender<TimelineInfo>,
    },
    /// M8.4 — the timeline state for the transport UI (current frame, the max scrubbable frame, running).
    SimTimeline {
        reply: Sender<TimelineInfo>,
    },
    /// M8.4 — toggle the contact/solver debugger overlay (off by default; zero per-frame cost when off —
    /// diagnostics aren't even queried). Non-mutating: reading the seam never perturbs the sim.
    SimOverlay {
        on: bool,
    },
    /// M8.4 — apply + RECORD a one-shot impulse ("shove") on a body at the current frame, so the replay
    /// reproduces it. Replies whether it applied.
    SimShove {
        id: String,
        impulse: [f64; 3],
        reply: Sender<bool>,
    },
    /// M8.4 — the live contacts at the current (paused or running) frame, each with its measured fields +
    /// a plain-language `explain` (the M3.1/ADR-016 "debug by looking" read). Non-mutating.
    PhysicsContacts {
        reply: Sender<Vec<ContactInfo>>,
    },
    /// M8.5 — import a URDF / USD-Physics scene (`format` = "urdf" | "usd") into registry components (one
    /// undoable tx), units reconciled; replies the summary (bodies/joints/units/notes).
    ImportInterchange {
        format: String,
        source: String,
        reply: Sender<ImportResult>,
    },
    /// M9.1 — commit a gizmo transform: the entity's new world TRS (position + rotation quat + uniform
    /// display scale) as Transform fields, ONE undoable transaction. A physics body re-simulates from the
    /// new pose.
    GizmoCommit {
        id: String,
        pos: [f32; 3],
        rot: [f32; 4],
        scale: f32,
    },
    /// M9.1 — read an entity's committed Transform x/y/z (a read; lets the gizmo + E2E confirm the move
    /// landed in the core, not just the render projection).
    ReadTransform {
        id: String,
        reply: Sender<[f64; 8]>,
    },
    /// M9.2 — reparent a part ("drag in hierarchy") = one `node.move` (parent `None` → root). Undoable.
    ReparentPart {
        id: String,
        parent: Option<String>,
    },
    /// M9.2 — deactivate-not-delete a part (or reactivate it); one undoable tx. Replies whether applied.
    SetPartActive {
        id: String,
        active: bool,
        reply: Sender<bool>,
    },
    /// M9.2 — save the selected part's whole character (its root subtree) as a reusable `Composition`,
    /// kept in an in-memory registry; replies the new composition id.
    SaveCharacter {
        id: String,
        reply: Sender<Option<String>>,
    },
    /// M9.2 — drop a fresh instance of a saved `Composition` (by id); replies the new instance root id.
    InstantiateCharacter {
        comp_id: String,
        reply: Sender<Option<String>>,
    },
    /// M9.2 — a part's resolved world position + active flag + override-key count, for the E2E to confirm
    /// the override landed / a deactivate hid it / the source link survives. `(x, y, z, active, n_over)`.
    PartDebug {
        id: String,
        reply: Sender<(f64, f64, f64, bool, usize)>,
    },
    /// M9.2 — the seeded demo character's `(root, [parts])` ids (so the UI/E2E can select a part to edit).
    DemoCharacter {
        reply: Sender<Option<(String, Vec<String>)>>,
    },
    /// M9.2 — the entity at a structural rel-path within an instance root (e.g. `"0"` = first child), so
    /// the E2E can address a fresh instance's matching part after `instantiate_character`.
    PartAtPath {
        root: String,
        path: String,
        reply: Sender<Option<String>>,
    },
    /// M9.2 — a part's current parent entity id (the `node.move` edge), or `None` for a root. Lets the
    /// build-acceptance gate read back a reparent (`reparent_part`) off a stable structural signal +
    /// confirm Ctrl-Z restored the original parent.
    PartParent {
        id: String,
        reply: Sender<Option<String>>,
    },
    /// M9.4 — the snap-graph for `id`: ranked candidate targets within `radius` (the shared ADR-011 ranker)
    /// each with an explained "why this" — the magnetic-intent surface (a read).
    SnapQuery {
        id: String,
        radius: f32,
        reply: Sender<Vec<SnapHit>>,
    },
    /// M9.4 — declare + apply a spatial **constraint** to `id` (solve + commit through the one pipeline,
    /// undoable), or reply the explained block. `kind` = snap/align-surface/coplanar/coaxial/clearance/
    /// symmetry; `target` = a snap-target entity id (its position drives the constraint); `value` = the
    /// clearance distance.
    ApplyConstraint {
        id: String,
        kind: String,
        target: Option<String>,
        value: f32,
        reply: Sender<SolveResult>,
    },
    /// M9.4 — a natural-language **placement sentence**: compile → resolve the intents against the
    /// snap-graph → apply as a schema-validated patch (`apply_transform_constraint`, ADR-017). Replies the
    /// editable intents + the outcome.
    PlacementSentence {
        id: String,
        text: String,
        reply: Sender<SolveResult>,
    },
    /// M10.3 (ADR-033) — the current project state (path · unsaved-changes · recents) for the File menu.
    ProjectState {
        reply: Sender<ProjectInfoResp>,
    },
    /// M10.3 — save the project to `path` (or the current path when `None`) as a `.mtk` (atomic). No
    /// path + an untitled project ⇒ an explained "use Save As" (the native dialog is the local-GUI step).
    SaveProject {
        path: Option<String>,
        reply: Sender<ProjectInfoResp>,
    },
    /// M10.3 — open a `.mtk` project from `path` (swapping in a fresh engine/scene + re-projecting). A
    /// corrupt/newer/missing file replies an explained error and leaves the current project intact.
    /// `None` path ⇒ the native Open dialog (the local-GUI step). **Live engine-swap accepted on a GUI run.**
    OpenProject {
        path: Option<String>,
        reply: Sender<ProjectInfoResp>,
    },
    /// M10.3 — new empty project (a fresh engine/scene, the session log reset). **Accepted on a GUI run.**
    NewProject {
        reply: Sender<ProjectInfoResp>,
    },
    /// M10.4 (ADR-034) — **Play**: snapshot the edit state, then run the deterministic sim on the current
    /// scene (enter play mode). Non-destructive — the sim projects to the render only (ADR-021).
    Play {
        reply: Sender<PlayInfo>,
    },
    /// M10.4 — **Stop**: restore the pre-Play edit state **bit-exactly** from the snapshot + re-project +
    /// reset the sim (exit play mode). Reuses the project-open swap from the in-memory snapshot.
    Stop {
        reply: Sender<PlayInfo>,
    },
    /// M10.4 — **Pause / Resume**: freeze (or unfreeze) the running sim while staying in play mode.
    Pause {
        reply: Sender<PlayInfo>,
    },
    /// M10.4 — the current Play-mode state for the runtime controls (a read).
    PlayStateQuery {
        reply: Sender<PlayInfo>,
    },
    /// Internal fixed-timestep heartbeat (M8.2): a self-sent tick that advances the sim one step + syncs
    /// transforms to the viewport. NOT a JS command — it never crosses the WebView boundary (invariant 4).
    Tick,
}

/// The result of an M8.5 interchange import, surfaced to the UI: how much imported, the declared units +
/// whether they were reconciled (the M8.3 scale check), and every explained "no".
#[derive(Serialize, Clone, Default)]
struct ImportResult {
    ok: bool,
    format: String,
    bodies: usize,
    joints: usize,
    meters_per_unit: f64,
    kilograms_per_unit: f64,
    reconciled: bool,
    notes: Vec<String>,
    error: Option<String>,
}

/// The sim timeline state for the M8.4 transport UI. `frame` is the cursor; `max_frame` is the furthest
/// simulated frame (you can't scrub past what's been simulated — the slider's right edge).
#[derive(Serialize, Clone, Copy, Default)]
struct TimelineInfo {
    frame: u64,
    max_frame: u64,
    running: bool,
    overlays_on: bool,
    bodies: usize,
}

/// One contact exposed to the UI (M8.4 click-to-explain + the overlay) — the measured fields plus the
/// plain-language `explain`. A flat DTO so the WebView reads it without knowing the physics boundary types.
#[derive(Serialize, Clone)]
struct ContactInfo {
    point: [f32; 3],
    normal: [f32; 3],
    depth: f64,
    normal_impulse: f64,
    tangent_impulse: f64,
    friction: f64,
    restitution: f64,
    friction_saturated: bool,
    explain: String,
}

impl ContactInfo {
    fn from_contact(c: &Contact) -> Self {
        Self {
            point: [c.point[0] as f32, c.point[1] as f32, c.point[2] as f32],
            normal: [c.normal[0] as f32, c.normal[1] as f32, c.normal[2] as f32],
            depth: c.depth,
            normal_impulse: c.normal_impulse,
            tangent_impulse: c.tangent_impulse,
            friction: c.friction,
            restitution: c.restitution,
            friction_saturated: c.friction_saturated,
            explain: explain_contact(c),
        }
    }
}

/// The "+ Add" result (M3.4) — the created entity (+ balance after a marketplace buy), or a seam
/// (insufficient balance / unknown item).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AddResponse {
    created: Option<String>,
    balance: Option<u32>,
    seam: Option<String>,
}

/// The generation result (M6): the grey placeholder that dropped in instantly + the inert token cost,
/// or — when the provider is off/offline — the honest degradation (`available = false`). The real mesh
/// arrives later over the projection Channel (a targeted stream-in delta), not in this reply.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct GenerateResponse {
    created: Option<String>,
    cost: Option<u32>,
    available: bool,
    seam: Option<String>,
    /// The user's token balance after reserving the generation (M7), for the wallet UI.
    balance: Option<u32>,
}

/// A token-economy reply (M7) — for the AI-edit, the sandbox top-up, and the wallet-balance read.
/// `ok` = the action applied/charged; `message` carries a refusal/rejection/seam reason.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct EconResponse {
    ok: bool,
    /// The user's token balance after, in whole tokens.
    balance: u32,
    /// Tokens charged (an edit) or granted (a top-up), if any.
    cost: Option<u32>,
    /// A refusal/rejection/seam reason, when not `ok`.
    message: Option<String>,
}

/// Hover-tooltip details for an entity (M3.3) — name · key components · provided/required caps · the
/// entities it's bound to. Read-only projection; fetched on hovered-entity change (not per frame).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct EntityDetails {
    id: String,
    name: String,
    components: Vec<String>,
    provides: Vec<String>,
    requires: Vec<String>,
    bound_to: Vec<String>,
}

/// M11.5 (ADR-044) — the inspector's asset-IDENTITY surface for a selected entity: where its mesh came
/// from, whether it was AI-generated (honestly flagged), and a near-duplicate hint. A read-only projection
/// over [`AssetsRuntime::provenance`]; `None` if the entity has no store-resolvable mesh. The perceptual
/// hash is rendered as hex so the React side can show it without a u64-precision-loss round-trip through JS.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ProvenanceInfo {
    /// "imported" / "generated" / "unknown".
    kind: String,
    /// File name, provider tag, or "(restored on reload)".
    source: String,
    /// Honestly surfaced — true only for the generation tier.
    ai_generated: bool,
    /// The store's content-address handle (referenced, not rebuilt).
    content_hash: String,
    /// The perceptual (dHash) fingerprint, hex — `"0"` when the asset carries no texture.
    perceptual_hash: String,
    /// The `source` of an already-loaded, different-bytes asset this one perceptually matches (a HINT —
    /// never a silent merge). `None` when nothing similar is loaded.
    near_duplicate_of: Option<String>,
}

// ── M12.1 (ADR-045) Rules-layer wire types ─────────────────────────────────

/// One entry in the Rules builder's "When"/"Then" dropdowns — a registry event or action verb.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleVocabItem {
    name: String,
    description: String,
}

/// A component the builder's If/Then can target, with its fields + scalar types (so the builder offers only
/// real `component.field`s and a type-matched value input — typo-proof by construction).
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleComponentVocab {
    name: String,
    fields: Vec<RuleFieldVocab>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleFieldVocab {
    name: String,
    /// `"integer"` / `"number"` / `"boolean"` / `"string"`.
    ty: String,
}

/// The whole registry-fed vocabulary the Rules builder is assembled from (ADR-045 deliverable 2) — what
/// makes every dropdown typo-proof. A pure read of the standard library; no engine round-trip needed.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleRegistryInfo {
    events: Vec<RuleVocabItem>,
    actions: Vec<RuleVocabItem>,
    components: Vec<RuleComponentVocab>,
}

/// A row in the editor's Rule list — a compact projection of an authored rule.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleSummary {
    id: String,
    name: String,
    enabled: bool,
    event: String,
    condition_count: usize,
    action_count: usize,
}

/// The result of authoring a rule: the new id on success, a plain-language `error` if the registry
/// **Blocked** it (ADR-016), and the proactively-offered **mirror** "cleanup" rule (the missing-"off"-switch
/// guard) for the UI to surface — `None` if there's no well-defined inverse.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AuthorRuleResult {
    id: Option<String>,
    error: Option<String>,
    mirror: Option<metrocalk_core::RuleData>,
}

/// M12.2 (ADR-046) — a state machine for the editor's state-graph view: the **full** machine (states +
/// transitions, so the React Flow graph can render nodes + edges) plus its id and live **current** state
/// (the M12.5 seam, defaulting to `initial`). The graph keys off the stable state names + transition ids
/// inside `machine`, never any label copy.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StateMachineInfo {
    id: String,
    current: String,
    machine: metrocalk_core::StateMachine,
}

/// The result of authoring a state machine: the new id on success, a plain-language `error` if it was
/// **Blocked** (ADR-016: no name / dangling transition / a typo'd transition Rule / not-a-state-change),
/// and the **unreachable** states — a warning surfaced (explained), never a rejection (ADR-046).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct AuthorStateMachineResult {
    id: Option<String>,
    error: Option<String>,
    unreachable: Vec<String>,
}

/// M12.3 (ADR-047) — the outcome of running a sandboxed WASM plugin: `ok` + how many field ops its effect
/// applied (committed as one undoable transaction), or a plain-language `error` if the plugin was Blocked
/// (a rejected effect — the ADR-017 guard) or **contained** (a missing plugin / trap / timeout / over-budget
/// / bad output — never a crash).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct RunPluginResult {
    ok: bool,
    applied: usize,
    error: Option<String>,
}

/// M12.4 (ADR-048) — a reviewable AI-compose PROPOSAL: the `composition` (validated against the live scene,
/// serialized so the UI can preview the patches) and a count of ops, or a plain-language `error` (offline,
/// no target, an unrecognized sentence, or a proposal that fails validation). `ok` ⇒ safe to apply. Nothing
/// is applied here — the user reviews, then submits the composition back via `compose` (the apply step).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ComposeProposal {
    ok: bool,
    /// The proposed composition as JSON (the exact payload to hand back to `compose`); `null` on error.
    composition: Option<serde_json::Value>,
    ops: usize,
    error: Option<String>,
}

/// M12.4 (ADR-048) — the outcome of APPLYING a composition: `ok` + how many ops `applied` (one undoable
/// transaction) and the project's `rules` / `stateMachines` counts after, or a plain-language `error` if the
/// composition was rejected-as-UX (nothing applied, all-or-nothing). Mirrors the MCP server's `ApplyResult`.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct ComposeResult {
    ok: bool,
    applied: usize,
    rules: usize,
    state_machines: usize,
    error: Option<String>,
}

/// M12.5 (ADR-049) — one rule's plain-language **explanation** at the current frame (`explain_rule`, the
/// M3.1/M8.4 explain engine on logic): why it did / didn't fire, shown not logged.
#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RuleExplain {
    rule: String,
    text: String,
}

/// M12.5 (ADR-049) — the **live truth-state debugger** payload: the clicked entity's truth-state (rules with
/// per-condition ✅/❌ + machine current state — `debug by looking`), each rule's `explain_rule` narration, the
/// frame-stamped **decision history** (time-travelable), and any rules **flagged** out of the deterministic
/// path (a non-deterministic plugin). `playing=false` ⇒ not in Play (the rest is empty). All a projection —
/// reading it never mutates the run or the doc.
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct RuleDebugInfo {
    playing: bool,
    /// The current decision-history frame the cursor is at.
    frame: u64,
    /// The highest frame reached (the scrubber's max — the live head).
    head: u64,
    /// The clicked entity's live truth-state (`None` if no entity was asked for / not in Play).
    truth: Option<metrocalk_core::TruthState>,
    /// Per-rule plain-language explanations for the rules in `truth`.
    explanations: Vec<RuleExplain>,
    /// The decision history up to the current frame (frame-stamped).
    decisions: Vec<metrocalk_core::DecisionEvent>,
    /// Rules excluded from the deterministic Play path (non-deterministic plugin) — surfaced, never silent.
    flagged: Vec<metrocalk_core::FlaggedRule>,
}

struct AppState {
    tx: Sender<EngineCmd>,
    shared: Shared,
}

// ── engine thread: owns the real Engine + the capability scene + the bridge ─────

/// The persistence log path — next to the executable, so it's stable across launches of the same
/// build (close→reopen restores). Falls back to the working dir if the exe path is unavailable.
fn log_path() -> std::path::PathBuf {
    sidecar("metrocalk-scene.jsonl")
}

/// The token wallet's persisted ledger (M7) — a sidecar beside the scene log + window state, so the
/// balance survives close→reopen (and the free grant can't be farmed by relaunching).
fn wallet_path() -> std::path::PathBuf {
    sidecar("metrocalk-wallet.json")
}

/// A sidecar file next to the executable (stable across launches of the same build), falling back to
/// the working dir if the exe path is unavailable. Both the scene log and the window state live here.
fn sidecar(name: &str) -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(name)
}

/// Persisted window geometry, so the editor reopens where it was left ("open where the last instance
/// was"). Saved by a write-on-change poll (see `setup`) — robust to a hard terminal kill, which fires
/// no close event, and needs no prior move to establish a baseline.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
struct WinGeom {
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    /// Whether the window was maximized — restored by `maximize()`, not by positioning (which would
    /// de-maximize and clip the frame off the top-left). `#[serde(default)]` so older files still parse.
    #[serde(default)]
    maximized: bool,
}

fn window_state_path() -> std::path::PathBuf {
    sidecar("metrocalk-window.json")
}

/// The window's current outer position + inner size (+ maximized flag), or `None` if unavailable or
/// minimized (zero size) — so we never persist an invisible 0×0 to restore into.
fn current_geom(window: &tauri::WebviewWindow) -> Option<WinGeom> {
    let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size()) else {
        return None;
    };
    if size.width == 0 || size.height == 0 {
        return None;
    }
    Some(WinGeom {
        x: pos.x,
        y: pos.y,
        w: size.width,
        h: size.height,
        maximized: window.is_maximized().unwrap_or(false),
    })
}

/// Write geometry to the sidecar. Best-effort.
fn save_geom(g: &WinGeom) {
    if let Ok(s) = serde_json::to_string(g) {
        let _ = std::fs::write(window_state_path(), s);
    }
}

/// Is a grab-able point on the window's title bar (just inside the saved top-left) on a currently
/// connected monitor? Guards against restoring onto a since-disconnected / rearranged monitor
/// (dock→undock), which would otherwise strand the only window off-screen with no reachable title bar.
/// Fails open (returns `true`) if monitors can't be enumerated.
fn position_on_a_monitor(window: &tauri::WebviewWindow, x: i32, y: i32) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return true;
    };
    let (px, py) = (x + 16, y + 16);
    monitors.iter().any(|m| {
        let p = m.position();
        let s = m.size();
        let mw = i32::try_from(s.width).unwrap_or(i32::MAX);
        let mh = i32::try_from(s.height).unwrap_or(i32::MAX);
        px >= p.x && py >= p.y && px < p.x + mw && py < p.y + mh
    })
}

/// Restore the saved window geometry, if any. Best-effort: a missing/corrupt file leaves the configured
/// (centered) default. A maximized window is re-maximized rather than positioned; otherwise the saved
/// position is applied only when its title bar would land on a connected monitor (so the window can
/// never open stranded off-screen after a monitor change).
fn restore_window_geom(window: &tauri::WebviewWindow) {
    let Ok(s) = std::fs::read_to_string(window_state_path()) else {
        return;
    };
    let Ok(g) = serde_json::from_str::<WinGeom>(&s) else {
        return;
    };
    if g.maximized {
        let _ = window.maximize();
        return;
    }
    let _ = window.set_size(tauri::PhysicalSize::new(g.w, g.h));
    if position_on_a_monitor(window, g.x, g.y) {
        let _ = window.set_position(tauri::PhysicalPosition::new(g.x, g.y));
    }
}

/// M15.7 (ADR-077) — a scene-visible proxy box size (metres). Proxies stand in for geometry the licensed
/// kernel would decode (proprietary CATIA reps); size them to ≈1/150 of the placement diagonal (clamped) so a
/// 15 m factory cell isn't rendered as invisible 1 mm dots.
fn cad_proxy_scale(report: &metrocalk_interchange::CadImport, m_per_unit: f64) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &report.parts {
        let t = metrocalk_interchange::translation_of(&p.transform);
        for k in 0..3 {
            lo[k] = lo[k].min(t[k] * m_per_unit);
            hi[k] = hi[k].max(t[k] * m_per_unit);
        }
    }
    let diag = (0..3)
        .map(|k| (hi[k] - lo[k]).max(0.0))
        .map(|d| d * d)
        .sum::<f64>()
        .sqrt();
    (diag / 70.0).clamp(0.1, 3.0)
}

/// FNV-1a over the 3×3 basis's f64 bit patterns — the deterministic per-instance tag a BAKED (mirrored /
/// scaled, non-rigid) placement's mesh handle carries, so same-basis instances of the same geometry still
/// dedup to one GPU mesh while differently-mirrored twins stay distinct.
fn basis_bits_hash(m: &[f64; 16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for i in [0usize, 1, 2, 4, 5, 6, 8, 9, 10] {
        for b in m[i].to_bits().to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

/// The rotation quaternion `[x, y, z, w]` of a column-major rigid 4×4 (the part's assembly orientation — the
/// reposition axes carry rotation, not just translation, so a bracket welded at an angle lands at that angle).
/// The basis columns are orthonormal (AXIS2_PLACEMENT frames), so this is the exact standard trace conversion.
fn quat_of_transform(m: &[f64; 16]) -> [f32; 4] {
    // R[row][col] = m[col*4 + row]; columns 0/1/2 are the x/y/z axes.
    let (r00, r01, r02) = (m[0], m[4], m[8]);
    let (r10, r11, r12) = (m[1], m[5], m[9]);
    let (r20, r21, r22) = (m[2], m[6], m[10]);
    let trace = r00 + r11 + r22;
    let (x, y, z, w) = if trace > 0.0 {
        let s = 0.5 / (trace + 1.0).sqrt();
        ((r21 - r12) * s, (r02 - r20) * s, (r10 - r01) * s, 0.25 / s)
    } else if r00 > r11 && r00 > r22 {
        let s = 2.0 * (1.0 + r00 - r11 - r22).sqrt();
        (0.25 * s, (r01 + r10) / s, (r02 + r20) / s, (r21 - r12) / s)
    } else if r11 > r22 {
        let s = 2.0 * (1.0 + r11 - r00 - r22).sqrt();
        ((r01 + r10) / s, 0.25 * s, (r12 + r21) / s, (r02 - r20) / s)
    } else {
        let s = 2.0 * (1.0 + r22 - r00 - r11).sqrt();
        ((r02 + r20) / s, (r12 + r21) / s, 0.25 * s, (r10 - r01) / s)
    };
    [x as f32, y as f32, z as f32, w as f32]
}

/// M15.7 (ADR-077) — land a CAD file (CATIA 3DXML / STEP AP242) onto the live scene: read it via the
/// never-empty/never-silent pipeline, register each UNIQUE tessellated mesh on the GPU (dedup → instancing),
/// and create one renderable entity per part at its real (units-normalized) transform as ONE undoable commit.
/// Proxies get a scene-visible size; real geometry the metric unit scale. Returns the first entity id, or
/// None + an explained log line on a container error (never a panic).
fn land_cad(
    bytes: &[u8],
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    assets: &mut AssetsRuntime,
    shared: &Shared,
) -> Option<EntityId> {
    // File log (the .exe is a windows-subsystem GUI app → eprintln has no console; log to a temp file so a
    // live import can be diagnosed).
    let logf = std::env::temp_dir().join("mtk-cad-import.log");
    let log = |m: &str| {
        use std::io::Write as _;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&logf)
        {
            let _ = writeln!(f, "{m}");
        }
        eprintln!("[shell] {m}");
    };
    log(&format!("CAD import start: {} bytes", bytes.len()));
    let report = match metrocalk_editor_shell::read_cad(bytes) {
        Ok(r) => r,
        Err(e) => {
            log(&format!("CAD import FAILED (read): {e}"));
            return None;
        }
    };
    log(&format!("CAD import read OK: {}", report.summary()));

    // Meshes are registered on the GPU on demand per part below, keyed by (geometry hash + authored colour) so
    // a part renders in its real STEP colour (baked into the mesh material) while identical geometry+colour
    // instances still share ONE GPU mesh (dedup → instancing).

    // Units: the source is mm; normalize placement + geometry to metres. Proxies get a scene-visible size.
    let m_per_unit = report.units.meters_per_unit;
    let proxy_scale = cad_proxy_scale(&report, m_per_unit);
    let renderable = scene
        .caps
        .get(&metrocalk_core::caps::canonical("Renderable"))
        .copied();

    let mut ops: Vec<metrocalk_core::Op> =
        Vec::with_capacity(report.parts.len() * 7 + report.groups.len() * 4);
    let mut first: Option<EntityId> = None;
    // Map each source hierarchy-node id (assembly occurrence) → its allocated engine entity, so leaf parts +
    // child groups resolve their `parent`. `report.groups` is topological (parent-before-child), so a group's
    // parent entity always exists in the map before a child references it, and all groups precede the leaf
    // parts — satisfying the commit's parent-before-child validation. (The OUTLINER reads the tree in pre-order
    // regardless of creation order — `bridge::project_full` sorts the projection into tree order.)
    let mut src_to_entity: std::collections::BTreeMap<u64, EntityId> =
        std::collections::BTreeMap::new();

    // (1) The NAMED structural tree: one geometry-free container entity per assembly occurrence, nested exactly
    // as the source file. Each is an IDENTITY transform (leaf parts carry the world placement) marked
    // `__meta__.kind = "group"` with NO MeshRenderer, so the rebuild skip renders nothing for it (no cube).
    for g in &report.groups {
        let ge = engine.alloc_entity_id();
        if first.is_none() {
            first = Some(ge);
        }
        src_to_entity.insert(g.id, ge);
        let parent = g.parent.and_then(|pid| src_to_entity.get(&pid).copied());
        ops.push(metrocalk_core::Op::CreateEntity { id: ge, parent });
        for (f, v) in [
            ("x", 0.0),
            ("y", 0.0),
            ("z", 0.0),
            ("qx", 0.0),
            ("qy", 0.0),
            ("qz", 0.0),
            ("qw", 1.0),
            ("scale", 1.0),
        ] {
            ops.push(metrocalk_core::Op::SetField {
                entity: ge,
                component: "Transform".into(),
                field: f.into(),
                value: FieldValue::Number(v),
            });
        }
        if !g.name.is_empty() {
            ops.push(metrocalk_core::Op::SetField {
                entity: ge,
                component: metrocalk_core::variant::INSTANCE_META.into(),
                field: "name".into(),
                value: FieldValue::Str(g.name.clone()),
            });
        }
        ops.push(metrocalk_core::Op::SetField {
            entity: ge,
            component: metrocalk_core::variant::INSTANCE_META.into(),
            field: "kind".into(),
            value: FieldValue::Str("group".into()),
        });
    }

    // (2) The leaf parts — each parented under its source assembly occurrence (its `parent` group) so it nests
    // in the outliner exactly where the source places it, while keeping its own world transform (identity
    // groups don't perturb `global_transform`, so placement is byte-identical to the flat import).
    for p in &report.parts {
        let e = engine.alloc_entity_id();
        if first.is_none() {
            first = Some(e);
        }
        let parent = p.parent.and_then(|pid| src_to_entity.get(&pid).copied());
        ops.push(metrocalk_core::Op::CreateEntity { id: e, parent });
        let t = metrocalk_interchange::translation_of(&p.transform);
        let scale = if p.fidelity.is_real_geometry() {
            m_per_unit
        } else {
            proxy_scale
        };
        // Real geometry carries its assembly orientation (the reposition axes rotate parts); a proxy box is
        // orientation-free, so only real tessellation writes the quaternion. A quaternion represents ONLY a
        // proper rigid rotation — a CATIA mirror/scaled instance basis (det<0, symmetry instances) is instead
        // BAKED into a per-instance mesh below, and the entity carries just the translation.
        let rigid = metrocalk_editor_shell::basis_is_rigid(&p.transform);
        let q = if p.fidelity.is_real_geometry() && rigid {
            quat_of_transform(&p.transform)
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        for (f, v) in [
            ("x", t[0] * m_per_unit),
            ("y", t[1] * m_per_unit),
            ("z", t[2] * m_per_unit),
            ("qx", f64::from(q[0])),
            ("qy", f64::from(q[1])),
            ("qz", f64::from(q[2])),
            ("qw", f64::from(q[3])),
            ("scale", scale),
        ] {
            ops.push(metrocalk_core::Op::SetField {
                entity: e,
                component: "Transform".into(),
                field: f.into(),
                value: FieldValue::Number(v),
            });
        }
        if let Some(mi) = p.mesh {
            let m = &report.meshes[mi];
            // A non-rigid instance basis (mirror/scale) is baked into a per-instance mesh — a mirrored
            // bracket is genuinely different geometry from its twin, so it keys its own handle (tagged with
            // the basis-bits hash; same mirrored geometry still dedups across its instances).
            let baked = if rigid || !p.fidelity.is_real_geometry() {
                None
            } else {
                Some(metrocalk_editor_shell::bake_basis_into_mesh(
                    &p.transform,
                    &m.tris,
                ))
            };
            let tris = baked.as_ref().unwrap_or(&m.tris);
            let basis_tag = if baked.is_some() {
                format!(":b{:016x}", basis_bits_hash(&p.transform))
            } else {
                String::new()
            };
            // Colour-aware GPU handle: the part's authored STEP colour is baked into the mesh material, so
            // same-geometry-different-colour parts are distinct GPU meshes while same-geometry+colour instances
            // still dedup to one (registered on demand here).
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let handle = match p.color {
                Some(c) => format!(
                    "mtkcad:{:016x}{basis_tag}:{:02x}{:02x}{:02x}",
                    m.hash,
                    (c[0] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (c[1] * 255.0).round().clamp(0.0, 255.0) as u8,
                    (c[2] * 255.0).round().clamp(0.0, 255.0) as u8,
                ),
                None => format!("mtkcad:{:016x}{basis_tag}", m.hash),
            };
            if !assets.handle_to_slot.contains_key(&handle) {
                let asset = metrocalk_editor_shell::csg_intent::trimesh_to_mesh_asset_colored(
                    tris, "cad", p.color,
                );
                let gpu = MeshGpu::from_asset(&asset);
                let slot = assets.meshes.len();
                assets.meshes.push(gpu.clone());
                assets.scales.push(1.0);
                assets.handle_to_slot.insert(handle.clone(), slot);
                let mut st = shared.lock().unwrap();
                st.meshes.push(gpu);
                st.meshes_revision = st.meshes_revision.wrapping_add(1);
                drop(st);
                // Persist the DERIVED mesh so the saved doc's `mtkcad:` handle re-resolves after restart +
                // open (load_assets restores it) — without this every imported part silently degraded to a
                // placeholder cube on reload. Never-silent: a write failure is logged, not swallowed.
                if let Err(e) = metrocalk_editor_shell::persist_cad_mesh(
                    &sidecar("metrocalk-cad-meshes"),
                    &handle,
                    tris,
                    p.color,
                ) {
                    log(&format!(
                        "CAD import: failed to persist mesh {handle} — it may not survive reload: {e}"
                    ));
                }
            }
            ops.push(metrocalk_core::Op::SetField {
                entity: e,
                component: "MeshRenderer".into(),
                field: "mesh".into(),
                value: FieldValue::Str(handle),
            });
        }
        for (field, value) in [
            ("fidelity", p.fidelity.token().to_string()),
            ("name", p.name.clone()),
        ] {
            ops.push(metrocalk_core::Op::SetField {
                entity: e,
                component: metrocalk_editor_shell::CAD_PART.into(),
                field: field.into(),
                value: FieldValue::Str(value),
            });
        }
        // The user-facing OUTLINER name (`__meta__.name`) — the real source part name (CATIA `V_Name` / STEP
        // product), so the scene tree reads "Overhead Crane", not a bare entity id.
        if !p.name.is_empty() {
            ops.push(metrocalk_core::Op::SetField {
                entity: e,
                component: metrocalk_core::variant::INSTANCE_META.into(),
                field: "name".into(),
                value: FieldValue::Str(p.name.clone()),
            });
        }
        if let Some(c) = renderable {
            ops.push(metrocalk_core::Op::AddPair {
                entity: e,
                rel: scene.rels.provides,
                target: c,
            });
        }
    }
    log(&format!(
        "CAD import: {} meshes registered, {} parts + {} group nodes, committing…",
        report.meshes.len(),
        report.parts.len(),
        report.groups.len()
    ));
    if let Err(err) = engine.commit("import-cad", ops) {
        log(&format!("CAD import commit REJECTED: {err:?}"));
        return None;
    }
    log("CAD import commit OK");
    first
}

fn engine_thread(rx: mpsc::Receiver<EngineCmd>, shared: Shared, self_tx: Sender<EngineCmd>) {
    // Import the demo mesh assets once (one-shot heavy op) before seeding, so the catalog is ready for
    // describe-to-create + replay and the viewport's geometry is published. `mut` so a *generated* asset
    // can be added to the render store at runtime (M6 stream-in).
    let mut assets = load_assets();
    // The marketplace index (M5) — a local checked-in catalog behind the trait; describe-to-create's
    // second tier (queried only on a no-local-match). A remote index slots in here unchanged.
    let market = LocalCatalog::builtin();
    // The generation tier (M6) — tier 3, opt-in. The deterministic FAKE provider returns the prop mesh
    // after a simulated round-trip so the placeholder→stream-in loop is visible offline; the REAL
    // provider is a documented seam (RemoteGenerator). The token meter is the ADR-004 stub (no money).
    let generator = metrocalk_editor_shell::generate::FakeGenerator::new(
        PROP_GLB.to_vec(),
        std::time::Duration::from_millis(700),
        true,
    );
    // M12.4 (ADR-048) — the in-app AI COMPOSE seam: the deterministic DEMO composer (available offline so
    // the demo + e2e work without a model/network); the REAL LLM composer is a documented seam beside the
    // shipped `metrocalk-mcp` server. It only PROPOSES — `apply_composition` is the every-"no" gate.
    let composer = metrocalk_editor_shell::compose_ai::DemoComposer::new(true);
    // The token wallet (M7) — the file-backed ledger the paid sinks meter against (free-tier seeded,
    // orphan holds released on load). The sandbox payment provider tops it up (no real money; the real
    // provider is a go-live seam). Separate from the scene log: replay never re-charges.
    let mut wallet = Wallet::open(wallet_path());
    let payments = SandboxProvider;
    {
        let mut st = shared.lock().unwrap();
        st.meshes = assets.meshes.clone();
        st.meshes_revision = st.meshes_revision.wrapping_add(1);
    }

    let mut world = FlecsWorld::new();
    // Intern the capability relationships BEFORE the engine takes the world (they are metadata, like
    // the registry's own interned rels — not scene entities).
    // `mut` so a project Open/New (M10.3) can swap in a fresh capability scene alongside the engine.
    let mut scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    // Mirror capability pairs into the durable Loro document so a load/merge re-derives them (ADR-032,
    // the M1.6 capability-rebuild fix). Set BEFORE seeding so every seeded `(Provides/Requires, cap)`
    // pair is mirrored — this is what makes the future `.mtk` Loro-document load path keep reveal/bind
    // working (the replay-log path doesn't need it, but the project format does).
    engine.set_capability_resolver(Box::new(capscene::CapResolver::from_scene(&scene)));
    // M12.1 (ADR-045) — the Rules-layer vocabulary registry: the events/actions/components the registry-fed
    // builder offers + `validate_rule` checks (typo-proof, Blocked+explained). A SEPARATE throwaway
    // FlecsWorld — `validate_rule`'s event/action/component-field lookups read the registry's own maps, never
    // the world (only capability-pair queries touch it, which the Rules layer doesn't use).
    let rules_registry = {
        let mut reg = metrocalk_core::Registry::new(FlecsWorld::new());
        for c in metrocalk_core::stdlib::standard_components() {
            let _ = reg.register(c);
        }
        for e in metrocalk_core::stdlib::standard_events() {
            reg.register_event(e);
        }
        for a in metrocalk_core::stdlib::standard_actions() {
            reg.register_action(a);
        }
        // M12.5 (ADR-049) — register the plugin vocabulary too, so the Play-time determinism partition
        // (`play_rules::build_recording`) can read each plugin's `deterministic` flag to hold a
        // non-deterministic plugin out of the lockstep replay path.
        for p in metrocalk_core::stdlib::standard_plugins() {
            reg.register_plugin(p);
        }
        reg
    };
    // The seed count defaults to the M2 stress target (`SCENE_N`), but `MTK_SCENE_N` overrides it — a
    // clean low-/zero-entity scene for visually inspecting a single imported asset (e.g. an FBX) without
    // 5000 cubes burying it. The fingerprint folds the count in, so a non-default seed just gets its own
    // replay log namespace (it never corrupts the default project's log).
    let scene_n: usize = std::env::var("MTK_SCENE_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SAMPLE_N); // C10: small named-able first-run by default; MTK_SCENE_N=5000 = stress fixture
    let index = capscene::seed(&mut engine, &scene, scene_n).expect("seed capability scene");
    // M9.2: a small **composed character** (a body root + two rigid child parts) for part editing —
    // seeded as deterministic scene construction right after the seed (its ids are stable across launches
    // so the override-edit persistence re-binds; the `mtkscene2` fingerprint reflects this added draw).
    // The parts render as the prop mesh; click one → gizmo-edit it (a per-field override).
    let demo_char = capscene::compose_character(
        &mut engine,
        [0.0, 1.0, 6.0],
        assets.catalog.get("MeshRenderer").map(String::as_str),
    )
    .ok();
    if let Some((root, parts)) = &demo_char {
        eprintln!(
            "[shell] composed a demo character: body {} with {} rigid parts (click a part to edit it)",
            root.to_loro_key(),
            parts.len()
        );
    }
    // The seed + the composed character are scene construction, not user edits — drop them from the undo
    // stack so Ctrl-Z can never undo past the user's binds and delete the whole world.
    engine.clear_history();

    // Live persistence: re-seeding is deterministic (same SEED → identical ids), so replay the
    // append-only edit log on top to restore the user's prior binds/edits. clear_history again so the
    // restored scene is non-undoable too (same Ctrl-Z guard as the seed). The catalog re-derives any
    // described kind's mesh handle so a *visible* described object survives reload too.
    let log = Log::open(log_path(), capscene::fingerprint(scene_n));
    let (restored, skipped) = log.replay(&mut engine, &scene, &assets.catalog);
    engine.clear_history();
    eprintln!(
        "[shell] seeded {} entities — {} HealthBars, {} unbound Health providers; restored {restored} edits ({skipped} skipped)",
        engine.entity_count(),
        index.health_bars.len(),
        index.unbound_health_providers
    );
    if let Some(first) = index.health_bars.first() {
        eprintln!(
            "[shell] click HealthBar {} to reveal bindable targets",
            first.to_loro_key()
        );
    }

    // M10.3 (ADR-033) deliverable 4 — startup = **open-last-else-the-seeded-sample**. The seed+replay
    // above is the known-good default (the "new"/sample scene); if the user's last project opens cleanly,
    // swap it in BEFORE the first projection. **FAIL-SAFE:** any open failure (missing/corrupt/newer)
    // keeps the sample, so a bad last-project can never break launch. (The seed work is "wasted" when a
    // project opens — a small one-shot boot cost that keeps this an additive, low-risk flip.)
    let recents_path = sidecar("metrocalk-recents.json");
    let mut current_path: Option<std::path::PathBuf> = None;
    if let Some(last) = mtk_project::load_recents(&recents_path).into_iter().next() {
        let p = std::path::PathBuf::from(&last);
        if p.exists() {
            let mut w = FlecsWorld::new();
            let s = CapScene::intern(&mut w);
            let mut e = Engine::new(w, 1);
            e.set_capability_resolver(Box::new(capscene::CapResolver::from_scene(&s)));
            if mtk_project::open_into(&mut e, &p).is_ok() {
                engine = e;
                scene = s;
                current_path = Some(p);
                eprintln!("[shell] opened last project: {last}");
            } else {
                eprintln!(
                    "[shell] last project didn't open cleanly — starting from the sample scene"
                );
            }
        }
    }
    // The Loro version vector at the last save/open/new (captured AFTER any open/seed, so a fresh session
    // starts "clean"): `dirty = current vv != saved_vv` needs no per-command instrumentation.
    let mut saved_vv: Vec<u8> = engine.version_vector();

    let mut positions: HashMap<Entity, [f32; 3]> = HashMap::new();
    rebuild(&engine, &shared, &mut positions, &assets);
    let mut channel: Option<Channel<ProjectionDelta>> = None;
    // Last-touched sequence per entity (higher = more recent) — the reveal's recency ranking signal,
    // bumped on every committed edit/bind so it's live, not inert.
    let mut recency: HashMap<Entity, u64> = HashMap::new();
    let mut touch: u64 = 0;
    // M10.6 — the scene-authoring clipboard: a copied/cut sub-tree's resolved `Composition` (serde), held
    // on this thread between copy/cut and paste. Cross-project paste (a project clipboard) is M10.3's seam.
    let mut clipboard: Option<metrocalk_core::Composition> = None;
    // In-flight generation reservations (M7): placeholder loro-key → the token Hold, so the async
    // completion settles (success) or releases (failure) exactly the right hold. Lives on this thread
    // only, so reserve→settle/release is atomic (no race).
    let mut pending_gen: HashMap<String, HoldId> = HashMap::new();
    // M9.2: saved characters — `save_composition` results kept in-memory by id, so a later "drop instance"
    // re-instantiates them (the marketplace index would carry these — the same serializable Composition).
    // A counter makes each saved id unique within the session. (Session-scoped; persisting saved
    // compositions + their instantiations across reload is a named follow-up — the override EDITS persist.)
    let mut compositions: HashMap<String, metrocalk_core::Composition> = HashMap::new();
    let mut comp_seq: u32 = 0;

    // ── M8.2/M8.4 physics: the deterministic sim (f64 + enhanced-determinism, gameplay fidelity) the
    // engine thread owns, driven over the **sim-replay channel** (ADR-021/023 — distinct from Loro
    // time-travel). The ECS is the single authority over which bodies EXIST; setup is undoable commits;
    // the per-tick transform stream is a projection to the render `SceneState`, NEVER a commit.
    //
    // The current RUN is described by a `Recording` (a fixed ground body 0 + every ECS physics body + the
    // recorded shove inputs) and a `frame` counter. `sim` is `recording.build()` stepped `frame` times —
    // so a SCRUB re-derives `sim` deterministically by rebuilding from the recording (M8.4 P2/P3), and a
    // body-set change starts a fresh run capturing the current state (no visual jump). `body_of` maps each
    // ECS physics entity → its sim body (rebuilt each run); `rec_entities` is the recording-body-index →
    // entity map (index 0 = the ground = `None`). A run's shove inputs live in `recording.inputs` (by body
    // index), so a SCRUB replays them; a new run rebuilds the recording fresh (past shoves are already
    // baked into the captured current state).
    let (mut recording, mut rec_entities, mut sim, mut body_of) = restart_run(
        &engine,
        &assets,
        &RapierPhysics::new(Fidelity::Gameplay.resolve().config),
        &HashMap::new(),
    );
    let mut frame: u64 = 0;
    // The furthest frame simulated this run — the scrub slider's right edge (you can't scrub into a future
    // that hasn't been simulated). Advances with `frame` during play; held across a scrub.
    let mut max_frame: u64 = 0;
    // The contact/solver debugger overlay — OFF by default (zero per-frame cost: diagnostics aren't even
    // queried when off). Non-mutating when on.
    let mut overlays_on = false;
    // Previous frame's body centres, for the swept-volume (trajectory) overlay segments.
    let mut prev_centers: HashMap<EntityId, [f32; 3]> = HashMap::new();
    let mut sim_running = !body_of.is_empty();
    // M10.4 (ADR-034) — Play mode. `play_mode` = the editor is RUNNING the scene (vs authoring); the
    // deterministic sim already projects per-tick transforms to the render only (ADR-021 — never the
    // ECS/Loro), so a running scene never mutates the authored document. `play_snapshot` captures the
    // edit-state Loro doc on Play; **Stop restores from it bit-exactly** (a fresh engine + merge — so
    // even an edit that leaked in Play is wiped; non-destructive by construction). `paused` = play mode
    // with the sim frozen (`sim_running == false` while `play_mode`).
    let mut play_mode = false;
    let mut play_snapshot: Option<Vec<u8>> = None;
    // M11.4 (ADR-043) — Play renders through the ACTIVE scene camera. On Play we snapshot the active
    // camera's view into the render override (`cam_override`, a projection — never Loro/undo, ADR-021),
    // saving whatever the editor view was (fly-cam or a manual look-through); on Stop we restore it.
    let mut pre_play_cam: Option<render::CamView> = None;
    // M12.5 (ADR-049) — the Play-time Rules session: the authored Rules + state machines, captured into a
    // deterministic `RuleReplay` at Play-start (`play_rules::build_recording`) and dropped on Stop. It runs
    // the Rules as a PROJECTION over a `RuntimeState` — never the ECS/Loro doc (ADR-021/034), so a Rule
    // firing in Play can't corrupt the authored scene (re-confirmed in `editor-shell/tests/rule_runtime.rs`).
    // `rule_flagged` = rules held out of the deterministic path (a non-deterministic plugin); `rule_head` =
    // the furthest decision-history frame reached (the scrubber's right edge — the M8.4 timeline for logic).
    let mut rule_session: Option<metrocalk_core::RuleReplay> = None;
    let mut rule_flagged: Vec<metrocalk_core::FlaggedRule> = Vec::new();
    let mut rule_head: u64 = 0;
    // A fixed-cadence heartbeat (~60/s) on its own thread enqueues `Tick` via the engine's own sender, so
    // the sim advances ON the engine thread (off the JS hot path, invariant 4) without blocking the
    // command loop. A `Tick` is a no-op until the sim is running with at least one body.
    {
        let ticker = self_tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(16));
            if ticker.send(EngineCmd::Tick).is_err() {
                // The engine thread is gone (exited or panicked) — say so once (audit F6) instead of the
                // ticker just silently stopping while the viewport appears frozen.
                eprintln!("[shell] engine thread is gone — the physics ticker has stopped");
                break;
            }
        });
    }

    while let Ok(cmd) = rx.recv() {
        match cmd {
            EngineCmd::Connect(ch) => {
                send_proj!(ch, proj_full(&engine, &scene)); // initial full-scene load
                channel = Some(ch);
            }
            EngineCmd::Edit(tx) => {
                // M10.4 (ADR-034): edits are DISABLED during Play — the authored scene must not change
                // while running (deliverable 4; the React UI also disables the edit affordances, and
                // Stop restores from the snapshot regardless, but this is the authoritative backstop on
                // the primary edit path).
                if play_mode {
                    continue;
                }
                let delta = apply_edit(&mut engine, &tx);
                let ok = delta.rejects.is_empty();
                if let Some(ch) = &channel {
                    send_proj!(ch, delta);
                }
                let mut edited_phys: Option<EntityId> = None;
                if ok {
                    // bump recency for the edited entity (SetField.id / Bind.from)
                    let (EditIntent::SetField { id, .. } | EditIntent::Bind { from: id, .. }) =
                        &tx.intent;
                    if let Some(eid) = EntityId::from_loro_key(id) {
                        if let Some(e) = engine.ecs_entity(eid) {
                            touch += 1;
                            recency.insert(e, touch);
                        }
                        // M8.4 edit-at-pause: an edit to a physics body (e.g. nudge `Collider.friction`
                        // mid-scrub) is one undoable commit (this pipeline) AND must re-derive the run so
                        // a resume re-simulates deterministically from the edited value.
                        let comps = engine.components_of(eid);
                        if comps.contains_key("RigidBody") && comps.contains_key("Collider") {
                            edited_phys = Some(eid);
                        }
                    }
                    log.append(&Record::Edit(tx)); // persist the committed edit
                }
                rebuild(&engine, &shared, &mut positions, &assets);
                if edited_phys.is_some() {
                    // Re-derive the run from the edited value, capturing the CURRENT state of every body as
                    // the new frame-0 (no jump) — so resuming re-simulates deterministically with the new
                    // friction/mass from here. Stays PAUSED (the user nudged mid-scrub; they resume to see
                    // the effect). The M8.3 mistake-checks re-run when the UI calls `physics_check` after.
                    (recording, rec_entities, sim, body_of) =
                        restart_run(&engine, &assets, &sim, &body_of);
                    frame = 0;
                    max_frame = 0;
                    sim_running = false;
                    prev_centers = sync_out(&sim, &body_of, &shared);
                    if overlays_on {
                        push_overlay(&sim, &body_of, &prev_centers, &shared);
                    }
                }
            }
            EngineCmd::Undo { reply } => {
                let did = engine.undo();
                if did {
                    log.append(&Record::Undo); // persist the undo so replay reproduces the net state
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene)); // simplest correct post-undo sync
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                    // The ECS is the single authority over which bodies EXIST — restart the run from the
                    // post-undo ECS. This covers BOTH undo of a spawn (the entity is gone → its body
                    // drops out) AND undo of make-dynamic (the entity REMAINS but lost its RigidBody/
                    // Collider → it's no longer a body). Surviving bodies keep their current simulated
                    // state (restart_run captures it), so undo doesn't snap the rest of the scene back.
                    (recording, rec_entities, sim, body_of) =
                        restart_run(&engine, &assets, &sim, &body_of);
                    frame = 0;
                    max_frame = 0;
                    if body_of.is_empty() {
                        sim_running = false;
                    }
                }
                let _ = reply.send(did);
            }
            EngineCmd::Reveal { id, reply } => {
                let resp = EntityId::from_loro_key(&id)
                    .map(|eid| compute_reveal(&engine, &scene, &positions, &recency, eid))
                    .unwrap_or_default();
                let _ = reply.send(resp);
            }
            EngineCmd::Bind { from, to } => {
                if let (Some(f), Some(t)) =
                    (EntityId::from_loro_key(&from), EntityId::from_loro_key(&to))
                {
                    if capscene::bind(&mut engine, &scene, f, t).is_ok() {
                        // bump recency for both endpoints of the bind
                        for id in [f, t] {
                            if let Some(e) = engine.ecs_entity(id) {
                                touch += 1;
                                recency.insert(e, touch);
                            }
                        }
                        log.append(&Record::Bind {
                            from: from.clone(),
                            to: to.clone(),
                        }); // persist the bind
                        if let Some(ch) = &channel {
                            // echo the new edge so the projection (and a reload) carries it
                            send_proj!(
                                ch,
                                ProjectionDelta {
                                    ops: vec![metrocalk_editor_shell::ProjectionOp::AddEdge {
                                        from: from.clone(),
                                        rel: capscene::TRACKS.to_string(),
                                        to: to.clone(),
                                    }],
                                    confirms: vec![],
                                    rejects: vec![],
                                    full: false,
                                }
                            );
                        }
                        rebuild(&engine, &shared, &mut positions, &assets);
                    }
                }
            }
            EngineCmd::Describe { query, reply } => {
                // Tiered resolve: local (offline) first; only on a no-local-match query the marketplace
                // index; nothing anywhere → the generate seam (unbuilt stub). Each tier instantiates a
                // pre-componentized working object as one undoable, replay-persisted transaction.
                let pos = [0.0; 3];
                let resp = if let Some((id, kind)) =
                    capscene::describe_create(&mut engine, &scene, &query, pos, &assets.catalog)
                {
                    log.append(&Record::Describe {
                        query: query.clone(),
                        pos,
                    });
                    echo_created(
                        &mut engine,
                        &shared,
                        &mut positions,
                        &assets,
                        &channel,
                        &mut recency,
                        &mut touch,
                        id,
                    );
                    DescribeResponse {
                        created: Some(id.to_loro_key()),
                        kind: Some(kind),
                        source: Some("local".into()),
                        price: None,
                        seam: None,
                        balance: Some(wallet.balance_tokens()),
                    }
                } else if let Some(m) = market.query(&query).into_iter().next() {
                    // Marketplace tier (M7): a pre-componentized entry, BOUGHT — debit the price (2–4) +
                    // accrue ~70% to the creator (its id namespace) on success, or refuse gracefully when
                    // broke (an honest "top up?", no scene change). The mesh handle resolves first.
                    let entry = m.entry;
                    let mesh = entry
                        .asset
                        .as_deref()
                        .and_then(|name| assets.asset_by_name.get(name).cloned());
                    let ref_id = format!("buy:{}:{}", entry.id, wallet.ledger().len());
                    let (created, outcome) = buy_marketplace(
                        &mut engine,
                        &scene,
                        &mut wallet,
                        &entry,
                        mesh.as_deref(),
                        pos,
                        &ref_id,
                    );
                    match (created, outcome) {
                        (Some(id), Outcome::Charged { balance_tokens, .. }) => {
                            log.append(&Record::ApplyMarketplace {
                                entry_id: entry.id.clone(),
                                pos,
                                mesh,
                            });
                            echo_created(
                                &mut engine,
                                &shared,
                                &mut positions,
                                &assets,
                                &channel,
                                &mut recency,
                                &mut touch,
                                id,
                            );
                            DescribeResponse {
                                created: Some(id.to_loro_key()),
                                kind: Some(entry.component.clone()),
                                source: Some("marketplace".into()),
                                price: entry.price,
                                seam: None,
                                balance: Some(balance_tokens),
                            }
                        }
                        (_, Outcome::Refused { needed, have }) => DescribeResponse {
                            kind: Some(entry.component.clone()),
                            source: Some("marketplace".into()),
                            price: entry.price,
                            seam: Some(format!(
                                "insufficient balance: this asset costs {needed} tokens, you have {have} — top up?"
                            )),
                            balance: Some(have),
                            ..Default::default()
                        },
                        (_, Outcome::Rejected(why)) => DescribeResponse {
                            seam: Some(why),
                            balance: Some(wallet.balance_tokens()),
                            ..Default::default()
                        },
                        _ => DescribeResponse::default(),
                    }
                } else {
                    // No match anywhere — the generate seam (the opt-in tier-3 metered last resort).
                    DescribeResponse {
                        seam: Some("generate".into()),
                        balance: Some(wallet.balance_tokens()),
                        ..Default::default()
                    }
                };
                let _ = reply.send(resp);
            }
            EngineCmd::Actions { id, reply } => {
                let items = EntityId::from_loro_key(&id)
                    .map(|e| actions_for(&engine, &scene, e))
                    .unwrap_or_default();
                let _ = reply.send(items);
            }
            EngineCmd::Remove { id } => {
                if let Some(e) = EntityId::from_loro_key(&id) {
                    // Capture the edges this entity participates in BEFORE removing, for the targeted
                    // re-projection (inv. 2: Remove(id) + RemoveEdge per freed binding — not a full reload).
                    let removed_edges: Vec<(String, String, String)> = engine
                        .bindings()
                        .into_iter()
                        .filter(|(from, _, to)| *from == e || *to == e)
                        .map(|(from, kind, to)| (from.to_loro_key(), kind, to.to_loro_key()))
                        .collect();
                    if capscene::remove_entity(&mut engine, &scene, e).is_ok() {
                        // Drop any physics body tracked for this entity (audit, medium): a stale `body_of`
                        // entry would keep simulating an invisible body + skew the body count after delete.
                        body_of.remove(&e);
                        log.append(&Record::Remove { id: id.clone() });
                        if let Some(ch) = &channel {
                            let mut ops = vec![ProjectionOp::Remove { id: id.clone() }];
                            for (from, rel, to) in removed_edges {
                                ops.push(ProjectionOp::RemoveEdge { from, rel, to });
                            }
                            send_proj!(
                                ch,
                                ProjectionDelta {
                                    ops,
                                    confirms: vec![],
                                    rejects: vec![],
                                    full: false,
                                }
                            );
                        }
                        rebuild(&engine, &shared, &mut positions, &assets);
                    }
                }
            }
            EngineCmd::Duplicate { id, reply } => {
                let new = EntityId::from_loro_key(&id)
                    .and_then(|e| capscene::duplicate_entity(&mut engine, &scene, e).ok());
                if let Some(new_id) = new {
                    log.append(&Record::Duplicate { source: id.clone() });
                    echo_created(
                        &mut engine,
                        &shared,
                        &mut positions,
                        &assets,
                        &channel,
                        &mut recency,
                        &mut touch,
                        new_id,
                    );
                }
                let _ = reply.send(new.map(|n| n.to_loro_key()));
            }
            // ── M10.6 scene-authoring verbs (ADR-036) — each one undoable commit → re-project the scene.
            // (Verbs persist via the M10.3 `.mtk` save/open — the Loro doc; the in-session replay-log
            // Records for these are a tracked follow-up.) ──
            EngineCmd::CreateEntity {
                x,
                y,
                z,
                name,
                reply,
            } => {
                let new = capscene::create_entity(&mut engine, [x, y, z], &name).ok();
                if new.is_some() {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(new.map(|n| n.to_loro_key()));
            }
            EngineCmd::AddLight {
                kind,
                pos,
                color,
                intensity,
                reply,
            } => {
                // M11.3 — author a Light entity (one undoable commit, persisted). rebuild() re-collects the
                // scene lights so the new light affects shading immediately (a render projection).
                let new =
                    capscene::add_light(&mut engine, &scene, &kind, pos, color, intensity).ok();
                if new.is_some() {
                    log.append(&Record::AddLight {
                        light_kind: kind.clone(),
                        pos,
                        color,
                        intensity,
                    });
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(new.map(|n| n.to_loro_key()));
            }
            EngineCmd::LightingDebug { reply } => {
                // The lighting truth the acceptance gate keys off (stable signal, not pixels): how many
                // lights are AUTHORED (doc/undo truth), what the render sees (incl. the synthesized default
                // when empty), and which light casts the shadow (index + kind). Reuses `collect_lights` —
                // the same projection the shader consumes — so the gate asserts the real render result.
                let authored = engine
                    .entity_ids()
                    .iter()
                    .filter(|id| engine.components_of(**id).contains_key("Light"))
                    .count();
                let (lights, caster) = collect_lights(&engine);
                let caster_kind = caster
                    .and_then(|i| lights.get(i))
                    .map_or(-1, |l| l.pos_kind[3] as i64);
                let shadow_caster = caster.map_or(-1, |i| i as i64);
                let _ = reply.send((authored, lights.len(), shadow_caster, caster_kind));
            }
            EngineCmd::AddCamera {
                pos,
                fov,
                active,
                reply,
            } => {
                // M11.4 — author a scene Camera entity (one undoable commit, persisted).
                let new = capscene::add_camera(&mut engine, &scene, pos, fov, active).ok();
                if new.is_some() {
                    log.append(&Record::AddCamera { pos, fov, active });
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(new.map(|n| n.to_loro_key()));
            }
            EngineCmd::LookThrough { on, reply } => {
                // Snapshot the active scene camera's view into the render override (read every frame in the
                // camera block) — a projection, never Loro/undo (ADR-021). `on=false` → back to the fly-cam.
                let found = if on {
                    if let Some((p, fov, near, far)) = capscene::active_camera(&engine) {
                        shared.lock().unwrap().cam_override = Some(render::CamView {
                            pos: p,
                            fov_deg: fov,
                            near,
                            far,
                        });
                        true
                    } else {
                        false
                    }
                } else {
                    shared.lock().unwrap().cam_override = None;
                    true
                };
                let _ = reply.send(found);
            }
            EngineCmd::CameraDebug { reply } => {
                let count = engine
                    .entity_ids()
                    .iter()
                    .filter(|id| engine.components_of(**id).contains_key("Camera"))
                    .count();
                let active = capscene::active_camera(&engine);
                let fov = active.map_or(-1.0, |(_, f, _, _)| f);
                let _ = reply.send((count, active.is_some(), fov));
            }
            EngineCmd::RenameEntity { id, name, reply } => {
                let ok = EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::rename(&mut engine, e, &name).is_ok());
                if ok {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                }
                let _ = reply.send(ok);
            }
            EngineCmd::GroupEntities { ids, name, reply } => {
                let members: Vec<EntityId> = ids
                    .iter()
                    .filter_map(|s| EntityId::from_loro_key(s))
                    .collect();
                let g = if members.is_empty() {
                    None
                } else {
                    capscene::group(&mut engine, &members, &name).ok()
                };
                if g.is_some() {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(g.map(|n| n.to_loro_key()));
            }
            EngineCmd::UngroupEntity { id, reply } => {
                let ok = EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::ungroup(&mut engine, e).is_ok());
                if ok {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(ok);
            }
            EngineCmd::MultiEdit {
                ids,
                component,
                field,
                value,
                reply,
            } => {
                let targets: Vec<EntityId> = ids
                    .iter()
                    .filter_map(|s| EntityId::from_loro_key(s))
                    .collect();
                let ok = !targets.is_empty()
                    && capscene::multi_edit(
                        &mut engine,
                        &targets,
                        &component,
                        &field,
                        &FieldValue::Number(value),
                    )
                    .is_ok();
                if ok {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(ok);
            }
            EngineCmd::DeleteDeactivate { id, reply } => {
                let ok = EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::delete_deactivate(&mut engine, &scene, e).is_ok());
                if ok {
                    // Persist the deactivate so it SURVIVES reload (R-NEXT-2) — replay re-runs it, and
                    // `project_full` then re-emits `active:false` so the hierarchy dims the row on reopen.
                    log.append(&Record::DeleteDeactivate { id: id.clone() });
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(ok);
            }
            EngineCmd::CopySubtree { id } => {
                clipboard = EntityId::from_loro_key(&id)
                    .map(|e| capscene::copy_subtree(&engine, e, "clipboard"));
            }
            EngineCmd::CutSubtree { id, reply } => {
                let ok = if let Some(e) = EntityId::from_loro_key(&id) {
                    match capscene::cut_subtree(&mut engine, &scene, e, "clipboard") {
                        Ok(c) => {
                            clipboard = Some(c);
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    false
                };
                if ok {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(ok);
            }
            EngineCmd::PasteClipboard { reply } => {
                let new = clipboard
                    .as_ref()
                    .and_then(|c| capscene::paste_composition(&mut engine, c).ok());
                if new.is_some() {
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(new.map(|n| n.to_loro_key()));
            }
            EngineCmd::ImportAsset { path, reply } => {
                // M11.1 — drop any file → a working asset. Read the file, route by MAGIC (glTF/OBJ/FBX/
                // PNG/…), register its GPU mesh if new, persist the bytes by content address (survives
                // reload — the M6/M11.1 residual), place an entity carrying the handle, and reply its id.
                // An unsupported/malformed file → `None` (the React surface explains it).
                let mut result = None;
                if let Ok(bytes) = std::fs::read(&path) {
                    if metrocalk_editor_shell::is_cad_file(&bytes) {
                        // M15.7 (ADR-077) — a CAD container (CATIA 3DXML / STEP AP242): the never-empty,
                        // never-silent pipeline lands each part as a renderable entity in one undoable commit
                        // (proxies for proprietary geometry the licensed kernel would decode, at real transforms).
                        if let Some(root) =
                            land_cad(&bytes, &mut engine, &scene, &mut assets, &shared)
                        {
                            rebuild(&engine, &shared, &mut positions, &assets);
                            if let Some(ch) = &channel {
                                send_proj!(ch, proj_full(&engine, &scene));
                            }
                            result = Some(root.to_loro_key());
                        }
                    } else if let Ok(metrocalk_assets::ImportedAsset::Mesh(asset)) =
                        metrocalk_assets::import_any(&bytes)
                    {
                        let handle = AssetId::of_bytes(&bytes).as_str().to_string();
                        if !assets.handle_to_slot.contains_key(&handle) {
                            // Normalise the imported geometry to ~1 unit, centred (FBX/glTF are often authored
                            // in cm, hundreds of units across). The renderer applies `Transform.scale`
                            // directly to these verts, so this makes `scale` an intuitive world-size
                            // multiplier (1.0 ≈ one unit) instead of `0.9/extent`-tiny; the collider reads
                            // the SAME verts (`mesh_geometry`) so it stays matched + centred on the entity.
                            let mut gpu = MeshGpu::from_asset(&asset);
                            gpu.normalize_to_unit();
                            let slot = assets.meshes.len();
                            assets.meshes.push(gpu.clone());
                            assets.scales.push(1.0);
                            assets.handle_to_slot.insert(handle.clone(), slot);
                            let mut st = shared.lock().unwrap();
                            st.meshes.push(gpu);
                            st.meshes_revision = st.meshes_revision.wrapping_add(1);
                        }
                        // M11.5 (ADR-044) — record the asset's provenance keyed by its content address, and
                        // surface a near-duplicate HINT: a perceptual-hash match against an already-imported
                        // asset with DIFFERENT bytes (a rescaled/recompressed copy the exact dedup misses).
                        // Never a silent merge — the exact-dedup above already collapsed identical bytes.
                        let source = std::path::Path::new(&path)
                            .file_name()
                            .and_then(|s| s.to_str())
                            .unwrap_or(path.as_str())
                            .to_string();
                        let phash = asset
                            .textures
                            .first()
                            .map_or(0, metrocalk_assets::perceptual_hash);
                        if phash != 0 {
                            if let Some(dup_of) = assets
                                .provenance
                                .values()
                                .filter(|p| p.content_hash != handle && p.perceptual_hash != 0)
                                .find(|p| {
                                    metrocalk_assets::is_near_duplicate(
                                        phash,
                                        p.perceptual_hash,
                                        10,
                                    )
                                })
                                .map(|p| p.source.clone())
                            {
                                eprintln!(
                                    "[shell] import: '{source}' looks like a near-duplicate of \
                                     '{dup_of}' (perceptual-hash match) — kept as a distinct asset"
                                );
                            }
                        }
                        assets.provenance.insert(
                            handle.clone(),
                            metrocalk_assets::Provenance::imported(source, handle.clone(), phash),
                        );
                        // Persist the bytes content-addressed so the handle re-resolves on reload (audit F3):
                        // log a write failure rather than swallow it — else the imported mesh silently
                        // vanishes after restart (the saved handle dangles).
                        if let Err(e) = metrocalk_editor_shell::blobstore::put(
                            &sidecar("metrocalk-assets"),
                            &bytes,
                        ) {
                            eprintln!(
                                "[shell] import: failed to persist asset bytes — it may not survive reload: {e}"
                            );
                        }
                        // M11.1 — lay successive imports out on a grid so they don't stack on the same spot
                        // (the persisted record keeps the chosen pos, so reload restores the same layout).
                        let pos = next_import_pos(&engine);
                        if let Ok(id) = capscene::place_mesh(&mut engine, &scene, &handle, pos) {
                            log.append(&Record::PlaceMesh {
                                asset: handle.clone(),
                                pos,
                            });
                            echo_created(
                                &mut engine,
                                &shared,
                                &mut positions,
                                &assets,
                                &channel,
                                &mut recency,
                                &mut touch,
                                id,
                            );
                            result = Some(id.to_loro_key());
                        }
                    }
                }
                let _ = reply.send(result);
            }
            EngineCmd::Details { id, reply } => {
                let details = EntityId::from_loro_key(&id)
                    .and_then(|e| build_entity_details(&engine, &scene, e));
                let _ = reply.send(details);
            }
            EngineCmd::AssetProvenance { id, reply } => {
                let info = EntityId::from_loro_key(&id)
                    .and_then(|e| asset_provenance_of(&assets, &engine, e));
                let _ = reply.send(info);
            }
            EngineCmd::ListRules { reply } => {
                let out = engine
                    .rules()
                    .into_iter()
                    .map(|(id, r)| RuleSummary {
                        id: id.as_str().to_string(),
                        name: r.name,
                        enabled: r.enabled,
                        event: r.event,
                        condition_count: r.conditions.len(),
                        action_count: r.actions.len(),
                    })
                    .collect();
                let _ = reply.send(out);
            }
            EngineCmd::AuthorRule { rule, id, reply } => {
                // Registry-validate FIRST (typo-proof, Blocked + explained — ADR-016): an unknown event/
                // component/field/action or a wrong-typed value is refused with a plain-language reason and
                // never committed. Only a valid rule becomes one undoable `SetRule` (+ a replay Record so it
                // survives reload), and the engine offers its mirror "cleanup" rule for the UI to surface.
                let resp = match metrocalk_core::validate_rule(&rules_registry, &rule, |e| {
                    engine.entity_exists(e)
                }) {
                    Err(e) => AuthorRuleResult {
                        error: Some(e.to_string()),
                        ..Default::default()
                    },
                    Ok(()) => {
                        let rule_id = id
                            .map(metrocalk_core::RuleId::new)
                            .unwrap_or_else(|| engine.alloc_rule_id());
                        let mirror = metrocalk_core::propose_mirror(&rule);
                        match engine.commit(
                            "author rule",
                            vec![metrocalk_core::Op::SetRule {
                                id: rule_id.clone(),
                                rule: rule.clone(),
                            }],
                        ) {
                            Ok(()) => {
                                log.append(&Record::AuthorRule {
                                    id: rule_id.as_str().to_string(),
                                    rule,
                                });
                                AuthorRuleResult {
                                    id: Some(rule_id.as_str().to_string()),
                                    mirror,
                                    ..Default::default()
                                }
                            }
                            Err(e) => AuthorRuleResult {
                                error: Some(e.to_string()),
                                ..Default::default()
                            },
                        }
                    }
                };
                let _ = reply.send(resp);
            }
            EngineCmd::DeleteRule { id, reply } => {
                let ok = engine
                    .commit(
                        "remove rule",
                        vec![metrocalk_core::Op::RemoveRule {
                            id: metrocalk_core::RuleId::new(id.clone()),
                        }],
                    )
                    .is_ok();
                if ok {
                    log.append(&Record::RemoveRule { id });
                }
                let _ = reply.send(ok);
            }
            EngineCmd::ListStateMachines { reply } => {
                let out = engine
                    .state_machines()
                    .into_iter()
                    .map(|(id, m)| StateMachineInfo {
                        current: engine
                            .state_machine_current(&id)
                            .unwrap_or_else(|| m.initial.clone()),
                        id: id.as_str().to_string(),
                        machine: m,
                    })
                    .collect();
                let _ = reply.send(out);
            }
            EngineCmd::AuthorStateMachine { mut sm, id, reply } => {
                // Stamp a peer-namespaced stable id onto any NEW (empty-id) transition before validating —
                // so the React Flow edge ids are server-allocated + collision-free (the e2e keys off them).
                for t in &mut sm.transitions {
                    if t.id.trim().is_empty() {
                        t.id = engine.alloc_transition_id();
                    }
                }
                // Validate FIRST (typo-proof, no-dangling, transition-is-a-Rule — Blocked + explained,
                // ADR-016): a dangling/typo'd machine is refused with a plain-language reason and never
                // committed. Reachability is a WARNING (the unreachable list), not a rejection. Only a valid
                // machine becomes one undoable `SetStateMachine` (+ a replay Record so it survives reload).
                let resp = match metrocalk_core::validate_state_machine(&rules_registry, &sm, |e| {
                    engine.entity_exists(e)
                }) {
                    Err(e) => AuthorStateMachineResult {
                        error: Some(e.to_string()),
                        ..Default::default()
                    },
                    Ok(report) => {
                        let sm_id = id
                            .map(metrocalk_core::StateMachineId::new)
                            .unwrap_or_else(|| engine.alloc_state_machine_id());
                        match engine.commit(
                            "author state machine",
                            vec![metrocalk_core::Op::SetStateMachine {
                                id: sm_id.clone(),
                                sm: sm.clone(),
                            }],
                        ) {
                            Ok(()) => {
                                log.append(&Record::AuthorStateMachine {
                                    id: sm_id.as_str().to_string(),
                                    machine: sm,
                                });
                                AuthorStateMachineResult {
                                    id: Some(sm_id.as_str().to_string()),
                                    unreachable: report.unreachable,
                                    ..Default::default()
                                }
                            }
                            Err(e) => AuthorStateMachineResult {
                                error: Some(e.to_string()),
                                ..Default::default()
                            },
                        }
                    }
                };
                let _ = reply.send(resp);
            }
            EngineCmd::DeleteStateMachine { id, reply } => {
                let ok = engine
                    .commit(
                        "remove state machine",
                        vec![metrocalk_core::Op::RemoveStateMachine {
                            id: metrocalk_core::StateMachineId::new(id.clone()),
                        }],
                    )
                    .is_ok();
                if ok {
                    log.append(&Record::RemoveStateMachine { id });
                }
                let _ = reply.send(ok);
            }
            EngineCmd::RunPlugin { name, input, reply } => {
                // Run the sandboxed plugin; its effect is applied through the ONE commit pipeline (undoable)
                // by `run_plugin` via the ADR-017 patch contract. On success: echo the delta to the viewport
                // (inv. 2) + persist a replay record (so a plugin-driven change survives reload). A rejected
                // effect (the plugin reached past validation) or a contained run failure (trap/timeout/budget/
                // bad output) replies an explained error — never an engine crash.
                let resp = match metrocalk_editor_shell::plugin_host::run_plugin(
                    &mut engine,
                    &metrocalk_core::stdlib::standard_components(),
                    &name,
                    &input,
                ) {
                    Ok(delta) if delta.rejects.is_empty() => {
                        let applied = delta.ops.len();
                        if let Some(ch) = &channel {
                            send_proj!(ch, delta);
                        }
                        rebuild(&engine, &shared, &mut positions, &assets);
                        log.append(&Record::RunPlugin {
                            name: name.clone(),
                            input,
                        });
                        RunPluginResult {
                            ok: true,
                            applied,
                            error: None,
                        }
                    }
                    Ok(delta) => RunPluginResult {
                        ok: false,
                        applied: 0,
                        error: delta.rejects.first().map(|r| r.reason.clone()),
                    },
                    Err(e) => RunPluginResult {
                        ok: false,
                        applied: 0,
                        error: Some(e.to_string()),
                    },
                };
                let _ = reply.send(resp);
            }
            EngineCmd::ProposeComposition {
                sentence,
                target,
                reply,
            } => {
                // The in-app AI compose seam: the composer PROPOSES (offline demo / real-LLM seam); the
                // engine then VALIDATES the proposal against the live scene (the ADR-017 gate) so even the
                // preview is pre-checked — a proposal that wouldn't apply is surfaced as a plain-language
                // reason NOW, before the user hits Apply. Nothing is committed here.
                let grammar = metrocalk_core::composition_grammar(
                    &metrocalk_core::stdlib::standard_components(),
                );
                let resp = match composer.propose(&sentence, target.as_deref(), &grammar) {
                    Ok(comp) => {
                        match metrocalk_core::validate_composition(&rules_registry, &comp, |e| {
                            engine.entity_exists(e)
                        }) {
                            Ok(()) => ComposeProposal {
                                ok: true,
                                ops: comp.ops.len(),
                                composition: serde_json::to_value(&comp).ok(),
                                error: None,
                            },
                            Err(e) => ComposeProposal {
                                ok: false,
                                error: Some(e.to_string()),
                                ..Default::default()
                            },
                        }
                    }
                    Err(e) => ComposeProposal {
                        ok: false,
                        error: Some(e.to_string()),
                        ..Default::default()
                    },
                };
                let _ = reply.send(resp);
            }
            EngineCmd::Compose { composition, reply } => {
                // Apply a reviewed composition through the ONE commit pipeline (one undoable tx, all-or-
                // nothing, rejected-as-UX). The SAME validated path a human / plugin uses — the AI is never a
                // raw mutation. On success: echo the full projection (a SetField can move/retexture an
                // entity) + rebuild the shared scene + persist a `Compose` replay record (survives reload).
                let resp = match metrocalk_core::apply_composition(
                    &mut engine,
                    &rules_registry,
                    &composition,
                ) {
                    Ok(()) => {
                        let applied = composition.ops.len();
                        rebuild(&engine, &shared, &mut positions, &assets);
                        if let Some(ch) = &channel {
                            send_proj!(ch, proj_full(&engine, &scene)); // simplest correct post-compose sync
                        }
                        log.append(&Record::Compose { composition });
                        ComposeResult {
                            ok: true,
                            applied,
                            rules: engine.rules().len(),
                            state_machines: engine.state_machines().len(),
                            error: None,
                        }
                    }
                    Err(e) => ComposeResult {
                        ok: false,
                        error: Some(e.to_string()),
                        ..Default::default()
                    },
                };
                let _ = reply.send(resp);
            }
            EngineCmd::FireRuleEvent {
                event,
                subject,
                selected,
                reply,
            } => {
                // M12.5 (ADR-049) — fire a live gameplay event into the running Rules (the When-channel).
                // ONLY in Play, and ONLY a PROJECTION: it advances the `RuleReplay`'s runtime state + decision
                // history, never the ECS/Loro doc (ADR-021/034). Recorded into the Play recording so a later
                // scrub replays it deterministically (M8.4). A no-op (empty info) when not playing.
                if let Some(session) = rule_session.as_mut() {
                    rule_head = session.fire(event, subject);
                }
                let _ = reply.send(rule_debug_info(
                    play_mode,
                    rule_session.as_ref(),
                    &rule_flagged,
                    rule_head,
                    selected.as_deref(),
                ));
            }
            EngineCmd::RuleDebug { id, reply } => {
                // The "debug by looking" read (test #5 box 3): the clicked entity's live truth-state + the
                // decision history. A non-mutating projection over the runtime state (the M8.4 overlay
                // discipline — reading it never perturbs the run).
                let _ = reply.send(rule_debug_info(
                    play_mode,
                    rule_session.as_ref(),
                    &rule_flagged,
                    rule_head,
                    id.as_deref(),
                ));
            }
            EngineCmd::RuleScrub {
                frame: target,
                selected,
                reply,
            } => {
                // Scrub the decision history over the M8.4 channel (rewind = rebuild-from-recording, then
                // replay forward — deterministic-by-rebuild) and reply the truth-state at that frame (box 4).
                if let Some(session) = rule_session.as_mut() {
                    session.seek(target.min(rule_head));
                }
                let _ = reply.send(rule_debug_info(
                    play_mode,
                    rule_session.as_ref(),
                    &rule_flagged,
                    rule_head,
                    selected.as_deref(),
                ));
            }
            EngineCmd::Generate { query, reply } => {
                // Tier 3, opt-in. Offline/unconfigured → an honest seam, no placeholder, never a fake
                // asset (contrast the available path below: meter, then drop the placeholder).
                if !generator.available() {
                    let _ = reply.send(GenerateResponse {
                        available: false,
                        seam: Some("generation unavailable offline".into()),
                        ..Default::default()
                    });
                } else {
                    // Tier 3, opt-in. RESERVE the cost up front (M7) — fences the tokens the instant the
                    // request is accepted (defeats free-tier-via-race), refuses gracefully when broke
                    // BEFORE any placeholder drops, and is only SETTLED on a successful stream-in (a
                    // failed generation RELEASES the hold — never charged for a failure).
                    let ref_id = format!("gen:{}:{}", query, wallet.ledger().len());
                    let resp = match wallet.reserve_generate(&ref_id) {
                        Err(refusal) => GenerateResponse {
                            available: true,
                            seam: Some(format!(
                                "insufficient balance: a generation costs {} tokens, you have {} — top up?",
                                refusal.needed.whole_tokens(),
                                refusal.available.whole_tokens()
                            )),
                            balance: Some(wallet.balance_tokens()),
                            ..Default::default()
                        },
                        Ok(hold) => {
                            match capscene::place_generation_placeholder(&mut engine, &scene, [0.0; 3])
                            {
                                Ok(ph) => {
                                    let ph_key = ph.to_loro_key();
                                    pending_gen.insert(ph_key.clone(), hold);
                                    echo_created(
                                        &mut engine,
                                        &shared,
                                        &mut positions,
                                        &assets,
                                        &channel,
                                        &mut recency,
                                        &mut touch,
                                        ph,
                                    );
                                    // Kick the provider off the hot path. It ALWAYS sends back exactly one
                                    // terminal message — GenerateComplete on success, GenerateFailed on a
                                    // provider Err OR a panic (caught) — so the reservation never leaks.
                                    let g = generator.clone();
                                    let back = self_tx.clone();
                                    let q = query.clone();
                                    let pk = ph_key.clone();
                                    std::thread::spawn(move || {
                                        let produced = std::panic::catch_unwind(
                                            std::panic::AssertUnwindSafe(|| {
                                                g.generate(&GenRequest::new(q.clone()))
                                            }),
                                        );
                                        let msg = match produced {
                                            Ok(Ok(bytes)) => EngineCmd::GenerateComplete {
                                                placeholder: pk,
                                                prompt: q,
                                                bytes,
                                            },
                                            Ok(Err(e)) => EngineCmd::GenerateFailed {
                                                placeholder: pk,
                                                reason: e.to_string(),
                                            },
                                            Err(_) => EngineCmd::GenerateFailed {
                                                placeholder: pk,
                                                reason: "generation worker panicked".to_string(),
                                            },
                                        };
                                        let _ = back.send(msg);
                                    });
                                    GenerateResponse {
                                        created: Some(ph_key),
                                        cost: Some(GENERATE_TOKENS),
                                        available: true,
                                        seam: None,
                                        // AVAILABLE (settled − the open hold), not the holds-blind balance: the
                                        // reserve fences the cost up front, so the spendable balance the user
                                        // sees must reflect the charge AT THE GESTURE (the legible-cost
                                        // contract, M7/M10.10) — not jump only when the async settle lands.
                                        balance: Some(wallet.available_tokens()),
                                    }
                                }
                                // Couldn't place the placeholder → release the reservation (no charge).
                                Err(_) => {
                                    wallet.release(hold, &ref_id);
                                    GenerateResponse::default()
                                }
                            }
                        }
                    };
                    let _ = reply.send(resp);
                }
            }
            EngineCmd::GenerateComplete {
                placeholder,
                prompt,
                bytes,
            } => {
                // The reservation for this placeholder (None if it was already made terminal).
                let hold = pending_gen.remove(&placeholder);
                let gen_ref = format!("gen-done:{placeholder}");
                let placeholder_live =
                    EntityId::from_loro_key(&placeholder).is_some_and(|e| engine.entity_exists(e));
                if !placeholder_live {
                    // The placeholder was undone/removed while generating → refund the hold, drop the
                    // result (never charge for an asset the user already discarded).
                    if let Some(h) = hold {
                        wallet.release(h, &gen_ref);
                    }
                } else {
                    // Import the generated mesh through the prompt-23 pipeline + add it to the render
                    // store if it's new (content-addressed; the fake's prop is idempotent). A handle is
                    // only USABLE once it resolves to imported geometry — a malformed/oversized mesh is
                    // REJECTED, so we never stream in (or persist) a dangling handle.
                    let handle = AssetId::of_bytes(&bytes).as_str().to_string();
                    let mut usable = assets.handle_to_slot.contains_key(&handle);
                    if !usable {
                        if let Ok(asset) = GltfImporter::new().import(&bytes) {
                            // M11.1 — persist the generated bytes by content address so this handle
                            // re-resolves after reload (the M6 residual). Persistence is a PREREQUISITE
                            // (audit F3): if the write fails the handle would dangle on reload, so we do NOT
                            // register / stream it in (and the charge below never settles) — the generation
                            // fails cleanly + visibly instead of charging for an asset that vanishes.
                            match metrocalk_editor_shell::blobstore::put(
                                &sidecar("metrocalk-assets"),
                                &bytes,
                            ) {
                                Ok(_) => {
                                    // Normalise to ~1 unit, centred (see the import path) so the generated
                                    // mesh's `scale` is an intuitive world-size multiplier + collider matched.
                                    let mut gpu = MeshGpu::from_asset(&asset);
                                    gpu.normalize_to_unit();
                                    let slot = assets.meshes.len();
                                    assets.meshes.push(gpu.clone());
                                    assets.scales.push(1.0);
                                    assets.handle_to_slot.insert(handle.clone(), slot);
                                    let mut st = shared.lock().unwrap();
                                    st.meshes.push(gpu);
                                    st.meshes_revision = st.meshes_revision.wrapping_add(1);
                                    usable = true;
                                }
                                Err(e) => eprintln!(
                                    "[generate] failed to persist generated mesh — not streaming it in: {e}"
                                ),
                            }
                        }
                    }
                    // Stream the mesh in as a VALIDATED AI patch (inv. 3) — same entity, swapped handle.
                    // ECHO it to the viewport but do NOT persist the scene yet (that waits on the charge).
                    let applied = usable && {
                        let patch = AiPatch {
                            client_op_id: "generate-stream-in".into(),
                            ops: vec![PatchOp::SetField {
                                id: placeholder.clone(),
                                component: "MeshRenderer".into(),
                                field: capscene::MESH_FIELD.into(),
                                value: serde_json::Value::String(handle.clone()),
                            }],
                        };
                        let delta = apply_ai_patch(
                            &mut engine,
                            &metrocalk_core::stdlib::standard_components(),
                            "generate-stream-in",
                            &patch,
                        );
                        let ok = delta.rejects.is_empty();
                        if ok {
                            if let Some(ch) = &channel {
                                send_proj!(ch, delta); // targeted stream-in delta (inv. 2)
                            }
                            rebuild(&engine, &shared, &mut positions, &assets);
                        }
                        ok
                    };
                    if applied {
                        // SUCCESS. Persist the WALLET first (settle = charge ≈10, platform revenue), then
                        // the SCENE — and persist the scene ONLY if the charge stuck. So a crash or a
                        // wallet-write failure never leaves a generated asset persisted without its charge
                        // (no free paid tier); the worst case is an over-charge with the asset un-persisted
                        // (refundable). The two-log seam errs toward the user/platform, never a free tier.
                        let charged = hold.is_some_and(|h| wallet.settle(h, &gen_ref));
                        if charged {
                            log.append(&Record::Generate {
                                prompt,
                                pos: [0.0; 3],
                                mesh: Some(handle),
                            });
                        } else {
                            // Couldn't settle (hold lost / write failed) → refund any hold and do NOT
                            // persist a free generation (it showed this session; won't survive reload).
                            if let Some(h) = hold {
                                wallet.release(h, &gen_ref);
                            }
                            eprintln!(
                                "[generate] could not settle the reservation — generation not persisted (refunded)"
                            );
                        }
                    } else {
                        // The importer (or the stream-in patch) rejected the result — RELEASE the hold
                        // (refund, persisted) THEN keep the honest grey placeholder as `mesh: None`
                        // (wallet before scene).
                        if let Some(h) = hold {
                            wallet.release(h, &gen_ref);
                        }
                        eprintln!(
                            "[generate] import rejected the generated mesh — keeping the grey placeholder (refunded)"
                        );
                        log.append(&Record::Generate {
                            prompt,
                            pos: [0.0; 3],
                            mesh: None,
                        });
                    }
                }
            }
            EngineCmd::GenerateFailed {
                placeholder,
                reason,
            } => {
                // The provider errored or the worker panicked — RELEASE the reservation (refund) and keep
                // the honest grey placeholder; never charged for a failure.
                if let Some(h) = pending_gen.remove(&placeholder) {
                    wallet.release(h, &format!("gen-fail:{placeholder}"));
                }
                eprintln!(
                    "[generate] provider failed ({reason}) — placeholder kept, reservation refunded"
                );
                if EntityId::from_loro_key(&placeholder).is_some_and(|e| engine.entity_exists(e)) {
                    log.append(&Record::Generate {
                        prompt: String::new(),
                        pos: [0.0; 3],
                        mesh: None,
                    });
                }
            }
            EngineCmd::AiEdit {
                id,
                material,
                reply,
            } => {
                // The live material AI-edit (M7 + M11.2): assign a named PBR material preset via a
                // schema-validated patch, metered at the edit rate (debit-on-success; a rejected patch or
                // insufficient balance never charges). `material` is the chosen preset (UI palette).
                let resp = if let Some(eid) = EntityId::from_loro_key(&id) {
                    let ref_id = format!("edit:{id}:{}", wallet.ledger().len());
                    // Supply the render material vocabulary: an unknown preset is rejected-as-UX BEFORE
                    // metering (it would otherwise charge then render unchanged — audit P1).
                    let (delta, outcome) = ai_edit_material(
                        &mut engine,
                        &mut wallet,
                        eid,
                        &ref_id,
                        &material,
                        material_preset(&material).is_some(),
                    );
                    match outcome {
                        Outcome::Charged {
                            cost_tokens,
                            balance_tokens,
                        } => {
                            if let (Some(d), Some(ch)) = (delta, &channel) {
                                send_proj!(ch, d); // echo the material edit to the inspector
                            }
                            log.append(&Record::AiEdit {
                                id: id.clone(),
                                material: material.clone(),
                            });
                            rebuild(&engine, &shared, &mut positions, &assets);
                            EconResponse {
                                ok: true,
                                balance: balance_tokens,
                                cost: Some(cost_tokens),
                                message: Some(format!("applied {material} material")),
                            }
                        }
                        Outcome::Refused { needed, have } => EconResponse {
                            ok: false,
                            balance: have,
                            cost: Some(needed),
                            message: Some(format!(
                                "insufficient balance: an edit costs {needed} tokens, you have {have} — top up?"
                            )),
                        },
                        Outcome::Rejected(why) => EconResponse {
                            ok: false,
                            balance: wallet.balance_tokens(),
                            message: Some(format!("edit rejected: {why}")),
                            ..Default::default()
                        },
                    }
                } else {
                    EconResponse {
                        ok: false,
                        balance: wallet.balance_tokens(),
                        message: Some("no such entity".to_string()),
                        ..Default::default()
                    }
                };
                let _ = reply.send(resp);
            }
            EngineCmd::TopUp { reply } => {
                // Sandbox top-up (M7): $10 ≈ 100 tokens via the payment seam — NO real money moves.
                let ref_id = format!("topup:{}", wallet.ledger().len());
                let resp = match wallet.top_up(&payments, 1000, &ref_id) {
                    Ok(granted) => EconResponse {
                        ok: true,
                        balance: wallet.balance_tokens(),
                        cost: Some(granted),
                        message: Some(format!(
                            "topped up {granted} tokens (sandbox — no real charge)"
                        )),
                    },
                    Err(e) => EconResponse {
                        ok: false,
                        balance: wallet.balance_tokens(),
                        message: Some(e.to_string()),
                        ..Default::default()
                    },
                };
                let _ = reply.send(resp);
            }
            EngineCmd::WalletInfo { reply } => {
                let _ = reply.send(EconResponse {
                    ok: true,
                    balance: wallet.balance_tokens(),
                    ..Default::default()
                });
            }
            EngineCmd::Catalog { reply } => {
                // The browse view (M3.4): ONE catalog query over the registry + the marketplace index,
                // grouped by category bucket. Pure metadata.
                let lib = metrocalk_core::stdlib::standard_components();
                let _ = reply.send(metrocalk_core::catalog::grouped(&lib, &market));
            }
            EngineCmd::CatalogSearch { query, reply } => {
                // Search (M3.4) REUSES the tiered resolver — one source, no parallel search path.
                let lib = metrocalk_core::stdlib::standard_components();
                let _ = reply.send(metrocalk_core::catalog::search(&lib, &market, &query));
            }
            EngineCmd::Add { id, source, reply } => {
                // Add = instantiate through the one pipeline (M3.4) — converges with describe: a LOCAL
                // kind is a free instantiate (`add_kind`, the same path describe uses); a MARKETPLACE
                // entry is a metered buy (`buy_marketplace`, the M7 path the marketplace describe tier uses).
                let pos = [0.0; 3];
                let resp = if source == "marketplace" {
                    match market.get(&id) {
                        Some(entry) => {
                            let mesh = entry
                                .asset
                                .as_deref()
                                .and_then(|name| assets.asset_by_name.get(name).cloned());
                            let ref_id = format!("add-buy:{}:{}", entry.id, wallet.ledger().len());
                            let (created, outcome) = buy_marketplace(
                                &mut engine,
                                &scene,
                                &mut wallet,
                                &entry,
                                mesh.as_deref(),
                                pos,
                                &ref_id,
                            );
                            match (created, outcome) {
                                (Some(eid), Outcome::Charged { balance_tokens, .. }) => {
                                    log.append(&Record::ApplyMarketplace {
                                        entry_id: entry.id.clone(),
                                        pos,
                                        mesh,
                                    });
                                    echo_created(
                                        &mut engine,
                                        &shared,
                                        &mut positions,
                                        &assets,
                                        &channel,
                                        &mut recency,
                                        &mut touch,
                                        eid,
                                    );
                                    AddResponse {
                                        created: Some(eid.to_loro_key()),
                                        balance: Some(balance_tokens),
                                        seam: None,
                                    }
                                }
                                (_, Outcome::Refused { needed, have }) => AddResponse {
                                    seam: Some(format!(
                                        "insufficient balance: this asset costs {needed} tokens, you have {have} — top up?"
                                    )),
                                    balance: Some(have),
                                    ..Default::default()
                                },
                                (_, Outcome::Rejected(why)) => AddResponse {
                                    seam: Some(why),
                                    balance: Some(wallet.balance_tokens()),
                                    ..Default::default()
                                },
                                _ => AddResponse::default(),
                            }
                        }
                        // An id the palette offered but the catalog no longer has — honest, not silent.
                        None => AddResponse {
                            seam: Some(format!("'{id}' is not in the catalog")),
                            balance: Some(wallet.balance_tokens()),
                            ..Default::default()
                        },
                    }
                } else {
                    match capscene::add_kind(&mut engine, &scene, &id, pos, &assets.catalog) {
                        Some(eid) => {
                            log.append(&Record::AddKind {
                                name: id.clone(),
                                pos,
                            });
                            echo_created(
                                &mut engine,
                                &shared,
                                &mut positions,
                                &assets,
                                &channel,
                                &mut recency,
                                &mut touch,
                                eid,
                            );
                            AddResponse {
                                created: Some(eid.to_loro_key()),
                                balance: Some(wallet.balance_tokens()),
                                seam: None,
                            }
                        }
                        // An unknown kind — honest seam, never a silent no-op.
                        None => AddResponse {
                            seam: Some(format!("unknown component kind '{id}'")),
                            balance: Some(wallet.balance_tokens()),
                            ..Default::default()
                        },
                    }
                };
                let _ = reply.send(resp);
            }
            EngineCmd::SpawnBody { pos, reply } => {
                // ECS-authoritative SETUP: one undoable commit creates the body entity (rendered as the
                // ball via its MeshRenderer handle). Then MIRROR it into the sim (a dynamic ball body at
                // the same position) and start the sim. The per-tick motion is synced separately (Tick).
                match capscene::spawn_physics_body(
                    &mut engine,
                    &scene,
                    Some(&assets.sphere),
                    pos,
                    BALL_RADIUS,
                ) {
                    Ok(id) => {
                        // A new body starts a fresh deterministic RUN (capturing the current state of any
                        // existing bodies, so they don't snap back to spawn) — the scrub timeline spans the
                        // run from here.
                        (recording, rec_entities, sim, body_of) =
                            restart_run(&engine, &assets, &sim, &body_of);
                        frame = 0;
                        max_frame = 0;
                        log.append(&Record::SpawnBody {
                            pos,
                            mesh: Some(assets.sphere.clone()),
                        });
                        echo_created(
                            &mut engine,
                            &shared,
                            &mut positions,
                            &assets,
                            &channel,
                            &mut recency,
                            &mut touch,
                            id,
                        );
                        sim_running = true;
                        let _ = reply.send(Some(id.to_loro_key()));
                    }
                    Err(_) => {
                        let _ = reply.send(None);
                    }
                }
            }
            EngineCmd::ProjectState { reply } => {
                let _ = reply.send(project_info(
                    current_path.as_deref(),
                    engine.version_vector() != saved_vv,
                    &recents_path,
                    None,
                ));
            }
            EngineCmd::SaveProject { path, reply } => {
                // Target = the explicit path, else the current project's. Untitled + no path ⇒ Save As
                // (the native dialog is the local-GUI step) — an explained reply, never a crash.
                let target = path
                    .map(std::path::PathBuf::from)
                    .or_else(|| current_path.clone());
                let resp = match target {
                    None => project_info(
                        current_path.as_deref(),
                        engine.version_vector() != saved_vv,
                        &recents_path,
                        Some(
                            "untitled project — choose a location (Save As); the native file dialog is the local-GUI step"
                                .into(),
                        ),
                    ),
                    Some(p) => match mtk_project::save(&engine, &p) {
                        Ok(()) => {
                            current_path = Some(p.clone());
                            saved_vv = engine.version_vector(); // now "clean" w.r.t. disk
                            mtk_project::push_recent(
                                &recents_path,
                                &p.display().to_string(),
                                mtk_project::RECENTS_CAP,
                            );
                            project_info(Some(&p), false, &recents_path, None)
                        }
                        Err(e) => project_info(
                            current_path.as_deref(),
                            engine.version_vector() != saved_vv,
                            &recents_path,
                            Some(format!("save failed: {e}")),
                        ),
                    },
                };
                let _ = reply.send(resp);
            }
            EngineCmd::NewProject { reply } => {
                // A fresh empty project: a new world + scene + engine (resolver set), the session log
                // reset, re-projected, the sim restarted empty. Render-coupled — ACCEPTED on a GUI run.
                let mut w = FlecsWorld::new();
                let s = CapScene::intern(&mut w);
                let mut e = Engine::new(w, 1);
                e.set_capability_resolver(Box::new(capscene::CapResolver::from_scene(&s)));
                engine = e;
                scene = s;
                current_path = None;
                saved_vv = engine.version_vector();
                log.clear(); // the session replay log resets for the new project
                recency.clear();
                touch = 0;
                rebuild(&engine, &shared, &mut positions, &assets);
                if let Some(ch) = &channel {
                    send_proj!(ch, proj_full(&engine, &scene));
                }
                (recording, rec_entities, sim, body_of) = restart_run(
                    &engine,
                    &assets,
                    &RapierPhysics::new(Fidelity::Gameplay.resolve().config),
                    &HashMap::new(),
                );
                frame = 0;
                max_frame = 0;
                sim_running = false;
                let _ = reply.send(project_info(None, false, &recents_path, None));
            }
            EngineCmd::OpenProject { path, reply } => {
                let Some(p) = path.map(std::path::PathBuf::from) else {
                    // No path ⇒ the native Open dialog (the local-GUI step) — explained reply.
                    let _ = reply.send(project_info(
                        current_path.as_deref(),
                        engine.version_vector() != saved_vv,
                        &recents_path,
                        Some(
                            "choose a file to open — the native file dialog is the local-GUI step"
                                .into(),
                        ),
                    ));
                    continue;
                };
                // Build a FRESH engine + scene + resolver and OPEN the .mtk into it; swap in ONLY on
                // success (a corrupt/newer/missing file leaves the current project intact — explained,
                // never a crash). Render-coupled swap — ACCEPTED on a GUI run.
                let mut w = FlecsWorld::new();
                let s = CapScene::intern(&mut w);
                let mut e = Engine::new(w, 1);
                e.set_capability_resolver(Box::new(capscene::CapResolver::from_scene(&s)));
                let resp = match mtk_project::open_into(&mut e, &p) {
                    Ok(_report) => {
                        engine = e;
                        scene = s;
                        current_path = Some(p.clone());
                        saved_vv = engine.version_vector();
                        log.clear(); // the opened document IS the state; reset the session log
                        recency.clear();
                        touch = 0;
                        rebuild(&engine, &shared, &mut positions, &assets);
                        if let Some(ch) = &channel {
                            send_proj!(ch, proj_full(&engine, &scene));
                        }
                        (recording, rec_entities, sim, body_of) = restart_run(
                            &engine,
                            &assets,
                            &RapierPhysics::new(Fidelity::Gameplay.resolve().config),
                            &HashMap::new(),
                        );
                        frame = 0;
                        max_frame = 0;
                        sim_running = !body_of.is_empty();
                        mtk_project::push_recent(
                            &recents_path,
                            &p.display().to_string(),
                            mtk_project::RECENTS_CAP,
                        );
                        project_info(Some(&p), false, &recents_path, None)
                    }
                    Err(err) => project_info(
                        current_path.as_deref(),
                        engine.version_vector() != saved_vv,
                        &recents_path,
                        Some(err.to_string()),
                    ),
                };
                let _ = reply.send(resp);
            }
            EngineCmd::SetSimRunning(run) => {
                sim_running = run;
            }
            EngineCmd::Play { reply } => {
                // Enter play mode: snapshot the edit state (so Stop restores it bit-exactly), then run
                // the deterministic sim from the current scene. The sim projects per-tick transforms to
                // the render only (ADR-021), so the authored ECS/Loro is never mutated by running.
                if !play_mode {
                    play_snapshot = Some(engine.snapshot());
                    play_mode = true;
                    // M11.4 (ADR-043) — render through the active scene camera while playing. Save the
                    // pre-Play editor view so Stop restores it; if no camera is active, keep the current
                    // view (fly-cam or a manual look-through). Render-only — never touches the doc.
                    let active = capscene::active_camera(&engine);
                    {
                        let mut st = shared.lock().unwrap();
                        pre_play_cam = st.cam_override;
                        if let Some((p, fov, near, far)) = active {
                            st.cam_override = Some(render::CamView {
                                pos: p,
                                fov_deg: fov,
                                near,
                                far,
                            });
                        }
                    }
                    (recording, rec_entities, sim, body_of) =
                        restart_run(&engine, &assets, &sim, &body_of);
                    frame = 0;
                    max_frame = 0;
                    sim_running = true;
                    // M12.5 (ADR-049) — capture the authored Rules + state machines into a deterministic
                    // `RuleReplay` at Play-start (a projection over a RuntimeState, never the doc). A rule
                    // with a non-deterministic plugin is held out of the lockstep path + surfaced.
                    let session = metrocalk_editor_shell::play_rules::build_recording(
                        &engine,
                        &rules_registry,
                    );
                    rule_flagged = session.flagged;
                    rule_session = Some(metrocalk_core::RuleReplay::new(session.recording));
                    rule_head = 0;
                }
                let _ = reply.send(PlayInfo {
                    playing: play_mode,
                    paused: play_mode && !sim_running,
                });
            }
            EngineCmd::Pause { reply } => {
                // Freeze / unfreeze the running sim while staying in play mode (no effect when Stopped).
                if play_mode {
                    sim_running = !sim_running;
                }
                let _ = reply.send(PlayInfo {
                    playing: play_mode,
                    paused: play_mode && !sim_running,
                });
            }
            EngineCmd::Stop { reply } => {
                // **Non-destructive Stop:** restore the pre-Play edit state bit-exactly from the snapshot
                // (a fresh engine + scene + resolver + merge — the same swap as project-open, but from the
                // in-memory snapshot), so even an edit that leaked during Play is wiped. The sim then
                // re-derives from the authored scene (bodies snap back to their edit positions).
                if play_mode {
                    if let Some(snap) = play_snapshot.take() {
                        let mut w = FlecsWorld::new();
                        let s = CapScene::intern(&mut w);
                        let mut e = Engine::new(w, 1);
                        e.set_capability_resolver(Box::new(capscene::CapResolver::from_scene(&s)));
                        // Restore the pre-Play edit state. If the snapshot merge fails we must NOT leave the
                        // play-mutated engine in place silently (audit F4) — log it loudly; the user sees the
                        // (uncorrupted) prior engine continue rather than silent scene corruption.
                        match e.merge(&snap) {
                            Ok(_) => {
                                engine = e;
                                scene = s;
                            }
                            Err(err) => eprintln!(
                                "[shell] Stop: snapshot merge FAILED — pre-Play scene NOT restored: {err}"
                            ),
                        }
                    }
                    play_mode = false;
                    sim_running = false;
                    // M12.5 (ADR-049) — drop the Play-time Rules session (its runtime state + decision history
                    // are a projection; Stop discards them, restoring the authored doc bit-exactly).
                    rule_session = None;
                    rule_flagged.clear();
                    rule_head = 0;
                    // M11.4 — leave look-through: restore the pre-Play editor view (fly-cam or manual).
                    shared.lock().unwrap().cam_override = pre_play_cam.take();
                    recency.clear(); // ECS handles changed on the restore swap — drop stale ranking state
                    touch = 0;
                    rebuild(&engine, &shared, &mut positions, &assets);
                    if let Some(ch) = &channel {
                        send_proj!(ch, proj_full(&engine, &scene));
                    }
                    (recording, rec_entities, sim, body_of) = restart_run(
                        &engine,
                        &assets,
                        &RapierPhysics::new(Fidelity::Gameplay.resolve().config),
                        &HashMap::new(),
                    );
                    frame = 0;
                    max_frame = 0;
                }
                let _ = reply.send(PlayInfo {
                    playing: false,
                    paused: false,
                });
            }
            EngineCmd::PlayStateQuery { reply } => {
                let _ = reply.send(PlayInfo {
                    playing: play_mode,
                    paused: play_mode && !sim_running,
                });
            }
            EngineCmd::Tick => {
                // One fixed-`dt` step + a delta sync of the moved bodies' transforms to the viewport, and
                // (only if the debugger is open) a refresh of the read-only overlay. A no-op until a body
                // exists + the sim runs. NEVER a commit/Loro write (ADR-021). Off the JS hot path (inv. 4).
                if sim_running && !body_of.is_empty() {
                    let centers = sync_out(&sim, &body_of, &shared);
                    sim.step();
                    frame += 1;
                    max_frame = max_frame.max(frame);
                    if overlays_on {
                        push_overlay(&sim, &body_of, &prev_centers, &shared);
                    }
                    prev_centers = centers;
                }
            }
            EngineCmd::SimScrub {
                frame: target,
                reply,
            } => {
                // Scrub over the sim-replay channel: deterministically re-derive the world at `target` by
                // replaying the recording (a rewind rebuilds the world — fresh broad-phase, no #910), then
                // PAUSE there. `sim` becomes the replayed world so a later resume continues bit-identically.
                let target = target.min(max_frame);
                let mut replay = Replay::new(recording.clone());
                replay.seek(target);
                (sim, body_of) = install_replay(replay, &rec_entities);
                frame = target;
                sim_running = false;
                prev_centers = sync_out(&sim, &body_of, &shared);
                if overlays_on {
                    // overlay at the scrubbed frame (prev == current, so no trajectory streak on a still)
                    push_overlay(&sim, &body_of, &prev_centers, &shared);
                }
                let _ = reply.send(TimelineInfo {
                    frame,
                    max_frame,
                    running: sim_running,
                    overlays_on,
                    bodies: body_of.len(),
                });
            }
            EngineCmd::SimTimeline { reply } => {
                let _ = reply.send(TimelineInfo {
                    frame,
                    max_frame,
                    running: sim_running,
                    overlays_on,
                    bodies: body_of.len(),
                });
            }
            EngineCmd::SimOverlay { on } => {
                overlays_on = on;
                if on {
                    push_overlay(&sim, &body_of, &prev_centers, &shared);
                } else {
                    clear_overlay(&shared); // zero cost when closed
                }
            }
            EngineCmd::SimShove { id, impulse, reply } => {
                // Apply the impulse live AND record it (by body index) at the current frame, so a scrub
                // replays it — the sim-replay input channel (M8.1 P2). The recorded artifact reproduces it.
                let ok = EntityId::from_loro_key(&id)
                    .and_then(|eid| {
                        body_of
                            .get(&eid)
                            .copied()
                            .zip(rec_index_of(&rec_entities, eid))
                    })
                    .map(|(h, idx)| {
                        sim.apply_impulse(h, impulse);
                        recording.add_input(frame, idx, impulse);
                        sync_out(&sim, &body_of, &shared);
                    })
                    .is_some();
                let _ = reply.send(ok);
            }
            EngineCmd::PhysicsContacts { reply } => {
                // The live contacts at the current frame, each explained — the click-to-explain read.
                // Non-mutating (the diagnostic seam is read-only).
                let contacts = sim
                    .diagnostics()
                    .contacts
                    .iter()
                    .map(ContactInfo::from_contact)
                    .collect();
                let _ = reply.send(contacts);
            }
            EngineCmd::ImportInterchange {
                format,
                source,
                reply,
            } => {
                // M8.5: parse a URDF / USD-Physics scene behind the Interchange trait → instantiate it as
                // registry components in ONE undoable tx (units reconciled) → restart the run so the bodies
                // simulate. Every unsupported feature is an explained note; a parse error is surfaced.
                let parsed = match format.as_str() {
                    "usd" => UsdInterchange.import(source.as_bytes()),
                    _ => UrdfInterchange.import(source.as_bytes()),
                };
                let result = match parsed {
                    Ok(scene_import) => {
                        match capscene::import_scene(&mut engine, &scene, &scene_import) {
                            Ok(ids) => {
                                (recording, rec_entities, sim, body_of) =
                                    restart_run(&engine, &assets, &sim, &body_of);
                                frame = 0;
                                max_frame = 0;
                                sim_running = true;
                                log.append(&Record::Import {
                                    format: format.clone(),
                                    source: source.clone(),
                                });
                                for id in &ids {
                                    echo_created(
                                        &mut engine,
                                        &shared,
                                        &mut positions,
                                        &assets,
                                        &channel,
                                        &mut recency,
                                        &mut touch,
                                        *id,
                                    );
                                }
                                ImportResult {
                                    ok: true,
                                    format: scene_import.format.clone(),
                                    bodies: scene_import.bodies.len(),
                                    joints: scene_import.joints.len(),
                                    meters_per_unit: scene_import.units.meters_per_unit,
                                    kilograms_per_unit: scene_import.units.kilograms_per_unit,
                                    reconciled: scene_import.units.needs_reconciliation(),
                                    notes: scene_import
                                        .notes
                                        .iter()
                                        .map(|n| format!("{} — {}", n.feature, n.detail))
                                        .collect(),
                                    error: None,
                                }
                            }
                            Err(e) => ImportResult {
                                ok: false,
                                error: Some(format!("instantiate failed: {e}")),
                                ..Default::default()
                            },
                        }
                    }
                    Err(e) => ImportResult {
                        ok: false,
                        error: Some(format!("{e}")),
                        ..Default::default()
                    },
                };
                let _ = reply.send(result);
            }
            EngineCmd::GizmoCommit {
                id,
                pos,
                rot,
                scale,
            } => {
                // M9.1/M9.2: the gizmo drag's coalesced delta lands as ONE undoable transaction. A CHILD
                // PART (M9.2 / ADR-026) gets **parent-space write-back → a per-field OVERRIDE** (non-
                // destructive, override-wins by structure); a flat root keeps the M9.1 base `set_transform`.
                // Both persist + a physics body re-simulates from the new pose.
                if let Some(eid) = EntityId::from_loro_key(&id) {
                    let applied = if engine.parent_of(eid).is_some() {
                        match capscene::edit_part_transform(
                            &mut engine,
                            eid,
                            GizmoTransform {
                                translation: pos,
                                rotation: rot,
                                scale: [scale, scale, scale],
                            },
                        ) {
                            // Persist the LOCAL (parent-independent ⇒ deterministic replay).
                            Ok(local) => {
                                log.append(&Record::EditPart {
                                    id: id.clone(),
                                    x: f64::from(local.translation[0]),
                                    y: f64::from(local.translation[1]),
                                    z: f64::from(local.translation[2]),
                                    qx: f64::from(local.rotation[0]),
                                    qy: f64::from(local.rotation[1]),
                                    qz: f64::from(local.rotation[2]),
                                    qw: f64::from(local.rotation[3]),
                                    scale: f64::from(local.scale[0]),
                                });
                                true
                            }
                            Err(_) => false,
                        }
                    } else if capscene::set_transform(&mut engine, eid, pos, rot, scale).is_ok() {
                        log.append(&Record::Transform {
                            id: id.clone(),
                            x: f64::from(pos[0]),
                            y: f64::from(pos[1]),
                            z: f64::from(pos[2]),
                            qx: f64::from(rot[0]),
                            qy: f64::from(rot[1]),
                            qz: f64::from(rot[2]),
                            qw: f64::from(rot[3]),
                            scale: f64::from(scale),
                        });
                        true
                    } else {
                        false
                    };
                    if applied {
                        let comps = engine.components_of(eid);
                        if comps.contains_key("RigidBody") && comps.contains_key("Collider") {
                            // The gizmo MOVED this body — restart it from the new ECS Transform, not its
                            // stale sim position. Dropping it from `body_of` makes restart_run's
                            // record_body read the just-committed Transform for it (zeroing velocity from
                            // the gizmo placement); the OTHER bodies keep their current simulated state.
                            body_of.remove(&eid);
                            (recording, rec_entities, sim, body_of) =
                                restart_run(&engine, &assets, &sim, &body_of);
                            frame = 0;
                            max_frame = 0;
                        }
                        echo_created(
                            &mut engine,
                            &shared,
                            &mut positions,
                            &assets,
                            &channel,
                            &mut recency,
                            &mut touch,
                            eid,
                        );
                    }
                }
            }
            EngineCmd::ReadTransform { id, reply } => {
                let t = EntityId::from_loro_key(&id)
                    .map(|eid| {
                        let comps = engine.components_of(eid);
                        let get = |f: &str, default: f64| -> f64 {
                            comps
                                .get("Transform")
                                .and_then(|m| m.get(f))
                                .map_or(default, |v| match v {
                                    FieldValue::Number(n) => *n,
                                    FieldValue::Integer(i) => *i as f64,
                                    _ => default,
                                })
                        };
                        // [x, y, z, qx, qy, qz, qw, scale] — quat identity / unit scale when unauthored.
                        [
                            get("x", 0.0),
                            get("y", 0.0),
                            get("z", 0.0),
                            get("qx", 0.0),
                            get("qy", 0.0),
                            get("qz", 0.0),
                            get("qw", 1.0),
                            get("scale", 1.0),
                        ]
                    })
                    .unwrap_or([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0]);
                let _ = reply.send(t);
            }
            EngineCmd::ReparentPart { id, parent } => {
                // M9.2 "drag in hierarchy": one node.move, undoable + persisted; re-sync the projection.
                if let Some(eid) = EntityId::from_loro_key(&id) {
                    let p = parent.as_deref().and_then(EntityId::from_loro_key);
                    if capscene::reparent(&mut engine, eid, p).is_ok() {
                        log.append(&Record::Reparent {
                            id: id.clone(),
                            parent: parent.clone(),
                        });
                        if let Some(ch) = &channel {
                            send_proj!(ch, proj_full(&engine, &scene));
                        }
                        rebuild(&engine, &shared, &mut positions, &assets);
                    }
                }
            }
            EngineCmd::SetPartActive { id, active, reply } => {
                // M9.2 deactivate-not-delete (or reactivate): one undoable tx + persist; rebuild hides /
                // re-shows the part. Undo restores it (the e2e: deactivate → Ctrl-Z brings it back).
                let ok = EntityId::from_loro_key(&id)
                    .is_some_and(|eid| capscene::set_part_active(&mut engine, eid, active).is_ok());
                if ok {
                    log.append(&Record::SetPartActive {
                        id: id.clone(),
                        active,
                    });
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(ok);
            }
            EngineCmd::SaveCharacter { id, reply } => {
                // M9.2 save-for-reuse: snapshot the selected part's WHOLE character (its root subtree) into
                // a reusable Composition (the edited state baked in), kept in the session registry.
                let comp_id = EntityId::from_loro_key(&id).map(|eid| {
                    let root = capscene::root_of(&engine, eid);
                    let cid = format!("char:{}:{comp_seq}", root.to_loro_key());
                    comp_seq += 1;
                    let comp = engine.save_composition(root, &cid);
                    compositions.insert(cid.clone(), comp);
                    cid
                });
                let _ = reply.send(comp_id);
            }
            EngineCmd::InstantiateCharacter { comp_id, reply } => {
                // M9.2 "drop a fresh instance": re-instantiate a saved Composition — independently-id'd,
                // pre-componentized, the edit present (override-wins), linked back to the source.
                let new_root = compositions.get(&comp_id).and_then(|comp| {
                    engine
                        .instantiate_composition(comp)
                        .ok()
                        .map(|root| (root, root.to_loro_key()))
                });
                if let Some((root, _)) = &new_root {
                    echo_created(
                        &mut engine,
                        &shared,
                        &mut positions,
                        &assets,
                        &channel,
                        &mut recency,
                        &mut touch,
                        *root,
                    );
                }
                let _ = reply.send(new_root.map(|(_, key)| key));
            }
            EngineCmd::PartDebug { id, reply } => {
                let info = EntityId::from_loro_key(&id).map_or((0.0, 0.0, 0.0, false, 0), |eid| {
                    let g = capscene::global_transform(&engine, eid);
                    (
                        f64::from(g.translation[0]),
                        f64::from(g.translation[1]),
                        f64::from(g.translation[2]),
                        engine.is_active(eid),
                        engine.overrides_of(eid).len(),
                    )
                });
                let _ = reply.send(info);
            }
            EngineCmd::DemoCharacter { reply } => {
                let ids = demo_char.as_ref().map(|(root, parts)| {
                    (
                        root.to_loro_key(),
                        parts.iter().map(EntityId::to_loro_key).collect(),
                    )
                });
                let _ = reply.send(ids);
            }
            EngineCmd::PartAtPath { root, path, reply } => {
                let part = EntityId::from_loro_key(&root)
                    .and_then(|rt| engine.entity_at_path(rt, &path))
                    .map(|e| e.to_loro_key());
                let _ = reply.send(part);
            }
            EngineCmd::PartParent { id, reply } => {
                let parent = EntityId::from_loro_key(&id)
                    .and_then(|eid| engine.parent_of(eid))
                    .map(|p| p.to_loro_key());
                let _ = reply.send(parent);
            }
            EngineCmd::SnapQuery { id, radius, reply } => {
                let hits = EntityId::from_loro_key(&id)
                    .map(|eid| snap_hits(&engine, &positions, &recency, eid, radius))
                    .unwrap_or_default();
                let _ = reply.send(hits);
            }
            EngineCmd::ApplyConstraint {
                id,
                kind,
                target,
                value,
                reply,
            } => {
                // Resolve `kind` + the target's position into a deterministic constraint, solve it, and
                // commit through the one pipeline (M9.4) — or reply the explained block (every "no").
                let mut applied = None;
                let result = match EntityId::from_loro_key(&id) {
                    None => SolveResult {
                        ok: false,
                        reason: Some("invalid entity id".into()),
                        ..Default::default()
                    },
                    Some(eid) => {
                        let tpos = target
                            .as_deref()
                            .and_then(EntityId::from_loro_key)
                            .and_then(|te| engine.ecs_entity(te))
                            .and_then(|ecs| positions.get(&ecs).copied());
                        let constraint = match (kind.as_str(), tpos) {
                            ("snap", Some(t)) => Some(Constraint::SnapToPoint { target: t }),
                            ("align-surface", Some(t)) => Some(Constraint::AlignToSurface {
                                point: t,
                                normal: [0.0, 1.0, 0.0],
                            }),
                            ("coplanar", Some(t)) => Some(Constraint::Coplanar {
                                point: t,
                                normal: [0.0, 1.0, 0.0],
                            }),
                            ("coaxial", Some(t)) => Some(Constraint::Coaxial {
                                point: t,
                                dir: [0.0, 1.0, 0.0],
                            }),
                            ("clearance", Some(t)) => Some(Constraint::Clearance {
                                from: t,
                                distance: value,
                            }),
                            ("symmetry", Some(t)) => Some(Constraint::Symmetry {
                                point: t,
                                normal: [1.0, 0.0, 0.0],
                            }),
                            _ => None,
                        };
                        match constraint {
                            None => SolveResult {
                                ok: false,
                                reason: Some(format!(
                                    "'{kind}' needs a valid snap target — pick one first"
                                )),
                                ..Default::default()
                            },
                            Some(c) => {
                                let current = capscene::global_transform(&engine, eid);
                                match transform_solver::solve(&c, &current) {
                                    Err(b) => SolveResult {
                                        ok: false,
                                        reason: Some(b.reason),
                                        ..Default::default()
                                    },
                                    Ok(world) => {
                                        let ok =
                                            commit_world_transform(&mut engine, &log, eid, world);
                                        if ok {
                                            applied = Some(eid);
                                        }
                                        SolveResult {
                                            ok,
                                            reason: (!ok).then(|| "commit failed".into()),
                                            ..Default::default()
                                        }
                                    }
                                }
                            }
                        }
                    }
                };
                if let Some(eid) = applied {
                    echo_created(
                        &mut engine,
                        &shared,
                        &mut positions,
                        &assets,
                        &channel,
                        &mut recency,
                        &mut touch,
                        eid,
                    );
                }
                let _ = reply.send(result);
            }
            EngineCmd::PlacementSentence { id, text, reply } => {
                // Compile the sentence → editable intents → resolve each against the snap-graph (the
                // nearest target/surface) → chain the solves → apply as a SCHEMA-VALIDATED patch
                // (apply_transform_constraint / apply_ai_patch, ADR-017 — never a raw mutation).
                let intents = transform_solver::compile(&text);
                let labels: Vec<String> = intents.iter().map(|i| format!("{i:?}")).collect();
                let mut applied = None;
                let result = match EntityId::from_loro_key(&id) {
                    None => SolveResult {
                        ok: false,
                        reason: Some("invalid entity id".into()),
                        intents: labels,
                    },
                    Some(_) if intents.is_empty() => SolveResult {
                        ok: false,
                        reason: Some("couldn't interpret the placement".into()),
                        intents: labels,
                    },
                    Some(eid) => {
                        let nearest = snap_hits(&engine, &positions, &recency, eid, 1.0e6)
                            .into_iter()
                            .next();
                        let pt = nearest.as_ref().map(|h| [h.x, h.y, h.z]);
                        let mut world = capscene::global_transform(&engine, eid);
                        let mut err = None;
                        for intent in &intents {
                            let c = match intent {
                                ConstraintIntent::Upright => Constraint::AlignToSurface {
                                    point: pt.unwrap_or(world.translation),
                                    normal: [0.0, 1.0, 0.0],
                                },
                                ConstraintIntent::SnapToNearest => match pt {
                                    Some(t) => Constraint::SnapToPoint { target: t },
                                    None => continue,
                                },
                                ConstraintIntent::Clearance(d) => match pt {
                                    Some(t) => Constraint::Clearance {
                                        from: t,
                                        distance: *d,
                                    },
                                    None => continue,
                                },
                                ConstraintIntent::Coaxial => match pt {
                                    Some(t) => Constraint::Coaxial {
                                        point: t,
                                        dir: [0.0, 1.0, 0.0],
                                    },
                                    None => continue,
                                },
                            };
                            match transform_solver::solve(&c, &world) {
                                Ok(w) => world = w,
                                Err(b) => {
                                    err = Some(b.reason);
                                    break;
                                }
                            }
                        }
                        match err {
                            Some(reason) => SolveResult {
                                ok: false,
                                reason: Some(reason),
                                intents: labels,
                            },
                            None => {
                                let delta = transform_solver::apply_transform_constraint(
                                    &mut engine,
                                    eid,
                                    &world,
                                    "placement-sentence",
                                );
                                let ok = delta.rejects.is_empty();
                                if ok {
                                    applied = Some(eid);
                                    // Persist the placed pose for reload (the AI patch committed it; record
                                    // the net world transform so replay reproduces it).
                                    log.append(&Record::Transform {
                                        id: eid.to_loro_key(),
                                        x: f64::from(world.translation[0]),
                                        y: f64::from(world.translation[1]),
                                        z: f64::from(world.translation[2]),
                                        qx: f64::from(world.rotation[0]),
                                        qy: f64::from(world.rotation[1]),
                                        qz: f64::from(world.rotation[2]),
                                        qw: f64::from(world.rotation[3]),
                                        scale: f64::from(world.scale[0]),
                                    });
                                }
                                SolveResult {
                                    ok,
                                    reason: (!ok)
                                        .then(|| "the constraint patch was rejected".into()),
                                    intents: labels,
                                }
                            }
                        }
                    }
                };
                if let Some(eid) = applied {
                    echo_created(
                        &mut engine,
                        &shared,
                        &mut positions,
                        &assets,
                        &channel,
                        &mut recency,
                        &mut touch,
                        eid,
                    );
                }
                let _ = reply.send(result);
            }
            EngineCmd::PhysicsDebug { reply } => {
                let min_y = body_of
                    .values()
                    .filter_map(|h| sim.transform(*h).map(|(t, _)| t[1]))
                    .fold(f64::INFINITY, f64::min);
                let contacts = sim.diagnostics().contact_count;
                let _ = reply.send((body_of.len(), min_y, contacts));
            }
            EngineCmd::BodySimPosition { id, reply } => {
                let pos = EntityId::from_loro_key(&id)
                    .and_then(|eid| body_of.get(&eid))
                    .and_then(|h| sim.transform(*h))
                    .map_or([0.0, 0.0, 0.0], |(t, _)| t);
                let _ = reply.send(pos);
            }
            EngineCmd::MakeDynamic { id, reply } => {
                // M8.3 ≤2-click: dead mesh → correct dynamic body (one undoable commit) → mirror into the
                // sim (a hull collider derived from the mesh) → it falls.
                let ok = if let Some(eid) = EntityId::from_loro_key(&id) {
                    match physics_intent::make_dynamic(&mut engine, &scene, eid, 1.0) {
                        Ok(()) => {
                            (recording, rec_entities, sim, body_of) =
                                restart_run(&engine, &assets, &sim, &body_of);
                            frame = 0;
                            max_frame = 0;
                            sim_running = true;
                            log.append(&Record::MakeDynamic { id: id.clone() });
                            echo_created(
                                &mut engine,
                                &shared,
                                &mut positions,
                                &assets,
                                &channel,
                                &mut recency,
                                &mut touch,
                                eid,
                            );
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    false
                };
                let _ = reply.send(ok);
            }
            EngineCmd::MakeStatic { id, reply } => {
                // M11.1 — imported mesh → STATIC collidable obstacle (fixed body + hull collider) in one
                // undoable commit, mirrored into the sim so dynamic bodies rest ON it. The sim run-state is
                // left as-is (a static obstacle doesn't animate; a later drop starts it).
                let ok = if let Some(eid) = EntityId::from_loro_key(&id) {
                    match physics_intent::make_static(&mut engine, &scene, eid) {
                        Ok(()) => {
                            (recording, rec_entities, sim, body_of) =
                                restart_run(&engine, &assets, &sim, &body_of);
                            frame = 0;
                            max_frame = 0;
                            log.append(&Record::MakeStatic { id: id.clone() });
                            echo_created(
                                &mut engine,
                                &shared,
                                &mut positions,
                                &assets,
                                &channel,
                                &mut recency,
                                &mut touch,
                                eid,
                            );
                            true
                        }
                        Err(_) => false,
                    }
                } else {
                    false
                };
                let _ = reply.send(ok);
            }
            EngineCmd::PhysicsCheck { id, reply } => {
                let warns = EntityId::from_loro_key(&id)
                    .map(|eid| {
                        physics_intent::check_physics(
                            &engine,
                            eid,
                            mesh_metrics(&assets, &engine, eid),
                        )
                    })
                    .unwrap_or_default();
                let _ = reply.send(warns);
            }
            EngineCmd::PhysicsFix { id, action, reply } => {
                // M8.3 one-click fix → one undoable commit → re-mirror the (now valid / reshaped) body.
                let ok = if let Some(eid) = EntityId::from_loro_key(&id) {
                    let r = match action.as_str() {
                        "add-collider" => {
                            physics_intent::add_collider(&mut engine, &scene, eid, true)
                        }
                        "use-hull" => physics_intent::use_convex_hull(&mut engine, eid),
                        "fix-mass" => physics_intent::fix_mass(&mut engine, eid, 1.0),
                        // fix-scale: a flagged suggestion; real unit reconciliation is M8.5 — acked, no-op.
                        _ => Ok(()),
                    };
                    if r.is_ok() {
                        (recording, rec_entities, sim, body_of) =
                            restart_run(&engine, &assets, &sim, &body_of);
                        frame = 0;
                        max_frame = 0;
                        sim_running = true;
                        log.append(&Record::PhysicsFix {
                            id: id.clone(),
                            action: action.clone(),
                        });
                        echo_created(
                            &mut engine,
                            &shared,
                            &mut positions,
                            &assets,
                            &channel,
                            &mut recency,
                            &mut touch,
                            eid,
                        );
                    }
                    r.is_ok()
                } else {
                    false
                };
                let _ = reply.send(ok);
            }
        }
    }
}

/// Read an entity's `Transform` x/y/z as a sim spawn position (`f64`, origin if absent) — used to
/// re-hydrate the sim from restored physics entities after replay.
fn body_spawn_pos(engine: &Engine<FlecsWorld>, id: EntityId) -> [f64; 3] {
    let comps = engine.components_of(id);
    let t = comps.get("Transform");
    let get = |f: &str| -> f64 {
        t.and_then(|m| m.get(f)).map_or(0.0, |v| match v {
            FieldValue::Number(n) => *n,
            FieldValue::Integer(i) => *i as f64,
            _ => 0.0,
        })
    };
    [get("x"), get("y"), get("z")]
}

/// M11.1 — a non-overlapping spawn position for an imported mesh: a 4-wide grid (2.5u spacing, y=1),
/// indexed by how many mesh entities already exist, so successive imports lay out side by side instead of
/// stacking on the same spot. Resets naturally with the scene (a cleared scene → index 0 again).
fn next_import_pos(engine: &Engine<FlecsWorld>) -> [f32; 3] {
    let n = engine
        .entity_ids()
        .iter()
        .filter(|id| engine.components_of(**id).contains_key("MeshRenderer"))
        .count();
    const COLS: usize = 4;
    const SPACING: f32 = 2.5;
    let col = (n % COLS) as f32;
    let row = (n / COLS) as f32;
    [(col - 1.5) * SPACING, 1.0, row * SPACING]
}

/// M11.5 (ADR-044) — the asset-IDENTITY projection for an entity: resolve its `MeshRenderer.mesh` handle to
/// the stored [`Provenance`] and render it for the inspector, computing a near-duplicate hint against the
/// OTHER loaded assets (a perceptual-hash match on different bytes — a HINT, never a silent merge). `None`
/// if the entity carries no store-resolvable mesh (a placeholder-cube / pure marker has no asset identity).
fn asset_provenance_of(
    assets: &AssetsRuntime,
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> Option<ProvenanceInfo> {
    let comps = engine.components_of(id);
    let FieldValue::Str(handle) = comps
        .get("MeshRenderer")
        .and_then(|m| m.get(capscene::MESH_FIELD))?
    else {
        return None;
    };
    let p = assets.provenance.get(handle)?;
    let kind = match p.kind {
        Some(metrocalk_assets::AssetKind::Imported) => "imported",
        Some(metrocalk_assets::AssetKind::Generated) => "generated",
        None => "unknown",
    };
    // A near-duplicate hint: another loaded asset, different bytes, perceptually similar.
    let near_duplicate_of = if p.perceptual_hash == 0 {
        None
    } else {
        assets
            .provenance
            .values()
            .filter(|o| o.content_hash != p.content_hash && o.perceptual_hash != 0)
            .find(|o| metrocalk_assets::is_near_duplicate(p.perceptual_hash, o.perceptual_hash, 10))
            .map(|o| o.source.clone())
    };
    Some(ProvenanceInfo {
        kind: kind.to_string(),
        source: p.source.clone(),
        ai_generated: p.ai_generated,
        content_hash: p.content_hash.clone(),
        perceptual_hash: format!("{:x}", p.perceptual_hash),
        near_duplicate_of,
    })
}

/// The entity's mesh geometry (positions as f64 + indices) from its `MeshRenderer.mesh` slot — for M8.3
/// collider derivation. `None` if the entity references no resolvable mesh.
fn mesh_geometry(
    assets: &AssetsRuntime,
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> Option<(Vec<[f64; 3]>, Vec<u32>)> {
    let comps = engine.components_of(id);
    let FieldValue::Str(handle) = comps
        .get("MeshRenderer")
        .and_then(|m| m.get(capscene::MESH_FIELD))?
    else {
        return None;
    };
    let slot = *assets.handle_to_slot.get(handle)?;
    let gpu = assets.meshes.get(slot)?;
    let verts = gpu
        .vertices
        .iter()
        .map(|v| {
            [
                f64::from(v.position[0]),
                f64::from(v.position[1]),
                f64::from(v.position[2]),
            ]
        })
        .collect();
    Some((verts, gpu.indices.clone()))
}

/// The M8.3 collider-intelligence metrics for an entity's mesh (bounds extent + hull fit/concavity). The
/// app-side derivation `physics_intent` (which stays `/physics`-free) consumes.
fn mesh_metrics(
    assets: &AssetsRuntime,
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> Option<MeshMetrics> {
    let (verts, idx) = mesh_geometry(assets, engine, id)?;
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in &verts {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    #[allow(clippy::cast_possible_truncation)]
    let max_extent = (0..3).map(|k| hi[k] - lo[k]).fold(0.0_f64, f64::max) as f32;
    let (fit_error, concave) = metrocalk_physics::derive_collider(&verts, &idx)
        .map_or((0.0, false), |d| (d.fit_error, d.concave));
    Some(MeshMetrics {
        max_extent,
        fit_error,
        concave,
    })
}

/// The collider shape to mirror for an entity, from its `Collider.shape` field: `convexHull` ⇒ a hull
/// derived from the mesh; otherwise a ball of `Collider.radius` (the spawn-ball default).
fn collider_shape_for(
    assets: &AssetsRuntime,
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> ColliderShape {
    let comps = engine.components_of(id);
    let col = comps.get("Collider");
    let shape = col.and_then(|m| m.get("shape")).and_then(|v| match v {
        FieldValue::Str(s) => Some(s.as_str()),
        _ => None,
    });
    let num = |field: &str, default: f64| -> f64 {
        col.and_then(|m| m.get(field)).map_or(default, |v| match v {
            FieldValue::Number(n) => *n,
            FieldValue::Integer(i) => *i as f64,
            _ => default,
        })
    };
    match shape {
        Some("convexHull") => {
            if let Some((verts, idx)) = mesh_geometry(assets, engine, id) {
                if let Ok(d) = metrocalk_physics::derive_collider(&verts, &idx) {
                    return d.shape;
                }
            }
            ColliderShape::Ball {
                radius: num("radius", f64::from(BALL_RADIUS)),
            }
        }
        // M8.5: imported (URDF/USD) bodies carry real primitive shapes — mirror them faithfully.
        Some("cuboid") => ColliderShape::Cuboid {
            half_extents: [num("halfX", 0.5), num("halfY", 0.5), num("halfZ", 0.5)],
        },
        Some("capsule") => ColliderShape::Capsule {
            half_height: num("halfHeight", 0.5),
            radius: num("radius", 0.25),
        },
        _ => ColliderShape::Ball {
            radius: num("radius", f64::from(BALL_RADIUS)),
        },
    }
}

/// The full collider for an entity — its [`collider_shape_for`] shape plus the material coefficients from
/// the `Collider` component (`density`/`friction`/`restitution`, sane defaults). So an **edit-at-pause**
/// to `Collider.friction` lands in the very next recorded run (M8.4 deliverable 4).
fn collider_desc_for(
    assets: &AssetsRuntime,
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> ColliderDesc {
    let comps = engine.components_of(id);
    let col = comps.get("Collider");
    let num = |field: &str, default: f64| -> f64 {
        col.and_then(|m| m.get(field)).map_or(default, |v| match v {
            FieldValue::Number(n) => *n,
            FieldValue::Integer(i) => *i as f64,
            _ => default,
        })
    };
    ColliderDesc {
        shape: collider_shape_for(assets, engine, id),
        density: num("density", 1.0),
        friction: num("friction", 0.5),
        restitution: num("restitution", 0.0),
    }
}

/// Capture an ECS physics entity as a recorded body. `None` unless it has BOTH a RigidBody and a Collider
/// (the ECS is authoritative — an incomplete body isn't simulated). If the entity is ALREADY live in
/// `old_body_of`, its **current** simulated transform + velocity become the recorded initial state — so
/// restarting a run mid-motion (e.g. dropping a second ball) doesn't snap the rest of the scene back to
/// spawn. Otherwise (a fresh body) it starts at rest at its ECS `Transform`.
fn record_body(
    engine: &Engine<FlecsWorld>,
    assets: &AssetsRuntime,
    old_sim: &RapierPhysics,
    old_body_of: &HashMap<EntityId, BodyHandle>,
    id: EntityId,
) -> Option<(BodyDesc, ColliderDesc)> {
    let comps = engine.components_of(id);
    if !comps.contains_key("RigidBody") || !comps.contains_key("Collider") {
        return None;
    }
    let kind = match comps.get("RigidBody").and_then(|m| m.get("kind")) {
        Some(FieldValue::Str(k)) if k == "fixed" => BodyKind::Fixed,
        Some(FieldValue::Str(k)) if k == "kinematicPosition" => BodyKind::KinematicPosition,
        Some(FieldValue::Str(k)) if k == "kinematicVelocity" => BodyKind::KinematicVelocity,
        _ => BodyKind::Dynamic,
    };
    let (translation, rotation, linvel, angvel) = match old_body_of.get(&id) {
        Some(h) => {
            let (t, q) = old_sim
                .transform(*h)
                .unwrap_or((body_spawn_pos(engine, id), [0.0, 0.0, 0.0, 1.0]));
            let (lv, av) = old_sim.velocity(*h).unwrap_or(([0.0; 3], [0.0; 3]));
            (t, q, lv, av)
        }
        None => (
            body_spawn_pos(engine, id),
            [0.0, 0.0, 0.0, 1.0],
            [0.0; 3],
            [0.0; 3],
        ),
    };
    let body = BodyDesc {
        kind,
        translation,
        rotation,
        linvel,
        angvel,
        can_sleep: true,
    };
    Some((body, collider_desc_for(assets, engine, id)))
}

/// Start a fresh deterministic RUN from the current ECS: a [`Recording`] of a fixed ground (body 0) +
/// every ECS physics body (captured at its current simulated state via [`record_body`], so the scene
/// doesn't jump) + EMPTY inputs (past shoves are baked into the captured state). Returns the recording,
/// the body-index → entity map (index 0 = the ground = `None`), the freshly-built `sim`, and the rebuilt
/// `body_of`. The caller resets `frame`/`max_frame` to 0. This is the M8.4 timeline's "scene changed →
/// new run" reset, and the M8.2/M8.3 mirror-into-the-sim path unified.
fn restart_run(
    engine: &Engine<FlecsWorld>,
    assets: &AssetsRuntime,
    old_sim: &RapierPhysics,
    old_body_of: &HashMap<EntityId, BodyHandle>,
) -> (
    Recording,
    Vec<Option<EntityId>>,
    RapierPhysics,
    HashMap<EntityId, BodyHandle>,
) {
    let mut recording = Recording::new(Fidelity::Gameplay.resolve().config);
    let mut entities: Vec<Option<EntityId>> = Vec::new();
    // Body 0 — a fixed ground plane at y=0 (top surface y=0.5, matching the viewport grid) so dropped
    // bodies fall + REST. Sim-only world geometry, never an ECS entity.
    recording.add_body(
        BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]),
        ColliderDesc::new(ColliderShape::Cuboid {
            half_extents: [60.0, 0.5, 60.0],
        }),
    );
    entities.push(None);
    for id in engine.entity_ids() {
        if let Some((body, collider)) = record_body(engine, assets, old_sim, old_body_of, id) {
            recording.add_body(body, collider);
            entities.push(Some(id));
        }
    }
    let (sim, handles) = recording.build();
    let mut body_of = HashMap::new();
    for (i, ent) in entities.iter().enumerate() {
        if let (Some(eid), Some(h)) = (ent, handles.get(i)) {
            body_of.insert(*eid, *h);
        }
    }
    (recording, entities, sim, body_of)
}

/// The recording-body index for an entity (the inverse of `rec_entities`) — so a shove records its input
/// against the right body. `None` if the entity isn't a recorded body.
fn rec_index_of(rec_entities: &[Option<EntityId>], id: EntityId) -> Option<usize> {
    rec_entities.iter().position(|e| *e == Some(id))
}

/// Install a scrubbed [`Replay`] as the live `sim`: take its world + rebuild `body_of` from the
/// recording-index → entity map. The replay world IS `recording`-built + stepped to the target frame, so
/// a resume continues bit-identically to a never-paused run (M8.4 P2/P3 — proven headless in `/physics`).
fn install_replay(
    replay: Replay,
    rec_entities: &[Option<EntityId>],
) -> (RapierPhysics, HashMap<EntityId, BodyHandle>) {
    let (world, handles) = replay.into_parts();
    let mut body_of = HashMap::new();
    for (i, ent) in rec_entities.iter().enumerate() {
        if let (Some(eid), Some(h)) = (ent, handles.get(i)) {
            body_of.insert(*eid, *h);
        }
    }
    (world, body_of)
}

// M8.4 contact-debugger overlay colours. Contacts are hot (load); a saturated-friction contact flips to
// white-hot (the jitter flag); the swept trajectory is cool.
const OVERLAY_CONTACT_COLOR: [f32; 3] = [1.0, 0.35, 0.2];
const OVERLAY_NORMAL_COLOR: [f32; 3] = [1.0, 0.85, 0.2];
const OVERLAY_SWEEP_COLOR: [f32; 3] = [0.3, 0.7, 1.0];

/// Build + publish the contact/solver debugger overlay (M8.4 deliverable 2) into the shared render state:
/// a small cross at each contact point, a segment along each contact normal (white-hot when friction is
/// saturated — the jitter flag), and the per-body **swept volume** as the prev→current trajectory segment
/// (the report's "4D spacetime collision" rendered as a *visualization*, not a 4D authoring system — the
/// honest scope). READ-ONLY: built from the diagnostic seam, it never perturbs the sim. Drawn by the
/// always-pass line pass so it reads as an overlay over the scene.
fn push_overlay(
    sim: &RapierPhysics,
    body_of: &HashMap<EntityId, BodyHandle>,
    prev_centers: &HashMap<EntityId, [f32; 3]>,
    shared: &Shared,
) {
    let mut seg: Vec<Instance> = Vec::new();
    let mut push = |a: [f32; 3], b: [f32; 3], color: [f32; 3]| {
        seg.push(Instance {
            center: a,
            scale: 0.0,
            color,
            selected: 0.0,
            rotation: render::IDENTITY_QUAT,
            material: [0.0; 4],
        });
        seg.push(Instance {
            center: b,
            scale: 0.0,
            color,
            selected: 0.0,
            rotation: render::IDENTITY_QUAT,
            material: [0.0; 4],
        });
    };
    for c in &sim.diagnostics().contacts {
        let p = [c.point[0] as f32, c.point[1] as f32, c.point[2] as f32];
        let n = [c.normal[0] as f32, c.normal[1] as f32, c.normal[2] as f32];
        let s = 0.08_f32;
        push(
            [p[0] - s, p[1], p[2]],
            [p[0] + s, p[1], p[2]],
            OVERLAY_CONTACT_COLOR,
        );
        push(
            [p[0], p[1] - s, p[2]],
            [p[0], p[1] + s, p[2]],
            OVERLAY_CONTACT_COLOR,
        );
        push(
            [p[0], p[1], p[2] - s],
            [p[0], p[1], p[2] + s],
            OVERLAY_CONTACT_COLOR,
        );
        let nl = 0.4_f32;
        let color = if c.friction_saturated {
            [1.0, 1.0, 1.0]
        } else {
            OVERLAY_NORMAL_COLOR
        };
        push(
            p,
            [p[0] + n[0] * nl, p[1] + n[1] * nl, p[2] + n[2] * nl],
            color,
        );
    }
    for (eid, h) in body_of {
        if let (Some((t, _)), Some(prev)) = (sim.transform(*h), prev_centers.get(eid)) {
            let cur = [t[0] as f32, t[1] as f32, t[2] as f32];
            if (prev[0] - cur[0]).abs() + (prev[1] - cur[1]).abs() + (prev[2] - cur[2]).abs() > 1e-4
            {
                push(*prev, cur, OVERLAY_SWEEP_COLOR);
            }
        }
    }
    let mut st = shared.lock().unwrap();
    st.overlay_lines = seg;
    st.overlay_revision = st.overlay_revision.wrapping_add(1);
}

/// Clear the debugger overlay (when it closes) — zero per-frame cost while off.
fn clear_overlay(shared: &Shared) {
    let mut st = shared.lock().unwrap();
    if !st.overlay_lines.is_empty() {
        st.overlay_lines.clear();
        st.overlay_revision = st.overlay_revision.wrapping_add(1);
    }
}

/// M12.5 (ADR-049) — assemble the live truth-state debugger payload from the Play-time `RuleReplay` (the
/// "debug by looking" read). For the selected entity: its truth-state (rules with per-condition ✅/❌ + the
/// machine current state) + each rule's `explain_rule` narration, plus the frame-stamped decision history and
/// the determinism-flagged rules. A pure read over the runtime state — never mutates the run or the doc. When
/// not playing, an empty info (`playing:false`) so the UI shows the authoring state.
fn rule_debug_info(
    playing: bool,
    session: Option<&metrocalk_core::RuleReplay>,
    flagged: &[metrocalk_core::FlaggedRule],
    head: u64,
    selected: Option<&str>,
) -> RuleDebugInfo {
    let Some(session) = session else {
        return RuleDebugInfo::default();
    };
    let truth = selected.map(|id| session.truth_state(id));
    // One plain-language explanation per rule shown for the entity (the M3.1/M8.4 explain engine on logic).
    let explanations = truth
        .as_ref()
        .map(|t| {
            t.rules
                .iter()
                .filter_map(|r| {
                    session.explain_rule(&r.rule).map(|text| RuleExplain {
                        rule: r.rule.clone(),
                        text,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    RuleDebugInfo {
        playing,
        frame: session.frame(),
        head,
        truth,
        explanations,
        decisions: session.history().to_vec(),
        flagged: flagged.to_vec(),
    }
}

/// M8.2 STEP 4 — the per-tick transform DELTA sync (hot path off JS). Writes each moved body's world
/// position straight into the shared render [`SceneState`] (the projection the render loop reads every
/// vsync) and bumps `revision` so the loop re-uploads — in place, no full rebuild, no `engine.commit`,
/// no `Channel` send (invariants 2 + 4). Rotation is dropped (the cube/mesh shaders only translate +
/// uniform-scale today — a rotation `Instance` field is a deferred render extension). Returns the centres
/// it wrote (entity → world centre) so the caller can feed the swept-volume overlay the previous frame.
fn sync_out(
    sim: &RapierPhysics,
    body_of: &HashMap<EntityId, BodyHandle>,
    shared: &Shared,
) -> HashMap<EntityId, [f32; 3]> {
    let mut centers = HashMap::with_capacity(body_of.len());
    let mut st = shared.lock().unwrap();
    let mut moved = false;
    for (eid, h) in body_of {
        if let Some((t, q)) = sim.transform(*h) {
            let c = [t[0] as f32, t[1] as f32, t[2] as f32];
            // The body's orientation (quat xyzw) — so a tumbling body actually LOOKS like it's tumbling,
            // not sliding (the renderer-rotation path; M8.2 dropped this). The render shaders apply it.
            let rot = [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32];
            centers.insert(*eid, c);
            let key = eid.to_loro_key();
            if let Some(i) = st.ids.iter().position(|k| *k == key) {
                if i < st.instances.len() {
                    st.instances[i].center = c;
                    st.instances[i].rotation = rot;
                    moved = true;
                }
            }
        }
    }
    if moved {
        st.revision = st.revision.wrapping_add(1);
    }
    centers
}

/// Build the hover-tooltip [`EntityDetails`] for `id` (name · components · provided/required caps via
/// their display names · the entities it's bound to). `None` if the id isn't a live entity.
fn build_entity_details(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    id: EntityId,
) -> Option<EntityDetails> {
    let ecs = engine.ecs_entity(id)?;
    let mut components: Vec<String> = engine.components_of(id).into_keys().collect();
    components.sort();
    let caps = |rel: Entity| -> Vec<String> {
        let mut v: Vec<String> = engine
            .world()
            .targets(ecs, rel)
            .iter()
            .filter_map(|c| scene.cap_name.get(c).cloned())
            .collect();
        v.sort();
        v.dedup();
        v
    };
    let bound_to: Vec<String> = engine
        .bindings()
        .into_iter()
        .filter(|(from, _, _)| *from == id)
        .map(|(_, _, to)| label_of(engine, to))
        .collect();
    Some(EntityDetails {
        id: id.to_loro_key(),
        name: label_of(engine, id),
        components,
        provides: caps(scene.rels.provides),
        requires: caps(scene.rels.requires),
        bound_to,
    })
}

/// Compute the reveal for `eid`: the required capabilities, the ranked compatible targets, a bounded
/// nearest-first set of greyed incompatibles (each with its specific reason), and the selection's
/// existing outgoing bindings. Uses the cached `positions` (built in [`rebuild`]) so the hot path is
/// the reveal's indexed query + a bounded `why_not` scan — never a fresh full-scene Loro read.
fn compute_reveal(
    engine: &Engine<FlecsWorld>,
    scene: &CapScene,
    positions: &HashMap<Entity, [f32; 3]>,
    recency: &HashMap<Entity, u64>,
    eid: EntityId,
) -> RevealResponse {
    let Some(sel_ecs) = engine.ecs_entity(eid) else {
        return RevealResponse::default();
    };
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: positions,
        recency,
    };
    let r = reveal(engine.world(), sel_ecs, scene.rels, &ctx);

    let label = |id: EntityId| label_of(engine, id);

    let compatible: Vec<Candidate> = r
        .compatible
        .iter()
        .filter_map(|c| {
            let id = engine.entity_id_of(c.entity)?;
            Some(Candidate {
                id: id.to_loro_key(),
                name: label(id),
                distance: c.distance,
                affinity: c.affinity,
            })
        })
        .collect();

    // Greyed: the nearest entities that have a reason they can't bind (bounded to 60 — the UI greys what
    // it shows). The selection's required caps are hoisted OUT of the per-candidate loop (perf audit F3) —
    // previously `why_not` re-ran `world.targets(selected, requires)` ~60× per select. With that hoisted,
    // `why_not_with_required` is O(1) per candidate, so a single O(n) pass + a bounded nearest-60
    // partial-select replaces the old sort-the-whole-scene O(n log n). The deep-Loro `label` read is
    // deferred to only the ≤60 kept.
    let sel_required = required_caps(engine.world(), sel_ecs, scene.rels);
    let req_set: HashSet<Entity> = sel_required.iter().copied().collect();
    let sel_pos = positions.get(&sel_ecs).copied().unwrap_or([0.0; 3]);
    let mut greyed_all: Vec<(f32, EntityId, String)> = engine
        .entity_ids()
        .into_iter()
        .filter(|&id| id != eid)
        .filter_map(|id| {
            let e = engine.ecs_entity(id)?;
            let wn = why_not_with_required(
                engine.world(),
                sel_ecs,
                scene.rels,
                e,
                &scene.cap_name,
                &sel_required,
                &req_set,
            )?;
            let p = positions.get(&e).copied().unwrap_or([0.0; 3]);
            Some((dist(sel_pos, p), id, wn.explain()))
        })
        .collect();
    // Nearest-60 by distance: a bounded partial-select (O(n)) rather than a full sort, then order the
    // kept few for a stable nearest-first presentation.
    if greyed_all.len() > 60 {
        greyed_all.select_nth_unstable_by(60, |a, b| {
            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        greyed_all.truncate(60);
    }
    greyed_all.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let greyed: Vec<Greyed> = greyed_all
        .into_iter()
        .map(|(_, id, reason)| Greyed {
            id: id.to_loro_key(),
            name: label(id),
            reason,
        })
        .collect();

    let bound: Vec<Bound> = engine
        .bindings()
        .into_iter()
        .filter(|(f, _, _)| *f == eid)
        .map(|(_, kind, to)| Bound {
            id: to.to_loro_key(),
            name: label(to),
            kind,
        })
        .collect();

    RevealResponse {
        required: r.required,
        compatible,
        greyed,
        bound,
    }
}

/// A short human label for an entity — its most salient stdlib component, else its id.
fn label_of(engine: &Engine<FlecsWorld>, id: EntityId) -> String {
    let comps = engine.components_of(id);
    for k in [
        "HealthBar",
        "Health",
        "Sprite",
        "MeshRenderer",
        "RigidBody",
        "AudioSource",
        "Light",
        "Camera",
    ] {
        if comps.contains_key(k) {
            return format!("{k}  ·  {}", id.to_loro_key());
        }
    }
    id.to_loro_key()
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Post-create echo for a freshly-instantiated entity (local describe OR marketplace apply): bump its
/// recency, send the targeted `project_entity` delta (deltas only, inv. 2), and rebuild the viewport.
#[allow(clippy::too_many_arguments)]
fn echo_created(
    engine: &mut Engine<FlecsWorld>,
    shared: &Shared,
    positions: &mut HashMap<Entity, [f32; 3]>,
    assets: &AssetsRuntime,
    channel: &Option<Channel<ProjectionDelta>>,
    recency: &mut HashMap<Entity, u64>,
    touch: &mut u64,
    id: EntityId,
) {
    if let Some(e) = engine.ecs_entity(id) {
        *touch += 1;
        recency.insert(e, *touch);
    }
    if let Some(ch) = channel {
        send_proj!(ch, project_entity(engine, id));
    }
    rebuild(engine, shared, positions, assets);
}

/// M9.4 — build the snap-graph hits for `dragged`: every OTHER entity's origin (or pivot, if it has
/// children) within `radius`, ranked by the **shared ADR-011 `intent_order`** (reused via
/// `transform_solver::snap_candidates` — not a parallel heuristic), each with an explained "why this".
fn snap_hits(
    engine: &Engine<FlecsWorld>,
    positions: &HashMap<Entity, [f32; 3]>,
    recency: &HashMap<Entity, u64>,
    dragged: EntityId,
    radius: f32,
) -> Vec<SnapHit> {
    let Some(dragged_ecs) = engine.ecs_entity(dragged) else {
        return Vec::new();
    };
    let from = positions.get(&dragged_ecs).copied().unwrap_or([0.0; 3]);
    let targets: Vec<SnapTarget> = positions
        .iter()
        .filter(|(e, _)| **e != dragged_ecs)
        .map(|(&e, &position)| {
            // A parent (has children) is a stronger spatial intent (a pivot) than a bare origin.
            let kind = engine
                .entity_id_of(e)
                .filter(|eid| !engine.children_of(*eid).is_empty())
                .map_or(SnapKind::Origin, |_| SnapKind::Pivot);
            SnapTarget {
                entity: e,
                kind,
                position,
            }
        })
        .collect();
    transform_solver::snap_candidates(&targets, from, radius, recency)
        .into_iter()
        .filter_map(|c| {
            let eid = engine.entity_id_of(c.entity)?;
            Some(SnapHit {
                id: eid.to_loro_key(),
                kind: c.kind.label().to_string(),
                x: c.position[0],
                y: c.position[1],
                z: c.position[2],
                distance: c.distance,
                why: format!(
                    "snap to the {} of {} — {:.2} m",
                    c.kind.label(),
                    eid.to_loro_key(),
                    c.distance
                ),
            })
        })
        .collect()
}

/// Commit a constraint-solved **world** transform to `eid` through the one pipeline (M9.1/M9.2 routing,
/// undoable): a CHILD part gets the parent-space-write-back **override** (+ `Record::EditPart`); a root
/// gets `set_transform` (+ `Record::Transform`). Returns whether it applied.
fn commit_world_transform(
    engine: &mut Engine<FlecsWorld>,
    log: &Log,
    eid: EntityId,
    world: GizmoTransform,
) -> bool {
    if engine.parent_of(eid).is_some() {
        match capscene::edit_part_transform(engine, eid, world) {
            Ok(local) => {
                log.append(&Record::EditPart {
                    id: eid.to_loro_key(),
                    x: f64::from(local.translation[0]),
                    y: f64::from(local.translation[1]),
                    z: f64::from(local.translation[2]),
                    qx: f64::from(local.rotation[0]),
                    qy: f64::from(local.rotation[1]),
                    qz: f64::from(local.rotation[2]),
                    qw: f64::from(local.rotation[3]),
                    scale: f64::from(local.scale[0]),
                });
                true
            }
            Err(_) => false,
        }
    } else if capscene::set_transform(
        engine,
        eid,
        world.translation,
        world.rotation,
        world.scale[0],
    )
    .is_ok()
    {
        log.append(&Record::Transform {
            id: eid.to_loro_key(),
            x: f64::from(world.translation[0]),
            y: f64::from(world.translation[1]),
            z: f64::from(world.translation[2]),
            qx: f64::from(world.rotation[0]),
            qy: f64::from(world.rotation[1]),
            qz: f64::from(world.rotation[2]),
            qw: f64::from(world.rotation[3]),
            scale: f64::from(world.scale[0]),
        });
        true
    } else {
        false
    }
}

/// The product of a part's ancestors' local display scales (each `Transform.scale` field, default 1) —
/// so a child renders enlarged/shrunk with a scaled parent (M9.2 hierarchy scale propagation). `1.0`
/// for a root (empty chain) ⇒ flat entities are unaffected.
fn ancestor_scale_product(engine: &Engine<FlecsWorld>, id: EntityId) -> f32 {
    let mut s = 1.0;
    let mut cur = engine.parent_of(id);
    while let Some(p) = cur {
        s *= capscene::local_transform(engine, p).scale[0];
        cur = engine.parent_of(p);
    }
    s
}

/// Rebuild the viewport instance list AND the cached `positions` map from the engine's `Transform`
/// components in one pass (scene truth → viewport + reveal input). The only place scene geometry flows
/// core → viewport.
/// Build a [`ProjectInfoResp`] for the File menu (M10.3) — the path, the dirty flag, the (freshly-read)
/// recents, and an optional explained error. Keeps the four project-command arms terse.
fn project_info(
    path: Option<&std::path::Path>,
    dirty: bool,
    recents_path: &std::path::Path,
    error: Option<String>,
) -> ProjectInfoResp {
    ProjectInfoResp {
        path: path.map(|p| p.display().to_string()),
        dirty,
        recents: mtk_project::load_recents(recents_path),
        error,
    }
}

/// M11.3 (ADR-042) — gather the scene's authored `Light` entities into GPU lights (a render projection: the
/// light ENTITY is the undoable Loro doc state; this is its per-frame upload, never written back to Loro).
/// `dir` is the direction a directional/spot light SHINES (so the shader's L = -dir); point/spot use the
/// entity Transform position + `range`. Falls back to a single default key light (the prior hard-coded
/// directional, now a real list entry) so a scene with no light entities still renders lit, not black.
/// Returns the GPU light list AND the INDEX of the scene's shadow-casting light (M11.3 inc.3): the FIRST
/// directional `Light` whose `castShadows` isn't explicitly false, else the default key light. `None` ⇒
/// nothing casts (e.g. only point lights authored) → the render skips the shadow pass. The index (not the
/// direction) so the shader can apply the single shadow map to ONLY that light, not every directional.
fn collect_lights(engine: &Engine<FlecsWorld>) -> (Vec<render::LightGpu>, Option<usize>) {
    let read = |m: &HashMap<String, FieldValue>, f: &str, d: f32| -> f32 {
        m.get(f).map_or(d, |v| match v {
            FieldValue::Number(n) => *n as f32,
            FieldValue::Integer(i) => *i as f32,
            _ => d,
        })
    };
    let mut lights: Vec<render::LightGpu> = Vec::new();
    let mut shadow_caster: Option<usize> = None;
    for id in engine.entity_ids() {
        let comps = engine.components_of(id);
        let Some(light) = comps.get("Light") else {
            continue;
        };
        let t = comps.get("Transform");
        let pos = t.map_or([0.0, 0.0, 0.0], |tm| {
            [read(tm, "x", 0.0), read(tm, "y", 0.0), read(tm, "z", 0.0)]
        });
        let kind = match light.get("kind") {
            Some(FieldValue::Str(s)) => match s.as_str() {
                "point" => 1.0,
                "spot" => 2.0,
                _ => 0.0,
            },
            _ => 0.0, // default: directional
        };
        let dir = [
            read(light, "dirX", 0.0),
            read(light, "dirY", -1.0),
            read(light, "dirZ", 0.0),
        ];
        // M11.3 inc.3 — the first directional light that casts is the shadow caster (a single shadow map).
        // Record its INDEX (the slot it's about to occupy) so the shader shadows only this one light.
        if shadow_caster.is_none() && kind == 0.0 {
            let casts = !matches!(light.get("castShadows"), Some(FieldValue::Bool(false)));
            if casts {
                shadow_caster = Some(lights.len());
            }
        }
        lights.push(render::LightGpu {
            pos_kind: [pos[0], pos[1], pos[2], kind],
            color_intensity: [
                read(light, "r", 1.0),
                read(light, "g", 1.0),
                read(light, "b", 1.0),
                read(light, "intensity", 1.0),
            ],
            dir_range: [dir[0], dir[1], dir[2], read(light, "range", 0.0)],
        });
    }
    if lights.is_empty() {
        // The default key light — the prior hard-coded directional (M11.2's LIGHT_DIR was the dir TO the
        // light, so the SHINE direction is its negation), intensity 2.4. Keeps unlit scenes readable, and
        // it casts the default shadow.
        lights.push(render::LightGpu {
            pos_kind: [0.0, 0.0, 0.0, 0.0],
            color_intensity: [1.0, 1.0, 1.0, 2.4],
            dir_range: [-0.4, -0.8, -0.3, 0.0],
        });
        shadow_caster = Some(0); // the default key light (index 0) casts
    }
    (lights, shadow_caster)
}

/// M11.4 — one line-segment endpoint for a marker icon glyph (only `center`/`color` are read by the overlay
/// shader; the rest are inert, matching the gizmo/line carriers).
fn glyph_pt(center: [f32; 3], color: [f32; 3]) -> Instance {
    Instance {
        center,
        scale: 0.0,
        color,
        selected: 0.0,
        rotation: render::IDENTITY_QUAT,
        material: [0.0; 4],
    }
}

/// M11.4 — a light marker glyph: a warm burst of rays from `p` (a recognizable "light", color-coded), as
/// `LineList` endpoint pairs for the overlay pass.
fn light_glyph(p: [f32; 3]) -> Vec<Instance> {
    const C: [f32; 3] = [1.0, 0.82, 0.2]; // warm amber (reads against the grey sky)
    let r = 0.5_f32;
    let d = r * 0.6;
    let rays = [
        [r, 0.0, 0.0],
        [-r, 0.0, 0.0],
        [0.0, r, 0.0],
        [0.0, -r, 0.0],
        [0.0, 0.0, r],
        [0.0, 0.0, -r],
        [d, d, d],
        [-d, -d, -d],
        [d, -d, -d],
        [-d, d, d],
    ];
    let mut out = Vec::with_capacity(rays.len() * 2);
    for ray in rays {
        out.push(glyph_pt(p, C));
        out.push(glyph_pt([p[0] + ray[0], p[1] + ray[1], p[2] + ray[2]], C));
    }
    out
}

/// M11.4 — a camera marker glyph: a small wireframe frustum at `p` opening along -Z (cyan), as `LineList`
/// endpoint pairs.
fn camera_glyph(p: [f32; 3]) -> Vec<Instance> {
    const C: [f32; 3] = [0.35, 0.8, 1.0]; // cyan
    let s = 0.22_f32;
    let dz = -0.42_f32;
    let corners = [
        [p[0] - s, p[1] - s, p[2] + dz],
        [p[0] + s, p[1] - s, p[2] + dz],
        [p[0] + s, p[1] + s, p[2] + dz],
        [p[0] - s, p[1] + s, p[2] + dz],
    ];
    let mut out = Vec::with_capacity(16);
    for c in corners {
        out.push(glyph_pt(p, C)); // apex → corner
        out.push(glyph_pt(c, C));
    }
    for i in 0..4 {
        out.push(glyph_pt(corners[i], C)); // front-rect edge
        out.push(glyph_pt(corners[(i + 1) % 4], C));
    }
    out
}

fn rebuild(
    engine: &Engine<FlecsWorld>,
    shared: &Shared,
    positions: &mut HashMap<Entity, [f32; 3]>,
    assets: &AssetsRuntime,
) {
    positions.clear();
    let mut instances = Vec::new();
    let mut ids = Vec::new();
    // Per-instance render routing: -1 ⇒ cube placeholder, else the imported-mesh slot (parallel to
    // `instances`). An entity with a `MeshRenderer.mesh` handle the store knows renders as that mesh.
    let mut mesh_slots: Vec<i32> = Vec::new();
    // M9.4 — per-instance snap affinity (parallel to `instances`): a parent (a pivot) outranks a bare
    // origin in the snap ranker. Built here so the render-thread snap (`nearest_snap`) stays 0-IPC.
    let mut snap_affinity: Vec<u32> = Vec::new();
    // M11.4 — wireframe ICON glyphs for light/camera marker entities (line-segment endpoint pairs). Markers
    // are drawn as these glyphs, NOT as solid placeholder cubes (see the marker skip below).
    let mut marker_glyphs: Vec<Instance> = Vec::new();
    for id in engine.entity_ids() {
        // M9.2 deactivate-not-delete: a deactivated PART is hidden from the viewport (the entity + its
        // data survive; undo re-activates it → it reappears on the next rebuild). Only children can be
        // deactivated, so flat entities skip the (override-map) `is_active` read entirely.
        let is_child = engine.parent_of(id).is_some();
        if is_child && !engine.is_active(id) {
            continue;
        }
        let comps = engine.components_of(id);
        let t = comps.get("Transform");
        let get = |f: &str| -> f32 {
            t.and_then(|m| m.get(f)).map_or(0.0, |v| match v {
                FieldValue::Number(n) => *n as f32,
                FieldValue::Integer(i) => *i as f32,
                _ => 0.0,
            })
        };
        // M11.4 — a pure light/camera MARKER (a Transform but no MeshRenderer) is not scene geometry: render
        // it as a wireframe ICON glyph (burst / frustum), never a solid placeholder cube. It stays in the
        // hierarchy (selectable there) + the inspector (numeric Transform edits); it just isn't an `instances`
        // entry, so it has no viewport gizmo (the gizmo indexes `instances`) — the documented icon trade.
        if !comps.contains_key("MeshRenderer")
            && (comps.contains_key("Light") || comps.contains_key("Camera"))
        {
            let p = [get("x"), get("y"), get("z")];
            marker_glyphs.extend(if comps.contains_key("Camera") {
                camera_glyph(p)
            } else {
                light_glyph(p)
            });
            continue;
        }
        // A geometry-free ASSEMBLY/GROUP container from a CAD import (a named `__meta__.kind == "group"` node
        // with no `MeshRenderer`): a pure hierarchy node — the outliner shows it (the source's exact assembly
        // tree / grouping) and its parts parent under it, but it is NEVER scene geometry → render nothing (no
        // placeholder cube, no glyph). Same pattern as the light/camera marker skip above.
        if !comps.contains_key("MeshRenderer")
            && comps
                .get(metrocalk_core::variant::INSTANCE_META)
                .and_then(|m| m.get("kind"))
                == Some(&FieldValue::Str("group".into()))
        {
            continue;
        }
        // Resolve the entity's mesh handle (if any) to a render slot + normalized scale.
        let slot = comps
            .get("MeshRenderer")
            .and_then(|m| m.get(capscene::MESH_FIELD))
            .and_then(|v| match v {
                FieldValue::Str(h) => assets.handle_to_slot.get(h).copied(),
                _ => None,
            });
        let asset_scale = slot.map_or(0.45, |s| assets.scales.get(s).copied().unwrap_or(0.45));
        // M9.2: a CHILD part renders at its **global** transform (`parent·local`, override-resolved) so
        // descendants follow a parent edit; a FLAT entity (root / instance root / physics body) keeps the
        // exact M9.1 base read — zero behavior change + zero override-resolution cost for the 5k scene.
        let (p, rot, scale) = if is_child {
            let g = capscene::global_transform(engine, id);
            let resolved_scale = engine
                .resolved_components(id)
                .get("Transform")
                .and_then(|m| m.get("scale"))
                .and_then(|v| match v {
                    // A whole-number scale (e.g. `2.0`) round-trips JSON as an Integer (json_to_field), so
                    // accept BOTH — else an integer scale silently falls through to the asset base.
                    FieldValue::Number(s) if *s > 0.0 => Some(*s as f32),
                    FieldValue::Integer(i) if *i > 0 => Some(*i as f32),
                    _ => None,
                });
            let own = resolved_scale.unwrap_or(asset_scale);
            let rot = if g.rotation == render::IDENTITY_QUAT {
                render::IDENTITY_QUAT
            } else {
                g.rotation
            };
            (g.translation, rot, own * ancestor_scale_product(engine, id))
        } else {
            let p = [get("x"), get("y"), get("z")];
            // Authored rotation (M9.1+ renderer-rotation path): qx/qy/qz/qw, identity when unauthored. A
            // physics body's per-tick rotation overrides this live via sync_out.
            let rot = {
                let q = [get("qx"), get("qy"), get("qz"), get("qw")];
                if q == [0.0; 4] {
                    render::IDENTITY_QUAT
                } else {
                    q
                }
            };
            // An authored display scale (a gizmo scale-edit, field `scale`) overrides the asset-normalized
            // base; absent ⇒ the base. So a scaled entity reloads at its edited size.
            let scale = match t.and_then(|m| m.get("scale")) {
                Some(FieldValue::Number(s)) if *s > 0.0 => *s as f32,
                // A whole-number scale (e.g. `2.0`) arrives as an Integer (json_to_field maps any whole
                // JSON number to Integer), so accept it too — otherwise an integer scale silently reverts
                // to the asset base (the bug where `scale=2` rendered identically to the default).
                Some(FieldValue::Integer(i)) if *i > 0 => *i as f32,
                _ => asset_scale,
            };
            (p, rot, scale)
        };
        if let Some(e) = engine.ecs_entity(id) {
            positions.insert(e, p);
        }
        let key = id.to_loro_key();
        // M11.2: a per-entity PBR material override from MeshRenderer.material (intent/AI-edit). When a
        // recognised preset is set, the shader uses its metallic/roughness + base color for THIS entity.
        let (mat_override, override_color) = comps
            .get("MeshRenderer")
            .and_then(|m| m.get("material"))
            .and_then(|v| match v {
                FieldValue::Str(s) => material_preset(s),
                _ => None,
            })
            .map_or(([0.0; 4], None), |(color, metallic, roughness)| {
                ([metallic, roughness, 1.0, 0.0], Some(color))
            });
        let c = override_color.unwrap_or_else(|| color_for(&key));
        instances.push(Instance {
            center: p,
            scale,
            color: c,
            selected: 0.0,
            rotation: rot,
            material: mat_override,
        });
        mesh_slots.push(slot.map_or(-1, |s| i32::try_from(s).unwrap_or(-1)));
        // A parent (has children) is a stronger spatial snap target (a pivot) than a bare origin
        // (`SnapKind::Pivot`=6 vs `Origin`=0 in the shared ranker's affinity).
        snap_affinity.push(if engine.children_of(id).is_empty() {
            0
        } else {
            6
        });
        ids.push(key);
    }
    // Tracking lines: one segment per binding, between the bound entities' centres — what makes a
    // *restored* bind visible on reload (the engine has the binding, the viewport now draws it) with no
    // click. Built by the pure, unit-tested `capscene::tracking_segments`; `vs_line` reads only `center`.
    let line_points: Vec<Instance> = capscene::tracking_segments(engine)
        .into_iter()
        .map(|center| Instance {
            center,
            scale: 0.0,
            color: TRACK_LINE_COLOR,
            selected: 0.0,
            rotation: render::IDENTITY_QUAT,
            material: [0.0; 4],
        })
        .collect();
    // M11.3 — the scene's lights + the shadow-caster index (a render projection from the authored Light
    // entities; inc.3 — castShadows picks the caster).
    let (lights, shadow_caster) = collect_lights(engine);
    let mut st = shared.lock().unwrap();
    // Preserve selection by entity ID, NOT index: the instance index is invalidated whenever entities are
    // added/removed, so restoring `selected`/`gizmo_sel` by index would silently retarget a DIFFERENT
    // entity (e.g. deleting an entity before the selected one). Capture the ids before `ids` is replaced.
    let prev_sel_id = st.selected.and_then(|i| st.ids.get(i).cloned());
    let prev_gizmo_id = st.gizmo_sel.and_then(|i| st.ids.get(i).cloned());
    st.instances = instances;
    st.ids = ids;
    st.mesh_slots = mesh_slots;
    st.snap_affinity = snap_affinity;
    st.line_points = line_points;
    st.marker_glyphs = marker_glyphs;
    st.lights = lights;
    st.shadow_caster = shadow_caster;
    st.lights_revision = st.lights_revision.wrapping_add(1);
    st.selected = prev_sel_id.and_then(|id| st.ids.iter().position(|k| *k == id));
    if let Some(i) = st.selected {
        st.instances[i].selected = 1.0;
    }
    // Keep an active gizmo drag pinned to its entity by ID; if the dragged entity is gone (deleted
    // elsewhere), end the drag cleanly rather than freezing it on a stale index.
    st.gizmo_sel = prev_gizmo_id.and_then(|id| st.ids.iter().position(|k| *k == id));
    if st.gizmo_dragging && st.gizmo_sel.is_none() {
        st.gizmo_dragging = false;
        st.gizmo.drag_end();
    }
    st.revision = st.revision.wrapping_add(1);
}

/// The tracking-line colour (matches the panel's `#9fe` "tracking" accent). Only carried for parity;
/// `vs_line` uses its own constant colour.
const TRACK_LINE_COLOR: [f32; 3] = [0.60, 1.0, 0.93];

#[allow(clippy::cast_precision_loss)] // hashing a key to a display color — precision is irrelevant
fn color_for(key: &str) -> [f32; 3] {
    let mut h: u32 = 2_166_136_261;
    for b in key.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
    }
    [
        0.4 + (h & 0xff) as f32 / 425.0,
        0.4 + ((h >> 8) & 0xff) as f32 / 425.0,
        0.4 + ((h >> 16) & 0xff) as f32 / 425.0,
    ]
}

/// M11.2 (ADR-041) — a named PBR material preset → `(base_color, metallic, roughness)`. Assigned per-entity
/// via the `MeshRenderer.material` field (the intent-assign / AI-edit path that reuses `apply_ai_patch`,
/// e.g. the "weathered-metal look" edit sets `"rusty"`); the render applies it as a per-instance override of
/// the asset's BAKED material, so one entity is "made metal/rusty/gold" without touching shared geometry.
/// Unknown / absent names → `None` (the baked asset material renders unchanged).
fn material_preset(name: &str) -> Option<([f32; 3], f32, f32)> {
    Some(match name {
        "rusty" | "rust" | "weathered" => ([0.42, 0.22, 0.12], 0.55, 0.65),
        "metal" | "metallic" | "steel" | "iron" => ([0.56, 0.57, 0.58], 1.0, 0.35),
        "chrome" | "mirror" | "polished" => ([0.55, 0.56, 0.58], 1.0, 0.06),
        "gold" => ([1.0, 0.78, 0.34], 1.0, 0.22),
        "copper" | "bronze" => ([0.95, 0.60, 0.40], 1.0, 0.30),
        "plastic" | "matte" => ([0.80, 0.80, 0.82], 0.0, 0.55),
        _ => return None,
    })
}

// ── tauri commands (UI → core) ─────────────────────────────────────────────────

/// Count one UI→core boundary crossing (render::IPC_CALLS) — the instrumentation behind the
/// zero-per-frame-IPC claim (invariant 4). Every command calls this exactly once.
fn ipc() {
    render::IPC_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// How long a command waits for the engine thread's reply before giving up (perf audit RC-1 / F1).
/// The commands are `#[tauri::command(async)]` (off the tao event-loop thread), so this bound protects
/// nothing per-frame — it only stops a *stalled/panicked* engine from parking a caller forever. It must
/// therefore be LONGER than any legitimate op: a busy engine (mid-way through landing a 262 MB CAD
/// import, a big undo, a whole-world rebuild) is *queued, not stalled* — a short bound here made every
/// queued command falsely report failure (undo → "nothing to undo", open → no project) while the op then
/// landed anyway. Fast-but-wrong is worse than slow-but-truthful; the window stays live either way.
const ENGINE_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// The reply bound for the IMPORT-class commands (`import_asset`(+dialog) / `import_interchange`): a
/// multi-hundred-MB CAD container legitimately takes minutes to parse + land on the serial engine
/// thread, and a false `None` here reads as "unsupported/malformed" in the UI while the assembly then
/// appears — the exact dishonest-state the M15.7 never-silent thesis forbids.
const IMPORT_REPLY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);

/// One-shot latch so a genuine engine stall logs ONCE (not once per queued command) and again on
/// recovery — never a hot-path spam of `eprintln!` (audit §8 flagged blocking stderr on hot paths).
static ENGINE_STALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Bounded blocking wait for an engine-thread reply (perf audit RC-1 / F1). A drop-in for the old
/// `recv()`: returns `Result` so every existing call site keeps its `.unwrap_or(..)` / `.ok().flatten()`
/// combinator, but on timeout it returns `Err` (→ the caller's stale/default fallback) instead of the
/// old unbounded `recv()` that parked the thread — the mechanism that painted *(Not Responding)*.
fn recv_reply<T>(rx: &mpsc::Receiver<T>) -> Result<T, mpsc::RecvTimeoutError> {
    recv_reply_within(rx, ENGINE_REPLY_TIMEOUT)
}

/// [`recv_reply`] with an explicit bound — the import-class commands pass [`IMPORT_REPLY_TIMEOUT`].
fn recv_reply_within<T>(
    rx: &mpsc::Receiver<T>,
    timeout: std::time::Duration,
) -> Result<T, mpsc::RecvTimeoutError> {
    use std::sync::atomic::Ordering::Relaxed;
    match rx.recv_timeout(timeout) {
        Ok(v) => {
            if ENGINE_STALLED.swap(false, Relaxed) {
                eprintln!("[shell] engine thread responsive again");
            }
            Ok(v)
        }
        Err(e) => {
            if matches!(e, mpsc::RecvTimeoutError::Timeout) && !ENGINE_STALLED.swap(true, Relaxed) {
                eprintln!(
                    "[shell] engine reply exceeded {timeout:?}; returning stale/default so the window stays live (perf audit F1)"
                );
            }
            Err(e)
        }
    }
}

#[tauri::command]
fn connect(state: State<AppState>, channel: Channel<ProjectionDelta>) {
    ipc();
    let _ = state.tx.send(EngineCmd::Connect(channel));
}

#[tauri::command]
fn submit_edit(state: State<AppState>, tx: EditTx) {
    ipc();
    let _ = state.tx.send(EngineCmd::Edit(tx));
}

#[tauri::command(async)]
fn undo(state: State<AppState>) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Undo { reply }).is_err() {
        return false;
    }
    recv_reply(&rx).unwrap_or(false) // true iff a transaction was actually reverted (honest "undo" vs "nothing to undo")
}

/// Reveal bindable targets for a selected entity (north-star test #1). Blocks briefly on the engine
/// thread's reply (a read).
#[tauri::command(async)]
fn reveal_targets(state: State<AppState>, id: String) -> RevealResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Reveal { id, reply }).is_err() {
        return RevealResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Bind the selection to a chosen compatible target (one undoable transaction).
#[tauri::command]
fn bind_target(state: State<AppState>, from: String, to: String) {
    ipc();
    let _ = state.tx.send(EngineCmd::Bind { from, to });
}

/// Describe-to-create (M3.2): resolve a free-text query + instantiate the top local match. Blocks
/// briefly on the engine thread's reply.
#[tauri::command(async)]
fn describe(state: State<AppState>, query: String) -> DescribeResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Describe { query, reply }).is_err() {
        return DescribeResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Begin a right-drag orbit. The render loop then polls the cursor and orbits natively — **zero IPC
/// per frame** (invariant 4); only this call and `drag_end` cross the boundary, once per gesture.
#[tauri::command]
fn drag_start(state: State<AppState>) {
    ipc();
    let mut st = state.shared.lock().unwrap();
    st.dragging = true;
    st.drag_last = None;
}

/// End the orbit drag.
#[tauri::command]
fn drag_end(state: State<AppState>) {
    ipc();
    state.shared.lock().unwrap().dragging = false;
}

/// Wheel zoom — folded into the camera distance natively next frame (one call per wheel tick, not
/// per frame).
#[tauri::command]
fn zoom(state: State<AppState>, delta: f32) {
    ipc();
    state.shared.lock().unwrap().zoom_delta += delta;
}

/// M14.2 (ADR-058) — render a live thumbnail of one entity (its REAL viewport render, off the per-frame
/// path). Pushes a request onto the shared `SceneState`; the render thread services it on its own encoder +
/// readback (invariant 4), then this polls the result and returns a `data:image/png;base64,…` URL — or
/// `None` (the entity has no renderable instance, or the render timed out → the UI keeps the icon fallback).
/// Discrete (counted like any IPC); the JS side is dirty-only + budget-limited, so it NEVER fires per frame
/// (an orbit dirties nothing → 0 thumbnail IPC during orbit, so invariant 4 holds with thumbnails active).
#[tauri::command]
fn thumbnail(state: State<AppState>, id: String, size: u32) -> Option<String> {
    ipc();
    {
        let mut st = state.shared.lock().unwrap();
        st.thumb_requests.push((id.clone(), size));
    }
    // Poll for the serviced result (~600 ms cap — the request may wait behind a queued burst before the
    // render thread services it a few-per-frame; a thumbnail is off the hot path, so a brief wait is fine and
    // the UI shows the icon fallback meanwhile). No lock is held during the sleep — zero render-thread contention.
    for _ in 0..120 {
        {
            let mut st = state.shared.lock().unwrap();
            if let Some(pos) = st.thumb_results.iter().position(|(rid, _)| rid == &id) {
                let (_, bytes) = st.thumb_results.remove(pos);
                drop(st);
                return bytes.map(|b| format!("data:image/png;base64,{}", base64_encode(&b)));
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    None
}

/// Standard base64 (RFC 4648) — a tiny self-contained encoder for the thumbnail `data:` URL (no new dep).
fn base64_encode(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = u32::from(*chunk.get(1).unwrap_or(&0));
        let b2 = u32::from(*chunk.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// Pick in the viewport (Rust — invariant 4). `x`/`y` are a normalized [0,1] window fraction
/// (DPI/offset-free), not pixels. Computed **synchronously** here — a pure projection over the current
/// instances + camera — so it never races the render loop's frame cadence (the bug a hidden/throttled
/// window exposed). Returns the picked entity's id, or `None` (only when the scene is empty).
#[tauri::command]
fn viewport_pick(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    x: f32,
    y: f32,
) -> Option<String> {
    ipc();
    let aspect = window.inner_size().map_or(16.0 / 9.0, |s| {
        s.width.max(1) as f32 / s.height.max(1) as f32
    });
    let mut st = state.shared.lock().unwrap();
    // Mirror the render loop's camera init so the pick uses the same view as what's drawn.
    if st.distance == 0.0 {
        st.distance = 60.0;
        st.elevation = 0.4;
    }
    let cam = render::camera_matrix(
        st.orbit,
        st.elevation,
        st.distance,
        aspect,
        st.cam_target.into(),
    );
    let hit = render::pick_nearest(&st.instances, (x, y), &cam);
    // update the highlight
    if let Some(p) = st.selected {
        if p < st.instances.len() {
            st.instances[p].selected = 0.0;
        }
    }
    st.selected = hit;
    if let Some(i) = hit {
        if i < st.instances.len() {
            st.instances[i].selected = 1.0;
        }
    }
    st.revision = st.revision.wrapping_add(1);
    hit.and_then(|i| st.ids.get(i).cloned())
}

/// Non-mutating pick (M3.3 hover) — identifies the entity under the cursor **without** changing the
/// selection or bumping the revision, so a hover sweep never disturbs the scene. The JS calls this
/// debounced (on hover-settle) and only re-fetches details when the returned id changes — so the
/// boundary is crossed on hovered-entity change, never per frame (invariant 4). Returns the id or
/// `None` (cursor over empty space).
#[tauri::command]
fn viewport_peek(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    x: f32,
    y: f32,
) -> Option<String> {
    ipc();
    let aspect = window.inner_size().map_or(16.0 / 9.0, |s| {
        s.width.max(1) as f32 / s.height.max(1) as f32
    });
    let st = state.shared.lock().unwrap();
    let (dist, elev) = if st.distance == 0.0 {
        (60.0, 0.4) // mirror the render loop's lazy camera init without mutating shared state
    } else {
        (st.distance, st.elevation)
    };
    let cam = render::camera_matrix(st.orbit, elev, dist, aspect, st.cam_target.into());
    let hit = render::pick_nearest(&st.instances, (x, y), &cam);
    hit.and_then(|i| st.ids.get(i).cloned())
}

/// Frame the camera on an entity (M3.3 Focus) — a pure camera/render-state op (no scene mutation, not
/// undoable, invariant 4). Three effects, all reversible by [`unfocus`]:
///   1. **Center** — set the orbit target to the entity's position so the camera looks straight at it
///      (it sits in the middle of the viewport).
///   2. **Get nearby** — zoom in to frame the entity by its size (saving the prior `distance` once, so
///      the first unfocus restores the original framing even after focusing several entities in turn).
///   3. **Gray the rest** — select the entity and raise the focus flag; the shader then dims every
///      *other* instance toward the background (the `focus_active` uniform), so only this one stays lit.
#[tauri::command]
fn focus_entity(state: State<AppState>, id: String) {
    ipc();
    let mut st = state.shared.lock().unwrap();
    if let Some(i) = st.ids.iter().position(|k| *k == id) {
        st.focus_on(i); // center + zoom-to-frame + select + raise the dim flag (see SceneState::focus_on)
    }
}

/// Exit M3.3 Focus mode ("everything comes back to normal"): un-dim the other entities and restore the
/// orbit `distance` saved when focus was entered (see [`render::SceneState::clear_focus`]). Idempotent —
/// a no-op when nothing is focused, so a stray Escape never disturbs the scene.
#[tauri::command]
fn unfocus(state: State<AppState>) {
    ipc();
    state.shared.lock().unwrap().clear_focus();
}

/// Introspect the Focus/camera state for the E2E (the viewport renders to a wgpu surface *under* the
/// transparent WebView, so WebdriverIO can't read its pixels — this exposes the observable state the
/// focus workflow test asserts). Returns `(orbit distance, focus-active flag)`.
#[tauri::command]
fn focus_debug(state: State<AppState>) -> (f32, bool) {
    let st = state.shared.lock().unwrap();
    (st.distance, st.focused.is_some())
}

// ── M10.7 camera & framing ergonomics (ADR-037) — pure camera/render-state ops (invariant 4) ───────────

/// M10.7 — **frame the whole scene** (the React toolbar's "Frame all"). A pure camera op (not undoable).
#[tauri::command]
fn frame_all(state: State<AppState>) {
    ipc();
    state.shared.lock().unwrap().frame_all();
}

/// M10.7 — snap the camera to a canonical view (`top`/`front`/`side`/`persp`) — the orientation cube /
/// view-preset buttons. A pure camera op.
#[tauri::command]
fn view_preset(state: State<AppState>, preset: String) {
    ipc();
    state.shared.lock().unwrap().set_view_preset(&preset);
}

/// M10.7 — the camera state `[orbit, elevation, distance, tx, ty, tz]` for the orientation cube + the E2E
/// (the wgpu viewport's pixels aren't WebDriver-readable; this exposes the observable camera state).
#[tauri::command]
fn camera_debug(state: State<AppState>) -> [f32; 6] {
    state.shared.lock().unwrap().camera_state()
}

/// The action model for an entity (M3.3) — valid actions + every-"no"-explained. A read; blocks
/// briefly on the engine thread.
#[tauri::command(async)]
fn entity_actions(state: State<AppState>, id: String) -> Vec<ActionItem> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Actions { id, reply }).is_err() {
        return Vec::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Remove an entity + its edges (M3.3) — one undoable transaction (Ctrl-Z restores).
#[tauri::command]
fn remove_entity(state: State<AppState>, id: String) {
    ipc();
    let _ = state.tx.send(EngineCmd::Remove { id });
}

/// Duplicate an entity (M3.3) — one undoable transaction; returns the clone's id.
#[tauri::command(async)]
fn duplicate_entity(state: State<AppState>, id: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Duplicate { id, reply }).is_err() {
        return None;
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Spawn a physics body (M8.2) — one undoable ECS setup commit, mirrored into the deterministic sim and
/// rendered as the ball; returns the new entity's id. Starts the sim running so it falls under gravity.
#[tauri::command(async)]
fn spawn_body(state: State<AppState>, x: f32, y: f32, z: f32) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::SpawnBody {
            pos: [x, y, z],
            reply,
        })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Play/pause the deterministic physics sim (M8.2) — setup stays editable while paused.
#[tauri::command]
fn set_sim_running(state: State<AppState>, run: bool) {
    ipc();
    let _ = state.tx.send(EngineCmd::SetSimRunning(run));
}

/// Physics introspection (M8.2) — `[body_count, lowest_y, contacts]`. Lets the E2E confirm a dropped ball
/// fell (lowest_y < spawn) and landed (contacts > 0). A read; the diagnostic seam is non-mutating.
#[tauri::command(async)]
fn physics_debug(state: State<AppState>) -> (usize, f64, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PhysicsDebug { reply }).is_err() {
        return (0, 0.0, 0);
    }
    recv_reply(&rx).unwrap_or((0, 0.0, 0))
}

/// M8 — a single body's CURRENT sim position `[x,y,z]` (the render-side transform the sim integrates). The
/// sim is render-only (ADR-021), so a shove/impulse moves the body in the sim, NOT the authored `Transform`
/// — so a test confirms motion against THIS, not `read_transform`. `[0,0,0]` if `id` isn't a live sim body.
#[tauri::command(async)]
fn body_sim_position(state: State<AppState>, id: String) -> [f64; 3] {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::BodySimPosition { id, reply })
        .is_err()
    {
        return [0.0, 0.0, 0.0];
    }
    recv_reply(&rx).unwrap_or([0.0, 0.0, 0.0])
}

/// M8.3 — make a dead mesh entity a correct dynamic body (the ≤2-click intent). Returns whether it applied.
#[tauri::command(async)]
fn make_dynamic(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::MakeDynamic { id, reply }).is_err() {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M11.1 — make an imported mesh a STATIC collidable obstacle (a fixed body + a convex-hull collider) so
/// dynamic bodies rest ON it. One undoable commit; survives reload. Returns whether it applied.
#[tauri::command(async)]
fn make_static(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::MakeStatic { id, reply }).is_err() {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M8.3 — the collider-intelligence warnings for an entity (each explained + a one-click fix id). A read.
#[tauri::command(async)]
fn physics_check(state: State<AppState>, id: String) -> Vec<PhysicsWarning> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::PhysicsCheck { id, reply })
        .is_err()
    {
        return Vec::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M8.3 — apply a one-click physics fix (`add-collider`/`use-hull`/`fix-mass`/`fix-scale`). Returns whether
/// it applied (the check then re-passes).
#[tauri::command(async)]
fn physics_fix(state: State<AppState>, id: String, action: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::PhysicsFix { id, action, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M8.4 — scrub the sim timeline to `frame` (deterministic replay over the sim-replay channel; pauses
/// there). Returns the timeline state `[frame, max_frame, running, overlays_on, bodies]` for the slider.
#[tauri::command(async)]
fn sim_scrub(state: State<AppState>, frame: u64) -> (u64, u64, bool, bool, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::SimScrub { frame, reply }).is_err() {
        return (0, 0, false, false, 0);
    }
    recv_reply(&rx)
        .map(|t| (t.frame, t.max_frame, t.running, t.overlays_on, t.bodies))
        .unwrap_or((0, 0, false, false, 0))
}

/// M8.4 — the current sim timeline state `[frame, max_frame, running, overlays_on, bodies]` (a read).
#[tauri::command(async)]
fn sim_timeline(state: State<AppState>) -> (u64, u64, bool, bool, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::SimTimeline { reply }).is_err() {
        return (0, 0, false, false, 0);
    }
    recv_reply(&rx)
        .map(|t| (t.frame, t.max_frame, t.running, t.overlays_on, t.bodies))
        .unwrap_or((0, 0, false, false, 0))
}

/// M8.4 — toggle the contact/solver debugger overlay (off by default; zero per-frame cost when off).
#[tauri::command]
fn sim_overlay(state: State<AppState>, on: bool) {
    ipc();
    let _ = state.tx.send(EngineCmd::SimOverlay { on });
}

/// M8.4 — apply + record a one-shot "shove" impulse on a body (the sim-replay input channel). Returns
/// whether it applied.
#[tauri::command(async)]
fn sim_shove(state: State<AppState>, id: String, impulse: [f64; 3]) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::SimShove { id, impulse, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M8.4 — the live contacts at the current frame, each with its measured fields + a plain-language
/// `explain` (the click-to-explain read). Non-mutating.
#[tauri::command(async)]
fn physics_contacts(state: State<AppState>) -> Vec<ContactInfo> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PhysicsContacts { reply }).is_err() {
        return Vec::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M8.5 — import a URDF / USD-Physics scene (`format` = "urdf" | "usd") as registry components (one
/// undoable tx, units reconciled). Returns the summary (bodies/joints/units/notes) for the UI.
#[tauri::command(async)]
fn import_interchange(state: State<AppState>, format: String, source: String) -> ImportResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ImportInterchange {
            format,
            source,
            reply,
        })
        .is_err()
    {
        return ImportResult::default();
    }
    recv_reply_within(&rx, IMPORT_REPLY_TIMEOUT).unwrap_or_default()
}

// ── M9.1 transform gizmo ────────────────────────────────────────────────────────────────────────────
//
// The per-frame drag runs NATIVELY in the render loop (0 per-frame IPC, like the orbit) — these commands
// only START a drag, set the mode/toggles, and COMMIT on release (2-ish IPC per gesture). A test-cursor
// override lets an E2E drive the SAME render-loop drag deterministically by supplying a world target.

const GIZMO_FOV: f32 = 55.0;
const GIZMO_K: f32 = 0.14;

/// `(aspect, selected_index)` for the gizmo, or `None` if nothing is selected / out of range.
fn gizmo_ctx(window: &tauri::WebviewWindow, st: &render::SceneState) -> Option<(f32, usize)> {
    let aspect = window.inner_size().map_or(16.0 / 9.0, |s| {
        s.width.max(1) as f32 / s.height.max(1) as f32
    });
    let sel = st.selected?;
    (sel < st.instances.len()).then_some((aspect, sel))
}

/// M9.1 — switch the persistent gizmo mode (W/E/R → translate/rotate/scale).
#[tauri::command]
fn gizmo_mode(state: State<AppState>, mode: String) {
    ipc();
    let m = match mode.as_str() {
        "rotate" => GizmoMode::Rotate,
        "scale" => GizmoMode::Scale,
        _ => GizmoMode::Translate,
    };
    state.shared.lock().unwrap().gizmo.set_mode(m);
}

/// M9.1 — toggle world/local axes; returns the new space ("world"|"local").
#[tauri::command]
fn gizmo_space_toggle(state: State<AppState>) -> String {
    ipc();
    let mut st = state.shared.lock().unwrap();
    st.gizmo.toggle_space();
    match st.gizmo.space() {
        GizmoSpace::World => "world",
        GizmoSpace::Local => "local",
    }
    .into()
}

/// M9.1 — toggle pivot/center; returns the new pivot ("origin"|"center").
#[tauri::command]
fn gizmo_pivot_toggle(state: State<AppState>) -> String {
    ipc();
    let mut st = state.shared.lock().unwrap();
    st.gizmo.toggle_pivot();
    match st.gizmo.pivot() {
        GizmoPivot::Origin => "origin",
        GizmoPivot::Center => "center",
    }
    .into()
}

/// M9.1 — select an entity by its loro-key (so the gizmo shows on it). Returns whether it was found.
#[tauri::command]
fn gizmo_select(state: State<AppState>, id: String) -> bool {
    ipc();
    let mut st = state.shared.lock().unwrap();
    let Some(i) = st.ids.iter().position(|k| *k == id) else {
        return false;
    };
    if let Some(p) = st.selected {
        if p < st.instances.len() {
            st.instances[p].selected = 0.0;
        }
    }
    st.selected = Some(i);
    if i < st.instances.len() {
        st.instances[i].selected = 1.0;
    }
    st.revision = st.revision.wrapping_add(1);
    true
}

/// M9 (React port) — the currently gizmo-selected entity's loro-key, so a React inspector button can act on
/// the SAME selection the live engine holds (exactly as the scaffold's module-level `selected` JS var did:
/// the scaffold set both together in `select()`, but the React panel learns the selection from the engine —
/// robust whether selection was set by a viewport pick or, in the acceptance harness, a direct
/// `gizmo_select`). `None` when nothing is selected.
#[tauri::command]
fn gizmo_selected(state: State<AppState>) -> Option<String> {
    ipc();
    let st = state.shared.lock().unwrap();
    st.selected.and_then(|i| st.ids.get(i).cloned())
}

/// M9.1 (LIVE path) — pick a gizmo handle under the normalized `(x,y)` cursor + start a drag. The render
/// loop then drives the per-frame move from the OS cursor (0 IPC). Returns whether a handle was hit (so
/// JS knows to NOT fall through to select/orbit).
#[tauri::command]
fn gizmo_pick_drag(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    x: f32,
    y: f32,
    ctrl: bool,
) -> bool {
    ipc();
    let mut st = state.shared.lock().unwrap();
    let Some((aspect, sel)) = gizmo_ctx(&window, &st) else {
        return false;
    };
    let origin = st.instances[sel].center;
    let eye = render::camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
    let scale = metrocalk_gizmo::pixel_scale(eye, origin, GIZMO_FOV.to_radians(), GIZMO_K);
    let (ro, rd) = render::cursor_ray(
        (x, y),
        st.orbit,
        st.elevation,
        st.distance,
        aspect,
        st.cam_target,
    );
    let ray = Ray {
        origin: ro,
        dir: rd,
    };
    let basis = [0.0, 0.0, 0.0, 1.0];
    let Some(handle) = st.gizmo.pick(ray, origin, basis, scale) else {
        return false;
    };
    // Carry the entity's CURRENT rotation so a rotate accumulates (and a translate preserves it); the
    // scale multiplier starts at [1,1,1] with the absolute held in `gizmo_start_scale`.
    let current = GizmoTransform {
        translation: origin,
        rotation: st.instances[sel].rotation,
        scale: [1.0; 3],
    };
    let scale0 = st.instances[sel].scale;
    st.gizmo
        .drag_start(handle, ray, origin, basis, scale, current);
    st.gizmo_start_scale = scale0;
    st.gizmo_dragging = true;
    st.gizmo_sel = Some(sel);
    st.gizmo_snap = ctrl;
    st.gizmo_test_cursor = None; // the live OS cursor drives the drag
    true
}

/// M9.1 (TEST path) — start a drag on a named axis ("x"|"y"|"z") with a deterministic ray, freezing at the
/// entity origin (so the render loop holds steady — no jump, no IPC — until [`gizmo_set_target`] moves it).
#[tauri::command]
fn gizmo_grab(window: tauri::WebviewWindow, state: State<AppState>, axis: String) -> bool {
    ipc();
    let mut st = state.shared.lock().unwrap();
    let Some((aspect, sel)) = gizmo_ctx(&window, &st) else {
        return false;
    };
    let origin = st.instances[sel].center;
    let rot0 = st.instances[sel].rotation;
    let scale0 = st.instances[sel].scale;
    let eye = render::camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
    let scale = metrocalk_gizmo::pixel_scale(eye, origin, GIZMO_FOV.to_radians(), GIZMO_K);
    let dir = [origin[0] - eye[0], origin[1] - eye[1], origin[2] - eye[2]];
    let ray = Ray { origin: eye, dir };
    let basis = [0.0, 0.0, 0.0, 1.0];
    let handle = match axis.as_str() {
        "y" => Handle::AxisY,
        "z" => Handle::AxisZ,
        _ => Handle::AxisX,
    };
    // `current` carries the entity's CURRENT rotation so a rotate accumulates from it (and a translate
    // preserves it); scale starts at the multiplier base [1,1,1] and `gizmo_start_scale` holds the absolute.
    let current = GizmoTransform {
        translation: origin,
        rotation: rot0,
        scale: [1.0; 3],
    };
    st.gizmo
        .drag_start(handle, ray, origin, basis, scale, current);
    st.gizmo_dragging = true;
    st.gizmo_sel = Some(sel);
    st.gizmo_snap = false;
    st.gizmo_start_scale = scale0;
    st.gizmo_test_cursor = render::project_to_screen(
        origin,
        st.orbit,
        st.elevation,
        st.distance,
        aspect,
        st.cam_target,
    );
    true
}

/// M9.1 (TEST path) — set the drag's target to a WORLD point (projected to a cursor the render loop then
/// drags the selection toward). Deterministic driving of the same render-loop drag.
#[tauri::command]
fn gizmo_set_target(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    tx: f32,
    ty: f32,
    tz: f32,
) {
    ipc();
    let mut st = state.shared.lock().unwrap();
    let aspect = window.inner_size().map_or(16.0 / 9.0, |s| {
        s.width.max(1) as f32 / s.height.max(1) as f32
    });
    st.gizmo_test_cursor = render::project_to_screen(
        [tx, ty, tz],
        st.orbit,
        st.elevation,
        st.distance,
        aspect,
        st.cam_target,
    );
}

/// M9.1 — the normalized screen position of a handle's grab point (for the live JS pick + the E2E).
#[tauri::command]
fn gizmo_handle_screen(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    axis: String,
) -> Option<(f32, f32)> {
    ipc();
    let st = state.shared.lock().unwrap();
    let (aspect, sel) = gizmo_ctx(&window, &st)?;
    let origin = st.instances[sel].center;
    let eye = render::camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
    let scale = metrocalk_gizmo::pixel_scale(eye, origin, GIZMO_FOV.to_radians(), GIZMO_K);
    let a = match axis.as_str() {
        "y" => [0.0, 1.0, 0.0],
        "z" => [0.0, 0.0, 1.0],
        _ => [1.0, 0.0, 0.0],
    };
    let tip = [
        origin[0] + a[0] * scale * 0.6,
        origin[1] + a[1] * scale * 0.6,
        origin[2] + a[2] * scale * 0.6,
    ];
    render::project_to_screen(
        tip,
        st.orbit,
        st.elevation,
        st.distance,
        aspect,
        st.cam_target,
    )
}

/// M9.1 — end the gizmo drag and COMMIT the coalesced move as ONE undoable transaction.
#[tauri::command]
fn gizmo_drag_end(window: tauri::WebviewWindow, state: State<AppState>) {
    ipc();
    // Snapshot the OS/windowing queries BEFORE taking `shared` (perf audit F11 / RC-4): never hold the hot
    // render mutex across a Win32 call. `aspect` + the real-cursor fallback are pure functions of the window;
    // the test-cursor override (from the locked state) still wins inside the critical section below.
    let aspect = window.inner_size().map_or(16.0 / 9.0, |s| {
        s.width.max(1) as f32 / s.height.max(1) as f32
    });
    let window_cursor = match (window.cursor_position(), window.inner_size()) {
        (Ok(p), Ok(s)) => Some((
            p.x as f32 / s.width.max(1) as f32,
            p.y as f32 / s.height.max(1) as f32,
        )),
        _ => None,
    };
    let commit = {
        let mut st = state.shared.lock().unwrap();
        if !st.gizmo_dragging {
            None
        } else {
            // Run ONE final drag_update with the current cursor so the committed position is the EXACT
            // release point — not the (up to one frame stale) last render-loop result (the race the review
            // flagged). drag_update is a pure function of the cursor, so re-running it is idempotent.
            let cursor = st.gizmo_test_cursor.or(window_cursor);
            if let (Some(cur), Some(sel)) = (cursor, st.gizmo_sel) {
                if sel < st.instances.len() {
                    let (ro, rd) = render::cursor_ray(
                        cur,
                        st.orbit,
                        st.elevation,
                        st.distance,
                        aspect,
                        st.cam_target,
                    );
                    let snap = st.gizmo_snap;
                    let world_final = st.gizmo.drag_update(
                        Ray {
                            origin: ro,
                            dir: rd,
                        },
                        snap,
                    );
                    // M9.4: apply the SAME magnetic snap as the live render-loop drag, so the COMMITTED
                    // pose lands exactly on the ghost (not the un-snapped release point).
                    let mut t = world_final.translation;
                    if !st.snap_disabled {
                        if let Some(i) = render::nearest_snap(
                            &st.instances,
                            &st.snap_affinity,
                            sel,
                            t,
                            render::SNAP_RADIUS,
                        ) {
                            t = st.instances[i].center;
                        }
                    }
                    st.instances[sel].center = t;
                }
            }
            st.gizmo_dragging = false;
            st.gizmo.drag_end();
            st.gizmo_test_cursor = None;
            st.snap_ghost = None;
            st.gizmo_sel.and_then(|sel| {
                (sel < st.instances.len() && sel < st.ids.len()).then(|| {
                    let inst = st.instances[sel];
                    (st.ids[sel].clone(), inst.center, inst.rotation, inst.scale)
                })
            })
        }
    };
    if let Some((id, pos, rot, scale)) = commit {
        let _ = state.tx.send(EngineCmd::GizmoCommit {
            id,
            pos,
            rot,
            scale,
        });
    }
}

/// M9.1 — read an entity's committed Transform `[x,y,z,qx,qy,qz,qw,scale]` (the gizmo HUD + E2E confirm the
/// move/rotate/scale landed in core, not just the render projection).
#[tauri::command(async)]
fn read_transform(state: State<AppState>, id: String) -> [f64; 8] {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ReadTransform { id, reply })
        .is_err()
    {
        return [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0];
    }
    recv_reply(&rx).unwrap_or([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0])
}

/// M9.2 — reparent a part ("drag in hierarchy"); `parent` `None` → root. Fire-and-forget (undoable).
#[tauri::command]
fn reparent_part(state: State<AppState>, id: String, parent: Option<String>) {
    ipc();
    let _ = state.tx.send(EngineCmd::ReparentPart { id, parent });
}

// ── M10.6 scene-authoring verbs (ADR-036) ────────────────────────────────────────────────────────────

/// M10.6 — create an empty named entity at a position; reply its id (the UI selects it).
#[tauri::command(async)]
fn create_entity(state: State<AppState>, x: f32, y: f32, z: f32, name: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::CreateEntity {
            x,
            y,
            z,
            name,
            reply,
        })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).unwrap_or(None)
}

/// M11.3 (ADR-042) — author a Light entity (kind = directional|point|spot) at a position with a linear RGB
/// colour + intensity; one undoable commit, persists. Reply its id (the UI selects it). Lighting is a render
/// projection — only the light ENTITY enters Loro/undo, never the per-frame lit result.
#[tauri::command(async)]
#[allow(clippy::needless_pass_by_value)]
// Tauri commands bind args positionally from the JS invoke; a colour/pos struct would need a matching JS
// shape, so the flat parameter list is the idiomatic command signature here.
#[allow(clippy::too_many_arguments)]
fn add_light(
    state: State<AppState>,
    kind: String,
    x: f32,
    y: f32,
    z: f32,
    r: f32,
    g: f32,
    b: f32,
    intensity: f32,
) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::AddLight {
            kind,
            pos: [x, y, z],
            color: [r, g, b],
            intensity,
            reply,
        })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).unwrap_or(None)
}

/// M10.6 — rename an entity (`__meta__.name`), one undoable tx; reply applied.
#[tauri::command(async)]
fn rename_entity(state: State<AppState>, id: String, name: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::RenameEntity { id, name, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M10.6 — group a selection under a new parent node; reply the group id.
#[tauri::command(async)]
fn group_entities(state: State<AppState>, ids: Vec<String>, name: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::GroupEntities { ids, name, reply })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).unwrap_or(None)
}

/// M10.6 — ungroup (dissolve a group); reply applied.
#[tauri::command(async)]
fn ungroup_entity(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::UngroupEntity { id, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M10.6 — multi-edit: set one numeric field on N entities as ONE batched, atomic, undoable tx.
#[tauri::command(async)]
fn multi_edit(
    state: State<AppState>,
    ids: Vec<String>,
    component: String,
    field: String,
    value: f64,
) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::MultiEdit {
            ids,
            component,
            field,
            value,
            reply,
        })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M10.6 — delete = deactivate (non-destructive; frees dependents); reply applied.
#[tauri::command(async)]
fn delete_deactivate(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::DeleteDeactivate { id, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M10.6 — copy a sub-tree to the clipboard (a read → fills the thread clipboard).
#[tauri::command]
fn copy_subtree(state: State<AppState>, id: String) {
    ipc();
    let _ = state.tx.send(EngineCmd::CopySubtree { id });
}

/// M10.6 — cut = copy + delete(deactivate); reply applied.
#[tauri::command(async)]
fn cut_subtree(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::CutSubtree { id, reply }).is_err() {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M10.6 — paste the clipboard under fresh ids; reply the new root id.
#[tauri::command(async)]
fn paste_clipboard(state: State<AppState>) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PasteClipboard { reply }).is_err() {
        return None;
    }
    recv_reply(&rx).unwrap_or(None)
}

/// M11.1 (ADR-040) — import an asset file from a known path (the e2e drives this); reply the new entity id.
#[tauri::command(async)]
fn import_asset(state: State<AppState>, path: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ImportAsset { path, reply })
        .is_err()
    {
        return None;
    }
    recv_reply_within(&rx, IMPORT_REPLY_TIMEOUT).unwrap_or(None)
}

/// M11.1 — **File → Import**: open a native file dialog filtered to 3D/asset formats, then import the
/// chosen file (the human path; the native dialog is the local-GUI step). Reply the new entity id, or
/// `None` if cancelled / unsupported.
#[tauri::command]
fn import_asset_dialog(app: tauri::AppHandle, state: State<AppState>) -> Option<String> {
    ipc();
    let path = app
        .dialog()
        .file()
        .add_filter(
            "3D models, CAD & assets",
            &[
                "fbx", "glb", "gltf", "obj", "png", "jpg", "jpeg", // meshes/textures
                "3dxml", "stp", "step", // CAD (M15.7 / ADR-077): CATIA 3DXML · STEP AP242
            ],
        )
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string())?;
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ImportAsset { path, reply })
        .is_err()
    {
        return None;
    }
    recv_reply_within(&rx, IMPORT_REPLY_TIMEOUT).unwrap_or(None)
}

/// M9.2 — deactivate-not-delete a part (or reactivate); replies whether it applied.
#[tauri::command(async)]
fn set_part_active(state: State<AppState>, id: String, active: bool) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::SetPartActive { id, active, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M9.2 — save the selected part's whole character for reuse; replies the composition id.
#[tauri::command(async)]
fn save_character(state: State<AppState>, id: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::SaveCharacter { id, reply })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).ok().flatten()
}

/// M9.2 — drop a fresh instance of a saved character; replies the new instance root id.
#[tauri::command(async)]
fn instantiate_character(state: State<AppState>, comp: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::InstantiateCharacter {
            comp_id: comp,
            reply,
        })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).ok().flatten()
}

/// M11.3 — a non-mutating lighting read for the acceptance gate: (authored light entities, render light
/// count incl. the synthesized default key light, shadow-caster index or -1, caster kind 0/1/2 or -1).
#[tauri::command(async)]
fn lighting_debug(state: State<AppState>) -> (usize, usize, i64, i64) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::LightingDebug { reply }).is_err() {
        return (0, 0, -1, -1);
    }
    recv_reply(&rx).unwrap_or((0, 0, -1, -1))
}

/// M11.4 — author a scene Camera entity (one undoable commit; survives reload). `pos` world position,
/// `fov` degrees, `active` whether it's the look-through/Play camera. Replies its id.
#[tauri::command(async)]
fn add_camera(
    state: State<AppState>,
    x: f32,
    y: f32,
    z: f32,
    fov: f32,
    active: bool,
) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::AddCamera {
            pos: [x, y, z],
            fov,
            active,
            reply,
        })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).unwrap_or(None)
}

/// M11.4 — look through the active scene camera (`on`) or back to the editor fly-cam (`!on`). Render-only
/// (a projection, 0-IPC, never Loro). Replies whether an active camera was found (when `on`).
#[tauri::command(async)]
fn look_through_camera(state: State<AppState>, on: bool) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::LookThrough { on, reply }).is_err() {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M11.4 — non-mutating SCENE-camera read for the gate: (authored Camera entities, an active one present,
/// the active fov in degrees or -1). Distinct from `camera_debug`, which reports the editor fly-cam state.
#[tauri::command(async)]
fn scene_camera_debug(state: State<AppState>) -> (usize, bool, f32) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::CameraDebug { reply }).is_err() {
        return (0, false, -1.0);
    }
    recv_reply(&rx).unwrap_or((0, false, -1.0))
}

/// M9.2 — a part's resolved world position + active flag + override-key count (the E2E read).
#[tauri::command(async)]
fn part_debug(state: State<AppState>, id: String) -> (f64, f64, f64, bool, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PartDebug { id, reply }).is_err() {
        return (0.0, 0.0, 0.0, false, 0);
    }
    recv_reply(&rx).unwrap_or((0.0, 0.0, 0.0, false, 0))
}

/// M9.2 — the seeded demo character's `(root, [parts])` ids (click a part to edit it).
#[tauri::command(async)]
fn demo_character(state: State<AppState>) -> Option<(String, Vec<String>)> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::DemoCharacter { reply }).is_err() {
        return None;
    }
    recv_reply(&rx).ok().flatten()
}

/// M9.2 — the entity at a structural rel-path within an instance root (e.g. `"0"` = first child).
#[tauri::command(async)]
fn part_at_path(state: State<AppState>, root: String, path: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::PartAtPath { root, path, reply })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).ok().flatten()
}

/// M9.2 — a part's current parent entity id (the `node.move` edge), `None` for a root. The stable
/// structural read-back the acceptance gate keys a reparant + its Ctrl-Z restore off.
#[tauri::command(async)]
fn part_parent(state: State<AppState>, id: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PartParent { id, reply }).is_err() {
        return None;
    }
    recv_reply(&rx).ok().flatten()
}

/// M9.4 — the snap-graph for `id`: ranked candidate targets (the shared ADR-011 ranker) + each one's
/// explained "why this", within `radius`.
#[tauri::command(async)]
fn snap_query(state: State<AppState>, id: String, radius: f32) -> Vec<SnapHit> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::SnapQuery { id, radius, reply })
        .is_err()
    {
        return Vec::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M9.4 — declare + apply a spatial constraint (solve + commit, undoable), or get the explained block.
#[tauri::command(async)]
fn apply_constraint(
    state: State<AppState>,
    id: String,
    kind: String,
    target: Option<String>,
    value: f32,
) -> SolveResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ApplyConstraint {
            id,
            kind,
            target,
            value,
            reply,
        })
        .is_err()
    {
        return SolveResult::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M9.4 — a natural-language placement sentence → editable intents → a schema-validated patch (ADR-017).
#[tauri::command(async)]
fn placement_sentence(state: State<AppState>, id: String, text: String) -> SolveResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::PlacementSentence { id, text, reply })
        .is_err()
    {
        return SolveResult::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M9.4 — toggle magnetic snapping (the live render-loop drag pulls the dragged entity onto the nearest
/// meaningful target). Default ON; touches only the shared `SceneState` (no engine round-trip).
#[tauri::command]
fn set_snap(state: State<AppState>, on: bool) {
    ipc();
    state.shared.lock().unwrap().snap_disabled = !on;
}

/// M11.4 — set the post-processing exposure (a linear multiplier applied before the ACES tonemap in
/// `display_encode`). Render-only state (no engine round-trip, 0-IPC, never Loro — ADR-021), clamped to a
/// sane range so the scene can't go fully black or blown out. Returns the applied value.
#[tauri::command]
fn set_exposure(state: State<AppState>, exposure: f32) -> f32 {
    ipc();
    let e = exposure.clamp(0.05, 8.0);
    state.shared.lock().unwrap().exposure = e;
    e
}

/// M9.4 — the current snap **ghost** position during a drag (the nearest target the dragged entity will
/// snap to), or `None` (no candidate in range / not dragging). The HUD + E2E read it.
#[tauri::command]
fn snap_ghost(state: State<AppState>) -> Option<[f32; 3]> {
    ipc();
    state.shared.lock().unwrap().snap_ghost
}

/// M9.1 — the gizmo state for the HUD/E2E: `(mode, has_selection, dragging, space, pivot)`.
#[tauri::command]
fn gizmo_debug(state: State<AppState>) -> (String, bool, bool, String, String) {
    ipc();
    let st = state.shared.lock().unwrap();
    let mode = match st.gizmo.mode() {
        GizmoMode::Translate => "translate",
        GizmoMode::Rotate => "rotate",
        GizmoMode::Scale => "scale",
    };
    let space = match st.gizmo.space() {
        GizmoSpace::World => "world",
        GizmoSpace::Local => "local",
    };
    let pivot = match st.gizmo.pivot() {
        GizmoPivot::Origin => "origin",
        GizmoPivot::Center => "center",
    };
    (
        mode.into(),
        st.selected.is_some(),
        st.gizmo_dragging,
        space.into(),
        pivot.into(),
    )
}

/// Total UI→core IPC calls so far — lets the E2E prove a gizmo drag is **0 per-frame IPC** (the count
/// stays flat across many render frames during an active drag; only the gesture's start/end cross JS).
#[tauri::command]
fn ipc_count() -> u64 {
    ipc();
    render::IPC_CALLS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Hover-tooltip details for an entity (M3.3) — fetched on hovered-entity change, not per frame.
#[tauri::command(async)]
fn entity_details(state: State<AppState>, id: String) -> Option<EntityDetails> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Details { id, reply }).is_err() {
        return None;
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M11.5 (ADR-044) — the selected entity's asset provenance for the inspector identity surface (where it
/// came from, AI-generated?, a near-duplicate hint). Fetched on selection change, not per frame.
#[tauri::command(async)]
fn asset_provenance(state: State<AppState>, id: String) -> Option<ProvenanceInfo> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::AssetProvenance { id, reply })
        .is_err()
    {
        return None;
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.1 (ADR-045) — the registry-fed Rules vocabulary the builder is assembled from (events · actions ·
/// components+fields+types) — what makes every dropdown typo-proof. A pure read of the standard library.
#[tauri::command]
fn rule_registry() -> RuleRegistryInfo {
    ipc();
    let field_ty = |t: metrocalk_core::FieldType| -> String {
        match t {
            metrocalk_core::FieldType::Integer => "integer",
            metrocalk_core::FieldType::Number => "number",
            metrocalk_core::FieldType::Boolean => "boolean",
            metrocalk_core::FieldType::String => "string",
        }
        .to_string()
    };
    RuleRegistryInfo {
        events: metrocalk_core::stdlib::standard_events()
            .into_iter()
            .map(|e| RuleVocabItem {
                name: e.name,
                description: e.description,
            })
            .collect(),
        actions: metrocalk_core::stdlib::standard_actions()
            .into_iter()
            .map(|a| RuleVocabItem {
                name: a.name,
                description: a.description,
            })
            .collect(),
        components: metrocalk_core::stdlib::standard_components()
            .into_iter()
            .map(|c| RuleComponentVocab {
                name: c.name,
                fields: c
                    .fields
                    .into_iter()
                    .map(|f| RuleFieldVocab {
                        name: f.name,
                        ty: field_ty(f.ty),
                    })
                    .collect(),
            })
            .collect(),
    }
}

/// M12.1 (ADR-045) — all authored rules for the editor Rule list. Fetched on change, not per frame.
#[tauri::command(async)]
fn list_rules(state: State<AppState>) -> Vec<RuleSummary> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::ListRules { reply }).is_err() {
        return Vec::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.1 (ADR-045) — author (or replace, if `id` is given) a rule: registry-validate (Blocked + explained),
/// commit one undoable transaction, reply the new id + the offered mirror "cleanup" rule.
#[tauri::command(async)]
fn author_rule(
    state: State<AppState>,
    rule: metrocalk_core::RuleData,
    id: Option<String>,
) -> AuthorRuleResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::AuthorRule { rule, id, reply })
        .is_err()
    {
        return AuthorRuleResult::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.1 (ADR-045) — remove a rule (one undoable transaction). Returns success.
#[tauri::command(async)]
fn delete_rule(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::DeleteRule { id, reply }).is_err() {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M12.2 (ADR-046) — all authored state machines for the editor's state-graph view (states + transitions +
/// the live current state). Fetched on change, not per frame.
#[tauri::command(async)]
fn state_machines(state: State<AppState>) -> Vec<StateMachineInfo> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ListStateMachines { reply })
        .is_err()
    {
        return Vec::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.2 (ADR-046) — author (or replace, if `id` is given) a state machine: validate (Blocked + explained,
/// no-dangling, registry-fed transitions), commit one undoable transaction, reply the new id + the
/// unreachable-states warning.
#[tauri::command(async)]
fn author_state_machine(
    state: State<AppState>,
    sm: metrocalk_core::StateMachine,
    id: Option<String>,
) -> AuthorStateMachineResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::AuthorStateMachine { sm, id, reply })
        .is_err()
    {
        return AuthorStateMachineResult::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.2 (ADR-046) — remove a state machine (one undoable transaction). Returns success.
#[tauri::command(async)]
fn delete_state_machine(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::DeleteStateMachine { id, reply })
        .is_err()
    {
        return false;
    }
    recv_reply(&rx).unwrap_or(false)
}

/// M12.3 (ADR-047) — run a sandboxed WASM plugin (the honest-ceiling escape) with a JSON `input`; its effect
/// lands as one undoable transaction, or an explained Blocked/contained reason. Returns the outcome.
#[tauri::command(async)]
fn run_plugin(state: State<AppState>, name: String, input: String) -> RunPluginResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::RunPlugin { name, input, reply })
        .is_err()
    {
        return RunPluginResult::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.4 (ADR-048) — turn a natural-language `sentence` into a REVIEWABLE composition proposal (the in-app AI
/// compose seam). The composer proposes; the engine validates it against the live scene so the preview is
/// pre-checked. `target` is the selected entity the rule acts on. Nothing is applied — call `compose` to apply.
#[tauri::command(async)]
fn propose_composition(
    state: State<AppState>,
    sentence: String,
    target: Option<String>,
) -> ComposeProposal {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::ProposeComposition {
            sentence,
            target,
            reply,
        })
        .is_err()
    {
        return ComposeProposal::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// M12.4 (ADR-048) — apply a reviewed `composition` (the validated op-set) through the one commit pipeline as
/// a single undoable transaction, or reject it whole with a plain-language reason (nothing applied). The same
/// validated path a human / plugin uses — the AI is never a raw mutation. Returns the applied/counts or error.
#[tauri::command(async)]
fn compose(
    state: State<AppState>,
    composition: metrocalk_core::compose::Composition,
) -> ComposeResult {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::Compose { composition, reply })
        .is_err()
    {
        return ComposeResult::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Generate (M6, tier 3) — opt-in last resort. Drops a grey placeholder instantly + kicks off async
/// text-to-3D; the real mesh streams in later over the projection Channel. Returns the placeholder + the
/// inert token cost, or the offline seam.
#[tauri::command(async)]
fn generate(state: State<AppState>, query: String) -> GenerateResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Generate { query, reply }).is_err() {
        return GenerateResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// AI-edit (M7 + M11.2) — assign a named PBR `material` preset (rusty/metal/chrome/gold/…) to an entity: a
/// schema-validated patch metered at the edit rate (debit-on-success). Blocks briefly on the engine reply.
#[tauri::command(async)]
fn ai_edit(state: State<AppState>, id: String, material: Option<String>) -> EconResponse {
    ipc();
    let material = material.unwrap_or_else(|| "rusty".to_string()); // back-compat: the original rustier edit
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::AiEdit {
            id,
            material,
            reply,
        })
        .is_err()
    {
        return EconResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Sandbox token top-up (M7) — $10 ≈ 100 tokens via the payment seam. **No real money moves** (the real
/// provider is a go-live seam).
#[tauri::command(async)]
fn top_up(state: State<AppState>) -> EconResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::TopUp { reply }).is_err() {
        return EconResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// The user's token balance (M7) — a read for the wallet UI.
#[tauri::command(async)]
fn wallet_info(state: State<AppState>) -> EconResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::WalletInfo { reply }).is_err() {
        return EconResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// The browsable "+ Add" catalog (M3.4), grouped by category bucket.
#[tauri::command(async)]
fn catalog(state: State<AppState>) -> BTreeMap<String, Vec<CatalogItem>> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Catalog { reply }).is_err() {
        return BTreeMap::new();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Search the "+ Add" catalog (M3.4) — reuses the tiered resolver (local → marketplace → generate seam).
#[tauri::command(async)]
fn catalog_search(state: State<AppState>, query: String) -> CatalogSearch {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::CatalogSearch { query, reply })
        .is_err()
    {
        return CatalogSearch {
            items: Vec::new(),
            seam: None,
        };
    }
    recv_reply(&rx).unwrap_or(CatalogSearch {
        items: Vec::new(),
        seam: None,
    })
}

/// Add a chosen catalog item (M3.4) — `source` is `"local"` (free instantiate) or `"marketplace"` (buy).
#[tauri::command(async)]
fn add_item(state: State<AppState>, id: String, source: String) -> AddResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Add { id, source, reply }).is_err() {
        return AddResponse::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

// ── M10.3 (ADR-033): project lifecycle — New / Open / Save / Save As over the `.mtk` document ──────

/// Query the engine thread for the current project state (a read) — also the no-op reply when a file
/// dialog is cancelled.
fn query_project_state(state: &State<AppState>) -> ProjectInfoResp {
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::ProjectState { reply }).is_err() {
        return ProjectInfoResp::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Show a native Open dialog filtered to `.mtk`; the chosen path string, or `None` if cancelled.
fn pick_open_path(app: &tauri::AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("Metrocalk project", &["mtk"])
        .blocking_pick_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
}

/// Show a native Save dialog defaulting to a `.mtk`; the chosen path string, or `None` if cancelled.
fn pick_save_path(app: &tauri::AppHandle) -> Option<String> {
    app.dialog()
        .file()
        .add_filter("Metrocalk project", &["mtk"])
        .set_file_name("untitled.mtk")
        .blocking_save_file()
        .and_then(|f| f.into_path().ok())
        .map(|p| p.display().to_string())
}

/// Enqueue a save/open/new command (built from the reply sender) and await the reply.
fn run_project_cmd(
    state: &State<AppState>,
    cmd: impl FnOnce(Sender<ProjectInfoResp>) -> EngineCmd,
) -> ProjectInfoResp {
    let (reply, rx) = mpsc::channel();
    if state.tx.send(cmd(reply)).is_err() {
        return ProjectInfoResp::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// The current project state (path · unsaved-changes · recents) for the File menu (ADR-033).
#[tauri::command(async)]
fn project_state(state: State<AppState>) -> ProjectInfoResp {
    ipc();
    query_project_state(&state)
}

/// Save the project as a `.mtk` (atomic). Explicit `path` wins; else save to the current path; else
/// (untitled) a native Save dialog. A cancelled dialog is a no-op.
#[tauri::command]
fn save_project(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: Option<String>,
) -> ProjectInfoResp {
    ipc();
    let target = match path {
        Some(p) => Some(p),
        None => query_project_state(&state)
            .path
            .or_else(|| pick_save_path(&app)),
    };
    let Some(target) = target else {
        return query_project_state(&state); // cancelled / no target — no-op
    };
    run_project_cmd(&state, |reply| EngineCmd::SaveProject {
        path: Some(target),
        reply,
    })
}

/// Save As — always a native Save dialog to a new path. A cancelled dialog is a no-op.
#[tauri::command]
fn save_project_as(app: tauri::AppHandle, state: State<AppState>) -> ProjectInfoResp {
    ipc();
    let Some(target) = pick_save_path(&app) else {
        return query_project_state(&state); // cancelled — no-op
    };
    run_project_cmd(&state, |reply| EngineCmd::SaveProject {
        path: Some(target),
        reply,
    })
}

/// Open a `.mtk` project (swap in a fresh engine/scene, re-derive caps — ADR-032). Explicit `path` (a
/// recent) wins; else a native Open dialog. A corrupt/newer/missing file replies an explained error and
/// leaves the current project intact; a cancelled dialog is a no-op. The live engine-swap is accepted on
/// a GUI run.
#[tauri::command]
fn open_project(
    app: tauri::AppHandle,
    state: State<AppState>,
    path: Option<String>,
) -> ProjectInfoResp {
    ipc();
    let Some(target) = path.or_else(|| pick_open_path(&app)) else {
        return query_project_state(&state); // cancelled — no-op
    };
    run_project_cmd(&state, |reply| EngineCmd::OpenProject {
        path: Some(target),
        reply,
    })
}

/// New empty project (a fresh engine/scene, the session log reset). Accepted on a GUI run.
#[tauri::command(async)]
fn new_project(state: State<AppState>) -> ProjectInfoResp {
    ipc();
    run_project_cmd(&state, |reply| EngineCmd::NewProject { reply })
}

// ── M10.4 (ADR-034): Play mode — run the scene non-destructively ───────────────────────────────────

/// Enter Play — run the deterministic sim on the current scene (snapshots the edit state for Stop).
#[tauri::command(async)]
fn play(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Play { reply }).is_err() {
        return PlayInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Stop — restore the exact pre-Play edit state (non-destructive) and exit play mode.
#[tauri::command(async)]
fn stop(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Stop { reply }).is_err() {
        return PlayInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Pause / resume the running sim (stays in play mode).
#[tauri::command(async)]
fn pause(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Pause { reply }).is_err() {
        return PlayInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// The current Play-mode state for the runtime controls (a read).
#[tauri::command(async)]
fn play_state(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PlayStateQuery { reply }).is_err() {
        return PlayInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

// ── M12.5 (ADR-049): Rules in Play + the live truth-state debugger ─────────────────────────────────

/// Fire a live gameplay `event` (e.g. `EnemyDied`) into the running Rules — the When-channel. A projection
/// (never the doc); recorded so a scrub replays it. Returns the fresh truth-state for `selected`.
#[tauri::command(async)]
fn fire_rule_event(
    state: State<AppState>,
    event: String,
    subject: Option<String>,
    selected: Option<String>,
) -> RuleDebugInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::FireRuleEvent {
            event,
            subject,
            selected,
            reply,
        })
        .is_err()
    {
        return RuleDebugInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// The live truth-state debugger read (test #5 box 3) — click an entity → its rule truth (✅/❌ per condition),
/// machine current state, `explain_rule` narration, the decision history, and any determinism-flagged rules.
#[tauri::command(async)]
fn rule_debug(state: State<AppState>, id: Option<String>) -> RuleDebugInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::RuleDebug { id, reply }).is_err() {
        return RuleDebugInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

/// Scrub the decision history to `frame` over the M8.4 replay channel (test #5 box 4) and return the
/// truth-state at that frame for `selected` — watch exactly when a counter incremented / a transition fired.
#[tauri::command(async)]
fn rule_scrub(state: State<AppState>, frame: u64, selected: Option<String>) -> RuleDebugInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state
        .tx
        .send(EngineCmd::RuleScrub {
            frame,
            selected,
            reply,
        })
        .is_err()
    {
        return RuleDebugInfo::default();
    }
    recv_reply(&rx).unwrap_or_default()
}

fn main() {
    let shared: Shared = Arc::new(Mutex::new(SceneState::default()));
    let (tx, rx) = mpsc::channel::<EngineCmd>();
    {
        let shared = shared.clone();
        let self_tx = tx.clone(); // so the engine thread can hand workers (generation) a way back
                                  // Wrap the engine loop so a panic is LOGGED, not silently swallowed (audit F6). A bare panic here
                                  // kills the thread invisibly: the viewport freezes, commands return defaults, and nothing says why.
        std::thread::spawn(move || {
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                engine_thread(rx, shared, self_tx);
            }))
            .is_err()
            {
                eprintln!("[shell] FATAL: the engine thread panicked — the editor is now unresponsive (the viewport will freeze). Please restart.");
            }
        });
    }
    let app_state = AppState {
        tx,
        shared: shared.clone(),
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init()) // native Open/Save file dialogs (M10.3 File menu)
        .manage(app_state)
        .setup(move |app| {
            let win = app.get_webview_window("main").expect("main window");
            // Reopen where the editor was left, then persist the geometry on a light write-on-change
            // poll so a hard terminal kill (no close event) still preserves it — and so there's always
            // a baseline even if the window is never moved. ~1s granularity, a tiny sidecar write only
            // when the window actually moved/resized.
            restore_window_geom(&win);
            {
                let w = win.clone();
                std::thread::spawn(move || {
                    let mut last: Option<WinGeom> = None;
                    loop {
                        std::thread::sleep(std::time::Duration::from_millis(1000));
                        if let Some(g) = current_geom(&w) {
                            if last != Some(g) {
                                save_geom(&g);
                                last = Some(g);
                            }
                        }
                    }
                });
            }
            render::start(win, shared.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            submit_edit,
            undo,
            reveal_targets,
            bind_target,
            describe,
            drag_start,
            drag_end,
            zoom,
            thumbnail,
            viewport_pick,
            viewport_peek,
            focus_entity,
            unfocus,
            focus_debug,
            frame_all,
            view_preset,
            camera_debug,
            entity_actions,
            remove_entity,
            duplicate_entity,
            entity_details,
            asset_provenance,
            rule_registry,
            list_rules,
            author_rule,
            delete_rule,
            state_machines,
            author_state_machine,
            delete_state_machine,
            run_plugin,
            propose_composition,
            compose,
            generate,
            ai_edit,
            top_up,
            wallet_info,
            catalog,
            catalog_search,
            add_item,
            spawn_body,
            set_sim_running,
            physics_debug,
            body_sim_position,
            make_dynamic,
            make_static,
            physics_check,
            physics_fix,
            sim_scrub,
            sim_timeline,
            sim_overlay,
            sim_shove,
            physics_contacts,
            import_interchange,
            gizmo_mode,
            gizmo_space_toggle,
            gizmo_pivot_toggle,
            gizmo_select,
            gizmo_selected,
            gizmo_pick_drag,
            gizmo_grab,
            gizmo_set_target,
            gizmo_handle_screen,
            gizmo_drag_end,
            read_transform,
            reparent_part,
            create_entity,
            add_light,
            lighting_debug,
            add_camera,
            look_through_camera,
            scene_camera_debug,
            rename_entity,
            group_entities,
            ungroup_entity,
            multi_edit,
            delete_deactivate,
            copy_subtree,
            cut_subtree,
            paste_clipboard,
            import_asset,
            import_asset_dialog,
            set_part_active,
            save_character,
            instantiate_character,
            part_debug,
            demo_character,
            part_at_path,
            part_parent,
            snap_query,
            apply_constraint,
            placement_sentence,
            set_snap,
            set_exposure,
            snap_ghost,
            gizmo_debug,
            project_state,
            save_project,
            save_project_as,
            open_project,
            new_project,
            play,
            stop,
            pause,
            play_state,
            fire_rule_event,
            rule_debug,
            rule_scrub,
            ipc_count
        ])
        .run(tauri::generate_context!())
        .expect("run editor shell");
}

#[cfg(test)]
mod material_tests {
    use super::material_preset;

    #[test]
    fn known_presets_map_to_plausible_pbr_and_metal_is_metallic() {
        // A metal preset is fully metallic + fairly smooth; rust is semi-metallic + rough; all valid [0,1].
        let (_c, m_metal, r_metal) = material_preset("metal").expect("metal preset");
        assert!((m_metal - 1.0).abs() < 1e-6 && r_metal < 0.5);
        let (_c, m_rust, r_rust) = material_preset("rusty").expect("rusty preset");
        assert!(m_rust > 0.0 && m_rust < 1.0 && r_rust > 0.5);
        for name in ["rusty", "metal", "chrome", "gold", "copper", "plastic"] {
            let (color, metal, rough) = material_preset(name).expect(name);
            assert!(
                color.iter().all(|c| (0.0..=1.0).contains(c)),
                "{name} color in [0,1]"
            );
            assert!(
                (0.0..=1.0).contains(&metal) && (0.0..=1.0).contains(&rough),
                "{name} m/r in [0,1]"
            );
        }
        // chrome is near-mirror (very low roughness); gold is warm + metallic.
        assert!(material_preset("chrome").unwrap().2 < 0.1);
        assert!((material_preset("gold").unwrap().1 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn unknown_material_is_no_override() {
        assert!(material_preset("banana").is_none());
        assert!(material_preset("").is_none());
    }
}

#[cfg(test)]
mod base64_tests {
    use super::base64_encode;

    #[test]
    fn rfc4648_known_vectors_with_padding() {
        // The canonical RFC 4648 §10 vectors (incl. the 1- and 2-byte padding cases) — so a thumbnail
        // `data:image/png;base64,…` URL is valid bytes a browser `<img>` decodes.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        // a byte that exercises the high bits (`+`/`/` alphabet)
        assert_eq!(base64_encode(&[0xff, 0xff, 0xff]), "////");
        assert_eq!(base64_encode(&[0xfb]), "+w==");
    }
}

#[cfg(test)]
mod reload_tests {
    use super::{reimport_persisted_blob, AssetId, AssetStore, GltfImporter};

    /// A minimal valid ASCII FBX 7.4 unit cube (mirrors `assets/tests/fbx_bakeoff.rs`); `ufbx` parses it.
    fn ascii_cube_fbx() -> &'static [u8] {
        b"; FBX 7.4.0 project file\n\
FBXHeaderExtension:  {\n\
\tFBXHeaderVersion: 1003\n\
\tFBXVersion: 7400\n\
}\n\
Objects:  {\n\
\tGeometry: 100, \"Geometry::Cube\", \"Mesh\" {\n\
\t\tVertices: *24 {\n\
\t\t\ta: -0.5,-0.5,-0.5,0.5,-0.5,-0.5,0.5,0.5,-0.5,-0.5,0.5,-0.5,-0.5,-0.5,0.5,0.5,-0.5,0.5,0.5,0.5,0.5,-0.5,0.5,0.5\n\
\t\t}\n\
\t\tPolygonVertexIndex: *24 {\n\
\t\t\ta: 0,1,2,-4,4,5,6,-8,0,1,5,-5,1,2,6,-6,2,3,7,-7,3,0,4,-8\n\
\t\t}\n\
\t}\n\
}\n"
    }

    /// Regression for the M11.1 reload hole (audit P1): the boot re-import router dropped `Fbx`/`Ktx2`
    /// to the placeholder (`_ => continue`), so a reopened FBX import dangled — contradicting ADR-040's
    /// "a generated/imported mesh survives reload". The `.mtk` saves the handle `AssetId::of_bytes(src)`;
    /// reload must re-register that EXACT handle so `MeshRenderer.mesh` resolves. (KTX2 rides the same arm.)
    #[test]
    fn persisted_fbx_blob_reimports_under_its_content_handle_on_reload() {
        let fbx = ascii_cube_fbx();
        let handle = AssetId::of_bytes(fbx).as_str().to_string();
        let mut store = AssetStore::new();
        assert!(
            !store.contains(&handle),
            "precondition: the FBX handle is absent before the reload re-import"
        );
        let registered = reimport_persisted_blob(&mut store, &GltfImporter::new(), fbx);
        assert!(
            registered,
            "an FBX blob must re-import on reload, not fall through to the placeholder (the M11.1 hole)"
        );
        assert!(
            store.contains(&handle),
            "the re-imported FBX resolves under the SAME content-address handle the .mtk saved"
        );
    }
}

#[cfg(test)]
mod lighting_debug_tests {
    use super::collect_lights;
    use metrocalk_core::{Engine, FieldValue, Op};
    use metrocalk_ecs::FlecsWorld;
    use metrocalk_editor_shell::capscene::{self, CapScene};

    fn engine() -> (Engine<FlecsWorld>, CapScene) {
        let mut world = FlecsWorld::new();
        let scene = CapScene::intern(&mut world);
        let mut e = Engine::new(world, 1);
        capscene::seed(&mut e, &scene, 4).expect("seed");
        e.clear_history();
        (e, scene)
    }

    #[test]
    fn empty_scene_synthesizes_a_default_casting_key_light() {
        // The lighting_debug signal the gate reads: with no AUTHORED lights, the render still has the
        // synthesized default key light (a directional caster) so the scene is never unlit.
        let (e, _s) = engine();
        let (lights, caster) = collect_lights(&e);
        assert_eq!(lights.len(), 1, "one synthesized default key light");
        assert_eq!(caster, Some(0), "the default directional casts the shadow");
        assert_eq!(lights[0].pos_kind[3], 0.0, "kind 0 = directional");
    }

    #[test]
    fn an_authored_directional_light_casts_and_undo_restores_the_default() {
        let (mut e, scene) = engine();
        capscene::add_light(
            &mut e,
            &scene,
            "directional",
            [0.0, 8.0, 0.0],
            [1.0, 1.0, 1.0],
            3.0,
        )
        .expect("add a directional light");
        let (lights, caster) = collect_lights(&e);
        assert_eq!(
            lights.len(),
            1,
            "the authored light replaces the synthesized default"
        );
        assert_eq!(
            caster,
            Some(0),
            "the authored directional is the shadow caster"
        );
        // One undoable commit — Ctrl-Z removes the authored light; the render falls back to the default.
        e.undo();
        let (lights2, caster2) = collect_lights(&e);
        assert_eq!(
            lights2.len(),
            1,
            "the synthesized default returns after undo"
        );
        assert_eq!(caster2, Some(0));
    }

    #[test]
    fn toggling_cast_shadows_off_drops_the_light_as_the_shadow_caster() {
        // M11.3 authorability: `add_light` now writes a real `castShadows` field, so the inspector can
        // toggle it (a `SetField` — the exact op `submit_edit` emits). With the only light's shadows off,
        // nothing casts (collect_lights only promotes a directional whose castShadows isn't Bool(false)).
        let (mut e, scene) = engine();
        let id = capscene::add_light(
            &mut e,
            &scene,
            "directional",
            [0.0, 8.0, 0.0],
            [1.0, 1.0, 1.0],
            3.0,
        )
        .expect("add a directional light");
        assert_eq!(
            collect_lights(&e).1,
            Some(0),
            "a fresh directional casts by default"
        );
        // The inspector's mechanism: one undoable SetField on the now-EXISTING castShadows field.
        e.commit(
            "toggle-shadows",
            vec![Op::SetField {
                entity: id,
                component: "Light".into(),
                field: "castShadows".into(),
                value: FieldValue::Bool(false),
            }],
        )
        .expect("toggle castShadows off");
        assert_eq!(
            collect_lights(&e).1,
            None,
            "with the only light's shadows toggled off, nothing casts (the authorable toggle works)"
        );
    }
}
