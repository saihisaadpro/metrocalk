//! The companion brain's PURE math — every decision a companion makes during Play is a
//! deterministic function in this module, unit-tested away from the engine thread.
//!
//! The brain drives a REAL dynamic body (gravity + collision stay honest) through the one
//! replay-safe input channel the physics recording already has: frame-stamped impulses.
//! Per tick the shell asks [`steer_impulse`] for a horizontal correction toward the current
//! goal; vertical velocity is never touched, so falling and resting stay Rapier's business.
//!
//! Goal selection is a fixed priority ladder (see [`CompanionGoal`]): an enemy inside aggro
//! outranks following, following outranks patrol, patrol outranks standing still. There is
//! deliberately NO configuration surface for the ladder — a companion "just works" the moment
//! the card is clicked, and the three numbers a user might tune (speed / attack range /
//! follow distance) are ordinary component fields the Inspector already edits.
//!
//! "Pathfinding" here is honest local avoidance, not a navmesh: when the body has provably
//! stopped making progress toward a far goal (a wall, a crate, another actor), the brain
//! walks a fixed-length DETOUR leg perpendicular to the goal direction — side chosen
//! deterministically from the entity id so replays and both-companion scenes stay stable —
//! then re-seeks. Cheap, deterministic, and enough to round static obstacles.

use metrocalk_core::FieldValue;
use std::collections::HashMap;

/// Default move speed (m/s) written by the Companion role card.
pub const DEFAULT_SPEED: f64 = 2.5;
/// Default attack reach (m).
pub const DEFAULT_ATTACK_RANGE: f64 = 1.4;
/// Default follow stand-off (m) — close enough to feel loyal, far enough not to shove.
pub const DEFAULT_FOLLOW_DISTANCE: f64 = 2.0;
/// Default aggro reach (m) — how far the companion notices an enemy.
pub const DEFAULT_AGGRO: f64 = 6.0;
/// Ticks between attack swings (60 Hz → 1.5 s).
pub const ATTACK_COOLDOWN_TICKS: u32 = 90;
/// A waypoint is "reached" inside this distance (m).
pub const WAYPOINT_REACH: f64 = 0.6;
/// Progress check cadence (ticks) for stuck detection.
pub const STUCK_CHECK_TICKS: u32 = 30;
/// Minimum progress (m) per check window before the brain calls itself stuck.
pub const STUCK_EPSILON: f64 = 0.05;
/// Length of one detour leg (ticks).
pub const DETOUR_TICKS: u32 = 45;
/// Velocity-correction gain for the impulse P-controller.
pub const STEER_GAIN: f64 = 0.6;
/// Per-tick impulse magnitude clamp — keeps a blocked companion from winding up.
pub const MAX_IMPULSE: f64 = 4.0;
/// Default player move speed (m/s) — a touch quicker than a companion so you lead the party.
pub const DEFAULT_PLAYER_SPEED: f64 = 3.2;
/// A driven body only turns to face its travel above this speed (m/s) — resting bodies keep
/// their physics rotation instead of snapping to a stale heading.
pub const FACING_MIN_SPEED: f64 = 0.3;
/// How long a defeated enemy tumbles from the knockback before it vanishes (ticks, 60 Hz).
pub const DEFEAT_LINGER_TICKS: u32 = 40;

/// What the brain decided to do this tick.
#[derive(Debug, Clone, PartialEq)]
pub enum CompanionGoal {
    /// Close on an enemy (chase until inside attack range).
    Attack { enemy: String, goal: [f64; 3] },
    /// Hold formation behind a friendly prop.
    Follow { goal: [f64; 3] },
    /// Walk the waypoint chain.
    Patrol { goal: [f64; 3] },
    /// Nothing to do — damp to a stop.
    Idle,
}

/// Pick this tick's goal by the fixed priority ladder. `enemies`/`players`/`props`/`waypoints`
/// carry LIVE positions (both sides of every distance check move every tick). A PLAYER outranks
/// any prop as the follow target — your companion heels to YOU, not to the nearest crate.
#[must_use]
#[allow(clippy::too_many_arguments)] // one flat, readable decision ladder — not branching config
pub fn choose_goal(
    self_pos: [f64; 3],
    aggro: f64,
    follow_distance: f64,
    enemies: &[(String, [f64; 3])],
    players: &[[f64; 3]],
    props: &[[f64; 3]],
    waypoints: &[[f64; 3]],
    patrol_ix: usize,
) -> CompanionGoal {
    if let Some((key, pos)) = nearest_within(self_pos, enemies, aggro) {
        return CompanionGoal::Attack {
            enemy: key,
            goal: pos,
        };
    }
    let follow_pool = if players.is_empty() { props } else { players };
    if let Some(target) = nearest_pos(self_pos, follow_pool) {
        return CompanionGoal::Follow {
            goal: stand_off_goal(target, self_pos, follow_distance),
        };
    }
    if !waypoints.is_empty() {
        return CompanionGoal::Patrol {
            goal: waypoints[patrol_ix % waypoints.len()],
        };
    }
    CompanionGoal::Idle
}

/// The yaw-only quaternion (xyzw) that faces a body along its horizontal velocity, or `None`
/// below [`FACING_MIN_SPEED`] — a render-only projection, never a sim or document write.
#[must_use]
pub fn facing_quat(vel: [f64; 3]) -> Option<[f32; 4]> {
    let speed = (vel[0] * vel[0] + vel[2] * vel[2]).sqrt();
    if speed < FACING_MIN_SPEED {
        return None;
    }
    let yaw = vel[0].atan2(vel[2]);
    let half = yaw * 0.5;
    #[allow(clippy::cast_possible_truncation)]
    Some([0.0, half.sin() as f32, 0.0, half.cos() as f32])
}

/// The player's steering: same clamped P-controller as a companion, but the goal is a
/// DIRECTION (the pressed keys), not a point — full speed while held, damp to rest on release.
#[must_use]
pub fn player_impulse(vel: [f64; 3], axis: [f64; 2], speed: f64) -> [f64; 3] {
    let mag = (axis[0] * axis[0] + axis[1] * axis[1]).sqrt();
    let desired = if mag < 1.0e-6 {
        [0.0, 0.0]
    } else {
        [axis[0] / mag * speed, axis[1] / mag * speed]
    };
    let ix = (desired[0] - vel[0]) * STEER_GAIN;
    let iz = (desired[1] - vel[2]) * STEER_GAIN;
    let m = (ix * ix + iz * iz).sqrt();
    if m > MAX_IMPULSE {
        [ix / m * MAX_IMPULSE, 0.0, iz / m * MAX_IMPULSE]
    } else {
        [ix, 0.0, iz]
    }
}

/// The point `follow_distance` short of `target` along the target→self line: the companion
/// heels beside its friend instead of climbing into it. Degenerate overlap holds position.
#[must_use]
pub fn stand_off_goal(target: [f64; 3], self_pos: [f64; 3], follow_distance: f64) -> [f64; 3] {
    let dx = self_pos[0] - target[0];
    let dz = self_pos[2] - target[2];
    let d = (dx * dx + dz * dz).sqrt();
    if d < 1.0e-6 {
        return self_pos;
    }
    [
        target[0] + dx / d * follow_distance,
        target[1],
        target[2] + dz / d * follow_distance,
    ]
}

/// The horizontal impulse that nudges the body's velocity toward "walk at `speed` toward
/// `goal`" — a clamped P-controller. Inside `arrive` of the goal the desired speed fades to
/// zero so the companion settles instead of orbiting. Y is ALWAYS zero: gravity is Rapier's.
#[must_use]
pub fn steer_impulse(
    pos: [f64; 3],
    vel: [f64; 3],
    goal: [f64; 3],
    speed: f64,
    arrive: f64,
) -> [f64; 3] {
    let dx = goal[0] - pos[0];
    let dz = goal[2] - pos[2];
    let d = (dx * dx + dz * dz).sqrt();
    let desired = if d < 1.0e-6 {
        [0.0, 0.0]
    } else {
        // Linear arrival ramp: full speed outside `arrive`, proportional inside.
        let s = if d < arrive {
            speed * d / arrive
        } else {
            speed
        };
        [dx / d * s, dz / d * s]
    };
    let ix = (desired[0] - vel[0]) * STEER_GAIN;
    let iz = (desired[1] - vel[2]) * STEER_GAIN;
    let mag = (ix * ix + iz * iz).sqrt();
    if mag > MAX_IMPULSE {
        [ix / mag * MAX_IMPULSE, 0.0, iz / mag * MAX_IMPULSE]
    } else {
        [ix, 0.0, iz]
    }
}

/// Rotate a horizontal goal direction 90° for a detour leg. `side` is ±1.
#[must_use]
pub fn detour_goal(pos: [f64; 3], goal: [f64; 3], side: f64) -> [f64; 3] {
    let dx = goal[0] - pos[0];
    let dz = goal[2] - pos[2];
    [pos[0] - dz * side, pos[1], pos[2] + dx * side]
}

/// Deterministic detour side for an entity: stable across runs and replays.
#[must_use]
pub fn detour_side(key: &str) -> f64 {
    let sum: u32 = key.bytes().map(u32::from).sum();
    if sum.is_multiple_of(2) {
        1.0
    } else {
        -1.0
    }
}

/// Advance the patrol index when the current waypoint is reached (loops forever).
#[must_use]
pub fn advance_patrol(pos: [f64; 3], waypoints: &[[f64; 3]], ix: usize) -> usize {
    if waypoints.is_empty() {
        return 0;
    }
    let wp = waypoints[ix % waypoints.len()];
    let dx = wp[0] - pos[0];
    let dz = wp[2] - pos[2];
    if (dx * dx + dz * dz).sqrt() <= WAYPOINT_REACH {
        (ix + 1) % waypoints.len()
    } else {
        ix % waypoints.len()
    }
}

/// Squared horizontal distance — the shared range test.
#[must_use]
pub fn dist2_xz(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = a[0] - b[0];
    let dz = a[2] - b[2];
    dx * dx + dz * dz
}

fn nearest_within(
    from: [f64; 3],
    candidates: &[(String, [f64; 3])],
    reach: f64,
) -> Option<(String, [f64; 3])> {
    candidates
        .iter()
        .map(|(k, p)| (dist2_xz(from, *p), k, *p))
        .filter(|(d2, _, _)| *d2 <= reach * reach)
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, k, p)| (k.clone(), p))
}

fn nearest_pos(from: [f64; 3], candidates: &[[f64; 3]]) -> Option<[f64; 3]> {
    candidates
        .iter()
        .map(|p| (dist2_xz(from, *p), *p))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, p)| p)
}

/// Read a numeric GameRole field accepting BOTH JSON arms (the codified footgun).
#[must_use]
#[allow(clippy::implicit_hasher)] // callers pass the engine's concrete component maps
pub fn role_number(fields: &HashMap<String, FieldValue>, key: &str, default: f64) -> f64 {
    match fields.get(key) {
        Some(FieldValue::Number(n)) if *n > 0.0 => *n,
        Some(FieldValue::Integer(i)) if *i > 0 => {
            #[allow(clippy::cast_precision_loss)]
            {
                *i as f64
            }
        }
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_prefers_enemy_then_player_then_prop_then_patrol() {
        let enemies = vec![("e1".into(), [3.0, 0.0, 0.0])];
        let players = vec![[-8.0, 0.0, 0.0]];
        let props = vec![[10.0, 0.0, 0.0]];
        let wps = vec![[0.0, 0.0, 5.0]];
        let g = choose_goal([0.0; 3], 6.0, 2.0, &enemies, &players, &props, &wps, 0);
        assert!(matches!(g, CompanionGoal::Attack { .. }));
        // Enemy out of aggro → the PLAYER outranks the (nearer) prop as follow target.
        let g = choose_goal([0.0; 3], 2.0, 2.0, &enemies, &players, &props, &wps, 0);
        match g {
            CompanionGoal::Follow { goal } => {
                assert!(goal[0] < 0.0, "heels toward the player, not the prop");
            }
            other => panic!("expected follow, got {other:?}"),
        }
        // No player → props serve.
        let g = choose_goal([0.0; 3], 2.0, 2.0, &enemies, &[], &props, &wps, 0);
        assert!(matches!(g, CompanionGoal::Follow { .. }));
        // No follow target at all → patrol.
        let g = choose_goal([0.0; 3], 2.0, 2.0, &enemies, &[], &[], &wps, 0);
        assert!(matches!(g, CompanionGoal::Patrol { .. }));
        // Nothing → idle.
        let g = choose_goal([0.0; 3], 2.0, 2.0, &[], &[], &[], &[], 0);
        assert_eq!(g, CompanionGoal::Idle);
    }

    #[test]
    fn facing_tracks_travel_and_ignores_rest() {
        assert!(
            facing_quat([0.1, 0.0, 0.1]).is_none(),
            "resting keeps physics rotation"
        );
        let q = facing_quat([0.0, -3.0, 2.0]).expect("moving along +z");
        assert!(
            q[1].abs() < 1.0e-6 && (f64::from(q[3]) - 1.0).abs() < 1.0e-6,
            "+z = identity yaw"
        );
        let q = facing_quat([2.0, 0.0, 0.0]).expect("moving along +x");
        let yaw = 2.0 * f64::from(q[1]).asin();
        assert!(
            (yaw - std::f64::consts::FRAC_PI_2).abs() < 1.0e-6,
            "+x = 90° yaw"
        );
    }

    #[test]
    fn player_impulse_is_direction_driven_and_damps_on_release() {
        let go = player_impulse([0.0; 3], [1.0, 0.0], 3.2);
        assert!(go[0] > 0.0 && go[1].abs() < 1.0e-12 && go[2].abs() < 1.0e-12);
        // Keys released while moving: the correction opposes the velocity (a brake, not a coast).
        let brake = player_impulse([3.0, 0.0, 0.0], [0.0, 0.0], 3.2);
        assert!(brake[0] < 0.0);
        // Diagonals normalize — no sqrt(2) speed advantage.
        let diag = player_impulse([0.0; 3], [1.0, 1.0], 3.2);
        let straight = player_impulse([0.0; 3], [1.0, 0.0], 3.2);
        let mag = |v: [f64; 3]| (v[0] * v[0] + v[2] * v[2]).sqrt();
        assert!((mag(diag) - mag(straight)).abs() < 1.0e-9);
    }

    #[test]
    fn stand_off_stays_short_of_the_target() {
        let goal = stand_off_goal([0.0; 3], [10.0, 0.0, 0.0], 2.0);
        assert!((goal[0] - 2.0).abs() < 1.0e-9 && goal[2].abs() < 1.0e-9);
        // Overlapping target: hold position rather than divide by zero.
        let hold = stand_off_goal([5.0, 0.0, 5.0], [5.0, 0.0, 5.0], 2.0);
        assert!(hold
            .iter()
            .zip([5.0, 0.0, 5.0])
            .all(|(a, b)| (a - b).abs() < 1.0e-12));
    }

    #[test]
    fn steering_never_touches_vertical_velocity_and_clamps() {
        let i = steer_impulse([0.0; 3], [0.0, -9.0, 0.0], [100.0, 0.0, 0.0], 2.5, 1.0);
        assert!(i[1].abs() < 1.0e-12, "gravity belongs to the sim");
        let mag = (i[0] * i[0] + i[2] * i[2]).sqrt();
        assert!(mag <= MAX_IMPULSE + 1.0e-9);
        // At the goal with no velocity: nothing to correct.
        let i = steer_impulse([1.0, 0.0, 1.0], [0.0; 3], [1.0, 0.0, 1.0], 2.5, 1.0);
        assert!(i[0].abs() < 1.0e-9 && i[2].abs() < 1.0e-9);
    }

    #[test]
    fn arrival_ramp_slows_near_goal() {
        let far = steer_impulse([0.0; 3], [0.0; 3], [10.0, 0.0, 0.0], 2.0, 2.0);
        let near = steer_impulse([9.5, 0.0, 0.0], [0.0; 3], [10.0, 0.0, 0.0], 2.0, 2.0);
        assert!(near[0] < far[0], "desired speed must fade inside arrive");
    }

    #[test]
    fn patrol_loops_and_detour_is_perpendicular_and_stable() {
        let wps = vec![[0.0; 3], [4.0, 0.0, 0.0]];
        assert_eq!(advance_patrol([0.1, 0.0, 0.0], &wps, 0), 1);
        assert_eq!(advance_patrol([4.0, 0.0, 0.1], &wps, 1), 0, "loops");
        assert_eq!(advance_patrol([2.0, 0.0, 0.0], &wps, 0), 0, "mid-leg holds");
        let d = detour_goal([0.0; 3], [2.0, 0.0, 0.0], 1.0);
        assert!((d[2] - 2.0).abs() < 1.0e-9, "90° turn");
        assert!(
            detour_side("abc").to_bits() == detour_side("abc").to_bits(),
            "deterministic"
        );
    }

    #[test]
    fn nearest_enemy_ties_break_by_key_for_determinism() {
        let enemies = vec![
            ("b".to_string(), [2.0, 0.0, 0.0]),
            ("a".to_string(), [-2.0, 0.0, 0.0]),
        ];
        let g = choose_goal([0.0; 3], 6.0, 2.0, &enemies, &[], &[], &[], 0);
        match g {
            CompanionGoal::Attack { enemy, .. } => assert_eq!(enemy, "a"),
            other => panic!("expected attack, got {other:?}"),
        }
    }
}
