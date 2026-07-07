//! Analytic curved-surface tessellation (M15.8 / ADR-078) — the **kernel-free half** of the curved-fidelity
//! gap: `CYLINDRICAL_SURFACE` / `CONICAL_SURFACE` / `SPHERICAL_SURFACE` / `TOROIDAL_SURFACE` have exact
//! closed forms, so a bore, boss, fillet, dome, or torus ring tessellates **smooth, deterministic, and
//! adaptive** (absolute deflection — a fixed sag in scene units, never relative/parallel) with no C++
//! kernel. **NURBS / freeform is NEVER hand-rolled** (the research AVOID: trimmed-NURBS intersection is a
//! multi-year problem) — those faces keep their explained OCCT/licensed-kernel note.
//!
//! **The declared subset (honesty first):** a face tessellates here only when (a) its surface is one of the
//! four analytic kinds, (b) every boundary vertex lies ON the surface (within [`ON_SURFACE_TOL`] — a
//! malformed or mis-referenced bound is skipped + noted, never guessed), and (c) its trim projects to a
//! **u–v parameter rectangle** (every boundary vertex sits on the param-rect border — the bore / boss /
//! fillet / cap / ring shape). A complex trim (a hole drilled through a cylinder wall, a freeform trim
//! curve) is beyond the subset → the face keeps the explained seam note. Nothing is ever silently wrong:
//! over-drawing a trimmed region would be *silently wrong geometry*, so the gate rejects instead.
//!
//! **Determinism:** segment counts are integer functions of the exact surface parameters + the fixed
//! absolute deflection; grid points are closed-form `f64` — same file → same mesh, bit-identical on a
//! platform. (Cross-platform bit-identity rides libm's `sin`/`cos`, which is NOT guaranteed correctly
//! rounded — the per-platform CI hash corpus gates same-platform determinism; cross-platform equality is
//! observed, not assumed — the ADR-020 discipline.)

// Surface math reads best in its conventional notation (triangle verts a/b/c/d, u/v parameter borders) —
// the "similar/single-char names" pedantic lints fight that idiom, as in the rasterizer evidence tool.
#![allow(clippy::similar_names, clippy::many_single_char_names)]

use crate::UnsupportedNote;

const TAU: f64 = std::f64::consts::TAU;

/// A recognized analytic curved surface — closed-form, deterministically tessellatable. The `frame` is the
/// surface's `AXIS2_PLACEMENT_3D` as a column-major rigid 4×4 (x/y/z basis columns + origin).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AnalyticSurface {
    /// Radius-`radius` cylinder around the frame's z axis: `P(u,v) = o + r·(cos u·x + sin u·y) + v·z`.
    Cylinder { frame: [f64; 16], radius: f64 },
    /// Cone: radius `radius` at v=0 growing by `tan(semi_angle)` per unit v along z.
    Cone {
        frame: [f64; 16],
        radius: f64,
        semi_angle: f64,
    },
    /// Sphere of `radius` centred at the frame origin; u = longitude, v = latitude (−π/2..π/2).
    Sphere { frame: [f64; 16], radius: f64 },
    /// Torus: `major` ring radius around z, `minor` tube radius; u = ring angle, v = tube angle.
    Torus {
        frame: [f64; 16],
        major: f64,
        minor: f64,
    },
}

/// Boundary vertices must lie on the surface within this distance (scene units — mm for STEP) or the face
/// is beyond the declared subset (a mis-referenced bound / a trim we can't represent) → skipped + noted.
const ON_SURFACE_TOL: f64 = 1e-3;

/// The absolute chord deflection (scene units — mm for STEP): the max sag between the true surface and a
/// triangle edge. Absolute (never relative) so tessellation is independent of batch order / bbox — the
/// determinism precondition the research names.
pub const DEFLECTION: f64 = 0.2;

/// The max angular sweep of one segment (radians) — caps segment size on huge radii where the sag bound
/// alone would allow visibly-flat 90° facets.
const MAX_SEG_ANGLE: f64 = std::f64::consts::PI / 8.0; // 22.5°

/// Segment count for an arc of `sweep` radians at curvature radius `r`, from the absolute sag `d`:
/// a chord over angle θ sags `r·(1−cos(θ/2))` → θ = 2·acos(1 − d/r), capped by [`MAX_SEG_ANGLE`], with a
/// hard ceiling so an adversarial radius can't allocate unbounded segments. Deterministic: an integer
/// function of exact inputs.
fn segments(sweep: f64, r: f64, d: f64) -> u32 {
    if !(sweep.is_finite() && r.is_finite()) || sweep <= 0.0 || r <= 0.0 {
        return 1;
    }
    let theta_sag = if d < r {
        2.0 * (1.0 - d / r).acos()
    } else {
        MAX_SEG_ANGLE
    };
    let theta = theta_sag.clamp(1e-4, MAX_SEG_ANGLE);
    let n = (sweep / theta).ceil();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = n as u32;
    n.clamp(2, 512)
}

// ── frame helpers (column-major rigid 4×4) ───────────────────────────────────────────────────────────────
fn f_origin(m: &[f64; 16]) -> [f64; 3] {
    [m[12], m[13], m[14]]
}
fn f_x(m: &[f64; 16]) -> [f64; 3] {
    [m[0], m[1], m[2]]
}
fn f_y(m: &[f64; 16]) -> [f64; 3] {
    [m[4], m[5], m[6]]
}
fn f_z(m: &[f64; 16]) -> [f64; 3] {
    [m[8], m[9], m[10]]
}
fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn norm3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}

impl AnalyticSurface {
    /// The world point at parameters `(u, v)`.
    fn point(&self, u: f64, v: f64) -> [f64; 3] {
        match *self {
            Self::Cylinder { frame, radius } => {
                let radial = add3(
                    scale3(f_x(&frame), radius * u.cos()),
                    scale3(f_y(&frame), radius * u.sin()),
                );
                add3(add3(f_origin(&frame), radial), scale3(f_z(&frame), v))
            }
            Self::Cone {
                frame,
                radius,
                semi_angle,
            } => {
                let r = (radius + v * semi_angle.tan()).max(0.0);
                let radial = add3(
                    scale3(f_x(&frame), r * u.cos()),
                    scale3(f_y(&frame), r * u.sin()),
                );
                add3(add3(f_origin(&frame), radial), scale3(f_z(&frame), v))
            }
            Self::Sphere { frame, radius } => {
                let (cu, su, cv, sv) = (u.cos(), u.sin(), v.cos(), v.sin());
                let p = add3(
                    add3(
                        scale3(f_x(&frame), radius * cv * cu),
                        scale3(f_y(&frame), radius * cv * su),
                    ),
                    scale3(f_z(&frame), radius * sv),
                );
                add3(f_origin(&frame), p)
            }
            Self::Torus {
                frame,
                major,
                minor,
            } => {
                let ring = major + minor * v.cos();
                let p = add3(
                    add3(
                        scale3(f_x(&frame), ring * u.cos()),
                        scale3(f_y(&frame), ring * u.sin()),
                    ),
                    scale3(f_z(&frame), minor * v.sin()),
                );
                add3(f_origin(&frame), p)
            }
        }
    }

    /// The exact outward surface normal at `(u, v)` (before `same_sense`): radially out of the axis /
    /// centre / tube — the STEP convention for these surfaces' positive orientation.
    fn normal(&self, u: f64, v: f64) -> [f64; 3] {
        match *self {
            Self::Cylinder { frame, .. } => {
                add3(scale3(f_x(&frame), u.cos()), scale3(f_y(&frame), u.sin()))
            }
            Self::Cone {
                frame, semi_angle, ..
            } => {
                // Radial direction tilted back by the cone slope: n = cos(α)·radial − sin(α)·z.
                let radial = add3(scale3(f_x(&frame), u.cos()), scale3(f_y(&frame), u.sin()));
                add3(
                    scale3(radial, semi_angle.cos()),
                    scale3(f_z(&frame), -semi_angle.sin()),
                )
            }
            Self::Sphere { frame, .. } => {
                let (cu, su, cv, sv) = (u.cos(), u.sin(), v.cos(), v.sin());
                add3(
                    add3(scale3(f_x(&frame), cv * cu), scale3(f_y(&frame), cv * su)),
                    scale3(f_z(&frame), sv),
                )
            }
            Self::Torus { frame, .. } => {
                let (cu, su, cv, sv) = (u.cos(), u.sin(), v.cos(), v.sin());
                let radial = add3(scale3(f_x(&frame), cu), scale3(f_y(&frame), su));
                add3(scale3(radial, cv), scale3(f_z(&frame), sv))
            }
        }
    }

    /// Project a world point to `(u, v)` + its distance OFF the surface (the on-surface check). A point on
    /// a parameter pole (sphere pole / cone apex — u undefined) reports `u_valid = false`.
    fn project(&self, p: [f64; 3]) -> Projected {
        match *self {
            Self::Cylinder { frame, radius } => {
                let d = sub3(p, f_origin(&frame));
                let v = dot3(d, f_z(&frame));
                let (px, py) = (dot3(d, f_x(&frame)), dot3(d, f_y(&frame)));
                let r = (px * px + py * py).sqrt();
                Projected {
                    u: py.atan2(px),
                    v,
                    off: (r - radius).abs(),
                    u_valid: r > ON_SURFACE_TOL,
                }
            }
            Self::Cone {
                frame,
                radius,
                semi_angle,
            } => {
                let d = sub3(p, f_origin(&frame));
                let v = dot3(d, f_z(&frame));
                let (px, py) = (dot3(d, f_x(&frame)), dot3(d, f_y(&frame)));
                let r = (px * px + py * py).sqrt();
                let expect = radius + v * semi_angle.tan();
                Projected {
                    u: py.atan2(px),
                    v,
                    off: (r - expect).abs() * semi_angle.cos(),
                    u_valid: r > ON_SURFACE_TOL,
                }
            }
            Self::Sphere { frame, radius } => {
                let d = sub3(p, f_origin(&frame));
                let (px, py, pz) = (
                    dot3(d, f_x(&frame)),
                    dot3(d, f_y(&frame)),
                    dot3(d, f_z(&frame)),
                );
                let r = norm3([px, py, pz]);
                let rxy = (px * px + py * py).sqrt();
                Projected {
                    u: py.atan2(px),
                    v: pz.atan2(rxy),
                    off: (r - radius).abs(),
                    u_valid: rxy > ON_SURFACE_TOL,
                }
            }
            Self::Torus {
                frame,
                major,
                minor,
            } => {
                let d = sub3(p, f_origin(&frame));
                let (px, py, pz) = (
                    dot3(d, f_x(&frame)),
                    dot3(d, f_y(&frame)),
                    dot3(d, f_z(&frame)),
                );
                let rxy = (px * px + py * py).sqrt();
                let ring = rxy - major;
                Projected {
                    u: py.atan2(px),
                    v: pz.atan2(ring),
                    off: ((ring * ring + pz * pz).sqrt() - minor).abs(),
                    u_valid: rxy > ON_SURFACE_TOL,
                }
            }
        }
    }

    /// The curvature radius governing u-direction sag (per-face constant — the largest, so segments err
    /// dense) and the v-direction one (`None` ⇒ v is straight → a single segment spans it).
    fn curvatures(&self) -> (f64, Option<f64>) {
        match *self {
            Self::Cylinder { radius, .. } => (radius, None),
            Self::Cone {
                radius, semi_angle, ..
            } => (radius.max(1e-6) / semi_angle.cos().max(1e-6), None),
            Self::Sphere { radius, .. } => (radius, Some(radius)),
            Self::Torus { major, minor, .. } => (major + minor, Some(minor)),
        }
    }
}

struct Projected {
    u: f64,
    v: f64,
    off: f64,
    u_valid: bool,
}

/// The tessellated patch for one analytic face: grid triangles + the exact per-triangle outward normal
/// test vector (the caller welds vertices + orients by it).
#[derive(Debug)]
pub struct AnalyticPatch {
    /// Grid vertices, row-major (`(nu+1) × (nv+1)`).
    pub positions: Vec<[f64; 3]>,
    /// Triangles as indices into `positions`, ORIENTED so the winding normal matches the face's outward
    /// (analytic × `same_sense`) normal.
    pub triangles: Vec<[u32; 3]>,
}

/// The validated tessellation plan for one analytic face — the (u, v) parameter rectangle + the adaptive
/// segment counts. Computing it is CHEAP (projections only, no grid), so `interpret_face` runs it at parse
/// time: a face beyond the subset gets its explained note THERE (never a silent skip later).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AnalyticPlan {
    u0: f64,
    sweep: f64,
    v0: f64,
    v1: f64,
    nu: u32,
    nv: u32,
}

/// Validate an analytic face's boundary against the declared subset and produce its tessellation plan.
/// `Err(note)` = beyond the subset (off-surface bounds / non-rectangular trim / degenerate patch) — the
/// caller surfaces the note; the face is never guessed.
pub fn plan_analytic(
    face_id: u64,
    surface: &AnalyticSurface,
    boundary: &[[f64; 3]],
) -> Result<AnalyticPlan, UnsupportedNote> {
    let beyond = |why: &str| UnsupportedNote {
        feature: format!("analytic surface on face #{face_id}"),
        detail: format!(
            "{why} — beyond the declared analytic-tessellation subset; the exact trim is the \
             licensed-kernel/OCCT seam (ADR-078)"
        ),
    };
    if boundary.is_empty() {
        return Err(beyond("the face has no boundary vertices"));
    }

    // (a) Every boundary vertex must lie ON the surface — else the bound doesn't belong to this surface
    // (malformed / a trim we can't project) and tessellating would be silently wrong geometry.
    let projected: Vec<Projected> = boundary.iter().map(|&p| surface.project(p)).collect();
    if let Some(p) = projected.iter().find(|p| p.off > ON_SURFACE_TOL) {
        return Err(beyond(&format!(
            "a boundary vertex lies {:.3} units off the surface",
            p.off
        )));
    }

    // v-range straight from the projections.
    let (mut v0, mut v1) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &projected {
        v0 = v0.min(p.v);
        v1 = v1.max(p.v);
    }
    if !(v0.is_finite() && v1.is_finite()) {
        return Err(beyond("the boundary projects to a degenerate v-range"));
    }

    // u-range: only pole-free vertices constrain u. A closed revolution's seam vertices cluster at one u —
    // detect via the largest angular gap: if the boundary's u-values leave a gap ≥ ~340°, the face wraps
    // the full revolution; otherwise the swept arc is the complement of the largest gap (branch-cut-safe:
    // works for arcs crossing ±π).
    let mut us: Vec<f64> = projected
        .iter()
        .filter(|p| p.u_valid)
        .map(|p| p.u)
        .collect();
    let (u0, sweep) = if us.is_empty() {
        (0.0, TAU) // all vertices on the pole/axis (e.g. a full sphere between poles) → full revolution
    } else {
        us.sort_by(f64::total_cmp);
        us.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        if us.len() == 1 {
            (us[0], TAU) // a single seam line → the full closed revolution (bore / boss / ring)
        } else {
            // Largest gap between consecutive u's (cyclic).
            let (mut gap, mut gap_start) = (0.0f64, 0.0f64);
            for i in 0..us.len() {
                let a = us[i];
                let b = if i + 1 < us.len() {
                    us[i + 1]
                } else {
                    us[0] + TAU
                };
                if b - a > gap {
                    gap = b - a;
                    gap_start = a;
                }
            }
            if gap < TAU * 0.05 {
                // Vertices all around the circle with no meaningful gap → a full revolution.
                (us[0], TAU)
            } else {
                // The swept arc is the complement of the largest gap.
                (gap_start + gap - TAU, TAU - gap)
            }
        }
    };
    let sweep = sweep.clamp(1e-6, TAU);

    // (c) The param-rect trim gate: every vertex must sit on the border of [u0,u0+sweep]×[v0,v1] — a vertex
    // interior on BOTH axes means the trim is not a rectangle (a hole in the wall / freeform trim).
    let v_tol = ((v1 - v0).abs() * 1e-3).max(1e-6);
    let u_tol = (sweep * 1e-3).max(1e-6);
    if sweep < TAU - 1e-9 {
        for p in projected.iter().filter(|p| p.u_valid) {
            // Normalize u into the [u0, u0+sweep] branch.
            let mut u = p.u;
            while u < u0 - u_tol {
                u += TAU;
            }
            let on_u_border = (u - u0).abs() <= u_tol || (u - (u0 + sweep)).abs() <= u_tol;
            let on_v_border = (p.v - v0).abs() <= v_tol || (p.v - v1).abs() <= v_tol;
            if !on_u_border && !on_v_border {
                return Err(beyond("the trim loop is not a u-v parameter rectangle"));
            }
        }
    }
    if (v1 - v0).abs() < 1e-9 {
        return Err(beyond("the boundary projects to a zero-height patch"));
    }

    // Adaptive segment counts from the ABSOLUTE deflection.
    let (ru, rv) = surface.curvatures();
    let nu = segments(sweep, ru, DEFLECTION);
    let nv = match rv {
        // The v direction is curved too (sphere latitude / torus tube) — its sweep is the v-range itself.
        Some(r) => segments((v1 - v0).abs(), r, DEFLECTION),
        None => 1, // straight generator (cylinder/cone) — one segment spans it exactly
    };
    Ok(AnalyticPlan {
        u0,
        sweep,
        v0,
        v1,
        nu,
        nv,
    })
}

/// Tessellate an analytic face bounded by `boundary` into a smooth adaptive grid (plan + grid).
///
/// # Errors
/// The face is beyond the declared subset — the note explains why (the caller surfaces it).
#[allow(clippy::cast_precision_loss)]
pub fn tessellate_analytic(
    face_id: u64,
    surface: &AnalyticSurface,
    boundary: &[[f64; 3]],
    same_sense: bool,
) -> Result<AnalyticPatch, UnsupportedNote> {
    let AnalyticPlan {
        u0,
        sweep,
        v0,
        v1,
        nu,
        nv,
    } = plan_analytic(face_id, surface, boundary)?;

    // The grid.
    let mut positions = Vec::with_capacity(((nu + 1) * (nv + 1)) as usize);
    for j in 0..=nv {
        let v = v0 + (v1 - v0) * f64::from(j) / f64::from(nv);
        for i in 0..=nu {
            let u = u0 + sweep * f64::from(i) / f64::from(nu);
            positions.push(surface.point(u, v));
        }
    }
    let mut triangles = Vec::with_capacity((nu * nv * 2) as usize);
    let idx = |i: u32, j: u32| j * (nu + 1) + i;
    let sense = if same_sense { 1.0 } else { -1.0 };
    for j in 0..nv {
        for i in 0..nu {
            let (a, b, c, d) = (idx(i, j), idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1));
            // Orient each triangle so its winding normal matches the exact analytic outward normal at the
            // quad centre (× same_sense) — no centroid heuristic for curved faces.
            let uc = u0 + sweep * (f64::from(i) + 0.5) / f64::from(nu);
            let vc = v0 + (v1 - v0) * (f64::from(j) + 0.5) / f64::from(nv);
            let n = scale3(surface.normal(uc, vc), sense);
            push_oriented(&positions, &mut triangles, [a, b, c], n);
            push_oriented(&positions, &mut triangles, [a, c, d], n);
        }
    }
    Ok(AnalyticPatch {
        positions,
        triangles,
    })
}

fn push_oriented(
    positions: &[[f64; 3]],
    triangles: &mut Vec<[u32; 3]>,
    tri: [u32; 3],
    outward: [f64; 3],
) {
    let p = |i: u32| positions[i as usize];
    let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
    let ab = sub3(b, a);
    let ac = sub3(c, a);
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    if dot3(n, n) == 0.0 {
        return; // degenerate sliver (e.g. a pole row)
    }
    if dot3(n, outward) >= 0.0 {
        triangles.push(tri);
    } else {
        triangles.push([tri[0], tri[2], tri[1]]);
    }
}

/// The worst angular deviation (radians) between each triangle's winding normal and the exact analytic
/// normal at its centroid — the SMOOTHNESS gate ("a faceted cylinder is a FAIL"): at deflection `d` and
/// radius `r` the max facet-to-surface angle is `acos(1 − d/r)` + the orientation slack; the test asserts
/// against the derived bound, never a magic constant.
#[must_use]
pub fn max_normal_deviation(patch: &AnalyticPatch, surface: &AnalyticSurface) -> f64 {
    let mut worst = 0.0f64;
    for t in &patch.triangles {
        let (a, b, c) = (
            patch.positions[t[0] as usize],
            patch.positions[t[1] as usize],
            patch.positions[t[2] as usize],
        );
        let centroid = scale3(add3(add3(a, b), c), 1.0 / 3.0);
        let pr = surface.project(centroid);
        let exact = surface.normal(pr.u, pr.v);
        let ab = sub3(b, a);
        let ac = sub3(c, a);
        let n = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let ln = norm3(n);
        let le = norm3(exact);
        if ln < 1e-12 || le < 1e-12 {
            continue;
        }
        // Winding may be flipped by same_sense — measure against the closer of ±exact (smoothness is
        // about facet-vs-surface angle, not orientation).
        let cosang = (dot3(n, exact) / (ln * le)).clamp(-1.0, 1.0).abs();
        worst = worst.max(cosang.acos());
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDENT: [f64; 16] = [
        1.0, 0.0, 0.0, 0.0, //
        0.0, 1.0, 0.0, 0.0, //
        0.0, 0.0, 1.0, 0.0, //
        0.0, 0.0, 0.0, 1.0,
    ];

    /// The derived smoothness bound: at deflection d and radius r one facet may tilt by at most the
    /// half-segment angle acos(1 − d/r); allow 2× for the corner-vs-centre measurement slack. Never a
    /// magic constant — the gate follows the tessellation's own parameters.
    fn smoothness_bound(r: f64) -> f64 {
        (2.0 * (1.0 - DEFLECTION / r).acos()).max(2.0 * MAX_SEG_ANGLE / 2.0)
    }

    #[test]
    fn a_full_cylinder_bore_tessellates_smooth_adaptive_and_on_surface() {
        let surf = AnalyticSurface::Cylinder {
            frame: IDENT,
            radius: 5.0,
        };
        // A bore's boundary: seam vertices at u=0 on two circles (z=0 and z=20) — the closed-revolution case.
        let boundary = [[5.0, 0.0, 0.0], [5.0, 0.0, 20.0]];
        let patch = tessellate_analytic(1, &surf, &boundary, true).expect("tessellates");
        assert!(
            patch.triangles.len() >= 2 * 16,
            "adaptive: a 5-unit-radius full revolution needs real segments, got {}",
            patch.triangles.len()
        );
        // Every grid vertex lies exactly on the surface.
        for p in &patch.positions {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            assert!((r - 5.0).abs() < 1e-9, "vertex off the cylinder: r={r}");
        }
        // The SMOOTHNESS gate ("a faceted cylinder is a FAIL"): facet normals within the derived bound.
        let dev = max_normal_deviation(&patch, &surf);
        assert!(
            dev <= smoothness_bound(5.0),
            "faceted: worst normal deviation {dev} rad exceeds the deflection-derived bound"
        );
    }

    #[test]
    fn a_quarter_fillet_respects_its_arc_no_overdraw() {
        let surf = AnalyticSurface::Cylinder {
            frame: IDENT,
            radius: 2.0,
        };
        // A quarter-arc fillet: corners at u=0 and u=π/2, two heights.
        let boundary = [
            [2.0, 0.0, 0.0],
            [0.0, 2.0, 0.0],
            [2.0, 0.0, 4.0],
            [0.0, 2.0, 4.0],
        ];
        let patch = tessellate_analytic(2, &surf, &boundary, true).expect("tessellates");
        // No overdraw: every vertex stays inside the quarter (x ≥ −eps AND y ≥ −eps).
        for p in &patch.positions {
            assert!(
                p[0] >= -1e-9 && p[1] >= -1e-9,
                "overdraw beyond the quarter arc at {p:?}"
            );
        }
        // And it is genuinely an arc, not one flat quad.
        assert!(patch.triangles.len() >= 2 * 3);
    }

    #[test]
    fn sphere_and_torus_and_cone_tessellate_on_surface_and_smooth() {
        let sphere = AnalyticSurface::Sphere {
            frame: IDENT,
            radius: 3.0,
        };
        // A band between two latitudes (seam vertices only → full revolution).
        let boundary = [
            [3.0, 0.0, 0.0],
            [2.121_320_343_559_642_5, 0.0, 2.121_320_343_559_642_5],
        ];
        let patch = tessellate_analytic(3, &sphere, &boundary, true).expect("sphere band");
        for p in &patch.positions {
            let r = norm3(*p);
            assert!((r - 3.0).abs() < 1e-9, "vertex off the sphere: r={r}");
        }
        assert!(max_normal_deviation(&patch, &sphere) <= smoothness_bound(3.0));

        let torus = AnalyticSurface::Torus {
            frame: IDENT,
            major: 5.0,
            minor: 1.0,
        };
        // Seam vertex on the outer equator + one on the top of the tube → a partial-in-v ring.
        let boundary = [[6.0, 0.0, 0.0], [5.0, 0.0, 1.0]];
        let patch = tessellate_analytic(4, &torus, &boundary, true).expect("torus ring");
        for p in &patch.positions {
            let rxy = (p[0] * p[0] + p[1] * p[1]).sqrt();
            let ring = rxy - 5.0;
            let d = (ring * ring + p[2] * p[2]).sqrt();
            assert!((d - 1.0).abs() < 1e-9, "vertex off the torus tube: d={d}");
        }

        let cone = AnalyticSurface::Cone {
            frame: IDENT,
            radius: 2.0,
            semi_angle: std::f64::consts::FRAC_PI_6, // 30°
        };
        let t30 = std::f64::consts::FRAC_PI_6.tan();
        let boundary = [[2.0, 0.0, 0.0], [2.0 + 5.0 * t30, 0.0, 5.0]];
        let patch = tessellate_analytic(5, &cone, &boundary, true).expect("cone frustum");
        for p in &patch.positions {
            let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
            let expect = 2.0 + p[2] * t30;
            assert!(
                (r - expect).abs() < 1e-9,
                "vertex off the cone: r={r} vs {expect}"
            );
        }
    }

    #[test]
    fn tessellation_is_deterministic_bit_identical() {
        let surf = AnalyticSurface::Cylinder {
            frame: IDENT,
            radius: 7.5,
        };
        let boundary = [[7.5, 0.0, 0.0], [7.5, 0.0, 12.0]];
        let a = tessellate_analytic(6, &surf, &boundary, true).expect("a");
        for _ in 0..3 {
            let b = tessellate_analytic(6, &surf, &boundary, true).expect("b");
            assert_eq!(a.positions, b.positions, "grid positions drifted");
            assert_eq!(a.triangles, b.triangles, "triangulation drifted");
        }
    }

    #[test]
    fn beyond_subset_is_an_explained_note_never_guessed_geometry() {
        let surf = AnalyticSurface::Cylinder {
            frame: IDENT,
            radius: 5.0,
        };
        // (a) A boundary vertex OFF the surface (a mis-referenced bound) → explained, not tessellated.
        let off = tessellate_analytic(7, &surf, &[[9.0, 0.0, 0.0], [5.0, 0.0, 4.0]], true);
        let note = off.expect_err("off-surface bound is beyond the subset");
        assert!(note.detail.contains("off the surface"), "{}", note.detail);
        // (b) A non-rectangular trim (a vertex interior on both axes) → explained.
        let complex = tessellate_analytic(
            8,
            &surf,
            &[
                [5.0, 0.0, 0.0], // u=0, v=0 (corner)
                [0.0, 5.0, 0.0], // u=π/2, v=0 (corner)
                [5.0, 0.0, 8.0], // u=0, v=8 (corner)
                [0.0, 5.0, 8.0], // u=π/2, v=8 (corner)
                [
                    5.0 * std::f64::consts::FRAC_1_SQRT_2,
                    5.0 * std::f64::consts::FRAC_1_SQRT_2,
                    4.0,
                ], // u=π/4, v=4 — INTERIOR
            ],
            true,
        );
        let note = complex.expect_err("a non-rectangular trim is beyond the subset");
        assert!(
            note.detail.contains("not a u-v parameter rectangle"),
            "{}",
            note.detail
        );
        // (c) A zero-height patch → explained.
        let flat = tessellate_analytic(9, &surf, &[[5.0, 0.0, 3.0], [-5.0, 0.0, 3.0]], true);
        assert!(flat.is_err());
    }

    #[test]
    fn same_sense_flips_the_winding_orientation() {
        let surf = AnalyticSurface::Cylinder {
            frame: IDENT,
            radius: 5.0,
        };
        let boundary = [[5.0, 0.0, 0.0], [5.0, 0.0, 10.0]];
        let outward = tessellate_analytic(10, &surf, &boundary, true).expect("outward");
        let inward = tessellate_analytic(10, &surf, &boundary, false).expect("inward");
        // Same grid, opposite winding on every triangle (a boss's wall vs a bore's wall).
        assert_eq!(outward.positions, inward.positions);
        assert_eq!(outward.triangles.len(), inward.triangles.len());
        let flipped = inward
            .triangles
            .iter()
            .zip(&outward.triangles)
            .all(|(i, o)| *i == [o[0], o[2], o[1]] || *i == *o);
        assert!(flipped, "inward winding should mirror outward");
        assert_ne!(outward.triangles, inward.triangles);
    }
}
