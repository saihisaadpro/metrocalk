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

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::{Entity, FlecsWorld};
use metrocalk_editor_shell::reveal::{reveal, why_not, Context};
use metrocalk_editor_shell::{apply_edit, capscene, project_full, CapScene, EditTx, ProjectionDelta};
use render::{Instance, SceneState, Shared};
use serde::Serialize;
use tauri::ipc::Channel;
use tauri::{Manager, State};

const SCENE_N: usize = 5000; // the real M1.4 stress scene (the M2 gate target)

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
}

struct AppState {
    tx: Sender<EngineCmd>,
    shared: Shared,
}

// ── engine thread: owns the real Engine + the capability scene + the bridge ─────

fn engine_thread(rx: mpsc::Receiver<EngineCmd>, shared: Shared) {
    let mut world = FlecsWorld::new();
    // Intern the capability relationships BEFORE the engine takes the world (they are metadata, like
    // the registry's own interned rels — not scene entities).
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, SCENE_N).expect("seed capability scene");
    // The seed is scene construction, not a user edit — drop it from the undo stack so Ctrl-Z can
    // never undo past the user's binds and delete the whole world (the bug a live Ctrl-Z surfaced).
    engine.clear_history();
    eprintln!(
        "[shell] seeded {} entities — {} HealthBars, {} unbound Health providers",
        engine.entity_count(),
        index.health_bars.len(),
        index.unbound_health_providers
    );
    if let Some(first) = index.health_bars.first() {
        eprintln!("[shell] click HealthBar {} to reveal bindable targets", first.to_loro_key());
    }

    let mut positions: HashMap<Entity, [f32; 3]> = HashMap::new();
    rebuild(&engine, &shared, &mut positions);
    let mut channel: Option<Channel<ProjectionDelta>> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            EngineCmd::Connect(ch) => {
                let _ = ch.send(project_full(&engine)); // initial full-scene load
                channel = Some(ch);
            }
            EngineCmd::Edit(tx) => {
                let delta = apply_edit(&mut engine, &tx);
                if let Some(ch) = &channel {
                    let _ = ch.send(delta);
                }
                rebuild(&engine, &shared, &mut positions);
            }
            EngineCmd::Undo => {
                if engine.undo() {
                    if let Some(ch) = &channel {
                        let _ = ch.send(project_full(&engine)); // simplest correct post-undo sync
                    }
                    rebuild(&engine, &shared, &mut positions);
                }
            }
            EngineCmd::Reveal { id, reply } => {
                let resp = EntityId::from_loro_key(&id)
                    .map(|eid| compute_reveal(&engine, &scene, &positions, eid))
                    .unwrap_or_default();
                let _ = reply.send(resp);
            }
            EngineCmd::Bind { from, to } => {
                if let (Some(f), Some(t)) =
                    (EntityId::from_loro_key(&from), EntityId::from_loro_key(&to))
                {
                    if capscene::bind(&mut engine, &scene, f, t).is_ok() {
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
                        rebuild(&engine, &shared, &mut positions);
                    }
                }
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
    eid: EntityId,
) -> RevealResponse {
    let Some(sel_ecs) = engine.ecs_entity(eid) else {
        return RevealResponse::default();
    };
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: positions,
        recency: &recency,
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

/// Rebuild the viewport instance list AND the cached `positions` map from the engine's `Transform`
/// components in one pass (scene truth → viewport + reveal input). The only place scene geometry flows
/// core → viewport.
fn rebuild(engine: &Engine<FlecsWorld>, shared: &Shared, positions: &mut HashMap<Entity, [f32; 3]>) {
    positions.clear();
    let mut instances = Vec::new();
    let mut ids = Vec::new();
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
        let c = color_for(&key);
        instances.push(Instance {
            center: p,
            scale: 0.45,
            color: c,
            selected: 0.0,
        });
        ids.push(key);
    }
    let mut st = shared.lock().unwrap();
    let prev_sel = st.selected;
    st.instances = instances;
    st.ids = ids;
    if let Some(i) = prev_sel {
        if i < st.instances.len() {
            st.instances[i].selected = 1.0;
        } else {
            st.selected = None;
        }
    }
    st.revision = st.revision.wrapping_add(1);
}

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

#[tauri::command]
fn connect(state: State<AppState>, channel: Channel<ProjectionDelta>) {
    let _ = state.tx.send(EngineCmd::Connect(channel));
}

#[tauri::command]
fn submit_edit(state: State<AppState>, tx: EditTx) {
    let _ = state.tx.send(EngineCmd::Edit(tx));
}

#[tauri::command]
fn undo(state: State<AppState>) {
    let _ = state.tx.send(EngineCmd::Undo);
}

/// Reveal bindable targets for a selected entity (north-star test #1). Blocks briefly on the engine
/// thread's reply (a read).
#[tauri::command]
fn reveal_targets(state: State<AppState>, id: String) -> RevealResponse {
    let (reply, rx) = mpsc::channel();
    if state.tx.send(EngineCmd::Reveal { id, reply }).is_err() {
        return RevealResponse::default();
    }
    rx.recv().unwrap_or_default()
}

/// Bind the selection to a chosen compatible target (one undoable transaction).
#[tauri::command]
fn bind_target(state: State<AppState>, from: String, to: String) {
    let _ = state.tx.send(EngineCmd::Bind { from, to });
}

/// Ray-pick in the viewport (Rust — invariant 4). `x`/`y` are a normalized [0,1] window fraction
/// (DPI/offset-free), not pixels. Returns the picked entity's id, or `None`.
#[tauri::command]
fn viewport_pick(state: State<AppState>, x: f32, y: f32) -> Option<String> {
    {
        let mut st = state.shared.lock().unwrap();
        st.cursor = (x, y);
        st.pick_request = true;
        st.picked = None;
    }
    // wait briefly for the render loop to service the pick
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_millis(2));
        let mut st = state.shared.lock().unwrap();
        if let Some(i) = st.picked.take() {
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
            return st.ids.get(i).cloned();
        }
    }
    None
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
            render::start(win, shared.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            connect,
            submit_edit,
            undo,
            reveal_targets,
            bind_target,
            viewport_pick
        ])
        .run(tauri::generate_context!())
        .expect("run editor shell");
}
