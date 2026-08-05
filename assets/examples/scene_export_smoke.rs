//! Retained-artifact smoke for the public complete-scene writers used by the editor shell.

use std::{env, fmt::Write as _, fs, path::PathBuf};

use metrocalk_assets::{
    export_scene_glb, export_usda, AnimationChannel, AnimationInterpolation, AnimationSample,
    AnimationTarget, AnimationValue, AnimationValueType, Material, Matrix4d, MeshAsset, Primitive,
    SceneAnimation, SceneAsset, SceneJoint, SceneMeshRef, SceneNode, SceneNodeId, SceneSkin,
    SceneSkinRef,
};

#[allow(clippy::too_many_lines)]
fn main() {
    let output_dir = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("usage: scene_export_smoke <output-directory>");
    fs::create_dir_all(&output_dir).expect("create output directory");

    let mesh = MeshAsset {
        name: "ReleaseRiggedTriangle".into(),
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
    let mut scene = SceneAsset::new("ReleaseSceneExportSmoke");
    scene.meshes.push(mesh);
    scene.nodes.extend([
        SceneNode {
            id: SceneNodeId(1),
            name: "Assembly".into(),
            parent: None,
            local_transform: Matrix4d::IDENTITY,
            mesh: None,
            skin: None,
            visible: true,
        },
        SceneNode {
            id: SceneNodeId(2),
            name: "HipNode".into(),
            parent: Some(SceneNodeId(1)),
            local_transform: Matrix4d::IDENTITY,
            mesh: None,
            skin: None,
            visible: true,
        },
        SceneNode {
            id: SceneNodeId(3),
            name: "KneeNode".into(),
            parent: Some(SceneNodeId(2)),
            local_transform: translated(0.0, 1.0, 0.0),
            mesh: None,
            skin: None,
            visible: true,
        },
        SceneNode {
            id: SceneNodeId(4),
            name: "Body".into(),
            parent: Some(SceneNodeId(1)),
            local_transform: Matrix4d::IDENTITY,
            mesh: Some(SceneMeshRef(0)),
            skin: Some(SceneSkinRef(0)),
            visible: true,
        },
    ]);
    scene.skins.push(SceneSkin {
        name: "ReleaseRig".into(),
        skeleton_root: Some(SceneNodeId(2)),
        joints: vec![
            SceneJoint {
                node: SceneNodeId(2),
                name: "Hip".into(),
                parent: None,
                rest_local_transform: Matrix4d::IDENTITY,
                bind_transform: Matrix4d::IDENTITY,
                inverse_bind_matrix: Matrix4d::IDENTITY,
            },
            SceneJoint {
                node: SceneNodeId(3),
                name: "Knee".into(),
                parent: Some(0),
                rest_local_transform: translated(0.0, 1.0, 0.0),
                bind_transform: translated(0.0, 1.0, 0.0),
                inverse_bind_matrix: translated(0.0, -1.0, 0.0),
            },
        ],
    });
    scene.animations.push(SceneAnimation {
        name: "ReleaseMotion".into(),
        channels: vec![
            AnimationChannel {
                target: AnimationTarget::NodeLocalTransform(SceneNodeId(3)),
                value_type: AnimationValueType::Matrix4d,
                interpolation: AnimationInterpolation::Linear,
                samples: vec![
                    AnimationSample {
                        time_code: 0.0,
                        value: AnimationValue::Matrix4d(translated(0.0, 1.0, 0.0)),
                    },
                    AnimationSample {
                        time_code: 24.0,
                        value: AnimationValue::Matrix4d(translated(0.25, 1.0, 0.0)),
                    },
                ],
            },
            AnimationChannel {
                target: AnimationTarget::NodeProperty {
                    node: SceneNodeId(4),
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

    let glb = export_scene_glb(&scene).expect("complete-scene GLB export");
    let usda = export_usda(&scene).expect("complete-scene USDA export");
    assert_eq!(glb.bytes.get(..4), Some(b"glTF".as_slice()));
    assert!(usda.text.starts_with("#usda 1.0"));
    assert!(glb.bytes.len() > 512, "GLB must contain a nontrivial scene");
    assert!(
        usda.text.len() > 512,
        "USDA must contain a nontrivial scene"
    );
    assert!(
        !glb.report.entries.is_empty(),
        "GLB fidelity ledger is required"
    );
    assert!(
        !usda.report.entries.is_empty(),
        "USDA fidelity ledger is required"
    );
    let parsed_glb =
        gltf::Gltf::from_slice(&glb.bytes).expect("independent glTF parser accepts GLB");
    assert_eq!(
        parsed_glb.skins().count(),
        1,
        "GLB retains the authored rig"
    );
    assert_eq!(
        parsed_glb.animations().count(),
        1,
        "GLB retains the representable animation clip"
    );
    assert!(glb
        .report
        .entries
        .iter()
        .any(|entry| entry.feature == "skins_weights_and_inverse_binds" && entry.count > 0));
    assert!(glb
        .report
        .entries
        .iter()
        .any(|entry| entry.feature == "matrix_animation_to_trs" && entry.count > 0));
    assert!(usda
        .report
        .entries
        .iter()
        .any(|entry| entry.feature == "skeleton_joint_metadata" && entry.count > 0));
    assert!(usda
        .report
        .entries
        .iter()
        .any(|entry| entry.feature == "typed_animation_samples" && entry.count > 0));

    let glb_path = output_dir.join("complete-scene-smoke.glb");
    let usda_path = output_dir.join("complete-scene-smoke.usda");
    let ledger_path = output_dir.join("complete-scene-fidelity.tsv");
    fs::write(&glb_path, &glb.bytes).expect("write GLB artifact");
    fs::write(&usda_path, &usda.text).expect("write USDA artifact");
    assert!(
        openusd::usd::Stage::open(usda_path.to_string_lossy().as_ref()).is_ok(),
        "workspace OpenUSD reader accepts retained USDA"
    );

    let mut ledger = String::from("format\tstatus\tfeature\tcount\tdetail\n");
    for (format, report) in [("glb", &glb.report), ("usda", &usda.report)] {
        for entry in &report.entries {
            writeln!(
                ledger,
                "{format}\t{:?}\t{}\t{}\t{}",
                entry.status,
                entry.feature,
                entry.count,
                entry.detail.replace(['\t', '\n'], " "),
            )
            .expect("write in-memory fidelity row");
        }
    }
    fs::write(&ledger_path, &ledger).expect("write fidelity ledger");

    println!(
        "GLB={} bytes={} fidelity_rows={}",
        glb_path.display(),
        glb.bytes.len(),
        glb.report.entries.len()
    );
    println!(
        "USDA={} bytes={} fidelity_rows={}",
        usda_path.display(),
        usda.text.len(),
        usda.report.entries.len()
    );
    println!(
        "LEDGER={} rows={}",
        ledger_path.display(),
        glb.report.entries.len() + usda.report.entries.len()
    );
}

fn translated(x: f64, y: f64, z: f64) -> Matrix4d {
    Matrix4d {
        rows: [
            [1.0, 0.0, 0.0, x],
            [0.0, 1.0, 0.0, y],
            [0.0, 0.0, 1.0, z],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}
