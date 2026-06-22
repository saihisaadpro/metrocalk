//! Real `.mtk` **project save → close → open** round-trip (M10.3, ADR-033) — the headline "save your
//! work and reopen it", verified headless on the real engine. Asserts the scene + binding **and** the
//! capability pairs restore (so the reveal/bind query works after open — ADR-032), that save is atomic +
//! deterministic, and that a corrupt/newer file is refused with an explained error, never a panic.

use std::collections::HashMap;
use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::{Entity, FlecsWorld};

use metrocalk_editor_shell::capscene::{self, CapResolver, CapScene, TRACKS};
use metrocalk_editor_shell::project::{self, OpenError};
use metrocalk_editor_shell::reveal::{reveal, Context};

fn engine_with_resolver() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

fn ctx<'a>(
    scene: &'a CapScene,
    pos: &'a HashMap<Entity, [f32; 3]>,
    recency: &'a HashMap<Entity, u64>,
) -> Context<'a> {
    Context {
        cap_name: &scene.cap_name,
        position: pos,
        recency,
    }
}

fn spawn(engine: &mut Engine<FlecsWorld>, scene: &CapScene, role: &str, x: f64) -> EntityId {
    let id = engine.alloc_entity_id();
    let mut ops = vec![
        Op::CreateEntity { id, parent: None },
        Op::SetField {
            entity: id,
            component: "Transform".into(),
            field: "x".into(),
            value: FieldValue::Number(x),
        },
    ];
    if role == "bar" {
        ops.push(Op::SetField {
            entity: id,
            component: "HealthBar".into(),
            field: "width".into(),
            value: FieldValue::Number(1.0),
        });
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.requires,
            target: scene.cap("Health"),
        });
    } else {
        ops.push(Op::SetField {
            entity: id,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(100),
        });
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: scene.cap("Health"),
        });
    }
    engine.commit("spawn", ops).expect("spawn commits");
    id
}

/// A unique temp path for this test process (no Date/random needed — process id + a per-test tag).
fn temp_mtk(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-proj-{}-{tag}.mtk", std::process::id()))
}

#[test]
fn save_close_open_restores_scene_binding_and_caps() {
    let path = temp_mtk("roundtrip");
    let _ = std::fs::remove_file(&path);

    // Build + bind, then SAVE.
    let (mut a, scene_a) = engine_with_resolver();
    let bar = spawn(&mut a, &scene_a, "bar", 0.0);
    let p1 = spawn(&mut a, &scene_a, "provider", 1.0);
    let p2 = spawn(&mut a, &scene_a, "provider", 2.0);
    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind commits");
    project::save(&a, &path).expect("save writes the .mtk atomically");

    // The atomic write left a real file and no temp residue.
    assert!(path.exists(), "the .mtk project file exists after save");
    assert!(
        std::fs::metadata(&path).unwrap().len() > 8,
        "the file is more than just the header"
    );
    let mut tmp = path.clone().into_os_string();
    tmp.push(".tmp");
    assert!(
        !PathBuf::from(tmp).exists(),
        "no leftover temp file after a successful save"
    );

    // OPEN into a fresh engine (clean world + scene + resolver) — the "reopen" half.
    let (mut b, scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("open re-opens the saved project");

    // Scene + binding restored.
    assert!(
        b.entity_exists(bar) && b.entity_exists(p1) && b.entity_exists(p2),
        "all entities restored from the project file"
    );
    assert!(
        b.bindings()
            .iter()
            .any(|(f, k, t)| *f == bar && k == TRACKS && *t == p1),
        "the binding edge survives save→open"
    );

    // Capabilities restored → the reveal works after open (ADR-032), excluding the bound provider.
    let bar_ecs = b.ecs_entity(bar).unwrap();
    let pos = capscene::positions(&b);
    let recency = HashMap::new();
    let r = reveal(
        b.world(),
        bar_ecs,
        scene_b.rels,
        &ctx(&scene_b, &pos, &recency),
    );
    assert_eq!(
        r.required,
        vec!["Health".to_string()],
        "the bar still requires Health"
    );
    let compat: Vec<EntityId> = r
        .compatible
        .iter()
        .filter_map(|c| b.entity_id_of(c.entity))
        .collect();
    assert_eq!(
        compat,
        vec![p2],
        "reveal offers exactly the unbound provider after open"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn save_is_deterministic_and_overwrites_atomically() {
    let path = temp_mtk("determinism");
    let _ = std::fs::remove_file(&path);

    let (mut a, scene_a) = engine_with_resolver();
    let bar = spawn(&mut a, &scene_a, "bar", 0.0);
    let p1 = spawn(&mut a, &scene_a, "provider", 1.0);
    capscene::bind(&mut a, &scene_a, bar, p1).expect("bind");

    project::save(&a, &path).expect("first save");
    let first = std::fs::read(&path).unwrap();
    // Save again (overwrite the existing file) — same scene ⇒ byte-identical (no timestamp; the
    // "two saves of the same scene differ" adversarial guard).
    project::save(&a, &path).expect("second save overwrites atomically");
    let second = std::fs::read(&path).unwrap();
    assert_eq!(
        first, second,
        "two saves of the same scene are byte-identical"
    );

    // The overwritten file still opens.
    let (mut b, _scene_b) = engine_with_resolver();
    project::open_into(&mut b, &path).expect("the overwritten project still opens");
    assert!(b.entity_exists(bar), "scene intact after the overwrite");

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_corrupt_file_is_refused_not_a_crash() {
    let path = temp_mtk("corrupt");
    // A valid envelope (MTKP + v1) wrapping garbage that is NOT a Loro snapshot.
    let bytes = metrocalk_core::project::build(b"this is not a loro snapshot at all");
    std::fs::write(&path, &bytes).unwrap();

    let (mut b, _scene_b) = engine_with_resolver();
    let result = project::open_into(&mut b, &path);
    // A corrupt payload surfaces as an explained Load (import failed) or Format error — never an Ok,
    // never a panic.
    assert!(
        matches!(result, Err(OpenError::Load(_) | OpenError::Format(_))),
        "a corrupt project must be refused with an explained error: {result:?}"
    );

    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_nonexistent_path_is_an_io_error_not_a_crash() {
    let path = temp_mtk("does-not-exist");
    let _ = std::fs::remove_file(&path);
    let (mut b, _scene_b) = engine_with_resolver();
    assert!(
        matches!(project::open_into(&mut b, &path), Err(OpenError::Io(_))),
        "opening a missing file is an explained IO error"
    );
}
