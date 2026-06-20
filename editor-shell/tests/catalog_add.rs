//! M3.4 "+ Add" wiring — Add **converges** on the SAME instantiate as describe-to-create (not a
//! parallel path), and a browsed-in object survives undo + export→replay (`Record::AddKind`). The
//! unified catalog query + the category taxonomy are covered in `/core`; here we prove the editor-side
//! convergence + persistence.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::{add_kind, describe_create, Log, Record};

const N: usize = 200;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("metrocalk-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

/// An entity's full component shape (component → field → value), order-independent.
fn comps(
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> BTreeMap<String, BTreeMap<String, FieldValue>> {
    engine
        .components_of(id)
        .into_iter()
        .map(|(c, fields)| (c, fields.into_iter().collect()))
        .collect()
}

#[test]
fn add_and_describe_converge_on_the_same_instantiate() {
    let catalog: HashMap<String, String> = HashMap::new();
    let (mut a, sa) = seeded();
    let (mut b, sb) = seeded();

    // The describe door: "health bar" resolves to the HealthBar kind.
    let (did, kind) = describe_create(&mut a, &sa, "health bar", [0.0; 3], &catalog).unwrap();
    assert_eq!(kind, "HealthBar");

    // The browse door: pick the HealthBar kind directly.
    let aid = add_kind(&mut b, &sb, "HealthBar", [0.0; 3], &catalog).unwrap();

    // Both doors reach ONE pre-componentized instantiate → byte-identical component shape (the
    // convergence guard: no divergent second path).
    assert_eq!(
        comps(&a, did),
        comps(&b, aid),
        "Add and describe-to-create converge on the same instantiate"
    );
}

#[test]
fn an_unknown_kind_is_an_honest_none() {
    let (mut e, s) = seeded();
    assert!(
        add_kind(&mut e, &s, "NotARealKind", [0.0; 3], &HashMap::new()).is_none(),
        "an unknown kind doesn't fabricate an object"
    );
}

#[test]
fn an_added_kind_is_undoable_and_survives_export_then_replay() {
    let catalog: HashMap<String, String> = HashMap::new();
    let (mut a, sa) = seeded();
    let before = a.entity_count();
    let id = add_kind(&mut a, &sa, "MeshRenderer", [1.0, 0.0, 0.0], &catalog).unwrap();
    assert_eq!(a.entity_count(), before + 1);
    // One undoable transaction.
    assert!(a.undo());
    assert!(
        !a.entity_exists(id),
        "Add is a single undoable pipeline transaction"
    );

    // Persist + reload: re-seed deterministically + replay the Add record.
    let log = Log::open(tmp("add-kind"), capscene::fingerprint(N));
    log.clear();
    log.append(&Record::AddKind {
        name: "MeshRenderer".to_string(),
        pos: [1.0, 0.0, 0.0],
    });
    let (mut b, sb) = seeded();
    let (applied, skipped) = log.replay(&mut b, &sb, &catalog);
    b.clear_history();
    assert_eq!((applied, skipped), (1, 0), "the Add replayed");
    assert!(
        b.entity_exists(id),
        "the browsed-in object survived reload at the same deterministic id"
    );
    log.clear();
}
