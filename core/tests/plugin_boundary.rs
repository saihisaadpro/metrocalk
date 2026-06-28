//! M12.3 (ADR-047) — the **Rules->plugin boundary** (the honest ceiling), in `/core`. A Rule action can
//! invoke a **registered plugin** (`RunPlugin`); an unknown plugin is **Blocked + explained** (ADR-016).
//! The plugin is registry-typed (reveal/explain applies) + carries the **determinism** flag (the Play/replay
//! gate). Rules **orchestrate**; the plugin **computes** — the line is drawn here. The sandboxed *running*
//! of the plugin is tested in `/plugins` + `/editor-shell`; this guards the core vocabulary + validator.

use metrocalk_core::rules::{Action, RuleData, RuleError};
use metrocalk_core::stdlib::{
    standard_actions, standard_components, standard_events, standard_plugins,
};
use metrocalk_core::{validate_rule, Engine, FieldValue, Op, Registry, RUN_PLUGIN_ACTION};
use metrocalk_ecs::FlecsWorld;

fn registry() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for m in standard_components() {
        reg.register(m).expect("register");
    }
    for e in standard_events() {
        reg.register_event(e);
    }
    for a in standard_actions() {
        reg.register_action(a);
    }
    for p in standard_plugins() {
        reg.register_plugin(p);
    }
    reg
}

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

fn spawn(e: &mut Engine<FlecsWorld>) -> String {
    let id = e.alloc_entity_id();
    e.commit("spawn", vec![Op::CreateEntity { id, parent: None }])
        .expect("create");
    id.to_loro_key()
}

/// A rule whose Then invokes a plugin: When EntitySpawned -> RunPlugin `plugin` on `entity`. The plugin
/// NAME lives in the `component` slot; `field`/`value` carry the plugin's own input contract.
fn run_plugin_rule(entity: &str, plugin: &str) -> RuleData {
    RuleData {
        name: "arrange on spawn".into(),
        enabled: true,
        event: "EntitySpawned".into(),
        conditions: vec![],
        actions: vec![Action {
            action: RUN_PLUGIN_ACTION.into(),
            entity: entity.into(),
            component: plugin.into(),
            field: "input".into(),
            value: FieldValue::Str(r#"{"ids":["1_0"],"seed":1}"#.into()),
        }],
    }
}

#[test]
fn a_rule_can_invoke_a_registered_plugin() {
    let reg = registry();
    let mut e = engine();
    let ent = spawn(&mut e);
    let rule = run_plugin_rule(&ent, "arrange");
    validate_rule(&reg, &rule, |id| e.entity_exists(id)).expect(
        "a RunPlugin action over a registered plugin validates (the honest-ceiling boundary)",
    );
    // The plugin is registry-typed: reveal/explain + the determinism flag are available.
    let meta = reg.plugin("arrange").expect("arrange is registered");
    assert!(
        meta.deterministic,
        "the example plugin is deterministic (so it's Play/replay eligible)"
    );
}

#[test]
fn an_unknown_plugin_is_blocked_and_explained() {
    let reg = registry();
    let mut e = engine();
    let ent = spawn(&mut e);
    let rule = run_plugin_rule(&ent, "definitely_not_a_plugin");
    let err = validate_rule(&reg, &rule, |id| e.entity_exists(id)).unwrap_err();
    assert!(matches!(err, RuleError::UnknownPlugin(_)));
    assert!(
        err.to_string().contains("isn't a plugin the engine knows"),
        "the rejection explains itself: {err}"
    );
}

#[test]
fn run_plugin_is_a_closed_registry_action_verb() {
    // The honest ceiling is a CLOSED vocabulary verb (still no free code in a Rule) — RunPlugin is in the
    // registry's action set, exactly like SetField / AdjustCounter.
    let reg = registry();
    assert!(reg.has_action(RUN_PLUGIN_ACTION));
    assert!(reg.has_plugin("arrange"));
}
