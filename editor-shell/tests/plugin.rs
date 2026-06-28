//! M12.3 (ADR-047) — the plugin round-trip in the shell: a sandboxed WASM plugin runs, and its effect lands
//! as a **schema-validated undoable transaction** (the ADR-017 patch contract) — run → committed → undoable
//! → **survives reload** (the deterministic plugin is re-run on replay). A misbehaving / over-reaching
//! plugin is **contained** (a missing plugin is an Err, never a crash) or **rejected-as-UX** (an effect on a
//! non-existent entity can't reach past the validation gate — a plugin is not a raw mutation path). The
//! sandbox containment (budget / allow-list) is tested in `/plugins`; this guards the effect-is-a-transaction
//! + persistence wiring.

use std::path::PathBuf;

use metrocalk_core::stdlib::standard_components;
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::capscene::{self, CapScene};
use metrocalk_editor_shell::persist::{Log, Record};
use metrocalk_editor_shell::plugin_host::run_plugin;
use metrocalk_editor_shell::MeshCatalog;

const N: usize = 50;

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("mtk-test-{name}.jsonl"))
}

fn seeded() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    (engine, scene)
}

fn eid(key: &str) -> EntityId {
    EntityId::from_loro_key(key).expect("key")
}

#[test]
fn a_plugin_effect_is_an_undoable_transaction() {
    let (mut a, _scene) = seeded();
    let schema = standard_components();
    let input = r#"{"ids":["1_0","1_1","1_2"],"seed":3,"spacing":2.0}"#;

    let before = a.get_field(eid("1_1"), "Transform", "px");
    let delta = run_plugin(&mut a, &schema, "arrange", input).expect("the sandboxed plugin runs");
    assert!(
        delta.rejects.is_empty() && !delta.confirms.is_empty(),
        "the plugin effect committed through the pipeline: {delta:?}"
    );
    let after = a.get_field(eid("1_1"), "Transform", "px");
    assert!(
        matches!(after, Some(FieldValue::Number(_))),
        "the plugin set Transform.px (a numeric value)"
    );
    assert_ne!(before, after, "the algorithmic effect moved the entity");

    // ONE undo reverts the whole plugin effect (it's a single transaction).
    assert!(a.undo());
    assert_eq!(
        a.get_field(eid("1_1"), "Transform", "px"),
        before,
        "undo reverts the plugin effect — a plugin is not a privileged path"
    );
}

#[test]
fn a_plugin_effect_survives_close_then_reopen_via_replay() {
    let log = Log::open(tmp("plugin"), capscene::fingerprint(N));
    log.clear();

    // run A: run the plugin, persist the record.
    let (mut a, _scene_a) = seeded();
    let input = r#"{"ids":["1_0","1_1","1_2"],"seed":3,"spacing":2.0}"#.to_string();
    let delta = run_plugin(&mut a, &standard_components(), "arrange", &input).expect("run");
    assert!(delta.rejects.is_empty());
    let want = a.get_field(eid("1_1"), "Transform", "px").expect("set");
    log.append(&Record::RunPlugin {
        name: "arrange".to_string(),
        input: input.clone(),
    });
    drop(a);

    // run B: fresh deterministic seed + replay → the plugin re-runs deterministically + re-applies its effect.
    let (mut b, scene) = seeded();
    let (applied, _skipped) = log.replay(&mut b, &scene, &MeshCatalog::new());
    assert_eq!(applied, 1, "the RunPlugin record replayed");
    assert_eq!(
        b.get_field(eid("1_1"), "Transform", "px"),
        Some(want),
        "the plugin effect survives reload (the deterministic plugin re-derives the same arrangement)"
    );
    log.clear();
}

#[test]
fn a_missing_plugin_and_an_overreaching_effect_are_contained() {
    let (mut a, _s) = seeded();
    let schema = standard_components();
    // An unknown plugin is contained as an Err — never a crash.
    assert!(run_plugin(&mut a, &schema, "nope", "{}").is_err());
    // An effect targeting an entity that doesn't exist is REJECTED-as-UX (the ADR-017 guard) — a plugin
    // can't reach past the registry + engine-state validation, and nothing is applied (all-or-nothing).
    let delta =
        run_plugin(&mut a, &schema, "arrange", r#"{"ids":["ff_ff"],"seed":1}"#).expect("run ok");
    assert!(
        !delta.rejects.is_empty(),
        "an effect on a non-existent entity is rejected, not applied: {delta:?}"
    );
}
