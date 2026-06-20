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

use metrocalk_assets::{AssetStore, GltfImporter, MeshGpu};
use metrocalk_core::marketplace::{LocalCatalog, MarketplaceIndex};
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::{Entity, FlecsWorld};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context};
use metrocalk_editor_shell::{
    apply_edit, capscene, project_entity, project_full, CapScene, EditIntent, EditTx, Log,
    MeshCatalog, ProjectionDelta, Record,
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
    /// Token price of a marketplace entry — surfaced as an **inert** economy seam (no money moves;
    /// ADR-004). The UI shows "buy ≈ N tokens / creator keeps ~70%" framing; nothing settles.
    price: Option<u32>,
    /// The seam tier when nothing matched anywhere (`"generate"`) — a documented stub.
    seam: Option<String>,
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

fn engine_thread(rx: mpsc::Receiver<EngineCmd>, shared: Shared) {
    // Import the demo mesh assets once (one-shot heavy op) before seeding, so the catalog is ready for
    // describe-to-create + replay and the viewport's geometry is published.
    let assets = load_assets();
    // The marketplace index (M5) — a local checked-in catalog behind the trait; describe-to-create's
    // second tier (queried only on a no-local-match). A remote index slots in here unchanged.
    let market = LocalCatalog::builtin();
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
                    }
                } else if let Some(m) = market.query(&query).into_iter().next() {
                    // Marketplace tier: a pre-componentized entry, applied already wired (namespaced
                    // caps + its mesh handle). The price is surfaced as an inert economy seam (ADR-004).
                    let entry = m.entry;
                    let mesh = entry
                        .asset
                        .as_deref()
                        .and_then(|name| assets.asset_by_name.get(name).cloned());
                    match capscene::apply_marketplace_entry(
                        &mut engine,
                        &scene,
                        &entry,
                        pos,
                        mesh.as_deref(),
                    ) {
                        Ok(id) => {
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
                            }
                        }
                        Err(_) => DescribeResponse::default(),
                    }
                } else {
                    // No match anywhere — the generate seam (Phase-2 text-to-3D; unbuilt).
                    DescribeResponse {
                        seam: Some("generate".into()),
                        ..Default::default()
                    }
                };
                let _ = reply.send(resp);
            }
        }
    }
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
    let cam = render::camera_matrix(st.orbit, st.elevation, st.distance, aspect);
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

fn main() {
    let shared: Shared = Arc::new(Mutex::new(SceneState::default()));
    let (tx, rx) = mpsc::channel::<EngineCmd>();
    {
        let shared = shared.clone();
        std::thread::spawn(move || engine_thread(rx, shared));
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
            viewport_pick
        ])
        .run(tauri::generate_context!())
        .expect("run editor shell");
}
