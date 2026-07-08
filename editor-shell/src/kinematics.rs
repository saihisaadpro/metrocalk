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

/// The joint component — a typed kinematic DOF on the moving part entity.
pub const JOINT: &str = "Joint";
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
    ops
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

/// Parse the `Joint` component off an entity. `None` when absent/malformed (a malformed joint is inert,
/// never a panic or a guessed axis).
#[must_use]
pub fn joint_of(engine: &Engine<FlecsWorld>, id: EntityId) -> Option<Joint> {
    let comps = engine.components_of(id);
    let j = comps.get(JOINT)?;
    let n = |f: &str| num_of(j.get(f));
    let revolute = matches!(j.get("type"), Some(FieldValue::Str(s)) if s == "revolute");
    let axis = normalize([n("ax")?, n("ay")?, n("az")?]);
    Some(Joint {
        revolute,
        axis,
        pivot: [n("px")?, n("py")?, n("pz")?],
        min: n("min").unwrap_or(f64::NEG_INFINITY),
        max: n("max").unwrap_or(f64::INFINITY),
        value: n("value").unwrap_or(0.0),
    })
}

/// The honesty label of a joint's source rung (`"manual"` when unlabeled — never oversold).
#[must_use]
pub fn joint_source(engine: &Engine<FlecsWorld>, id: EntityId) -> String {
    match engine.components_of(id).get(JOINT).and_then(|j| {
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
            Some((t.trim().parse().ok()?, v.trim().parse().ok()?))
        })
        .collect();
    out.sort_by(|a, b| a.0.total_cmp(&b.0));
    out
}

/// Encode `(t, value)` pairs (sorted by `t`; 17-sig-digit round-trippable f64 so a keyed pose replays
/// bit-identically).
#[must_use]
pub fn encode_track(keys: &[(f64, f64)]) -> String {
    let mut sorted: Vec<(f64, f64)> = keys.to_vec();
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
    let v = value.clamp(joint.min, joint.max);
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
    }
}
