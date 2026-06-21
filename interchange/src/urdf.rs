//! URDF (Unified Robot Description Format) import — the M8.5 "lead with it, real now" piece. `urdf-rs`
//! (OpenRR, the standard Rust parser) reads the XML; we map links → bodies (with **forward-kinematics**
//! world poses, since URDF poses live on the joint tree), joints → our [`JointDesc`], and collision
//! geometry → our open collider enum — recording **every** approximation as a note. URDF is SI (metres,
//! kilograms). No `urdf_rs` type crosses back out of this module (invariant 5).

use std::collections::HashMap;

use metrocalk_physics::{BodyKind, ColliderDesc, ColliderShape, JointDesc};

use crate::{
    rpy_to_quat, ImportedBody, ImportedJoint, Interchange, InterchangeError, Pose, SceneImport,
    Units, UnsupportedNote,
};

/// The URDF importer ([`Interchange`] impl) — stateless.
pub struct UrdfInterchange;

impl Interchange for UrdfInterchange {
    fn format(&self) -> &'static str {
        "URDF"
    }

    fn import(&self, source: &[u8]) -> Result<SceneImport, InterchangeError> {
        let text = std::str::from_utf8(source)
            .map_err(|e| InterchangeError::Parse(format!("URDF is not UTF-8: {e}")))?;
        let robot =
            urdf_rs::read_from_string(text).map_err(|e| InterchangeError::Parse(format!("{e}")))?;
        if robot.links.is_empty() {
            return Err(InterchangeError::Empty("URDF declares no links".into()));
        }
        let mut notes = Vec::new();

        let index: HashMap<&str, usize> = robot
            .links
            .iter()
            .enumerate()
            .map(|(i, link)| (link.name.as_str(), i))
            .collect();

        // The ROOT link is the one that is no joint's child — the base. We anchor it (Fixed) so an imported
        // arm STANDS (the success criterion) rather than collapsing; every other link is Dynamic.
        let mut is_child = vec![false; robot.links.len()];
        for jt in &robot.joints {
            if let Some(&ci) = index.get(jt.child.link.as_str()) {
                is_child[ci] = true;
            }
        }

        // Forward kinematics: a world pose per link, walked from the root(s) through the joint origins
        // (URDF stores no absolute link pose — it's defined by the kinematic tree). Iterate to a fixed
        // point so joint order doesn't matter (a tree resolves in ≤ depth passes).
        let mut world: Vec<Option<Pose>> = vec![None; robot.links.len()];
        for (i, anchored) in is_child.iter().enumerate() {
            if !anchored {
                world[i] = Some(Pose::IDENTITY);
            }
        }
        for _ in 0..robot.links.len() {
            let mut progressed = false;
            for jt in &robot.joints {
                let (Some(&pi), Some(&ci)) = (
                    index.get(jt.parent.link.as_str()),
                    index.get(jt.child.link.as_str()),
                ) else {
                    continue;
                };
                if world[ci].is_none() {
                    if let Some(parent_pose) = world[pi] {
                        let origin = &jt.origin;
                        world[ci] = Some(parent_pose.compose(
                            [origin.xyz[0], origin.xyz[1], origin.xyz[2]],
                            rpy_to_quat([origin.rpy[0], origin.rpy[1], origin.rpy[2]]),
                        ));
                        progressed = true;
                    }
                }
            }
            if !progressed {
                break;
            }
        }

        let bodies: Vec<ImportedBody> = robot
            .links
            .iter()
            .enumerate()
            .map(|(i, link)| {
                let pose = world[i].unwrap_or(Pose::IDENTITY);
                let kind = if is_child[i] {
                    BodyKind::Dynamic
                } else {
                    BodyKind::Fixed
                };
                let mass = Some(link.inertial.mass.value).filter(|kg| *kg > 0.0);
                ImportedBody {
                    name: link.name.clone(),
                    kind,
                    translation: pose.t,
                    rotation: pose.q,
                    mass,
                    collider: collider_of(link, &mut notes),
                }
            })
            .collect();

        let joints: Vec<ImportedJoint> = robot
            .joints
            .iter()
            .filter_map(|jt| {
                let pi = *index.get(jt.parent.link.as_str())?;
                let ci = *index.get(jt.child.link.as_str())?;
                map_joint(jt, pi, ci, &mut notes)
            })
            .collect();

        Ok(SceneImport {
            name: robot.name.clone(),
            format: "URDF".into(),
            units: Units::SI,
            bodies,
            joints,
            notes,
        })
    }
}

/// Map a link's (first) collision geometry to our open collider enum, noting every approximation.
fn collider_of(link: &urdf_rs::Link, notes: &mut Vec<UnsupportedNote>) -> Option<ColliderDesc> {
    let col = link.collision.first()?;
    if link.collision.len() > 1 {
        notes.push(UnsupportedNote {
            feature: format!("link '{}' has {} collision shapes", link.name, link.collision.len()),
            detail: "the registry Collider is single-shape — using the first; compound colliders are M8.5+"
                .into(),
        });
    }
    let o = &col.origin;
    let nonzero = |v: &urdf_rs::Vec3| v[0] != 0.0 || v[1] != 0.0 || v[2] != 0.0;
    if nonzero(&o.xyz) || nonzero(&o.rpy) {
        notes.push(UnsupportedNote {
            feature: format!("link '{}' collider has a local offset", link.name),
            detail: "the current Collider attaches at the body origin — the local offset is dropped (sub-collider frames are M8.5+)"
                .into(),
        });
    }
    let shape = match &col.geometry {
        urdf_rs::Geometry::Box { size } => Some(ColliderShape::Cuboid {
            half_extents: [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5],
        }),
        urdf_rs::Geometry::Sphere { radius } => Some(ColliderShape::Ball { radius: *radius }),
        urdf_rs::Geometry::Capsule { radius, length } => Some(ColliderShape::Capsule {
            half_height: length * 0.5,
            radius: *radius,
        }),
        urdf_rs::Geometry::Cylinder { radius, length } => {
            notes.push(UnsupportedNote {
                feature: format!("link '{}' uses a cylinder collider", link.name),
                detail: "no cylinder primitive in the collider enum — approximated as a capsule (rounded ends)"
                    .into(),
            });
            Some(ColliderShape::Capsule {
                half_height: length * 0.5,
                radius: *radius,
            })
        }
        urdf_rs::Geometry::Mesh { filename, .. } => {
            notes.push(UnsupportedNote {
                feature: format!("link '{}' uses a mesh collider '{filename}'", link.name),
                detail: "mesh colliders resolve through the asset pipeline (M4) — derive a convex hull (M8.3) once imported"
                    .into(),
            });
            None
        }
    };
    shape.map(ColliderDesc::new)
}

/// Map a URDF joint to our [`JointDesc`], noting unsupported types + unenforced limits.
fn map_joint(
    j: &urdf_rs::Joint,
    parent: usize,
    child: usize,
    notes: &mut Vec<UnsupportedNote>,
) -> Option<ImportedJoint> {
    // The joint frame sits at `origin` on the parent; the child link frame coincides with it.
    let anchor_a = [j.origin.xyz[0], j.origin.xyz[1], j.origin.xyz[2]];
    let anchor_b = [0.0; 3];
    let axis = [j.axis.xyz[0], j.axis.xyz[1], j.axis.xyz[2]];
    let limit = Some((j.limit.lower, j.limit.upper)).filter(|(l, u)| *l != 0.0 || *u != 0.0);

    let joint = match j.joint_type {
        urdf_rs::JointType::Revolute | urdf_rs::JointType::Continuous => {
            if limit.is_some() {
                notes.push(UnsupportedNote {
                    feature: format!("joint '{}' has angular limits", j.name),
                    detail: "limits recorded for provenance but not yet enforced by the joint solver (M8.5+) — motion is unclamped"
                        .into(),
                });
            }
            JointDesc::Revolute {
                axis,
                anchor_a,
                anchor_b,
            }
        }
        urdf_rs::JointType::Fixed => JointDesc::Fixed { anchor_a, anchor_b },
        urdf_rs::JointType::Prismatic => {
            notes.push(UnsupportedNote {
                feature: format!("joint '{}' is prismatic (sliding)", j.name),
                detail: "no prismatic joint in the current model — declined (the child stays free); a powered prismatic constraint is M8.5+"
                    .into(),
            });
            return None;
        }
        urdf_rs::JointType::Floating => {
            notes.push(UnsupportedNote {
                feature: format!("joint '{}' is floating (6-DoF)", j.name),
                detail: "a floating joint adds no constraint — the child is already a free dynamic body; declined"
                    .into(),
            });
            return None;
        }
        urdf_rs::JointType::Planar => {
            notes.push(UnsupportedNote {
                feature: format!("joint '{}' is planar", j.name),
                detail: "no planar joint in the current model — declined; approximate with a prismatic pair (M8.5+)"
                    .into(),
            });
            return None;
        }
        urdf_rs::JointType::Spherical => JointDesc::Spherical { anchor_a, anchor_b },
    };
    Some(ImportedJoint {
        name: j.name.clone(),
        parent,
        child,
        joint,
        limit,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal 2-link arm: a fixed base + an upper arm on a revolute joint with limits, the base carrying
    // a box collider and the arm a cylinder (→ capsule, noted). Enough to exercise FK, root-anchoring,
    // joint mapping, limit-noting, and shape approximation.
    const ARM: &str = r#"
<robot name="test_arm">
  <link name="base">
    <inertial><mass value="5.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><box size="0.4 0.4 0.2"/></geometry></collision>
  </link>
  <link name="upper">
    <inertial><mass value="2.0"/><inertia ixx="1" ixy="0" ixz="0" iyy="1" iyz="0" izz="1"/></inertial>
    <collision><geometry><cylinder radius="0.1" length="0.8"/></geometry></collision>
  </link>
  <joint name="shoulder" type="revolute">
    <parent link="base"/>
    <child link="upper"/>
    <origin xyz="0 0 0.6" rpy="0 0 0"/>
    <axis xyz="0 1 0"/>
    <limit lower="-1.57" upper="1.57" effort="100" velocity="1"/>
  </joint>
</robot>
"#;

    #[test]
    fn imports_an_arm_into_neutral_components() {
        let scene = UrdfInterchange.import(ARM.as_bytes()).unwrap();
        assert_eq!(scene.format, "URDF");
        assert_eq!(scene.units, Units::SI);
        assert!(!scene.units.needs_reconciliation());
        assert_eq!(scene.bodies.len(), 2, "two links → two bodies");

        // The base is the ROOT (no joint's child) → anchored Fixed so the arm stands.
        let base = scene.bodies.iter().find(|b| b.name == "base").unwrap();
        assert_eq!(base.kind, BodyKind::Fixed);
        assert_eq!(base.mass, Some(5.0));
        assert!(matches!(
            base.collider.as_ref().unwrap().shape,
            ColliderShape::Cuboid { half_extents } if (half_extents[0] - 0.2).abs() < 1e-9
        ));

        // The upper arm is Dynamic, FK-placed at the joint origin (z = 0.6), cylinder → capsule.
        let upper = scene.bodies.iter().find(|b| b.name == "upper").unwrap();
        assert_eq!(upper.kind, BodyKind::Dynamic);
        assert!(
            (upper.translation[2] - 0.6).abs() < 1e-9,
            "FK placed it at z=0.6"
        );
        assert!(matches!(
            upper.collider.as_ref().unwrap().shape,
            ColliderShape::Capsule { .. }
        ));

        // One revolute joint, base → upper, with its limit recorded.
        assert_eq!(scene.joints.len(), 1);
        let jt = &scene.joints[0];
        assert!(matches!(jt.joint, JointDesc::Revolute { .. }));
        assert_eq!(jt.limit, Some((-1.57, 1.57)));

        // The approximations are EXPLAINED, not silent: the cylinder→capsule + the unenforced limit.
        assert!(scene.notes.iter().any(|n| n.feature.contains("cylinder")));
        assert!(scene.notes.iter().any(|n| n.feature.contains("limit")));
    }

    #[test]
    fn malformed_urdf_is_an_explained_error_not_a_panic() {
        let err = UrdfInterchange.import(b"<robot>not closed").unwrap_err();
        assert!(matches!(err, InterchangeError::Parse(_)));
    }
}
