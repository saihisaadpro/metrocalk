//! Capture the authoritative editor world into the versioned asset scene IR.
//!
//! The exporter never reads viewport instance buffers: hierarchy, local transforms, activation, mesh
//! handles and keyframe tracks come from `/core`, while geometry comes from the content-addressed asset
//! store through a caller-supplied resolver. This keeps complete-scene export reproducible after reload.

use std::collections::BTreeMap;
use std::fmt;

use metrocalk_assets::{
    AnimationChannel, AnimationInterpolation, AnimationSample, AnimationTarget, AnimationValue,
    AnimationValueType, AssetAffine, Matrix4d, MeshAsset, SceneAnimation, SceneAsset, SceneJoint,
    SceneMeshRef, SceneNode, SceneNodeId, SceneSkin, SceneSkinRef,
};
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_gizmo::{mat_mul, Mat4};

use crate::capscene::{self, MESH_FIELD};
use crate::kinematics::{joint_of, joint_pose, parse_track, JOINT_TRACK};

/// One resolved asset-store payload and the canonical affine used by the viewport for that handle.
#[derive(Clone, Debug, PartialEq)]
pub struct CapturedMesh {
    pub asset: MeshAsset,
    pub display: AssetAffine,
}

/// A complete-scene capture failure. Export must explain omissions rather than silently skip them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SceneCaptureError {
    UnresolvedMesh { entity: String, handle: String },
    InvalidDisplayAffine { entity: String, handle: String },
    InvalidSkin { entity: String, detail: String },
    TooManyNodes,
}

impl fmt::Display for SceneCaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnresolvedMesh { entity, handle } => {
                write!(
                    formatter,
                    "entity {entity} references unresolved mesh {handle}"
                )
            }
            Self::InvalidDisplayAffine { entity, handle } => write!(
                formatter,
                "entity {entity} has an invalid display transform for mesh {handle}"
            ),
            Self::InvalidSkin { entity, detail } => {
                write!(formatter, "entity {entity} has an invalid skin: {detail}")
            }
            Self::TooManyNodes => write!(formatter, "scene node identity space was exhausted"),
        }
    }
}

impl std::error::Error for SceneCaptureError {}

/// Capture hierarchy, reusable meshes, imported skeletons, and viewport-authored mechanism animation.
///
/// `resolve_mesh` must return the content-addressed asset and the exact affine used to display it. Repeated
/// handles are resolved once and remain shared in [`SceneAsset::meshes`].
#[allow(clippy::too_many_lines)] // Hierarchy, mesh reuse, skins and channels share one identity map.
pub fn capture_scene<F>(
    engine: &Engine<FlecsWorld>,
    name: impl Into<String>,
    mut resolve_mesh: F,
) -> Result<SceneAsset, SceneCaptureError>
where
    F: FnMut(&str) -> Option<CapturedMesh>,
{
    let mut entity_ids = engine.entity_ids();
    entity_ids.sort_unstable();
    let id_map: BTreeMap<EntityId, SceneNodeId> = entity_ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            let index = u64::try_from(index).expect("entity count was already bounded by u64");
            (*id, SceneNodeId(index + 1))
        })
        .collect();
    let mut next_id = u64::try_from(entity_ids.len())
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or(SceneCaptureError::TooManyNodes)?;
    let mut scene = SceneAsset::new(name);

    for &entity in &entity_ids {
        let node_id = id_map[&entity];
        scene.nodes.push(SceneNode {
            id: node_id,
            name: entity_node_name(engine, entity),
            parent: engine
                .parent_of(entity)
                .and_then(|parent| id_map.get(&parent).copied()),
            local_transform: matrix_from_columns(
                capscene::local_transform(engine, entity).to_matrix(),
            ),
            mesh: None,
            skin: None,
            visible: engine.is_active(entity),
        });
    }

    let mut mesh_refs: BTreeMap<String, SceneMeshRef> = BTreeMap::new();
    let mut display_by_handle: BTreeMap<String, AssetAffine> = BTreeMap::new();
    for &entity in &entity_ids {
        let components = engine.resolved_components(entity);
        let Some(handle) = components
            .get("MeshRenderer")
            .and_then(|renderer| renderer.get(MESH_FIELD))
            .and_then(field_string)
        else {
            continue;
        };
        let entity_key = entity.to_loro_key();
        let mesh_reference = if let Some(reference) = mesh_refs.get(handle).copied() {
            reference
        } else {
            let mut resolved =
                resolve_mesh(handle).ok_or_else(|| SceneCaptureError::UnresolvedMesh {
                    entity: entity_key.clone(),
                    handle: handle.to_string(),
                })?;
            if !resolved.display.is_valid() {
                return Err(SceneCaptureError::InvalidDisplayAffine {
                    entity: entity_key.clone(),
                    handle: handle.to_string(),
                });
            }
            if resolved
                .asset
                .skeleton
                .as_ref()
                .is_some_and(|skeleton| skeleton.joints.is_empty())
            {
                resolved.asset.skeleton = None;
            }
            let reference = SceneMeshRef(scene.meshes.len());
            display_by_handle.insert(handle.to_string(), resolved.display);
            mesh_refs.insert(handle.to_string(), reference);
            scene.meshes.push(resolved.asset);
            reference
        };
        let display = display_by_handle[handle];
        let geometry_id = allocate_node_id(&mut next_id)?;
        let geometry_index = scene.nodes.len();
        scene.nodes.push(SceneNode {
            id: geometry_id,
            name: "Geometry".into(),
            parent: Some(id_map[&entity]),
            local_transform: affine_matrix(display),
            mesh: Some(mesh_reference),
            skin: None,
            visible: true,
        });

        let mesh = &scene.meshes[mesh_reference.0];
        let has_skin_streams = mesh
            .primitives
            .iter()
            .any(|primitive| !primitive.joints.is_empty() || !primitive.weights.is_empty());
        match (&mesh.skeleton, has_skin_streams) {
            (None, true) => {
                return Err(SceneCaptureError::InvalidSkin {
                    entity: entity_key,
                    detail: "joint/weight streams have no skeleton".into(),
                });
            }
            (Some(skeleton), false) if !skeleton.joints.is_empty() => {
                return Err(SceneCaptureError::InvalidSkin {
                    entity: entity_key,
                    detail: "skeleton has no joint/weight vertex streams".into(),
                });
            }
            (Some(_), true) => {
                let (skin, joint_nodes) =
                    capture_skin(mesh, geometry_id, &mut next_id, &entity_key)?;
                let skin_reference = SceneSkinRef(scene.skins.len());
                scene.skins.push(skin);
                scene.nodes.extend(joint_nodes);
                scene.nodes[geometry_index].skin = Some(skin_reference);
            }
            (None | Some(_), false) => {}
        }
    }

    let mut channels = Vec::new();
    for &entity in &entity_ids {
        let components = engine.resolved_components(entity);
        let Some(encoded) = components
            .get(JOINT_TRACK)
            .and_then(|track| track.get("keys"))
            .and_then(field_string)
        else {
            continue;
        };
        let Some(joint) = joint_of(engine, entity) else {
            continue;
        };
        let mut keys: Vec<(f64, f64)> = Vec::new();
        for key in parse_track(encoded)
            .into_iter()
            .filter(|(time, value)| time.is_finite() && *time >= 0.0 && value.is_finite())
        {
            if let Some(previous) = keys
                .last_mut()
                .filter(|previous| previous.0.total_cmp(&key.0).is_eq())
            {
                *previous = key;
            } else {
                keys.push(key);
            }
        }
        if keys.is_empty() {
            continue;
        }
        let base = capscene::local_transform(engine, entity);
        let base_position = base.translation.map(f64::from);
        let base_rotation = base.rotation.map(f64::from);
        let (zero_position, zero_rotation) =
            joint_pose(&joint, base_position, base_rotation, -joint.value);
        let samples = keys
            .into_iter()
            .map(|(seconds, value)| {
                let (position, rotation) = joint_pose(&joint, zero_position, zero_rotation, value);
                AnimationSample {
                    time_code: seconds * scene.time_codes_per_second,
                    value: AnimationValue::Matrix4d(trs_matrix(
                        position,
                        rotation,
                        base.scale.map(f64::from),
                    )),
                }
            })
            .collect();
        channels.push(AnimationChannel {
            target: AnimationTarget::NodeLocalTransform(id_map[&entity]),
            value_type: AnimationValueType::Matrix4d,
            interpolation: AnimationInterpolation::Linear,
            samples,
        });
    }
    if !channels.is_empty() {
        scene.animations.push(SceneAnimation {
            name: "Mechanism".into(),
            channels,
        });
    }
    Ok(scene)
}

fn capture_skin(
    mesh: &MeshAsset,
    geometry_node: SceneNodeId,
    next_id: &mut u64,
    entity: &str,
) -> Result<(SceneSkin, Vec<SceneNode>), SceneCaptureError> {
    let skeleton = mesh
        .skeleton
        .as_ref()
        .expect("capture_skin is called only for a validated skeleton binding");
    let mut global_columns: Vec<Mat4> = Vec::with_capacity(skeleton.joints.len());
    let mut joint_ids = Vec::with_capacity(skeleton.joints.len());
    for (index, joint) in skeleton.joints.iter().enumerate() {
        if joint.parent.is_some_and(|parent| parent >= index) {
            return Err(SceneCaptureError::InvalidSkin {
                entity: entity.into(),
                detail: format!("joint {index} parent must precede the child"),
            });
        }
        let local = joint.local_bind.to_matrix();
        let global = joint
            .parent
            .map_or(local, |parent| mat_mul(global_columns[parent], local));
        global_columns.push(global);
        joint_ids.push(allocate_node_id(next_id)?);
    }
    let mut nodes = Vec::with_capacity(skeleton.joints.len());
    let mut joints = Vec::with_capacity(skeleton.joints.len());
    for (index, joint) in skeleton.joints.iter().enumerate() {
        let local = matrix_from_columns(joint.local_bind.to_matrix());
        nodes.push(SceneNode {
            id: joint_ids[index],
            name: format!("Joint_{index}"),
            parent: Some(
                joint
                    .parent
                    .map_or(geometry_node, |parent| joint_ids[parent]),
            ),
            local_transform: local,
            mesh: None,
            skin: None,
            visible: true,
        });
        joints.push(SceneJoint {
            node: joint_ids[index],
            name: format!("Joint_{index}"),
            parent: joint.parent,
            rest_local_transform: local,
            bind_transform: matrix_from_columns(global_columns[index]),
            inverse_bind_matrix: matrix_from_columns(joint.inverse_bind),
        });
    }
    let skeleton_root = skeleton
        .joints
        .iter()
        .position(|joint| joint.parent.is_none())
        .map(|index| joint_ids[index]);
    Ok((
        SceneSkin {
            name: format!("Skin_{}", entity.replace('_', "x")),
            skeleton_root,
            joints,
        },
        nodes,
    ))
}

fn allocate_node_id(next_id: &mut u64) -> Result<SceneNodeId, SceneCaptureError> {
    let id = *next_id;
    *next_id = (*next_id)
        .checked_add(1)
        .ok_or(SceneCaptureError::TooManyNodes)?;
    Ok(SceneNodeId(id))
}

fn field_string(value: &FieldValue) -> Option<&str> {
    match value {
        FieldValue::Str(value) if !value.is_empty() => Some(value),
        _ => None,
    }
}

fn entity_node_name(engine: &Engine<FlecsWorld>, entity: EntityId) -> String {
    let components = engine.resolved_components(entity);
    let label = components
        .get(metrocalk_core::variant::INSTANCE_META)
        .and_then(|metadata| metadata.get("name"))
        .and_then(field_string)
        .unwrap_or("Entity");
    let mut safe = String::with_capacity(label.len().min(64) + 36);
    for character in label.chars().take(64) {
        if character.is_ascii_alphanumeric() || character == '_' {
            safe.push(character);
        } else if !safe.ends_with('_') {
            safe.push('_');
        }
    }
    if safe.is_empty() || safe.as_bytes()[0].is_ascii_digit() {
        safe.insert_str(0, "Entity_");
    }
    format!("{safe}_{:x}_{:x}", entity.peer, entity.counter)
}

fn matrix_from_columns(columns: Mat4) -> Matrix4d {
    let mut rows = [[0.0; 4]; 4];
    for row in 0..4 {
        for column in 0..4 {
            rows[row][column] = f64::from(columns[column][row]);
        }
    }
    Matrix4d { rows }
}

fn affine_matrix(affine: AssetAffine) -> Matrix4d {
    Matrix4d {
        rows: [
            [
                f64::from(affine.scale),
                0.0,
                0.0,
                f64::from(affine.translation[0]),
            ],
            [
                0.0,
                f64::from(affine.scale),
                0.0,
                f64::from(affine.translation[1]),
            ],
            [
                0.0,
                0.0,
                f64::from(affine.scale),
                f64::from(affine.translation[2]),
            ],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

fn trs_matrix(translation: [f64; 3], rotation: [f64; 4], scale: [f64; 3]) -> Matrix4d {
    let magnitude = rotation
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    let [x, y, z, w] = if magnitude > 1.0e-15 && magnitude.is_finite() {
        rotation.map(|value| value / magnitude)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    let (xx, yy, zz) = (x * x, y * y, z * z);
    let (xy, xz, yz) = (x * y, x * z, y * z);
    let (xw, yw, zw) = (x * w, y * w, z * w);
    Matrix4d {
        rows: [
            [
                (1.0 - 2.0 * (yy + zz)) * scale[0],
                (2.0 * (xy - zw)) * scale[1],
                (2.0 * (xz + yw)) * scale[2],
                translation[0],
            ],
            [
                (2.0 * (xy + zw)) * scale[0],
                (1.0 - 2.0 * (xx + zz)) * scale[1],
                (2.0 * (yz - xw)) * scale[2],
                translation[1],
            ],
            [
                (2.0 * (xz - yw)) * scale[0],
                (2.0 * (yz + xw)) * scale[1],
                (1.0 - 2.0 * (xx + yy)) * scale[2],
                translation[2],
            ],
            [0.0, 0.0, 0.0, 1.0],
        ],
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // exact affine and time-code fixtures are part of the capture contract
mod tests {
    use std::cell::Cell;

    use super::*;
    use metrocalk_assets::{Material, Primitive};
    use metrocalk_core::Op;
    use metrocalk_gizmo::Transform as GizmoTransform;
    use metrocalk_skeleton::{Joint, Skeleton};

    use crate::kinematics::{set_joint_ops, JOINT_TRACK};

    fn triangle_asset() -> MeshAsset {
        MeshAsset {
            name: "triangle".into(),
            primitives: vec![Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: vec![[0.0, 0.0, 1.0]; 3],
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                tangents: vec![[1.0, 0.0, 0.0, 1.0]; 3],
                indices: vec![0, 1, 2],
                material: 0,
                ..Primitive::default()
            }],
            materials: vec![Material::default()],
            ..MeshAsset::default()
        }
    }

    fn set_field(entity: EntityId, component: &str, field: &str, value: FieldValue) -> Op {
        Op::SetField {
            entity,
            component: component.into(),
            field: field.into(),
            value,
        }
    }

    #[test]
    fn trs_and_column_major_conversion_keep_translation_scale_and_rotation() {
        let transform = GizmoTransform {
            translation: [2.0, -3.0, 4.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [2.0, 3.0, 4.0],
        };
        assert_eq!(
            matrix_from_columns(transform.to_matrix()),
            trs_matrix([2.0, -3.0, 4.0], [0.0, 0.0, 0.0, 1.0], [2.0, 3.0, 4.0])
        );
        assert_eq!(
            affine_matrix(AssetAffine {
                scale: 0.5,
                translation: [1.0, 2.0, 3.0]
            })
            .rows[0],
            [0.5, 0.0, 0.0, 1.0]
        );
    }

    #[test]
    fn capture_keeps_hierarchy_reuses_meshes_and_applies_the_viewport_affine_once_per_instance() {
        let mut engine = Engine::new(FlecsWorld::new(), 7);
        let parent = engine.alloc_entity_id();
        let child = engine.alloc_entity_id();
        engine
            .commit(
                "scene-fixture",
                vec![
                    Op::CreateEntity {
                        id: parent,
                        parent: None,
                    },
                    Op::CreateEntity {
                        id: child,
                        parent: Some(parent),
                    },
                    set_field(
                        parent,
                        "__meta__",
                        "name",
                        FieldValue::Str("Pump root".into()),
                    ),
                    set_field(
                        child,
                        "__meta__",
                        "name",
                        FieldValue::Str("Valve child".into()),
                    ),
                    set_field(parent, "Transform", "x", FieldValue::Number(3.0)),
                    set_field(child, "Transform", "y", FieldValue::Number(2.0)),
                    set_field(
                        parent,
                        "MeshRenderer",
                        MESH_FIELD,
                        FieldValue::Str("mesh:shared".into()),
                    ),
                    set_field(
                        child,
                        "MeshRenderer",
                        MESH_FIELD,
                        FieldValue::Str("mesh:shared".into()),
                    ),
                ],
            )
            .expect("fixture commit");
        let calls = Cell::new(0);
        let scene = capture_scene(&engine, "assembly", |handle| {
            assert_eq!(handle, "mesh:shared");
            calls.set(calls.get() + 1);
            Some(CapturedMesh {
                asset: triangle_asset(),
                display: AssetAffine {
                    scale: 0.25,
                    translation: [1.0, -2.0, 3.0],
                },
            })
        })
        .expect("capture");

        assert_eq!(calls.get(), 1, "content-addressed geometry resolves once");
        assert_eq!(scene.meshes.len(), 1);
        assert_eq!(
            scene.nodes.len(),
            4,
            "two entities plus two geometry children"
        );
        let parent_node = scene
            .nodes
            .iter()
            .find(|node| node.name.starts_with("Pump_root"))
            .expect("parent node");
        let child_node = scene
            .nodes
            .iter()
            .find(|node| node.name.starts_with("Valve_child"))
            .expect("child node");
        assert_eq!(child_node.parent, Some(parent_node.id));
        assert_eq!(parent_node.local_transform.rows[0][3], 3.0);
        assert_eq!(child_node.local_transform.rows[1][3], 2.0);
        let geometry_nodes: Vec<_> = scene
            .nodes
            .iter()
            .filter(|node| node.mesh.is_some())
            .collect();
        assert_eq!(geometry_nodes.len(), 2);
        assert!(geometry_nodes.iter().all(|node| node.local_transform
            == affine_matrix(AssetAffine {
                scale: 0.25,
                translation: [1.0, -2.0, 3.0],
            })));
    }

    #[test]
    fn capture_converts_mechanism_keys_and_imported_skin_to_standard_scene_channels() {
        let mut engine = Engine::new(FlecsWorld::new(), 9);
        let entity = engine.alloc_entity_id();
        let mut ops = vec![
            Op::CreateEntity {
                id: entity,
                parent: None,
            },
            set_field(
                entity,
                "MeshRenderer",
                MESH_FIELD,
                FieldValue::Str("mesh:rig".into()),
            ),
            set_field(
                entity,
                JOINT_TRACK,
                "keys",
                FieldValue::Str("0:0;1:0.5;1:0.75;2:1".into()),
            ),
        ];
        ops.extend(set_joint_ops(
            entity,
            true,
            [0.0, 0.0, 1.0],
            [0.0, 0.0, 0.0],
            (-1.5, 1.5),
            "manual",
        ));
        engine.commit("rig-fixture", ops).expect("fixture commit");

        let mut asset = triangle_asset();
        asset.primitives[0].joints = vec![[0, 0, 0, 0]; 3];
        asset.primitives[0].weights = vec![[1.0, 0.0, 0.0, 0.0]; 3];
        let identity = GizmoTransform {
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
        };
        asset.skeleton = Some(Skeleton {
            joints: vec![Joint {
                // The joint's own source name. `capture_scene` does NOT read it yet — it synthesizes
                // `Joint_{index}` for both the node and the `SceneJoint` (above, ~line 287) — because
                // `usd_export::validate_identifier` rejects the punctuation real rigs use
                // (`mixamorig:LeftForeArm` has a colon). Carrying a real name here keeps the fixture
                // honest about what an imported rig actually holds, and is the value the tracked
                // name-preserving export slice has to start reading.
                name: "root".into(),
                parent: None,
                local_bind: identity,
                inverse_bind: identity.to_matrix(),
            }],
        });
        let scene = capture_scene(&engine, "rigged", |_| {
            Some(CapturedMesh {
                asset: asset.clone(),
                display: AssetAffine::IDENTITY,
            })
        })
        .expect("capture");

        assert_eq!(scene.skins.len(), 1);
        assert_eq!(scene.skins[0].joints.len(), 1);
        assert_eq!(scene.animations.len(), 1);
        let channel = &scene.animations[0].channels[0];
        assert_eq!(channel.value_type, AnimationValueType::Matrix4d);
        assert_eq!(
            channel.samples.len(),
            3,
            "duplicate time keeps the last authored key"
        );
        assert_eq!(channel.samples[1].time_code, scene.time_codes_per_second);
    }
}
