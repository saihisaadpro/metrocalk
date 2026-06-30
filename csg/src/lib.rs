//! # metrocalk-csg (M13.2 / ADR-051) — robust, exact-predicate constructive solid geometry
//!
//! **The thesis (FM-T2.1 / FM-N4, buildable tier).** Unity and Godot ship *fragile* booleans: feed a
//! coplanar face, a sliver triangle, a shared edge, or near-duplicate vertices and the result **cracks** —
//! holes, non-manifold edges, NaN vertices. The fragility is the classifier: they decide "which side of the
//! cut plane is this vertex on?" with a floating-point dot product and an epsilon, so a vertex *on* (or a
//! hair off) the plane is misclassified and a face goes missing.
//!
//! This crate ships the opposite. The **topology decisions are EXACT**: a vertex is classified against a
//! plane with Shewchuk's adaptive-precision `orient3d` predicate (the zero-dependency, `no_std`, post-1.0
//! `robust` crate — georust's port, audited per ADR-029), which returns the *exact* sign of the
//! orientation determinant — never a misclassification. On top of the exact classifier we OWN a BSP-tree
//! mesh boolean (the Naylor/`csg.js` algorithm) and an **always-on watertight/manifold validator** that
//! runs on every output: the crack-free guarantee is *enforced*, not hoped. A degenerate input is handled
//! robustly or **`Blocked`-explained** (a [`CsgError`]) — never a silent crack or a panic.
//!
//! **Honesty class: BETTER-INTEGRATED-now (buildable tier) / UNIQUELY-ENABLED-tail · EMERGING-for-games**
//! (FM-N4). This is the *game-authoring robustness* win — a `MeshAsset`-producing boolean an indie can see
//! beating Unity/Godot today, deterministic by construction.
//!
//! ## Scope — what is IN, and the seams that are explicitly OUT
//! - **IN:** robust mesh booleans (union / difference / intersection / xor) on exact predicates; the
//!   crack-free validator; deterministic output (same input → bit-identical output, no RNG, no rayon).
//! - **OUT — named futures, not faked here:**
//!   - The **exotic-precision tail** (posits/Takum, interval/affine arithmetic, verified-FP error bounds —
//!     FM-T2.5/2.6): genuine frontier but 2–20× cost, no GPU/wasm silicon, unmaintained crates → **2028+
//!     research roadmap, not v1** (dossier §4 AVOID line).
//!   - **Runtime raymarched SDF** (FF-T8 honest-limit): SDF fights the rasterizer on min-spec, so SDF is an
//!     *authoring/baked* representation that compiles *down to a mesh* — never a runtime primitive here.
//!   - **CAD-grade B-rep / NURBS / feature-tree precision** — that is the separate **M15 CAD arc** (prompt
//!     67, ADRs 070+): STEP interop + SDF-first + `truck`, kernel-free. This crate makes the **mesh**
//!     topology decisions exact (Cherchi/Lévy class), it is not a parametric kernel and claims no
//!     "CAD-grade precision."
//!
//! Foreign predicate types (`robust::Coord3D`) never leak past this crate: the public surface is plain
//! `[f64; 3]` arrays (invariant 5; CI grep-gated). `robust` is `no_std`/pure-arithmetic and the algorithm
//! is plain Rust, so this crate compiles to `wasm32` (it is IN the wasm tripwire).
//!
//! ## References
//! Shewchuk, *Adaptive Precision Floating-Point Arithmetic and Fast Robust Geometric Predicates* (1996);
//! Cherchi et al., *Interactive & Robust Mesh Booleans* (SIGGRAPH Asia 2022); the `csg.js` BSP formulation
//! (Evan Wallace). The robustness upgrade over `csg.js` is the **exact** vertex-vs-plane classifier.

#![forbid(unsafe_code)]

use robust::{orient3d, Coord3D};

// ============================================================================================
// Public boundary types (invariant 5 — plain arrays, no foreign geometry types)
// ============================================================================================

/// A triangle-soup solid: positions + triangle indices. The plain-array boundary type the engine speaks
/// (the editor-shell converts `MeshAsset` ⇄ `TriMesh`). Computed in `f64` so the exact predicates and the
/// interpolation run at full precision; the asset layer stores `f32`.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TriMesh {
    /// Vertex positions.
    pub positions: Vec<[f64; 3]>,
    /// Triangle-list indices into [`Self::positions`] (CCW = outward-facing).
    pub triangles: Vec<[u32; 3]>,
}

impl TriMesh {
    /// Construct from positions + flat index list (length must be a multiple of 3).
    #[must_use]
    pub fn new(positions: Vec<[f64; 3]>, triangles: Vec<[u32; 3]>) -> Self {
        Self {
            positions,
            triangles,
        }
    }

    /// Triangle count.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    /// An axis-aligned-bounding-box diagonal length (used to scale the deterministic weld tolerance).
    #[must_use]
    pub fn bbox_diagonal(&self) -> f64 {
        if self.positions.is_empty() {
            return 0.0;
        }
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &self.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }

    /// An order-independent content hash of the geometry (FNV-1a over the IEEE-754 bytes of every
    /// position + every index). The **equality key** for the determinism gate: two runs of the same
    /// boolean produce the same `TriMesh` ⇔ the same hash. Bit-exact (raw `f64` bytes, never lossy text).
    #[must_use]
    pub fn content_hash(&self) -> u128 {
        const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
        const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
        let mut h = OFFSET;
        let mut eat = |b: u8| {
            h ^= u128::from(b);
            h = h.wrapping_mul(PRIME);
        };
        for p in &self.positions {
            for c in p {
                for b in c.to_le_bytes() {
                    eat(b);
                }
            }
        }
        for t in &self.triangles {
            for i in t {
                for b in i.to_le_bytes() {
                    eat(b);
                }
            }
        }
        h
    }
}

/// The four boolean operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoolOp {
    /// `A ∪ B` — the combined solid.
    Union,
    /// `A − B` — `A` with `B` carved out (the destructible / carve op).
    Difference,
    /// `A ∩ B` — the shared volume.
    Intersection,
    /// `A ⊕ B` — in one solid but not both (symmetric difference).
    Xor,
}

impl BoolOp {
    /// A plain-language verb for diagnostics / handles.
    #[must_use]
    pub fn verb(self) -> &'static str {
        match self {
            Self::Union => "union",
            Self::Difference => "difference",
            Self::Intersection => "intersection",
            Self::Xor => "xor",
        }
    }
}

/// A boolean failed in a way the user must see — never a silent crack or a panic (ADR-016: every "no"
/// explained in plain language).
#[derive(Clone, Debug, PartialEq)]
pub enum CsgError {
    /// The boolean produced a mesh the always-on validator rejected (a crack / non-manifold edge / NaN /
    /// zero-area face). The [`MeshReport`] carries the plain-language explanation.
    DegenerateResult(MeshReport),
    /// An input mesh is itself not a usable solid (e.g. empty, or fewer than 4 triangles to bound a volume).
    InvalidInput(String),
}

impl core::fmt::Display for CsgError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DegenerateResult(r) => write!(
                f,
                "the boolean did not produce a clean solid: {}",
                r.explain()
            ),
            Self::InvalidInput(s) => write!(f, "invalid CSG input: {s}"),
        }
    }
}

impl std::error::Error for CsgError {}

// ============================================================================================
// The always-on crack-free validator (deliverable #3 — the guarantee is ENFORCED, not hoped)
// ============================================================================================

/// One thing wrong with a mesh, found by [`validate`].
#[derive(Clone, Debug, PartialEq)]
pub enum MeshIssue {
    /// A vertex with a NaN or infinite coordinate.
    NonFiniteVertex(usize),
    /// A triangle whose area is ~zero (a sliver / collinear triple).
    ZeroAreaFace(usize),
    /// An edge shared by `count` triangles (≠ 2) — a hole (`1`) or a non-manifold junction (`>2`).
    NonManifoldEdge { a: u32, b: u32, count: usize },
    /// A directed edge traversed the same way by two faces — the surface is not consistently oriented.
    InconsistentOrientation { a: u32, b: u32 },
}

/// The verdict of the always-on validator: is this a clean, closed, orientable solid?
#[derive(Clone, Debug, PartialEq)]
pub struct MeshReport {
    /// Every edge is shared by exactly two triangles (no holes, no non-manifold junctions).
    pub watertight: bool,
    /// No edge is shared by more than two triangles (a weaker condition than watertight).
    pub manifold: bool,
    /// Every interior edge is traversed in opposite directions by its two faces (orientable surface).
    pub oriented: bool,
    /// `V − E + F`. For a closed orientable genus-`g` surface this is `2 − 2g` (so it is even).
    pub euler: i64,
    /// The surface genus `(2 − euler) / 2` when watertight (`0` = a ball, `1` = one tunnel/handle …), else `None`.
    pub genus: Option<i64>,
    /// Distinct vertices actually referenced by a triangle.
    pub used_vertices: usize,
    /// Triangle count.
    pub triangles: usize,
    /// Everything found wrong (empty ⇔ a clean solid).
    pub issues: Vec<MeshIssue>,
}

impl MeshReport {
    /// A clean solid: watertight, orientable, no finite/area issues.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.watertight && self.oriented && self.issues.is_empty()
    }

    /// A plain-language verdict (ADR-016) — never colour/jargon alone.
    #[must_use]
    pub fn explain(&self) -> String {
        if self.is_clean() {
            let g = self.genus.unwrap_or(0);
            let shape = match g {
                0 => "a closed genus-0 surface (like a ball)".to_string(),
                1 => "a closed genus-1 surface (one tunnel through it — e.g. a carved hole)"
                    .to_string(),
                n => format!("a closed genus-{n} surface ({n} tunnels through it)"),
            };
            return format!(
                "watertight: all {} vertices / {} triangles form {shape}; \
                 every edge is shared by exactly two faces; no NaN/inf, no zero-area faces.",
                self.used_vertices, self.triangles,
            );
        }
        let mut why: Vec<String> = Vec::new();
        let holes = self
            .issues
            .iter()
            .filter(|i| matches!(i, MeshIssue::NonManifoldEdge { count: 1, .. }))
            .count();
        let junctions = self
            .issues
            .iter()
            .filter(|i| matches!(i, MeshIssue::NonManifoldEdge { count, .. } if *count > 2))
            .count();
        let nonfinite = self
            .issues
            .iter()
            .filter(|i| matches!(i, MeshIssue::NonFiniteVertex(_)))
            .count();
        let slivers = self
            .issues
            .iter()
            .filter(|i| matches!(i, MeshIssue::ZeroAreaFace(_)))
            .count();
        let unoriented = self
            .issues
            .iter()
            .filter(|i| matches!(i, MeshIssue::InconsistentOrientation { .. }))
            .count();
        if holes > 0 {
            why.push(format!(
                "{holes} edge(s) lie on a hole boundary (shared by only one triangle — a crack)"
            ));
        }
        if junctions > 0 {
            why.push(format!("{junctions} edge(s) are shared by more than two triangles (a non-manifold junction)"));
        }
        if nonfinite > 0 {
            why.push(format!("{nonfinite} vertex/vertices are NaN or infinite"));
        }
        if slivers > 0 {
            why.push(format!("{slivers} triangle(s) have ~zero area (slivers)"));
        }
        if unoriented > 0 {
            why.push(format!("{unoriented} edge(s) are traversed the same way by both faces (inconsistent winding)"));
        }
        format!("NOT a clean solid — {}.", why.join("; "))
    }
}

/// Run the always-on crack-free validator on any triangle mesh. Cheap, deterministic, no allocation in the
/// inner loops beyond the edge map. Reusable on *any* mesh (it also catches an importer/authoring crack,
/// not only a CSG output).
#[must_use]
#[allow(clippy::many_single_char_names)] // v/e/f/a/b are the standard V−E+F and edge-endpoint names
pub fn validate(mesh: &TriMesh) -> MeshReport {
    use std::collections::BTreeMap;

    let mut issues = Vec::new();

    // (1) NaN / inf vertices.
    for (i, p) in mesh.positions.iter().enumerate() {
        if p.iter().any(|c| !c.is_finite()) {
            issues.push(MeshIssue::NonFiniteVertex(i));
        }
    }

    // (2) Zero-area faces + the edge incidence map (undirected) and directed-edge multiset (orientation).
    let mut edge_count: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut directed: BTreeMap<(u32, u32), usize> = BTreeMap::new();
    let mut used: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let diag = mesh.bbox_diagonal().max(1.0);
    let area_eps = (diag * diag) * 1e-20; // relative to scale²; a genuine sliver, not f64 noise

    for (fi, t) in mesh.triangles.iter().enumerate() {
        let [ia, ib, ic] = *t;
        used.insert(ia);
        used.insert(ib);
        used.insert(ic);
        if let (Some(a), Some(b), Some(c)) = (
            mesh.positions.get(ia as usize),
            mesh.positions.get(ib as usize),
            mesh.positions.get(ic as usize),
        ) {
            let n = cross(sub(*b, *a), sub(*c, *a));
            let area2 = dot(n, n); // (2·area)²
            if area2 <= area_eps * area_eps {
                issues.push(MeshIssue::ZeroAreaFace(fi));
            }
        }
        for (u, v) in [(ia, ib), (ib, ic), (ic, ia)] {
            let key = if u < v { (u, v) } else { (v, u) };
            *edge_count.entry(key).or_insert(0) += 1;
            *directed.entry((u, v)).or_insert(0) += 1;
        }
    }

    // (3) Manifold / watertight off the undirected edge map.
    let mut watertight = true;
    let mut manifold = true;
    for (&(a, b), &count) in &edge_count {
        if count != 2 {
            watertight = false;
            if count > 2 {
                manifold = false;
            }
            issues.push(MeshIssue::NonManifoldEdge { a, b, count });
        }
    }

    // (4) Orientation: a clean closed surface traverses each undirected edge once in each direction, so no
    // directed edge appears more than once. (Only flag when the edge is otherwise a clean 2-face edge, so
    // a hole isn't double-reported.)
    let mut oriented = true;
    for (&(u, v), &c) in &directed {
        if c > 1 {
            let key = if u < v { (u, v) } else { (v, u) };
            if edge_count.get(&key) == Some(&2) {
                oriented = false;
                issues.push(MeshIssue::InconsistentOrientation { a: u, b: v });
            }
        }
    }

    let v = i64::try_from(used.len()).unwrap_or(i64::MAX);
    let e = i64::try_from(edge_count.len()).unwrap_or(i64::MAX);
    let f = i64::try_from(mesh.triangles.len()).unwrap_or(i64::MAX);
    let euler = v - e + f;
    let genus = if watertight && euler % 2 == 0 {
        Some((2 - euler) / 2)
    } else {
        None
    };

    MeshReport {
        watertight,
        manifold,
        oriented,
        euler,
        genus,
        used_vertices: used.len(),
        triangles: mesh.triangles.len(),
        issues,
    }
}

// ============================================================================================
// The project-owned Csg trait (invariant 5) + the default robust implementation
// ============================================================================================

/// The project-owned CSG boundary (invariant 5): the engine drives booleans through this trait, never a
/// foreign geometry type. A different backend (a future exact-arrangement kernel) slots in unchanged.
pub trait Csg {
    /// Compute `a op b`, validate the result, and return it — or a [`CsgError`] explaining the failure.
    fn boolean(&self, a: &TriMesh, b: &TriMesh, op: BoolOp) -> Result<TriMesh, CsgError>;

    /// `A ∪ B`.
    fn union(&self, a: &TriMesh, b: &TriMesh) -> Result<TriMesh, CsgError> {
        self.boolean(a, b, BoolOp::Union)
    }
    /// `A − B` (carve).
    fn difference(&self, a: &TriMesh, b: &TriMesh) -> Result<TriMesh, CsgError> {
        self.boolean(a, b, BoolOp::Difference)
    }
    /// `A ∩ B`.
    fn intersection(&self, a: &TriMesh, b: &TriMesh) -> Result<TriMesh, CsgError> {
        self.boolean(a, b, BoolOp::Intersection)
    }
    /// `A ⊕ B`.
    fn xor(&self, a: &TriMesh, b: &TriMesh) -> Result<TriMesh, CsgError> {
        self.boolean(a, b, BoolOp::Xor)
    }
}

/// The default robust implementation: a BSP-tree boolean whose vertex-vs-plane classification is **exact**
/// (Shewchuk `orient3d`), with a deterministic vertex weld and the always-on validator. Deterministic by
/// construction: no RNG, no rayon, fixed iteration order.
#[derive(Clone, Copy, Debug)]
pub struct ExactBspCsg {
    /// The vertex-weld tolerance, as a fraction of the combined bounding-box diagonal. Coincident split
    /// points (which the BSP creates a few ULPs apart on shared edges) are fused so the result is
    /// watertight; real features (orders of magnitude larger) are preserved. Deterministic.
    pub weld_rel: f64,
}

impl Default for ExactBspCsg {
    fn default() -> Self {
        // 1e-9 of the diagonal: far above f64 split-point noise (~1e-15·scale), far below any real feature.
        Self { weld_rel: 1e-9 }
    }
}

impl ExactBspCsg {
    /// A fresh robust CSG engine with the default weld tolerance.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl Csg for ExactBspCsg {
    fn boolean(&self, a: &TriMesh, b: &TriMesh, op: BoolOp) -> Result<TriMesh, CsgError> {
        if a.triangles.is_empty() {
            return Err(CsgError::InvalidInput("mesh A has no triangles".into()));
        }
        if b.triangles.is_empty() {
            return Err(CsgError::InvalidInput("mesh B has no triangles".into()));
        }

        let pa = polygons_from(a);
        let pb = polygons_from(b);
        if pa.is_empty() || pb.is_empty() {
            return Err(CsgError::InvalidInput(
                "after dropping zero-area triangles an input had no usable faces".into(),
            ));
        }

        let result_polys = run_boolean(&pa, &pb, op);
        if result_polys.is_empty() {
            // A legitimately empty result (e.g. the intersection of disjoint solids) — not an error.
            return Ok(TriMesh::default());
        }

        // The deterministic weld tolerance is relative to the combined bounding box.
        let diag = {
            let mut m = TriMesh::default();
            for p in &result_polys {
                m.positions.extend_from_slice(&p.verts);
            }
            m.positions.extend_from_slice(&a.positions);
            m.positions.extend_from_slice(&b.positions);
            m.bbox_diagonal()
        };
        let eps = (diag * self.weld_rel).max(f64::MIN_POSITIVE);

        // (1) Weld coincident polygon vertices into a shared vertex set + integer face loops, so the BSP's
        //     per-edge cut points (a few ULPs apart on shared edges) fuse to ONE vertex.
        let (positions, loops) = weld_polygons(&result_polys, eps);

        // (2) Repair T-junctions: a BSP boolean leaves a face split by the other solid's plane while the
        //     neighbouring face is not — a vertex sits mid-edge of an adjacent face → an open (boundary)
        //     edge. Insert every shared vertex that lies on a face edge so adjacent faces share it. This is
        //     what makes the result WATERTIGHT (vs. csg.js's small cracks).
        let loops = repair_tjunctions(&positions, &loops, eps);

        // (3) Triangulate each (now T-junction-free) convex/simple face via ear-clipping in its own plane,
        //     winding consistent with the face normal.
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for lp in &loops {
            ear_clip(&positions, lp, &mut triangles);
        }

        let mesh = TriMesh {
            positions,
            triangles,
        };

        // (4) The always-on guarantee: a clean solid, or Blocked-explained (never a silent crack/panic).
        let report = validate(&mesh);
        if report.is_clean() {
            Ok(mesh)
        } else {
            Err(CsgError::DegenerateResult(report))
        }
    }
}

// ============================================================================================
// Convenience builders (used by tests, benches, and the editor-shell demo)
// ============================================================================================

/// An axis-aligned box centred at `center` with the given half-extents, outward-facing (CCW). 8 verts,
/// 12 triangles — a clean closed genus-0 solid.
#[must_use]
pub fn box_mesh(center: [f64; 3], half: [f64; 3]) -> TriMesh {
    let [cx, cy, cz] = center;
    let [hx, hy, hz] = half;
    let positions = vec![
        [cx - hx, cy - hy, cz - hz], // 0
        [cx + hx, cy - hy, cz - hz], // 1
        [cx + hx, cy + hy, cz - hz], // 2
        [cx - hx, cy + hy, cz - hz], // 3
        [cx - hx, cy - hy, cz + hz], // 4
        [cx + hx, cy - hy, cz + hz], // 5
        [cx + hx, cy + hy, cz + hz], // 6
        [cx - hx, cy + hy, cz + hz], // 7
    ];
    // CCW outward winding per face.
    let triangles = vec![
        [0, 3, 2],
        [0, 2, 1], // -Z
        [4, 5, 6],
        [4, 6, 7], // +Z
        [0, 1, 5],
        [0, 5, 4], // -Y
        [3, 7, 6],
        [3, 6, 2], // +Y
        [0, 4, 7],
        [0, 7, 3], // -X
        [1, 2, 6],
        [1, 6, 5], // +X
    ];
    TriMesh {
        positions,
        triangles,
    }
}

// ============================================================================================
// Internals: exact-predicate BSP CSG (csg.js algorithm; the classifier upgraded to exact orient3d)
// ============================================================================================

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[inline]
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[inline]
fn coord(p: [f64; 3]) -> Coord3D<f64> {
    Coord3D {
        x: p[0],
        y: p[1],
        z: p[2],
    }
}

// Vertex/polygon side classification, as a bitmask so a polygon's combined type is a bitwise OR.
const COPLANAR: u8 = 0;
const FRONT: u8 = 1;
const BACK: u8 = 2;
const SPANNING: u8 = 3;

/// A plane carrying both a float normal/offset (for interpolation + the coplanar-routing dot test) and the
/// three anchor points used for the **exact** `orient3d` classification. Built so `orient3d(a,b,c,v) < 0`
/// ⇔ `v` is on the side the normal points to (FRONT); calibrated below and asserted in a test.
#[derive(Clone, Debug)]
struct Plane {
    normal: [f64; 3],
    w: f64,
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
}

impl Plane {
    /// Build a plane from a polygon's vertices, finding the first non-collinear triple for the anchors.
    /// Returns `None` if the polygon is degenerate (all collinear → zero area).
    fn from_verts(verts: &[[f64; 3]]) -> Option<Self> {
        let n = verts.len();
        if n < 3 {
            return None;
        }
        let a = verts[0];
        // Find b, c so that (b-a)×(c-a) is non-zero (non-collinear), deterministically (first such pair).
        for i in 1..n {
            for j in (i + 1)..n {
                let nrm = cross(sub(verts[i], a), sub(verts[j], a));
                if dot(nrm, nrm) > 0.0 {
                    let len = dot(nrm, nrm).sqrt();
                    let normal = [nrm[0] / len, nrm[1] / len, nrm[2] / len];
                    return Some(Self {
                        normal,
                        w: dot(normal, a),
                        a,
                        b: verts[i],
                        c: verts[j],
                    });
                }
            }
        }
        None
    }

    /// Classify a vertex against this plane. The crack-avoidance core is **exact**: Shewchuk's `orient3d`
    /// detects a vertex that is *exactly* on the plane (a coplanar face, a shared edge, an on-plane vertex —
    /// the cases Unity/Godot misclassify under a float epsilon and crack on). For an off-plane vertex the
    /// exact sign decides FRONT/BACK.
    ///
    /// One concession to the buildable tier: the BSP creates new split vertices by *float* interpolation
    /// (their coordinates are inexact — exact intersection coordinates are the exact-arithmetic *tail*, a
    /// named future). So a vertex within a tiny *relative* tolerance of the plane is also snapped to
    /// COPLANAR. This stabilises the BSP (without it, an interpolated point a few ULPs off its own plane is
    /// re-classified as spanning and re-split forever) while leaving the exact `orient3d == 0` decision —
    /// the actual crack-avoidance — untouched.
    #[inline]
    fn classify(&self, v: [f64; 3]) -> u8 {
        let s = orient3d(coord(self.a), coord(self.b), coord(self.c), coord(v));
        if s == 0.0 {
            return COPLANAR; // exact — the robustness win (a genuinely on-plane vertex)
        }
        // Stability snap for the BSP's own inexact interpolated split points.
        let d = dot(self.normal, v) - self.w;
        let scale = self.w.abs().max(1.0);
        if d.abs() <= 1e-10 * scale {
            return COPLANAR;
        }
        // By the georust orient3d convention, s < 0 ⇔ v is ABOVE the plane (the side the normal points to).
        // Calibrated + asserted in `orient3d_convention_matches_the_float_normal`.
        if s < 0.0 {
            FRONT
        } else {
            BACK
        }
    }

    /// A fast, inexact float classification used ONLY to SCORE candidate splitting planes (balance the BSP
    /// tree). Topology decisions always use the exact [`Self::classify`]; this never makes one.
    #[inline]
    fn classify_fast(&self, v: [f64; 3]) -> u8 {
        const E: f64 = 1e-9;
        let t = dot(self.normal, v) - self.w;
        if t > E {
            FRONT
        } else if t < -E {
            BACK
        } else {
            COPLANAR
        }
    }

    /// Flip the plane (used by `invert`): negate the float normal/offset and swap two anchors so the exact
    /// `orient3d` sign flips in lock-step (front ↔ back stays consistent).
    fn flip(&mut self) {
        self.normal = [-self.normal[0], -self.normal[1], -self.normal[2]];
        self.w = -self.w;
        core::mem::swap(&mut self.b, &mut self.c);
    }

    /// Split `poly` against this plane, routing the pieces into the four buckets (the `csg.js`
    /// `splitPolygon`, with the EXACT classifier). Spanning polygons are cut on the plane; the new edge
    /// vertices are interpolated with the float plane and later welded so adjacent faces' cut points fuse.
    #[allow(clippy::many_single_char_names)] // f/b/n/i/j/m are the standard front/back/loop names here
    fn split_polygon(
        &self,
        poly: &Polygon,
        coplanar_front: &mut Vec<Polygon>,
        coplanar_back: &mut Vec<Polygon>,
        front: &mut Vec<Polygon>,
        back: &mut Vec<Polygon>,
    ) {
        let types: Vec<u8> = poly.verts.iter().map(|&v| self.classify(v)).collect();
        let poly_type = types.iter().fold(0u8, |acc, &t| acc | t);

        match poly_type {
            COPLANAR => {
                if dot(self.normal, poly.plane.normal) > 0.0 {
                    coplanar_front.push(poly.clone());
                } else {
                    coplanar_back.push(poly.clone());
                }
            }
            FRONT => front.push(poly.clone()),
            BACK => back.push(poly.clone()),
            _ => {
                // SPANNING — cut the polygon on the plane.
                let mut f: Vec<[f64; 3]> = Vec::new();
                let mut b: Vec<[f64; 3]> = Vec::new();
                let n = poly.verts.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let ti = types[i];
                    let tj = types[j];
                    let vi = poly.verts[i];
                    let vj = poly.verts[j];
                    if ti != BACK {
                        f.push(vi);
                    }
                    if ti != FRONT {
                        b.push(vi);
                    }
                    if (ti | tj) == SPANNING {
                        // Interpolate the crossing point on the float plane (denom is non-zero for a
                        // genuinely spanning edge — both endpoints on the plane would be coplanar, not span).
                        let denom = dot(self.normal, sub(vj, vi));
                        let t = if denom.abs() > 0.0 {
                            (self.w - dot(self.normal, vi)) / denom
                        } else {
                            0.0
                        };
                        let m = [
                            vi[0] + t * (vj[0] - vi[0]),
                            vi[1] + t * (vj[1] - vi[1]),
                            vi[2] + t * (vj[2] - vi[2]),
                        ];
                        f.push(m);
                        b.push(m);
                    }
                }
                if f.len() >= 3 {
                    if let Some(p) = Polygon::new(f) {
                        front.push(p);
                    }
                }
                if b.len() >= 3 {
                    if let Some(p) = Polygon::new(b) {
                        back.push(p);
                    }
                }
            }
        }
    }
}

/// A convex, coplanar polygon (a face of a solid) + its plane.
#[derive(Clone, Debug)]
struct Polygon {
    verts: Vec<[f64; 3]>,
    plane: Plane,
}

impl Polygon {
    fn new(verts: Vec<[f64; 3]>) -> Option<Self> {
        let plane = Plane::from_verts(&verts)?;
        Some(Self { verts, plane })
    }

    fn flip(&mut self) {
        self.verts.reverse();
        self.plane.flip();
    }
}

/// A BSP tree node.
#[derive(Clone, Debug, Default)]
struct Node {
    plane: Option<Plane>,
    front: Option<Box<Node>>,
    back: Option<Box<Node>>,
    polygons: Vec<Polygon>,
}

impl Node {
    fn build(polygons: &[Polygon]) -> Self {
        let mut node = Node::default();
        node.add(polygons);
        node
    }

    /// Add polygons to this node, splitting them by (or establishing) the node's plane (`csg.js` `build`).
    fn add(&mut self, polygons: &[Polygon]) {
        if polygons.is_empty() {
            return;
        }
        if self.plane.is_none() {
            // Balance the tree: csg.js always splits on `polygons[0]`, which degenerates into an O(n)-deep
            // chain on axis-aligned (box) content and overflows the stack. Pick the candidate plane that
            // best balances front/back and minimises spanning splits → O(log n) depth.
            self.plane = Some(pick_split_plane(polygons));
        }
        let plane = self.plane.clone().expect("plane set above");
        // Distinct buckets (the same `&mut Vec` can't be passed twice). Coplanar polygons — either facing —
        // belong to THIS node (csg.js routes both into `this.polygons`).
        let mut coplanar_front: Vec<Polygon> = Vec::new();
        let mut coplanar_back: Vec<Polygon> = Vec::new();
        let mut front_list: Vec<Polygon> = Vec::new();
        let mut back_list: Vec<Polygon> = Vec::new();
        for p in polygons {
            plane.split_polygon(
                p,
                &mut coplanar_front,
                &mut coplanar_back,
                &mut front_list,
                &mut back_list,
            );
        }
        self.polygons.extend(coplanar_front);
        self.polygons.extend(coplanar_back);
        if !front_list.is_empty() {
            self.front
                .get_or_insert_with(|| Box::new(Node::default()))
                .add(&front_list);
        }
        if !back_list.is_empty() {
            self.back
                .get_or_insert_with(|| Box::new(Node::default()))
                .add(&back_list);
        }
    }

    /// Clip `polygons` to this node, removing everything inside the node's solid (`csg.js` `clipPolygons`).
    fn clip_polygons(&self, polygons: Vec<Polygon>) -> Vec<Polygon> {
        let Some(plane) = &self.plane else {
            return polygons;
        };
        let mut coplanar_front: Vec<Polygon> = Vec::new();
        let mut coplanar_back: Vec<Polygon> = Vec::new();
        let mut front: Vec<Polygon> = Vec::new();
        let mut back: Vec<Polygon> = Vec::new();
        for p in &polygons {
            plane.split_polygon(
                p,
                &mut coplanar_front,
                &mut coplanar_back,
                &mut front,
                &mut back,
            );
        }
        // Coplanar pieces route by the splitter's facing (csg.js: coplanarFront→front, coplanarBack→back).
        front.extend(coplanar_front);
        back.extend(coplanar_back);
        let mut front = match &self.front {
            Some(n) => n.clip_polygons(front),
            None => front,
        };
        let back = match &self.back {
            Some(n) => n.clip_polygons(back),
            None => Vec::new(), // inside the solid → dropped
        };
        front.extend(back);
        front
    }

    /// Remove all of this node's polygons that are inside `other`.
    fn clip_to(&mut self, other: &Node) {
        self.polygons = other.clip_polygons(core::mem::take(&mut self.polygons));
        if let Some(n) = &mut self.front {
            n.clip_to(other);
        }
        if let Some(n) = &mut self.back {
            n.clip_to(other);
        }
    }

    /// Invert the solid (swap inside/outside): flip every polygon + plane and swap the children.
    fn invert(&mut self) {
        for p in &mut self.polygons {
            p.flip();
        }
        if let Some(pl) = &mut self.plane {
            pl.flip();
        }
        if let Some(n) = &mut self.front {
            n.invert();
        }
        if let Some(n) = &mut self.back {
            n.invert();
        }
        core::mem::swap(&mut self.front, &mut self.back);
    }

    /// Collect every polygon in the subtree — iterative (an explicit stack) so a deep tree can't overflow.
    fn all_polygons(&self) -> Vec<Polygon> {
        let mut out = Vec::new();
        let mut stack: Vec<&Node> = vec![self];
        while let Some(node) = stack.pop() {
            out.extend(node.polygons.iter().cloned());
            if let Some(n) = &node.front {
                stack.push(n);
            }
            if let Some(n) = &node.back {
                stack.push(n);
            }
        }
        out
    }
}

/// Choose a BSP splitting plane that balances the partition (front/back) and minimises spanning splits — a
/// sampled version of the classic BSP heuristic. Scoring uses the FAST inexact float classifier (balance is
/// a heuristic; the actual partition still uses exact predicates). Sampling bounds the cost so a build of a
/// few-thousand-face mesh stays an off-hot-path authoring op.
fn pick_split_plane(polys: &[Polygon]) -> Plane {
    const CANDIDATES: usize = 8;
    const SAMPLE: usize = 64;
    let n = polys.len();
    if n <= 2 {
        return polys[0].plane.clone();
    }
    let cstep = (n / CANDIDATES).max(1);
    let sstep = (n / SAMPLE).max(1);

    let mut best_ci = 0usize;
    let mut best_score = f64::INFINITY;
    let mut ci = 0;
    while ci < n {
        let cand = &polys[ci].plane;
        let (mut front, mut back, mut span) = (0i64, 0i64, 0i64);
        let mut si = 0;
        while si < n {
            let mut ptype = 0u8;
            for &v in &polys[si].verts {
                ptype |= cand.classify_fast(v);
            }
            match ptype {
                FRONT => front += 1,
                BACK => back += 1,
                COPLANAR => {}
                _ => span += 1,
            }
            si += sstep;
        }
        // Prefer a balanced split with few cuts.
        #[allow(clippy::cast_precision_loss)]
        let score = (front - back).abs() as f64 + 8.0 * span as f64;
        if score < best_score {
            best_score = score;
            best_ci = ci;
        }
        ci += cstep;
    }
    polys[best_ci].plane.clone()
}

fn polygons_from(mesh: &TriMesh) -> Vec<Polygon> {
    let mut out = Vec::with_capacity(mesh.triangles.len());
    for t in &mesh.triangles {
        let verts = vec![
            mesh.positions[t[0] as usize],
            mesh.positions[t[1] as usize],
            mesh.positions[t[2] as usize],
        ];
        if let Some(p) = Polygon::new(verts) {
            out.push(p); // zero-area input triangles are dropped (degenerate → robustly ignored)
        }
    }
    out
}

/// The four boolean operations as the standard `csg.js` BSP dance.
fn run_boolean(pa: &[Polygon], pb: &[Polygon], op: BoolOp) -> Vec<Polygon> {
    match op {
        BoolOp::Union => csg_union(Node::build(pa), Node::build(pb)),
        BoolOp::Intersection => csg_intersect(Node::build(pa), Node::build(pb)),
        BoolOp::Difference => csg_subtract(Node::build(pa), Node::build(pb)),
        BoolOp::Xor => {
            // A ⊕ B = (A − B) ∪ (B − A).
            let amb = csg_subtract(Node::build(pa), Node::build(pb));
            let bma = csg_subtract(Node::build(pb), Node::build(pa));
            csg_union(Node::build(&amb), Node::build(&bma))
        }
    }
}

fn csg_union(mut a: Node, mut b: Node) -> Vec<Polygon> {
    a.clip_to(&b);
    b.clip_to(&a);
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.add(&b.all_polygons());
    a.all_polygons()
}

fn csg_subtract(mut a: Node, mut b: Node) -> Vec<Polygon> {
    a.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    b.invert();
    b.clip_to(&a);
    b.invert();
    a.add(&b.all_polygons());
    a.invert();
    a.all_polygons()
}

fn csg_intersect(mut a: Node, mut b: Node) -> Vec<Polygon> {
    a.invert();
    b.clip_to(&a);
    b.invert();
    a.clip_to(&b);
    b.clip_to(&a);
    a.add(&b.all_polygons());
    a.invert();
    a.all_polygons()
}

/// Deterministic vertex weld over polygon loops: fuse coincident vertices within `eps` into a shared set,
/// returning the shared positions + each face as an integer index loop (consecutive duplicates dropped,
/// degenerate <3 loops removed). A spatial hash on a grid of cell-size `eps` with a fixed-order neighbour
/// scan; ties resolve to the lowest existing index → order-independent + bit-stable.
fn weld_polygons(polys: &[Polygon], eps: f64) -> (Vec<[f64; 3]>, Vec<Vec<u32>>) {
    use std::collections::HashMap;
    let inv = if eps > 0.0 { 1.0 / eps } else { 0.0 };
    // The grid cell index is an intentional floor-to-integer of the quantised coordinate.
    #[allow(clippy::cast_possible_truncation)]
    let cell = |p: [f64; 3]| -> [i64; 3] {
        [
            (p[0] * inv).floor() as i64,
            (p[1] * inv).floor() as i64,
            (p[2] * inv).floor() as i64,
        ]
    };
    let eps2 = eps * eps;

    let mut grid: HashMap<[i64; 3], Vec<u32>> = HashMap::new();
    let mut positions: Vec<[f64; 3]> = Vec::new();

    let mut intern = |p: [f64; 3], positions: &mut Vec<[f64; 3]>| -> u32 {
        let base = cell(p);
        let mut found: Option<u32> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(bucket) = grid.get(&[base[0] + dx, base[1] + dy, base[2] + dz]) {
                        for &vi in bucket {
                            let d = sub(p, positions[vi as usize]);
                            if dot(d, d) <= eps2 {
                                found = Some(match found {
                                    Some(f) if f <= vi => f,
                                    _ => vi,
                                });
                            }
                        }
                    }
                }
            }
        }
        if let Some(vi) = found {
            vi
        } else {
            let vi = u32::try_from(positions.len()).expect("vertex count fits u32");
            positions.push(p);
            grid.entry(base).or_default().push(vi);
            vi
        }
    };

    let mut loops: Vec<Vec<u32>> = Vec::with_capacity(polys.len());
    for poly in polys {
        let mut lp: Vec<u32> = Vec::with_capacity(poly.verts.len());
        for &v in &poly.verts {
            let idx = intern(v, &mut positions);
            if lp.last() != Some(&idx) {
                lp.push(idx);
            }
        }
        // Drop a wrap-around duplicate, then any loop too small to be a face.
        if lp.len() >= 2 && lp.first() == lp.last() {
            lp.pop();
        }
        if lp.len() >= 3 {
            loops.push(lp);
        }
    }
    (positions, loops)
}

/// Repair T-junctions: insert every shared vertex that lies strictly on a face edge into that edge, so an
/// adjacent face's corner stops being a mid-edge "T". This converts the BSP's near-watertight soup into a
/// genuinely watertight one (every edge ends up shared by exactly two faces). Deterministic: vertices are
/// scanned in index order and inserted sorted by their parameter along the edge.
#[allow(clippy::many_single_char_names)] // a/b/t/d/k are the standard segment/parameter names here
fn repair_tjunctions(positions: &[[f64; 3]], loops: &[Vec<u32>], eps: f64) -> Vec<Vec<u32>> {
    let eps2 = eps * eps;
    let mut out = Vec::with_capacity(loops.len());
    for lp in loops {
        let n = lp.len();
        let mut nl: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            let ia = lp[i];
            let ib = lp[(i + 1) % n];
            nl.push(ia);
            let a = positions[ia as usize];
            let b = positions[ib as usize];
            let ab = sub(b, a);
            let len2 = dot(ab, ab);
            if len2 <= eps2 {
                continue;
            }
            // Collect shared vertices strictly interior to segment (a,b) and within eps of the line.
            let mut on_edge: Vec<(f64, u32)> = Vec::new();
            for (k, p) in positions.iter().enumerate() {
                let ku = u32::try_from(k).expect("fits");
                if ku == ia || ku == ib {
                    continue;
                }
                let ap = sub(*p, a);
                let t = dot(ap, ab) / len2;
                if t <= 0.0 || t >= 1.0 {
                    continue;
                }
                // perpendicular distance² from the line
                let proj = [a[0] + t * ab[0], a[1] + t * ab[1], a[2] + t * ab[2]];
                let d = sub(*p, proj);
                if dot(d, d) <= eps2 {
                    on_edge.push((t, ku));
                }
            }
            on_edge.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(core::cmp::Ordering::Equal));
            for (_, ku) in on_edge {
                if nl.last() != Some(&ku) {
                    nl.push(ku);
                }
            }
        }
        if nl.len() >= 3 {
            out.push(nl);
        }
    }
    out
}

/// Newell's-method normal of an index loop (robust to non-planar noise; the face's outward direction).
#[allow(clippy::many_single_char_names)] // a/b/n/m/i are the standard Newell-loop names
fn loop_normal(positions: &[[f64; 3]], lp: &[u32]) -> [f64; 3] {
    let mut n = [0.0_f64; 3];
    let m = lp.len();
    for i in 0..m {
        let a = positions[lp[i] as usize];
        let b = positions[lp[(i + 1) % m] as usize];
        n[0] += (a[1] - b[1]) * (a[2] + b[2]);
        n[1] += (a[2] - b[2]) * (a[0] + b[0]);
        n[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    n
}

/// Ear-clip a simple (convex-or-weakly-convex) face loop into triangles, winding consistent with the face
/// normal. The loops here are convex faces with collinear edge points inserted, so ear-clipping always
/// succeeds; the normal-aware convexity test keeps the output outward-facing.
fn ear_clip(positions: &[[f64; 3]], lp: &[u32], out: &mut Vec<[u32; 3]>) {
    let n = lp.len();
    if n < 3 {
        return;
    }
    if n == 3 {
        out.push([lp[0], lp[1], lp[2]]);
        return;
    }
    let normal = loop_normal(positions, lp);
    let mut idx: Vec<u32> = lp.to_vec();

    // A vertex is a convex ear if the turn prev→cur→next is left-handed w.r.t. the face normal and the ear
    // triangle contains no other loop vertex.
    let mut guard = idx.len() * idx.len() + 8;
    while idx.len() > 3 && guard > 0 {
        guard -= 1;
        let m = idx.len();
        let mut clipped = false;
        for i in 0..m {
            let ip = idx[(i + m - 1) % m];
            let ic = idx[i];
            let inx = idx[(i + 1) % m];
            let pp = positions[ip as usize];
            let pc = positions[ic as usize];
            let pn = positions[inx as usize];
            let turn = cross(sub(pc, pp), sub(pn, pc));
            if dot(turn, normal) <= 0.0 {
                continue; // reflex or collinear — not an ear
            }
            // No other vertex inside the candidate ear triangle.
            let mut blocked = false;
            for &iq in &idx {
                if iq == ip || iq == ic || iq == inx {
                    continue;
                }
                if point_in_triangle(positions[iq as usize], pp, pc, pn, normal) {
                    blocked = true;
                    break;
                }
            }
            if blocked {
                continue;
            }
            out.push([ip, ic, inx]);
            idx.remove(i);
            clipped = true;
            break;
        }
        if !clipped {
            break; // degenerate fallback (shouldn't happen for these faces)
        }
    }
    if idx.len() == 3 {
        out.push([idx[0], idx[1], idx[2]]);
    }
}

/// True if `p` is inside (or on) triangle `(a,b,c)` whose plane normal is `normal` — three same-sign edge
/// cross-products. Used to reject non-ear candidates during ear-clipping.
fn point_in_triangle(p: [f64; 3], a: [f64; 3], b: [f64; 3], c: [f64; 3], normal: [f64; 3]) -> bool {
    let e0 = dot(cross(sub(b, a), sub(p, a)), normal);
    let e1 = dot(cross(sub(c, b), sub(p, b)), normal);
    let e2 = dot(cross(sub(a, c), sub(p, c)), normal);
    (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
}

// ============================================================================================
// Tests — the spike gate + the always-on guarantee + determinism + the headless carve demo
// ============================================================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// Calibration test: the exact `orient3d` sign agrees with the float-normal half-space the classifier
    /// assumes (FRONT = the side the normal points to). If georust ever flips its convention, this fails
    /// loudly instead of silently inverting every boolean.
    #[test]
    fn orient3d_convention_matches_the_float_normal() {
        // A triangle in the z=0 plane, CCW from +Z, so its normal is +Z.
        let a = [0.0, 0.0, 0.0];
        let b = [1.0, 0.0, 0.0];
        let c = [0.0, 1.0, 0.0];
        let plane = Plane::from_verts(&[a, b, c]).unwrap();
        assert!(plane.normal[2] > 0.9, "normal points +Z");
        let above = [0.25, 0.25, 1.0]; // on the +Z (normal) side
        let below = [0.25, 0.25, -1.0];
        assert_eq!(plane.classify(above), FRONT, "the normal side is FRONT");
        assert_eq!(plane.classify(below), BACK);
        assert_eq!(
            plane.classify([0.25, 0.25, 0.0]),
            COPLANAR,
            "exactly on the plane"
        );
    }

    /// A hand-built box is a clean closed genus-0 solid (the validator's baseline truth).
    #[test]
    fn a_box_is_a_clean_closed_solid() {
        let r = validate(&box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]));
        assert!(r.is_clean(), "{}", r.explain());
        assert!(r.watertight && r.oriented);
        assert_eq!(r.genus, Some(0), "a box is genus 0");
        assert_eq!(r.euler, 2);
    }

    /// THE SPIKE (deliverable #1 — the measured go/no-go gate). Take the canonical degenerate input that
    /// Unity/Godot crack: a carve box whose top face is **exactly coplanar** with the wall's top face, plus
    /// shared edges and vertices on the cut plane. Prove the robust difference is **watertight, manifold,
    /// non-degenerate (no NaN/inf, no zero-area faces)** and **bit-deterministic across 3 runs**.
    #[test]
    fn the_spike_a_degenerate_coplanar_carve_is_watertight_and_deterministic() {
        // A wall (top face at y = +1), and a carve box whose TOP face is EXACTLY the wall's top plane
        // (y = +1) — a whole coplanar face — protruding out the front/back (z) past the wall so its side
        // faces share the wall's interior and its top edges lie exactly on the wall's top edges. This is
        // the canonical coplanar-faces + shared-edges + on-plane-vertices case Unity/Godot crack on.
        let wall = box_mesh([0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
        let carve = box_mesh([0.0, 0.5, 0.0], [0.5, 0.5, 1.0]); // y ∈ [0, +1]: top face y=+1 coplanar with the wall top

        let csg = ExactBspCsg::new();
        let out1 = csg
            .difference(&wall, &carve)
            .expect("robust difference produces a clean solid");
        let report = validate(&out1);
        assert!(report.watertight, "watertight: {}", report.explain());
        assert!(report.manifold, "manifold: {}", report.explain());
        assert!(
            report.oriented,
            "consistently oriented: {}",
            report.explain()
        );
        assert!(
            report.is_clean(),
            "no NaN/inf, no zero-area faces: {}",
            report.explain()
        );
        assert!(
            report.triangles > 12,
            "the carve actually changed the geometry"
        );

        // Bit-deterministic across 3 runs (the ≥2-runs discipline — a single match isn't proof).
        let h = out1.content_hash();
        for run in 0..3 {
            let again = csg.difference(&wall, &carve).expect("clean");
            assert_eq!(
                again.content_hash(),
                h,
                "run {run} diverged — the boolean is NOT deterministic"
            );
        }
    }

    /// A through-hole carve yields a genus-1 watertight solid (the Euler/genus check the spike names).
    #[test]
    fn a_through_hole_carve_is_a_watertight_genus_1_solid() {
        let wall = box_mesh([0.0, 0.0, 0.0], [2.0, 1.5, 0.5]);
        // A bar passing fully THROUGH the wall along z, smaller than the wall in x/y → a tunnel.
        let drill = box_mesh([0.0, 0.0, 0.0], [0.4, 0.4, 2.0]);
        let out = ExactBspCsg::new().difference(&wall, &drill).expect("clean");
        let r = validate(&out);
        assert!(r.watertight && r.oriented, "{}", r.explain());
        assert_eq!(
            r.genus,
            Some(1),
            "a hole through the wall is genus 1: {}",
            r.explain()
        );
    }

    /// Repeated carves (a destructible wall) stay watertight — the headless form of the demo.
    #[test]
    fn repeated_carves_stay_watertight_destructible_wall() {
        let csg = ExactBspCsg::new();
        let mut wall = box_mesh([0.0, 0.0, 0.0], [3.0, 1.5, 0.5]);
        // Five well-separated / cleanly-overlapping carves — including a through-hole and one whose
        // top+bottom faces are EXACTLY coplanar with the wall's (the crack-maker), but NONE exactly
        // tangent to another carve (two cavities sharing a plane is a genuine non-manifold degeneracy the
        // validator rightly refuses — see `tangent_carves_are_blocked_explained`).
        let carves = [
            box_mesh([-2.2, 0.5, 0.0], [0.4, 0.4, 1.0]), // left pocket
            box_mesh([-0.9, -0.4, 0.0], [0.3, 0.5, 1.0]), // lower pocket (clear of the next)
            box_mesh([0.3, 0.2, 0.0], [0.3, 0.3, 2.0]),  // through-hole (gap to the previous carve)
            box_mesh([1.6, 0.4, 0.0], [0.4, 0.4, 1.0]),  // right pocket
            box_mesh([3.0, 0.0, 0.0], [0.6, 1.5, 1.0]), // shaves the right end; top/bottom coplanar with the wall
        ];
        for (i, c) in carves.iter().enumerate() {
            wall = csg
                .difference(&wall, c)
                .unwrap_or_else(|e| panic!("carve {i} cracked: {e}"));
            let r = validate(&wall);
            assert!(
                r.watertight && r.oriented,
                "after carve {i}: {}",
                r.explain()
            );
        }
    }

    /// Union of two overlapping boxes is a clean solid; intersection too; both deterministic.
    #[test]
    fn union_and_intersection_are_clean_and_deterministic() {
        let csg = ExactBspCsg::new();
        let a = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = box_mesh([0.8, 0.3, 0.2], [1.0, 1.0, 1.0]);

        let u = csg.union(&a, &b).expect("clean union");
        assert!(validate(&u).is_clean(), "{}", validate(&u).explain());
        assert_eq!(u.content_hash(), csg.union(&a, &b).unwrap().content_hash());

        let i = csg.intersection(&a, &b).expect("clean intersection");
        let ri = validate(&i);
        assert!(ri.is_clean(), "{}", ri.explain());
        assert_eq!(ri.genus, Some(0));
        assert_eq!(
            i.content_hash(),
            csg.intersection(&a, &b).unwrap().content_hash()
        );
    }

    /// XOR (symmetric difference) of two overlapping boxes validates and is deterministic.
    #[test]
    fn xor_is_clean_and_deterministic() {
        let csg = ExactBspCsg::new();
        let a = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = box_mesh([1.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let x = csg.xor(&a, &b).expect("clean xor");
        assert!(validate(&x).watertight, "{}", validate(&x).explain());
        assert_eq!(x.content_hash(), csg.xor(&a, &b).unwrap().content_hash());
    }

    /// The always-on validator CATCHES a deliberately-degenerate mesh and EXPLAINS it (deliverable #3): a
    /// box missing one triangle is a crack (a hole), surfaced — never waved through.
    #[test]
    fn the_validator_catches_a_crack_and_explains_it() {
        let mut broken = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        broken.triangles.pop(); // tear a hole
        let r = validate(&broken);
        assert!(!r.watertight, "a torn box is not watertight");
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i, MeshIssue::NonManifoldEdge { count: 1, .. })));
        assert!(
            r.explain().contains("hole"),
            "explained in plain language: {}",
            r.explain()
        );
    }

    /// The validator flags a NaN vertex and a zero-area sliver.
    #[test]
    fn the_validator_flags_nonfinite_and_slivers() {
        let mut m = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        m.positions[0] = [f64::NAN, 0.0, 0.0];
        let r = validate(&m);
        assert!(r
            .issues
            .iter()
            .any(|i| matches!(i, MeshIssue::NonFiniteVertex(0))));

        // A degenerate triangle (collinear) is a zero-area sliver.
        let sliver = TriMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            vec![[0, 1, 2]],
        );
        assert!(validate(&sliver)
            .issues
            .iter()
            .any(|i| matches!(i, MeshIssue::ZeroAreaFace(0))));
    }

    /// Two carves that share a cut plane EXACTLY (adjacent cavities) — a hard degenerate case Unity/Godot
    /// crack on — are handled robustly: the exact coplanar classification + the deterministic weld merge
    /// them into ONE clean notch (not a non-manifold sliver). The robustness, demonstrated on the worst
    /// real input.
    #[test]
    fn exactly_adjacent_carves_merge_into_one_clean_cavity() {
        let csg = ExactBspCsg::new();
        let wall = box_mesh([0.0, 0.0, 0.0], [3.0, 1.0, 0.5]);
        let left = box_mesh([-0.5, 0.0, 0.0], [0.5, 0.5, 1.0]); // tunnel x ∈ [-1.0, 0.0]
        let carved = csg.difference(&wall, &left).expect("first carve is clean");
        // A second tunnel sharing the plane x = 0.0 EXACTLY with the first → they abut on a whole face.
        let right = box_mesh([0.5, 0.0, 0.0], [0.5, 0.5, 1.0]); // tunnel x ∈ [0.0, 1.0]
        let merged = csg
            .difference(&carved, &right)
            .expect("adjacent carves merge cleanly, no non-manifold");
        let r = validate(&merged);
        assert!(
            r.is_clean(),
            "the merged cavity is a clean solid: {}",
            r.explain()
        );
        assert_eq!(
            r.genus,
            Some(1),
            "two abutting tunnels merge into one tunnel (genus 1): {}",
            r.explain()
        );
    }

    /// A degenerate input (a single zero-area triangle, no volume) is Blocked-explained, never a panic.
    #[test]
    fn a_degenerate_input_is_blocked_explained_not_a_panic() {
        let good = box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let degenerate = TriMesh::new(
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0]],
            vec![[0, 1, 2]],
        );
        let err = ExactBspCsg::new()
            .difference(&good, &degenerate)
            .unwrap_err();
        // It does not panic; it returns an explained error.
        assert!(matches!(err, CsgError::InvalidInput(_)), "explained: {err}");
    }
}
