//! Public-boundary validation for complete-scene GLB export. Detailed math/format unit tests remain beside
//! the writer; these tests parse the artifact through the independent glTF reader and the engine importer.

use std::collections::BTreeSet;

use metrocalk_assets::{
    export_scene_glb, export_scene_glb_with_limits, AnimationChannel, AnimationInterpolation,
    AnimationSample, AnimationTarget, AnimationValue, AnimationValueType, FidelityStatus,
    GltfImporter, Material, Matrix4d, MeshAsset, MeshSource, Primitive, SceneAnimation, SceneAsset,
    SceneGlbExportError, SceneGlbExportLimits, SceneJoint, SceneMeshRef, SceneNode, SceneNodeId,
    SceneSkin, SceneSkinRef, SceneUpAxis, Texture,
};

#[test]
#[allow(clippy::too_many_lines)]
fn complete_scene_is_deterministic_valid_and_round_trips_standard_payloads() {
    let scene = complete_scene_fixture();
    let first = export_scene_glb(&scene).expect("complete scene exports");
    let second = export_scene_glb(&scene).expect("repeat export");
    assert_eq!(
        first.bytes, second.bytes,
        "same scene produces identical GLB bytes"
    );
    assert_eq!(first.report, second.report);
    assert_eq!(&first.bytes[..4], b"glTF");
    assert_eq!(first.bytes.len() % 4, 0);

    let document = gltf::Gltf::from_slice(&first.bytes).expect("artifact is valid glTF 2.0");
    assert_eq!(document.scenes().count(), 1);
    assert_eq!(
        document.meshes().count(),
        2,
        "static + skinned reusable payloads"
    );
    assert_eq!(
        document.materials().count(),
        2,
        "mesh-local materials are globally remapped"
    );
    assert_eq!(
        document.textures().count(),
        1,
        "decoded RGBA texture is embedded"
    );

    let nodes = document.nodes().collect::<Vec<_>>();
    let by_name = nodes
        .iter()
        .map(|node| (node.name().expect("authored node name"), node.index()))
        .collect::<std::collections::BTreeMap<_, _>>();
    let shared_a = &nodes[by_name["SharedA"]];
    let shared_b = &nodes[by_name["SharedB"]];
    assert_eq!(
        shared_a.mesh().expect("mesh instance").index(),
        shared_b.mesh().expect("mesh instance").index(),
        "two scene nodes share one mesh object"
    );
    let assembly_children = nodes[by_name["Assembly"]]
        .children()
        .map(|child| child.name().unwrap().to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        assembly_children,
        BTreeSet::from(["SharedA".into(), "SharedB".into()])
    );

    let skin = document.skins().next().expect("standard glTF skin");
    assert_eq!(skin.joints().count(), 2);
    assert_eq!(skin.skeleton().unwrap().name(), Some("HipNode"));
    let body = &nodes[by_name["Body"]];
    assert_eq!(
        body.skin().expect("body skin binding").index(),
        skin.index()
    );
    let skinned_primitive = document
        .meshes()
        .nth(1)
        .unwrap()
        .primitives()
        .next()
        .unwrap();
    assert!(skinned_primitive.get(&gltf::Semantic::Joints(0)).is_some());
    assert!(skinned_primitive.get(&gltf::Semantic::Weights(0)).is_some());
    let blob = document.blob.as_deref().expect("GLB binary chunk");
    let inverse_binds = skin
        .reader(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => Some(blob),
            gltf::buffer::Source::Uri(_) => None,
        })
        .read_inverse_bind_matrices()
        .expect("inverse bind accessor")
        .collect::<Vec<_>>();
    assert_eq!(inverse_binds.len(), 2);
    assert!((inverse_binds[1][3][1] + 1.0).abs() < 1.0e-6);

    let animation = document.animations().next().expect("representable clip");
    assert_eq!(animation.name(), Some("WalkAndState"));
    assert_eq!(
        animation.channels().count(),
        3,
        "one matrix channel becomes T/R/S"
    );
    let paths = animation
        .channels()
        .map(|channel| channel.target().property())
        .collect::<Vec<_>>();
    assert!(paths.contains(&gltf::animation::Property::Translation));
    assert!(paths.contains(&gltf::animation::Property::Rotation));
    assert!(paths.contains(&gltf::animation::Property::Scale));
    for channel in animation.channels() {
        assert_eq!(channel.target().node().name(), Some("KneeNode"));
        let reader = channel.reader(|buffer| match buffer.source() {
            gltf::buffer::Source::Bin => Some(blob),
            gltf::buffer::Source::Uri(_) => None,
        });
        assert_eq!(
            reader.read_inputs().unwrap().collect::<Vec<_>>(),
            vec![0.0, 1.0]
        );
    }

    let restored = GltfImporter::new()
        .import(&first.bytes)
        .expect("engine importer reads standard geometry/skin payloads");
    assert_eq!(restored.primitives.len(), 2);
    assert_eq!(restored.materials.len(), 2);
    assert_eq!(restored.textures.len(), 1);
    assert_eq!(restored.skeleton.as_ref().unwrap().joints.len(), 2);
    assert!(restored
        .primitives
        .iter()
        .any(|primitive| !primitive.joints.is_empty()));

    assert!(first.report.entries.iter().any(|entry| {
        entry.feature == "matrix_animation_to_trs" && entry.status == FidelityStatus::Converted
    }));
    assert!(first.report.entries.iter().any(|entry| {
        entry.feature == "project_property_animation" && entry.status == FidelityStatus::Omitted
    }));
    assert!(
        !first.report.is_lossless(),
        "omitted property animation is explicit"
    );
}

#[test]
fn z_up_centimetres_use_a_single_generated_conversion_root() {
    let mut scene = static_scene_fixture();
    scene.up_axis = SceneUpAxis::Z;
    scene.meters_per_unit = 0.01;
    let export = export_scene_glb(&scene).expect("scene exports");
    let document = gltf::Gltf::from_slice(&export.bytes).expect("valid GLB");
    let root = document
        .default_scene()
        .unwrap()
        .nodes()
        .next()
        .expect("generated root");
    assert_eq!(root.name(), Some("__MetrocalkAxisAndUnitConversion"));
    assert_eq!(root.children().count(), 1);
    let matrix = root.transform().matrix();
    assert!((matrix[0][0] - 0.01).abs() < 1.0e-6);
    assert!(
        (matrix[2][1] - 0.01).abs() < 1.0e-6,
        "source +Z becomes glTF +Y"
    );
    assert!(
        (matrix[1][2] + 0.01).abs() < 1.0e-6,
        "source +Y becomes glTF -Z"
    );
    assert!(export.report.entries.iter().any(|entry| {
        entry.feature == "scene_units_and_up_axis" && entry.status == FidelityStatus::Converted
    }));
}

#[test]
fn shear_animation_is_omitted_without_flattening_the_static_matrix() {
    let mut scene = static_scene_fixture();
    scene.nodes[0].local_transform.rows[0][1] = 0.25;
    scene.animations.push(SceneAnimation {
        name: "Shear".into(),
        channels: vec![AnimationChannel {
            target: AnimationTarget::NodeLocalTransform(SceneNodeId(1)),
            value_type: AnimationValueType::Matrix4d,
            interpolation: AnimationInterpolation::Linear,
            samples: vec![
                AnimationSample {
                    time_code: 0.0,
                    value: AnimationValue::Matrix4d(scene.nodes[0].local_transform),
                },
                AnimationSample {
                    time_code: 24.0,
                    value: AnimationValue::Matrix4d(scene.nodes[0].local_transform),
                },
            ],
        }],
    });
    let export = export_scene_glb(&scene).expect("static scene remains exportable");
    let document = gltf::Gltf::from_slice(&export.bytes).expect("valid GLB");
    assert_eq!(document.animations().count(), 0);
    let matrix = document.nodes().next().unwrap().transform().matrix();
    assert!(
        (matrix[1][0] - 0.25).abs() < 1.0e-6,
        "static shear matrix survives"
    );
    assert!(export.report.entries.iter().any(|entry| {
        entry.feature == "non_trs_matrix_animation" && entry.status == FidelityStatus::Omitted
    }));
}

#[test]
fn hierarchy_skin_weights_and_output_budgets_fail_preflight() {
    let mut cyclic = static_scene_fixture();
    cyclic.nodes[0].parent = Some(SceneNodeId(1));
    assert!(matches!(
        export_scene_glb(&cyclic),
        Err(SceneGlbExportError::InvalidHierarchy(_))
    ));

    let mut malformed = complete_scene_fixture();
    malformed.meshes[1].primitives[0].weights[0] = [0.25, 0.0, 0.0, 0.0];
    assert!(matches!(
        export_scene_glb(&malformed),
        Err(SceneGlbExportError::InvalidMesh { mesh: 1, .. })
    ));

    let mut contradictory = complete_scene_fixture();
    contradictory.skins[0].joints[1].parent = None;
    assert!(matches!(
        export_scene_glb(&contradictory),
        Err(SceneGlbExportError::InvalidSkin { skin: 0, .. })
    ));

    let limits = SceneGlbExportLimits {
        max_output_bytes: 64,
        ..SceneGlbExportLimits::default()
    };
    assert!(matches!(
        export_scene_glb_with_limits(&static_scene_fixture(), limits),
        Err(SceneGlbExportError::BudgetExceeded {
            kind: "output bytes",
            ..
        })
    ));
}

fn complete_scene_fixture() -> SceneAsset {
    let static_mesh = MeshAsset {
        name: "SharedTriangle".into(),
        primitives: vec![triangle_primitive(false)],
        materials: vec![Material {
            base_color_texture: Some(0),
            ..Material::default()
        }],
        textures: vec![Texture {
            width: 1,
            height: 1,
            rgba8: vec![255, 128, 64, 255],
        }],
        skeleton: None,
    };
    let skinned_mesh = MeshAsset {
        name: "SkinnedTriangle".into(),
        primitives: vec![triangle_primitive(true)],
        materials: vec![Material {
            base_color: [0.2, 0.4, 0.8, 1.0],
            ..Material::default()
        }],
        textures: vec![],
        skeleton: None,
    };
    let mut scene = SceneAsset::new("CompleteRoundTrip");
    scene.meshes.extend([static_mesh, skinned_mesh]);
    scene.nodes.extend([
        node(1, "Assembly", None, None, None),
        node(2, "SharedA", Some(1), Some(0), None),
        node(3, "SharedB", Some(1), Some(0), None),
        node(10, "HipNode", None, None, None),
        SceneNode {
            local_transform: translation(0.0, 1.0, 0.0),
            ..node(20, "KneeNode", Some(10), None, None)
        },
        node(30, "Body", None, Some(1), Some(0)),
    ]);
    scene.skins.push(SceneSkin {
        name: "BodyRig".into(),
        skeleton_root: Some(SceneNodeId(10)),
        joints: vec![
            SceneJoint {
                node: SceneNodeId(10),
                name: "Hip".into(),
                parent: None,
                rest_local_transform: Matrix4d::IDENTITY,
                bind_transform: Matrix4d::IDENTITY,
                inverse_bind_matrix: Matrix4d::IDENTITY,
            },
            SceneJoint {
                node: SceneNodeId(20),
                name: "Knee".into(),
                parent: Some(0),
                rest_local_transform: translation(0.0, 1.0, 0.0),
                bind_transform: translation(0.0, 1.0, 0.0),
                inverse_bind_matrix: translation(0.0, -1.0, 0.0),
            },
        ],
    });
    scene.animations.push(SceneAnimation {
        name: "WalkAndState".into(),
        channels: vec![
            AnimationChannel {
                target: AnimationTarget::NodeLocalTransform(SceneNodeId(20)),
                value_type: AnimationValueType::Matrix4d,
                interpolation: AnimationInterpolation::Linear,
                samples: vec![
                    AnimationSample {
                        time_code: 0.0,
                        value: AnimationValue::Matrix4d(translation(0.0, 1.0, 0.0)),
                    },
                    AnimationSample {
                        time_code: 24.0,
                        value: AnimationValue::Matrix4d(translation(0.25, 1.0, 0.0)),
                    },
                ],
            },
            AnimationChannel {
                target: AnimationTarget::NodeProperty {
                    node: SceneNodeId(30),
                    component: "Gameplay".into(),
                    field: "Enabled".into(),
                },
                value_type: AnimationValueType::Boolean,
                interpolation: AnimationInterpolation::Step,
                samples: vec![
                    AnimationSample {
                        time_code: 0.0,
                        value: AnimationValue::Boolean(true),
                    },
                    AnimationSample {
                        time_code: 24.0,
                        value: AnimationValue::Boolean(false),
                    },
                ],
            },
        ],
    });
    scene
}

fn static_scene_fixture() -> SceneAsset {
    let mut scene = SceneAsset::new("Static");
    scene.meshes.push(MeshAsset {
        name: "Triangle".into(),
        primitives: vec![triangle_primitive(false)],
        materials: vec![Material::default()],
        ..MeshAsset::default()
    });
    scene.nodes.push(node(1, "Root", None, Some(0), None));
    scene
}

fn triangle_primitive(skinned: bool) -> Primitive {
    Primitive {
        positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        normals: vec![[0.0, 0.0, 1.0]; 3],
        uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
        indices: vec![0, 1, 2],
        material: 0,
        joints: if skinned {
            vec![[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]]
        } else {
            vec![]
        },
        weights: if skinned {
            vec![[1.0, 0.0, 0.0, 0.0]; 3]
        } else {
            vec![]
        },
        ..Primitive::default()
    }
}

fn node(
    id: u64,
    name: &str,
    parent: Option<u64>,
    mesh: Option<usize>,
    skin: Option<usize>,
) -> SceneNode {
    SceneNode {
        id: SceneNodeId(id),
        name: name.into(),
        parent: parent.map(SceneNodeId),
        local_transform: Matrix4d::IDENTITY,
        mesh: mesh.map(SceneMeshRef),
        skin: skin.map(SceneSkinRef),
        visible: true,
    }
}

fn translation(x: f64, y: f64, z: f64) -> Matrix4d {
    Matrix4d {
        rows: [
            [1.0, 0.0, 0.0, x],
            [0.0, 1.0, 0.0, y],
            [0.0, 0.0, 1.0, z],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}
