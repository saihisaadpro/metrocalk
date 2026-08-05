#![allow(clippy::float_cmp, clippy::too_many_lines)]

use std::collections::BTreeMap;

use super::*;
use crate::{
    AnimValue, AnimationEvent, Binding, EventId, Interpolation, KeyId, Keyframe, LoopPolicy,
    PropertyPath, Sequence, SequenceId, Tick, Track, TrackId, ValueKind,
};

fn binding(property: &str, kind: ValueKind) -> Binding {
    Binding {
        path: PropertyPath::new("entity:hero", "Transform", property),
        value_kind: kind,
    }
}

fn constant_sequence(
    id: &str,
    values: Vec<(Binding, AnimValue)>,
    event_tick: Option<i64>,
) -> crate::CompiledSequence {
    let mut sequence = Sequence::new(id, id, Tick(100));
    sequence.loop_policy = LoopPolicy::Loop;
    for (index, (binding, value)) in values.into_iter().enumerate() {
        sequence.tracks.push(Track {
            id: TrackId::new(format!("track:{index}")),
            name: format!("Track {index}"),
            binding,
            interpolation: if value.is_interpolatable() {
                Interpolation::Linear
            } else {
                Interpolation::Step
            },
            keyframes: vec![
                Keyframe::new(
                    KeyId::new(format!("key:{index}:0")),
                    Tick::ZERO,
                    value.clone(),
                ),
                Keyframe::new(KeyId::new(format!("key:{index}:1")), Tick(100), value),
            ],
            enabled: true,
        });
    }
    if let Some(tick) = event_tick {
        sequence.events.push(AnimationEvent {
            id: EventId::new(format!("event:{id}")),
            name: id.to_owned(),
            tick: Tick(tick),
            payload: None,
        });
    }
    sequence.compile().unwrap()
}

fn clip_node(id: &str, sequence: &str) -> GraphNode {
    GraphNode {
        id: GraphNodeId::new(id),
        name: id.to_owned(),
        kind: GraphNodeKind::Clip {
            sequence: SequenceId::new(sequence),
            rate: RationalRate::ONE,
            start_tick: Tick::ZERO,
        },
    }
}

fn reference(binding: Binding, value: AnimValue) -> GraphReferenceValue {
    GraphReferenceValue { binding, value }
}

fn catalog(
    sequences: impl IntoIterator<Item = crate::CompiledSequence>,
) -> BTreeMap<SequenceId, crate::CompiledSequence> {
    sequences
        .into_iter()
        .map(|sequence| (sequence.id.clone(), sequence))
        .collect()
}

fn number(frame: &AnimationGraphFrame, property: &str) -> f64 {
    frame
        .values
        .iter()
        .find(|value| value.binding.path.property == property)
        .and_then(|value| match value.value {
            AnimValue::Number(value) => Some(value),
            _ => None,
        })
        .unwrap()
}

#[test]
fn reference_pose_node_is_a_complete_external_source_free_graph() {
    let x = binding("x", ValueKind::Number);
    let graph = AnimationGraph::flat(
        "graph:reference",
        "Reference",
        GraphNodeId::new("node:reference"),
        vec![reference(x, AnimValue::Number(42.0))],
        vec![GraphNode {
            id: GraphNodeId::new("node:reference"),
            name: "Reference".into(),
            kind: GraphNodeKind::ReferencePose,
        }],
    );
    let plan = CompiledAnimationGraph::compile(&graph, &BTreeMap::new()).unwrap();
    assert!(plan.referenced_sequence_ids().is_empty());
    assert_eq!(plan.binding_signatures().len(), 1);
    let runtime = AnimationGraphRuntimeInstance::new(plan);
    assert_eq!(number(runtime.frame(), "x"), 42.0);
}

#[test]
fn canonical_compile_is_reorder_deterministic_and_authored_schema_round_trips() {
    let x = binding("x", ValueKind::Number);
    let source_a = constant_sequence(
        "sequence:a",
        vec![(x.clone(), AnimValue::Number(1.0))],
        None,
    );
    let source_b = constant_sequence(
        "sequence:b",
        vec![(x.clone(), AnimValue::Number(3.0))],
        None,
    );
    let sources = catalog([source_a, source_b]);
    let blend = GraphNode {
        id: GraphNodeId::new("node:blend"),
        name: "Blend".into(),
        kind: GraphNodeKind::Blend {
            mode: GraphBlendMode::Normalized,
            inputs: vec![
                GraphBlendInput {
                    node: GraphNodeId::new("node:b"),
                    weight: GraphWeight::Constant(1.0),
                },
                GraphBlendInput {
                    node: GraphNodeId::new("node:a"),
                    weight: GraphWeight::Constant(1.0),
                },
            ],
        },
    };
    let mut graph = AnimationGraph::flat(
        "graph:stable",
        "Stable",
        blend.id.clone(),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![
            clip_node("node:b", "sequence:b"),
            blend,
            clip_node("node:a", "sequence:a"),
        ],
    );
    let first = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    graph.nodes.reverse();
    if let GraphNodeKind::Blend { inputs, .. } = &mut graph.nodes[1].kind {
        inputs.reverse();
    }
    let second = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    assert_eq!(first.stable_hash, second.stable_hash);
    assert_eq!(first.schedule(), second.schedule());
    assert_eq!(
        first.referenced_sequence_ids(),
        vec![SequenceId::new("sequence:a"), SequenceId::new("sequence:b")]
    );

    let json = serde_json::to_string(&graph).unwrap();
    let decoded: AnimationGraph = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, graph);
    assert_eq!(decoded.schema_version, ANIMATION_GRAPH_SCHEMA_VERSION);

    let mut legacy = serde_json::to_value(&graph).unwrap();
    legacy["limits"]
        .as_object_mut()
        .unwrap()
        .remove("max_pose_edges");
    let decoded: AnimationGraph = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.limits.max_pose_edges, 2_048);

    let mut old_schema = graph.clone();
    old_schema.schema_version = 1;
    let error = CompiledAnimationGraph::compile(&old_schema, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::UnsupportedSchemaVersion
    }));

    graph.limits.max_pose_edges = 1;
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::LimitExceeded
            && diagnostic.location == "graph.pose_edges"
    }));
}

#[test]
fn compilation_fails_closed_for_missing_reference_sequence_cycles_and_bad_types() {
    let x = binding("x", ValueKind::Number);
    let source = constant_sequence("sequence:a", vec![(x, AnimValue::Number(1.0))], None);
    let mut graph = AnimationGraph::flat(
        "graph:invalid",
        "Invalid",
        GraphNodeId::new("node:a"),
        Vec::new(),
        vec![clip_node("node:a", "sequence:a")],
    );
    let error = CompiledAnimationGraph::compile(&graph, &catalog([source])).unwrap_err();
    assert!(error
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == AnimationGraphIssueCode::MissingReferenceBinding));

    graph.reference_pose = vec![reference(
        binding("x", ValueKind::Number),
        AnimValue::Number(0.0),
    )];
    if let GraphNodeKind::Clip { sequence, .. } = &mut graph.nodes[0].kind {
        *sequence = SequenceId::new("sequence:missing");
    }
    let error = CompiledAnimationGraph::compile(&graph, &BTreeMap::new()).unwrap_err();
    assert!(error
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == AnimationGraphIssueCode::MissingSequence));

    graph.nodes = vec![
        GraphNode {
            id: GraphNodeId::new("node:a"),
            name: "A".into(),
            kind: GraphNodeKind::Blend {
                mode: GraphBlendMode::Normalized,
                inputs: vec![GraphBlendInput {
                    node: GraphNodeId::new("node:b"),
                    weight: GraphWeight::Constant(1.0),
                }],
            },
        },
        GraphNode {
            id: GraphNodeId::new("node:b"),
            name: "B".into(),
            kind: GraphNodeKind::Blend {
                mode: GraphBlendMode::Normalized,
                inputs: vec![GraphBlendInput {
                    node: GraphNodeId::new("node:a"),
                    weight: GraphWeight::Constant(1.0),
                }],
            },
        },
    ];
    let error = CompiledAnimationGraph::compile(&graph, &BTreeMap::new()).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| diagnostic.code
        == AnimationGraphIssueCode::PoseCycle
        && diagnostic.node_id.is_some()));
}

#[test]
fn normalized_and_direct_blends_handle_quaternions_and_discrete_ties_deterministically() {
    let x = binding("x", ValueKind::Number);
    let rotation = binding("rotation", ValueKind::Quaternion);
    let visible = binding("visible", ValueKind::Boolean);
    let a = constant_sequence(
        "sequence:a",
        vec![
            (x.clone(), AnimValue::Number(0.0)),
            (
                rotation.clone(),
                AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0]),
            ),
            (visible.clone(), AnimValue::Boolean(false)),
        ],
        None,
    );
    let b = constant_sequence(
        "sequence:b",
        vec![
            (x.clone(), AnimValue::Number(10.0)),
            (
                rotation.clone(),
                AnimValue::Quaternion([0.0, 0.0, 1.0, 0.0]),
            ),
            (visible.clone(), AnimValue::Boolean(true)),
        ],
        None,
    );
    let blend_id = GraphNodeId::new("node:blend");
    let mut graph = AnimationGraph::flat(
        "graph:blend",
        "Blend",
        blend_id.clone(),
        vec![
            reference(x, AnimValue::Number(0.0)),
            reference(rotation, AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0])),
            reference(visible, AnimValue::Boolean(false)),
        ],
        vec![
            clip_node("node:a", "sequence:a"),
            clip_node("node:b", "sequence:b"),
            GraphNode {
                id: blend_id,
                name: "Blend".into(),
                kind: GraphNodeKind::Blend {
                    mode: GraphBlendMode::Normalized,
                    inputs: vec![
                        GraphBlendInput {
                            node: GraphNodeId::new("node:a"),
                            weight: GraphWeight::Constant(1.0),
                        },
                        GraphBlendInput {
                            node: GraphNodeId::new("node:b"),
                            weight: GraphWeight::Constant(1.0),
                        },
                    ],
                },
            },
        ],
    );
    let sources = catalog([a, b]);
    let plan = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    let runtime = AnimationGraphRuntimeInstance::new(plan);
    assert_eq!(number(runtime.frame(), "x"), 5.0);
    let rotation = runtime
        .frame()
        .values
        .iter()
        .find(|value| value.binding.path.property == "rotation")
        .unwrap();
    let AnimValue::Quaternion(rotation) = rotation.value else {
        panic!()
    };
    assert!((rotation[2].abs() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1.0e-9);
    let visible = runtime
        .frame()
        .values
        .iter()
        .find(|value| value.binding.path.property == "visible")
        .unwrap();
    assert_eq!(visible.value, AnimValue::Boolean(false));

    let GraphNodeKind::Blend { mode, inputs } = &mut graph.nodes[2].kind else {
        panic!()
    };
    *mode = GraphBlendMode::Direct;
    inputs[0].weight = GraphWeight::Constant(0.0);
    inputs[1].weight = GraphWeight::Constant(0.25);
    let runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    assert_eq!(number(runtime.frame(), "x"), 2.5);
}

#[test]
fn input_trace_aggregates_shared_dag_direct_and_layer_contributions() {
    let x = binding("x", ValueKind::Number);
    let sources = catalog([
        constant_sequence(
            "sequence:a",
            vec![(x.clone(), AnimValue::Number(2.0))],
            None,
        ),
        constant_sequence(
            "sequence:b",
            vec![(x.clone(), AnimValue::Number(4.0))],
            None,
        ),
    ]);
    let graph = AnimationGraph::flat(
        "graph:input-trace",
        "Input trace",
        GraphNodeId::new("node:output"),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![
            clip_node("node:a", "sequence:a"),
            clip_node("node:b", "sequence:b"),
            GraphNode {
                id: GraphNodeId::new("node:shared"),
                name: "Shared direct".into(),
                kind: GraphNodeKind::Blend {
                    mode: GraphBlendMode::Direct,
                    inputs: vec![
                        GraphBlendInput {
                            node: GraphNodeId::new("node:a"),
                            weight: GraphWeight::Constant(0.5),
                        },
                        GraphBlendInput {
                            node: GraphNodeId::new("node:b"),
                            weight: GraphWeight::Constant(0.25),
                        },
                    ],
                },
            },
            GraphNode {
                id: GraphNodeId::new("node:output"),
                name: "Shared additive layer".into(),
                kind: GraphNodeKind::AdditiveLayer {
                    base: GraphNodeId::new("node:shared"),
                    layer: GraphNodeId::new("node:shared"),
                    weight: GraphWeight::Constant(0.2),
                    mask: None,
                },
            },
        ],
    );
    let runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );

    let trace: Vec<_> = runtime
        .input_trace()
        .iter()
        .map(|entry| {
            (
                entry.from.to_string(),
                entry.to.to_string(),
                entry.contribution_weight,
            )
        })
        .collect();
    assert_eq!(
        trace,
        vec![
            ("node:a".into(), "node:shared".into(), 0.6),
            ("node:b".into(), "node:shared".into(), 0.3),
            ("node:shared".into(), "node:output".into(), 1.2),
        ]
    );
    assert!(!runtime.input_trace_truncated());
}

#[test]
fn input_trace_propagates_deep_shared_diamonds_in_linear_schedule_order() {
    const LEVELS: usize = 64;

    let x = binding("x", ValueKind::Number);
    let sources = catalog([constant_sequence(
        "sequence:source",
        vec![(x.clone(), AnimValue::Number(1.0))],
        None,
    )]);
    let mut nodes = vec![clip_node("node:source", "sequence:source")];
    let mut previous = GraphNodeId::new("node:source");
    for level in 0..LEVELS {
        let left = GraphNodeId::new(format!("node:diamond:{level:02}:left"));
        let right = GraphNodeId::new(format!("node:diamond:{level:02}:right"));
        let merge = GraphNodeId::new(format!("node:diamond:{level:02}:merge"));
        for branch in [&left, &right] {
            nodes.push(GraphNode {
                id: branch.clone(),
                name: branch.to_string(),
                kind: GraphNodeKind::Blend {
                    mode: GraphBlendMode::Direct,
                    inputs: vec![GraphBlendInput {
                        node: previous.clone(),
                        weight: GraphWeight::Constant(1.0),
                    }],
                },
            });
        }
        nodes.push(GraphNode {
            id: merge.clone(),
            name: merge.to_string(),
            kind: GraphNodeKind::Blend {
                mode: GraphBlendMode::Normalized,
                inputs: vec![
                    GraphBlendInput {
                        node: left,
                        weight: GraphWeight::Constant(1.0),
                    },
                    GraphBlendInput {
                        node: right,
                        weight: GraphWeight::Constant(1.0),
                    },
                ],
            },
        });
        previous = merge;
    }
    let graph = AnimationGraph::flat(
        "graph:diamond-stress",
        "Diamond stress",
        previous,
        vec![reference(x, AnimValue::Number(0.0))],
        nodes,
    );
    let runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );

    assert_eq!(runtime.input_trace().len(), LEVELS * 4);
    assert!(!runtime.input_trace_truncated());
    assert!(runtime
        .input_trace()
        .iter()
        .all(|edge| (edge.contribution_weight - 0.5).abs() < 1.0e-12));
    assert!(runtime
        .input_trace()
        .windows(2)
        .all(|pair| { (&pair[0].from, &pair[0].to) < (&pair[1].from, &pair[1].to) }));
    for node in runtime.node_trace() {
        let expected = if node.node_id.as_str().ends_with(":left")
            || node.node_id.as_str().ends_with(":right")
        {
            0.5
        } else {
            1.0
        };
        assert!((node.contribution_weight - expected).abs() < 1.0e-12);
    }
}

#[test]
fn one_and_two_dimensional_blend_spaces_clamp_and_project_deterministically() {
    let x = binding("x", ValueKind::Number);
    let sources = catalog([
        constant_sequence(
            "sequence:zero",
            vec![(x.clone(), AnimValue::Number(0.0))],
            None,
        ),
        constant_sequence(
            "sequence:ten",
            vec![(x.clone(), AnimValue::Number(10.0))],
            None,
        ),
        constant_sequence(
            "sequence:twenty",
            vec![(x.clone(), AnimValue::Number(20.0))],
            None,
        ),
    ]);
    let speed = GraphParameterId::new("parameter:speed");
    let mut graph = AnimationGraph::flat(
        "graph:spaces",
        "Spaces",
        GraphNodeId::new("node:1d"),
        vec![reference(x.clone(), AnimValue::Number(0.0))],
        vec![
            clip_node("node:zero", "sequence:zero"),
            clip_node("node:ten", "sequence:ten"),
            clip_node("node:twenty", "sequence:twenty"),
            GraphNode {
                id: GraphNodeId::new("node:1d"),
                name: "1D".into(),
                kind: GraphNodeKind::Blend1d {
                    parameter: speed.clone(),
                    points: vec![
                        GraphBlend1dPoint {
                            id: GraphBlendPointId::new("point:10"),
                            position: 10.0,
                            node: GraphNodeId::new("node:ten"),
                        },
                        GraphBlend1dPoint {
                            id: GraphBlendPointId::new("point:0"),
                            position: 0.0,
                            node: GraphNodeId::new("node:zero"),
                        },
                    ],
                },
            },
        ],
    );
    graph.parameters.push(GraphParameter {
        id: speed.clone(),
        name: "Speed".into(),
        kind: GraphParameterKind::Number,
        default: GraphParameterValue::Number(5.0),
    });
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    assert_eq!(number(runtime.frame(), "x"), 5.0);
    runtime
        .set_parameter(&speed, GraphParameterValue::Number(50.0))
        .unwrap();
    assert_eq!(number(runtime.frame(), "x"), 10.0);

    let px = GraphParameterId::new("parameter:x");
    let py = GraphParameterId::new("parameter:y");
    graph.parameters = vec![
        GraphParameter {
            id: speed,
            name: "Speed".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(5.0),
        },
        GraphParameter {
            id: px.clone(),
            name: "X".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.25),
        },
        GraphParameter {
            id: py.clone(),
            name: "Y".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.25),
        },
    ];
    graph.output = GraphNodeId::new("node:2d");
    graph.nodes.push(GraphNode {
        id: graph.output.clone(),
        name: "2D".into(),
        kind: GraphNodeKind::Blend2d {
            parameter_x: px.clone(),
            parameter_y: py.clone(),
            points: vec![
                GraphBlend2dPoint {
                    id: GraphBlendPointId::new("point:a"),
                    position: [0.0, 0.0],
                    node: GraphNodeId::new("node:zero"),
                },
                GraphBlend2dPoint {
                    id: GraphBlendPointId::new("point:b"),
                    position: [1.0, 0.0],
                    node: GraphNodeId::new("node:ten"),
                },
                GraphBlend2dPoint {
                    id: GraphBlendPointId::new("point:c"),
                    position: [0.0, 1.0],
                    node: GraphNodeId::new("node:twenty"),
                },
            ],
            triangles: vec![GraphBlend2dTriangle {
                points: [
                    GraphBlendPointId::new("point:a"),
                    GraphBlendPointId::new("point:b"),
                    GraphBlendPointId::new("point:c"),
                ],
            }],
            outside_hull: GraphOutsideHullMode::ProjectToHull,
        },
    });
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    assert!((number(runtime.frame(), "x") - 7.5).abs() < 1.0e-9);
    runtime
        .set_parameter(&px, GraphParameterValue::Number(2.0))
        .unwrap();
    runtime
        .set_parameter(&py, GraphParameterValue::Number(0.0))
        .unwrap();
    assert_eq!(number(runtime.frame(), "x"), 10.0);
}

#[test]
fn cartesian_blend_rejects_extreme_geometry_and_clamps_extreme_runtime_queries() {
    let x = binding("x", ValueKind::Number);
    let sources = catalog([constant_sequence(
        "sequence:sample",
        vec![(x.clone(), AnimValue::Number(1.0))],
        None,
    )]);
    let parameter_x = GraphParameterId::new("parameter:x");
    let parameter_y = GraphParameterId::new("parameter:y");
    let mut graph = AnimationGraph::flat(
        "graph:bounded-2d",
        "Bounded 2D",
        GraphNodeId::new("node:2d"),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![
            clip_node("node:sample", "sequence:sample"),
            GraphNode {
                id: GraphNodeId::new("node:2d"),
                name: "2D".into(),
                kind: GraphNodeKind::Blend2d {
                    parameter_x: parameter_x.clone(),
                    parameter_y: parameter_y.clone(),
                    points: [("a", [0.0, 0.0]), ("b", [1.0, 0.0]), ("c", [0.0, 1.0])]
                        .map(|(id, position)| GraphBlend2dPoint {
                            id: GraphBlendPointId::new(format!("point:{id}")),
                            position,
                            node: GraphNodeId::new("node:sample"),
                        })
                        .to_vec(),
                    triangles: vec![GraphBlend2dTriangle {
                        points: ["a", "b", "c"]
                            .map(|id| GraphBlendPointId::new(format!("point:{id}"))),
                    }],
                    outside_hull: GraphOutsideHullMode::ProjectToHull,
                },
            },
        ],
    );
    graph.parameters = vec![
        GraphParameter {
            id: parameter_x.clone(),
            name: "X".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.25),
        },
        GraphParameter {
            id: parameter_y.clone(),
            name: "Y".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.25),
        },
    ];

    let mut invalid = graph.clone();
    let GraphNodeKind::Blend2d { points, .. } = &mut invalid.nodes[1].kind else {
        panic!()
    };
    points[2].position = [f64::MAX, 1.0];
    let error = CompiledAnimationGraph::compile(&invalid, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidBlendSpace
            && diagnostic.message.contains("absolute geometry limit")
    }));

    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    runtime
        .set_parameter(&parameter_x, GraphParameterValue::Number(f64::MAX))
        .unwrap();
    runtime
        .set_parameter(&parameter_y, GraphParameterValue::Number(-f64::MAX))
        .unwrap();
    assert!(number(runtime.frame(), "x").is_finite());
    assert!(runtime
        .input_trace()
        .iter()
        .all(|edge| edge.contribution_weight.is_finite()));
}

#[test]
fn cartesian_blend_mesh_rejects_duplicate_malformed_overlap_and_disconnected_data() {
    let x = binding("x", ValueKind::Number);
    let sources = catalog([constant_sequence(
        "sequence:sample",
        vec![(x.clone(), AnimValue::Number(1.0))],
        None,
    )]);
    let parameter_x = GraphParameterId::new("parameter:x");
    let parameter_y = GraphParameterId::new("parameter:y");
    let mut graph = AnimationGraph::flat(
        "graph:mesh-admission",
        "Mesh admission",
        GraphNodeId::new("node:2d"),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![
            clip_node("node:sample", "sequence:sample"),
            GraphNode {
                id: GraphNodeId::new("node:2d"),
                name: "2D".into(),
                kind: GraphNodeKind::Blend2d {
                    parameter_x: parameter_x.clone(),
                    parameter_y: parameter_y.clone(),
                    points: vec![
                        GraphBlend2dPoint {
                            id: GraphBlendPointId::new("point:a"),
                            position: [0.0, 0.0],
                            node: GraphNodeId::new("node:sample"),
                        },
                        GraphBlend2dPoint {
                            id: GraphBlendPointId::new("point:b"),
                            position: [1.0, 0.0],
                            node: GraphNodeId::new("node:sample"),
                        },
                        GraphBlend2dPoint {
                            id: GraphBlendPointId::new("point:c"),
                            position: [1.0, 0.0],
                            node: GraphNodeId::new("node:sample"),
                        },
                    ],
                    triangles: vec![GraphBlend2dTriangle {
                        points: [
                            GraphBlendPointId::new("point:a"),
                            GraphBlendPointId::new("point:b"),
                            GraphBlendPointId::new("point:c"),
                        ],
                    }],
                    outside_hull: GraphOutsideHullMode::ProjectToHull,
                },
            },
        ],
    );
    graph.parameters = vec![
        GraphParameter {
            id: parameter_x,
            name: "X".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.0),
        },
        GraphParameter {
            id: parameter_y,
            name: "Y".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.0),
        },
    ];

    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidBlendSpace
            && diagnostic.message.contains("same Cartesian coordinate")
    }));

    {
        let GraphNodeKind::Blend2d {
            points, triangles, ..
        } = &mut graph.nodes[1].kind
        else {
            panic!()
        };
        points[2].position = [0.0, 1.0];
        triangles[0].points = [
            GraphBlendPointId::new("point:a"),
            GraphBlendPointId::new("point:a"),
            GraphBlendPointId::new("point:c"),
        ];
    }
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidTriangle
            && diagnostic.message.contains("repeats")
    }));

    {
        let GraphNodeKind::Blend2d { triangles, .. } = &mut graph.nodes[1].kind else {
            panic!()
        };
        triangles[0].points = [
            GraphBlendPointId::new("point:a"),
            GraphBlendPointId::new("point:b"),
            GraphBlendPointId::new("point:c"),
        ];
        let duplicate = triangles[0].clone();
        triangles.push(duplicate);
    }
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidTriangle
            && diagnostic.message.contains("more than once")
    }));

    {
        let GraphNodeKind::Blend2d {
            points, triangles, ..
        } = &mut graph.nodes[1].kind
        else {
            panic!()
        };
        triangles.truncate(1);
        points.push(GraphBlend2dPoint {
            id: GraphBlendPointId::new("point:d"),
            position: [1.0, 1.0],
            node: GraphNodeId::new("node:sample"),
        });
        triangles.push(GraphBlend2dTriangle {
            points: [
                GraphBlendPointId::new("point:a"),
                GraphBlendPointId::new("point:b"),
                GraphBlendPointId::new("point:d"),
            ],
        });
    }
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidTriangle
            && diagnostic.message.contains("overlap")
    }));

    {
        let GraphNodeKind::Blend2d {
            points, triangles, ..
        } = &mut graph.nodes[1].kind
        else {
            panic!()
        };
        points.push(GraphBlend2dPoint {
            id: GraphBlendPointId::new("point:h"),
            position: [0.5, -1.0],
            node: GraphNodeId::new("node:sample"),
        });
        triangles.push(GraphBlend2dTriangle {
            points: [
                GraphBlendPointId::new("point:a"),
                GraphBlendPointId::new("point:b"),
                GraphBlendPointId::new("point:h"),
            ],
        });
    }
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidTriangle
            && diagnostic.message.contains("non-manifold")
    }));

    {
        let GraphNodeKind::Blend2d {
            points, triangles, ..
        } = &mut graph.nodes[1].kind
        else {
            panic!()
        };
        points.extend([
            GraphBlend2dPoint {
                id: GraphBlendPointId::new("point:e"),
                position: [3.0, 0.0],
                node: GraphNodeId::new("node:sample"),
            },
            GraphBlend2dPoint {
                id: GraphBlendPointId::new("point:f"),
                position: [4.0, 0.0],
                node: GraphNodeId::new("node:sample"),
            },
            GraphBlend2dPoint {
                id: GraphBlendPointId::new("point:g"),
                position: [3.0, 1.0],
                node: GraphNodeId::new("node:sample"),
            },
        ]);
        triangles.truncate(1);
        triangles.push(GraphBlend2dTriangle {
            points: [
                GraphBlendPointId::new("point:e"),
                GraphBlendPointId::new("point:f"),
                GraphBlendPointId::new("point:g"),
            ],
        });
    }
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidBlendSpace
            && diagnostic.message.contains("edge-connected")
    }));
}

#[test]
fn concave_cartesian_projection_uses_only_stable_boundary_edges() {
    let x = binding("x", ValueKind::Number);
    let samples = [
        ("a", [0.0, 0.0], 0.0),
        ("b", [3.0, 0.0], 30.0),
        ("c", [3.0, 1.0], 31.0),
        ("d", [1.0, 1.0], 11.0),
        ("e", [1.0, 3.0], 13.0),
        ("f", [0.0, 3.0], 3.0),
    ];
    let sources = catalog(samples.iter().map(|(id, _, value)| {
        constant_sequence(
            &format!("sequence:{id}"),
            vec![(x.clone(), AnimValue::Number(*value))],
            None,
        )
    }));
    let parameter_x = GraphParameterId::new("parameter:x");
    let parameter_y = GraphParameterId::new("parameter:y");
    let mut nodes: Vec<_> = samples
        .iter()
        .map(|(id, _, _)| clip_node(&format!("node:{id}"), &format!("sequence:{id}")))
        .collect();
    nodes.push(GraphNode {
        id: GraphNodeId::new("node:2d"),
        name: "Concave 2D".into(),
        kind: GraphNodeKind::Blend2d {
            parameter_x: parameter_x.clone(),
            parameter_y: parameter_y.clone(),
            points: samples
                .iter()
                .map(|(id, position, _)| GraphBlend2dPoint {
                    id: GraphBlendPointId::new(format!("point:{id}")),
                    position: *position,
                    node: GraphNodeId::new(format!("node:{id}")),
                })
                .collect(),
            triangles: [
                ["a", "b", "d"],
                ["b", "c", "d"],
                ["a", "d", "f"],
                ["d", "e", "f"],
            ]
            .map(|ids| GraphBlend2dTriangle {
                points: ids.map(|id| GraphBlendPointId::new(format!("point:{id}"))),
            })
            .to_vec(),
            outside_hull: GraphOutsideHullMode::ProjectToHull,
        },
    });
    let mut graph = AnimationGraph::flat(
        "graph:concave",
        "Concave",
        GraphNodeId::new("node:2d"),
        vec![reference(x, AnimValue::Number(0.0))],
        nodes,
    );
    graph.parameters = vec![
        GraphParameter {
            id: parameter_x,
            name: "X".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(2.0),
        },
        GraphParameter {
            id: parameter_y,
            name: "Y".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(2.0),
        },
    ];

    let plan = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    let blend = plan
        .nodes
        .iter()
        .find(|node| node.id == GraphNodeId::new("node:2d"))
        .unwrap();
    let CompiledNodeKind::Blend2d {
        points,
        boundary_edges,
        ..
    } = &blend.kind
    else {
        panic!()
    };
    let boundary_ids: Vec<_> = boundary_edges
        .iter()
        .map(|[left, right]| (points[*left].id.clone(), points[*right].id.clone()))
        .collect();
    assert!(!boundary_ids.contains(&(
        GraphBlendPointId::new("point:a"),
        GraphBlendPointId::new("point:d")
    )));
    assert!(!boundary_ids.contains(&(
        GraphBlendPointId::new("point:b"),
        GraphBlendPointId::new("point:d")
    )));
    assert_eq!(
        number(AnimationGraphRuntimeInstance::new(plan).frame(), "x"),
        21.0
    );

    let mut reordered = graph.clone();
    reordered.nodes.reverse();
    let GraphNodeKind::Blend2d {
        points, triangles, ..
    } = &mut reordered
        .nodes
        .iter_mut()
        .find(|node| node.id == GraphNodeId::new("node:2d"))
        .unwrap()
        .kind
    else {
        panic!()
    };
    points.reverse();
    triangles.reverse();
    let reordered_plan = CompiledAnimationGraph::compile(&reordered, &sources).unwrap();
    assert_eq!(
        number(
            AnimationGraphRuntimeInstance::new(reordered_plan).frame(),
            "x"
        ),
        21.0
    );
}

#[test]
fn override_masks_and_additive_number_rotation_use_explicit_reference_delta() {
    let x = binding("x", ValueKind::Number);
    let y = binding("y", ValueKind::Number);
    let rotation = binding("rotation", ValueKind::Quaternion);
    let base = constant_sequence(
        "sequence:base",
        vec![
            (x.clone(), AnimValue::Number(10.0)),
            (y.clone(), AnimValue::Number(10.0)),
            (
                rotation.clone(),
                AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0]),
            ),
        ],
        None,
    );
    let layer = constant_sequence(
        "sequence:layer",
        vec![
            (x.clone(), AnimValue::Number(4.0)),
            (y.clone(), AnimValue::Number(4.0)),
            (
                rotation.clone(),
                AnimValue::Quaternion([0.0, 0.0, 1.0, 0.0]),
            ),
        ],
        None,
    );
    let sources = catalog([base, layer]);
    let mut graph = AnimationGraph::flat(
        "graph:layers",
        "Layers",
        GraphNodeId::new("node:override"),
        vec![
            reference(x.clone(), AnimValue::Number(2.0)),
            reference(y.clone(), AnimValue::Number(2.0)),
            reference(rotation, AnimValue::Quaternion([0.0, 0.0, 0.0, 1.0])),
        ],
        vec![
            clip_node("node:base", "sequence:base"),
            clip_node("node:layer", "sequence:layer"),
            GraphNode {
                id: GraphNodeId::new("node:override"),
                name: "Override".into(),
                kind: GraphNodeKind::OverrideLayer {
                    base: GraphNodeId::new("node:base"),
                    layer: GraphNodeId::new("node:layer"),
                    weight: GraphWeight::Constant(1.0),
                    mask: Some(GraphMaskId::new("mask:x")),
                },
            },
        ],
    );
    graph.masks.push(GraphPropertyMask {
        id: GraphMaskId::new("mask:x"),
        name: "X only".into(),
        default_weight: 0.0,
        entries: vec![GraphMaskEntry {
            path: x.path.clone(),
            weight: 1.0,
        }],
    });
    let runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    assert_eq!(number(runtime.frame(), "x"), 4.0);
    assert_eq!(number(runtime.frame(), "y"), 10.0);

    graph.output = GraphNodeId::new("node:additive");
    graph.nodes.push(GraphNode {
        id: graph.output.clone(),
        name: "Additive".into(),
        kind: GraphNodeKind::AdditiveLayer {
            base: GraphNodeId::new("node:base"),
            layer: GraphNodeId::new("node:layer"),
            weight: GraphWeight::Constant(0.5),
            mask: None,
        },
    });
    let runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    assert_eq!(number(runtime.frame(), "x"), 11.0);
    let value = runtime
        .frame()
        .values
        .iter()
        .find(|value| value.binding.path.property == "rotation")
        .unwrap();
    let AnimValue::Quaternion(rotation) = value.value else {
        panic!()
    };
    assert!((rotation[2].abs() - std::f64::consts::FRAC_1_SQRT_2).abs() < 1.0e-9);
}

#[test]
fn additive_validation_ignores_zero_masked_discrete_bindings_only() {
    let x = binding("x", ValueKind::Number);
    let visible = binding("visible", ValueKind::Boolean);
    let sources = catalog([
        constant_sequence(
            "sequence:base",
            vec![
                (x.clone(), AnimValue::Number(10.0)),
                (visible.clone(), AnimValue::Boolean(true)),
            ],
            None,
        ),
        constant_sequence(
            "sequence:layer",
            vec![
                (x.clone(), AnimValue::Number(4.0)),
                (visible.clone(), AnimValue::Boolean(false)),
            ],
            None,
        ),
    ]);
    let mut graph = AnimationGraph::flat(
        "graph:masked-additive",
        "Masked additive",
        GraphNodeId::new("node:additive"),
        vec![
            reference(x.clone(), AnimValue::Number(2.0)),
            reference(visible.clone(), AnimValue::Boolean(false)),
        ],
        vec![
            clip_node("node:base", "sequence:base"),
            clip_node("node:layer", "sequence:layer"),
            GraphNode {
                id: GraphNodeId::new("node:additive"),
                name: "Additive".into(),
                kind: GraphNodeKind::AdditiveLayer {
                    base: GraphNodeId::new("node:base"),
                    layer: GraphNodeId::new("node:layer"),
                    weight: GraphWeight::Constant(0.5),
                    mask: Some(GraphMaskId::new("mask:numeric")),
                },
            },
        ],
    );
    graph.masks.push(GraphPropertyMask {
        id: GraphMaskId::new("mask:numeric"),
        name: "Numeric only".into(),
        default_weight: 0.0,
        entries: vec![GraphMaskEntry {
            path: x.path.clone(),
            weight: 1.0,
        }],
    });

    let runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    assert_eq!(number(runtime.frame(), "x"), 11.0);
    assert!(runtime.frame().values.iter().any(|value| {
        value.binding.path == visible.path && value.value == AnimValue::Boolean(true)
    }));

    graph.masks[0].entries.push(GraphMaskEntry {
        path: visible.path,
        weight: 1.0,
    });
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.code == AnimationGraphIssueCode::UnsupportedAdditiveType }));
}

fn state_graph() -> (
    AnimationGraph,
    BTreeMap<SequenceId, crate::CompiledSequence>,
) {
    let x = binding("x", ValueKind::Number);
    let sources = catalog([
        constant_sequence(
            "sequence:a",
            vec![(x.clone(), AnimValue::Number(0.0))],
            None,
        ),
        constant_sequence(
            "sequence:b",
            vec![(x.clone(), AnimValue::Number(10.0))],
            None,
        ),
        constant_sequence(
            "sequence:c",
            vec![(x.clone(), AnimValue::Number(20.0))],
            None,
        ),
    ]);
    let go = GraphParameterId::new("trigger:go");
    let interrupt = GraphParameterId::new("trigger:interrupt");
    let mut graph = AnimationGraph::flat(
        "graph:state",
        "State",
        GraphNodeId::new("node:a"),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![
            clip_node("node:a", "sequence:a"),
            clip_node("node:b", "sequence:b"),
            clip_node("node:c", "sequence:c"),
        ],
    );
    graph.parameters = vec![
        GraphParameter {
            id: go.clone(),
            name: "Go".into(),
            kind: GraphParameterKind::Trigger,
            default: GraphParameterValue::Trigger(false),
        },
        GraphParameter {
            id: interrupt.clone(),
            name: "Interrupt".into(),
            kind: GraphParameterKind::Trigger,
            default: GraphParameterValue::Trigger(false),
        },
    ];
    graph.state_machine = Some(GraphStateMachine {
        initial_state: GraphStateId::new("state:a"),
        states: vec![
            GraphState {
                id: GraphStateId::new("state:c"),
                name: "C".into(),
                node: GraphNodeId::new("node:c"),
                reset_on_entry: true,
            },
            GraphState {
                id: GraphStateId::new("state:a"),
                name: "A".into(),
                node: GraphNodeId::new("node:a"),
                reset_on_entry: true,
            },
            GraphState {
                id: GraphStateId::new("state:b"),
                name: "B".into(),
                node: GraphNodeId::new("node:b"),
                reset_on_entry: true,
            },
        ],
        transitions: vec![
            GraphTransition {
                id: GraphTransitionId::new("transition:a-b"),
                from: GraphStateId::new("state:a"),
                to: GraphStateId::new("state:b"),
                priority: 10,
                duration: Tick(10),
                curve: GraphTransitionCurve::Linear,
                interruption: GraphInterruptionPolicy::Destination,
                mode: GraphTransitionMode::Crossfade,
                conditions: vec![GraphCondition {
                    parameter: go,
                    test: GraphConditionTest::Triggered,
                }],
            },
            GraphTransition {
                id: GraphTransitionId::new("transition:b-c"),
                from: GraphStateId::new("state:b"),
                to: GraphStateId::new("state:c"),
                priority: 10,
                duration: Tick(10),
                curve: GraphTransitionCurve::Linear,
                interruption: GraphInterruptionPolicy::None,
                mode: GraphTransitionMode::Crossfade,
                conditions: vec![GraphCondition {
                    parameter: interrupt,
                    test: GraphConditionTest::Triggered,
                }],
            },
        ],
    });
    (graph, sources)
}

#[test]
fn state_crossfade_trigger_and_interruption_capture_the_composite_pose() {
    let (graph, sources) = state_graph();
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    runtime
        .fire_trigger(&GraphParameterId::new("trigger:go"))
        .unwrap();
    runtime.advance(Tick::ZERO);
    assert_eq!(
        runtime.state_trace().current_state,
        Some(GraphStateId::new("state:b"))
    );
    assert_eq!(number(runtime.frame(), "x"), 0.0);
    assert_eq!(
        runtime.parameters()[0].value,
        GraphParameterValue::Trigger(false)
    );

    runtime.advance(Tick(5));
    assert_eq!(number(runtime.frame(), "x"), 5.0);
    runtime
        .fire_trigger(&GraphParameterId::new("trigger:interrupt"))
        .unwrap();
    runtime.advance(Tick::ZERO);
    assert_eq!(number(runtime.frame(), "x"), 5.0);
    assert!(
        runtime
            .state_trace()
            .transition
            .as_ref()
            .unwrap()
            .source_is_captured_composite
    );
    assert!(runtime.input_trace_truncated());
    runtime.advance(Tick(5));
    assert_eq!(number(runtime.frame(), "x"), 12.5);
    assert!(runtime.input_trace_truncated());
    runtime.advance(Tick(5));
    assert_eq!(number(runtime.frame(), "x"), 20.0);
    assert!(!runtime.input_trace_truncated());
}

#[test]
fn triggers_latch_until_a_winning_transition_references_them() {
    let (graph, sources) = state_graph();
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    let go = GraphParameterId::new("trigger:go");
    let interrupt = GraphParameterId::new("trigger:interrupt");

    runtime.fire_trigger(&interrupt).unwrap();
    runtime.advance(Tick::ZERO);
    assert_eq!(
        runtime
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id == interrupt)
            .unwrap()
            .value,
        GraphParameterValue::Trigger(true)
    );

    runtime.fire_trigger(&go).unwrap();
    runtime.advance(Tick::ZERO);
    assert_eq!(
        runtime
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id == go)
            .unwrap()
            .value,
        GraphParameterValue::Trigger(false)
    );
    assert_eq!(
        runtime
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id == interrupt)
            .unwrap()
            .value,
        GraphParameterValue::Trigger(true)
    );

    runtime.advance(Tick::ZERO);
    assert_eq!(
        runtime
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id == interrupt)
            .unwrap()
            .value,
        GraphParameterValue::Trigger(false)
    );
    assert_eq!(
        runtime.state_trace().current_state,
        Some(GraphStateId::new("state:c"))
    );
}

#[test]
fn transition_priority_id_ties_are_deterministic_without_an_automatic_cycle() {
    let (mut graph, sources) = state_graph();
    let machine = graph.state_machine.as_mut().unwrap();
    machine.transitions.clear();
    machine.transitions.extend([
        GraphTransition {
            id: GraphTransitionId::new("transition:z"),
            from: GraphStateId::new("state:a"),
            to: GraphStateId::new("state:c"),
            priority: 5,
            duration: Tick::ZERO,
            curve: GraphTransitionCurve::Linear,
            interruption: GraphInterruptionPolicy::None,
            mode: GraphTransitionMode::Crossfade,
            conditions: Vec::new(),
        },
        GraphTransition {
            id: GraphTransitionId::new("transition:a"),
            from: GraphStateId::new("state:a"),
            to: GraphStateId::new("state:b"),
            priority: 5,
            duration: Tick::ZERO,
            curve: GraphTransitionCurve::Linear,
            interruption: GraphInterruptionPolicy::None,
            mode: GraphTransitionMode::Crossfade,
            conditions: Vec::new(),
        },
    ]);
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    runtime.advance(Tick::ZERO);
    assert_eq!(runtime.state_trace().transitions_taken_this_update, 1);
    assert!(!runtime.state_trace().transition_limit_reached);
    // Lexical transition ID wins the equal-priority first choice.
    assert_eq!(
        runtime.state_trace().current_state,
        Some(GraphStateId::new("state:b"))
    );
}

#[test]
fn unconditional_zero_duration_transition_cycles_fail_compilation() {
    let (mut graph, sources) = state_graph();
    let machine = graph.state_machine.as_mut().unwrap();
    machine.transitions.clear();
    machine.transitions.extend([
        GraphTransition {
            id: GraphTransitionId::new("transition:a-b"),
            from: GraphStateId::new("state:a"),
            to: GraphStateId::new("state:b"),
            priority: 5,
            duration: Tick::ZERO,
            curve: GraphTransitionCurve::Linear,
            interruption: GraphInterruptionPolicy::None,
            mode: GraphTransitionMode::Crossfade,
            conditions: Vec::new(),
        },
        GraphTransition {
            id: GraphTransitionId::new("transition:b-a"),
            from: GraphStateId::new("state:b"),
            to: GraphStateId::new("state:a"),
            priority: 5,
            duration: Tick::ZERO,
            curve: GraphTransitionCurve::Linear,
            interruption: GraphInterruptionPolicy::None,
            mode: GraphTransitionMode::Crossfade,
            conditions: Vec::new(),
        },
    ]);

    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::InvalidTransition
            && diagnostic.message.contains("zero-duration")
            && diagnostic
                .recovery
                .contains("condition or positive duration")
    }));
}

#[test]
fn events_are_collected_then_filtered_and_bounded() {
    let x = binding("x", ValueKind::Number);
    let sources = catalog([
        constant_sequence(
            "sequence:a",
            vec![(x.clone(), AnimValue::Number(0.0))],
            Some(5),
        ),
        constant_sequence(
            "sequence:b",
            vec![(x.clone(), AnimValue::Number(10.0))],
            Some(5),
        ),
    ]);
    let output = GraphNodeId::new("node:blend");
    let mut graph = AnimationGraph::flat(
        "graph:events",
        "Events",
        output.clone(),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![
            clip_node("node:a", "sequence:a"),
            clip_node("node:b", "sequence:b"),
            GraphNode {
                id: output,
                name: "Blend".into(),
                kind: GraphNodeKind::Blend {
                    mode: GraphBlendMode::Normalized,
                    inputs: vec![
                        GraphBlendInput {
                            node: GraphNodeId::new("node:a"),
                            weight: GraphWeight::Constant(0.8),
                        },
                        GraphBlendInput {
                            node: GraphNodeId::new("node:b"),
                            weight: GraphWeight::Constant(0.2),
                        },
                    ],
                },
            },
        ],
    );
    graph.event_policy = GraphEventPolicy::Dominant;
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    runtime.advance(Tick(6));
    assert_eq!(runtime.events().len(), 1);
    assert_eq!(runtime.events()[0].source_node, GraphNodeId::new("node:a"));

    graph.event_policy = GraphEventPolicy::All;
    let mut runtime = AnimationGraphRuntimeInstance::with_event_limit(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
        1,
    );
    runtime.advance(Tick(6));
    assert_eq!(runtime.events().len(), 1);
    assert!(runtime.events_truncated());
}

#[test]
fn duplicate_event_routes_merge_weight_before_policy_and_candidate_bounds() {
    let x = binding("x", ValueKind::Number);
    let source = constant_sequence(
        "sequence:shared",
        vec![(x.clone(), AnimValue::Number(1.0))],
        Some(5),
    );
    let sources = catalog([source]);
    let trigger = GraphParameterId::new("trigger:go");
    let mut graph = AnimationGraph::flat(
        "graph:shared-event",
        "Shared event",
        GraphNodeId::new("node:shared"),
        vec![reference(x, AnimValue::Number(0.0))],
        vec![clip_node("node:shared", "sequence:shared")],
    );
    graph.event_policy = GraphEventPolicy::Threshold {
        minimum_weight: 0.75,
    };
    graph.limits.max_event_candidates_per_update = 1;
    graph.parameters.push(GraphParameter {
        id: trigger.clone(),
        name: "Go".into(),
        kind: GraphParameterKind::Trigger,
        default: GraphParameterValue::Trigger(false),
    });
    graph.state_machine = Some(GraphStateMachine {
        initial_state: GraphStateId::new("state:a"),
        states: vec![
            GraphState {
                id: GraphStateId::new("state:a"),
                name: "A".into(),
                node: GraphNodeId::new("node:shared"),
                reset_on_entry: true,
            },
            GraphState {
                id: GraphStateId::new("state:b"),
                name: "B".into(),
                node: GraphNodeId::new("node:shared"),
                reset_on_entry: true,
            },
        ],
        transitions: vec![GraphTransition {
            id: GraphTransitionId::new("transition:a-b"),
            from: GraphStateId::new("state:a"),
            to: GraphStateId::new("state:b"),
            priority: 1,
            duration: Tick(12),
            curve: GraphTransitionCurve::Linear,
            interruption: GraphInterruptionPolicy::None,
            mode: GraphTransitionMode::Crossfade,
            conditions: vec![GraphCondition {
                parameter: trigger.clone(),
                test: GraphConditionTest::Triggered,
            }],
        }],
    });

    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    runtime.fire_trigger(&trigger).unwrap();
    runtime.advance(Tick::ZERO);
    runtime.advance(Tick(6));

    assert_eq!(runtime.events().len(), 1);
    assert!((runtime.events()[0].contribution_weight - 1.0).abs() < 1.0e-12);
    assert!(!runtime.events_truncated());
}

#[test]
fn hot_rebind_preserves_state_but_never_replays_a_latched_trigger() {
    let (graph, sources) = state_graph();
    let plan = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    let mut runtime = AnimationGraphRuntimeInstance::new(plan);
    runtime
        .fire_trigger(&GraphParameterId::new("trigger:go"))
        .unwrap();
    runtime.advance(Tick::ZERO);
    runtime.advance(Tick(10));
    assert_eq!(
        runtime.state_trace().current_state,
        Some(GraphStateId::new("state:b"))
    );
    let interrupt = GraphParameterId::new("trigger:interrupt");
    runtime.fire_trigger(&interrupt).unwrap();
    assert_eq!(
        runtime
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id == interrupt)
            .unwrap()
            .value,
        GraphParameterValue::Trigger(true)
    );

    let mut reordered = graph.clone();
    reordered.state_machine.as_mut().unwrap().states.reverse();
    reordered.nodes.reverse();
    let replacement = CompiledAnimationGraph::compile(&reordered, &sources).unwrap();
    runtime.rebind_plan(replacement, AnimationGraphRebindPolicy::PreserveState);
    assert_eq!(
        runtime.state_trace().current_state,
        Some(GraphStateId::new("state:b"))
    );
    assert!(runtime.state_trace().transition.is_none());
    assert_eq!(
        runtime
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id == interrupt)
            .unwrap()
            .value,
        GraphParameterValue::Trigger(false)
    );
}

#[test]
fn invalid_reference_weight_triangle_and_future_transition_modes_are_diagnosed() {
    let x = binding("x", ValueKind::Number);
    let source = constant_sequence(
        "sequence:a",
        vec![(x.clone(), AnimValue::Number(1.0))],
        None,
    );
    let sources = catalog([source]);
    let mut graph = AnimationGraph::flat(
        "graph:admission",
        "Admission",
        GraphNodeId::new("node:blend"),
        vec![reference(x, AnimValue::Number(f64::NAN))],
        vec![
            clip_node("node:a", "sequence:a"),
            GraphNode {
                id: GraphNodeId::new("node:blend"),
                name: "Bad blend".into(),
                kind: GraphNodeKind::Blend {
                    mode: GraphBlendMode::Normalized,
                    inputs: vec![GraphBlendInput {
                        node: GraphNodeId::new("node:a"),
                        weight: GraphWeight::Constant(-1.0),
                    }],
                },
            },
        ],
    );
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.code == AnimationGraphIssueCode::InvalidReferenceValue }));
    assert!(error
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == AnimationGraphIssueCode::InvalidWeight));

    graph.reference_pose[0].value = AnimValue::Number(0.0);
    graph.parameters = vec![
        GraphParameter {
            id: GraphParameterId::new("parameter:x"),
            name: "X".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.0),
        },
        GraphParameter {
            id: GraphParameterId::new("parameter:y"),
            name: "Y".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(0.0),
        },
    ];
    graph.output = GraphNodeId::new("node:2d");
    graph.nodes[1] = GraphNode {
        id: graph.output.clone(),
        name: "Degenerate 2D".into(),
        kind: GraphNodeKind::Blend2d {
            parameter_x: GraphParameterId::new("parameter:x"),
            parameter_y: GraphParameterId::new("parameter:y"),
            points: vec![
                GraphBlend2dPoint {
                    id: GraphBlendPointId::new("point:a"),
                    position: [0.0, 0.0],
                    node: GraphNodeId::new("node:a"),
                },
                GraphBlend2dPoint {
                    id: GraphBlendPointId::new("point:b"),
                    position: [1.0, 0.0],
                    node: GraphNodeId::new("node:a"),
                },
                GraphBlend2dPoint {
                    id: GraphBlendPointId::new("point:c"),
                    position: [2.0, 0.0],
                    node: GraphNodeId::new("node:a"),
                },
            ],
            triangles: vec![GraphBlend2dTriangle {
                points: [
                    GraphBlendPointId::new("point:a"),
                    GraphBlendPointId::new("point:b"),
                    GraphBlendPointId::new("point:c"),
                ],
            }],
            outside_hull: GraphOutsideHullMode::NearestPoint,
        },
    };
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.code == AnimationGraphIssueCode::InvalidTriangle));

    let (mut graph, sources) = state_graph();
    graph.state_machine.as_mut().unwrap().transitions[0].mode = GraphTransitionMode::Inertialized;
    let error = CompiledAnimationGraph::compile(&graph, &sources).unwrap_err();
    assert!(error.diagnostics.iter().any(|diagnostic| {
        diagnostic.code == AnimationGraphIssueCode::UnsupportedTransitionMode
    }));
}

#[test]
fn scalar_condition_matrix_and_crossfade_curves_are_deterministic() {
    let (mut graph, sources) = state_graph();
    let number_id = GraphParameterId::new("parameter:number");
    let integer_id = GraphParameterId::new("parameter:integer");
    let boolean_id = GraphParameterId::new("parameter:boolean");
    graph.parameters = vec![
        GraphParameter {
            id: number_id.clone(),
            name: "Number".into(),
            kind: GraphParameterKind::Number,
            default: GraphParameterValue::Number(1.0),
        },
        GraphParameter {
            id: integer_id.clone(),
            name: "Integer".into(),
            kind: GraphParameterKind::Integer,
            default: GraphParameterValue::Integer(1),
        },
        GraphParameter {
            id: boolean_id.clone(),
            name: "Boolean".into(),
            kind: GraphParameterKind::Boolean,
            default: GraphParameterValue::Boolean(true),
        },
    ];
    let transition = &mut graph.state_machine.as_mut().unwrap().transitions[0];
    transition.curve = GraphTransitionCurve::EaseIn;
    transition.conditions = vec![
        GraphCondition {
            parameter: number_id.clone(),
            test: GraphConditionTest::NumberEquals(1.0),
        },
        GraphCondition {
            parameter: number_id.clone(),
            test: GraphConditionTest::NumberNotEquals(2.0),
        },
        GraphCondition {
            parameter: number_id.clone(),
            test: GraphConditionTest::NumberGreaterThan(0.0),
        },
        GraphCondition {
            parameter: number_id.clone(),
            test: GraphConditionTest::NumberGreaterOrEqual(1.0),
        },
        GraphCondition {
            parameter: number_id.clone(),
            test: GraphConditionTest::NumberLessThan(2.0),
        },
        GraphCondition {
            parameter: number_id,
            test: GraphConditionTest::NumberLessOrEqual(1.0),
        },
        GraphCondition {
            parameter: integer_id.clone(),
            test: GraphConditionTest::IntegerEquals(1),
        },
        GraphCondition {
            parameter: integer_id.clone(),
            test: GraphConditionTest::IntegerNotEquals(2),
        },
        GraphCondition {
            parameter: integer_id.clone(),
            test: GraphConditionTest::IntegerGreaterThan(0),
        },
        GraphCondition {
            parameter: integer_id.clone(),
            test: GraphConditionTest::IntegerGreaterOrEqual(1),
        },
        GraphCondition {
            parameter: integer_id.clone(),
            test: GraphConditionTest::IntegerLessThan(2),
        },
        GraphCondition {
            parameter: integer_id,
            test: GraphConditionTest::IntegerLessOrEqual(1),
        },
        GraphCondition {
            parameter: boolean_id,
            test: GraphConditionTest::BooleanEquals(true),
        },
    ];
    graph
        .state_machine
        .as_mut()
        .unwrap()
        .transitions
        .truncate(1);
    let mut runtime = AnimationGraphRuntimeInstance::new(
        CompiledAnimationGraph::compile(&graph, &sources).unwrap(),
    );
    runtime.advance(Tick::ZERO);
    runtime.advance(Tick(5));
    assert_eq!(number(runtime.frame(), "x"), 2.5);
    let trace = runtime.state_trace().transition.as_ref().unwrap();
    assert_eq!(trace.linear_progress, 0.5);
    assert_eq!(trace.curved_progress, 0.25);
}

#[test]
fn rational_clip_rate_event_threshold_none_and_source_revision_are_observable() {
    let x = binding("x", ValueKind::Number);
    let source = constant_sequence(
        "sequence:rate",
        vec![(x.clone(), AnimValue::Number(1.0))],
        Some(5),
    );
    let mut sources = catalog([source]);
    let node = GraphNode {
        id: GraphNodeId::new("node:rate"),
        name: "Rate".into(),
        kind: GraphNodeKind::Clip {
            sequence: SequenceId::new("sequence:rate"),
            rate: RationalRate::new(2, 1),
            start_tick: Tick::ZERO,
        },
    };
    let mut graph = AnimationGraph::flat(
        "graph:rate",
        "Rate",
        node.id.clone(),
        vec![reference(x.clone(), AnimValue::Number(0.0))],
        vec![node],
    );
    graph.event_policy = GraphEventPolicy::Threshold {
        minimum_weight: 1.0,
    };
    let first = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    let first_hash = first.stable_hash.clone();
    let mut runtime = AnimationGraphRuntimeInstance::new(first);
    runtime.advance(Tick(3));
    assert_eq!(runtime.events().len(), 1);
    assert_eq!(
        runtime
            .node_trace()
            .iter()
            .find(|trace| trace.node_id == GraphNodeId::new("node:rate"))
            .unwrap()
            .local_tick,
        Some(Tick(6))
    );

    graph.event_policy = GraphEventPolicy::None;
    let none_plan = CompiledAnimationGraph::compile(&graph, &sources).unwrap();
    let none_hash = none_plan.stable_hash.clone();
    let mut runtime = AnimationGraphRuntimeInstance::new(none_plan);
    runtime.advance(Tick(3));
    assert!(runtime.events().is_empty());

    let changed = constant_sequence("sequence:rate", vec![(x, AnimValue::Number(2.0))], Some(5));
    sources.insert(changed.id.clone(), changed);
    let changed_hash = CompiledAnimationGraph::compile(&graph, &sources)
        .unwrap()
        .stable_hash;
    assert_ne!(none_hash, changed_hash);
    assert_ne!(first_hash, none_hash);
}
