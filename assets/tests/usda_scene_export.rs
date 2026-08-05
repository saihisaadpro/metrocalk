//! Cross-boundary syntax check: the public neutral scene/export API emits a stage the workspace OpenUSD
//! reader can open. Detailed deterministic/fidelity tests live beside the writer implementation.

use std::sync::atomic::{AtomicU64, Ordering};

use metrocalk_assets::{
    export_usda, AnimationChannel, AnimationInterpolation, AnimationSample, AnimationTarget,
    AnimationValue, AnimationValueType, Material, Matrix4d, MeshAsset, Primitive, SceneAnimation,
    SceneAsset, SceneJoint, SceneMeshRef, SceneNode, SceneNodeId, SceneSkin, SceneSkinRef,
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn generated_ascii_stage_opens_in_the_workspace_openusd_reader() {
    let mesh = MeshAsset {
        name: "Triangle".into(),
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            material: 0,
            ..Primitive::default()
        }],
        materials: vec![Material::default()],
        ..MeshAsset::default()
    };
    let mut scene = SceneAsset::new("ParserCheck");
    scene.meshes.push(mesh);
    scene.nodes.push(SceneNode {
        id: SceneNodeId(1),
        name: "Root".into(),
        parent: None,
        local_transform: Matrix4d {
            rows: [
                [1.0, 0.0, 0.0, 1.0],
                [0.0, 1.0, 0.0, 2.0],
                [0.0, 0.0, 1.0, 3.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        },
        mesh: Some(SceneMeshRef(0)),
        skin: None,
        visible: true,
    });

    let export = export_usda(&scene).expect("stage encodes");
    assert_opens(&export.text, "static");
}

#[test]
// Keeping the complete rig + binding + two-channel fixture inline makes the independently parsed contract
// visible in one place; the production writer remains decomposed and lint-clean.
#[allow(clippy::too_many_lines)]
fn rigged_animated_stage_constructs_parse_as_usda() {
    let mesh = MeshAsset {
        name: "SkinnedTriangle".into(),
        primitives: vec![Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            material: 0,
            joints: vec![[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0]],
            weights: vec![[1.0, 0.0, 0.0, 0.0]; 3],
            ..Primitive::default()
        }],
        materials: vec![Material::default()],
        ..MeshAsset::default()
    };
    let mut scene = SceneAsset::new("RigAnimationParserCheck");
    scene.meshes.push(mesh);
    scene.nodes.extend([
        SceneNode {
            id: SceneNodeId(1),
            name: "HipNode".into(),
            parent: None,
            local_transform: Matrix4d::IDENTITY,
            mesh: None,
            skin: None,
            visible: true,
        },
        SceneNode {
            id: SceneNodeId(2),
            name: "KneeNode".into(),
            parent: Some(SceneNodeId(1)),
            local_transform: translation(0.0, 1.0, 0.0),
            mesh: None,
            skin: None,
            visible: true,
        },
        SceneNode {
            id: SceneNodeId(3),
            name: "Body".into(),
            parent: None,
            local_transform: Matrix4d::IDENTITY,
            mesh: Some(SceneMeshRef(0)),
            skin: Some(SceneSkinRef(0)),
            visible: true,
        },
    ]);
    scene.skins.push(SceneSkin {
        name: "BodyRig".into(),
        skeleton_root: Some(SceneNodeId(1)),
        joints: vec![
            SceneJoint {
                node: SceneNodeId(1),
                name: "Hip".into(),
                parent: None,
                rest_local_transform: Matrix4d::IDENTITY,
                bind_transform: Matrix4d::IDENTITY,
                inverse_bind_matrix: Matrix4d::IDENTITY,
            },
            SceneJoint {
                node: SceneNodeId(2),
                name: "Knee".into(),
                parent: Some(0),
                rest_local_transform: translation(0.0, 1.0, 0.0),
                bind_transform: translation(0.0, 1.0, 0.0),
                inverse_bind_matrix: translation(0.0, -1.0, 0.0),
            },
        ],
    });
    scene.animations.push(SceneAnimation {
        name: "PoseAndVisibility".into(),
        channels: vec![
            AnimationChannel {
                target: AnimationTarget::NodeLocalTransform(SceneNodeId(2)),
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
                    node: SceneNodeId(3),
                    component: "Visibility".into(),
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

    let export = export_usda(&scene).expect("rigged animated stage encodes");
    assert_opens(&export.text, "rig-animation");
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

fn assert_opens(text: &str, tag: &str) {
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "metrocalk-usda-export-{}-{sequence}-{tag}.usda",
        std::process::id(),
    ));
    std::fs::write(&path, text).expect("write parser fixture");
    let opened = openusd::usd::Stage::open(path.to_string_lossy().as_ref());
    let _ = std::fs::remove_file(&path);
    assert!(
        opened.is_ok(),
        "generated USDA must parse through the workspace OpenUSD reader: {:?}",
        opened.err()
    );
}
