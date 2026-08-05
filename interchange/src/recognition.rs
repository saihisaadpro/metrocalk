//! # M15.11 (ADR-081) — the AI semantic pass: B-rep feature recognition on ALREADY-DECODED geometry
//!
//! After translation, understand the CAD: recognize features (holes · bosses · fillets · chamfers ·
//! pockets), classify parts (fastener · bracket · plate · …), group repeated instances, and recover
//! analytic primitives from tessellation-only meshes. The deployable baseline is **AAG (attributed
//! adjacency graph) symbolic recognition** — deterministic, explainable, no training data — per the
//! research verdict that no shipping CAD tool has replaced 1988-era AAG with ML (learned recognizers are
//! synthetic/single-vendor-trained and degrade on real feature-history-stripped STEP; BrepMFR confirms).
//! A learned recognizer is the **confidence-gated seam** behind the same trait — never trusted blindly.
//!
//! **AI never decodes** (the M15.11 boundary): every input type here is already-decoded, engine-native
//! geometry (`CadFace` / `TriMesh`). This module has no access to file bytes, readers, kernels, or the
//! Part-21 parser — enforced by a CI grep-gate.
//!
//! Every recognition is **confidence-scored + explainable** (`why` is a human-readable justification a
//! kernel check can re-derive) and **deterministic**: iteration is index/BTreeMap-ordered and every
//! compared scalar is quantized to an integer grid before a decision (the ADR-020 discipline).

use std::collections::BTreeMap;

use metrocalk_csg::TriMesh;

use crate::analytic::AnalyticSurface;
use crate::step::{CadFace, FaceKind};

// ── Decision quantization (ADR-020: transcendentals are native-per-ISA at the ULP; decisions must not be) ──
const QUANTUM: f64 = 1e-6;
fn q(x: f64) -> i64 {
    #[allow(clippy::cast_possible_truncation)]
    let v = (x / QUANTUM).round() as i64;
    v
}

// ── The recognized-feature vocabulary (closed, honest: only what the AAG actually detects) ────────────────
/// A recognized form feature. CLOSED enum — threads, slots, and sheet-metal bends are named futures (real
/// STEP rarely models thread geometry; it rides PMI callouts), never silently mislabeled into these.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FeatureKind {
    /// A bore that exits the part on both ends (concave cylinder, both rims adjacent to other faces).
    ThroughHole,
    /// A bore closed at one end (concave cylinder with a floor/cap adjacency at one rim).
    BlindHole,
    /// A protruding cylinder (convex, near-full sweep) on a base.
    Boss,
    /// A concave blend (cylinder/torus whose shared edges are tangent-smooth) — an inside-corner radius.
    Fillet,
    /// A convex blend (outside-corner radius).
    Round,
    /// A conical transition band (countersink / edge break) between a bore and a face.
    Chamfer,
    /// A planar depression (planar faces linked by concave edges around a floor). Weak-heuristic class —
    /// always below the auto-commit gate (surfaced, not auto-committed).
    Pocket,
}

impl FeatureKind {
    /// The stable token tests + the ECS component key on (never UI copy).
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::ThroughHole => "through-hole",
            Self::BlindHole => "blind-hole",
            Self::Boss => "boss",
            Self::Fillet => "fillet",
            Self::Round => "round",
            Self::Chamfer => "chamfer",
            Self::Pocket => "pocket",
        }
    }
}

/// One recognized feature — confidence-scored, explainable, kernel-verifiable (the `faces` + params let
/// [`verify_feature`] re-derive the geometric claim from the geometry itself).
#[derive(Clone, PartialEq, Debug)]
pub struct RecognizedFeature {
    pub kind: FeatureKind,
    /// The STEP face `#id`s this feature is made of (the typed-relationship targets).
    pub faces: Vec<u64>,
    /// The feature axis (unit, canonical sign: the lexicographically-larger direction), where meaningful.
    pub axis: Option<[f64; 3]>,
    /// A point on the axis / the feature origin (scene units — mm for STEP).
    pub origin: Option<[f64; 3]>,
    /// Radius in scene units (bore/boss/fillet radius; chamfer start radius).
    pub radius: Option<f64>,
    /// Extent along the axis (bore depth / boss height / chamfer slant), where derivable.
    pub depth: Option<f64>,
    /// Recognition confidence in `[0, 1]` — quantized; below the caller's gate a feature is SURFACED for
    /// human adjudication, never auto-committed.
    pub confidence: f64,
    /// The human-readable justification (the neuro-symbolic "why") — face ids + the geometric evidence.
    pub why: String,
}

/// The recognizer seam. `AagRecognizer` is the shipped, deterministic, symbolic baseline; a learned
/// (BRepFormer/BRepNet-class) recognizer implements the same trait behind the confidence gate — its
/// proposals go through the SAME [`verify_feature`] kernel check and the same surfaced-not-auto-committed
/// band (validated-AI: neural proposes, the kernel verifies).
pub trait FeatureRecognizer {
    /// The provenance token stamped on every landed feature (`"aag"` / `"learned"` / `"recovered"`).
    fn source(&self) -> &'static str;
    /// Whether same input ⇒ same output, bit-for-bit (the AAG is; a learned model must declare honestly).
    fn deterministic(&self) -> bool;
    /// Recognize features on one part's decoded B-rep faces.
    fn recognize(&self, faces: &[CadFace]) -> Vec<RecognizedFeature>;
}

/// The 1988-class attributed-adjacency-graph recognizer — the deployable baseline. Symbolic, deterministic,
/// zero-dep, explainable: every rule below is a geometric predicate over already-parsed analytic surfaces
/// (M15.8) + face adjacency (shared `EDGE_CURVE` ids), never a learned weight.
pub struct AagRecognizer;

impl FeatureRecognizer for AagRecognizer {
    fn source(&self) -> &'static str {
        "aag"
    }
    fn deterministic(&self) -> bool {
        true
    }
    fn recognize(&self, faces: &[CadFace]) -> Vec<RecognizedFeature> {
        recognize_aag(faces)
    }
}

/// The learned-recognizer seam (BRepFormer/BRepNet-class, ONNX/on-device). **No model ships in M15.11** —
/// every published high-accuracy recognizer is synthetic/single-vendor-trained and measured to degrade on
/// real STEP (the honest gap), so shipping weights would fabricate an accuracy claim. The seam exists so
/// the gate machinery (schema check + kernel verify + confidence band) is real and tested (with a mock in
/// tests); wiring a real model is the named upgrade.
pub struct LearnedRecognizerSeam;

impl FeatureRecognizer for LearnedRecognizerSeam {
    fn source(&self) -> &'static str {
        "learned"
    }
    fn deterministic(&self) -> bool {
        false
    }
    fn recognize(&self, _faces: &[CadFace]) -> Vec<RecognizedFeature> {
        Vec::new() // Unavailable — never a fabricated recognition (the FmiSolver discipline, ADR-075).
    }
}

// ── Small vector helpers (self-contained; the analytic frame helpers stay private to analytic.rs) ─────────
fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}
fn unit3(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = norm3(a);
    (n > 1e-12).then(|| scale3(a, 1.0 / n))
}
/// Canonical sign for an undirected axis (a cylinder's ±z are the same axis): the lexicographically
/// larger of `d`/`-d` on the quantized grid — deterministic, orientation-free grouping key.
fn canonical_dir(d: [f64; 3]) -> [f64; 3] {
    let k = [q(d[0]), q(d[1]), q(d[2])];
    let nk = [q(-d[0]), q(-d[1]), q(-d[2])];
    if k >= nk {
        d
    } else {
        scale3(d, -1.0)
    }
}

fn frame_origin(m: &[f64; 16]) -> [f64; 3] {
    [m[12], m[13], m[14]]
}
fn frame_z(m: &[f64; 16]) -> [f64; 3] {
    [m[8], m[9], m[10]]
}

/// Newell's method over an ordered polygon — the plane normal (un-oriented) + centroid. Deterministic.
fn newell(poly: &[[f64; 3]]) -> Option<([f64; 3], [f64; 3])> {
    if poly.len() < 3 {
        return None;
    }
    let mut n = [0.0f64; 3];
    let mut c = [0.0f64; 3];
    for i in 0..poly.len() {
        let a = poly[i];
        let b = poly[(i + 1) % poly.len()];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
        c = add3(c, a);
    }
    #[allow(clippy::cast_precision_loss)]
    let c = scale3(c, 1.0 / poly.len() as f64);
    unit3(n).map(|n| (n, c))
}

/// A face's OUTWARD normal near a point `p` on it, and a representative interior point. For an analytic
/// face this is the exact surface normal × `same_sense` (the same orientation M15.8's tessellation renders
/// smooth with — verified on real files). For a planar face it is the Newell normal of the outer loop
/// oriented by the bound sense × `same_sense` (the STEP loop convention).
fn face_normal_at(f: &CadFace, p: [f64; 3]) -> Option<[f64; 3]> {
    let sense = if f.same_sense { 1.0 } else { -1.0 };
    if let Some(s) = &f.recognized {
        let pr = s.project(p);
        if !pr.u_valid {
            return None;
        }
        return unit3(scale3(s.normal(pr.u, pr.v), sense));
    }
    if f.kind == FaceKind::Planar {
        let (n, _c) = newell(&f.outer)?;
        let bound = if f.outer_sense { 1.0 } else { -1.0 };
        return unit3(scale3(n, sense * bound));
    }
    None
}

/// The polygon centroid of a face's outer boundary — the "interior probe" point for edge convexity.
fn face_centroid(f: &CadFace) -> Option<[f64; 3]> {
    newell(&f.outer).map(|(_n, c)| c)
}

// ── Edge attribution (the "attributed" in AAG) ─────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EdgeKind {
    Convex,
    Concave,
    /// Tangent-continuous (the blend signature — a fillet meets its walls smoothly).
    Smooth,
}

/// Below this normalized deviation an edge is tangent-smooth. Loose enough to absorb boundary-polygon
/// sampling error, tight enough that a 90° corner (deviation ≈ 0.7) never reads smooth.
const SMOOTH_TOL: f64 = 0.12;

/// Classify the shared edge between faces `a` and `b`: probe each face's outward normal at the edge
/// midpoint and test which side of the other's tangent plane its interior lies on. Convex ⇔ each face's
/// interior is BELOW the other's outward tangent plane (a cube edge); concave ⇔ above (an inside corner);
/// smooth ⇔ the signed offsets vanish (a tangent blend).
fn classify_edge(a: &CadFace, b: &CadFace, mid: [f64; 3]) -> Option<EdgeKind> {
    let na = face_normal_at(a, mid)?;
    let nb = face_normal_at(b, mid)?;
    let ca = face_centroid(a)?;
    let cb = face_centroid(b)?;
    // Signed, distance-normalized height of each face's interior probe above the OTHER face's tangent plane.
    let ha = {
        let w = sub3(ca, mid);
        let d = norm3(w);
        if d < 1e-9 {
            0.0
        } else {
            dot3(nb, w) / d
        }
    };
    let hb = {
        let w = sub3(cb, mid);
        let d = norm3(w);
        if d < 1e-9 {
            0.0
        } else {
            dot3(na, w) / d
        }
    };
    let s = f64::midpoint(ha, hb);
    if q(s.abs()) <= q(SMOOTH_TOL) {
        // Tangency corroboration: smooth blends also have near-parallel normals at the edge.
        if q(dot3(na, nb)) >= q(1.0 - 2.0 * SMOOTH_TOL) {
            return Some(EdgeKind::Smooth);
        }
    }
    Some(if s > 0.0 {
        EdgeKind::Concave
    } else {
        EdgeKind::Convex
    })
}

/// The adjacency map: `EDGE_CURVE #id → the face indices bounding it`. Faceted (`POLY_LOOP`) faces key
/// edges on the loop id (unique per face) so they contribute no cross-face adjacency — degraded, honest.
fn adjacency(faces: &[CadFace]) -> BTreeMap<u64, Vec<usize>> {
    let mut adj: BTreeMap<u64, Vec<usize>> = BTreeMap::new();
    for (i, f) in faces.iter().enumerate() {
        for e in &f.edges {
            let v = adj.entry(e.id).or_default();
            if !v.contains(&i) {
                v.push(i);
            }
        }
    }
    adj
}

/// Whether the cylinder face's outward material side faces the axis (a bore) or away (a shaft/boss wall):
/// probe the outward normal at a boundary point and dot it with the radial direction. `Some(true)` = bore.
#[allow(clippy::many_single_char_names)] // p/n/o/z/d/r are the canonical geometry names here
fn cylinder_is_bore(f: &CadFace, s: &AnalyticSurface) -> Option<bool> {
    let p = *f.outer.first()?;
    let n = face_normal_at(f, p)?;
    let (o, z) = match s {
        AnalyticSurface::Cylinder { frame, .. } => (frame_origin(frame), frame_z(frame)),
        _ => return None,
    };
    // Radial direction at p: the component of (p − axis_origin) perpendicular to the axis.
    let d = sub3(p, o);
    let radial = sub3(d, scale3(z, dot3(d, z)));
    let r = unit3(radial)?;
    let inward = dot3(n, r) < 0.0;
    Some(inward)
}

/// The angular sweep (radians) the face's outer boundary covers around the cylinder axis, and the v-extent
/// along the axis. A full bore sweeps ≈ 2π; a fillet strip sweeps ≈ π/2. A REAL parsed STEP bore's boundary
/// polygon is sparse (a closed circle `EDGE_CURVE` contributes one traversal vertex), so a **closed rim
/// edge** (`ends[0] == ends[1]` — the circle's v1 == v2) is itself full-circle evidence.
fn cylinder_sweep_extent(f: &CadFace, s: &AnalyticSurface) -> (f64, f64, f64) {
    let mut us: Vec<f64> = Vec::new();
    let (mut v_min, mut v_max) = (f64::INFINITY, f64::NEG_INFINITY);
    for p in &f.outer {
        let pr = s.project(*p);
        if pr.u_valid {
            us.push(pr.u);
        }
        v_min = v_min.min(pr.v);
        v_max = v_max.max(pr.v);
    }
    // A closed rim edge is a full circle regardless of how sparsely the polygon sampled it; its endpoint
    // also carries the rim's v (the boundary polygon may have missed one end entirely).
    let mut closed_rim = false;
    for e in &f.edges {
        if e.ends[0].map(q) == e.ends[1].map(q) {
            closed_rim = true;
            let pr = s.project(e.ends[0]);
            v_min = v_min.min(pr.v);
            v_max = v_max.max(pr.v);
        }
    }
    if !v_min.is_finite() {
        return (0.0, 0.0, 0.0);
    }
    if closed_rim {
        return (std::f64::consts::TAU, v_min, v_max);
    }
    if us.is_empty() {
        return (0.0, 0.0, 0.0);
    }
    // Angular coverage on a circle: sort, take TAU minus the largest gap (handles wrap-around).
    us.sort_by_key(|u| q(*u));
    let mut largest_gap = std::f64::consts::TAU + us[0] - us[us.len() - 1];
    for w in us.windows(2) {
        largest_gap = largest_gap.max(w[1] - w[0]);
    }
    let sweep = (std::f64::consts::TAU - largest_gap).max(0.0);
    (sweep, v_min, v_max)
}

/// One coaxial group of cylinder faces (a bore split into half-cylinders is ONE hole).
struct CoaxialGroup {
    face_idx: Vec<usize>,
    axis: [f64; 3],
    /// The axis foot: the point on the axis nearest the origin (a translation-stable axis anchor).
    foot: [f64; 3],
    radius: f64,
    bore: bool,
    sweep: f64,
    v_min: f64,
    v_max: f64,
}

/// Group cylinder faces by (canonical axis, axis foot, radius, bore-ness) on the quantized grid.
fn coaxial_groups(faces: &[CadFace]) -> Vec<CoaxialGroup> {
    let mut groups: BTreeMap<([i64; 3], [i64; 3], i64, bool), CoaxialGroup> = BTreeMap::new();
    for (i, f) in faces.iter().enumerate() {
        let Some(s @ AnalyticSurface::Cylinder { frame, radius }) = &f.recognized else {
            continue;
        };
        let Some(bore) = cylinder_is_bore(f, s) else {
            continue;
        };
        let Some(z) = unit3(frame_z(frame)) else {
            continue;
        };
        let axis = canonical_dir(z);
        let o = frame_origin(frame);
        let foot = sub3(o, scale3(axis, dot3(o, axis)));
        let (sweep, v_min, v_max) = cylinder_sweep_extent(f, s);
        if sweep <= 0.0 {
            continue;
        }
        let key = (
            [q(axis[0]), q(axis[1]), q(axis[2])],
            // The foot is quantized at boundary-sampling tolerance (1e-3 scene units), not the decision
            // quantum — two half-cylinders of one bore agree to sampling noise, not to 1e-6.
            [
                q(foot[0] * QUANTUM / 1e-3),
                q(foot[1] * QUANTUM / 1e-3),
                q(foot[2] * QUANTUM / 1e-3),
            ],
            q(*radius),
            bore,
        );
        let g = groups.entry(key).or_insert(CoaxialGroup {
            face_idx: Vec::new(),
            axis,
            foot,
            radius: *radius,
            bore,
            sweep: 0.0,
            v_min: f64::INFINITY,
            v_max: f64::NEG_INFINITY,
        });
        g.face_idx.push(i);
        g.sweep += sweep;
        g.v_min = g.v_min.min(v_min);
        g.v_max = g.v_max.max(v_max);
    }
    groups.into_values().collect()
}

/// Whether every shared (cross-face) edge of face `i` is tangent-smooth — the classic blend signature.
/// Returns `None` when the face has NO cross-face adjacency at all (nothing to attribute).
fn all_shared_edges_smooth(
    faces: &[CadFace],
    adj: &BTreeMap<u64, Vec<usize>>,
    i: usize,
) -> Option<bool> {
    let mut saw_shared = false;
    for e in &faces[i].edges {
        let Some(others) = adj.get(&e.id) else {
            continue;
        };
        for &j in others {
            if j == i {
                continue;
            }
            saw_shared = true;
            let mid = scale3(add3(e.ends[0], e.ends[1]), 0.5);
            match classify_edge(&faces[i], &faces[j], mid) {
                Some(EdgeKind::Smooth) => {}
                _ => return Some(false),
            }
        }
    }
    saw_shared.then_some(true)
}

/// The part's characteristic extent (bounding-box diagonal over all face boundary points) — the relative
/// scale features are judged against ("small" is relative to the part, never an absolute).
fn part_extent(faces: &[CadFace]) -> f64 {
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for f in faces {
        for p in f.outer.iter().chain(f.inner.iter().flatten()) {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
    }
    if lo[0] > hi[0] {
        return 0.0;
    }
    norm3(sub3(hi, lo))
}

/// The AAG pass over one part's faces. Deterministic: faces are visited in index order, groups in BTreeMap
/// key order, and every threshold compares quantized values.
#[allow(clippy::too_many_lines, clippy::many_single_char_names)] // one linear pass per rule family
fn recognize_aag(faces: &[CadFace]) -> Vec<RecognizedFeature> {
    let mut out: Vec<RecognizedFeature> = Vec::new();
    if faces.is_empty() {
        return out;
    }
    let adj = adjacency(faces);
    let extent = part_extent(faces);
    let by_id: BTreeMap<u64, usize> = faces.iter().enumerate().map(|(i, f)| (f.id, i)).collect();

    // ── Holes + bosses: coaxial cylinder groups ───────────────────────────────────────────────────────────
    // A blend (fillet/round) is also a cylinder — exclude cylinder faces whose shared edges are ALL smooth
    // from the hole/boss rules (they take the blend rules below).
    let mut is_blend = vec![false; faces.len()];
    for (i, f) in faces.iter().enumerate() {
        if matches!(
            f.recognized,
            Some(AnalyticSurface::Cylinder { .. } | AnalyticSurface::Torus { .. })
        ) && all_shared_edges_smooth(faces, &adj, i) == Some(true)
        {
            is_blend[i] = true;
        }
    }

    for g in coaxial_groups(faces) {
        if g.face_idx.iter().all(|&i| is_blend[i]) {
            continue; // a smooth-edged cylinder is a blend, not a bore/boss
        }
        let ids: Vec<u64> = g.face_idx.iter().map(|&i| faces[i].id).collect();
        let depth = g.v_max - g.v_min;
        let full = q(g.sweep) >= q(0.9 * std::f64::consts::TAU);
        let id_list = ids
            .iter()
            .map(|i| format!("#{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        if g.bore {
            // Through vs blind: probe which axial ends have adjacent faces via the group's rim edges.
            let (mut rim_lo, mut rim_hi) = (false, false);
            for &i in &g.face_idx {
                for e in &faces[i].edges {
                    let Some(others) = adj.get(&e.id) else {
                        continue;
                    };
                    if others.iter().all(|&j| j == i || g.face_idx.contains(&j)) {
                        continue; // an internal seam of the split bore, not a rim
                    }
                    let mid = scale3(add3(e.ends[0], e.ends[1]), 0.5);
                    let v = dot3(sub3(mid, g.foot), g.axis);
                    let t = (v - g.v_min) / (g.v_max - g.v_min).max(1e-12);
                    if q(t) <= q(0.25) {
                        rim_lo = true;
                    }
                    if q(t) >= q(0.75) {
                        rim_hi = true;
                    }
                }
            }
            let (kind, mut confidence, through_why) = if rim_lo && rim_hi {
                (
                    FeatureKind::ThroughHole,
                    0.95,
                    "both rims adjacent to other faces (exits both ends)",
                )
            } else {
                (
                    FeatureKind::BlindHole,
                    0.85,
                    "one open rim (closed/floored at the other end)",
                )
            };
            if !full {
                confidence = 0.6; // a partial concave arc might be a slot wall / blend — surfaced, not assumed
            }
            out.push(RecognizedFeature {
                kind,
                faces: ids,
                axis: Some(g.axis),
                origin: Some(g.foot),
                radius: Some(g.radius),
                depth: Some(depth),
                confidence: quantized(confidence),
                why: format!(
                    "cylindrical face(s) {id_list}: outward normal points TOWARD the axis (material \
                     outside — a bore), ∅{:.3} × {depth:.3} deep, angular coverage {:.0}°; {through_why}",
                    2.0 * g.radius,
                    g.sweep.to_degrees(),
                ),
            });
        } else if full {
            out.push(RecognizedFeature {
                kind: FeatureKind::Boss,
                faces: ids,
                axis: Some(g.axis),
                origin: Some(g.foot),
                radius: Some(g.radius),
                depth: Some(depth),
                confidence: quantized(0.9),
                why: format!(
                    "cylindrical face(s) {id_list}: outward normal points AWAY from the axis (material \
                     inside — a protrusion wall), ∅{:.3} × {depth:.3} tall, full sweep",
                    2.0 * g.radius
                ),
            });
        }
        // A partial convex arc alone is not evidence of a boss — skipped (never a guessed label).
    }

    // ── Fillets / rounds: smooth-edged blend faces ────────────────────────────────────────────────────────
    for (i, f) in faces.iter().enumerate() {
        if !is_blend[i] {
            continue;
        }
        let (radius, concave, surf) = match &f.recognized {
            Some(s @ AnalyticSurface::Cylinder { radius, .. }) => {
                let Some(bore) = cylinder_is_bore(f, s) else {
                    continue;
                };
                (*radius, bore, "cylindrical")
            }
            Some(AnalyticSurface::Torus { minor, frame, .. }) => {
                // Torus blend: concave (fillet) iff the outward normal at a boundary point faces the tube
                // axis circle — probe like the cylinder but against the tube centre.
                let Some(p) = f.outer.first().copied() else {
                    continue;
                };
                let Some(n) = face_normal_at(f, p) else {
                    continue;
                };
                let o = frame_origin(frame);
                let d = sub3(p, o);
                let z = frame_z(frame);
                let radial = sub3(d, scale3(z, dot3(d, z)));
                let Some(r) = unit3(radial) else { continue };
                (*minor, dot3(n, r) < 0.0, "toroidal")
            }
            _ => continue,
        };
        let small = extent <= 0.0 || q(radius) <= q(0.25 * extent);
        let (kind, verb) = if concave {
            (FeatureKind::Fillet, "an inside-corner blend")
        } else {
            (FeatureKind::Round, "an outside-corner blend")
        };
        out.push(RecognizedFeature {
            kind,
            faces: vec![f.id],
            axis: None,
            origin: face_centroid(f),
            radius: Some(radius),
            depth: None,
            confidence: quantized(if small { 0.85 } else { 0.6 }),
            why: format!(
                "{surf} face #{}: every shared edge is tangent-smooth (a blend), r={radius:.3} — {verb}{}",
                f.id,
                if small {
                    String::new()
                } else {
                    format!(
                        "; radius is large relative to the part ({:.0}% of extent) — surfaced for review",
                        100.0 * radius / extent.max(1e-12)
                    )
                }
            ),
        });
    }

    // ── Chamfers: conical transition bands (countersinks / edge breaks) ───────────────────────────────────
    for f in faces {
        let Some(AnalyticSurface::Cone {
            radius, semi_angle, ..
        }) = &f.recognized
        else {
            continue;
        };
        // A chamfer band is axially SHORT relative to the part; a structural cone (a funnel) is not.
        let (mut v_min, mut v_max) = (f64::INFINITY, f64::NEG_INFINITY);
        if let Some(s) = &f.recognized {
            for p in &f.outer {
                let pr = s.project(*p);
                v_min = v_min.min(pr.v);
                v_max = v_max.max(pr.v);
            }
        }
        if !v_min.is_finite() {
            continue;
        }
        let band = v_max - v_min;
        let small = extent > 0.0 && q(band) <= q(0.15 * extent);
        if !small {
            continue; // a big cone is geometry, not an edge break — never guessed into a chamfer
        }
        out.push(RecognizedFeature {
            kind: FeatureKind::Chamfer,
            faces: vec![f.id],
            axis: None,
            origin: face_centroid(f),
            radius: Some(*radius),
            depth: Some(band),
            confidence: quantized(0.8),
            why: format!(
                "conical face #{}: a short transition band ({band:.3} along the axis, {:.1}° half-angle) — \
                 an edge break / countersink",
                f.id,
                semi_angle.to_degrees()
            ),
        });
    }

    // ── Pockets: planar clusters linked by concave edges (weak heuristic — always surfaced) ───────────────
    let mut visited = vec![false; faces.len()];
    for start in 0..faces.len() {
        if visited[start] || faces[start].kind != FaceKind::Planar {
            continue;
        }
        // Flood over planar faces connected through CONCAVE shared edges.
        let mut cluster = vec![start];
        visited[start] = true;
        let mut head = 0;
        while head < cluster.len() {
            let i = cluster[head];
            head += 1;
            for e in &faces[i].edges {
                let Some(others) = adj.get(&e.id) else {
                    continue;
                };
                for &j in others {
                    if j == i || visited[j] || faces[j].kind != FaceKind::Planar {
                        continue;
                    }
                    let mid = scale3(add3(e.ends[0], e.ends[1]), 0.5);
                    if classify_edge(&faces[i], &faces[j], mid) == Some(EdgeKind::Concave) {
                        visited[j] = true;
                        cluster.push(j);
                    }
                }
            }
        }
        if cluster.len() >= 3 {
            let ids: Vec<u64> = cluster.iter().map(|&i| faces[i].id).collect();
            let id_list = ids
                .iter()
                .map(|i| format!("#{i}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push(RecognizedFeature {
                kind: FeatureKind::Pocket,
                faces: ids,
                axis: None,
                origin: face_centroid(&faces[cluster[0]]),
                radius: None,
                depth: None,
                confidence: quantized(0.6), // deliberately below the gate — a pocket claim is adjudicated
                why: format!(
                    "{} planar faces {id_list} linked by concave edges — a depression (pocket candidate; \
                     weak heuristic, surfaced for confirmation)",
                    cluster.len()
                ),
            });
        }
    }

    // Deterministic output order: by first face id, then kind token.
    out.sort_by(|a, b| {
        let ka = (a.faces.first().copied().unwrap_or(0), a.kind.token());
        let kb = (b.faces.first().copied().unwrap_or(0), b.kind.token());
        ka.cmp(&kb)
    });
    let _ = by_id; // (kept for future rules that look faces up by STEP id)
    out
}

fn quantized(c: f64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let v = q(c) as f64 * QUANTUM;
    v
}

// ── The kernel check (validated-AI: neural/symbolic proposes → the kernel VERIFIES from geometry) ─────────
/// Re-derive a proposed feature's geometric claim from the faces themselves. A proposal whose named faces
/// don't exist, aren't the claimed surface type, or contradict the claimed orientation/radius is REJECTED
/// with the reason — this is what stands between a wrong (learned or corrupted) label and the scene.
///
/// # Errors
/// A human-readable reason the claim does not hold on the geometry.
pub fn verify_feature(feature: &RecognizedFeature, faces: &[CadFace]) -> Result<(), String> {
    if feature.faces.is_empty() {
        return Err("the feature names no faces".into());
    }
    let by_id: BTreeMap<u64, &CadFace> = faces.iter().map(|f| (f.id, f)).collect();
    let mut resolved: Vec<&CadFace> = Vec::with_capacity(feature.faces.len());
    for id in &feature.faces {
        match by_id.get(id) {
            Some(f) => resolved.push(f),
            None => return Err(format!("face #{id} does not exist on this part")),
        }
    }
    match feature.kind {
        FeatureKind::ThroughHole | FeatureKind::BlindHole => {
            for f in &resolved {
                let Some(s @ AnalyticSurface::Cylinder { radius, .. }) = &f.recognized else {
                    return Err(format!(
                        "face #{} is not cylindrical — a bore must be",
                        f.id
                    ));
                };
                if let Some(r) = feature.radius {
                    if q(*radius) != q(r) {
                        return Err(format!(
                            "claimed radius {r:.6} ≠ face #{} cylinder radius {radius:.6}",
                            f.id
                        ));
                    }
                }
                if cylinder_is_bore(f, s) != Some(true) {
                    return Err(format!(
                        "face #{}'s outward normal points AWAY from the axis — a wall, not a bore",
                        f.id
                    ));
                }
            }
            Ok(())
        }
        FeatureKind::Boss => {
            for f in &resolved {
                let Some(s @ AnalyticSurface::Cylinder { .. }) = &f.recognized else {
                    return Err(format!(
                        "face #{} is not cylindrical — a boss wall must be",
                        f.id
                    ));
                };
                if cylinder_is_bore(f, s) != Some(false) {
                    return Err(format!(
                        "face #{}'s outward normal points TOWARD the axis — a bore, not a boss",
                        f.id
                    ));
                }
            }
            Ok(())
        }
        FeatureKind::Fillet | FeatureKind::Round => {
            let adj = adjacency(faces);
            for f in &resolved {
                if !matches!(
                    f.recognized,
                    Some(AnalyticSurface::Cylinder { .. } | AnalyticSurface::Torus { .. })
                ) {
                    return Err(format!(
                        "face #{} is not a cylinder/torus — a blend must be",
                        f.id
                    ));
                }
                let i = faces.iter().position(|g| g.id == f.id).unwrap_or(0);
                if all_shared_edges_smooth(faces, &adj, i) != Some(true) {
                    return Err(format!(
                        "face #{} has a non-tangent shared edge — not a blend",
                        f.id
                    ));
                }
            }
            Ok(())
        }
        FeatureKind::Chamfer => {
            for f in &resolved {
                if !matches!(f.recognized, Some(AnalyticSurface::Cone { .. })) {
                    return Err(format!(
                        "face #{} is not conical — this chamfer rule is",
                        f.id
                    ));
                }
            }
            Ok(())
        }
        FeatureKind::Pocket => {
            for f in &resolved {
                if f.kind != FaceKind::Planar {
                    return Err(format!(
                        "face #{} is not planar — this pocket rule is",
                        f.id
                    ));
                }
            }
            Ok(())
        }
    }
}

// ── Part classification (D3) ───────────────────────────────────────────────────────────────────────────────
/// A part-level class. CLOSED vocabulary with an honest `Unknown` default — a part is never guessed into a
/// class without stated evidence.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PartClass {
    Fastener,
    Washer,
    Pin,
    Shaft,
    Plate,
    Bracket,
    Housing,
    Unknown,
}

impl PartClass {
    /// The stable token ("fastener" / "plate" / …) tests + the ECS field key on.
    #[must_use]
    pub fn token(self) -> &'static str {
        match self {
            Self::Fastener => "fastener",
            Self::Washer => "washer",
            Self::Pin => "pin",
            Self::Shaft => "shaft",
            Self::Plate => "plate",
            Self::Bracket => "bracket",
            Self::Housing => "housing",
            Self::Unknown => "unknown",
        }
    }
}

/// A confidence-scored, explained part classification.
#[derive(Clone, PartialEq, Debug)]
pub struct PartClassification {
    pub class: PartClass,
    pub confidence: f64,
    pub why: String,
}

/// Aspect shape from the sorted covariance eigenvalues (ASCENDING — the `sym3_eigenvalues_sorted`
/// convention): rod (one dominant spread axis), plate (two dominant, one thin), block (isotropic-ish).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Aspect {
    Rod,
    Plate,
    Block,
}

fn aspect_of(moments: [f64; 3]) -> Aspect {
    let (small, mid, large) = (
        moments[0].max(1e-18),
        moments[1].max(1e-18),
        moments[2].max(1e-18),
    );
    if q(large / mid) >= q(9.0) {
        Aspect::Rod // one long axis (√9 = 3× the std-dev)
    } else if q(mid / small) >= q(9.0) {
        Aspect::Plate // two spread axes, one thin
    } else {
        Aspect::Block
    }
}

/// Classify one part from its decoded geometry (+ its recognized features and, when B-rep is absent, the
/// mesh alone). `name` is an HONEST SECONDARY signal — a name hint only ever raises confidence and is
/// always stated in `why` (never the primary evidence).
#[must_use]
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
pub fn classify_part(
    faces: Option<&[CadFace]>,
    mesh: &TriMesh,
    features: &[RecognizedFeature],
    name: &str,
) -> PartClassification {
    if mesh.triangles.is_empty() {
        return PartClassification {
            class: PartClass::Unknown,
            confidence: 0.0,
            why: "no geometry to classify".into(),
        };
    }
    let (_vol, _c, cov) = crate::reimport_identity::volume_centroid_covariance(mesh);
    let moments = crate::reimport_identity::sym3_eigenvalues_sorted(cov);
    let aspect = aspect_of(moments);

    // Surface composition (B-rep only): fraction of faces that are cylinders/cones, coaxial dominance.
    let (n_faces, cyl_frac, planar_frac, coax_frac) =
        faces.map_or((0, 0.0, 0.0, 0.0), |fs| {
            let n = fs.len().max(1);
            let cyl = fs
                .iter()
                .filter(|f| {
                    matches!(
                        f.recognized,
                        Some(AnalyticSurface::Cylinder { .. } | AnalyticSurface::Cone { .. })
                    )
                })
                .count();
            let planar = fs.iter().filter(|f| f.kind == FaceKind::Planar).count();
            // Dominant coaxial family: the largest same-axis cylinder/cone group.
            let mut by_axis: BTreeMap<[i64; 3], usize> = BTreeMap::new();
            for f in fs {
                let Some(
                    AnalyticSurface::Cylinder { frame, .. } | AnalyticSurface::Cone { frame, .. },
                ) = &f.recognized
                else {
                    continue;
                };
                if let Some(z) = unit3(frame_z(frame)) {
                    let a = canonical_dir(z);
                    *by_axis.entry([q(a[0]), q(a[1]), q(a[2])]).or_default() += 1;
                }
            }
            let coax = by_axis.values().copied().max().unwrap_or(0);
            (
                fs.len(),
                cyl as f64 / n as f64,
                planar as f64 / n as f64,
                coax as f64 / n as f64,
            )
        });

    let holes = features
        .iter()
        .filter(|f| matches!(f.kind, FeatureKind::ThroughHole | FeatureKind::BlindHole))
        .count();
    let name_lc = name.to_lowercase();
    let name_hits: Vec<&str> = [
        "bolt", "screw", "vis", "nut", "ecrou", "washer", "rondelle", "fastener", "rivet",
    ]
    .into_iter()
    .filter(|t| name_lc.contains(t))
    .collect();
    let name_hint = !name_hits.is_empty();

    let evidence = |base: &str| -> String {
        let mut s = format!(
            "{base} (shape: {aspect:?}, {n_faces} faces, {:.0}% cylindrical/conical, {:.0}% planar, \
             dominant coaxial family {:.0}%, {holes} hole(s))",
            cyl_frac * 100.0,
            planar_frac * 100.0,
            coax_frac * 100.0
        );
        if name_hint {
            use std::fmt::Write as _;
            let _ = write!(
                s,
                " + name hint {name_hits:?} (secondary signal, stated not trusted)"
            );
        }
        s
    };

    // The rules, most specific first. Every accepted class states its evidence.
    if faces.is_some() {
        if aspect == Aspect::Rod && q(cyl_frac) >= q(0.5) && q(coax_frac) >= q(0.4) {
            let (class, base, mut conf) = if q(coax_frac) >= q(0.6) && holes == 0 {
                (
                    PartClass::Fastener,
                    "rod-shaped, dominated by one coaxial cylinder/cone family (shank + tip/head)",
                    0.8,
                )
            } else {
                (
                    PartClass::Shaft,
                    "rod-shaped with a strong coaxial cylinder family",
                    0.7,
                )
            };
            if name_hint {
                conf += 0.1;
            }
            return PartClassification {
                class,
                confidence: quantized(conf),
                why: evidence(base),
            };
        }
        if aspect == Aspect::Plate {
            let through = features
                .iter()
                .filter(|f| f.kind == FeatureKind::ThroughHole)
                .count();
            if through == 1 && n_faces <= 8 && q(cyl_frac) >= q(0.2) {
                let mut conf = 0.75;
                if name_hint {
                    conf += 0.1;
                }
                return PartClassification {
                    class: PartClass::Washer,
                    confidence: quantized(conf),
                    why: evidence("thin annular disc: flat, one coaxial through-bore, few faces"),
                };
            }
            if q(planar_frac) >= q(0.6) {
                let (class, base, conf) = if holes >= 1 && n_faces >= 7 {
                    (
                        PartClass::Bracket,
                        "flat, planar-dominated, with mounting hole(s)",
                        0.7,
                    )
                } else {
                    (PartClass::Plate, "flat and planar-dominated", 0.75)
                };
                return PartClassification {
                    class,
                    confidence: quantized(conf),
                    why: evidence(base),
                };
            }
        }
        if aspect == Aspect::Rod && q(cyl_frac) >= q(0.6) {
            return PartClassification {
                class: PartClass::Pin,
                confidence: quantized(0.65),
                why: evidence("rod-shaped and cylinder-dominated, no head family"),
            };
        }
        if aspect == Aspect::Block
            && n_faces >= 12
            && (holes >= 2 || features.iter().any(|f| f.kind == FeatureKind::Pocket))
        {
            return PartClassification {
                class: PartClass::Housing,
                confidence: quantized(0.6),
                why: evidence("blocky with many faces and internal features"),
            };
        }
        return PartClassification {
            class: PartClass::Unknown,
            confidence: 0.0,
            why: evidence("no rule matched — honestly unclassified"),
        };
    }

    // Mesh-only fallback (tessellation-only parts — no B-rep): shape aspect + triangle-normal spread.
    let (class, base, conf) = match aspect {
        Aspect::Plate => (PartClass::Plate, "flat (mesh-only, no B-rep)", 0.55),
        Aspect::Rod => (PartClass::Shaft, "rod-shaped (mesh-only, no B-rep)", 0.5),
        Aspect::Block => (
            PartClass::Unknown,
            "blocky (mesh-only, no B-rep) — unclassified",
            0.0,
        ),
    };
    let mut conf = conf;
    if name_hint && class != PartClass::Unknown {
        conf += 0.1;
    }
    PartClassification {
        class,
        confidence: quantized(conf),
        why: evidence(base),
    }
}

// ── Repeated-part clustering (D3): instance-of groups ─────────────────────────────────────────────────────
/// One group of geometrically-identical part occurrences. `representative` is the lexicographically
/// smallest member reference — the stable "instance-of" target.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InstanceGroup {
    pub representative: String,
    /// Part-report ids (`PartReport::id`) of every member occurrence.
    pub members: Vec<u64>,
}

/// Group part occurrences that share identical geometry — byte-identical mesh (`mesh_hash`) OR an exactly
/// equal quantized fingerprint (translation/rotation-invariant, so re-exported duplicates with different
/// reference ids still group). Deterministic: exact quantized equality, no similarity threshold to drift.
#[must_use]
pub fn cluster_instances(
    identities: &[crate::reimport_identity::PartIdentity],
) -> Vec<InstanceGroup> {
    let mut groups: BTreeMap<String, (String, Vec<u64>)> = BTreeMap::new();
    for pid in identities {
        // Key: the mesh hash when present (exact), else the quantized fingerprint tuple.
        let key = pid.mesh_hash.map_or_else(
            || {
                let f = &pid.fingerprint;
                format!(
                    "fp:{}:{}:{}:{}:{}:{}",
                    q(f.volume),
                    q(f.area),
                    q(f.moments[0]),
                    q(f.moments[1]),
                    q(f.moments[2]),
                    f.chirality
                )
            },
            |h| format!("mh:{h:016x}"),
        );
        let e = groups
            .entry(key)
            .or_insert_with(|| (pid.reference.clone(), Vec::new()));
        if pid.reference < e.0 {
            e.0.clone_from(&pid.reference);
        }
        e.1.push(pid.id);
    }
    let mut out: Vec<InstanceGroup> = groups
        .into_values()
        .map(|(representative, mut members)| {
            members.sort_unstable();
            InstanceGroup {
                representative,
                members,
            }
        })
        .collect();
    out.sort_by(|a, b| a.representative.cmp(&b.representative));
    out
}

// ── Mesh→CAD recovery (D7): the tessellation-only fallback — opt-in, labeled, never a replacement ─────────
/// A recovered analytic primitive — a LABELED, confidence-scored description of a mesh region. It never
/// replaces geometry; it annotates it (`source: "recovered"`).
#[derive(Clone, PartialEq, Debug)]
pub struct RecoveredPrimitive {
    /// `"plane"` / `"cylinder"` / `"sphere"` — the stable token.
    pub kind: &'static str,
    /// Triangles covered by this region.
    pub tri_count: usize,
    /// RMS distance of region vertices off the fitted surface (scene units) — the measured fit quality.
    pub rms: f64,
    /// `1 − rms/tol` clamped to `[0, 1]`, quantized — honest, measured, never asserted.
    pub confidence: f64,
    /// An auditable, human-readable reconstruction recipe (CAD-Recode-style output honesty: the recovery
    /// is reviewable text, not an opaque swap).
    pub recipe: String,
}

/// Fit tolerance for recovery, relative to the mesh extent (recovered surfaces are preview-grade).
const RECOVER_REL_TOL: f64 = 0.01;

/// Recover analytic primitives from a tessellation-only mesh by deterministic region-growing over
/// triangle normals (planes), then axis-from-normal-covariance fits (cylinders) and centre fits (spheres)
/// on the remainder. Everything is index-ordered + quantized — same mesh ⇒ same recovery.
#[must_use]
#[allow(clippy::too_many_lines, clippy::cast_precision_loss)]
pub fn recover_primitives(mesh: &TriMesh) -> Vec<RecoveredPrimitive> {
    let nt = mesh.triangles.len();
    if nt == 0 {
        return Vec::new();
    }
    // Per-triangle unit normals + centroids.
    let mut tri_n: Vec<[f64; 3]> = Vec::with_capacity(nt);
    let mut tri_c: Vec<[f64; 3]> = Vec::with_capacity(nt);
    for t in &mesh.triangles {
        let [a, b, c] = [
            mesh.positions[t[0] as usize],
            mesh.positions[t[1] as usize],
            mesh.positions[t[2] as usize],
        ];
        let n = cross3(sub3(b, a), sub3(c, a));
        tri_n.push(unit3(n).unwrap_or([0.0, 0.0, 1.0]));
        tri_c.push(scale3(add3(add3(a, b), c), 1.0 / 3.0));
    }
    // Edge adjacency between triangles (shared vertex-index pair).
    let mut edge_owner: BTreeMap<(u32, u32), Vec<usize>> = BTreeMap::new();
    for (i, t) in mesh.triangles.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            let key = (a.min(b), a.max(b));
            edge_owner.entry(key).or_default().push(i);
        }
    }
    let mut neighbors: Vec<Vec<usize>> = vec![Vec::new(); nt];
    for owners in edge_owner.values() {
        for &i in owners {
            for &j in owners {
                if i != j && !neighbors[i].contains(&j) {
                    neighbors[i].push(j);
                }
            }
        }
    }
    for n in &mut neighbors {
        n.sort_unstable();
    }

    // Extent → absolute tolerance.
    let (mut lo, mut hi) = ([f64::INFINITY; 3], [f64::NEG_INFINITY; 3]);
    for p in &mesh.positions {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let extent = norm3(sub3(hi, lo)).max(1e-9);
    let tol = RECOVER_REL_TOL * extent;

    let mut claimed = vec![false; nt];
    let mut out: Vec<RecoveredPrimitive> = Vec::new();

    // ONE region-growing pass over normal-continuous neighbors, then classify each region by its normal
    // SPREAD (the covariance eigenvalues of the region's normals, ASCENDING): all-equal normals → a plane;
    // normals in a plane ⊥ one axis → a cylinder; a full 2-sphere spread → a sphere. Growing smooth
    // regions FIRST (not planes first) matters: the coplanar facet-PAIRS of a tessellated cylinder would
    // otherwise each be claimed as a tiny "plane" and the cylinder never seen.
    for seed in 0..nt {
        if claimed[seed] {
            continue;
        }
        let mut region = vec![seed];
        let mut mark = vec![false; nt];
        mark[seed] = true;
        let mut head = 0;
        while head < region.len() {
            let i = region[head];
            head += 1;
            for &j in &neighbors[i] {
                if mark[j] || claimed[j] {
                    continue;
                }
                if q(dot3(tri_n[j], tri_n[i])) >= q(0.8) {
                    mark[j] = true;
                    region.push(j);
                }
            }
        }
        // Normal spread of the region.
        let mut mean_n = [0.0f64; 3];
        for &i in &region {
            mean_n = add3(mean_n, tri_n[i]);
        }
        mean_n = scale3(mean_n, 1.0 / region.len() as f64);
        let mut cov = [[0.0f64; 3]; 3];
        for &i in &region {
            let d = sub3(tri_n[i], mean_n);
            for r in 0..3 {
                for c in 0..3 {
                    cov[r][c] += d[r] * d[c];
                }
            }
        }
        let evs = crate::reimport_identity::sym3_eigenvalues_sorted(cov);

        // PLANE: every normal in the region is (near-)identical.
        if region.len() >= 2 && q(evs[2]) <= q(1e-4) {
            let n0 = unit3(mean_n).unwrap_or(tri_n[seed]);
            let p0 = tri_c[seed];
            let mut sum2 = 0.0;
            let mut count = 0usize;
            for &i in &region {
                for &vi in &mesh.triangles[i] {
                    let d = dot3(sub3(mesh.positions[vi as usize], p0), n0);
                    sum2 += d * d;
                    count += 1;
                }
            }
            let rms = (sum2 / count.max(1) as f64).sqrt();
            if q(rms) <= q(tol) {
                for &i in &region {
                    claimed[i] = true;
                }
                out.push(RecoveredPrimitive {
                    kind: "plane",
                    tri_count: region.len(),
                    rms,
                    confidence: quantized((1.0 - rms / tol).clamp(0.0, 1.0)),
                    recipe: format!(
                        "plane(normal=({:.4}, {:.4}, {:.4}), point=({:.3}, {:.3}, {:.3})) over {} \
                         tris, rms {:.5}",
                        n0[0],
                        n0[1],
                        n0[2],
                        p0[0],
                        p0[1],
                        p0[2],
                        region.len(),
                        rms
                    ),
                });
                continue;
            }
        }
        if region.len() < 8 {
            continue; // too small to support a curved-surface claim
        }
        // A deterministic axis candidate: the cross product of the first sufficiently-different normal pair.
        let mut axis: Option<[f64; 3]> = None;
        'outer: for &i in &region {
            for &j in &region {
                if q(dot3(tri_n[i], tri_n[j]).abs()) <= q(0.5) {
                    axis = unit3(cross3(tri_n[i], tri_n[j]));
                    break 'outer;
                }
            }
        }
        // Try CYLINDER when the normal spread is planar (smallest eigenvalue ≈ 0 → normals lie in a
        // plane ⊥ the axis; eigenvalues are ASCENDING) and an axis candidate exists.
        if let Some(axis) = axis {
            if q(evs[0]) <= q(0.05 * evs[2].max(1e-12)) {
                // Radius + axis point: average radial distance of region vertices from the axis line
                // through the region centroid.
                let mut c0 = [0.0f64; 3];
                for &i in &region {
                    c0 = add3(c0, tri_c[i]);
                }
                c0 = scale3(c0, 1.0 / region.len() as f64);
                let mut rs: Vec<f64> = Vec::new();
                for &i in &region {
                    for &vi in &mesh.triangles[i] {
                        let d = sub3(mesh.positions[vi as usize], c0);
                        let radial = sub3(d, scale3(axis, dot3(d, axis)));
                        rs.push(norm3(radial));
                    }
                }
                let r_mean = rs.iter().sum::<f64>() / rs.len().max(1) as f64;
                let rms = (rs.iter().map(|r| (r - r_mean) * (r - r_mean)).sum::<f64>()
                    / rs.len().max(1) as f64)
                    .sqrt();
                if q(rms) <= q(tol) && q(r_mean) > q(0.0) {
                    for &i in &region {
                        claimed[i] = true;
                    }
                    let a = canonical_dir(axis);
                    out.push(RecoveredPrimitive {
                        kind: "cylinder",
                        tri_count: region.len(),
                        rms,
                        confidence: quantized((1.0 - rms / tol).clamp(0.0, 1.0)),
                        recipe: format!(
                            "cylinder(radius={r_mean:.4}, axis=({:.4}, {:.4}, {:.4}), \
                             through=({:.3}, {:.3}, {:.3})) over {} tris, rms {:.5}",
                            a[0],
                            a[1],
                            a[2],
                            c0[0],
                            c0[1],
                            c0[2],
                            region.len(),
                            rms
                        ),
                    });
                    continue;
                }
            }
        }
        // Try SPHERE: centre = mean of (vertex − R·normal) is circular; simpler deterministic fit — the
        // centre that makes vertex distances most equal is approximated by c = mean(p_i) − R·mean(n_i)
        // solved by averaging distance: iterate twice (fixed, deterministic).
        {
            let mut c0 = [0.0f64; 3];
            for &i in &region {
                c0 = add3(c0, tri_c[i]);
            }
            c0 = scale3(c0, 1.0 / region.len() as f64);
            let mut centre = c0;
            let mut radius = 0.0;
            for _ in 0..2 {
                let mut rs = 0.0;
                for &i in &region {
                    rs += norm3(sub3(tri_c[i], centre));
                }
                radius = rs / region.len() as f64;
                // Move the centre opposite the mean normal by the radius (spheres: p − r·n = centre).
                let mut est = [0.0f64; 3];
                for &i in &region {
                    est = add3(est, sub3(tri_c[i], scale3(tri_n[i], radius)));
                }
                centre = scale3(est, 1.0 / region.len() as f64);
            }
            let mut sum2 = 0.0;
            let mut count = 0usize;
            for &i in &region {
                for &vi in &mesh.triangles[i] {
                    let d = norm3(sub3(mesh.positions[vi as usize], centre)) - radius;
                    sum2 += d * d;
                    count += 1;
                }
            }
            let rms = (sum2 / count.max(1) as f64).sqrt();
            if q(rms) <= q(tol) && q(radius) > q(0.0) {
                for &i in &region {
                    claimed[i] = true;
                }
                out.push(RecoveredPrimitive {
                    kind: "sphere",
                    tri_count: region.len(),
                    rms,
                    confidence: quantized((1.0 - rms / tol).clamp(0.0, 1.0)),
                    recipe: format!(
                        "sphere(radius={radius:.4}, centre=({:.3}, {:.3}, {:.3})) over {} tris, rms {:.5}",
                        centre[0], centre[1], centre[2], region.len(), rms
                    ),
                });
            }
        }
    }

    // Deterministic order: biggest regions first, then recipe.
    out.sort_by(|a, b| b.tri_count.cmp(&a.tri_count).then(a.recipe.cmp(&b.recipe)));
    out
}

#[cfg(test)]
#[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)] // tiny fixture counts
#[allow(clippy::many_single_char_names)] // o/r/v/n are the canonical geometry names in fixtures
mod tests {
    use super::*;
    use crate::step::CadEdge;

    /// Column-major frame with z = `z`, origin = `o` (x/y any orthonormal completion).
    fn frame(o: [f64; 3], z: [f64; 3]) -> [f64; 16] {
        let z = unit3(z).unwrap();
        let pick = if z[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let x = unit3(cross3(pick, z)).unwrap();
        let y = cross3(z, x);
        [
            x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, o[0], o[1], o[2],
            1.0,
        ]
    }

    /// A full-circle boundary polygon on a cylinder at height `v`, radius `r` around +z at `o`.
    fn circle(o: [f64; 3], r: f64, v: f64, n: usize) -> Vec<[f64; 3]> {
        (0..n)
            .map(|i| {
                let a = std::f64::consts::TAU * i as f64 / n as f64;
                [o[0] + r * a.cos(), o[1] + r * a.sin(), o[2] + v]
            })
            .collect()
    }

    /// A cylinder face: full-sweep wall of radius `r`, height `h`, at `o`, with `same_sense` chosen so the
    /// outward normal points inward (bore) or outward (boss wall).
    fn cyl_face(id: u64, o: [f64; 3], r: f64, h: f64, bore: bool) -> CadFace {
        let mut outer = circle(o, r, 0.0, 16);
        outer.extend(circle(o, r, h, 16));
        CadFace {
            id,
            kind: FaceKind::Curved,
            outer,
            inner: Vec::new(),
            edges: vec![
                CadEdge {
                    id: id * 100 + 1,
                    ends: [[o[0] + r, o[1], o[2]], [o[0] - r, o[1], o[2]]],
                },
                CadEdge {
                    id: id * 100 + 2,
                    ends: [[o[0] + r, o[1], o[2] + h], [o[0] - r, o[1], o[2] + h]],
                },
            ],
            surface: Some(AnalyticSurface::Cylinder {
                frame: frame(o, [0.0, 0.0, 1.0]),
                radius: r,
            }),
            recognized: Some(AnalyticSurface::Cylinder {
                frame: frame(o, [0.0, 0.0, 1.0]),
                radius: r,
            }),
            same_sense: !bore, // same_sense=false flips the radially-outward normal → a bore
            outer_sense: true,
        }
    }

    /// A rectangular planar face in the z=`z` plane, normal +z when `up`.
    fn plane_face(id: u64, cx: f64, cy: f64, half: f64, z: f64, up: bool) -> CadFace {
        // Counter-clockwise seen from +z.
        let mut outer = vec![
            [cx - half, cy - half, z],
            [cx + half, cy - half, z],
            [cx + half, cy + half, z],
            [cx - half, cy + half, z],
        ];
        if !up {
            outer.reverse();
        }
        let edges = (0..4)
            .map(|i| CadEdge {
                id: id * 100 + i as u64,
                ends: [outer[i], outer[(i + 1) % 4]],
            })
            .collect();
        CadFace {
            id,
            kind: FaceKind::Planar,
            outer,
            inner: Vec::new(),
            edges,
            surface: None,
            recognized: None,
            same_sense: true,
            outer_sense: true,
        }
    }

    #[test]
    fn bore_recognized_as_hole_and_boss_as_boss() {
        // A bore (inward normal) with planar faces adjacent at both rims → THROUGH hole.
        let mut bore = cyl_face(1, [0.0, 0.0, 0.0], 5.0, 10.0, true);
        let mut top = plane_face(2, 0.0, 0.0, 20.0, 10.0, true);
        let mut bottom = plane_face(3, 0.0, 0.0, 20.0, 0.0, false);
        // Share rim edge ids: bottom rim edge 101 ↔ bottom plane, top rim edge 102 ↔ top plane.
        top.edges.push(CadEdge {
            id: 102,
            ends: bore.edges[1].ends,
        });
        bottom.edges.push(CadEdge {
            id: 101,
            ends: bore.edges[0].ends,
        });
        // Give the rims non-blend company so the smooth check sees a real corner (bore→plane is not smooth).
        bore.edges.push(CadEdge {
            id: 103,
            ends: bore.edges[0].ends,
        });
        let faces = vec![bore.clone(), top, bottom];
        let feats = recognize_aag(&faces);
        let hole = feats
            .iter()
            .find(|f| matches!(f.kind, FeatureKind::ThroughHole | FeatureKind::BlindHole))
            .expect("a hole is recognized");
        assert_eq!(hole.kind, FeatureKind::ThroughHole, "why: {}", hole.why);
        assert_eq!(hole.faces, vec![1]);
        assert!((hole.radius.unwrap() - 5.0).abs() < 1e-9);
        assert!(hole.confidence > 0.8, "confident: {}", hole.why);
        assert!(verify_feature(hole, &faces).is_ok());

        // The same wall with outward normal → a boss.
        let boss_wall = cyl_face(7, [50.0, 0.0, 0.0], 4.0, 8.0, false);
        let base = plane_face(8, 50.0, 0.0, 20.0, 0.0, true);
        let faces2 = vec![boss_wall, base];
        let feats2 = recognize_aag(&faces2);
        let boss = feats2
            .iter()
            .find(|f| f.kind == FeatureKind::Boss)
            .expect("a boss is recognized");
        assert_eq!(boss.faces, vec![7]);
        assert!(verify_feature(boss, &faces2).is_ok());
    }

    #[test]
    fn kernel_check_rejects_a_wrong_label() {
        // A "hole" claimed on a BOSS wall (outward normal) must be rejected — the wrong-label guard.
        let wall = cyl_face(1, [0.0; 3], 5.0, 10.0, false);
        let faces = vec![wall];
        let wrong = RecognizedFeature {
            kind: FeatureKind::ThroughHole,
            faces: vec![1],
            axis: None,
            origin: None,
            radius: Some(5.0),
            depth: None,
            confidence: 0.99,
            why: "a confidently wrong learned label".into(),
        };
        let err = verify_feature(&wrong, &faces).unwrap_err();
        assert!(err.contains("AWAY from the axis"), "{err}");
        // A claim naming a non-existent face is rejected too.
        let ghost = RecognizedFeature {
            faces: vec![99],
            ..wrong
        };
        assert!(verify_feature(&ghost, &faces)
            .unwrap_err()
            .contains("does not exist"));
    }

    #[test]
    fn recognition_is_deterministic() {
        let faces = vec![
            cyl_face(1, [0.0; 3], 5.0, 10.0, true),
            plane_face(2, 0.0, 0.0, 20.0, 10.0, true),
            plane_face(3, 0.0, 0.0, 20.0, 0.0, false),
            cyl_face(9, [40.0, 0.0, 0.0], 3.0, 6.0, false),
        ];
        let a = recognize_aag(&faces);
        let b = recognize_aag(&faces);
        assert_eq!(a, b, "same faces ⇒ same features, bit-for-bit");
    }

    #[test]
    fn classify_fastener_vs_plate() {
        // A bolt-ish rod: shank cylinder + head cone, coaxial, rod-shaped mesh.
        let shank = cyl_face(1, [0.0; 3], 2.0, 20.0, false);
        let mut head = cyl_face(2, [0.0, 0.0, 20.0], 4.0, 3.0, false);
        head.recognized = Some(AnalyticSurface::Cone {
            frame: frame([0.0, 0.0, 20.0], [0.0, 0.0, 1.0]),
            radius: 4.0,
            semi_angle: 0.4,
        });
        let rod_mesh = metrocalk_csg::TriMesh {
            positions: vec![
                [-2.0, -2.0, 0.0],
                [2.0, -2.0, 0.0],
                [2.0, 2.0, 0.0],
                [-2.0, 2.0, 0.0],
                [-2.0, -2.0, 23.0],
                [2.0, -2.0, 23.0],
                [2.0, 2.0, 23.0],
                [-2.0, 2.0, 23.0],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        let faces = vec![shank, head];
        let c = classify_part(Some(&faces), &rod_mesh, &[], "HEX BOLT M4");
        assert_eq!(c.class, PartClass::Fastener, "why: {}", c.why);
        assert!(c.confidence >= 0.8);
        assert!(
            c.why.contains("name hint"),
            "the secondary signal is stated"
        );

        // A flat plate mesh, planar faces → Plate.
        let plate_mesh = metrocalk_csg::TriMesh {
            positions: vec![
                [0.0, 0.0, 0.0],
                [100.0, 0.0, 0.0],
                [100.0, 80.0, 0.0],
                [0.0, 80.0, 0.0],
                [0.0, 0.0, 2.0],
                [100.0, 0.0, 2.0],
                [100.0, 80.0, 2.0],
                [0.0, 80.0, 2.0],
            ],
            triangles: vec![
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ],
        };
        let pfaces = vec![
            plane_face(1, 50.0, 40.0, 50.0, 0.0, false),
            plane_face(2, 50.0, 40.0, 50.0, 2.0, true),
        ];
        let c2 = classify_part(Some(&pfaces), &plate_mesh, &[], "PLATE 100x80");
        assert_eq!(c2.class, PartClass::Plate, "why: {}", c2.why);
    }

    #[test]
    fn learned_seam_never_fabricates() {
        let faces = vec![cyl_face(1, [0.0; 3], 5.0, 10.0, true)];
        let seam = LearnedRecognizerSeam;
        assert!(
            seam.recognize(&faces).is_empty(),
            "no model ⇒ no recognition — never fabricated"
        );
        assert!(!seam.deterministic());
        assert_eq!(seam.source(), "learned");
    }

    #[test]
    fn instance_clustering_groups_identical_geometry() {
        use crate::reimport_identity::{fingerprint, PartIdentity};
        let mesh = metrocalk_csg::TriMesh {
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            triangles: vec![[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]],
        };
        let fp = fingerprint(&mesh, None);
        let mk = |id: u64, reference: &str, h: Option<u64>| PartIdentity {
            id,
            reference: reference.into(),
            mesh_hash: h,
            world_centroid: [0.0; 3],
            fingerprint: fp,
            name: String::new(),
            parent: None,
        };
        let ids = vec![
            mk(1, "pd-10", Some(42)),
            mk(2, "pd-11", Some(42)),
            mk(3, "pd-12", Some(7)),
        ];
        let groups = cluster_instances(&ids);
        assert_eq!(groups.len(), 2);
        let g42 = groups.iter().find(|g| g.members == vec![1, 2]).unwrap();
        assert_eq!(
            g42.representative, "pd-10",
            "lexicographically smallest member"
        );
    }

    #[test]
    fn recovery_finds_planes_and_cylinder_and_labels_fit() {
        // A coarse open cylinder tube mesh (16 segments), radius 5, height 10.
        let n = 16usize;
        let mut positions = Vec::new();
        for i in 0..n {
            let a = std::f64::consts::TAU * i as f64 / n as f64;
            positions.push([5.0 * a.cos(), 5.0 * a.sin(), 0.0]);
            positions.push([5.0 * a.cos(), 5.0 * a.sin(), 10.0]);
        }
        let mut triangles = Vec::new();
        for i in 0..n {
            let (a0, a1) = ((2 * i) as u32, (2 * i + 1) as u32);
            let (b0, b1) = ((2 * ((i + 1) % n)) as u32, (2 * ((i + 1) % n) + 1) as u32);
            triangles.push([a0, b0, b1]);
            triangles.push([a0, b1, a1]);
        }
        let tube = metrocalk_csg::TriMesh {
            positions,
            triangles,
        };
        let rec = recover_primitives(&tube);
        let cyl = rec
            .iter()
            .find(|r| r.kind == "cylinder")
            .expect("cylinder recovered");
        assert!(
            cyl.recipe.contains("radius="),
            "auditable recipe: {}",
            cyl.recipe
        );
        assert!(cyl.confidence > 0.0, "measured, non-zero fit confidence");
        // Determinism.
        assert_eq!(recover_primitives(&tube), rec);
    }
}
