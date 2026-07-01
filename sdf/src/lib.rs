//! # metrocalk-sdf (M15.0 / ADR-070, Leg B) — SDF/implicit as the **canonical geometry** representation
//!
//! **The bet (FF-T8, CAD-N5 leg (c)).** "Geometry is a program you evaluate." A shape is an analytic
//! **signed-distance field** — negative inside, zero on the surface, positive outside — and **CSG is free**:
//! union is `min`, intersection is `max`, difference is `max(a, -b)`, evaluated pointwise. The field is the
//! authoritative, resolution-independent, GPU-native, CRDT-diffable rep that stays coherent with **every**
//! Metrocalk axis (determinism · min-spec · wgpu · wasm).
//!
//! **The honest limit (FF-T8, baked into the design — not papered over).** Raymarching an SDF at runtime
//! fights the rasterizer on min-spec, so SDF here is an **authoring / baked** representation: it compiles
//! **down to a deterministic, watertight, manifold triangle mesh** for the wgpu rasterizer (reusing the
//! M13.2 mesh path). There is **no runtime raymarcher** in this crate — that exclusion is a decision, not an
//! omission (ADR-070).
//!
//! **Watertight + manifold by construction.** The compiler is **Marching Tetrahedra** over the space-tiling
//! Freudenthal 6-tet decomposition of a uniform grid, with surface vertices **deduplicated by their global
//! grid-edge id**. Unlike Marching Cubes (whose ambiguous faces can produce non-manifold junctions), MT over
//! a conforming tetrahedralization yields a **closed, orientable, manifold** surface for any field with no
//! grid sample landing exactly on the isosurface. The guarantee is **structural**; it is additionally
//! **debug-asserted every `compile`** via the M13.2 `validate`, **enforced at the `sdf_intent::bake` seam**
//! (a non-watertight compile is `Blocked`, never enters the engine), and checked in every test (release
//! trusts the structural guarantee — the O(tris) check is off the hot path).
//!
//! **Determinism (ADR-020, re-confirmed test-first here).** The field evaluates using **only IEEE-754
//! correctly-rounded operations** (`+ - * /`, `sqrt`, `min`, `max`, `abs`, comparisons) — **no `fma`, no
//! transcendentals** — and the compiler iterates a fixed grid with no RNG and no rayon. So the compiled mesh
//! is **bit-deterministic** (same field + grid ⇒ identical [`metrocalk_csg::TriMesh::content_hash`]). Native
//! `f64` is bit-identical (the standing ADR-020 property); the crate is wasm-portable (it is IN the wasm
//! tripwire), and because it avoids `fma`/transcendentals it is *designed* to be wasm-deterministic too — the
//! cross-platform wasm-run number is the CI `sdf-determinism` job (wasmtime), with the web path
//! server-authoritative until confirmed (the standing wasm32 boundary; the Wasm-3.0 deterministic profile is
//! the named future).
//!
//! **Reuse, not reinvention (ADR-051).** The mesh boundary type ([`metrocalk_csg::TriMesh`]), the always-on
//! watertight/manifold/oriented **validator** ([`metrocalk_csg::validate`]), and the exact-predicate mesh
//! path are the M13.2 substrate — this crate adds **no new geometry-predicate dependency**.
//!
//! **SDF→B-rep is NOT lossless (§4 AVOID).** Fitting a B-rep or an exact mesh back out of a field has a real
//! fitting error bounded by the grid resolution; the honest measure is the **chord tolerance budget**
//! ([`Grid::chord_tolerance`]) we publish, never a "lossless" claim.

#![forbid(unsafe_code)]

use std::collections::HashMap;

pub use metrocalk_csg::{validate, MeshReport, TriMesh};

// ============================================================================================
// The field — "geometry is a program"
// ============================================================================================

/// The axis a [`Sdf::Cylinder`] is aligned to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Aligned to X.
    X,
    /// Aligned to Y (the default "upright" cylinder).
    Y,
    /// Aligned to Z.
    Z,
}

/// An analytic signed-distance field: an immutable **program** whose value at a point is the signed
/// distance to the surface (negative inside). CSG combinators (`Union`/`Difference`/`Intersection`) compose
/// primitives into the canonical rep — the tree IS the geometry (CRDT-diffable, AI-authorable).
///
/// The sign is **exact at the surface** for every combinator (`min`/`max` of true fields preserve the zero
/// isosurface); the *magnitude* away from the surface is a valid bound, which is all surface extraction
/// needs.
#[derive(Clone, Debug, PartialEq)]
pub enum Sdf {
    /// A sphere of `radius` centred at `center`.
    Sphere {
        /// Centre.
        center: [f64; 3],
        /// Radius (> 0).
        radius: f64,
    },
    /// An axis-aligned box: `|p - center|` clamped against the per-axis `half` extents.
    Cuboid {
        /// Centre.
        center: [f64; 3],
        /// Half-extents per axis (all > 0).
        half: [f64; 3],
    },
    /// A finite cylinder of `radius` and `half_height`, aligned to `axis`, centred at `center`.
    Cylinder {
        /// Centre.
        center: [f64; 3],
        /// Radius (> 0).
        radius: f64,
        /// Half of the length along `axis` (> 0).
        half_height: f64,
        /// The alignment axis.
        axis: Axis,
    },
    /// `A ∪ B` — the union (min of the two fields).
    Union(Box<Sdf>, Box<Sdf>),
    /// `A − B` — carve B out of A (`max(a, -b)`).
    Difference(Box<Sdf>, Box<Sdf>),
    /// `A ∩ B` — the intersection (max of the two fields).
    Intersection(Box<Sdf>, Box<Sdf>),
}

impl Sdf {
    /// A sphere.
    #[must_use]
    pub fn sphere(center: [f64; 3], radius: f64) -> Self {
        Sdf::Sphere { center, radius }
    }

    /// An axis-aligned box.
    #[must_use]
    pub fn cuboid(center: [f64; 3], half: [f64; 3]) -> Self {
        Sdf::Cuboid { center, half }
    }

    /// A finite cylinder aligned to `axis`.
    #[must_use]
    pub fn cylinder(center: [f64; 3], radius: f64, half_height: f64, axis: Axis) -> Self {
        Sdf::Cylinder {
            center,
            radius,
            half_height,
            axis,
        }
    }

    /// `self ∪ other` (consuming builder — the program grows).
    #[must_use]
    pub fn union(self, other: Sdf) -> Self {
        Sdf::Union(Box::new(self), Box::new(other))
    }

    /// `self − other` (carve `other` out of `self`).
    #[must_use]
    pub fn difference(self, other: Sdf) -> Self {
        Sdf::Difference(Box::new(self), Box::new(other))
    }

    /// `self ∩ other`.
    #[must_use]
    pub fn intersection(self, other: Sdf) -> Self {
        Sdf::Intersection(Box::new(self), Box::new(other))
    }

    /// The signed distance at `p` (negative inside). Uses ONLY IEEE-754 correctly-rounded operations
    /// (no `fma`, no transcendentals) so the evaluation is bit-deterministic and wasm-portable.
    #[must_use]
    pub fn eval(&self, p: [f64; 3]) -> f64 {
        match self {
            Sdf::Sphere { center, radius } => length(sub(p, *center)) - radius,
            Sdf::Cuboid { center, half } => sd_box(sub(p, *center), *half),
            Sdf::Cylinder {
                center,
                radius,
                half_height,
                axis,
            } => sd_cylinder(sub(p, *center), *radius, *half_height, *axis),
            Sdf::Union(a, b) => a.eval(p).min(b.eval(p)),
            Sdf::Difference(a, b) => a.eval(p).max(-b.eval(p)),
            Sdf::Intersection(a, b) => a.eval(p).max(b.eval(p)),
        }
    }

    /// A conservative axis-aligned bounding box `(min, max)` that contains the solid (`field ≤ 0` region).
    /// Used to auto-size the meshing grid. For a difference `A − B` the solid is a subset of `A`, so `A`'s
    /// box bounds it.
    #[must_use]
    pub fn bounds(&self) -> ([f64; 3], [f64; 3]) {
        match self {
            Sdf::Sphere { center, radius } => sym(*center, [*radius; 3]),
            Sdf::Cuboid { center, half } => sym(*center, *half),
            Sdf::Cylinder {
                center,
                radius,
                half_height,
                axis,
            } => {
                let r = *radius;
                let h = *half_height;
                let ext = match axis {
                    Axis::X => [h, r, r],
                    Axis::Y => [r, h, r],
                    Axis::Z => [r, r, h],
                };
                sym(*center, ext)
            }
            Sdf::Union(a, b) => union_box(a.bounds(), b.bounds()),
            // A − B ⊆ A; A ∩ B ⊆ A. Bounding by the left operand is conservative and cheap.
            Sdf::Difference(a, _) | Sdf::Intersection(a, _) => a.bounds(),
        }
    }
}

// ── primitive distance functions (Quilez), f64, correctly-rounded ops only ─────────────────────────────

#[inline]
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[inline]
fn length(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Signed distance to an axis-aligned box of half-extents `half`, evaluated at `q = p - center`.
#[inline]
fn sd_box(q: [f64; 3], half: [f64; 3]) -> f64 {
    let d = [
        q[0].abs() - half[0],
        q[1].abs() - half[1],
        q[2].abs() - half[2],
    ];
    let outside = length([d[0].max(0.0), d[1].max(0.0), d[2].max(0.0)]);
    let inside = d[0].max(d[1].max(d[2])).min(0.0);
    outside + inside
}

/// Signed distance to a finite cylinder (radius `r`, half-height `h`) along `axis`, at `q = p - center`.
#[inline]
#[allow(clippy::many_single_char_names)] // r/h/u/v/w are the standard cylinder radial/axial names
fn sd_cylinder(q: [f64; 3], r: f64, h: f64, axis: Axis) -> f64 {
    // Decompose q into (radial-plane components, axial component).
    let (u, v, w) = match axis {
        Axis::X => (q[1], q[2], q[0]),
        Axis::Y => (q[0], q[2], q[1]),
        Axis::Z => (q[0], q[1], q[2]),
    };
    let d_rad = (u * u + v * v).sqrt() - r; // distance from the axis, minus radius
    let d_ax = w.abs() - h; // distance past the caps
    let outside = ((d_rad.max(0.0)).powi_sq() + (d_ax.max(0.0)).powi_sq()).sqrt();
    let inside = d_rad.max(d_ax).min(0.0);
    outside + inside
}

/// Helper: `x * x` without any `fma` risk (explicit multiply keeps the op-graph IEEE-754-deterministic).
trait Sq {
    fn powi_sq(self) -> f64;
}
impl Sq for f64 {
    #[inline]
    fn powi_sq(self) -> f64 {
        self * self
    }
}

#[inline]
fn sym(center: [f64; 3], ext: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    (
        [center[0] - ext[0], center[1] - ext[1], center[2] - ext[2]],
        [center[0] + ext[0], center[1] + ext[1], center[2] + ext[2]],
    )
}

#[inline]
fn union_box(a: ([f64; 3], [f64; 3]), b: ([f64; 3], [f64; 3])) -> ([f64; 3], [f64; 3]) {
    let mut lo = a.0;
    let mut hi = a.1;
    for k in 0..3 {
        lo[k] = lo[k].min(b.0[k]);
        hi[k] = hi[k].max(b.1[k]);
    }
    (lo, hi)
}

// ============================================================================================
// The compiler — Marching Tetrahedra → a deterministic, watertight, manifold TriMesh
// ============================================================================================

/// The regular sampling grid the field is compiled over. `res` cells per axis (so `(res+1)^3` samples);
/// `iso` is the isolevel (0 = the true surface). Choose bounds so no sample lands exactly on the surface —
/// the resulting mesh is then watertight + manifold by construction ([`compile`] validates it).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Lower corner of the sampling box.
    pub min: [f64; 3],
    /// Upper corner of the sampling box.
    pub max: [f64; 3],
    /// Cells per axis (the surface has ~O(res²) triangles).
    pub res: usize,
    /// The isolevel (0 = the surface).
    pub iso: f64,
}

impl Grid {
    /// A grid that bounds the field's solid with a small relative `margin` (so the walls aren't clipped),
    /// at `res` cells per axis. The bounds are nudged by an irrational fraction of a cell so an
    /// axis-aligned face is unlikely to coincide with a sample plane (avoids the degenerate on-surface
    /// sample).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // grid indices/resolution are small; the cast is exact here
    pub fn around(sdf: &Sdf, res: usize, margin: f64) -> Self {
        let (lo, hi) = sdf.bounds();
        let mut min = [0.0; 3];
        let mut max = [0.0; 3];
        for k in 0..3 {
            let span = (hi[k] - lo[k]).max(1e-6);
            let m = span * margin;
            // The 0.031830989… offset (≈ 1/(10π)) de-aligns sample planes from axis-aligned faces.
            let jitter = span * 0.031_830_989 / (res.max(1) as f64);
            min[k] = lo[k] - m + jitter;
            max[k] = hi[k] + m + jitter;
        }
        Grid {
            min,
            max,
            res,
            iso: 0.0,
        }
    }

    /// The published **chord tolerance budget** for this grid: the largest cell edge. A tessellated planar
    /// face is exact (0), but a curved surface deviates from the mesh by at most ~half a cell — the honest
    /// number we declare instead of claiming "lossless" (§4).
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // small resolution; the cast is exact here
    pub fn chord_tolerance(&self) -> f64 {
        let mut m: f64 = 0.0;
        for k in 0..3 {
            m = m.max((self.max[k] - self.min[k]).abs() / (self.res.max(1) as f64));
        }
        m
    }

    #[inline]
    #[allow(clippy::cast_precision_loss)] // grid indices/resolution are small; the cast is exact here
    fn sample_pos(&self, i: usize, j: usize, k: usize) -> [f64; 3] {
        let f = |lo: f64, hi: f64, idx: usize| lo + (hi - lo) * (idx as f64) / (self.res as f64);
        [
            f(self.min[0], self.max[0], i),
            f(self.min[1], self.max[1], j),
            f(self.min[2], self.max[2], k),
        ]
    }

    #[inline]
    fn vid(&self, i: usize, j: usize, k: usize) -> u64 {
        let n = (self.res + 1) as u64;
        (i as u64) + n * ((j as u64) + n * (k as u64))
    }
}

/// The 6 space-tiling tetrahedra of a cube (Freudenthal / Kuhn decomposition), each a monotone path from
/// corner 0 = (0,0,0) to corner 7 = (1,1,1); every tet shares the main diagonal 0–7, so adjacent cells'
/// shared faces are split identically ⇒ the tetrahedralization is **conforming** (watertight across cells).
/// Corner index = di + 2·dj + 4·dk for local offset (di,dj,dk) ∈ {0,1}³.
const TETS: [[usize; 4]; 6] = [
    [0, 1, 3, 7],
    [0, 1, 5, 7],
    [0, 2, 3, 7],
    [0, 2, 6, 7],
    [0, 4, 5, 7],
    [0, 4, 6, 7],
];

/// Local corner offsets, indexed by corner index (di + 2·dj + 4·dk).
const CORNER: [[usize; 3]; 8] = [
    [0, 0, 0],
    [1, 0, 0],
    [0, 1, 0],
    [1, 1, 0],
    [0, 0, 1],
    [1, 0, 1],
    [0, 1, 1],
    [1, 1, 1],
];

/// Compile a field into a deterministic, watertight, manifold triangle mesh by Marching Tetrahedra.
///
/// Deterministic by construction: fixed grid iteration, no RNG, no rayon, `f64` only. Surface vertices are
/// deduplicated by their global grid-edge id, so the result is a **closed** surface; each triangle is
/// oriented outward (normal toward the positive/outside field), so the surface is consistently **oriented**.
/// Run [`validate`] on the result to assert watertight/manifold/oriented (the always-on M13.2 gate).
#[must_use]
pub fn compile(sdf: &Sdf, grid: &Grid) -> TriMesh {
    let res = grid.res;
    let n = res + 1;
    // Pre-sample the field on the full grid (deterministic order).
    let mut vals = vec![0.0f64; n * n * n];
    let sidx = |i: usize, j: usize, k: usize| i + n * (j + n * k);
    for k in 0..n {
        for j in 0..n {
            for i in 0..n {
                vals[sidx(i, j, k)] = sdf.eval(grid.sample_pos(i, j, k));
            }
        }
    }

    let mut positions: Vec<[f64; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    // Global grid-edge id (ordered vertex-id pair) → surface-vertex index. The shared key across cells is
    // what makes the mesh watertight.
    let mut vmap: HashMap<(u64, u64), u32> = HashMap::new();

    for ck in 0..res {
        for cj in 0..res {
            for ci in 0..res {
                // The 8 cube corners: positions, field values, global vertex ids.
                let mut cp = [[0.0f64; 3]; 8];
                let mut cv = [0.0f64; 8];
                let mut cg = [0u64; 8];
                for (c, off) in CORNER.iter().enumerate() {
                    let (i, j, k) = (ci + off[0], cj + off[1], ck + off[2]);
                    cp[c] = grid.sample_pos(i, j, k);
                    cv[c] = vals[sidx(i, j, k)];
                    cg[c] = grid.vid(i, j, k);
                }
                for tet in &TETS {
                    emit_tet(
                        [cp[tet[0]], cp[tet[1]], cp[tet[2]], cp[tet[3]]],
                        [cv[tet[0]], cv[tet[1]], cv[tet[2]], cv[tet[3]]],
                        [cg[tet[0]], cg[tet[1]], cg[tet[2]], cg[tet[3]]],
                        grid.iso,
                        &mut positions,
                        &mut triangles,
                        &mut vmap,
                    );
                }
            }
        }
    }

    let mesh = TriMesh::new(positions, triangles);
    // Watertight+manifold BY CONSTRUCTION (edge-keyed dedup + the conforming Freudenthal decomposition);
    // debug-assert it every compile so the guarantee is runtime-enforced in debug/tests, not merely prose
    // (release skips the O(tris) check — the guarantee is structural; the `sdf_intent::bake` seam validates
    // at the point geometry enters the engine, and every test validates).
    debug_assert!(
        validate(&mesh).is_clean() || mesh.triangles.is_empty(),
        "MT compile must be watertight+manifold+oriented by construction: {}",
        validate(&mesh).explain()
    );
    mesh
}

/// True iff [`compile`] produces a **bit-identical** mesh across `runs` (≥2) runs — the determinism gate
/// (compares [`TriMesh::content_hash`]).
#[must_use]
pub fn compile_reproduces(sdf: &Sdf, grid: &Grid, runs: usize) -> bool {
    let first = compile(sdf, grid).content_hash();
    (1..runs.max(2)).all(|_| compile(sdf, grid).content_hash() == first)
}

/// Emit the Marching-Tetrahedra polygon for one tetrahedron. Vertices are keyed by global grid-edge id
/// (dedup ⇒ watertight); each triangle is oriented outward (⇒ consistently oriented).
#[allow(clippy::too_many_arguments)]
fn emit_tet(
    pts: [[f64; 3]; 4],
    vals: [f64; 4],
    gids: [u64; 4],
    iso: f64,
    positions: &mut Vec<[f64; 3]>,
    triangles: &mut Vec<[u32; 3]>,
    vmap: &mut HashMap<(u64, u64), u32>,
) {
    // Inside iff strictly below the isolevel.
    let inside = [vals[0] < iso, vals[1] < iso, vals[2] < iso, vals[3] < iso];
    let n_in = inside.iter().filter(|&&b| b).count();
    if n_in == 0 || n_in == 4 {
        return; // fully inside or outside — no surface crosses this tet
    }

    // Outward direction: from the inside centroid toward the outside centroid.
    let mut in_c = [0.0; 3];
    let mut out_c = [0.0; 3];
    let (mut ni, mut no) = (0.0, 0.0);
    for c in 0..4 {
        if inside[c] {
            for k in 0..3 {
                in_c[k] += pts[c][k];
            }
            ni += 1.0;
        } else {
            for k in 0..3 {
                out_c[k] += pts[c][k];
            }
            no += 1.0;
        }
    }
    let out_dir = [
        out_c[0] / no - in_c[0] / ni,
        out_c[1] / no - in_c[1] / ni,
        out_c[2] / no - in_c[2] / ni,
    ];

    // Interpolate + dedup the crossing vertex on tet edge (a,b) where a is inside, b is outside.
    let mut vertex_on = |a: usize, b: usize, positions: &mut Vec<[f64; 3]>| -> u32 {
        let key = if gids[a] < gids[b] {
            (gids[a], gids[b])
        } else {
            (gids[b], gids[a])
        };
        if let Some(&idx) = vmap.get(&key) {
            return idx;
        }
        let (va, vb) = (vals[a], vals[b]);
        // t along a→b where the field crosses iso; guaranteed va != vb (strict sign difference).
        let t = (iso - va) / (vb - va);
        let p = [
            pts[a][0] + t * (pts[b][0] - pts[a][0]),
            pts[a][1] + t * (pts[b][1] - pts[a][1]),
            pts[a][2] + t * (pts[b][2] - pts[a][2]),
        ];
        // Bounded by the grid ((res+1)^3 samples ≪ u32::MAX for any grid that fits in memory); a saturating
        // fallback keeps this a never-panic path rather than an assertion.
        let idx = u32::try_from(positions.len()).unwrap_or(u32::MAX);
        positions.push(p);
        vmap.insert(key, idx);
        idx
    };

    let ins: Vec<usize> = (0..4).filter(|&c| inside[c]).collect();
    let outs: Vec<usize> = (0..4).filter(|&c| !inside[c]).collect();

    match n_in {
        1 => {
            let s = ins[0];
            let v0 = vertex_on(s, outs[0], positions);
            let v1 = vertex_on(s, outs[1], positions);
            let v2 = vertex_on(s, outs[2], positions);
            push_oriented(positions, triangles, [v0, v1, v2], out_dir);
        }
        3 => {
            let s = outs[0];
            let v0 = vertex_on(ins[0], s, positions);
            let v1 = vertex_on(ins[1], s, positions);
            let v2 = vertex_on(ins[2], s, positions);
            push_oriented(positions, triangles, [v0, v1, v2], out_dir);
        }
        2 => {
            // Two inside {i0,i1}, two outside {o0,o1}: the four crossing edges form a quad; split in two.
            let (i0, i1) = (ins[0], ins[1]);
            let (o0, o1) = (outs[0], outs[1]);
            let a = vertex_on(i0, o0, positions);
            let b = vertex_on(i0, o1, positions);
            let c = vertex_on(i1, o1, positions);
            let d = vertex_on(i1, o0, positions);
            push_oriented(positions, triangles, [a, b, c], out_dir);
            push_oriented(positions, triangles, [a, c, d], out_dir);
        }
        _ => unreachable!("n_in is 1, 2 or 3 here"),
    }
}

/// Push a triangle oriented so its geometric normal points along `out_dir` (outward). A zero-area triangle
/// (a sample landing on the surface) is dropped — the always-on validator would flag it, but the grid is
/// chosen so this does not happen; the guard keeps a pathological input from emitting a sliver.
fn push_oriented(
    positions: &[[f64; 3]],
    triangles: &mut Vec<[u32; 3]>,
    tri: [u32; 3],
    out_dir: [f64; 3],
) {
    let p = |idx: u32| positions[idx as usize];
    let (a, b, c) = (p(tri[0]), p(tri[1]), p(tri[2]));
    let ab = sub(b, a);
    let ac = sub(c, a);
    let nrm = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let nlen2 = nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2];
    if nlen2 == 0.0 {
        return; // degenerate sliver — skip (grid is chosen to avoid on-surface samples)
    }
    let dot = nrm[0] * out_dir[0] + nrm[1] * out_dir[1] + nrm[2] * out_dir[2];
    if dot >= 0.0 {
        triangles.push(tri);
    } else {
        triangles.push([tri[0], tri[2], tri[1]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_csg::{box_mesh, BoolOp, Csg, ExactBspCsg};

    const RUNS: usize = 3; // the ≥2-runs reproducibility discipline (<benchmark_discipline>)

    #[test]
    fn primitive_signs_are_correct() {
        let s = Sdf::sphere([0.0, 0.0, 0.0], 1.0);
        assert!(s.eval([0.0, 0.0, 0.0]) < 0.0, "centre is inside");
        assert!((s.eval([1.0, 0.0, 0.0])).abs() < 1e-12, "on the surface");
        assert!(s.eval([2.0, 0.0, 0.0]) > 0.0, "outside");

        let b = Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        assert!(b.eval([0.0, 0.0, 0.0]) < 0.0);
        assert!(b.eval([2.0, 0.0, 0.0]) > 0.0);
        assert!((b.eval([1.0, 0.0, 0.0])).abs() < 1e-12);

        let c = Sdf::cylinder([0.0, 0.0, 0.0], 0.5, 1.0, Axis::Y);
        assert!(c.eval([0.0, 0.0, 0.0]) < 0.0);
        assert!(c.eval([1.0, 0.0, 0.0]) > 0.0, "outside the radius");
        assert!(c.eval([0.0, 2.0, 0.0]) > 0.0, "past the cap");
    }

    #[test]
    fn csg_combinators_have_the_right_field_semantics() {
        let left = Sdf::sphere([-0.5, 0.0, 0.0], 1.0);
        let right = Sdf::sphere([0.5, 0.0, 0.0], 1.0);
        // union: inside either
        let uni = left.clone().union(right.clone());
        assert!(uni.eval([-1.4, 0.0, 0.0]) < 0.0 && uni.eval([1.4, 0.0, 0.0]) < 0.0);
        // intersection: inside both (the lens near the origin)
        let inter = left.clone().intersection(right.clone());
        assert!(inter.eval([0.0, 0.0, 0.0]) < 0.0);
        assert!(
            inter.eval([-1.4, 0.0, 0.0]) > 0.0,
            "not in the right sphere"
        );
        // difference: in A but not B
        let diff = left.difference(right);
        assert!(diff.eval([-1.2, 0.0, 0.0]) < 0.0, "in A, clear of B");
        assert!(diff.eval([0.9, 0.0, 0.0]) > 0.0, "carved away by B");
    }

    #[test]
    fn a_single_primitive_compiles_watertight() {
        let s = Sdf::sphere([0.0, 0.0, 0.0], 1.0);
        let grid = Grid::around(&s, 24, 0.15);
        let mesh = compile(&s, &grid);
        let r = validate(&mesh);
        assert!(
            r.is_clean(),
            "sphere compiles to a clean solid: {}",
            r.explain()
        );
        assert_eq!(r.genus, Some(0), "a sphere is genus 0");
    }

    #[test]
    fn the_spike_box_minus_cylinder_is_watertight_manifold_and_deterministic() {
        // The canonical Leg-B op: a box with a cylindrical bore carved out.
        let sdf = Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).difference(Sdf::cylinder(
            [0.0, 0.0, 0.0],
            0.5,
            2.0,
            Axis::Y,
        ));
        let grid = Grid::around(&sdf, 48, 0.06);
        let mesh = compile(&sdf, &grid);
        let r = validate(&mesh);
        assert!(
            r.watertight && r.manifold && r.oriented,
            "box−cylinder is watertight+manifold+oriented: {}",
            r.explain()
        );
        assert!(
            r.issues.is_empty(),
            "no non-degenerate issues: {}",
            r.explain()
        );
        assert_eq!(r.genus, Some(1), "a bored box is genus 1 (one tunnel)");
        assert!(mesh.triangle_count() > 100, "a real surface, not a stub");

        // Bit-deterministic across ≥2 runs (a single match is not proof).
        assert!(
            compile_reproduces(&sdf, &grid, RUNS),
            "the SDF→mesh compile is bit-deterministic across {RUNS} runs"
        );
    }

    #[test]
    fn sdf_and_exact_mesh_csg_agree_on_watertightness() {
        // Coherence with the M13.2 exact-predicate MESH path (ADR-051): the same box−box difference is
        // watertight both as a compiled SDF field AND as an exact-predicate BSP boolean — the field is the
        // resolution-independent canonical rep, the BSP is the exact-mesh op; both are crack-free.
        let sdf = Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 0.5])
            .difference(Sdf::cuboid([0.0, 0.5, 0.0], [0.5, 0.5, 1.0]));
        let field_mesh = compile(&sdf, &Grid::around(&sdf, 40, 0.06));
        assert!(
            validate(&field_mesh).watertight,
            "the SDF field path is clean"
        );

        let bsp = ExactBspCsg::default()
            .boolean(
                &box_mesh([0.0, 0.0, 0.0], [1.0, 1.0, 0.5]),
                &box_mesh([0.0, 0.5, 0.0], [0.5, 0.5, 1.0]),
                BoolOp::Difference,
            )
            .expect("exact-predicate BSP difference is clean");
        assert!(
            validate(&bsp).watertight,
            "the M13.2 exact-mesh path is clean"
        );
    }

    #[test]
    fn the_chord_tolerance_budget_shrinks_with_resolution() {
        // SDF→mesh is an APPROXIMATION for curved faces: the declared budget is the cell edge, and it
        // halves as resolution doubles (never claimed lossless — §4).
        let s = Sdf::sphere([0.0, 0.0, 0.0], 1.0);
        let coarse = Grid::around(&s, 20, 0.1).chord_tolerance();
        let fine = Grid::around(&s, 40, 0.1).chord_tolerance();
        assert!(
            fine < coarse * 0.6,
            "doubling res roughly halves the budget"
        );
    }
}
