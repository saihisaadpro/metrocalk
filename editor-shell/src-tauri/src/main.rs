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

mod render;

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use metrocalk_assets::{
    detect, AssetId, AssetStore, Detected, GltfImporter, ImageImporter, MeshGpu, MeshSource,
    ObjImporter,
};
use metrocalk_core::catalog::{CatalogItem, CatalogSearch};
use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_economy::{HoldId, SandboxProvider, GENERATE_TOKENS};
use metrocalk_ecs::{Entity, FlecsWorld, World};
use metrocalk_editor_shell::generate::{GenRequest, MeshGenerator};
use metrocalk_editor_shell::physics_intent::{self, MeshMetrics, PhysicsWarning};
use metrocalk_editor_shell::project as mtk_project;
use metrocalk_editor_shell::reveal::{reveal, why_not, Context};
use metrocalk_editor_shell::transform_solver::{
    Constraint, ConstraintIntent, SnapKind, SnapTarget,
};
use metrocalk_editor_shell::{
    actions_for, ai_edit_rustier, apply_ai_patch, apply_edit, buy_marketplace, capscene,
    project_entity, project_full, transform_solver, ActionItem, AiPatch, CapScene, EditIntent,
    EditTx, Log, MeshCatalog, Outcome, PatchOp, ProjectionDelta, ProjectionOp, Record, Wallet,
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

const SCENE_N: usize = 5000; // the real M1.4 stress scene (the M2 gate target)

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
        let stored = match detect(&bytes) {
            Some(Detected::Gltf) => store.import(&importer, &bytes),
            Some(Detected::Obj) => store.import(&ObjImporter::new(), &bytes),
            Some(Detected::Image) => store.import(&ImageImporter::new(), &bytes),
            _ => continue, // audio / unrecognized — not a mesh asset
        };
        if stored.is_ok() {
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

    let mut meshes = Vec::new();
    let mut handle_to_slot = HashMap::new();
    let mut scales = Vec::new();
    for (id, asset) in store.iter() {
        let slot = meshes.len();
        meshes.push(MeshGpu::from_asset(asset));
        let ext = asset.bounds().max_extent();
        scales.push(if ext > 0.0 { 0.9 / ext } else { 1.0 });
        handle_to_slot.insert(id.as_str().to_string(), slot);
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
    Undo,
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
    /// A live AI-edit (M7) — "make it rustier": a schema-validated patch metered at the edit rate
    /// (debit-on-success). Replies the economy outcome.
    AiEdit {
        id: String,
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
    // The seed count defaults to the M2 stress target (`SCENE_N`), but `MTK_SCENE_N` overrides it — a
    // clean low-/zero-entity scene for visually inspecting a single imported asset (e.g. an FBX) without
    // 5000 cubes burying it. The fingerprint folds the count in, so a non-default seed just gets its own
    // replay log namespace (it never corrupts the default project's log).
    let scene_n: usize = std::env::var("MTK_SCENE_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(SCENE_N);
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
    // A fixed-cadence heartbeat (~60/s) on its own thread enqueues `Tick` via the engine's own sender, so
    // the sim advances ON the engine thread (off the JS hot path, invariant 4) without blocking the
    // command loop. A `Tick` is a no-op until the sim is running with at least one body.
    {
        let ticker = self_tx.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(16));
            if ticker.send(EngineCmd::Tick).is_err() {
                break; // engine thread gone
            }
        });
    }

    while let Ok(cmd) = rx.recv() {
        match cmd {
            EngineCmd::Connect(ch) => {
                let _ = ch.send(project_full(&engine)); // initial full-scene load
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
                    let _ = ch.send(delta);
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
            EngineCmd::Undo => {
                if engine.undo() {
                    log.append(&Record::Undo); // persist the undo so replay reproduces the net state
                    if let Some(ch) = &channel {
                        let _ = ch.send(project_full(&engine)); // simplest correct post-undo sync
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
                            let _ = ch.send(ProjectionDelta {
                                ops: vec![metrocalk_editor_shell::ProjectionOp::AddEdge {
                                    from: from.clone(),
                                    rel: capscene::TRACKS.to_string(),
                                    to: to.clone(),
                                }],
                                confirms: vec![],
                                rejects: vec![],
                                full: false,
                            });
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
                        log.append(&Record::Remove { id: id.clone() });
                        if let Some(ch) = &channel {
                            let mut ops = vec![ProjectionOp::Remove { id: id.clone() }];
                            for (from, rel, to) in removed_edges {
                                ops.push(ProjectionOp::RemoveEdge { from, rel, to });
                            }
                            let _ = ch.send(ProjectionDelta {
                                ops,
                                confirms: vec![],
                                rejects: vec![],
                                full: false,
                            });
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
                        let _ = ch.send(project_full(&engine));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(new.map(|n| n.to_loro_key()));
            }
            EngineCmd::RenameEntity { id, name, reply } => {
                let ok = EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::rename(&mut engine, e, &name).is_ok());
                if ok {
                    if let Some(ch) = &channel {
                        let _ = ch.send(project_full(&engine));
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
                        let _ = ch.send(project_full(&engine));
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
                        let _ = ch.send(project_full(&engine));
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
                        let _ = ch.send(project_full(&engine));
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
                }
                let _ = reply.send(ok);
            }
            EngineCmd::DeleteDeactivate { id, reply } => {
                let ok = EntityId::from_loro_key(&id)
                    .is_some_and(|e| capscene::delete_deactivate(&mut engine, &scene, e).is_ok());
                if ok {
                    if let Some(ch) = &channel {
                        let _ = ch.send(project_full(&engine));
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
                        let _ = ch.send(project_full(&engine));
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
                        let _ = ch.send(project_full(&engine));
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
                    if let Ok(metrocalk_assets::ImportedAsset::Mesh(asset)) =
                        metrocalk_assets::import_any(&bytes)
                    {
                        let handle = AssetId::of_bytes(&bytes).as_str().to_string();
                        if !assets.handle_to_slot.contains_key(&handle) {
                            let gpu = MeshGpu::from_asset(&asset);
                            let ext = asset.bounds().max_extent();
                            let slot = assets.meshes.len();
                            assets.meshes.push(gpu.clone());
                            assets.scales.push(if ext > 0.0 { 0.9 / ext } else { 1.0 });
                            assets.handle_to_slot.insert(handle.clone(), slot);
                            let mut st = shared.lock().unwrap();
                            st.meshes.push(gpu);
                            st.meshes_revision = st.meshes_revision.wrapping_add(1);
                        }
                        let _ = metrocalk_editor_shell::blobstore::put(
                            &sidecar("metrocalk-assets"),
                            &bytes,
                        );
                        if let Ok(id) =
                            capscene::place_mesh(&mut engine, &scene, &handle, [0.0, 1.0, 0.0])
                        {
                            log.append(&Record::PlaceMesh {
                                asset: handle.clone(),
                                pos: [0.0, 1.0, 0.0],
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
                            // re-resolves after reload (the M6 residual: a generated mesh survives reopen).
                            let _ = metrocalk_editor_shell::blobstore::put(
                                &sidecar("metrocalk-assets"),
                                &bytes,
                            );
                            let gpu = MeshGpu::from_asset(&asset);
                            let ext = asset.bounds().max_extent();
                            let slot = assets.meshes.len();
                            assets.meshes.push(gpu.clone());
                            assets.scales.push(if ext > 0.0 { 0.9 / ext } else { 1.0 });
                            assets.handle_to_slot.insert(handle.clone(), slot);
                            let mut st = shared.lock().unwrap();
                            st.meshes.push(gpu);
                            st.meshes_revision = st.meshes_revision.wrapping_add(1);
                            usable = true;
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
                                let _ = ch.send(delta); // targeted stream-in delta (inv. 2)
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
            EngineCmd::AiEdit { id, reply } => {
                // The live "make it rustier" AI-edit (M7): a schema-validated patch metered at the edit
                // rate (debit-on-success; a rejected patch or insufficient balance never charges).
                let resp = if let Some(eid) = EntityId::from_loro_key(&id) {
                    let ref_id = format!("edit:{id}:{}", wallet.ledger().len());
                    let (delta, outcome) = ai_edit_rustier(&mut engine, &mut wallet, eid, &ref_id);
                    match outcome {
                        Outcome::Charged {
                            cost_tokens,
                            balance_tokens,
                        } => {
                            if let (Some(d), Some(ch)) = (delta, &channel) {
                                let _ = ch.send(d); // echo the material edit to the inspector
                            }
                            log.append(&Record::AiEdit { id: id.clone() });
                            rebuild(&engine, &shared, &mut positions, &assets);
                            EconResponse {
                                ok: true,
                                balance: balance_tokens,
                                cost: Some(cost_tokens),
                                message: Some("made it rustier".to_string()),
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
                    let _ = ch.send(project_full(&engine));
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
                            let _ = ch.send(project_full(&engine));
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
                    (recording, rec_entities, sim, body_of) =
                        restart_run(&engine, &assets, &sim, &body_of);
                    frame = 0;
                    max_frame = 0;
                    sim_running = true;
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
                        if e.merge(&snap).is_ok() {
                            engine = e;
                            scene = s;
                        }
                    }
                    play_mode = false;
                    sim_running = false;
                    recency.clear(); // ECS handles changed on the restore swap — drop stale ranking state
                    touch = 0;
                    rebuild(&engine, &shared, &mut positions, &assets);
                    if let Some(ch) = &channel {
                        let _ = ch.send(project_full(&engine));
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
                            let _ = ch.send(project_full(&engine));
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
        });
        seg.push(Instance {
            center: b,
            scale: 0.0,
            color,
            selected: 0.0,
            rotation: render::IDENTITY_QUAT,
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

    // Greyed: the nearest entities that have a reason they can't bind (bounded — the UI greys what it
    // shows; `why_not` is O(1) per target, so this stays cheap even at scene scale).
    let sel_pos = positions.get(&sel_ecs).copied().unwrap_or([0.0; 3]);
    let mut others: Vec<(EntityId, Entity, f32)> = engine
        .entity_ids()
        .into_iter()
        .filter(|&id| id != eid)
        .filter_map(|id| {
            let e = engine.ecs_entity(id)?;
            let p = positions.get(&e).copied().unwrap_or([0.0; 3]);
            Some((id, e, dist(sel_pos, p)))
        })
        .collect();
    others.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap_or(std::cmp::Ordering::Equal));
    let mut greyed = Vec::new();
    for (id, e, _) in others {
        if greyed.len() >= 60 {
            break;
        }
        if let Some(wn) = why_not(engine.world(), sel_ecs, scene.rels, e, &scene.cap_name) {
            greyed.push(Greyed {
                id: id.to_loro_key(),
                name: label(id),
                reason: wn.explain(),
            });
        }
    }

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
        let _ = ch.send(project_entity(engine, id));
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
                    FieldValue::Number(s) if *s > 0.0 => Some(*s as f32),
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
                _ => asset_scale,
            };
            (p, rot, scale)
        };
        if let Some(e) = engine.ecs_entity(id) {
            positions.insert(e, p);
        }
        let key = id.to_loro_key();
        let c = color_for(&key);
        instances.push(Instance {
            center: p,
            scale,
            color: c,
            selected: 0.0,
            rotation: rot,
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
        })
        .collect();
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

// ── tauri commands (UI → core) ─────────────────────────────────────────────────

/// Count one UI→core boundary crossing (render::IPC_CALLS) — the instrumentation behind the
/// zero-per-frame-IPC claim (invariant 4). Every command calls this exactly once.
fn ipc() {
    render::IPC_CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

#[tauri::command]
fn undo(state: State<AppState>) {
    ipc();
    let _ = state.tx.send(EngineCmd::Undo);
}

/// Reveal bindable targets for a selected entity (north-star test #1). Blocks briefly on the engine
/// thread's reply (a read).
#[tauri::command]
fn reveal_targets(state: State<AppState>, id: String) -> RevealResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Reveal { id, reply }).is_err() {
        return RevealResponse::default();
    }
    rx.recv().unwrap_or_default()
}

/// Bind the selection to a chosen compatible target (one undoable transaction).
#[tauri::command]
fn bind_target(state: State<AppState>, from: String, to: String) {
    ipc();
    let _ = state.tx.send(EngineCmd::Bind { from, to });
}

/// Describe-to-create (M3.2): resolve a free-text query + instantiate the top local match. Blocks
/// briefly on the engine thread's reply.
#[tauri::command]
fn describe(state: State<AppState>, query: String) -> DescribeResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Describe { query, reply }).is_err() {
        return DescribeResponse::default();
    }
    rx.recv().unwrap_or_default()
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
#[tauri::command]
fn entity_actions(state: State<AppState>, id: String) -> Vec<ActionItem> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Actions { id, reply }).is_err() {
        return Vec::new();
    }
    rx.recv().unwrap_or_default()
}

/// Remove an entity + its edges (M3.3) — one undoable transaction (Ctrl-Z restores).
#[tauri::command]
fn remove_entity(state: State<AppState>, id: String) {
    ipc();
    let _ = state.tx.send(EngineCmd::Remove { id });
}

/// Duplicate an entity (M3.3) — one undoable transaction; returns the clone's id.
#[tauri::command]
fn duplicate_entity(state: State<AppState>, id: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Duplicate { id, reply }).is_err() {
        return None;
    }
    rx.recv().unwrap_or_default()
}

/// Spawn a physics body (M8.2) — one undoable ECS setup commit, mirrored into the deterministic sim and
/// rendered as the ball; returns the new entity's id. Starts the sim running so it falls under gravity.
#[tauri::command]
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
    rx.recv().unwrap_or_default()
}

/// Play/pause the deterministic physics sim (M8.2) — setup stays editable while paused.
#[tauri::command]
fn set_sim_running(state: State<AppState>, run: bool) {
    ipc();
    let _ = state.tx.send(EngineCmd::SetSimRunning(run));
}

/// Physics introspection (M8.2) — `[body_count, lowest_y, contacts]`. Lets the E2E confirm a dropped ball
/// fell (lowest_y < spawn) and landed (contacts > 0). A read; the diagnostic seam is non-mutating.
#[tauri::command]
fn physics_debug(state: State<AppState>) -> (usize, f64, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PhysicsDebug { reply }).is_err() {
        return (0, 0.0, 0);
    }
    rx.recv().unwrap_or((0, 0.0, 0))
}

/// M8 — a single body's CURRENT sim position `[x,y,z]` (the render-side transform the sim integrates). The
/// sim is render-only (ADR-021), so a shove/impulse moves the body in the sim, NOT the authored `Transform`
/// — so a test confirms motion against THIS, not `read_transform`. `[0,0,0]` if `id` isn't a live sim body.
#[tauri::command]
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
    rx.recv().unwrap_or([0.0, 0.0, 0.0])
}

/// M8.3 — make a dead mesh entity a correct dynamic body (the ≤2-click intent). Returns whether it applied.
#[tauri::command]
fn make_dynamic(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::MakeDynamic { id, reply }).is_err() {
        return false;
    }
    rx.recv().unwrap_or(false)
}

/// M8.3 — the collider-intelligence warnings for an entity (each explained + a one-click fix id). A read.
#[tauri::command]
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
    rx.recv().unwrap_or_default()
}

/// M8.3 — apply a one-click physics fix (`add-collider`/`use-hull`/`fix-mass`/`fix-scale`). Returns whether
/// it applied (the check then re-passes).
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M8.4 — scrub the sim timeline to `frame` (deterministic replay over the sim-replay channel; pauses
/// there). Returns the timeline state `[frame, max_frame, running, overlays_on, bodies]` for the slider.
#[tauri::command]
fn sim_scrub(state: State<AppState>, frame: u64) -> (u64, u64, bool, bool, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::SimScrub { frame, reply }).is_err() {
        return (0, 0, false, false, 0);
    }
    rx.recv()
        .map(|t| (t.frame, t.max_frame, t.running, t.overlays_on, t.bodies))
        .unwrap_or((0, 0, false, false, 0))
}

/// M8.4 — the current sim timeline state `[frame, max_frame, running, overlays_on, bodies]` (a read).
#[tauri::command]
fn sim_timeline(state: State<AppState>) -> (u64, u64, bool, bool, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::SimTimeline { reply }).is_err() {
        return (0, 0, false, false, 0);
    }
    rx.recv()
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
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M8.4 — the live contacts at the current frame, each with its measured fields + a plain-language
/// `explain` (the click-to-explain read). Non-mutating.
#[tauri::command]
fn physics_contacts(state: State<AppState>) -> Vec<ContactInfo> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PhysicsContacts { reply }).is_err() {
        return Vec::new();
    }
    rx.recv().unwrap_or_default()
}

/// M8.5 — import a URDF / USD-Physics scene (`format` = "urdf" | "usd") as registry components (one
/// undoable tx, units reconciled). Returns the summary (bodies/joints/units/notes) for the UI.
#[tauri::command]
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
    rx.recv().unwrap_or_default()
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
    let commit = {
        let mut st = state.shared.lock().unwrap();
        if !st.gizmo_dragging {
            None
        } else {
            // Run ONE final drag_update with the current cursor so the committed position is the EXACT
            // release point — not the (up to one frame stale) last render-loop result (the race the review
            // flagged). drag_update is a pure function of the cursor, so re-running it is idempotent.
            let aspect = window.inner_size().map_or(16.0 / 9.0, |s| {
                s.width.max(1) as f32 / s.height.max(1) as f32
            });
            let cursor = st.gizmo_test_cursor.or_else(|| {
                let p = window.cursor_position().ok()?;
                let s = window.inner_size().ok()?;
                Some((
                    p.x as f32 / s.width.max(1) as f32,
                    p.y as f32 / s.height.max(1) as f32,
                ))
            });
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
#[tauri::command]
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
    rx.recv()
        .unwrap_or([0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0])
}

/// M9.2 — reparent a part ("drag in hierarchy"); `parent` `None` → root. Fire-and-forget (undoable).
#[tauri::command]
fn reparent_part(state: State<AppState>, id: String, parent: Option<String>) {
    ipc();
    let _ = state.tx.send(EngineCmd::ReparentPart { id, parent });
}

// ── M10.6 scene-authoring verbs (ADR-036) ────────────────────────────────────────────────────────────

/// M10.6 — create an empty named entity at a position; reply its id (the UI selects it).
#[tauri::command]
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
    rx.recv().unwrap_or(None)
}

/// M10.6 — rename an entity (`__meta__.name`), one undoable tx; reply applied.
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M10.6 — group a selection under a new parent node; reply the group id.
#[tauri::command]
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
    rx.recv().unwrap_or(None)
}

/// M10.6 — ungroup (dissolve a group); reply applied.
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M10.6 — multi-edit: set one numeric field on N entities as ONE batched, atomic, undoable tx.
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M10.6 — delete = deactivate (non-destructive; frees dependents); reply applied.
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M10.6 — copy a sub-tree to the clipboard (a read → fills the thread clipboard).
#[tauri::command]
fn copy_subtree(state: State<AppState>, id: String) {
    ipc();
    let _ = state.tx.send(EngineCmd::CopySubtree { id });
}

/// M10.6 — cut = copy + delete(deactivate); reply applied.
#[tauri::command]
fn cut_subtree(state: State<AppState>, id: String) -> bool {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::CutSubtree { id, reply }).is_err() {
        return false;
    }
    rx.recv().unwrap_or(false)
}

/// M10.6 — paste the clipboard under fresh ids; reply the new root id.
#[tauri::command]
fn paste_clipboard(state: State<AppState>) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PasteClipboard { reply }).is_err() {
        return None;
    }
    rx.recv().unwrap_or(None)
}

/// M11.1 (ADR-040) — import an asset file from a known path (the e2e drives this); reply the new entity id.
#[tauri::command]
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
    rx.recv().unwrap_or(None)
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
            "3D models & assets",
            &["fbx", "glb", "gltf", "obj", "png", "jpg", "jpeg"],
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
    rx.recv().unwrap_or(None)
}

/// M9.2 — deactivate-not-delete a part (or reactivate); replies whether it applied.
#[tauri::command]
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
    rx.recv().unwrap_or(false)
}

/// M9.2 — save the selected part's whole character for reuse; replies the composition id.
#[tauri::command]
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
    rx.recv().ok().flatten()
}

/// M9.2 — drop a fresh instance of a saved character; replies the new instance root id.
#[tauri::command]
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
    rx.recv().ok().flatten()
}

/// M9.2 — a part's resolved world position + active flag + override-key count (the E2E read).
#[tauri::command]
fn part_debug(state: State<AppState>, id: String) -> (f64, f64, f64, bool, usize) {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PartDebug { id, reply }).is_err() {
        return (0.0, 0.0, 0.0, false, 0);
    }
    rx.recv().unwrap_or((0.0, 0.0, 0.0, false, 0))
}

/// M9.2 — the seeded demo character's `(root, [parts])` ids (click a part to edit it).
#[tauri::command]
fn demo_character(state: State<AppState>) -> Option<(String, Vec<String>)> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::DemoCharacter { reply }).is_err() {
        return None;
    }
    rx.recv().ok().flatten()
}

/// M9.2 — the entity at a structural rel-path within an instance root (e.g. `"0"` = first child).
#[tauri::command]
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
    rx.recv().ok().flatten()
}

/// M9.2 — a part's current parent entity id (the `node.move` edge), `None` for a root. The stable
/// structural read-back the acceptance gate keys a reparant + its Ctrl-Z restore off.
#[tauri::command]
fn part_parent(state: State<AppState>, id: String) -> Option<String> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PartParent { id, reply }).is_err() {
        return None;
    }
    rx.recv().ok().flatten()
}

/// M9.4 — the snap-graph for `id`: ranked candidate targets (the shared ADR-011 ranker) + each one's
/// explained "why this", within `radius`.
#[tauri::command]
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
    rx.recv().unwrap_or_default()
}

/// M9.4 — declare + apply a spatial constraint (solve + commit, undoable), or get the explained block.
#[tauri::command]
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
    rx.recv().unwrap_or_default()
}

/// M9.4 — a natural-language placement sentence → editable intents → a schema-validated patch (ADR-017).
#[tauri::command]
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
    rx.recv().unwrap_or_default()
}

/// M9.4 — toggle magnetic snapping (the live render-loop drag pulls the dragged entity onto the nearest
/// meaningful target). Default ON; touches only the shared `SceneState` (no engine round-trip).
#[tauri::command]
fn set_snap(state: State<AppState>, on: bool) {
    ipc();
    state.shared.lock().unwrap().snap_disabled = !on;
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
#[tauri::command]
fn entity_details(state: State<AppState>, id: String) -> Option<EntityDetails> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Details { id, reply }).is_err() {
        return None;
    }
    rx.recv().unwrap_or_default()
}

/// Generate (M6, tier 3) — opt-in last resort. Drops a grey placeholder instantly + kicks off async
/// text-to-3D; the real mesh streams in later over the projection Channel. Returns the placeholder + the
/// inert token cost, or the offline seam.
#[tauri::command]
fn generate(state: State<AppState>, query: String) -> GenerateResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Generate { query, reply }).is_err() {
        return GenerateResponse::default();
    }
    rx.recv().unwrap_or_default()
}

/// AI-edit (M7) — "make it rustier" on an entity: a schema-validated patch metered at the edit rate
/// (debit-on-success). Blocks briefly on the engine thread's reply.
#[tauri::command]
fn ai_edit(state: State<AppState>, id: String) -> EconResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::AiEdit { id, reply }).is_err() {
        return EconResponse::default();
    }
    rx.recv().unwrap_or_default()
}

/// Sandbox token top-up (M7) — $10 ≈ 100 tokens via the payment seam. **No real money moves** (the real
/// provider is a go-live seam).
#[tauri::command]
fn top_up(state: State<AppState>) -> EconResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::TopUp { reply }).is_err() {
        return EconResponse::default();
    }
    rx.recv().unwrap_or_default()
}

/// The user's token balance (M7) — a read for the wallet UI.
#[tauri::command]
fn wallet_info(state: State<AppState>) -> EconResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::WalletInfo { reply }).is_err() {
        return EconResponse::default();
    }
    rx.recv().unwrap_or_default()
}

/// The browsable "+ Add" catalog (M3.4), grouped by category bucket.
#[tauri::command]
fn catalog(state: State<AppState>) -> BTreeMap<String, Vec<CatalogItem>> {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Catalog { reply }).is_err() {
        return BTreeMap::new();
    }
    rx.recv().unwrap_or_default()
}

/// Search the "+ Add" catalog (M3.4) — reuses the tiered resolver (local → marketplace → generate seam).
#[tauri::command]
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
    rx.recv().unwrap_or(CatalogSearch {
        items: Vec::new(),
        seam: None,
    })
}

/// Add a chosen catalog item (M3.4) — `source` is `"local"` (free instantiate) or `"marketplace"` (buy).
#[tauri::command]
fn add_item(state: State<AppState>, id: String, source: String) -> AddResponse {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Add { id, source, reply }).is_err() {
        return AddResponse::default();
    }
    rx.recv().unwrap_or_default()
}

// ── M10.3 (ADR-033): project lifecycle — New / Open / Save / Save As over the `.mtk` document ──────

/// Query the engine thread for the current project state (a read) — also the no-op reply when a file
/// dialog is cancelled.
fn query_project_state(state: &State<AppState>) -> ProjectInfoResp {
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::ProjectState { reply }).is_err() {
        return ProjectInfoResp::default();
    }
    rx.recv().unwrap_or_default()
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
    rx.recv().unwrap_or_default()
}

/// The current project state (path · unsaved-changes · recents) for the File menu (ADR-033).
#[tauri::command]
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
#[tauri::command]
fn new_project(state: State<AppState>) -> ProjectInfoResp {
    ipc();
    run_project_cmd(&state, |reply| EngineCmd::NewProject { reply })
}

// ── M10.4 (ADR-034): Play mode — run the scene non-destructively ───────────────────────────────────

/// Enter Play — run the deterministic sim on the current scene (snapshots the edit state for Stop).
#[tauri::command]
fn play(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Play { reply }).is_err() {
        return PlayInfo::default();
    }
    rx.recv().unwrap_or_default()
}

/// Stop — restore the exact pre-Play edit state (non-destructive) and exit play mode.
#[tauri::command]
fn stop(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Stop { reply }).is_err() {
        return PlayInfo::default();
    }
    rx.recv().unwrap_or_default()
}

/// Pause / resume the running sim (stays in play mode).
#[tauri::command]
fn pause(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Pause { reply }).is_err() {
        return PlayInfo::default();
    }
    rx.recv().unwrap_or_default()
}

/// The current Play-mode state for the runtime controls (a read).
#[tauri::command]
fn play_state(state: State<AppState>) -> PlayInfo {
    ipc();
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::PlayStateQuery { reply }).is_err() {
        return PlayInfo::default();
    }
    rx.recv().unwrap_or_default()
}

fn main() {
    let shared: Shared = Arc::new(Mutex::new(SceneState::default()));
    let (tx, rx) = mpsc::channel::<EngineCmd>();
    {
        let shared = shared.clone();
        let self_tx = tx.clone(); // so the engine thread can hand workers (generation) a way back
        std::thread::spawn(move || engine_thread(rx, shared, self_tx));
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
            ipc_count
        ])
        .run(tauri::generate_context!())
        .expect("run editor shell");
}
