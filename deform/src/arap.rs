//! As-Rigid-As-Possible (Sorkine & Alexa 2007) handle deformation, **reimplemented** (not `baby_shark`)
//! so the determinism is ours: one **one-time precompute** (build the cotangent Laplacian over the
//! region, partition free/constrained, factor the free block once with the sequential dense Cholesky in
//! [`crate::linalg`]), then **cheap per-frame** local/global iterations — exactly B.3's "front-load a
//! one-time precompute, deform cheaply per frame." The **region-of-interest is the cost knob** (ARAP
//! degrades on dense full meshes → restrict it); the precompute is one-shot, off the hot path.

use crate::linalg::{
    add, add_outer_scaled, best_rotation, cholesky, cholesky_solve, mat_vec, scale, sub, Mat3,
    Vec3, I3,
};
use std::collections::HashMap;

/// A mesh region to deform: rest-pose vertex positions + a triangle list (indices into `positions`).
/// For G5 this is a **localized region of interest** (a leg, an arm) — not a whole dense character —
/// because ARAP's cost is in the region size (the documented knob).
#[derive(Clone, Debug, Default)]
pub struct DeformMesh {
    /// Rest-pose positions (`f64` — the deterministic path).
    pub positions: Vec<Vec3>,
    /// Triangle-list indices into [`Self::positions`].
    pub triangles: Vec<[u32; 3]>,
}

/// The constraint partition over a [`DeformMesh`]: which vertices the user drags (`handles`, moved to
/// targets each frame) and which are pinned (`anchors`, e.g. the region boundary that stitches the
/// deformed region back to the static mesh). Every other vertex is **free** (solved by ARAP).
#[derive(Clone, Debug, Default)]
pub struct Region {
    /// Driven vertices — their order is the order of the `handle_targets` slice passed to
    /// [`Deformer::deform`].
    pub handles: Vec<u32>,
    /// Pinned vertices (Dirichlet at their rest position) — typically the region boundary.
    pub anchors: Vec<u32>,
}

/// ARAP iteration budget. A few local/global iterations is the interactive sweet spot; **fixed** (not
/// convergence-thresholded) so the result is deterministic regardless of the input scale.
#[derive(Clone, Copy, Debug)]
pub struct ArapConfig {
    /// Local/global iterations per deform. 4 is the default interactive count.
    pub iters: usize,
}

impl Default for ArapConfig {
    fn default() -> Self {
        Self { iters: 4 }
    }
}

/// The deformation contract (invariant 5 — project-owned, so ARAP can be swapped for a future cage
/// deformer behind the same per-frame interface). The precompute is implementation-specific; the shared
/// contract is the cheap per-frame `deform`.
pub trait Deformer {
    /// Deform from the current handle target positions → new positions for **all** mesh vertices
    /// (free vertices solved; anchors stay at rest; handles land exactly on their targets).
    /// `handle_targets` is parallel to [`Region::handles`].
    fn deform(&self, handle_targets: &[Vec3]) -> Vec<Vec3>;
    /// The mesh vertex count (length of [`Self::deform`]'s output).
    fn vertex_count(&self) -> usize;
    /// The number of handles (expected length of `handle_targets`).
    fn handle_count(&self) -> usize;
}

/// A prepared ARAP deformer: the precompute is baked in (`neighbors`/weights, the free/constrained
/// partition, and the dense Cholesky factor of the free–free Laplacian block). `deform` is then cheap.
pub struct ArapDeformer {
    rest: Vec<Vec3>,
    /// Per-vertex cotangent-weighted adjacency `(neighbor, wᵢⱼ)`, sorted by neighbor index
    /// (deterministic accumulation + iteration order).
    neighbors: Vec<Vec<(usize, f64)>>,
    /// `true` for handles + anchors.
    constrained: Vec<bool>,
    /// Free vertex indices, ascending.
    free: Vec<usize>,
    /// Handle vertices, in `Region::handles` order (parallel to the `deform` input).
    handles: Vec<usize>,
    /// Anchor vertices.
    anchors: Vec<usize>,
    /// Lower Cholesky factor of the free–free block (`nfree × nfree`, row-major). Empty if `nfree == 0`.
    chol: Vec<f64>,
    nfree: usize,
    iters: usize,
}

impl ArapDeformer {
    /// **Precompute** (one-shot, off the hot path): build the cotangent Laplacian, partition the
    /// vertices, and factor the free block once. Returns `None` only if the free–free system is not
    /// positive-definite (a pathological region) — the caller falls back to the rigid/handle placement.
    #[must_use]
    pub fn prepare(mesh: &DeformMesh, region: &Region, config: ArapConfig) -> Option<Self> {
        let n = mesh.positions.len();
        let neighbors = cotangent_adjacency(&mesh.positions, &mesh.triangles, n);

        let mut constrained = vec![false; n];
        for &h in &region.handles {
            if (h as usize) < n {
                constrained[h as usize] = true;
            }
        }
        for &a in &region.anchors {
            if (a as usize) < n {
                constrained[a as usize] = true;
            }
        }
        let free: Vec<usize> = (0..n).filter(|&i| !constrained[i]).collect();
        let mut free_of = vec![usize::MAX; n];
        for (fi, &i) in free.iter().enumerate() {
            free_of[i] = fi;
        }
        let nfree = free.len();

        // Build the dense free–free Laplacian block L_ff and factor it once.
        let chol = if nfree == 0 {
            Vec::new()
        } else {
            let mut a = vec![0.0f64; nfree * nfree];
            for (fi, &i) in free.iter().enumerate() {
                let mut diag = 0.0;
                for &(j, w) in &neighbors[i] {
                    diag += w; // the full diagonal includes constrained neighbors
                    let fj = free_of[j];
                    if fj != usize::MAX {
                        a[fi * nfree + fj] -= w;
                    }
                }
                // A tiny PD regularizer keeps the factorization robust for a fully-interior free vertex
                // whose constrained coupling is weak — deterministic (a fixed constant).
                a[fi * nfree + fi] += diag + 1e-9;
            }
            cholesky(&a, nfree)?
        };

        let handles: Vec<usize> = region.handles.iter().map(|&h| h as usize).collect();
        let anchors: Vec<usize> = region.anchors.iter().map(|&x| x as usize).collect();
        Some(Self {
            rest: mesh.positions.clone(),
            neighbors,
            constrained,
            free,
            handles,
            anchors,
            chol,
            nfree,
            iters: config.iters.max(1),
        })
    }

    /// The per-vertex rotation (ARAP local step) for the current deformed positions `p`.
    fn rotations(&self, p: &[Vec3]) -> Vec<Mat3> {
        let mut rs = vec![I3; self.rest.len()];
        for (i, ri) in rs.iter_mut().enumerate() {
            let mut s: Mat3 = [[0.0; 3]; 3];
            for &(j, w) in &self.neighbors[i] {
                let e = sub(self.rest[i], self.rest[j]); // rest edge
                let ep = sub(p[i], p[j]); // deformed edge
                add_outer_scaled(&mut s, e, ep, w);
            }
            *ri = best_rotation(&s);
        }
        rs
    }
}

impl Deformer for ArapDeformer {
    fn deform(&self, handle_targets: &[Vec3]) -> Vec<Vec3> {
        // Initial guess: rest everywhere, with constrained vertices placed at their targets.
        let mut p = self.rest.clone();
        for (k, &h) in self.handles.iter().enumerate() {
            if let Some(&t) = handle_targets.get(k) {
                p[h] = t;
            }
        }
        for &a in &self.anchors {
            p[a] = self.rest[a];
        }
        if self.nfree == 0 {
            return p; // everything constrained — placement is the answer
        }

        for _ in 0..self.iters {
            // Local step: best-fit rotation per vertex.
            let rs = self.rotations(&p);
            // Global step: assemble the RHS for the free rows and solve L_ff · p'_f = rhs.
            let mut rhs = vec![0.0f64; self.nfree * 3];
            for (fi, &i) in self.free.iter().enumerate() {
                let mut b: Vec3 = [0.0; 3];
                for &(j, w) in &self.neighbors[i] {
                    // ARAP RHS term: (wᵢⱼ/2)(Rᵢ + Rⱼ)(restᵢ − restⱼ).
                    let e = sub(self.rest[i], self.rest[j]);
                    let rsum = add(mat_vec(&rs[i], e), mat_vec(&rs[j], e));
                    b = add(b, scale(rsum, w * 0.5));
                    // Move constrained neighbors to the RHS: + wᵢⱼ · p'_j.
                    if self.constrained[j] {
                        b = add(b, scale(p[j], w));
                    }
                }
                rhs[fi] = b[0];
                rhs[self.nfree + fi] = b[1];
                rhs[2 * self.nfree + fi] = b[2];
            }
            // One factor, three solves (x, y, z share the system matrix).
            for axis in 0..3 {
                let slice = &mut rhs[axis * self.nfree..(axis + 1) * self.nfree];
                cholesky_solve(&self.chol, self.nfree, slice);
            }
            for (fi, &i) in self.free.iter().enumerate() {
                p[i] = [rhs[fi], rhs[self.nfree + fi], rhs[2 * self.nfree + fi]];
            }
        }
        p
    }

    fn vertex_count(&self) -> usize {
        self.rest.len()
    }

    fn handle_count(&self) -> usize {
        self.handles.len()
    }
}

/// Build the **cotangent-weighted** vertex adjacency from a triangle mesh. Each triangle contributes
/// `½·cot(angle)` to the weight of the edge **opposite** that angle. Cotangents are **clamped to ≥ 0**
/// (the "intrinsic/clamped" Laplacian) so the operator stays a valid PSD M-matrix even on obtuse/sliver
/// triangles — which (with the Dirichlet anchors) guarantees the free block is positive-definite for the
/// dense Cholesky. Accumulation is in fixed triangle order and each adjacency list is sorted →
/// deterministic.
#[must_use]
pub(crate) fn cotangent_adjacency(
    pos: &[Vec3],
    tris: &[[u32; 3]],
    n: usize,
) -> Vec<Vec<(usize, f64)>> {
    fn accum(w: &mut HashMap<(usize, usize), f64>, i: usize, j: usize, c: f64) {
        let key = if i < j { (i, j) } else { (j, i) };
        *w.entry(key).or_insert(0.0) += c;
    }
    let mut w: HashMap<(usize, usize), f64> = HashMap::new();
    for t in tris {
        let a = t[0] as usize;
        let b = t[1] as usize;
        let c = t[2] as usize;
        if a >= n || b >= n || c >= n {
            continue;
        }
        // Half-cotangent at each vertex → the opposite edge.
        let cot_a = half_cot(pos[a], pos[b], pos[c]);
        let cot_b = half_cot(pos[b], pos[c], pos[a]);
        let cot_c = half_cot(pos[c], pos[a], pos[b]);
        accum(&mut w, b, c, cot_a);
        accum(&mut w, c, a, cot_b);
        accum(&mut w, a, b, cot_c);
    }
    let mut adj: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];
    for (&(i, j), &weight) in &w {
        adj[i].push((j, weight));
        adj[j].push((i, weight));
    }
    for list in &mut adj {
        list.sort_by_key(|&(j, _)| j);
    }
    adj
}

/// `½·cot(θ)` where `θ` is the angle at `apex` between edges `apex→p` and `apex→q`. Clamped to `≥ 0`
/// (see [`cotangent_adjacency`]). Degenerate (zero-area) triangles contribute 0.
#[must_use]
fn half_cot(apex: Vec3, p: Vec3, q: Vec3) -> f64 {
    let u = sub(p, apex);
    let v = sub(q, apex);
    let dot = u[0] * v[0] + u[1] * v[1] + u[2] * v[2];
    let cx = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let sin_area = (cx[0] * cx[0] + cx[1] * cx[1] + cx[2] * cx[2]).sqrt();
    if sin_area < 1e-12 {
        return 0.0;
    }
    (0.5 * dot / sin_area).max(0.0)
}
