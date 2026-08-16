//! USD-Physics import — OpenUSD (`.usda`) behind the [`Interchange`] trait via the pure-Rust `openusd`
//! crate (pre-1.0 ⇒ wrapped, invariant 5; no `openusd` type leaks out). The HIGH-VALUE deliverable is
//! **units interchange** (closing the M8.3 loop): USD declares `metersPerUnit` / `kilogramsPerUnit`, often
//! centimetres/grams — we read them, **carry real units into the scene**, and feed the M8.3 scale check
//! ground truth so the classic scale-explosion footgun is caught **at import**, explained, reconciled.
//!
//! Scope (honest): reads stage units + the rigid-body / collision prims (Cube/Sphere/Capsule/Cylinder
//! geometry + `xformOp:translate` + `physics:mass`) → our neutral [`SceneImport`], units-reconciled.
//! `PhysicsJoint` mapping, binary `.usdc`/`.usdz`, and full composition (references/variants/layers) are
//! **explained seams** (URDF carries articulated mechanisms today). `Stage::open` is file-based, so this
//! is a native action (the ADR-006 wasm boundary).

use std::sync::atomic::{AtomicU64, Ordering};

use metrocalk_physics::{BodyKind, ColliderDesc, ColliderShape};

use crate::{
    ImportedBody, Interchange, InterchangeError, SceneImport, Units, UnsupportedNote, Vec3,
};

/// The USD-Physics importer ([`Interchange`] impl) — stateless.
pub struct UsdInterchange;

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

impl Interchange for UsdInterchange {
    fn format(&self) -> &'static str {
        "USD-Physics"
    }

    fn import(&self, source: &[u8]) -> Result<SceneImport, InterchangeError> {
        let text = std::str::from_utf8(source).map_err(|e| {
            InterchangeError::Parse(format!("USD is not UTF-8 (.usda text only): {e}"))
        })?;
        // openusd's Stage opens from a path; stage the bytes to a unique temp `.usda` (native action).
        let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
        let tmp =
            std::env::temp_dir().join(format!("metrocalk_usd_{}_{seq}.usda", std::process::id()));
        std::fs::write(&tmp, text)
            .map_err(|e| InterchangeError::Parse(format!("could not stage USD: {e}")))?;
        let result = import_stage(tmp.to_string_lossy().as_ref());
        let _ = std::fs::remove_file(&tmp);
        result
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "one linear read of a stage: units, then the prim walk, then the never-silent notes -               splitting it would separate a drop from the place that decided to drop it"
)]
fn import_stage(path: &str) -> Result<SceneImport, InterchangeError> {
    use openusd::usd::{PrimPredicate, Stage};
    let stage = Stage::open(path).map_err(|e| InterchangeError::Parse(format!("{e}")))?;

    let mut notes = Vec::new();

    // Stage units (the deliverable-2 ground truth). Default SI when unauthored.
    let meters_per_unit = stage_f64(&stage, "metersPerUnit").unwrap_or(1.0);
    let kilograms_per_unit = stage_f64(&stage, "kilogramsPerUnit").unwrap_or(1.0);
    let units = Units {
        meters_per_unit,
        kilograms_per_unit,
    };
    if units.needs_reconciliation() {
        notes.push(UnsupportedNote {
            feature: format!(
                "USD authored at metersPerUnit={meters_per_unit}, kilogramsPerUnit={kilograms_per_unit}"
            ),
            detail: "converting to the scene's SI metres/kilograms — the M8.3 scale check confirms this; an unreconciled unit mismatch is the classic sim-explosion footgun"
                .into(),
        });
    }

    let mut paths = Vec::new();
    stage
        .traverse(PrimPredicate::DEFAULT, |p| paths.push(p.clone()))
        .map_err(|e| InterchangeError::Parse(format!("traverse: {e}")))?;

    let mut bodies: Vec<ImportedBody> = Vec::new();
    let mut joint_seen = false;
    // Transform ops seen but not applied, and prims whose non-unit scale was dropped. Both are
    // reported once at the end rather than per prim, so a thousand-prim stage does not bury the rest
    // of the report.
    let mut unsupported_ops: Vec<String> = Vec::new();
    let mut scaled_prims: Vec<String> = Vec::new();
    for path in &paths {
        let prim = stage.prim_at(path.clone());
        let type_name = prim.type_name().ok().flatten().unwrap_or_default();
        if type_name.contains("Joint") {
            joint_seen = true;
            continue;
        }
        let is_rigid = prim.has_api_schema("PhysicsRigidBodyAPI").unwrap_or(false);
        let is_collision = prim.has_api_schema("PhysicsCollisionAPI").unwrap_or(false);
        if !is_rigid && !is_collision {
            continue;
        }
        let name = prim
            .path()
            .as_str()
            .rsplit('/')
            .next()
            .unwrap_or("prim")
            .to_string();
        // A rigid body that isn't disabled is Dynamic; a bare collision prim is static world geometry.
        let enabled = attr_bool(&prim, "physics:rigidBodyEnabled").unwrap_or(true);
        let kind = if is_rigid && enabled {
            BodyKind::Dynamic
        } else {
            BodyKind::Fixed
        };
        // The WORLD pose, composed down the ancestor chain.
        //
        // This used to read only the prim's OWN `xformOp:translate` and hard-code the rotation to
        // identity. Both are wrong in the same silent way: a body under a translated or rotated `Xform`
        // - which is how essentially every real USD stage is organised - landed at the wrong place, at
        // the wrong angle, with no note. A wrong pose that reports success is worse than a refusal,
        // and it directly contradicted this module's own never-silent claim.
        let (translation, rotation, dropped_scale) =
            world_pose(&stage, path, meters_per_unit, &mut unsupported_ops);
        let mass = attr_f64(&prim, "physics:mass")
            .map(|m| m * kilograms_per_unit)
            .filter(|m| *m > 0.0);
        let collider = collider_of(&prim, &type_name, meters_per_unit, &name, &mut notes);
        if dropped_scale {
            scaled_prims.push(name.clone());
        }
        bodies.push(ImportedBody {
            name,
            kind,
            translation,
            rotation,
            mass,
            collider,
        });
    }

    // The transform reader's SCOPE, stated once, always. A limit that can only be reported when it is
    // detected is not reported at all in the case that matters most — a matrix op this crate cannot
    // read. Saying what the pose IS composed from lets a reader work out what it is not.
    notes.push(UnsupportedNote {
        feature: "transform scope".into(),
        detail: "world poses are composed from xformOp:translate, xformOp:orient /                  xformOp:rotateXYZ, and the full ancestor chain. A 4x4 xformOp:transform is NOT                  applied — this reader cannot read a matrix value — so a stage authored with matrix                  ops will place bodies at their ancestors' poses only."
            .into(),
    });
    if !unsupported_ops.is_empty() {
        unsupported_ops.sort_unstable();
        unsupported_ops.dedup();
        notes.push(UnsupportedNote {
            feature: "xformOp".into(),
            detail: format!(
                "these transform ops were seen and NOT applied: {} - the pose you get is composed from \
                 translate, orient/rotate and the ancestor chain only",
                unsupported_ops.join(", ")
            ),
        });
    }
    if !scaled_prims.is_empty() {
        scaled_prims.sort_unstable();
        scaled_prims.dedup();
        let shown: Vec<&str> = scaled_prims.iter().take(8).map(String::as_str).collect();
        notes.push(UnsupportedNote {
            feature: "xformOp:scale".into(),
            detail: format!(
                "{} prim(s) carry a non-unit scale that a rigid body cannot hold ({}{}) - the collider \
                 is the authored size, unscaled",
                scaled_prims.len(),
                shown.join(", "),
                if scaled_prims.len() > shown.len() { ", ..." } else { "" }
            ),
        });
    }

    if bodies.is_empty() {
        return Err(InterchangeError::Empty(
            "USD declares no PhysicsRigidBodyAPI / PhysicsCollisionAPI prims".into(),
        ));
    }
    if joint_seen {
        notes.push(UnsupportedNote {
            feature: "USD PhysicsJoint prims present".into(),
            detail: "USD joint mapping (body0/body1 rels + local frames) isn't wired yet — bodies imported, joints declined; use URDF for articulated mechanisms (M8.5+)"
                .into(),
        });
    }
    notes.push(UnsupportedNote {
        feature: "USD scope".into(),
        detail: "binary .usdc/.usdz + full composition (references/variants/layers) read via the openusd crate are a documented seam; .usda physics + units import is real"
            .into(),
    });

    Ok(SceneImport {
        name: "usd_scene".into(),
        format: "USD-Physics".into(),
        units,
        bodies,
        joints: Vec::new(),
        notes,
    })
}

/// Map a USD geometry prim to our collider, scaling by `metersPerUnit`, noting approximations.
fn collider_of(
    prim: &openusd::usd::Prim,
    type_name: &str,
    mpu: f64,
    name: &str,
    notes: &mut Vec<UnsupportedNote>,
) -> Option<ColliderDesc> {
    let shape = match type_name {
        "Cube" => {
            // USD Cube has a single `size` (full edge length); UsdPhysics treats it as an axis box.
            let s = attr_f64(prim, "size").unwrap_or(2.0) * mpu * 0.5;
            Some(ColliderShape::Cuboid {
                half_extents: [s, s, s],
            })
        }
        "Sphere" => Some(ColliderShape::Ball {
            radius: attr_f64(prim, "radius").unwrap_or(1.0) * mpu,
        }),
        "Capsule" => Some(ColliderShape::Capsule {
            half_height: attr_f64(prim, "height").unwrap_or(1.0) * mpu * 0.5,
            radius: attr_f64(prim, "radius").unwrap_or(0.5) * mpu,
        }),
        "Cylinder" => {
            notes.push(UnsupportedNote {
                feature: format!("prim '{name}' is a USD Cylinder collider"),
                detail: "no cylinder primitive in the collider enum — approximated as a capsule"
                    .into(),
            });
            Some(ColliderShape::Capsule {
                half_height: attr_f64(prim, "height").unwrap_or(1.0) * mpu * 0.5,
                radius: attr_f64(prim, "radius").unwrap_or(0.5) * mpu,
            })
        }
        "Mesh" => {
            notes.push(UnsupportedNote {
                feature: format!("prim '{name}' is a USD Mesh collider"),
                detail: "mesh colliders resolve through the asset pipeline (M4) — derive a convex hull (M8.3) once imported"
                    .into(),
            });
            None
        }
        other => {
            notes.push(UnsupportedNote {
                feature: format!("prim '{name}' has unmapped geometry '{other}'"),
                detail: "no collider mapped — declined (no silent approximation)".into(),
            });
            None
        }
    };
    shape.map(ColliderDesc::new)
}

// ── openusd value readers (the foreign-type firewall: openusd types stay inside these helpers) ───────

/// The world pose of `path`, composed from its own transform ops and every ancestor's.
///
/// USD's transform stack is `xformOpOrder`, a list naming which ops apply and in what sequence. This
/// reads the ops it can honour - `translate`, `orient` (a quaternion), `rotateXYZ` (Euler degrees) -
/// and RECORDS the name of any other op it saw, so a matrix or a pivot op is reported rather than
/// quietly ignored. Scale is read separately: a rigid body has no scale, so a non-unit scale is a
/// reported drop rather than a silent one.
///
/// Returns `(translation_in_metres, rotation_quat_xyzw, dropped_non_unit_scale)`.
fn world_pose(
    stage: &openusd::usd::Stage,
    path: &openusd::sdf::Path,
    meters_per_unit: f64,
    unsupported_ops: &mut Vec<String>,
) -> ([f64; 3], [f64; 4], bool) {
    // Walk up to the root, collecting each ancestor's local transform, then compose root-first.
    let mut chain: Vec<openusd::sdf::Path> = Vec::new();
    let mut cursor = Some(path.clone());
    let mut depth = 0usize;
    while let Some(p) = cursor {
        chain.push(p.clone());
        depth += 1;
        // A malformed stage must not spin. 256 matches the parser's own nesting guard.
        if depth > 256 {
            unsupported_ops.push("an ancestor chain deeper than 256".into());
            break;
        }
        cursor = parent_path(&p);
    }
    chain.reverse();

    let (mut t, mut q) = ([0.0f64; 3], [0.0f64, 0.0, 0.0, 1.0]);
    let mut dropped_scale = false;
    for p in &chain {
        let prim = stage.prim_at(p.clone());
        let (lt, lq, ls) = local_pose(&prim, unsupported_ops);
        if ls {
            dropped_scale = true;
        }
        // world = world * local: rotate the child's offset into the parent's frame first.
        let rotated = quat_rotate(q, lt);
        t = [t[0] + rotated[0], t[1] + rotated[1], t[2] + rotated[2]];
        q = quat_mul(q, lq);
    }
    (
        [
            t[0] * meters_per_unit,
            t[1] * meters_per_unit,
            t[2] * meters_per_unit,
        ],
        normalize_quat(q),
        dropped_scale,
    )
}

/// One prim's own transform: translation, rotation, and whether a non-unit scale was dropped.
fn local_pose(
    prim: &openusd::usd::Prim,
    unsupported_ops: &mut Vec<String>,
) -> ([f64; 3], [f64; 4], bool) {
    let t = attr_vec3(prim, "xformOp:translate").unwrap_or([0.0; 3]);

    // `orient` is authored as a quaternion; USD writes it WXYZ while this engine uses XYZW.
    let q = if let Some(w) = attr_quat(prim, "xformOp:orient") {
        w
    } else if let Some(e) = attr_vec3(prim, "xformOp:rotateXYZ") {
        euler_xyz_degrees_to_quat(e)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };

    let scale = attr_vec3(prim, "xformOp:scale");
    let dropped_scale = scale.is_some_and(|s| {
        (s[0] - 1.0).abs() > 1e-9 || (s[1] - 1.0).abs() > 1e-9 || (s[2] - 1.0).abs() > 1e-9
    });

    // Any op this function does not apply is NAMED, not ignored.
    unsupported_ops.extend(unapplied_ops(prim));
    (t, q, dropped_scale)
}

/// The transform ops this prim authors that `local_pose` does NOT apply.
///
/// USD names the active ops in `xformOpOrder`, a token array the Rust crate does not expose a typed
/// reader for. Probing for the op ATTRIBUTES by name is the reliable alternative: an op that is
/// present but absent from the order list is inert in USD too, so at worst this over-reports, and
/// over-reporting a note is the right side to err on when the alternative is a silently wrong pose.
fn unapplied_ops(prim: &openusd::usd::Prim) -> Vec<String> {
    const PIVOTS: [&str; 2] = ["xformOp:translate:pivot", "xformOp:scale:pivot"];
    const OTHER_EULER: [&str; 8] = [
        "xformOp:rotateXZY",
        "xformOp:rotateYXZ",
        "xformOp:rotateYZX",
        "xformOp:rotateZXY",
        "xformOp:rotateZYX",
        "xformOp:rotateX",
        "xformOp:rotateY",
        "xformOp:rotateZ",
    ];
    let mut found = Vec::new();
    // `xformOp:transform` (a 4x4 matrix op) is deliberately NOT probed here: the USD crate exposes no
    // reader for a matrix value, so its presence cannot be detected per prim. It is covered instead by
    // the unconditional scope note below, which states exactly which ops the pose is composed from —
    // the honest way to report a limit you cannot detect case by case.
    for name in PIVOTS {
        if attr_vec3(prim, name).is_some() {
            found.push(name.to_string());
        }
    }
    for name in OTHER_EULER {
        // The three-axis forms carry a vec3; the single-axis forms carry a scalar.
        if attr_vec3(prim, name).is_some() || attr_f64(prim, name).is_some() {
            found.push(name.to_string());
        }
    }
    found
}

/// A quaternion attribute, converted from USD's WXYZ order to this engine's XYZW.
fn attr_quat(prim: &openusd::usd::Prim, name: &str) -> Option<[f64; 4]> {
    if let Ok(Some(v)) = prim.attribute(name).get::<[f64; 4]>() {
        return Some([v[1], v[2], v[3], v[0]]);
    }
    if let Ok(Some(v)) = prim.attribute(name).get::<[f32; 4]>() {
        return Some([
            f64::from(v[1]),
            f64::from(v[2]),
            f64::from(v[3]),
            f64::from(v[0]),
        ]);
    }
    None
}

/// The parent of a USD path, or `None` at the pseudo-root.
fn parent_path(path: &openusd::sdf::Path) -> Option<openusd::sdf::Path> {
    let s = path.as_str();
    let cut = s.rfind('/')?;
    if cut == 0 {
        return None;
    }
    openusd::sdf::Path::new(&s[..cut]).ok()
}

/// USD `rotateXYZ` is degrees, applied X then Y then Z.
fn euler_xyz_degrees_to_quat(e: [f64; 3]) -> [f64; 4] {
    let half = |d: f64| (d.to_radians()) * 0.5;
    let (sx, cx) = half(e[0]).sin_cos();
    let (sy, cy) = half(e[1]).sin_cos();
    let (sz, cz) = half(e[2]).sin_cos();
    let qx = [sx, 0.0, 0.0, cx];
    let qy = [0.0, sy, 0.0, cy];
    let qz = [0.0, 0.0, sz, cz];
    quat_mul(quat_mul(qz, qy), qx)
}

/// Hamilton product, XYZW.
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}

/// Rotate a vector by a unit quaternion: `v + 2q_w(q_v x v) + 2 q_v x (q_v x v)`.
fn quat_rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [q[0], q[1], q[2]];
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v);
    let t2 = cross(u, t);
    [
        v[0] + 2.0 * (q[3] * t[0] + t2[0]),
        v[1] + 2.0 * (q[3] * t[1] + t2[1]),
        v[2] + 2.0 * (q[3] * t[2] + t2[2]),
    ]
}

/// Guard against a denormal composition drifting off the unit sphere.
fn normalize_quat(q: [f64; 4]) -> [f64; 4] {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    if !n.is_finite() || n < 1e-12 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
}

fn stage_f64(stage: &openusd::usd::Stage, field: &str) -> Option<f64> {
    let v = stage.stage_metadata(field).ok().flatten()?;
    value_to_f64(&v)
}

fn attr_f64(prim: &openusd::usd::Prim, name: &str) -> Option<f64> {
    if let Ok(Some(v)) = prim.attribute(name).get::<f64>() {
        return Some(v);
    }
    if let Ok(Some(v)) = prim.attribute(name).get::<f32>() {
        return Some(f64::from(v));
    }
    None
}

fn attr_bool(prim: &openusd::usd::Prim, name: &str) -> Option<bool> {
    prim.attribute(name).get::<bool>().ok().flatten()
}

fn attr_vec3(prim: &openusd::usd::Prim, name: &str) -> Option<Vec3> {
    if let Ok(Some(v)) = prim.attribute(name).get::<[f64; 3]>() {
        return Some(v);
    }
    if let Ok(Some(v)) = prim.attribute(name).get::<[f32; 3]>() {
        return Some([f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]);
    }
    None
}

/// Extract an f64 from an `sdf::Value` (USD metadata is `double`/`float`).
fn value_to_f64(v: &openusd::sdf::Value) -> Option<f64> {
    if let Ok(d) = v.clone().try_into() as Result<f64, _> {
        return Some(d);
    }
    if let Ok(f) = v.clone().try_into() as Result<f32, _> {
        return Some(f64::from(f));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // A minimal UsdPhysics scene authored in CENTIMETRES (the classic footgun): a static ground cube + a
    // dynamic sphere. The importer must read the units, reconcile to metres, and import both bodies.
    const SCENE: &str = r#"#usda 1.0
(
    metersPerUnit = 0.01
    kilogramsPerUnit = 1.0
    upAxis = "Y"
)

def Xform "World"
{
    def Cube "ground" (
        prepend apiSchemas = ["PhysicsCollisionAPI"]
    )
    {
        double size = 100
        double3 xformOp:translate = (0, 0, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
    }

    def Sphere "ball" (
        prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI", "PhysicsMassAPI"]
    )
    {
        double radius = 20
        double3 xformOp:translate = (0, 300, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]
        float physics:mass = 2.0
    }
}
"#;

    // A body parented under a TRANSLATED and ROTATED Xform - which is how essentially every real USD
    // stage is organised. Before the world-pose composition landed, this imported at the child's own
    // local offset with an identity rotation: the wrong place, the wrong angle, and no note about it.
    const NESTED: &str = r#"#usda 1.0
(
    metersPerUnit = 1.0
    kilogramsPerUnit = 1.0
    upAxis = "Y"
)

def Xform "Rig"
{
    double3 xformOp:translate = (10, 0, 0)
    double3 xformOp:rotateXYZ = (0, 90, 0)
    uniform token[] xformOpOrder = ["xformOp:translate", "xformOp:rotateXYZ"]

    def Xform "Arm"
    {
        double3 xformOp:translate = (0, 5, 0)
        uniform token[] xformOpOrder = ["xformOp:translate"]

        def Sphere "hand" (
            prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"]
        )
        {
            double radius = 0.5
            double3 xformOp:translate = (2, 0, 0)
            uniform token[] xformOpOrder = ["xformOp:translate"]
        }
    }
}
"#;

    #[test]
    fn a_body_under_a_moved_and_rotated_parent_lands_where_usd_says() {
        let scene = UsdInterchange
            .import(NESTED.as_bytes())
            .expect("imports the nested stage");
        let hand = scene
            .bodies
            .iter()
            .find(|b| b.name == "hand")
            .expect("the hand is imported");

        // Rig sits at x=10 and is yawed 90 deg about Y. Arm adds (0,5,0), which the yaw leaves alone.
        // The hand's own (2,0,0) is rotated by that yaw into -Z. So the world pose is (10, 5, -2).
        let t = hand.translation;
        assert!(
            (t[0] - 10.0).abs() < 1e-6 && (t[1] - 5.0).abs() < 1e-6 && (t[2] + 2.0).abs() < 1e-6,
            "expected the composed world pose (10, 5, -2), got {t:?}"
        );

        // And the rotation is the parent's, not a hard-coded identity.
        let q = hand.rotation;
        let half = std::f64::consts::FRAC_PI_4; // 90 deg / 2
        assert!(
            (q[1].abs() - half.sin()).abs() < 1e-6,
            "expected the inherited 90 deg yaw, got {q:?}"
        );
    }

    #[test]
    fn the_transform_reader_always_states_its_own_scope() {
        // A limit that is only reported when detected is not reported at all in the case that matters
        // most - a matrix op this crate cannot read. So the scope is stated on every import.
        let scene = UsdInterchange.import(NESTED.as_bytes()).expect("imports");
        let scope = scene
            .notes
            .iter()
            .find(|n| n.feature == "transform scope")
            .expect("every import states the transform scope");
        assert!(
            scope.detail.contains("xformOp:transform"),
            "{:?}",
            scope.detail
        );
        assert!(scope.detail.contains("ancestor"), "{:?}", scope.detail);
    }

    #[test]
    fn imports_usd_physics_with_unit_reconciliation() {
        let scene = UsdInterchange.import(SCENE.as_bytes()).unwrap();
        assert_eq!(scene.format, "USD-Physics");
        // Units read + flagged for reconciliation (cm → m).
        assert!((scene.units.meters_per_unit - 0.01).abs() < 1e-12);
        assert!(scene.units.needs_reconciliation());
        assert!(
            scene
                .notes
                .iter()
                .any(|n| n.feature.contains("metersPerUnit")),
            "the cm→m reconciliation is explained"
        );

        assert_eq!(scene.bodies.len(), 2, "ground + ball");
        let ball = scene.bodies.iter().find(|b| b.name == "ball").unwrap();
        assert_eq!(ball.kind, BodyKind::Dynamic);
        // 300 cm → 3 m, mass 2 kg, radius 20 cm → 0.2 m.
        assert!(
            (ball.translation[1] - 3.0).abs() < 1e-9,
            "300 cm reconciled to 3 m"
        );
        assert_eq!(ball.mass, Some(2.0));
        assert!(matches!(
            ball.collider.as_ref().unwrap().shape,
            ColliderShape::Ball { radius } if (radius - 0.2).abs() < 1e-9
        ));

        let ground = scene.bodies.iter().find(|b| b.name == "ground").unwrap();
        assert_eq!(
            ground.kind,
            BodyKind::Fixed,
            "a bare collision prim is static"
        );
    }
}
