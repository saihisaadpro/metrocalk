//! Auto **skin weights** (M9.5 / G5 deliverable 2): given a mesh + a bone skeleton, compute per-vertex
//! `JOINTS_0`/`WEIGHTS_0` so a posed bone deforms the surface **smoothly** through G3's LBS (a lighter,
//! dirty-mesh-robust alternative to BBW, which is precompute-heavy and has no Rust impl).
//!
//! **Formula honesty (the ADR-029 audit).** The research report cited a specific Robust-Skin-Weights
//! "`Q = -L + L M⁻¹ L`" form and flagged it **inferred, not confirmed**. Rather than commit to an
//! unverified formula, we implement the **established biharmonic weight-inpainting** energy that
//! Robust-Skin-Weights' inpainting step (and libigl's biharmonic weights) is built on — multi-source,
//! primary-doc confirmed: minimise the squared Laplacian `‖Δw‖²_M`, i.e. `wᵀ Q w` with the **biharmonic
//! operator** `Q = L M⁻¹ L` (`L` = the cotangent stiffness Laplacian, `M` = the diagonal mass matrix).
//! Bone "handle" vertices are Dirichlet seeds; the smooth blend in between is the solve. This is the
//! verified core; the report's `-L` regulariser is deliberately **not** adopted.
//!
//! Determinism: same primitives as ARAP — sequential dense Cholesky, fixed pipeline, `f64`. One
//! factorization of `Q_ff`, then **one solve per bone** (shared system matrix) → the weight fields.

use crate::arap::{cotangent_adjacency, DeformMesh};
use crate::linalg::{add, cholesky, cholesky_solve, dot, norm, scale, sub, Vec3};

/// How to finalize the per-vertex weights for LBS.
#[derive(Clone, Copy, Debug)]
pub struct SkinWeightConfig {
    /// Max influences per vertex (glTF/LBS convention is 4). The top-`k` bones are kept + renormalized.
    pub max_influences: usize,
}

impl Default for SkinWeightConfig {
    fn default() -> Self {
        Self { max_influences: 4 }
    }
}

/// Per-vertex skin binding parallel to the mesh's vertices — exactly the `JOINTS_0`/`WEIGHTS_0` that
/// [`metrocalk_skeleton::skin_position`] (G3 LBS) consumes. `joints[v]` indexes the bone list passed to
/// [`auto_skin_weights`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SkinBinding {
    /// Up to-4 bone indices per vertex (zero-padded; unused slots carry weight 0).
    pub joints: Vec<[u16; 4]>,
    /// Weights parallel to [`Self::joints`], normalized to sum 1 (partition of unity).
    pub weights: Vec<[f32; 4]>,
}

/// A vertex is a **confident seed** for its nearest bone when that bone is clearly nearest — its distance
/// is under this fraction of the second-nearest bone's. Vertices in the "blend band" between bones (ratio
/// near 1) are left **free** and solved smoothly. (A single fixed constant — no per-mesh tuning.)
const SEED_RATIO: f64 = 0.5;

/// Compute auto skin weights: seed the bones' confidently-owned vertices, **biharmonic-inpaint** the
/// smooth blend everywhere else, normalize to a partition of unity, and keep the top-`max_influences`
/// bones per vertex. `bones` are bind-pose segments `(start, end)` in mesh space (derive them from the
/// skeleton's bind globals). Returns `None` if the inpainting system is degenerate.
#[must_use]
#[allow(
    clippy::too_many_lines, // the seed→assemble→solve→normalize pipeline reads best as one function
    clippy::cast_possible_truncation // LBS weights are f32 by contract — the f64→f32 narrowing is intended
)]
pub fn auto_skin_weights(
    mesh: &DeformMesh,
    bones: &[(Vec3, Vec3)],
    config: SkinWeightConfig,
) -> Option<SkinBinding> {
    let n = mesh.positions.len();
    let nb = bones.len();
    if n == 0 || nb == 0 {
        return None;
    }

    // ── Seeding: nearest-bone assignment + the confident-seed test ────────────────────────────────
    let mut nearest = vec![0usize; n];
    let mut margin = vec![0.0f64; n]; // (second-nearest − nearest) bone distance = confidence
    let mut is_seed = vec![false; n];
    for (v, &p) in mesh.positions.iter().enumerate() {
        let mut d1 = f64::INFINITY;
        let mut d2 = f64::INFINITY;
        let mut b1 = 0usize;
        for (b, &(a, c)) in bones.iter().enumerate() {
            let d = dist_point_segment(p, a, c);
            if d < d1 {
                d2 = d1;
                d1 = d;
                b1 = b;
            } else if d < d2 {
                d2 = d;
            }
        }
        nearest[v] = b1;
        margin[v] = d2 - d1;
        // Confident seed: this bone is CLEARLY nearest. Junction vertices (d1 ≈ d2) stay free → they're
        // the blend band the inpainting solves smoothly.
        if nb == 1 || d1 < SEED_RATIO * d2 {
            is_seed[v] = true;
        }
    }
    // Guarantee each bone has ≥1 seed — using its MOST confident owned vertex (never an ambiguous
    // junction vertex), so a bone with only blend-band vertices still anchors its field.
    let mut bone_has_seed = vec![false; nb];
    for v in 0..n {
        if is_seed[v] {
            bone_has_seed[nearest[v]] = true;
        }
    }
    for b in 0..nb {
        if bone_has_seed[b] {
            continue;
        }
        let mut best_v = usize::MAX;
        let mut best_margin = f64::NEG_INFINITY;
        for v in 0..n {
            if nearest[v] == b && margin[v] > best_margin {
                best_margin = margin[v];
                best_v = v;
            }
        }
        if best_v != usize::MAX {
            is_seed[best_v] = true;
        }
    }

    // ── Build L (cotangent stiffness) and M (barycentric mass) ────────────────────────────────────
    let adj = cotangent_adjacency(&mesh.positions, &mesh.triangles, n);
    let mass = barycentric_mass(&mesh.positions, &mesh.triangles, n);
    // Dense L (row-major). L_ii = Σ w; L_ij = -w.
    let mut lmat = vec![0.0f64; n * n];
    for (i, list) in adj.iter().enumerate() {
        let mut diag = 0.0;
        for &(j, w) in list {
            diag += w;
            lmat[i * n + j] -= w;
        }
        lmat[i * n + i] += diag;
    }
    // Biharmonic operator Q = L M⁻¹ L (dense). minv applied between the two L factors.
    let minv: Vec<f64> = mass.iter().map(|&m| 1.0 / m.max(1e-12)).collect();
    let mut q = vec![0.0f64; n * n];
    for i in 0..n {
        for k in 0..n {
            let mut acc = 0.0;
            for j in 0..n {
                acc += lmat[i * n + j] * minv[j] * lmat[j * n + k];
            }
            q[i * n + k] = acc;
        }
    }

    // ── Partition free/constrained, factor Q_ff once ──────────────────────────────────────────────
    let free: Vec<usize> = (0..n).filter(|&i| !is_seed[i]).collect();
    let mut free_of = vec![usize::MAX; n];
    for (fi, &i) in free.iter().enumerate() {
        free_of[i] = fi;
    }
    let nf = free.len();

    // Per-vertex per-bone weight field (row = vertex, col = bone).
    let mut field = vec![0.0f64; n * nb];
    // Seeds: 1 for their owned bone, 0 elsewhere.
    for v in 0..n {
        if is_seed[v] {
            field[v * nb + nearest[v]] = 1.0;
        }
    }

    if nf > 0 {
        let mut a = vec![0.0f64; nf * nf];
        for (fi, &i) in free.iter().enumerate() {
            for (fj, &j) in free.iter().enumerate() {
                a[fi * nf + fj] = q[i * n + j];
            }
            a[fi * nf + fi] += 1e-9; // PD regularizer (deterministic)
        }
        let chol = cholesky(&a, nf)?;
        // One solve per bone: Q_ff x = -Q_fc · w_c (the seed boundary values for bone b).
        for b in 0..nb {
            let mut rhs = vec![0.0f64; nf];
            for (fi, &i) in free.iter().enumerate() {
                let mut acc = 0.0;
                for v in 0..n {
                    if is_seed[v] {
                        acc += q[i * n + v] * field[v * nb + b];
                    }
                }
                rhs[fi] = -acc;
            }
            cholesky_solve(&chol, nf, &mut rhs);
            for (fi, &i) in free.iter().enumerate() {
                field[i * nb + b] = rhs[fi];
            }
        }
    }

    // ── Normalize (partition of unity) + keep top-k influences ────────────────────────────────────
    let k = config.max_influences.clamp(1, 4);
    let mut binding = SkinBinding {
        joints: vec![[0u16; 4]; n],
        weights: vec![[0.0f32; 4]; n],
    };
    for v in 0..n {
        // Clamp negatives (biharmonic can overshoot slightly) and rank bones by weight.
        let mut bw: Vec<(usize, f64)> = (0..nb).map(|b| (b, field[v * nb + b].max(0.0))).collect();
        bw.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        let top = &bw[..k.min(nb)];
        let sum: f64 = top.iter().map(|&(_, w)| w).sum();
        for (slot, &(b, w)) in top.iter().enumerate() {
            binding.joints[v][slot] = u16::try_from(b).unwrap_or(0);
            binding.weights[v][slot] = if sum > 1e-12 { (w / sum) as f32 } else { 0.0 };
        }
        // Degenerate (all-zero) → fully weight the nearest bone so LBS stays well-defined.
        if sum <= 1e-12 {
            binding.joints[v] = [u16::try_from(nearest[v]).unwrap_or(0), 0, 0, 0];
            binding.weights[v] = [1.0, 0.0, 0.0, 0.0];
        }
    }
    Some(binding)
}

/// Distance from point `p` to segment `[a, b]`.
#[must_use]
fn dist_point_segment(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let ab = sub(b, a);
    let len2 = dot(ab, ab);
    if len2 < 1e-18 {
        return norm(sub(p, a));
    }
    let t = (dot(sub(p, a), ab) / len2).clamp(0.0, 1.0);
    norm(sub(p, add(a, scale(ab, t))))
}

/// Barycentric (one-third-incident-triangle-area) vertex mass — the diagonal mass matrix `M`. Guards a
/// zero-area vertex with a tiny floor so `M⁻¹` is finite.
#[must_use]
fn barycentric_mass(pos: &[Vec3], tris: &[[u32; 3]], n: usize) -> Vec<f64> {
    let mut m = vec![0.0f64; n];
    for t in tris {
        let a = t[0] as usize;
        let b = t[1] as usize;
        let c = t[2] as usize;
        if a >= n || b >= n || c >= n {
            continue;
        }
        let ab = sub(pos[b], pos[a]);
        let ac = sub(pos[c], pos[a]);
        let cx = [
            ab[1] * ac[2] - ab[2] * ac[1],
            ab[2] * ac[0] - ab[0] * ac[2],
            ab[0] * ac[1] - ab[1] * ac[0],
        ];
        let area = 0.5 * (cx[0] * cx[0] + cx[1] * cx[1] + cx[2] * cx[2]).sqrt();
        let third = area / 3.0;
        m[a] += third;
        m[b] += third;
        m[c] += third;
    }
    for mi in &mut m {
        if *mi < 1e-12 {
            *mi = 1e-12;
        }
    }
    m
}
