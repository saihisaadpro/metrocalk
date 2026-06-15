#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! M2.6 desktop editor shell — the convergence. A transparent WebView2 (the editor UI) over a native
//! wgpu viewport (M2.2 instanced scene) on one HWND (ADR-008, single-window, OS-composited). The
//! **real** `/core` Engine drives both: it lives on a dedicated thread (Flecs is `!Send`, so it can't
//! sit in Tauri's `Send+Sync` managed state — M2.1's finding), fed editor `EditTx`s over `invoke` and
//! pushing `ProjectionDelta`s back over a Tauri `Channel` (the desktop binding of the M2.4 transport
//! contract). Camera + picking stay in Rust (invariant 4); only the committed delta crosses (inv. 2).

mod render;

use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};

use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{apply_edit, project_full, EditTx, ProjectionDelta};
use render::{Instance, SceneState, Shared};
use tauri::ipc::Channel;
use tauri::{Manager, State};

const SCENE_N: usize = 2000;

/// Commands to the engine thread (which owns the `!Send` Engine).
enum EngineCmd {
    Connect(Channel<ProjectionDelta>),
    Edit(EditTx),
    Undo,
}

struct AppState {
    tx: Sender<EngineCmd>,
    shared: Shared,
}

// ── engine thread: owns the real Engine + the bridge ───────────────────────────

fn engine_thread(rx: mpsc::Receiver<EngineCmd>, shared: Shared) {
    let mut engine = Engine::new(FlecsWorld::new(), 1);
    seed_scene(&mut engine, SCENE_N);
    rebuild_viewport(&engine, &shared);
    let mut channel: Option<Channel<ProjectionDelta>> = None;

    while let Ok(cmd) = rx.recv() {
        match cmd {
            EngineCmd::Connect(ch) => {
                // initial scene load: project the whole authoritative scene to the editor
                let _ = ch.send(project_full(&engine));
                channel = Some(ch);
            }
            EngineCmd::Edit(tx) => {
                let delta = apply_edit(&mut engine, &tx);
                if let Some(ch) = &channel {
                    let _ = ch.send(delta);
                }
                rebuild_viewport(&engine, &shared); // reflect committed Transform changes in the viewport
            }
            EngineCmd::Undo => {
                if engine.undo() {
                    if let Some(ch) = &channel {
                        // a full re-projection is the simplest correct post-undo sync for the scaffold
                        let _ = ch.send(project_full(&engine));
                    }
                    rebuild_viewport(&engine, &shared);
                }
            }
        }
    }
}

/// Seed a deterministic stress scene: `n` entities with a `Transform` (spread in a volume) + a color.
fn seed_scene(engine: &mut Engine<FlecsWorld>, n: usize) {
    let mut s: u64 = 0x4D45_5452_4F43_4131; // "METROCA1" — same seed family as M1.4
    let mut rnd = || {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((s >> 33) as f32) / (1u64 << 31) as f32
    };
    let extent = 18.0 * (n as f32 / 5000.0).cbrt().max(0.3);
    let mut ops = Vec::with_capacity(n * 4);
    for _ in 0..n {
        let id = engine.alloc_entity_id();
        ops.push(Op::CreateEntity { id, parent: None });
        for (f, v) in [
            ("x", (rnd() * 2.0 - 1.0) * extent),
            ("y", (rnd() * 2.0 - 1.0) * extent),
            ("z", (rnd() * 2.0 - 1.0) * extent),
        ] {
            ops.push(Op::SetField { entity: id, component: "Transform".into(), field: f.into(), value: FieldValue::Number(f64::from(v)) });
        }
    }
    engine.commit("seed-scene", ops).expect("seed scene commits");
    eprintln!("[shell] seeded {} entities", engine.entity_count());
}

/// Rebuild the viewport instance list from the engine's `Transform` components (the render loop reads
/// the shared state; this is the only place scene truth flows core → viewport).
fn rebuild_viewport(engine: &Engine<FlecsWorld>, shared: &Shared) {
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
        let key = id.to_loro_key();
        let c = color_for(&key);
        instances.push(Instance { center: [get("x"), get("y"), get("z")], scale: 0.45, color: c, selected: 0.0 });
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

fn color_for(key: &str) -> [f32; 3] {
    let mut h: u32 = 2166136261;
    for b in key.bytes() {
        h = (h ^ u32::from(b)).wrapping_mul(16777619);
    }
    [0.4 + (h & 0xff) as f32 / 425.0, 0.4 + ((h >> 8) & 0xff) as f32 / 425.0, 0.4 + ((h >> 16) & 0xff) as f32 / 425.0]
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

/// Pixel ray-pick in the viewport (Rust — invariant 4). Returns the picked entity's id, or `None`.
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
            // update highlight
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
        if !st.pick_request {
            // serviced with no hit
            if st.picked.is_none() {
                // clear selection on empty pick
            }
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
    let app_state = AppState { tx, shared: shared.clone() };

    tauri::Builder::default()
        .manage(app_state)
        .setup(move |app| {
            let win = app.get_webview_window("main").expect("main window");
            render::start(win, shared.clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![connect, submit_edit, undo, viewport_pick])
        .run(tauri::generate_context!())
        .expect("run editor shell");
}
