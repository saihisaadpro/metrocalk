//! Mechanism kinematics (M15.9 / ADR-079) — **animate a mechanism from the viewport**: a joint is a typed
//! ECS component on the moving part (axis · type · limits · the honesty-labeled source), keyframes are an
//! undoable per-joint track, and playback/scrub is a **closed-form kinematic solve** (deterministic — same
//! timeline → bit-identical transforms; NEVER a physics sim, which would be non-deterministic and wrong for
//! the determinism axis). Authoring (set a joint · key a pose · commit a drag) goes through the one
//! undoable commit pipeline (invariant 3); scrubbing is a render-only PROJECTION over the authored state
//! (the M8.4 sim-scrub discipline — the doc is never mutated by playback).
//!
//! **The pivot is the REAL joint axis** (a point + direction in world space — from the part's geometry /
//! URDF / the designer's gizmo pick), never the assembly origin: the exact thing Datasmith's origin-parked
//! pivots make impossible without manual re-rigging.
//!
//! Source ladder (honesty-labeled, ADR-079): `"urdf"` (reliable, robots) → `"inferred"` (from cylindrical/
//! concentric geometry — a labeled proposal) → `"manual"` (gizmo-authored — the default). The label rides
//! the component so the UI can say which rung produced the rig, never overselling "automatic".

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;

/// One authored sample in a complete mechanism-track transaction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JointTrackKey {
    pub time: f64,
    pub value: f64,
}

/// A complete joint definition and the keys the user wants to add or replace.  A collection of these is
/// deliberately committed as one transaction by the desktop shell: large assemblies pay for validation,
/// animation-plan compilation and viewport publication once, while Ctrl-Z still describes the one visible
/// "author mechanism" action the user performed.
#[derive(Clone, Debug, PartialEq)]
pub struct JointTrackAuthoring {
    pub entity: EntityId,
    pub revolute: bool,
    pub axis: [f64; 3],
    pub pivot: [f64; 3],
    pub limits: (f64, f64),
    pub source: String,
    pub keys: Vec<JointTrackKey>,
}

/// The authored mechanism component — a typed kinematic DOF on the moving part entity.
///
/// This is intentionally distinct from the registry's physics `Joint` relation (`kind`/`bodyA`/`bodyB`).
/// New mechanism data must use this name so a physics constraint can never be mistaken for an animation
/// channel merely because both domains use the word "joint".
pub const KINEMATIC_JOINT: &str = "KinematicJoint";
/// The pre-split mechanism component name. Read-only compatibility keeps existing projects playable when
/// (and only when) the component contains the complete kinematic field shape.
pub const LEGACY_JOINT: &str = "Joint";
/// Compatibility alias used by the existing authoring call sites. It deliberately points at the new,
/// unambiguous component; legacy data is handled by [`joint_of_with_component`].
pub const JOINT: &str = KINEMATIC_JOINT;
/// The per-joint keyframe track component (`keys` = the encoded track).
pub const JOINT_TRACK: &str = "JointTrack";

/// A parsed joint (the typed view over the `Joint` component's fields).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Joint {
    /// `true` = revolute (radians about the axis); `false` = prismatic (units along the axis).
    pub revolute: bool,
    /// The joint axis direction (unit, world space).
    pub axis: [f64; 3],
    /// A point ON the axis (world space) — the real pivot, never the origin by default.
    pub pivot: [f64; 3],
    /// DOF limits (radians / units). Values clamp here — a mechanism never over-travels silently.
    pub min: f64,
    pub max: f64,
    /// The current DOF value.
    pub value: f64,
}

/// A stable, plain-language validation failure suitable for returning at the command boundary.
pub type JointValidationResult = Result<(), &'static str>;

/// Validate a newly-authored kinematic joint before committing it to the document.
///
/// Persisted legacy joints may omit limits (and therefore read as unbounded), but command-authored data is
/// required to be finite. Keeping that distinction here prevents JSON/IPC NaNs and reversed ranges from
/// reaching `f64::clamp`, transforms, or the GPU.
pub fn validate_joint_spec(
    axis: [f64; 3],
    pivot: [f64; 3],
    (min, max): (f64, f64),
) -> JointValidationResult {
    if !axis.iter().all(|part| part.is_finite()) {
        return Err("joint axis must contain only finite numbers");
    }
    let axis_len_sq = axis.iter().map(|part| part * part).sum::<f64>();
    if !axis_len_sq.is_finite() || axis_len_sq <= 1e-24 {
        return Err("joint axis must have a non-zero direction");
    }
    if !pivot.iter().all(|part| part.is_finite()) {
        return Err("joint pivot must contain only finite numbers");
    }
    if !min.is_finite() || !max.is_finite() {
        return Err("joint limits must contain only finite numbers");
    }
    if min > max {
        return Err("joint minimum must not exceed its maximum");
    }
    Ok(())
}

/// Validate a requested DOF value before previewing or committing it.
pub fn validate_joint_value(value: f64) -> JointValidationResult {
    if value.is_finite() {
        Ok(())
    } else {
        Err("joint value must be finite")
    }
}

/// Validate a key/scrub time before it enters an authored track.
pub fn validate_joint_time(time: f64) -> JointValidationResult {
    if !time.is_finite() {
        Err("joint time must be finite")
    } else if time < 0.0 {
        Err("joint time must be zero or greater")
    } else {
        Ok(())
    }
}

/// Build the undoable ops that author a joint on `entity` (one commit = one Ctrl-Z). `source` is the
/// honesty label: `"manual"` (gizmo-authored) · `"inferred"` · `"urdf"`.
#[must_use]
pub fn set_joint_ops(
    entity: EntityId,
    revolute: bool,
    axis: [f64; 3],
    pivot: [f64; 3],
    (min, max): (f64, f64),
    source: &str,
) -> Vec<Op> {
    try_set_joint_ops(entity, revolute, axis, pivot, (min, max), source).unwrap_or_default()
}

/// Validated form of [`set_joint_ops`]. New command-boundary code should prefer this API so a rejection
/// carries an explanation; the compatibility wrapper above returns no ops for an invalid specification.
pub fn try_set_joint_ops(
    entity: EntityId,
    revolute: bool,
    axis: [f64; 3],
    pivot: [f64; 3],
    (min, max): (f64, f64),
    source: &str,
) -> Result<Vec<Op>, &'static str> {
    validate_joint_spec(axis, pivot, (min, max))?;
    let axis = normalize(axis);
    let mut ops = Vec::with_capacity(10);
    let mut num = |field: &str, v: f64| {
        ops.push(Op::SetField {
            entity,
            component: JOINT.into(),
            field: field.into(),
            value: FieldValue::Number(v),
        });
    };
    num("ax", axis[0]);
    num("ay", axis[1]);
    num("az", axis[2]);
    num("px", pivot[0]);
    num("py", pivot[1]);
    num("pz", pivot[2]);
    num("min", min);
    num("max", max);
    num("value", 0.0);
    ops.push(Op::SetField {
        entity,
        component: JOINT.into(),
        field: "type".into(),
        value: FieldValue::Str(if revolute { "revolute" } else { "prismatic" }.into()),
    });
    ops.push(Op::SetField {
        entity,
        component: JOINT.into(),
        field: "source".into(),
        value: FieldValue::Str(source.into()),
    });
    Ok(ops)
}

/// Validate and compile a complete multi-part mechanism authoring gesture into one bounded operation list.
///
/// This is the durable high-volume authoring seam used by the ordinary editor transport.  It intentionally
/// preserves the established single-command semantics:
///
/// - a joint definition starts at value zero without moving the current authored transform;
/// - keys at an existing time replace that sample, while unrelated existing samples survive;
/// - values outside the authored travel limits are clamped before both storage and final posing;
/// - the last supplied value becomes the committed pose and joint readback value;
/// - validation is all-or-nothing, so a bad row cannot leave a half-authored assembly.
///
/// The returned operations contain no work proportional to unrelated scene entities.  The caller lands the
/// whole vector with one `Engine::commit`, refreshes the animation plan once and publishes the viewport once.
pub fn try_author_joint_tracks_ops(
    engine: &Engine<FlecsWorld>,
    requests: &[JointTrackAuthoring],
) -> Result<Vec<Op>, String> {
    if requests.is_empty() {
        return Err("at least one joint is required".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut ops = Vec::new();
    for (index, request) in requests.iter().enumerate() {
        if engine.ecs_entity(request.entity).is_none() {
            return Err(format!(
                "joint {} targets an entity that no longer exists",
                index + 1
            ));
        }
        if !seen.insert(request.entity) {
            return Err(format!(
                "joint {} targets the same entity more than once",
                index + 1
            ));
        }
        validate_joint_spec(request.axis, request.pivot, request.limits)
            .map_err(|reason| format!("joint {}: {reason}", index + 1))?;
        for (key_index, key) in request.keys.iter().enumerate() {
            validate_joint_time(key.time).map_err(|reason| {
                format!("joint {}, key {}: {reason}", index + 1, key_index + 1)
            })?;
            validate_joint_value(key.value).map_err(|reason| {
                format!("joint {}, key {}: {reason}", index + 1, key_index + 1)
            })?;
        }

        ops.extend(
            try_set_joint_ops(
                request.entity,
                request.revolute,
                request.axis,
                request.pivot,
                request.limits,
                &request.source,
            )
            .map_err(|reason| format!("joint {}: {reason}", index + 1))?,
        );

        let components = engine.components_of(request.entity);
        let transform = components.get("Transform");
        let number = |field: &str| -> f64 {
            num_of(transform.and_then(|fields| fields.get(field))).unwrap_or(0.0)
        };
        let base_position = [number("x"), number("y"), number("z")];
        let raw_rotation = [number("qx"), number("qy"), number("qz"), number("qw")];
        let base_rotation = if raw_rotation == [0.0; 4] {
            [0.0, 0.0, 0.0, 1.0]
        } else {
            raw_rotation
        };

        let existing = components
            .get(JOINT_TRACK)
            .and_then(|fields| fields.get("keys"))
            .and_then(|value| match value {
                FieldValue::Str(keys) => Some(keys.as_str()),
                _ => None,
            })
            .unwrap_or_default();
        let mut keys = parse_track(existing);
        let authored_joint = Joint {
            revolute: request.revolute,
            axis: normalize(request.axis),
            pivot: request.pivot,
            min: request.limits.0,
            max: request.limits.1,
            value: 0.0,
        };
        for key in &request.keys {
            let clamped = safe_clamp(key.value, authored_joint.min, authored_joint.max);
            keys.retain(|(time, _)| (*time - key.time).abs() > 1.0e-9);
            keys.push((key.time, clamped));
        }
        if !request.keys.is_empty() {
            ops.push(Op::SetField {
                entity: request.entity,
                component: JOINT_TRACK.into(),
                field: "keys".into(),
                value: FieldValue::Str(encode_track(&keys)),
            });
            let final_value = request
                .keys
                .last()
                .map(|key| safe_clamp(key.value, authored_joint.min, authored_joint.max))
                .unwrap_or(0.0);
            let (position, rotation) =
                joint_pose(&authored_joint, base_position, base_rotation, final_value);
            for (field, value) in [
                ("x", position[0]),
                ("y", position[1]),
                ("z", position[2]),
                ("qx", rotation[0]),
                ("qy", rotation[1]),
                ("qz", rotation[2]),
                ("qw", rotation[3]),
            ] {
                ops.push(Op::SetField {
                    entity: request.entity,
                    component: "Transform".into(),
                    field: field.into(),
                    value: FieldValue::Number(value),
                });
            }
            ops.push(Op::SetField {
                entity: request.entity,
                component: JOINT.into(),
                field: "value".into(),
                value: FieldValue::Number(final_value),
            });
        }
    }
    Ok(ops)
}

/// Read a number field that may have landed as either numeric arm (the FieldValue::Integer-vs-Number
/// gotcha — whole numbers arrive as Integer; matching only Number silently falls to default).
fn num_of(v: Option<&FieldValue>) -> Option<f64> {
    match v {
        Some(FieldValue::Number(n)) => Some(*n),
        #[allow(clippy::cast_precision_loss)]
        Some(FieldValue::Integer(i)) => Some(*i as f64),
        _ => None,
    }
}

fn joint_from_component(
    engine: &Engine<FlecsWorld>,
    id: EntityId,
    component: &str,
) -> Option<Joint> {
    let comps = engine.components_of(id);
    let j = comps.get(component)?;
    let n = |f: &str| num_of(j.get(f));
    let raw_axis = [n("ax")?, n("ay")?, n("az")?];
    let pivot = [n("px")?, n("py")?, n("pz")?];
    if !raw_axis.iter().chain(&pivot).all(|part| part.is_finite()) {
        return None;
    }
    let axis_len_sq = raw_axis.iter().map(|part| part * part).sum::<f64>();
    if !axis_len_sq.is_finite() || axis_len_sq <= 1e-24 {
        return None;
    }
    let min = n("min").unwrap_or(f64::NEG_INFINITY);
    let max = n("max").unwrap_or(f64::INFINITY);
    let value = n("value").unwrap_or(0.0);
    if min.is_nan() || max.is_nan() || min > max || !value.is_finite() {
        return None;
    }
    let revolute = matches!(j.get("type"), Some(FieldValue::Str(s)) if s == "revolute");
    Some(Joint {
        revolute,
        axis: normalize(raw_axis),
        pivot,
        min,
        max,
        value,
    })
}

/// Parse the kinematic component off an entity and report which schema supplied it. The new component is
/// preferred; a legacy `Joint` is accepted only if it has the complete kinematic field shape, so a physics
/// relation with `kind/bodyA/bodyB` remains inert.
#[must_use]
pub fn joint_of_with_component(
    engine: &Engine<FlecsWorld>,
    id: EntityId,
) -> Option<(Joint, &'static str)> {
    joint_from_component(engine, id, KINEMATIC_JOINT)
        .map(|joint| (joint, KINEMATIC_JOINT))
        .or_else(|| {
            joint_from_component(engine, id, LEGACY_JOINT).map(|joint| (joint, LEGACY_JOINT))
        })
}

/// Parse a mechanism joint off an entity. `None` when absent/malformed (including physics-only `Joint`
/// relations); malformed data is inert, never a panic or a guessed axis.
#[must_use]
pub fn joint_of(engine: &Engine<FlecsWorld>, id: EntityId) -> Option<Joint> {
    joint_of_with_component(engine, id).map(|(joint, _)| joint)
}

/// The honesty label of a joint's source rung (`"manual"` when unlabeled — never oversold).
#[must_use]
pub fn joint_source(engine: &Engine<FlecsWorld>, id: EntityId) -> String {
    let Some((_, component)) = joint_of_with_component(engine, id) else {
        return "manual".into();
    };
    match engine.components_of(id).get(component).and_then(|j| {
        j.get("source").and_then(|v| match v {
            FieldValue::Str(s) => Some(s.clone()),
            _ => None,
        })
    }) {
        Some(s) if !s.is_empty() => s,
        _ => "manual".into(),
    }
}

// ── the keyframe track: "t:v;t:v;…" sorted by t — undoable (a string field), deterministic to parse ──────

/// Parse an encoded track into `(t, value)` pairs, sorted by `t` (malformed segments are skipped — a
/// corrupt track plays what it can, never panics).
#[must_use]
pub fn parse_track(keys: &str) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = keys
        .split(';')
        .filter_map(|seg| {
            let (t, v) = seg.split_once(':')?;
            let sample: (f64, f64) = (t.trim().parse().ok()?, v.trim().parse().ok()?);
            (sample.0.is_finite() && sample.1.is_finite()).then_some(sample)
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Encode `(t, value)` pairs (sorted by `t`; 17-sig-digit round-trippable f64 so a keyed pose replays
/// bit-identically).
#[must_use]
pub fn encode_track(keys: &[(f64, f64)]) -> String {
    let mut sorted: Vec<(f64, f64)> = keys
        .iter()
        .copied()
        .filter(|(time, value)| time.is_finite() && value.is_finite())
        .collect();
    sorted.sort_by(|a, b| a.0.total_cmp(&b.0));
    sorted
        .iter()
        .map(|(t, v)| format!("{t:.17e}:{v:.17e}"))
        .collect::<Vec<_>>()
        .join(";")
}

/// The track's value at time `t` — clamped linear interpolation (closed-form; same `t` → bit-identical
/// result, the determinism gate). An empty track holds 0.
#[must_use]
pub fn track_value(keys: &[(f64, f64)], t: f64) -> f64 {
    if !t.is_finite() {
        return 0.0;
    }
    match keys {
        [] => 0.0,
        [only] => only.1,
        _ => {
            if t <= keys[0].0 {
                return keys[0].1;
            }
            if let Some(last) = keys.last() {
                if t >= last.0 {
                    return last.1;
                }
            }
            for w in keys.windows(2) {
                let (t0, v0) = w[0];
                let (t1, v1) = w[1];
                if t >= t0 && t <= t1 {
                    if t1 - t0 <= 0.0 {
                        return v1;
                    }
                    let f = (t - t0) / (t1 - t0);
                    return v0 + (v1 - v0) * f;
                }
            }
            keys.last().map_or(0.0, |k| k.1)
        }
    }
}

/// The end of the track (the timeline length for the scrub UI).
#[must_use]
pub fn track_end(keys: &[(f64, f64)]) -> f64 {
    keys.last().map_or(0.0, |k| k.0)
}

// ── the closed-form pose solve ────────────────────────────────────────────────────────────────────────────

/// The posed `(position, quaternion)` of a part whose BASE (authored) transform is `(base_pos, base_quat)`
/// and whose joint DOF is at `value` (clamped to the joint's limits):
/// - **revolute**: rotate the base pose about the joint's REAL axis (pivot + direction) by `value` radians —
///   the position orbits the axis, the orientation compounds. A pivot ON the part's own axis means the part
///   spins in place; a pivot elsewhere swings it — exactly the physical joint.
/// - **prismatic**: slide the base position along the axis by `value` units (orientation unchanged).
#[must_use]
pub fn joint_pose(
    joint: &Joint,
    base_pos: [f64; 3],
    base_quat: [f64; 4],
    value: f64,
) -> ([f64; 3], [f64; 4]) {
    let v = safe_clamp(value, joint.min, joint.max);
    if joint.revolute {
        let q = axis_angle_quat(joint.axis, v);
        // p' = pivot + R·(p − pivot)
        let rel = sub(base_pos, joint.pivot);
        let pos = add(joint.pivot, rotate(q, rel));
        (pos, quat_mul(q, base_quat))
    } else {
        (add(base_pos, scale(joint.axis, v)), base_quat)
    }
}

/// Clamp without `f64::clamp`'s panic on reversed/NaN bounds. A malformed range is treated as unbounded,
/// and a non-finite value becomes the neutral zero pose; command-authored data is rejected earlier by the
/// public validation helpers, while this is the last defensive line for persisted legacy data.
fn safe_clamp(value: f64, min: f64, max: f64) -> f64 {
    if !value.is_finite() {
        return 0.0;
    }
    if min.is_nan() || max.is_nan() || min > max {
        return value;
    }
    value.max(min).min(max)
}

// ── small exact f64 vector/quaternion helpers ─────────────────────────────────────────────────────────────
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn normalize(a: [f64; 3]) -> [f64; 3] {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    if l > 1e-12 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        [0.0, 0.0, 1.0]
    }
}
fn axis_angle_quat(axis: [f64; 3], angle: f64) -> [f64; 4] {
    let h = angle * 0.5;
    let s = h.sin();
    [axis[0] * s, axis[1] * s, axis[2] * s, h.cos()]
}
/// Hamilton product `a·b` (`[x,y,z,w]`).
fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
/// Rotate vector `v` by quaternion `q`.
fn rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    // v' = v + 2·qv×(qv×v + w·v)
    let qv = [q[0], q[1], q[2]];
    let t = scale(cross(qv, v), 2.0);
    add(add(v, scale(t, q[3])), cross(qv, t))
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[cfg(test)]
#[allow(clippy::float_cmp, clippy::unreadable_literal)] // bit-exact determinism IS the claim under test
mod tests {
    use super::*;

    fn legacy_joint_ops(id: EntityId) -> Vec<Op> {
        let mut ops = set_joint_ops(
            id,
            true,
            [0.0, 0.0, 2.0],
            [4.0, 5.0, 6.0],
            (-1.0, 1.0),
            "manual",
        );
        for op in &mut ops {
            if let Op::SetField { component, .. } = op {
                *component = LEGACY_JOINT.into();
            }
        }
        ops
    }

    #[test]
    fn new_mechanisms_use_the_unambiguous_component_and_legacy_data_still_reads() {
        let mut engine = Engine::new(FlecsWorld::new(), 17);
        let new_id = engine.alloc_entity_id();
        let legacy_id = engine.alloc_entity_id();
        let mut ops = vec![
            Op::CreateEntity {
                id: new_id,
                parent: None,
            },
            Op::CreateEntity {
                id: legacy_id,
                parent: None,
            },
        ];
        ops.extend(set_joint_ops(
            new_id,
            false,
            [1.0, 0.0, 0.0],
            [1.0, 2.0, 3.0],
            (-2.0, 2.0),
            "manual",
        ));
        ops.extend(legacy_joint_ops(legacy_id));
        engine
            .commit("component-split", ops)
            .expect("fixture commit");

        assert!(engine.components_of(new_id).contains_key(KINEMATIC_JOINT));
        assert!(!engine.components_of(new_id).contains_key(LEGACY_JOINT));
        assert_eq!(
            joint_of_with_component(&engine, new_id).map(|(_, name)| name),
            Some(KINEMATIC_JOINT)
        );
        let (legacy, component) =
            joint_of_with_component(&engine, legacy_id).expect("legacy joint");
        assert_eq!(component, LEGACY_JOINT);
        assert_eq!(legacy.axis, [0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_physics_joint_relation_is_not_misread_as_a_mechanism() {
        let mut engine = Engine::new(FlecsWorld::new(), 19);
        let id = engine.alloc_entity_id();
        let field = |name: &str, value: &str| Op::SetField {
            entity: id,
            component: LEGACY_JOINT.into(),
            field: name.into(),
            value: FieldValue::Str(value.into()),
        };
        engine
            .commit(
                "physics-joint",
                vec![
                    Op::CreateEntity { id, parent: None },
                    field("kind", "revolute"),
                    field("bodyA", "a"),
                    field("bodyB", "b"),
                ],
            )
            .expect("fixture commit");
        assert_eq!(joint_of(&engine, id), None);
    }

    #[test]
    fn command_boundary_validation_rejects_non_finite_and_reversed_specs() {
        assert!(validate_joint_spec([0.0, 0.0, 1.0], [1.0, 2.0, 3.0], (-1.0, 1.0)).is_ok());
        assert!(validate_joint_spec([0.0; 3], [0.0; 3], (-1.0, 1.0)).is_err());
        assert!(validate_joint_spec([f64::NAN, 0.0, 1.0], [0.0; 3], (-1.0, 1.0)).is_err());
        assert!(
            validate_joint_spec([0.0, 0.0, 1.0], [f64::INFINITY, 0.0, 0.0], (-1.0, 1.0)).is_err()
        );
        assert!(validate_joint_spec([0.0, 0.0, 1.0], [0.0; 3], (2.0, 1.0)).is_err());
        assert!(validate_joint_value(f64::NAN).is_err());
        assert!(validate_joint_value(0.25).is_ok());
        assert!(validate_joint_time(-0.01).is_err());
        assert!(validate_joint_time(f64::INFINITY).is_err());
        assert!(validate_joint_time(0.0).is_ok());

        let mut engine = Engine::new(FlecsWorld::new(), 23);
        let id = engine.alloc_entity_id();
        assert!(try_set_joint_ops(id, true, [0.0; 3], [0.0; 3], (-1.0, 1.0), "manual").is_err());
        assert!(set_joint_ops(id, true, [0.0; 3], [0.0; 3], (-1.0, 1.0), "manual").is_empty());
    }

    #[test]
    fn complete_mechanism_authoring_is_atomic_readable_and_one_undo_step() {
        let mut engine = Engine::new(FlecsWorld::new(), 29);
        let first = engine.alloc_entity_id();
        let second = engine.alloc_entity_id();
        engine
            .commit(
                "fixture",
                vec![
                    Op::CreateEntity {
                        id: first,
                        parent: None,
                    },
                    Op::CreateEntity {
                        id: second,
                        parent: None,
                    },
                    Op::SetField {
                        entity: first,
                        component: "Transform".into(),
                        field: "x".into(),
                        value: FieldValue::Number(12.0),
                    },
                    Op::SetField {
                        entity: first,
                        component: JOINT_TRACK.into(),
                        field: "keys".into(),
                        value: FieldValue::Str(encode_track(&[(9.0, 0.25)])),
                    },
                ],
            )
            .expect("fixture");
        engine.clear_history();

        let requests = vec![
            JointTrackAuthoring {
                entity: first,
                revolute: true,
                axis: [0.0, 0.0, 2.0],
                pivot: [10.0, 0.0, 0.0],
                limits: (-2.0, 2.0),
                source: "manual".into(),
                // Duplicate t=1 mirrors sequential jointKey replacement; the last sample wins.
                keys: vec![
                    JointTrackKey {
                        time: 0.0,
                        value: 0.0,
                    },
                    JointTrackKey {
                        time: 1.0,
                        value: 0.5,
                    },
                    JointTrackKey {
                        time: 1.0,
                        value: 1.0,
                    },
                ],
            },
            JointTrackAuthoring {
                entity: second,
                revolute: false,
                axis: [1.0, 0.0, 0.0],
                pivot: [0.0; 3],
                limits: (-1.0, 1.0),
                source: "inferred".into(),
                keys: vec![JointTrackKey {
                    time: 2.0,
                    value: 4.0,
                }],
            },
        ];
        let ops = try_author_joint_tracks_ops(&engine, &requests).expect("valid batch");
        engine.commit("author-mechanism", ops).expect("one commit");

        let first_joint = joint_of(&engine, first).expect("first joint readback");
        assert_eq!(first_joint.axis, [0.0, 0.0, 1.0]);
        assert_eq!(first_joint.value, 1.0);
        let first_keys = engine
            .components_of(first)
            .get(JOINT_TRACK)
            .and_then(|fields| fields.get("keys"))
            .and_then(|value| match value {
                FieldValue::Str(value) => Some(parse_track(value)),
                _ => None,
            })
            .expect("track readback");
        assert_eq!(first_keys, vec![(0.0, 0.0), (1.0, 1.0), (9.0, 0.25)]);
        assert_eq!(joint_of(&engine, second).expect("second joint").value, 1.0);

        assert!(
            engine.undo(),
            "one undo removes the complete visible action"
        );
        assert!(joint_of(&engine, first).is_none());
        assert!(joint_of(&engine, second).is_none());
        assert!(
            !engine.undo(),
            "there was exactly one user-visible transaction"
        );
    }

    #[test]
    fn mechanism_batch_cost_and_op_count_do_not_scale_with_unrelated_scene_size() {
        let mut engine = Engine::new(FlecsWorld::new(), 31);
        let mut create = Vec::with_capacity(15_711);
        let mut targets = Vec::with_capacity(24);
        for index in 0..15_711 {
            let id = engine.alloc_entity_id();
            if index < 24 {
                targets.push(id);
            }
            create.push(Op::CreateEntity { id, parent: None });
        }
        engine.commit("large-scene", create).expect("large fixture");
        engine.clear_history();
        let requests: Vec<_> = targets
            .into_iter()
            .map(|entity| JointTrackAuthoring {
                entity,
                revolute: true,
                axis: [0.0, 0.0, 1.0],
                pivot: [0.0; 3],
                limits: (-3.2, 3.2),
                source: "manual".into(),
                keys: (0..7)
                    .map(|key| JointTrackKey {
                        time: f64::from(key),
                        value: f64::from(key) * 0.1,
                    })
                    .collect(),
            })
            .collect();
        let started = std::time::Instant::now();
        let ops = try_author_joint_tracks_ops(&engine, &requests).expect("24-track batch");
        let elapsed = started.elapsed();
        // 11 joint fields + one encoded track + 7 transform fields + final value per target. Keys are
        // compacted into one field, and none of the 15,687 unrelated entities adds an operation.
        assert_eq!(ops.len(), 24 * 20);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "batch preparation should be local to 24 targets, took {elapsed:?}"
        );
        engine
            .commit("author-mechanism", ops)
            .expect("batch commit");
        assert!(engine.undo());
        assert!(requests
            .iter()
            .all(|request| joint_of(&engine, request.entity).is_none()));
    }

    #[test]
    fn a_revolute_joint_rotates_about_its_real_axis_not_the_origin() {
        // The Datasmith failure inverted: a part at (12, 0, 0) with a joint whose REAL axis is the z line
        // through (10, 0, 0) — a quarter turn must orbit the PIVOT (→ (10, 2, 0)), NOT the world origin
        // (which would fling it to (0, 12, 0)).
        let j = Joint {
            revolute: true,
            axis: [0.0, 0.0, 1.0],
            pivot: [10.0, 0.0, 0.0],
            min: f64::NEG_INFINITY,
            max: f64::INFINITY,
            value: 0.0,
        };
        let (p, _q) = joint_pose(
            &j,
            [12.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
            std::f64::consts::FRAC_PI_2,
        );
        assert!(
            (p[0] - 10.0).abs() < 1e-12 && (p[1] - 2.0).abs() < 1e-12 && p[2].abs() < 1e-12,
            "rotated about the REAL axis: {p:?}"
        );
        // The radius from the axis is preserved (a rigid orbit, not a drift).
        let r = ((p[0] - 10.0).powi(2) + p[1].powi(2)).sqrt();
        assert!((r - 2.0).abs() < 1e-12);
    }

    #[test]
    fn a_prismatic_joint_slides_along_its_axis_and_clamps_at_its_limits() {
        let j = Joint {
            revolute: false,
            axis: [1.0, 0.0, 0.0],
            pivot: [0.0, 0.0, 0.0],
            min: -5.0,
            max: 5.0,
            value: 0.0,
        };
        let (p, q) = joint_pose(&j, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], 4.0);
        assert_eq!(p, [5.0, 2.0, 3.0], "slid along x");
        assert_eq!(q, [0.0, 0.0, 0.0, 1.0], "orientation unchanged");
        // Over-travel clamps — a mechanism never silently exceeds its limits.
        let (p, _) = joint_pose(&j, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], 99.0);
        assert_eq!(p, [6.0, 2.0, 3.0], "clamped at max=5");
    }

    #[test]
    fn the_track_scrubs_deterministically_and_round_trips_bit_exact() {
        let keys = vec![(0.0, 0.0), (1.0, std::f64::consts::PI), (2.0, 0.25)];
        let enc = encode_track(&keys);
        let back = parse_track(&enc);
        assert_eq!(back, keys, "17-sig-digit encode round-trips bit-exact");
        // Deterministic scrub: the same t always yields the identical bits (closed-form lerp).
        let a = track_value(&back, 0.6180339887);
        for _ in 0..5 {
            assert_eq!(track_value(&back, 0.6180339887).to_bits(), a.to_bits());
        }
        // Clamped ends + midpoints.
        assert_eq!(track_value(&back, -1.0), 0.0);
        assert_eq!(track_value(&back, 99.0), 0.25);
        assert!((track_value(&back, 0.5) - std::f64::consts::PI / 2.0).abs() < 1e-12);
        assert_eq!(track_end(&back), 2.0);
        // A malformed segment is skipped, never a panic.
        assert_eq!(parse_track("0:1;garbage;2:3").len(), 2);
        assert_eq!(
            parse_track("0:1;1:NaN;2:inf;3:-inf;4:5"),
            vec![(0.0, 1.0), (4.0, 5.0)],
            "non-finite samples are inert"
        );
        assert_eq!(
            encode_track(&[(0.0, 1.0), (f64::NAN, 2.0), (2.0, f64::INFINITY)]),
            "0.00000000000000000e0:1.00000000000000000e0"
        );
        assert_eq!(track_value(&back, f64::NAN), 0.0);
    }

    #[test]
    fn malformed_limits_and_values_never_panic_or_emit_non_finite_transforms() {
        let malformed = Joint {
            revolute: false,
            axis: [1.0, 0.0, 0.0],
            pivot: [0.0; 3],
            min: 5.0,
            max: -5.0,
            value: 0.0,
        };
        let (position, rotation) =
            joint_pose(&malformed, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], 2.0);
        assert_eq!(
            position,
            [3.0, 2.0, 3.0],
            "a reversed legacy range is treated as unbounded"
        );
        assert_eq!(rotation, [0.0, 0.0, 0.0, 1.0]);

        let (position, rotation) =
            joint_pose(&malformed, [1.0, 2.0, 3.0], [0.0, 0.0, 0.0, 1.0], f64::NAN);
        assert_eq!(
            position,
            [1.0, 2.0, 3.0],
            "a bad value resolves to the neutral pose"
        );
        assert!(position.into_iter().chain(rotation).all(f64::is_finite));
    }
}
