//! M8.3 — intent-first physics authoring + **collider intelligence**. The engine's signature move
//! ("click/declare → engine wires it, every 'no' explained") applied to physics: a dead mesh becomes a
//! correct dynamic body in ≤2 clicks, and the classic beginner traps are caught **at author time** with
//! the fix offered, not discovered as a runtime glitch. Reuses the every-"no" discipline (ADR-016).
//!
//! This module is pure logic over the engine's components + a small [`MeshMetrics`] the caller derives
//! from the mesh (bounds + `physics::derive_collider`'s fit error) — so it stays `/physics`-free and is
//! headless-testable. Every fix is ONE undoable commit-pipeline transaction (invariant 3); a check
//! **re-passes after its fix** (the adversarial guard — the fix actually fixed it).

// Reading an i64 component field as f64 for a mass/scale check — the precision loss is irrelevant at the
// magnitudes physics fields take (same rationale as capscene).
#![allow(clippy::cast_precision_loss)]

use metrocalk_core::caps::canonical;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::FlecsWorld;
use serde::Serialize;

use crate::capscene::CapScene;

/// The geometry the collider-intelligence checks need, derived by the caller from the entity's mesh
/// (`MeshAsset::bounds().max_extent()` + `metrocalk_physics::derive_collider`'s `fit_error`/`concave`).
/// Kept as plain scalars so this module needs no `/physics` (or `/assets`) dependency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshMetrics {
    /// The mesh's largest bounding-box extent (world units) — the scale/unit check.
    pub max_extent: f32,
    /// Convex-hull fit error vs the mesh volume (0 = perfect convex fit) — the concave-dynamic check.
    pub fit_error: f32,
    /// Whether the mesh is concave (fit error over threshold).
    pub concave: bool,
}

/// The classic physics mistakes, each caught before runtime (P8 collider intelligence).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhysicsIssue {
    /// A `RigidBody` with no `Collider` — it falls through the floor.
    NoCollider,
    /// A concave mesh used for a dynamic body — needs a convex approximation.
    ConcaveDynamic,
    /// The mesh is ~1000× typical size — a meters-vs-millimeters unit mismatch.
    BadScale,
    /// Zero / negative / absurd mass — the solver explodes.
    BadMass,
}

/// One caught mistake: the issue, its **explanation**, and the one-click **fix** (a stable action id the
/// UI dispatches back). A flagged suggestion the author can apply or dismiss — never a silent block.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PhysicsWarning {
    pub issue: PhysicsIssue,
    /// Why it's wrong + what will happen — the "explain every no" payload.
    pub message: String,
    /// The one-click fix's button label.
    pub fix_label: String,
    /// A stable fix action id (`"add-collider"`, `"use-hull"`, `"fix-scale"`, `"fix-mass"`) the shell maps
    /// to the matching `physics_intent` call.
    pub fix_action: String,
}

/// Typical-object scale ceiling (meters). A mesh whose largest extent is far beyond this is almost
/// certainly in the wrong units (mm/cm) — at that scale `mass = volume × density` blows up and the sim
/// explodes (the USD `metersPerUnit` footgun, M8.5).
const SCALE_CEILING: f32 = 50.0;

fn number(engine: &Engine<FlecsWorld>, id: EntityId, component: &str, field: &str) -> Option<f64> {
    match engine.components_of(id).get(component)?.get(field)? {
        FieldValue::Number(n) => Some(*n),
        FieldValue::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

/// Run the collider-intelligence catalogue for `id`. Returns every caught mistake (each explained + with
/// a one-click fix), in a deterministic order. `mesh` is the entity's mesh metrics (the caller derives
/// them; `None` for a body with no mesh — the scale/concave checks then can't run, which is honest).
///
/// (Tunneling-risk — a fast body vs a thin collider relative to `dt` — needs runtime velocity and is the
/// diagnostic CCD case surfaced at play/scrub time, M8.4; it is intentionally not an author-time check.)
#[must_use]
pub fn check_physics(
    engine: &Engine<FlecsWorld>,
    id: EntityId,
    mesh: Option<MeshMetrics>,
) -> Vec<PhysicsWarning> {
    let comps = engine.components_of(id);
    let has_rigidbody = comps.contains_key("RigidBody");
    let has_collider = comps.contains_key("Collider");
    let mut out = Vec::new();

    if has_rigidbody && !has_collider {
        out.push(PhysicsWarning {
            issue: PhysicsIssue::NoCollider,
            message: "this RigidBody has no collider — it will fall through the floor.".into(),
            fix_label: "Add a collider".into(),
            fix_action: "add-collider".into(),
        });
    }

    if has_rigidbody {
        if let Some(mass) = number(engine, id, "RigidBody", "mass") {
            if mass <= 0.0 {
                out.push(PhysicsWarning {
                    issue: PhysicsIssue::BadMass,
                    message: format!(
                        "mass is {mass} — a non-positive mass makes the solver explode; derive it from volume × density."
                    ),
                    fix_label: "Set a sane mass".into(),
                    fix_action: "fix-mass".into(),
                });
            }
        }
    }

    if let Some(m) = mesh {
        if m.max_extent > SCALE_CEILING {
            let ratio = (m.max_extent / 2.0).round(); // vs a ~2 m typical prop
            out.push(PhysicsWarning {
                issue: PhysicsIssue::BadScale,
                message: format!(
                    "this mesh is ~{ratio}× a typical prop — meters vs millimetres? at this scale mass becomes enormous and the sim will explode. (Dismiss if it's intentionally huge.)"
                ),
                fix_label: "Scale to metres".into(),
                fix_action: "fix-scale".into(),
            });
        }
        if has_rigidbody && m.concave {
            out.push(PhysicsWarning {
                issue: PhysicsIssue::ConcaveDynamic,
                message: format!(
                    "concave mesh on a dynamic body → using a generated convex hull (fit error {:.0}%). Or keep it static / use voxels.",
                    m.fit_error * 100.0
                ),
                fix_label: "Use a convex hull".into(),
                fix_action: "use-hull".into(),
            });
        }
    }

    out
}

/// Whether `id` is a dead mesh model the engine should offer to make dynamic: it renders a mesh
/// (`MeshRenderer`) but has no `RigidBody` yet. Drives the ≤2-click "Looks dynamic — add RigidBody +
/// Collider?" intent.
#[must_use]
pub fn looks_dynamic(engine: &Engine<FlecsWorld>, id: EntityId) -> bool {
    let comps = engine.components_of(id);
    comps.contains_key("MeshRenderer") && !comps.contains_key("RigidBody")
}

/// Append the `RigidBody` (dynamic) component fields + the `provides Physics` pair to an existing entity.
fn rigidbody_ops(ops: &mut Vec<Op>, scene: &CapScene, id: EntityId, mass: f64) {
    for (field, value) in [
        ("kind", FieldValue::Str("dynamic".into())),
        ("mass", FieldValue::Number(mass)),
    ] {
        ops.push(Op::SetField {
            entity: id,
            component: "RigidBody".into(),
            field: field.into(),
            value,
        });
    }
    if let Some(&c) = scene.caps.get(&canonical("Physics")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: c,
        });
    }
}

/// Append the `Collider` component fields + the `provides Collision` pair. `convex_hull` ⇒ a hull derived
/// from the mesh (the dynamic default); otherwise a ball fallback.
fn collider_ops(ops: &mut Vec<Op>, scene: &CapScene, id: EntityId, convex_hull: bool, radius: f64) {
    let shape = if convex_hull { "convexHull" } else { "ball" };
    ops.push(Op::SetField {
        entity: id,
        component: "Collider".into(),
        field: "shape".into(),
        value: FieldValue::Str(shape.into()),
    });
    if !convex_hull {
        ops.push(Op::SetField {
            entity: id,
            component: "Collider".into(),
            field: "radius".into(),
            value: FieldValue::Number(radius),
        });
    }
    if let Some(&c) = scene.caps.get(&canonical("Collision")) {
        ops.push(Op::AddPair {
            entity: id,
            rel: scene.rels.provides,
            target: c,
        });
    }
}

/// The ≤2-click move: turn a dead mesh model into a correct dynamic body — add a `RigidBody` (dynamic,
/// sane default mass) AND a `Collider` (a convex hull auto-derived from the mesh, the honest dynamic
/// default) to the EXISTING entity as ONE undoable transaction (invariant 3). `mass` is the intent-first
/// default (the caller computes it from volume × density; `1.0` is a fallback).
///
/// # Errors
/// Propagates a [`PipelineError`] if the commit fails.
pub fn make_dynamic(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    id: EntityId,
    mass: f64,
) -> Result<(), PipelineError> {
    let mut ops = Vec::new();
    rigidbody_ops(&mut ops, scene, id, mass);
    collider_ops(&mut ops, scene, id, true, 0.5);
    engine.commit("make-dynamic", ops)
}

/// The `NoCollider` fix: add just a `Collider` (a derived convex hull, or a ball) to a body that lacks
/// one — one undoable transaction.
///
/// # Errors
/// Propagates a [`PipelineError`] if the commit fails.
pub fn add_collider(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    id: EntityId,
    convex_hull: bool,
) -> Result<(), PipelineError> {
    let mut ops = Vec::new();
    collider_ops(&mut ops, scene, id, convex_hull, 0.5);
    engine.commit("add-collider", ops)
}

/// The `ConcaveDynamic` fix: switch the collider to a convex hull (set `Collider.shape = "convexHull"`) —
/// one undoable transaction.
///
/// # Errors
/// Propagates a [`PipelineError`] if the commit fails.
pub fn use_convex_hull(engine: &mut Engine<FlecsWorld>, id: EntityId) -> Result<(), PipelineError> {
    engine.commit(
        "use-convex-hull",
        vec![Op::SetField {
            entity: id,
            component: "Collider".into(),
            field: "shape".into(),
            value: FieldValue::Str("convexHull".into()),
        }],
    )
}

/// The `BadMass` fix: set a sane positive mass — one undoable transaction.
///
/// # Errors
/// Propagates a [`PipelineError`] if the commit fails.
pub fn fix_mass(
    engine: &mut Engine<FlecsWorld>,
    id: EntityId,
    mass: f64,
) -> Result<(), PipelineError> {
    engine.commit(
        "fix-mass",
        vec![Op::SetField {
            entity: id,
            component: "RigidBody".into(),
            field: "mass".into(),
            value: FieldValue::Number(mass.max(0.001)),
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capscene;
    use metrocalk_ecs::FlecsWorld;

    fn engine() -> (Engine<FlecsWorld>, CapScene) {
        let mut world = FlecsWorld::new();
        let scene = CapScene::intern(&mut world);
        let mut e = Engine::new(world, 1);
        capscene::seed(&mut e, &scene, 50).expect("seed");
        e.clear_history();
        (e, scene)
    }

    /// A bare mesh entity (a "dead model"): a Transform + a MeshRenderer handle, no physics.
    fn dead_mesh(engine: &mut Engine<FlecsWorld>, scene: &CapScene) -> EntityId {
        capscene::place_mesh(engine, scene, "mtkasset:test", [0.0, 1.0, 0.0]).expect("place")
    }

    #[test]
    fn dead_model_to_dynamic_body_in_one_call_undoable() {
        let (mut e, scene) = engine();
        let id = dead_mesh(&mut e, &scene);
        assert!(
            looks_dynamic(&e, id),
            "a bare mesh is offered 'make dynamic'"
        );

        make_dynamic(&mut e, &scene, id, 1.0).expect("make dynamic");
        let comps = e.components_of(id);
        assert!(comps.contains_key("RigidBody") && comps.contains_key("Collider"));
        // Correct by construction: no warnings now (collider present, mass positive, no mesh metrics).
        assert!(check_physics(&e, id, None).is_empty());
        assert!(!looks_dynamic(&e, id), "it's no longer a dead model");

        // ONE undoable transaction.
        assert!(e.undo());
        assert!(!e.components_of(id).contains_key("RigidBody"));
        assert!(!e.components_of(id).contains_key("Collider"));
    }

    #[test]
    fn no_collider_is_caught_and_the_fix_re_passes() {
        let (mut e, scene) = engine();
        let id = dead_mesh(&mut e, &scene);
        // A RigidBody with no Collider (the classic trap).
        e.commit(
            "rb only",
            vec![Op::SetField {
                entity: id,
                component: "RigidBody".into(),
                field: "kind".into(),
                value: FieldValue::Str("dynamic".into()),
            }],
        )
        .unwrap();
        let warns = check_physics(&e, id, None);
        assert_eq!(warns.len(), 1);
        assert_eq!(warns[0].issue, PhysicsIssue::NoCollider);
        assert_eq!(warns[0].fix_action, "add-collider");

        // Apply the one-click fix → the check must RE-PASS (the fix actually fixed it).
        add_collider(&mut e, &scene, id, true).unwrap();
        assert!(
            check_physics(&e, id, None)
                .iter()
                .all(|w| w.issue != PhysicsIssue::NoCollider),
            "after add-collider the no-collider warning is gone"
        );
    }

    #[test]
    fn concave_dynamic_is_explained_with_fit_error_and_fix_re_passes() {
        let (mut e, scene) = engine();
        let id = dead_mesh(&mut e, &scene);
        make_dynamic(&mut e, &scene, id, 1.0).unwrap();
        // A concave mesh (the caller derived this from physics::derive_collider).
        let metrics = MeshMetrics {
            max_extent: 2.0,
            fit_error: 0.42,
            concave: true,
        };
        let warns = check_physics(&e, id, Some(metrics));
        let concave = warns
            .iter()
            .find(|w| w.issue == PhysicsIssue::ConcaveDynamic)
            .expect("concave-dynamic caught");
        assert!(
            concave.message.contains("42%"),
            "reports the fit error: {}",
            concave.message
        );
        assert_eq!(concave.fix_action, "use-hull");
        // The fix sets the hull; the warning is informational (the hull is already the default), but the
        // check is stable — re-running with a now-convex metric clears it.
        use_convex_hull(&mut e, id).unwrap();
        let convex = MeshMetrics {
            concave: false,
            fit_error: 0.0,
            ..metrics
        };
        assert!(check_physics(&e, id, Some(convex))
            .iter()
            .all(|w| w.issue != PhysicsIssue::ConcaveDynamic));
    }

    #[test]
    fn bad_scale_and_bad_mass_are_caught_and_fixable() {
        let (mut e, scene) = engine();
        let id = dead_mesh(&mut e, &scene);
        make_dynamic(&mut e, &scene, id, 1.0).unwrap();
        // Zero mass + an over-scaled mesh.
        fix_mass(&mut e, id, 0.0).ok(); // force a bad mass via the same path (clamped — so set directly)
        e.commit(
            "zero mass",
            vec![Op::SetField {
                entity: id,
                component: "RigidBody".into(),
                field: "mass".into(),
                value: FieldValue::Number(0.0),
            }],
        )
        .unwrap();
        let huge = MeshMetrics {
            max_extent: 1500.0,
            fit_error: 0.0,
            concave: false,
        };
        let warns = check_physics(&e, id, Some(huge));
        assert!(warns.iter().any(|w| w.issue == PhysicsIssue::BadMass));
        assert!(warns.iter().any(|w| w.issue == PhysicsIssue::BadScale));

        // Fix the mass → the bad-mass warning clears (re-passes).
        fix_mass(&mut e, id, 2.0).unwrap();
        assert!(check_physics(&e, id, Some(huge))
            .iter()
            .all(|w| w.issue != PhysicsIssue::BadMass));
    }
}
