//! Live-shape reload regression (Prompt 22). The diagnosis (measured live) was that the bug is
//! **surfacing**, not the data layer: the engine always restored the binds/edits/describe via
//! deterministic-seed + replay — the live shell prints `restored N edits (0 skipped)` — but the UI
//! didn't *show* the restored state on reload. The existing `persistence.rs` proves the replay logic at
//! n=500 against a temp file; it never exercised the shell's real path: `SCENE_N = 5000`, the exact
//! `Bind`/`Edit`/`Describe`/`Undo` record stream the engine thread writes, and the data→projection seam
//! the UI (panel + the viewport's tracking lines) actually reads.
//!
//! This closes that delta: at SCENE_N, drive the live record stream through a REAL on-disk `Log`,
//! re-seed from scratch + replay (a true close→reopen), and assert (a) the net state is correct and
//! (b) `project_full` — what the shell sends the WebView on connect, and what `rebuild` builds the
//! viewport tracking lines from — carries the surviving binding edge and NOT the undone one. Plus the
//! lost-write guard (header + records really hit disk) and that a matching-fingerprint log isn't
//! discarded.

use std::collections::HashMap;
use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::reveal::{reveal, Context};
use metrocalk_editor_shell::{apply_edit, project_full, EditIntent, EditTx, ProjectionOp, TRACKS};

const SCENE_N: usize = 5000; // the shell's real scene size — the untested delta the n=500 test missed

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

fn has_binding(engine: &Engine<FlecsWorld>, from: EntityId, to: EntityId) -> bool {
    engine
        .bindings()
        .iter()
        .any(|(f, k, t)| *f == from && k == TRACKS && *t == to)
}

/// A fresh seeded engine at SCENE_N + a HealthBar and its two nearest compatible Health providers.
fn make() -> (Engine<FlecsWorld>, CapScene, EntityId, EntityId, EntityId) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, SCENE_N).expect("seed");
    let bar = index.health_bars[0];
    let pos = capscene::positions(&engine);
    let recency = HashMap::new();
    let ctx = Context {
        cap_name: &scene.cap_name,
        position: &pos,
        recency: &recency,
    };
    let bar_ecs = engine.ecs_entity(bar).unwrap();
    let r = reveal(engine.world(), bar_ecs, scene.rels, &ctx);
    let p1 = engine.entity_id_of(r.compatible[0].entity).unwrap();
    let p2 = engine.entity_id_of(r.compatible[1].entity).unwrap();
    (engine, scene, bar, p1, p2)
}

/// Simulate a fresh process: deterministic re-seed (identical ids) + replay the log + clear history.
fn relaunch(log: &Log) -> (Engine<FlecsWorld>, usize, usize) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, SCENE_N).expect("re-seed");
    engine.clear_history(); // seed not undoable
    let (applied, skipped) = log.replay(&mut engine, &scene);
    engine.clear_history(); // restored scene not undoable
    (engine, applied, skipped)
}

#[test]
#[allow(clippy::too_many_lines)] // one cohesive close→reopen assertion, read top to bottom
fn live_shape_reload_restores_and_surfaces() {
    let path = tmp("reload-surface");
    let log = Log::open(path.clone(), capscene::fingerprint(SCENE_N));
    log.clear();

    // "Run A": the live record stream the engine thread writes — a bind, a field edit, a describe-create,
    // a second bind, then an undo of that second bind.
    let (mut a, scene_a, bar, p1, p2) = make();

    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind p1");
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: p1.to_loro_key(),
    });

    let edit = EditTx {
        client_op_id: "ui-1".into(),
        label: "set HealthBar.width".into(),
        patches: vec![],
        intent: EditIntent::SetField {
            id: bar.to_loro_key(),
            component: "HealthBar".into(),
            field: "width".into(),
            value: serde_json::json!(2.5),
        },
    };
    assert!(
        apply_edit(&mut a, &edit).rejects.is_empty(),
        "the field edit commits live"
    );
    log.append(&Record::Edit(edit));

    let (created, _kind) = capscene::describe_create(&mut a, &scene_a, "health bar", [0.0; 3])
        .expect("describe-create");
    log.append(&Record::Describe {
        query: "health bar".into(),
        pos: [0.0; 3],
    });

    capscene::bind(&mut a, &scene_a, bar, p2).expect("bind p2");
    log.append(&Record::Bind {
        from: bar.to_loro_key(),
        to: p2.to_loro_key(),
    });

    assert!(a.undo(), "undo the p2 bind");
    log.append(&Record::Undo);
    drop(a); // close

    // Lost-write guard: the file really has the header + all five records on disk (catches a silently
    // swallowed write — the failure mode the live diagnosis ruled out, now regression-locked).
    let body = std::fs::read_to_string(&path).expect("log file exists after appends");
    assert!(
        body.lines().next().unwrap().starts_with("#mtk "),
        "header written"
    );
    assert_eq!(
        body.lines().filter(|l| !l.starts_with("#mtk ")).count(),
        5,
        "all five records persisted"
    );

    // "Run B": a true close→reopen — fresh deterministic seed + replay. A matching-fingerprint log is
    // NOT discarded (the guard the live shell depends on), and the live stream replays clean at 5000.
    let (b, applied, skipped) = relaunch(&log);
    assert_eq!(
        (applied, skipped),
        (5, 0),
        "every live record replays at SCENE_N (none discarded or diverged)"
    );

    // Net state restored.
    assert!(has_binding(&b, bar, p1), "the surviving bind is restored");
    assert!(
        !has_binding(&b, bar, p2),
        "the undone bind does NOT persist (replay reproduces the undo too)"
    );
    let width = b
        .components_of(bar)
        .get("HealthBar")
        .and_then(|m| m.get("width").cloned());
    assert!(
        matches!(width, Some(FieldValue::Number(n)) if (n - 2.5).abs() < 1e-9),
        "the field edit is restored (HealthBar.width == 2.5), got {width:?}"
    );
    assert!(
        b.ecs_entity(created).is_some(),
        "the described entity is recreated with the same deterministic id"
    );
    assert_eq!(
        b.entity_count(),
        SCENE_N + 1,
        "exactly one new entity (the describe-create) beyond the seed"
    );

    // Surfacing seam: `project_full` — the connect-time load the WebView applies, and the source the
    // viewport's tracking lines are built from — carries the surviving edge and not the undone one.
    // (Pre-fix the data was here too; this locks the seam so a regression can't silently un-surface it.)
    let pf = project_full(&b);
    let has_edge = |to: EntityId| {
        pf.ops.iter().any(|op| {
            matches!(op,
                ProjectionOp::AddEdge { from, rel, to: t }
                    if *from == bar.to_loro_key() && rel == TRACKS && *t == to.to_loro_key())
        })
    };
    assert!(
        has_edge(p1),
        "project_full surfaces the restored binding edge (panel + viewport lines read this)"
    );
    assert!(
        !has_edge(p2),
        "project_full does not surface the undone binding edge"
    );

    // Viewport seam: `tracking_segments` (the source the wgpu `vs_line` pass draws) reflects exactly the
    // surviving bind — one segment (2 endpoints, at the two entities' centres) for bar→p1, none for the
    // undone p2. Guards the most novel/visual part of the fix without a live GPU.
    let segs = capscene::tracking_segments(&b);
    assert_eq!(
        segs.len(),
        2,
        "exactly one tracking-line segment (bar→p1) survives; the undone bar→p2 contributes none"
    );
    let pos = capscene::positions(&b);
    let close = |u: [f32; 3], v: [f32; 3]| (0..3).all(|i| (u[i] - v[i]).abs() < 1e-6);
    let bar_pos = pos[&b.ecs_entity(bar).unwrap()];
    let p1_pos = pos[&b.ecs_entity(p1).unwrap()];
    assert!(
        close(segs[0], bar_pos),
        "segment starts at the requirer's centre"
    );
    assert!(
        close(segs[1], p1_pos),
        "segment ends at the bound provider's centre"
    );

    log.clear();
}
