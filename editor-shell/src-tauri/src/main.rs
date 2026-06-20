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

use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use metrocalk_assets::{AssetId, AssetStore, GltfImporter, MeshGpu, MeshSource};
use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_economy::{HoldId, SandboxProvider, GENERATE_TOKENS};
use metrocalk_ecs::{Entity, FlecsWorld, World};
use metrocalk_editor_shell::generate::{GenRequest, MeshGenerator};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context};
use metrocalk_editor_shell::{
    actions_for, ai_edit_rustier, apply_ai_patch, apply_edit, buy_marketplace, capscene,
    project_entity, project_full, ActionItem, AiPatch, CapScene, EditIntent, EditTx, Log,
    MeshCatalog, Outcome, PatchOp, ProjectionDelta, ProjectionOp, Record, Wallet,
};
use render::{Instance, SceneState, Shared};
use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{Manager, State};

const SCENE_N: usize = 5000; // the real M1.4 stress scene (the M2 gate target)

/// The checked-in demo assets — **embedded** so the packaged app has no runtime file dependency, while
/// the importer still runs on real glTF bytes (provenance: `assets/examples/gen_fixtures.rs`).
const HEALTHBAR_GLB: &[u8] = include_bytes!("../../assets/healthbar.glb");
const PROP_GLB: &[u8] = include_bytes!("../../assets/prop.glb");

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
    let import_ms = t0.elapsed().as_secs_f64() * 1000.0;

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
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, SCENE_N).expect("seed capability scene");
    // The seed is scene construction, not a user edit — drop it from the undo stack so Ctrl-Z can
    // never undo past the user's binds and delete the whole world (the bug a live Ctrl-Z surfaced).
    engine.clear_history();

    // Live persistence: re-seeding is deterministic (same SEED → identical ids), so replay the
    // append-only edit log on top to restore the user's prior binds/edits. clear_history again so the
    // restored scene is non-undoable too (same Ctrl-Z guard as the seed). The catalog re-derives any
    // described kind's mesh handle so a *visible* described object survives reload too.
    let log = Log::open(log_path(), capscene::fingerprint(SCENE_N));
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

    let mut positions: HashMap<Entity, [f32; 3]> = HashMap::new();
    rebuild(&engine, &shared, &mut positions, &assets);
    let mut channel: Option<Channel<ProjectionDelta>> = None;
    // Last-touched sequence per entity (higher = more recent) — the reveal's recency ranking signal,
    // bumped on every committed edit/bind so it's live, not inert.
    let mut recency: HashMap<Entity, u64> = HashMap::new();
    let mut touch: u64 = 0;
    // In-flight generation reservations (M7): placeholder loro-key → the token Hold, so the async
    // completion settles (success) or releases (failure) exactly the right hold. Lives on this thread
    // only, so reserve→settle/release is atomic (no race).
    let mut pending_gen: HashMap<String, HoldId> = HashMap::new();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            EngineCmd::Connect(ch) => {
                let _ = ch.send(project_full(&engine)); // initial full-scene load
                channel = Some(ch);
            }
            EngineCmd::Edit(tx) => {
                let delta = apply_edit(&mut engine, &tx);
                let ok = delta.rejects.is_empty();
                if let Some(ch) = &channel {
                    let _ = ch.send(delta);
                }
                if ok {
                    // bump recency for the edited entity (SetField.id / Bind.from)
                    let (EditIntent::SetField { id, .. } | EditIntent::Bind { from: id, .. }) =
                        &tx.intent;
                    if let Some(e) = EntityId::from_loro_key(id).and_then(|x| engine.ecs_entity(x))
                    {
                        touch += 1;
                        recency.insert(e, touch);
                    }
                    log.append(&Record::Edit(tx)); // persist the committed edit
                }
                rebuild(&engine, &shared, &mut positions, &assets);
            }
            EngineCmd::Undo => {
                if engine.undo() {
                    log.append(&Record::Undo); // persist the undo so replay reproduces the net state
                    if let Some(ch) = &channel {
                        let _ = ch.send(project_full(&engine)); // simplest correct post-undo sync
                    }
                    rebuild(&engine, &shared, &mut positions, &assets);
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
                                        balance: Some(wallet.balance_tokens()),
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
        }
    }
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

/// Rebuild the viewport instance list AND the cached `positions` map from the engine's `Transform`
/// components in one pass (scene truth → viewport + reveal input). The only place scene geometry flows
/// core → viewport.
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
    for id in engine.entity_ids() {
        let comps = engine.components_of(id);
        let t = comps.get("Transform");
        let get = |f: &str| -> f32 {
            t.and_then(|m| m.get(f)).map_or(0.0, |v| match v {
                FieldValue::Number(n) => *n as f32,
                FieldValue::Integer(i) => *i as f32,
                _ => 0.0,
            })
        };
        let p = [get("x"), get("y"), get("z")];
        if let Some(e) = engine.ecs_entity(id) {
            positions.insert(e, p);
        }
        let key = id.to_loro_key();
        // Resolve the entity's mesh handle (if any) to a render slot + normalized scale.
        let slot = comps
            .get("MeshRenderer")
            .and_then(|m| m.get(capscene::MESH_FIELD))
            .and_then(|v| match v {
                FieldValue::Str(h) => assets.handle_to_slot.get(h).copied(),
                _ => None,
            });
        let scale = slot.map_or(0.45, |s| assets.scales.get(s).copied().unwrap_or(0.45));
        let c = color_for(&key);
        instances.push(Instance {
            center: p,
            scale,
            color: c,
            selected: 0.0,
        });
        mesh_slots.push(slot.map_or(-1, |s| i32::try_from(s).unwrap_or(-1)));
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
        })
        .collect();
    let mut st = shared.lock().unwrap();
    let prev_sel = st.selected;
    st.instances = instances;
    st.ids = ids;
    st.mesh_slots = mesh_slots;
    st.line_points = line_points;
    if let Some(i) = prev_sel {
        if i < st.instances.len() {
            st.instances[i].selected = 1.0;
        } else {
            st.selected = None;
        }
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

/// Frame the camera on an entity (M3.3 Focus) — a pure camera op (no scene mutation, not undoable,
/// invariant 4): set the orbit target to the entity's position so orbit/zoom now revolve around it.
#[tauri::command]
fn focus_entity(state: State<AppState>, id: String) {
    ipc();
    let mut st = state.shared.lock().unwrap();
    if let Some(i) = st.ids.iter().position(|k| *k == id) {
        st.cam_target = st.instances[i].center;
        st.revision = st.revision.wrapping_add(1);
    }
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
            entity_actions,
            remove_entity,
            duplicate_entity,
            entity_details,
            generate,
            ai_edit,
            top_up,
            wallet_info
        ])
        .run(tauri::generate_context!())
        .expect("run editor shell");
}
