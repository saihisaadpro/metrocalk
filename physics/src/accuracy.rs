//! Validated f64 accuracy (M8.5 deliverable 3) — a reference suite against **closed-form / canonical**
//! cases, each measured value reported **vs the reference it's measured against**, with an honest error
//! bound. This is what earns the word "validated": not "trust us, it's f64", but *here is the pendulum
//! period the analytic formula predicts, here is what the sim produced, here is the error*.
//!
//! These run through the [`Physics`] trait on the authoritative deterministic config, so the numbers are
//! reproducible (the M8.1 hash backs them). They are the substrate of the **`Validation` fidelity rung**
//! (M8.2's declared facet, now real): declaring `Validation` means "I want the accuracy these bounds
//! report." The honest caveat is stated per case — an impulse-based rigid-body solver has O(`dt`)
//! discretization error and approximate restitution; this suite *measures* that rather than hiding it.

// Physics/math code: x/y/z/w quaternion components + i/n loop counts are the canonical names, and step
// counts → f64 for timing/averaging lose no meaningful precision at these magnitudes.
#![allow(clippy::many_single_char_names, clippy::cast_precision_loss)]

use crate::{
    BodyDesc, BodyKind, ColliderDesc, ColliderShape, JointDesc, Physics, PhysicsConfig, Quat,
    RapierPhysics, Vec3,
};

/// One validated case: a measured quantity against a named closed-form reference, with the error + bound.
#[derive(Clone, Debug, PartialEq)]
pub struct AccuracyCase {
    pub name: String,
    /// The closed-form / canonical value (the ground truth).
    pub reference: f64,
    /// What the simulation produced.
    pub measured: f64,
    /// The error against the reference — RELATIVE (a fraction) when the reference is non-zero, or the
    /// ABSOLUTE deviation (in the quantity's own units) when the reference is ~0 (e.g. ideal-zero drift).
    pub error: f64,
    /// The stated bound `error` must hold within — relative or absolute, matching `error`.
    pub tolerance: f64,
    /// `true` ⇒ `error`/`tolerance` are absolute (the reference is ~0); `false` ⇒ relative.
    pub absolute: bool,
    /// The formula / citation the reference comes from — a number with no reference is a failed
    /// deliverable, so this is never empty.
    pub reference_source: String,
}

impl AccuracyCase {
    fn new(
        name: &str,
        reference: f64,
        measured: f64,
        tolerance: f64,
        reference_source: &str,
    ) -> Self {
        // A reference of ~0 (ideal-zero drift) has no meaningful RELATIVE error — compare the absolute
        // deviation against an absolute bound; otherwise use the relative error.
        let absolute = reference.abs() < 1e-9;
        let error = if absolute {
            (measured - reference).abs()
        } else {
            (measured - reference).abs() / reference.abs()
        };
        Self {
            name: name.into(),
            reference,
            measured,
            error,
            tolerance,
            absolute,
            reference_source: reference_source.into(),
        }
    }
    /// `true` when the measured value is within the stated bound of the reference.
    #[must_use]
    pub fn within_bound(&self) -> bool {
        self.error <= self.tolerance
    }
    /// A one-line report (what the editor's `Validation` fidelity surfaces, and the bench prints).
    #[must_use]
    pub fn report(&self) -> String {
        let (err, bound) = if self.absolute {
            (
                format!("{:.6} abs", self.error),
                format!("{:.6}", self.tolerance),
            )
        } else {
            (
                format!("{:.3}%", self.error * 100.0),
                format!("{:.1}%", self.tolerance * 100.0),
            )
        };
        format!(
            "{}: measured {:.6} vs reference {:.6} ({}) → {err} error (bound {bound}) [{}]",
            self.name,
            self.measured,
            self.reference,
            self.reference_source,
            if self.within_bound() { "PASS" } else { "FAIL" },
        )
    }
}

const G: f64 = 9.81;

/// Rotate `v` by unit quaternion `q` `[x,y,z,w]` — to read a body-local anchor in world space.
fn rotate(q: Quat, v: Vec3) -> Vec3 {
    let [x, y, z, w] = q;
    let tx = 2.0 * (y * v[2] - z * v[1]);
    let ty = 2.0 * (z * v[0] - x * v[2]);
    let tz = 2.0 * (x * v[1] - y * v[0]);
    [
        v[0] + w * tx + (y * tz - z * ty),
        v[1] + w * ty + (z * tx - x * tz),
        v[2] + w * tz + (x * ty - y * tx),
    ]
}

fn world_point(p: &RapierPhysics, body: crate::BodyHandle, local: Vec3) -> Vec3 {
    let (t, q) = p
        .transform(body)
        .unwrap_or(([0.0; 3], [0.0, 0.0, 0.0, 1.0]));
    let r = rotate(q, local);
    [t[0] + r[0], t[1] + r[1], t[2] + r[2]]
}

/// **Free fall** vs `y(t) = y₀ − ½ g t²` (constant-acceleration kinematics). The semi-implicit Euler
/// integrator has a known O(`dt`) position error (it's exact in velocity), so this *measures* that error
/// against the continuous closed form rather than asserting perfection.
#[must_use]
pub fn free_fall() -> AccuracyCase {
    let mut p = RapierPhysics::new(PhysicsConfig::default());
    let y0 = 100.0;
    let b = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [0.0, y0, 0.0]));
    p.add_collider(b, &ColliderDesc::new(ColliderShape::Ball { radius: 0.1 }))
        .unwrap();
    let steps = 120u32;
    for _ in 0..steps {
        p.step();
    }
    let dt = p.config().fixed_dt;
    let t = f64::from(steps) * dt;
    let reference = y0 - 0.5 * G * t * t;
    let measured = p.transform(b).unwrap().0[1];
    AccuracyCase::new(
        "free fall (drop)",
        y0 - reference, // the DROP distance (so rel error is meaningful, not dominated by y0)
        y0 - measured,
        0.03,
        "y = y0 - 1/2 g t^2 (kinematics)",
    )
}

/// **Projectile range** vs `R = v₀² sin(2θ) / g` (no drag). Launched at 45°; the landing x (where the
/// parabola re-crosses its launch height) is measured by linear interpolation across the crossing step.
#[must_use]
pub fn projectile_range() -> AccuracyCase {
    let mut p = RapierPhysics::new(PhysicsConfig::default());
    let v0 = 12.0;
    let theta = std::f64::consts::FRAC_PI_4; // 45°
    let (vx, vy) = (v0 * theta.cos(), v0 * theta.sin());
    let y_launch = 0.05;
    let b = p.add_body(&BodyDesc {
        kind: BodyKind::Dynamic,
        translation: [0.0, y_launch, 0.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        linvel: [vx, vy, 0.0],
        angvel: [0.0; 3],
        can_sleep: false,
    });
    p.add_collider(b, &ColliderDesc::new(ColliderShape::Ball { radius: 0.01 }))
        .unwrap();
    let mut prev = (0.0, y_launch);
    let mut measured = 0.0;
    for _ in 0..2000 {
        p.step();
        let t = p.transform(b).unwrap().0;
        let (x, y) = (t[0], t[1]);
        if y <= y_launch && prev.1 > y_launch {
            // linear interpolation to the launch-height crossing
            let frac = (prev.1 - y_launch) / (prev.1 - y);
            measured = prev.0 + (x - prev.0) * frac;
            break;
        }
        prev = (x, y);
    }
    let reference = v0 * v0 * (2.0 * theta).sin() / G;
    AccuracyCase::new(
        "projectile range (45°)",
        reference,
        measured,
        0.03,
        "R = v0^2 sin(2θ) / g (ballistics)",
    )
}

/// **Simple-pendulum period** vs `T = 2π √(L/g)` (small-angle). A dynamic bob hangs length `L` below a
/// fixed pivot on a revolute joint, released from ~6°; the period is measured from the bob's x-swing
/// zero-crossings over several oscillations.
#[must_use]
pub fn pendulum_period() -> AccuracyCase {
    let mut p = RapierPhysics::new(PhysicsConfig::default());
    let length = 1.0;
    let pivot = p.add_body(&BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]));
    // released ~6° off vertical
    let angle = 6.0_f64.to_radians();
    let bob_pos = [length * angle.sin(), -length * angle.cos(), 0.0];
    let bob = p.add_body(&BodyDesc::new(BodyKind::Dynamic, bob_pos).never_sleeping());
    p.add_collider(
        bob,
        &ColliderDesc::new(ColliderShape::Ball { radius: 0.02 }),
    )
    .unwrap();
    // revolute about Z at the pivot; anchor on the pivot body = origin, anchor on the bob = its top point
    p.add_joint(
        pivot,
        bob,
        &JointDesc::Revolute {
            axis: [0.0, 0.0, 1.0],
            anchor_a: [0.0, 0.0, 0.0],
            anchor_b: [-bob_pos[0], -bob_pos[1], 0.0],
        },
    )
    .unwrap();
    let dt = p.config().fixed_dt;
    let mut crossings: Vec<f64> = Vec::new();
    let mut prev_x = bob_pos[0];
    for n in 0..2000u32 {
        p.step();
        let x = p.transform(bob).unwrap().0[0];
        if (prev_x > 0.0 && x <= 0.0) || (prev_x < 0.0 && x >= 0.0) {
            crossings.push(f64::from(n) * dt);
        }
        prev_x = x;
    }
    // a full period = two zero-crossings; average the half-periods over all crossings
    let measured = if crossings.len() >= 3 {
        let span = crossings.last().unwrap() - crossings.first().unwrap();
        2.0 * span / (crossings.len() as f64 - 1.0)
    } else {
        0.0
    };
    let reference = std::f64::consts::TAU * (length / G).sqrt();
    AccuracyCase::new(
        "pendulum period",
        reference,
        measured,
        0.05,
        "T = 2π√(L/g) (small-angle pendulum)",
    )
}

/// **Restitution / energy** vs `h_rebound = e² · h_drop` (Newton's restitution). A ball of restitution
/// `e` dropped onto a floor of restitution `e`; the rebound apex is measured. Impulse-based restitution is
/// approximate, so the bound is honestly looser — and the *measured* energy ratio is the point.
#[must_use]
pub fn restitution_energy() -> AccuracyCase {
    let e = 0.8;
    let mut p = RapierPhysics::new(PhysicsConfig::default());
    let ground = p.add_body(&BodyDesc::new(BodyKind::Fixed, [0.0, 0.0, 0.0]));
    p.add_collider(
        ground,
        &ColliderDesc {
            shape: ColliderShape::Cuboid {
                half_extents: [10.0, 0.5, 10.0],
            },
            density: 1.0,
            friction: 0.5,
            restitution: e,
        },
    )
    .unwrap();
    let radius = 0.2;
    let drop_apex = 3.0; // ball centre apex height above the floor surface (y=0.5)
    let start_y = 0.5 + drop_apex + radius;
    let b = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [0.0, start_y, 0.0]));
    p.add_collider(
        b,
        &ColliderDesc {
            shape: ColliderShape::Ball { radius },
            density: 1.0,
            friction: 0.5,
            restitution: e,
        },
    )
    .unwrap();
    // drop, bounce, and track the post-bounce apex (the first local maximum after the velocity flips up)
    let mut bounced = false;
    let mut apex = 0.0f64;
    let mut prev_y = start_y;
    for _ in 0..2000 {
        p.step();
        let y = p.transform(b).unwrap().0[1];
        let vy = p.velocity(b).unwrap().0[1];
        if vy > 0.0 {
            bounced = true;
        }
        if bounced {
            apex = apex.max(y);
            if y < prev_y && vy < 0.0 {
                break; // past the rebound apex
            }
        }
        prev_y = y;
    }
    let rebound = (apex - radius - 0.5).max(0.0); // back to apex-above-floor
    let measured_ratio = rebound / drop_apex;
    AccuracyCase::new(
        "restitution energy ratio",
        e * e,
        measured_ratio,
        0.15,
        "h_rebound/h_drop = e^2 (Newton restitution)",
    )
}

/// **Constraint drift over 10k steps** vs an ideal rigid joint (reference 0). A dynamic body hangs from a
/// fixed body on a revolute joint and swings under gravity; the joint anchor points should stay coincident
/// — the max separation over 10 000 steps is the measured drift (a hard solver-stability case). Reported
/// in metres against a 1 mm bound.
#[must_use]
pub fn constraint_drift_10k() -> AccuracyCase {
    let mut p = RapierPhysics::new(PhysicsConfig::default());
    let anchor = p.add_body(&BodyDesc::new(BodyKind::Fixed, [0.0, 5.0, 0.0]));
    let arm = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [1.0, 5.0, 0.0]).never_sleeping());
    p.add_collider(
        arm,
        &ColliderDesc::new(ColliderShape::Cuboid {
            half_extents: [0.5, 0.05, 0.05],
        }),
    )
    .unwrap();
    let anchor_a = [0.0, 0.0, 0.0]; // pivot on the fixed body
    let anchor_b = [-1.0, 0.0, 0.0]; // the arm's end that meets the pivot
    p.add_joint(
        anchor,
        arm,
        &JointDesc::Revolute {
            axis: [0.0, 0.0, 1.0],
            anchor_a,
            anchor_b,
        },
    )
    .unwrap();
    let mut max_drift = 0.0f64;
    for _ in 0..10_000 {
        p.step();
        let wa = world_point(&p, anchor, anchor_a);
        let wb = world_point(&p, arm, anchor_b);
        let d =
            ((wa[0] - wb[0]).powi(2) + (wa[1] - wb[1]).powi(2) + (wa[2] - wb[2]).powi(2)).sqrt();
        max_drift = max_drift.max(d);
    }
    AccuracyCase::new(
        "revolute constraint drift (10k steps)",
        0.0,
        max_drift,
        0.001, // 1 mm absolute (rel_error vs 0 falls back to the abs value)
        "ideal rigid joint: anchor separation = 0",
    )
}

/// **Multibody stability** — a 2-link chain (double pendulum) is chaotic, so there's no closed-form
/// trajectory; total mechanical energy *should* be conserved (frictionless), and its drift is the honest
/// stability metric. This is the deliverable-4 **honest characterization**, NOT a tight-conservation
/// claim: the reduced-coordinate multibody solver is **dissipative and lossy** — energy drifts
/// substantially over a long run (Dimforge's acknowledged open multibody/joint-solver bugs; the
/// MuJoCo-inspired accuracy solver is 2026-planned, not shipped). What the bound asserts is **stability**
/// — the energy stays bounded and DISSIPATES (it never grows / explodes, the instability signature). The
/// measured drift is reported plainly; we don't ship long-horizon multibody conservation on this solver.
#[must_use]
pub fn multibody_energy_drift() -> AccuracyCase {
    let mut p = RapierPhysics::new(PhysicsConfig::default());
    let pivot = p.add_body(&BodyDesc::new(BodyKind::Fixed, [0.0, 3.0, 0.0]));
    let l1 = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [0.6, 3.0, 0.0]).never_sleeping());
    let l2 = p.add_body(&BodyDesc::new(BodyKind::Dynamic, [1.2, 3.0, 0.0]).never_sleeping());
    for b in [l1, l2] {
        p.add_collider(
            b,
            &ColliderDesc::new(ColliderShape::Cuboid {
                half_extents: [0.3, 0.04, 0.04],
            }),
        )
        .unwrap();
    }
    p.add_joint(
        pivot,
        l1,
        &JointDesc::Revolute {
            axis: [0.0, 0.0, 1.0],
            anchor_a: [0.0, 0.0, 0.0],
            anchor_b: [-0.6, 0.0, 0.0],
        },
    )
    .unwrap();
    p.add_joint(
        l1,
        l2,
        &JointDesc::Revolute {
            axis: [0.0, 0.0, 1.0],
            anchor_a: [0.6, 0.0, 0.0],
            anchor_b: [-0.6, 0.0, 0.0],
        },
    )
    .unwrap();
    let energy = |p: &RapierPhysics| -> f64 {
        // total mechanical energy via the diagnostics seam (kinetic + potential, M8.4)
        p.diagnostics().total_energy
    };
    // settle one step so the joints engage, then take the baseline
    p.step();
    let e0 = energy(&p);
    for _ in 0..3000 {
        p.step();
    }
    let e1 = energy(&p);
    AccuracyCase::new(
        "double-pendulum stability (energy drift, 3k steps)",
        e0,
        e1,
        // 50% bound = "stable + dissipative, doesn't explode" (the energy must STAY BOUNDED, not a
        // conservation claim). The measured drift (~40%) is the HONEST multibody characterization — the
        // test separately asserts energy does not GROW (the real instability signature).
        0.50,
        "total mechanical energy is conserved (frictionless) — honest stability bound, lossy multibody solver",
    )
}

/// The full validated-accuracy suite (M8.5 deliverable 3) — every case measured vs its named reference.
/// The editor's `Validation` fidelity surfaces these bounds; the bench/test prints the report.
#[must_use]
pub fn run_suite() -> Vec<AccuracyCase> {
    vec![
        free_fall(),
        projectile_range(),
        pendulum_period(),
        restitution_energy(),
        constraint_drift_10k(),
        multibody_energy_drift(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validated_accuracy_suite_holds_against_closed_form_references() {
        let suite = run_suite();
        for case in &suite {
            // The report cites the reference (never a bare number) and the measurement is within bound.
            assert!(
                !case.reference_source.is_empty(),
                "every accuracy number must cite its reference"
            );
            eprintln!("[M8.5] {}", case.report());
            assert!(
                case.within_bound(),
                "accuracy out of bound: {}",
                case.report()
            );
        }
        assert_eq!(suite.len(), 6, "the full reference suite ran");
    }

    #[test]
    fn multibody_is_stable_dissipative_not_exploding() {
        // The honest multibody stability property (deliverable 4): the chaotic double pendulum loses
        // energy (dissipative) but NEVER GAINS it — energy growth is the instability/explosion signature.
        let case = multibody_energy_drift();
        assert!(
            case.measured.is_finite(),
            "energy stayed finite (no blowup)"
        );
        assert!(
            case.measured <= case.reference * 1.05,
            "energy must not GROW (instability): {} → {}",
            case.reference,
            case.measured
        );
        assert!(
            case.measured > 0.0,
            "the system didn't collapse to zero energy"
        );
    }
}
