//! Registry behaviour: capability queries run through the M1.2 `World` wrapper, metadata
//! round-trips byte-identically, and malformed JSON Schemas are rejected with actionable errors.

use metrocalk_core::registry::RegistryError;
use metrocalk_core::stdlib::standard_components;
use metrocalk_core::{ComponentMeta, FieldType, Registry};
use metrocalk_ecs::FlecsWorld;

fn registry_with_stdlib() -> Registry<FlecsWorld> {
    let mut reg = Registry::new(FlecsWorld::new());
    for meta in standard_components() {
        reg.register(meta).expect("stdlib component registers");
    }
    reg
}

#[test]
fn capability_queries_through_wrapper() {
    let reg = registry_with_stdlib();
    assert_eq!(reg.len(), 17); // 13 base kinds + the 4 M12.1 rule-target primitives (ADR-045)

    // "what provides Health?" — single provider.
    assert_eq!(reg.providers_of("Health"), vec!["Health"]);
    // Two kinds provide Renderable (sorted result).
    assert_eq!(
        reg.providers_of("Renderable"),
        vec!["MeshRenderer", "Sprite"]
    );
    // What binds to Health? (requires it)
    assert_eq!(reg.requirers_of("Health"), vec!["HealthBar"]);
    assert_eq!(reg.observers_of("Health"), vec!["HealthBar"]);
    // Unknown capability → empty, not an error.
    assert!(reg.providers_of("Nonexistent").is_empty());
    // Many kinds require Spatial.
    assert!(reg.requirers_of("Spatial").len() >= 6);

    // M8.2 physics intent wiring: RigidBody provides "Physics"; Collider + Joint REQUIRE it, so a
    // Collider rides the M3.1 reveal as a one-click attach onto a RigidBody (the "this body needs a
    // collider" intent) — exactly like HealthBar↔Health.
    assert_eq!(reg.providers_of("Physics"), vec!["RigidBody"]);
    assert_eq!(reg.requirers_of("Physics"), vec!["Collider", "Joint"]);
    assert_eq!(reg.providers_of("Collision"), vec!["Collider"]);
}

#[test]
fn every_stdlib_component_round_trips_byte_identical() {
    for meta in standard_components() {
        let s1 = meta.to_json();
        let back = ComponentMeta::from_json(&s1).expect("round-trips");
        assert_eq!(back, meta, "struct equality for {}", meta.name);
        assert_eq!(
            back.to_json(),
            s1,
            "byte-identical re-serialize for {}",
            meta.name
        );
    }
}

#[test]
fn registers_from_json_schema_with_types_required_format_and_hints() {
    let schema = r#"{
        "type": "object",
        "properties": {
            "hp":    { "type": "integer", "description": "current hit points" },
            "maxHp": { "type": "integer" },
            "regen": { "type": "number" },
            "icon":  { "type": "string", "format": "asset" }
        },
        "required": ["hp", "maxHp"]
    }"#;

    let meta = ComponentMeta::builder("SchemaHealth")
        .fields_from_json_str(schema)
        .expect("valid schema parses")
        .provides("Health")
        .build();

    // fields sorted by name: hp, icon, maxHp, regen
    let names: Vec<&str> = meta.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["hp", "icon", "maxHp", "regen"]);

    let hp = meta.fields.iter().find(|f| f.name == "hp").unwrap();
    assert_eq!(hp.ty, FieldType::Integer);
    assert!(hp.required);
    let icon = meta.fields.iter().find(|f| f.name == "icon").unwrap();
    assert_eq!(icon.format.as_deref(), Some("asset"));
    let regen = meta.fields.iter().find(|f| f.name == "regen").unwrap();
    assert!(!regen.required);
    assert_eq!(
        meta.ui_hints.get("hp").map(String::as_str),
        Some("current hit points")
    );

    // and it's queryable through the wrapper
    let mut reg = Registry::new(FlecsWorld::new());
    reg.register(meta).unwrap();
    assert_eq!(reg.providers_of("Health"), vec!["SchemaHealth"]);
}

#[test]
#[allow(clippy::type_complexity)] // a table of (schema, expected-variant predicate, needle) test cases
fn malformed_schemas_are_rejected_with_actionable_errors() {
    let cases: &[(&str, fn(&RegistryError) -> bool, &str)] = &[
        (
            r#"{ "type": "array" }"#,
            |e| matches!(e, RegistryError::RootNotObject),
            "object",
        ),
        (
            r#"{ "type": "object", "properties": { "hp": { "type": "banana" } } }"#,
            |e| matches!(e, RegistryError::FieldUnsupportedType { .. }),
            "banana",
        ),
        (
            r#"{ "type": "object", "properties": { "hp": {} } }"#,
            |e| matches!(e, RegistryError::FieldMissingType { .. }),
            "hp",
        ),
        (
            r#"{ "type": "object", "properties": { "hp": { "type": "integer" } }, "required": ["nope"] }"#,
            |e| matches!(e, RegistryError::RequiredUnknownField(_)),
            "nope",
        ),
        (
            r#"{ "type": "object", "properties": 5 }"#,
            |e| matches!(e, RegistryError::PropertiesNotObject(_)),
            "number",
        ),
        (
            "not json at all",
            |e| matches!(e, RegistryError::InvalidJson(_)),
            "JSON",
        ),
    ];

    for (schema, is_expected, needle) in cases {
        let err = ComponentMeta::builder("Bad")
            .fields_from_json_str(schema)
            .expect_err("malformed schema must be rejected, not accepted or panic");
        assert!(
            is_expected(&err),
            "wrong variant for {schema:?}: got {err:?}"
        );
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains(&needle.to_lowercase()),
            "error message {msg:?} should mention {needle:?} to be actionable"
        );
    }
}

#[test]
fn rejects_empty_name_and_duplicates() {
    let mut reg = Registry::new(FlecsWorld::new());
    assert_eq!(
        reg.register(ComponentMeta::builder("").build())
            .unwrap_err(),
        RegistryError::EmptyName
    );
    reg.register(ComponentMeta::builder("Health").provides("Health").build())
        .unwrap();
    assert_eq!(
        reg.register(ComponentMeta::builder("Health").build())
            .unwrap_err(),
        RegistryError::Duplicate("Health".to_string())
    );
}
