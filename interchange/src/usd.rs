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
        let t = attr_vec3(&prim, "xformOp:translate").unwrap_or([0.0; 3]);
        let translation = [
            t[0] * meters_per_unit,
            t[1] * meters_per_unit,
            t[2] * meters_per_unit,
        ];
        let mass = attr_f64(&prim, "physics:mass")
            .map(|m| m * kilograms_per_unit)
            .filter(|m| *m > 0.0);
        let collider = collider_of(&prim, &type_name, meters_per_unit, &name, &mut notes);
        bodies.push(ImportedBody {
            name,
            kind,
            translation,
            rotation: [0.0, 0.0, 0.0, 1.0],
            mass,
            collider,
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
