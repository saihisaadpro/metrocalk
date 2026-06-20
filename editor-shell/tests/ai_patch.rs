//! M6 AI seam — the schema-validated transactional-patch contract (invariant 3), through the **real**
//! `/core` engine. Every AI/generation scene mutation rides this: validated against the registry schema
//! and engine state, applied through the one commit pipeline (undoable) on success, **rejected-as-UX**
//! (nothing applied, all-or-nothing) on a malformed/over-reaching patch. No raw LLM mutation path.

use metrocalk_core::stdlib;
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;

use metrocalk_editor_shell::ai::{apply_ai_patch, AiPatch, PatchOp};
use metrocalk_editor_shell::capscene::{self, CapScene};

const N: usize = 50;

fn seeded() -> (Engine<FlecsWorld>, CapScene, EntityId) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    let index = capscene::seed(&mut engine, &scene, N).expect("seed");
    engine.clear_history();
    let bar = index.health_bars[0]; // a HealthBar — has the stdlib HealthBar.width (Number) field
    (engine, scene, bar)
}

fn patch(id: &str, component: &str, field: &str, value: serde_json::Value) -> AiPatch {
    AiPatch {
        client_op_id: "ai-1".into(),
        ops: vec![PatchOp::SetField {
            id: id.to_string(),
            component: component.to_string(),
            field: field.to_string(),
            value,
        }],
    }
}

fn width(engine: &Engine<FlecsWorld>, bar: EntityId) -> Option<FieldValue> {
    engine
        .components_of(bar)
        .get("HealthBar")
        .and_then(|m| m.get("width").cloned())
}

#[test]
fn a_valid_ai_patch_commits_and_is_undoable() {
    let (mut engine, _scene, bar) = seeded();
    let lib = stdlib::standard_components();
    let before = width(&engine, bar);

    let delta = apply_ai_patch(
        &mut engine,
        &lib,
        "ai-edit",
        &patch(
            &bar.to_loro_key(),
            "HealthBar",
            "width",
            serde_json::json!(3.5),
        ),
    );
    assert!(
        delta.rejects.is_empty(),
        "a schema-valid patch is not rejected"
    );
    assert_eq!(delta.confirms, vec!["ai-1".to_string()]);
    assert_eq!(
        width(&engine, bar),
        Some(FieldValue::Number(3.5)),
        "the field was set through the pipeline"
    );

    // undoable.
    assert!(engine.undo());
    assert_eq!(width(&engine, bar), before, "undo reverses the AI patch");
}

#[test]
fn unknown_component_is_rejected_not_applied() {
    let (mut engine, _scene, bar) = seeded();
    let lib = stdlib::standard_components();
    let delta = apply_ai_patch(
        &mut engine,
        &lib,
        "ai",
        &patch(
            &bar.to_loro_key(),
            "Wobblifier",
            "intensity",
            serde_json::json!(9),
        ),
    );
    assert_eq!(delta.rejects.len(), 1);
    assert!(delta.rejects[0].reason.contains("unknown component"));
    assert!(
        !engine.components_of(bar).contains_key("Wobblifier"),
        "nothing applied"
    );
}

#[test]
fn unknown_field_is_rejected() {
    let (mut engine, _scene, bar) = seeded();
    let lib = stdlib::standard_components();
    let delta = apply_ai_patch(
        &mut engine,
        &lib,
        "ai",
        &patch(
            &bar.to_loro_key(),
            "HealthBar",
            "sparkliness",
            serde_json::json!(1.0),
        ),
    );
    assert_eq!(delta.rejects.len(), 1);
    assert!(delta.rejects[0].reason.contains("no field"));
}

#[test]
fn wrong_value_type_is_rejected() {
    let (mut engine, _scene, bar) = seeded();
    let lib = stdlib::standard_components();
    // HealthBar.width is a Number — a string value must be rejected, not silently coerced.
    let delta = apply_ai_patch(
        &mut engine,
        &lib,
        "ai",
        &patch(
            &bar.to_loro_key(),
            "HealthBar",
            "width",
            serde_json::json!("huge"),
        ),
    );
    assert_eq!(delta.rejects.len(), 1);
    assert!(delta.rejects[0].reason.contains("is not a"));
    assert_ne!(width(&engine, bar), Some(FieldValue::Str("huge".into())));
}

#[test]
fn non_existent_entity_is_rejected() {
    let (mut engine, _scene, _bar) = seeded();
    let lib = stdlib::standard_components();
    let delta = apply_ai_patch(
        &mut engine,
        &lib,
        "ai",
        &patch("1_ffff", "HealthBar", "width", serde_json::json!(2.0)),
    );
    assert_eq!(delta.rejects.len(), 1);
    assert!(delta.rejects[0].reason.contains("does not exist"));
}

#[test]
fn a_patch_is_all_or_nothing() {
    // One invalid op rejects the WHOLE patch — no partial application.
    let (mut engine, _scene, bar) = seeded();
    let lib = stdlib::standard_components();
    let before = width(&engine, bar);
    let multi = AiPatch {
        client_op_id: "ai-multi".into(),
        ops: vec![
            PatchOp::SetField {
                id: bar.to_loro_key(),
                component: "HealthBar".into(),
                field: "width".into(),
                value: serde_json::json!(7.0),
            },
            PatchOp::SetField {
                id: bar.to_loro_key(),
                component: "HealthBar".into(),
                field: "nope".into(), // invalid → rejects the whole patch
                value: serde_json::json!(1.0),
            },
        ],
    };
    let delta = apply_ai_patch(&mut engine, &lib, "ai", &multi);
    assert_eq!(delta.rejects.len(), 1);
    assert_eq!(
        width(&engine, bar),
        before,
        "the valid op was NOT applied (all-or-nothing)"
    );
}
