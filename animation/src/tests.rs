use std::collections::{BTreeMap, BTreeSet};

use super::*;

fn binding(property: &str, value_kind: ValueKind) -> Binding {
    Binding {
        path: PropertyPath::new("entity:hero", "Transform", property),
        value_kind,
    }
}

fn key(id: &str, tick: i64, value: AnimValue) -> Keyframe {
    Keyframe::new(id, Tick(tick), value)
}

fn track(
    id: &str,
    property: &str,
    kind: ValueKind,
    interpolation: Interpolation,
    keyframes: Vec<Keyframe>,
) -> Track {
    Track {
        id: TrackId::new(id),
        name: property.to_owned(),
        binding: binding(property, kind),
        interpolation,
        keyframes,
        enabled: true,
    }
}

fn sequence() -> Sequence {
    Sequence::new("sequence:test", "Test", Tick(100))
}

fn sampled_value(compiled: &CompiledSequence, tick: i64) -> AnimValue {
    compiled.evaluate(Tick(tick)).bindings[0].value.clone()
}

#[test]
fn integer_time_base_converts_once_and_rejects_invalid_input() {
    let clock = TimeBase::new(48_000);
    assert_eq!(clock.from_seconds(0.5), Ok(Tick(24_000)));
    assert_eq!(clock.to_seconds(Tick(12_000)), Ok(0.25));
    assert_eq!(
        TimeBase::new(1_000_000_000).convert_tick(Tick(2_000_000_000), TimeBase::GAME_60),
        Ok(Tick(120_000))
    );
    assert_eq!(TimeBase::new(0).from_seconds(1.0), Err(TimeError::ZeroRate));
    assert_eq!(
        clock.from_seconds(f64::NAN),
        Err(TimeError::NonFiniteSeconds)
    );
}

#[test]
fn sequence_retiming_preserves_wall_time_cubic_shape_and_event_ticks() {
    let source_clock = TimeBase::new(1_000_000_000);
    let mut first = key("a", 0, AnimValue::Number(0.0));
    first.out_tangent = Some(AnimValue::Number(1.0 / 1_000_000_000.0));
    let mut second = key("b", 2_000_000_000, AnimValue::Number(2.0));
    second.in_tangent = Some(AnimValue::Number(1.0 / 1_000_000_000.0));
    let mut authored = Sequence::new(
        "sequence:nanosecond-source",
        "Nanosecond source",
        Tick(2_000_000_000),
    );
    authored.time_base = source_clock;
    authored.tracks.push(track(
        "track:cubic-timebase",
        "x",
        ValueKind::Number,
        Interpolation::Cubic,
        vec![first, second],
    ));
    authored.markers.push(Marker {
        id: MarkerId::new("marker:midpoint"),
        name: "Midpoint".into(),
        tick: Tick(1_000_000_000),
        color: None,
    });
    authored.events.push(AnimationEvent {
        id: EventId::new("event:midpoint"),
        name: "Midpoint".into(),
        tick: Tick(1_000_000_000),
        payload: None,
    });

    let source_midpoint = sampled_value(&authored.compile().unwrap(), 1_000_000_000);
    let retimed = authored.retimed(TimeBase::GAME_60).unwrap();
    assert_eq!(retimed.time_base, TimeBase::GAME_60);
    assert_eq!(retimed.duration, Tick(120_000));
    assert_eq!(retimed.tracks[0].keyframes[1].tick, Tick(120_000));
    assert_eq!(retimed.markers[0].tick, Tick(60_000));
    assert_eq!(retimed.events[0].tick, Tick(60_000));
    let runtime_midpoint = sampled_value(&retimed.compile().unwrap(), 60_000);
    let (AnimValue::Number(source_value), AnimValue::Number(runtime_value)) =
        (source_midpoint, runtime_midpoint)
    else {
        panic!("expected numeric cubic samples");
    };
    assert!((source_value - runtime_value).abs() < 1.0e-12);
    assert!((runtime_value - 1.0).abs() < 1.0e-12);
}

#[test]
fn sequence_retiming_fails_closed_on_quantization_collisions_and_fractional_integer_slopes() {
    let source_clock = TimeBase::new(1_000_000_000);
    let mut collision = Sequence::new("sequence:collision", "Collision", Tick(20_000));
    collision.time_base = source_clock;
    collision.tracks.push(track(
        "track:collision",
        "x",
        ValueKind::Number,
        Interpolation::Linear,
        vec![
            key("a", 1, AnimValue::Number(0.0)),
            key("b", 2, AnimValue::Number(1.0)),
        ],
    ));
    assert_eq!(
        collision.retimed(TimeBase::GAME_60),
        Err(TimeError::PrecisionLoss)
    );

    let mut event_collision =
        Sequence::new("sequence:event-collision", "Event collision", Tick(20_000));
    event_collision.time_base = source_clock;
    event_collision.events = vec![
        AnimationEvent {
            id: EventId::new("event:first"),
            name: "First".into(),
            tick: Tick(1),
            payload: None,
        },
        AnimationEvent {
            id: EventId::new("event:second"),
            name: "Second".into(),
            tick: Tick(2),
            payload: None,
        },
    ];
    assert_eq!(
        event_collision.retimed(TimeBase::GAME_60),
        Err(TimeError::PrecisionLoss)
    );

    let mut marker_collision = Sequence::new(
        "sequence:marker-collision",
        "Marker collision",
        Tick(20_000),
    );
    marker_collision.time_base = source_clock;
    marker_collision.markers = vec![
        Marker {
            id: MarkerId::new("marker:first"),
            name: "First".into(),
            tick: Tick(1),
            color: None,
        },
        Marker {
            id: MarkerId::new("marker:second"),
            name: "Second".into(),
            tick: Tick(2),
            color: None,
        },
    ];
    assert_eq!(
        marker_collision.retimed(TimeBase::GAME_60),
        Err(TimeError::PrecisionLoss)
    );

    let mut first = key("a", 0, AnimValue::Integer(0));
    first.out_tangent = Some(AnimValue::Integer(1));
    let mut second = key("b", 1_000_000_000, AnimValue::Integer(1));
    second.in_tangent = Some(AnimValue::Integer(1));
    let mut integer_curve = Sequence::new(
        "sequence:integer-curve",
        "Integer curve",
        Tick(1_000_000_000),
    );
    integer_curve.time_base = source_clock;
    integer_curve.tracks.push(track(
        "track:integer-curve",
        "x",
        ValueKind::Integer,
        Interpolation::Cubic,
        vec![first, second],
    ));
    assert_eq!(
        integer_curve.retimed(TimeBase::GAME_60),
        Err(TimeError::PrecisionLoss)
    );
}

#[test]
fn malformed_and_non_finite_content_returns_actionable_codes() {
    let mut authored = sequence();
    authored.time_base = TimeBase::new(0);
    authored.tracks = vec![
        track(
            "duplicate",
            "translation",
            ValueKind::Vec3,
            Interpolation::Linear,
            vec![key("key:a", 0, AnimValue::Vec3([0.0, f64::NAN, 0.0]))],
        ),
        track(
            "duplicate",
            "visible",
            ValueKind::Boolean,
            Interpolation::Cubic,
            vec![key("", 101, AnimValue::String("wrong".into()))],
        ),
    ];
    let error = authored
        .compile()
        .expect_err("malformed sequence must fail");
    let codes: BTreeSet<_> = error.issues.iter().map(|issue| issue.code).collect();
    assert!(codes.contains(&IssueCode::InvalidTimeBase));
    assert!(codes.contains(&IssueCode::DuplicateStableId));
    assert!(codes.contains(&IssueCode::NonFiniteValue));
    assert!(codes.contains(&IssueCode::UnsupportedInterpolation));
    assert!(codes.contains(&IssueCode::ValueTypeMismatch));
    assert!(codes.contains(&IssueCode::KeyOutOfRange));
    assert!(codes.contains(&IssueCode::EmptyStableId));
    assert!(error
        .issues
        .iter()
        .all(|issue| !issue.location.is_empty() && !issue.remediation.is_empty()));
}

#[test]
fn conflicting_enabled_bindings_require_an_explicit_graph_or_layer_blend() {
    let first = track(
        "track:first",
        "x",
        ValueKind::Number,
        Interpolation::Linear,
        vec![key("key:first", 0, AnimValue::Number(1.0))],
    );
    let second = track(
        "track:second",
        "x",
        ValueKind::Number,
        Interpolation::Linear,
        vec![key("key:second", 0, AnimValue::Number(2.0))],
    );

    let mut authored = sequence();
    authored.tracks = vec![second.clone(), first.clone()];
    let conflict = authored
        .validate()
        .into_iter()
        .find(|issue| issue.code == IssueCode::ConflictingBinding)
        .expect("the ambiguous flat projection must fail validation");
    assert_eq!(conflict.severity, Severity::Error);
    assert!(conflict.message.contains("'track:first', 'track:second'"));
    assert!(conflict.remediation.contains("graph/layer blend"));
    assert!(authored.compile().is_err());

    let mut reordered = sequence();
    reordered.tracks = vec![first, second.clone()];
    let reordered_conflict = reordered
        .validate()
        .into_iter()
        .find(|issue| issue.code == IssueCode::ConflictingBinding)
        .expect("reordered tracks must report the same conflict");
    assert_eq!(conflict, reordered_conflict);

    let mut disabled_alternative = second;
    disabled_alternative.enabled = false;
    reordered.tracks[1] = disabled_alternative;
    assert!(reordered
        .validate()
        .iter()
        .all(|issue| issue.code != IssueCode::ConflictingBinding));
    assert!(
        reordered.compile().is_ok(),
        "disabled alternatives may coexist"
    );
}

#[test]
fn same_tick_key_id_tie_break_converges_independent_of_arrival_order() {
    let low = key("key:001", 50, AnimValue::Number(1.0));
    let high = key("key:999", 50, AnimValue::Number(9.0));
    let mut first = sequence();
    first.tracks.push(track(
        "track:x",
        "x",
        ValueKind::Number,
        Interpolation::Linear,
        vec![
            low.clone(),
            high.clone(),
            key("key:end", 100, AnimValue::Number(10.0)),
        ],
    ));
    let mut second = first.clone();
    second.tracks[0].keyframes = vec![key("key:end", 100, AnimValue::Number(10.0)), high, low];

    let a = first.compile().unwrap();
    let b = second.compile().unwrap();
    assert_eq!(sampled_value(&a, 50), AnimValue::Number(9.0));
    assert_eq!(
        a, b,
        "compiled form converges after CRDT arrival reordering"
    );
    assert_eq!(a.stable_hash, b.stable_hash);
    assert_eq!(a.evaluate(Tick(75)), b.evaluate(Tick(75)));
}

#[test]
fn linear_and_step_sampling_cover_all_value_shapes() {
    let cases = [
        (
            ValueKind::Number,
            AnimValue::Number(0.0),
            AnimValue::Number(10.0),
            AnimValue::Number(5.0),
        ),
        (
            ValueKind::Integer,
            AnimValue::Integer(0),
            AnimValue::Integer(9),
            AnimValue::Integer(5),
        ),
        (
            ValueKind::Vec3,
            AnimValue::Vec3([0.0; 3]),
            AnimValue::Vec3([2.0, 4.0, 6.0]),
            AnimValue::Vec3([1.0, 2.0, 3.0]),
        ),
        (
            ValueKind::Vec4,
            AnimValue::Vec4([0.0; 4]),
            AnimValue::Vec4([2.0; 4]),
            AnimValue::Vec4([1.0; 4]),
        ),
        (
            ValueKind::Weights,
            AnimValue::Weights(vec![0.0, 1.0]),
            AnimValue::Weights(vec![1.0, 0.0]),
            AnimValue::Weights(vec![0.5, 0.5]),
        ),
    ];
    for (index, (kind, start, end, expected)) in cases.into_iter().enumerate() {
        let mut authored = sequence();
        authored.tracks.push(track(
            &format!("track:{index}"),
            &format!("p{index}"),
            kind,
            Interpolation::Linear,
            vec![key("a", 0, start), key("b", 100, end)],
        ));
        assert_eq!(sampled_value(&authored.compile().unwrap(), 50), expected);
    }

    let mut authored = sequence();
    authored.tracks.push(track(
        "track:string",
        "state",
        ValueKind::String,
        Interpolation::Step,
        vec![
            key("a", 0, AnimValue::String("idle".into())),
            key("b", 100, AnimValue::String("run".into())),
        ],
    ));
    assert_eq!(
        sampled_value(&authored.compile().unwrap(), 99),
        AnimValue::String("idle".into())
    );
}

#[test]
fn quaternion_linear_sampling_takes_shortest_normalized_path() {
    let mut authored = sequence();
    authored.tracks.push(track(
        "track:rotation",
        "rotation",
        ValueKind::Quaternion,
        Interpolation::Linear,
        vec![
            key("a", 0, AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0])),
            // Same orientation in the opposite quaternion hemisphere.
            key("b", 100, AnimValue::Quaternion([0.0, 0.0, 0.0, -2.0])),
        ],
    ));
    let AnimValue::Quaternion(midpoint) = sampled_value(&authored.compile().unwrap(), 50) else {
        panic!("expected quaternion")
    };
    assert!((midpoint[3] - 1.0).abs() < 1.0e-12);
    assert!((midpoint.iter().map(|value| value * value).sum::<f64>() - 1.0).abs() < 1.0e-12);
}

#[test]
fn quaternion_linear_sampling_uses_slerp_for_nontrivial_arc_fraction() {
    let mut authored = sequence();
    // Identity -> 120 degrees about +Z. At one quarter of the segment, true slerp is exactly 30 degrees.
    authored.tracks.push(track(
        "track:slerp",
        "rotation",
        ValueKind::Quaternion,
        Interpolation::Linear,
        vec![
            key("a", 0, AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0])),
            key(
                "b",
                100,
                AnimValue::Quaternion([0.0, 0.0, 3.0_f64.sqrt() / 2.0, 0.5]),
            ),
        ],
    ));
    let AnimValue::Quaternion(value) = sampled_value(&authored.compile().unwrap(), 25) else {
        panic!("expected quaternion")
    };
    let expected = [
        0.0,
        0.0,
        (15.0_f64.to_radians()).sin(),
        (15.0_f64.to_radians()).cos(),
    ];
    for index in 0..4 {
        assert!(
            (value[index] - expected[index]).abs() < 1.0e-12,
            "component {index}: {value:?}"
        );
    }

    // The exact midpoint is the 60-degree orientation and remains unit length.
    let AnimValue::Quaternion(midpoint) = sampled_value(&authored.compile().unwrap(), 50) else {
        panic!("expected quaternion")
    };
    assert!((midpoint[2] - 0.5).abs() < 1.0e-12);
    assert!((midpoint[3] - 3.0_f64.sqrt() / 2.0).abs() < 1.0e-12);
}

#[test]
fn quaternion_cubic_preserves_authored_signs_and_tangents_before_normalizing() {
    let mut first = key("a", 0, AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0]));
    first.out_tangent = Some(AnimValue::Quaternion([0.01, 0.0, 0.0, 0.0]));
    let mut second = key("b", 100, AnimValue::Quaternion([0.0, 0.0, 0.0, -1.0]));
    second.in_tangent = Some(AnimValue::Quaternion([0.0; 4]));
    let mut authored = sequence();
    authored.tracks.push(track(
        "track:cubic-rotation",
        "rotation",
        ValueKind::Quaternion,
        Interpolation::Cubic,
        vec![first, second],
    ));
    let AnimValue::Quaternion(midpoint) = sampled_value(&authored.compile().unwrap(), 50) else {
        panic!("expected quaternion")
    };
    // Component Hermite gives [0.125, 0, 0, 0] at t=0.5, then normalization gives +X exactly.
    // A forbidden endpoint hemisphere flip would instead retain a large W component.
    assert!((midpoint[0] - 1.0).abs() < 1.0e-12, "{midpoint:?}");
    assert!(midpoint[1].abs() < 1.0e-12);
    assert!(midpoint[2].abs() < 1.0e-12);
    assert!(
        midpoint[3].abs() < 1.0e-12,
        "authored -W endpoint was flipped: {midpoint:?}"
    );
}

#[test]
fn cubic_hermite_sampling_uses_optional_zero_tangents() {
    let mut authored = sequence();
    authored.tracks.push(track(
        "track:cubic",
        "x",
        ValueKind::Number,
        Interpolation::Cubic,
        vec![
            key("a", 0, AnimValue::Number(0.0)),
            key("b", 100, AnimValue::Number(1.0)),
        ],
    ));
    let AnimValue::Number(value) = sampled_value(&authored.compile().unwrap(), 25) else {
        panic!("expected number")
    };
    assert!((value - 0.15625).abs() < 1.0e-12);
}

#[test]
fn looping_and_ping_pong_evaluation_have_explicit_direction() {
    let mut looping = sequence();
    looping.loop_policy = LoopPolicy::Loop;
    looping.tracks.push(track(
        "track:x",
        "x",
        ValueKind::Number,
        Interpolation::Linear,
        vec![
            key("a", 0, AnimValue::Number(0.0)),
            key("b", 100, AnimValue::Number(100.0)),
        ],
    ));
    let evaluated = looping.compile().unwrap().evaluate(Tick(125));
    assert_eq!(evaluated.local_tick, Tick(25));
    assert_eq!(evaluated.iteration, 1);
    assert_eq!(evaluated.direction, Direction::Forward);

    let mut ping_pong = looping;
    ping_pong.loop_policy = LoopPolicy::PingPong;
    let evaluated = ping_pong.compile().unwrap().evaluate(Tick(125));
    assert_eq!(evaluated.local_tick, Tick(75));
    assert_eq!(evaluated.direction, Direction::Reverse);
    let negative = ping_pong.compile().unwrap().evaluate(Tick(-25));
    assert_eq!(negative.local_tick, Tick(25));
    assert_eq!(negative.direction, Direction::Reverse);
}

#[test]
fn once_event_intervals_do_not_double_fire_shared_endpoints() {
    let mut authored = sequence();
    authored.events.push(AnimationEvent {
        id: EventId::new("event:footstep"),
        name: "footstep".into(),
        tick: Tick(50),
        payload: None,
    });
    let compiled = authored.compile().unwrap();
    assert_eq!(compiled.events_crossed(Tick(0), Tick(50)).len(), 1);
    assert!(compiled.events_crossed(Tick(50), Tick(100)).is_empty());
    assert_eq!(compiled.events_crossed(Tick(100), Tick(50)).len(), 1);
    assert!(compiled.events_crossed(Tick(50), Tick(0)).is_empty());
}

#[test]
fn loop_and_reverse_event_crossings_are_stable_across_cycles() {
    let mut authored = sequence();
    authored.loop_policy = LoopPolicy::Loop;
    authored.events.push(AnimationEvent {
        id: EventId::new("event:pulse"),
        name: "pulse".into(),
        tick: Tick(20),
        payload: Some(AnimValue::Integer(7)),
    });
    let compiled = authored.compile().unwrap();
    let forward = compiled.events_crossed(Tick(80), Tick(225));
    assert_eq!(
        forward.iter().map(|item| item.raw_tick).collect::<Vec<_>>(),
        vec![Tick(120), Tick(220)]
    );
    assert!(forward
        .iter()
        .all(|item| item.direction == Direction::Forward));
    let reverse = compiled.events_crossed(Tick(225), Tick(80));
    assert_eq!(
        reverse.iter().map(|item| item.raw_tick).collect::<Vec<_>>(),
        vec![Tick(220), Tick(120)]
    );
    assert!(reverse
        .iter()
        .all(|item| item.direction == Direction::Reverse));
    let limited = compiled.events_crossed_limited(Tick(0), Tick(i64::MAX), 3);
    assert_eq!(limited.occurrences.len(), 3);
    assert!(limited.truncated);
}

#[test]
fn ping_pong_events_report_local_leg_direction() {
    let mut authored = sequence();
    authored.loop_policy = LoopPolicy::PingPong;
    authored.events.push(AnimationEvent {
        id: EventId::new("event:contact"),
        name: "contact".into(),
        tick: Tick(30),
        payload: None,
    });
    let compiled = authored.compile().unwrap();
    let occurrences = compiled.events_crossed(Tick(0), Tick(250));
    assert_eq!(
        occurrences
            .iter()
            .map(|item| (item.raw_tick, item.direction))
            .collect::<Vec<_>>(),
        vec![
            (Tick(30), Direction::Forward),
            (Tick(170), Direction::Reverse),
            (Tick(230), Direction::Forward),
        ]
    );
}

#[test]
fn repeated_compile_hash_evaluation_and_binary_round_trip_are_identical() {
    let mut authored = sequence();
    authored.tracks.push(track(
        "track:x",
        "x",
        ValueKind::Number,
        Interpolation::Linear,
        vec![
            key("a", 0, AnimValue::Number(-0.0)),
            key("b", 100, AnimValue::Number(1.0)),
        ],
    ));
    let first = authored.compile().unwrap();
    let second = authored.compile().unwrap();
    assert_eq!(first.stable_hash, second.stable_hash);
    assert_eq!(first.evaluate(Tick(37)), second.evaluate(Tick(37)));

    let encoded = bincode::serialize(&first).unwrap();
    let decoded: CompiledSequence = bincode::deserialize(&encoded).unwrap();
    assert_eq!(first, decoded);
    assert_eq!(first.evaluate(Tick(37)), decoded.evaluate(Tick(37)));

    let json = serde_json::to_string(&authored).unwrap();
    let decoded_authored: Sequence = serde_json::from_str(&json).unwrap();
    assert_eq!(authored, decoded_authored);
}

fn skeleton(hash: &str) -> SkeletonSignature {
    SkeletonSignature {
        signature_hash: hash.into(),
        rest_pose_hash: "rest:1".into(),
        joint_names: vec!["hips".into(), "foot_l".into()],
        parent_indices: vec![None, Some(0)],
        humanoid_profile: Some("metrocalk-humanoid-v1".into()),
    }
}

fn facet() -> AnimationFacet {
    AnimationFacet {
        id: AnimationFacetId::new("facet:motion"),
        capabilities: BTreeSet::from([
            AnimationCapability::TransformTracks,
            AnimationCapability::SkeletalPose,
            AnimationCapability::Events,
        ]),
        clips: vec![ClipSummary {
            clip_id: ClipId::new("clip:walk"),
            sequence_id: SequenceId::new("sequence:walk"),
            name: "Walk".into(),
            duration: Tick(60_000),
            time_base: TimeBase::GAME_60,
            track_count: 12,
            event_count: 2,
            marker_count: 1,
            animated_value_kinds: BTreeSet::from([ValueKind::Vec3, ValueKind::Quaternion]),
        }],
        skeleton_signature: Some(skeleton("rig:hero")),
        skeleton_compatibility: Some(SkeletonCompatibility {
            policy: SkeletonMatchPolicy::NamedSubset,
            required_signature_hash: "rig:hero".into(),
            required_joints: BTreeSet::from(["hips".into(), "foot_l".into()]),
            required_humanoid_profile: None,
            retarget_profile: Some("retarget:hero".into()),
        }),
        root_motion: RootMotionMetadata {
            present: false,
            source_joint: None,
            translation_axes: [true, false, true],
            extracts_yaw: true,
            average_speed_metres_per_second: 1.4,
            in_place_preview_supported: true,
        },
        contacts: vec![ContactMetadata {
            name: "left_foot".into(),
            joint: "foot_l".into(),
            start: Tick(10_000),
            end: Tick(20_000),
            semantic: "ground_contact".into(),
        }],
        events: EventMetadata {
            names: BTreeSet::from(["footstep".into()]),
            payload_kinds: BTreeSet::from([ValueKind::String]),
            interval_crossing_safe: true,
        },
        morphs: None,
        extensions: vec![],
        compression: Some(CompressionMetadata {
            codec: "acl-compatible-v1".into(),
            codec_version: "1".into(),
            source_bytes: 1_000,
            compressed_bytes: 250,
            maximum_translation_error_metres: 0.0005,
            maximum_rotation_error_degrees: 0.05,
            maximum_scale_error: 0.0001,
            measured_sample_count: 120,
        }),
        reimport: ReimportMetadata {
            strategy: "stable-channel-id".into(),
            previous_source_hash: None,
            preserves_authored_overrides: true,
            channel_bindings: BTreeMap::new(),
            orphaned_bindings: BTreeSet::new(),
        },
        determinism: DeterminismMetadata {
            compiler_version: "metrocalk-animation/0.1".into(),
            canonicalization_version: 1,
            compiled_hash: "mtkanim:1234".into(),
            integer_tick_time: true,
            stable_ordering: true,
            deterministic_cpu_evaluation: true,
        },
    }
}

fn animation_manifest() -> AnimationAssetManifest {
    AnimationAssetManifest {
        schema_version: ANIMATION_MANIFEST_VERSION,
        asset_id: AnimationAssetId::new("animation:walk"),
        display_name: "Hero Walk".into(),
        source: SourceIdentity {
            uri: "project://characters/hero/walk.fbx".into(),
            source_hash: "source:abcd".into(),
            importer: "metrocalk-fbx".into(),
            importer_version: "1".into(),
            modified_epoch_ms: None,
        },
        content: ContentIdentity {
            content_hash: "content:abcd".into(),
            canonical_hash: "canonical:abcd".into(),
            byte_size: 250,
        },
        facets: vec![facet()],
    }
}

#[test]
fn self_aware_manifest_explains_capability_and_skeleton_compatibility() {
    let facet = facet();
    let compatible = PlaybackTarget {
        capabilities: facet.capabilities.clone(),
        skeleton: Some(skeleton("different-hash-but-named-subset")),
        maximum_morph_targets: 64,
        extensions: BTreeSet::new(),
    };
    assert!(facet.can_play_on(&compatible));
    assert!(facet.compatibility_diagnostics(&compatible).is_empty());

    let incompatible = PlaybackTarget {
        capabilities: BTreeSet::from([AnimationCapability::TransformTracks]),
        skeleton: None,
        maximum_morph_targets: 0,
        extensions: BTreeSet::new(),
    };
    assert!(!facet.can_play_on(&incompatible));
    assert_eq!(
        facet.missing_capabilities(&incompatible),
        vec![
            AnimationCapability::SkeletalPose,
            AnimationCapability::Events,
        ]
    );
    let codes: BTreeSet<_> = facet
        .compatibility_diagnostics(&incompatible)
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect();
    assert!(codes.contains(&QualityCode::IncompatibleSkeleton));
    assert!(codes.contains(&QualityCode::MissingPlaybackCapability));

    let mut exact = facet.clone();
    exact.skeleton_compatibility = Some(SkeletonCompatibility {
        policy: SkeletonMatchPolicy::Exact,
        required_signature_hash: "rig:hero".into(),
        required_joints: BTreeSet::new(),
        required_humanoid_profile: None,
        retarget_profile: None,
    });
    let mut different_rest = skeleton("rig:hero");
    different_rest.rest_pose_hash = "rest:different".into();
    let target = PlaybackTarget {
        capabilities: exact.capabilities.clone(),
        skeleton: Some(different_rest),
        maximum_morph_targets: 64,
        extensions: BTreeSet::new(),
    };
    assert!(
        !exact.can_play_on(&target),
        "exact skeletal playback includes bind-pose identity"
    );
}

#[test]
fn manifest_quality_and_bincode_round_trip_are_self_describing() {
    let manifest = animation_manifest();
    assert!(manifest.quality_diagnostics().is_empty());
    let bytes = bincode::serialize(&manifest).unwrap();
    let decoded: AnimationAssetManifest = bincode::deserialize(&bytes).unwrap();
    assert_eq!(manifest, decoded);

    let mut degraded = manifest;
    degraded.facets[0]
        .compression
        .as_mut()
        .unwrap()
        .maximum_rotation_error_degrees = 2.0;
    assert!(degraded
        .quality_diagnostics()
        .iter()
        .any(|item| item.code == QualityCode::CompressionErrorHigh));
}

#[test]
fn repository_record_defaults_wrap_a_v1_manifest_without_changing_its_wire_shape() {
    let manifest = animation_manifest();
    let legacy_bytes = bincode::serialize(&manifest).unwrap();
    let decoded_manifest: AnimationAssetManifest = bincode::deserialize(&legacy_bytes).unwrap();
    assert_eq!(
        decoded_manifest, manifest,
        "the existing v1 manifest wire shape is untouched"
    );

    // A source-control-friendly repository can minimally wrap a legacy manifest. Every lifecycle field
    // defaults, while effective identities reuse the stable IDs already present in v1.
    let legacy_record = serde_json::json!({ "manifest": manifest });
    let record: AnimationAssetRecord = serde_json::from_value(legacy_record).unwrap();
    assert_eq!(record.record_schema_version, ANIMATION_ASSET_RECORD_VERSION);
    assert_eq!(record.effective_logical_id().as_str(), "animation:walk");
    assert_eq!(
        record.effective_revision_id().as_str(),
        "revision:canonical:abcd"
    );
    assert_eq!(record.import_status.state, AnimationImportState::Unknown);
    assert!(record.quality_diagnostics().is_empty());
}

#[test]
fn repository_fingerprint_is_stable_across_collection_arrival_order() {
    let mut first = AnimationAssetRecord::from(animation_manifest());
    first.editor.primary_context = AnimationEditorContext::ThreeD;
    first.editor.supported_contexts =
        BTreeSet::from([AnimationEditorContext::ThreeD, AnimationEditorContext::Cad]);
    first.dependencies = vec![
        AnimationAssetDependency {
            id: AnimationDependencyId::new("dependency:rig"),
            asset_id: ReferencedAssetId::new("asset:hero-rig"),
            kind: AnimationDependencyKind::Skeleton,
            requirement: AnimationDependencyRequirement::Required,
            role: "playback_skeleton".into(),
            expected_revision: Some(AnimationRevisionId::new("revision:rig:4")),
            expected_content_hash: Some("rig-content:4".into()),
        },
        AnimationAssetDependency {
            id: AnimationDependencyId::new("dependency:footsteps"),
            asset_id: ReferencedAssetId::new("asset:footsteps"),
            kind: AnimationDependencyKind::Audio,
            requirement: AnimationDependencyRequirement::Optional,
            role: "event_payload".into(),
            expected_revision: None,
            expected_content_hash: None,
        },
    ];
    first.reimport_diagnostics = vec![
        AnimationReimportDiagnostic {
            severity: Severity::Warning,
            code: "orphaned_channel".into(),
            channel_id: Some("channel:hand".into()),
            message: "authored override has no confident source match".into(),
            remediation: "Reassign or retain the previous channel.".into(),
            previous_revision: Some(AnimationRevisionId::new("revision:old")),
            candidate_source_hash: Some("source:new".into()),
        },
        AnimationReimportDiagnostic {
            severity: Severity::Info,
            code: "matched_channel".into(),
            channel_id: Some("channel:root".into()),
            message: "channel matched exactly".into(),
            remediation: "No action required.".into(),
            previous_revision: None,
            candidate_source_hash: Some("source:new".into()),
        },
    ];
    let mut second = first.clone();
    second.dependencies.reverse();
    second.reimport_diagnostics.reverse();
    assert_eq!(
        first.repository_fingerprint(),
        second.repository_fingerprint(),
        "arrival order is not repository identity"
    );
    second.logical_id = AnimationAssetId::new("animation:walk-copy");
    assert_ne!(
        first.repository_fingerprint(),
        second.repository_fingerprint()
    );
}

#[test]
fn lifecycle_validation_is_deterministic_and_blocks_only_fatal_readiness() {
    let mut record = AnimationAssetRecord::from(animation_manifest());
    record.revision.content_hash = "content:wrong".into();
    record.import_status.state = AnimationImportState::Failed;
    record.editor.primary_context = AnimationEditorContext::Ui;
    record.editor.supported_contexts = BTreeSet::from([AnimationEditorContext::ThreeD]);
    record.source.last_observed_hash = Some("source:new".into());
    let self_dependency = AnimationAssetDependency {
        id: AnimationDependencyId::new("dependency:self"),
        asset_id: ReferencedAssetId::new(record.logical_id.as_str()),
        kind: AnimationDependencyKind::Other,
        requirement: AnimationDependencyRequirement::Required,
        role: "cycle".into(),
        expected_revision: None,
        expected_content_hash: None,
    };
    record.dependencies = vec![self_dependency.clone(), self_dependency];
    record
        .reimport_diagnostics
        .push(AnimationReimportDiagnostic {
            severity: Severity::Error,
            code: "binding_conflict".into(),
            channel_id: Some("channel:root".into()),
            message: "two candidates have equal confidence".into(),
            remediation: "Choose the intended target explicitly.".into(),
            previous_revision: None,
            candidate_source_hash: Some("source:new".into()),
        });

    let diagnostics = record.quality_diagnostics();
    let codes: BTreeSet<_> = diagnostics.iter().map(|item| item.code).collect();
    for expected in [
        QualityCode::RevisionContentMismatch,
        QualityCode::DuplicateDependencyIdentity,
        QualityCode::CyclicDependency,
        QualityCode::ImportStateBlocksPlayback,
        QualityCode::EditorContextMismatch,
        QualityCode::SourceIdentityMismatch,
        QualityCode::ReimportDiagnostic,
    ] {
        assert!(
            codes.contains(&expected),
            "missing {expected:?}: {diagnostics:?}"
        );
    }
    assert!(record.has_fatal_readiness_error());
    assert!(diagnostics.windows(2).all(|items| {
        (&items[0].location, items[0].code, &items[0].message)
            <= (&items[1].location, items[1].code, &items[1].message)
    }));
}
