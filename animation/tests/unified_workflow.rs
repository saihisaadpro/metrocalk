use std::time::Duration;

use metrocalk_animation::{
    AnimValue, AnimationRuntimeInstance, AnimationUpdateMode, EventId, KeyId, MarkerId,
    PropertyPath, Sequence, SequenceId, Severity, Tick, TrackId,
};

const FIXTURE: &str = include_str!("fixtures/unified_workflow_v1.json");

fn fixture_sequence() -> Sequence {
    serde_json::from_str(FIXTURE).expect("the repository fixture must remain valid JSON")
}

fn assert_number(value: &AnimValue, expected: f64) {
    let AnimValue::Number(actual) = value else {
        panic!("expected a numeric animation value, got {value:?}");
    };
    assert!((actual - expected).abs() < 1.0e-12);
}

fn duplicate_with_fresh_ids(mut sequence: Sequence) -> Sequence {
    sequence.id = SequenceId::new("sequence:unified-workflow-v1:copy");
    for track in &mut sequence.tracks {
        track.id = TrackId::new(format!("{}:copy", track.id));
        for key in &mut track.keyframes {
            key.id = KeyId::new(format!("{}:copy", key.id));
        }
    }
    for marker in &mut sequence.markers {
        marker.id = MarkerId::new(format!("{}:copy", marker.id));
    }
    for event in &mut sequence.events {
        event.id = EventId::new(format!("{}:copy", event.id));
    }
    sequence
}

#[test]
fn unified_fixture_round_trips_and_previews_without_mutating_authored_data() {
    let authored_json = FIXTURE.to_owned();
    let authored_json_before_preview = authored_json.clone();
    let sequence = fixture_sequence();
    let sequence_before_preview = sequence.clone();

    let issues = sequence.validate();
    assert!(
        issues.iter().all(|issue| issue.severity != Severity::Error),
        "fixture should compile without validation errors: {issues:#?}"
    );
    assert_eq!(sequence.tracks.len(), 4);
    assert_eq!(sequence.markers.len(), 1);
    assert_eq!(sequence.events.len(), 1);

    let compiled = sequence
        .compile()
        .expect("the workflow fixture should compile");
    let original_hash = compiled.stable_hash.clone();
    let sprite_path = PropertyPath::new("entity:hero", "Sprite", "frame");
    let translation_path = PropertyPath::new("entity:hero", "Transform", "translation");
    let opacity_path = PropertyPath::new("entity:hud-panel", "UiStyle", "opacity");
    let visibility_path = PropertyPath::new("entity:hud-panel", "UiStyle", "visible");

    let mut manual = AnimationRuntimeInstance::new(compiled.clone(), AnimationUpdateMode::Manual);
    let frame = manual.seek(Tick(15));
    assert_eq!(frame.raw_tick, Tick(15));
    assert_eq!(
        frame
            .write(&sprite_path, &TrackId::new("track:hero:sprite-frame"))
            .expect("2D sprite write")
            .value,
        AnimValue::Integer(0)
    );
    assert_eq!(
        frame
            .write(
                &translation_path,
                &TrackId::new("track:hero:transform-translation"),
            )
            .expect("3D transform write")
            .value,
        AnimValue::Vec3([1.0, 0.25, -0.5])
    );
    assert_number(
        &frame
            .write(&opacity_path, &TrackId::new("track:hud:opacity"))
            .expect("UI opacity write")
            .value,
        0.5125,
    );
    assert_eq!(
        frame
            .write(&visibility_path, &TrackId::new("track:hud:visible"))
            .expect("UI visibility write")
            .value,
        AnimValue::Boolean(true)
    );
    assert!(
        manual.events().is_empty(),
        "scrubbing must not dispatch events"
    );

    manual.advance(Tick(15));
    assert_eq!(manual.current_raw_tick(), Tick(30));
    assert_eq!(manual.events().len(), 1);
    assert_eq!(
        manual.events()[0].event.id.as_str(),
        "event:attack-window-open"
    );

    let preview = manual.preview(Tick(90));
    assert_eq!(preview.raw_tick, Tick(90));
    assert_eq!(manual.current_raw_tick(), Tick(30));
    assert_eq!(manual.events().len(), 1);
    assert_eq!(authored_json, authored_json_before_preview);
    assert_eq!(sequence, sequence_before_preview);

    let mut fixed = AnimationRuntimeInstance::new(
        compiled,
        AnimationUpdateMode::Fixed {
            ticks_per_update: Tick(30),
        },
    );
    let fixed_frame = fixed.update(Duration::from_secs(99));
    assert_eq!(fixed_frame.raw_tick, Tick(30));
    assert_eq!(
        fixed_frame
            .write(&sprite_path, &TrackId::new("track:hero:sprite-frame"))
            .expect("fixed-clock sprite write")
            .value,
        AnimValue::Integer(1)
    );
    assert_eq!(fixed.events().len(), 1);
    assert_eq!(fixed.events()[0].event.name, "attack_window_open");

    let serialized = serde_json::to_string_pretty(&sequence).expect("sequence should serialize");
    let reopened: Sequence =
        serde_json::from_str(&serialized).expect("serialized sequence should reopen");
    assert_eq!(reopened, sequence);
    assert_eq!(
        reopened
            .compile()
            .expect("reopened sequence should compile")
            .stable_hash,
        original_hash
    );
}

#[test]
fn duplicated_fixture_uses_fresh_stable_ids_but_retains_authored_behavior() {
    let original = fixture_sequence();
    let duplicate = duplicate_with_fresh_ids(original.clone());

    assert_ne!(duplicate.id, original.id);
    assert!(duplicate
        .tracks
        .iter()
        .zip(&original.tracks)
        .all(|(copy, source)| copy.id != source.id));
    assert!(duplicate
        .tracks
        .iter()
        .zip(&original.tracks)
        .all(|(copy, source)| copy
            .keyframes
            .iter()
            .zip(&source.keyframes)
            .all(|(copy_key, source_key)| copy_key.id != source_key.id)));

    let original_plan = original.compile().expect("source should compile");
    let duplicate_plan = duplicate.compile().expect("duplicate should compile");
    assert_ne!(duplicate_plan.stable_hash, original_plan.stable_hash);

    let source_values: Vec<_> = original_plan
        .evaluate(Tick(75))
        .bindings
        .into_iter()
        .map(|binding| (binding.binding, binding.value))
        .collect();
    let duplicate_values: Vec<_> = duplicate_plan
        .evaluate(Tick(75))
        .bindings
        .into_iter()
        .map(|binding| (binding.binding, binding.value))
        .collect();
    assert_eq!(duplicate_values, source_values);
}
