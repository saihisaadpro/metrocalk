//! Deterministic dense linear algebra for the ARAP solver — **the determinism gate lives here** (the
//! adversarial worry is "a `rayon` reduction order makes the deform non-deterministic"). Everything in
//! this module is **sequential, fixed-iteration, and `f64`** (the M8.1 / ADR-020 cross-ISA path): a
//! symmetric-`3×3` Jacobi eigensolver with a **fixed sweep count**, a `best_rotation` (the ARAP local
//! step) with a **pinned reflection-fix sign convention**, and a **sequential dense Cholesky** (no
//! threads, so no reduction-order nondeterminism). Same inputs → bit-identical outputs.
//!
//! Math is plain `f64` arrays (no foreign matrix type leaks — invariant 5): a `Mat3` is row-major
//! `m[row][col]`, a `Vec3` is `[f64; 3]`.

/// Row-major `3×3`, `m[row][col]`.
pub type Mat3 = [[f64; 3]; 3];
/// A 3-vector.
pub type Vec3 = [f64; 3];

/// The `3×3` identity.
pub const I3: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Jacobi sweeps for the symmetric `3×3` eigensolver. A `3×3` converges to `f64` precision in ~5–6
/// cyclic sweeps; **12 is a fixed, generous, deterministic cap** (never data-dependent — that would be a
/// determinism hole).
const JACOBI_SWEEPS: usize = 12;

/// Singular values below this (relative work scale) are treated as zero — the rank-deficient guard in
/// [`best_rotation`] (a degenerate vertex with collinear/zero edges).
const SVD_EPS: f64 = 1e-12;

#[must_use]
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[must_use]
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
#[must_use]
pub fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
#[must_use]
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
#[must_use]
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
#[must_use]
pub fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}
#[must_use]
pub fn normalize_or(a: Vec3, fallback: Vec3) -> Vec3 {
    let n = norm(a);
    if n < SVD_EPS {
        fallback
    } else {
        scale(a, 1.0 / n)
    }
}

/// `m · v` (row-major).
#[must_use]
pub fn mat_vec(m: &Mat3, v: Vec3) -> Vec3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

#[must_use]
pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for (r, orow) in o.iter_mut().enumerate() {
        for (c, ocell) in orow.iter_mut().enumerate() {
            *ocell = a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c];
        }
    }
    o
}

#[must_use]
pub fn transpose(a: &Mat3) -> Mat3 {
    [
        [a[0][0], a[1][0], a[2][0]],
        [a[0][1], a[1][1], a[2][1]],
        [a[0][2], a[1][2], a[2][2]],
    ]
}

#[must_use]
pub fn det(a: &Mat3) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

/// The outer product `a · bᵀ` accumulated, scaled by `w` — one term of the ARAP covariance `S`.
pub fn add_outer_scaled(s: &mut Mat3, a: Vec3, b: Vec3, w: f64) {
    for (r, srow) in s.iter_mut().enumerate() {
        for (c, scell) in srow.iter_mut().enumerate() {
            *scell += w * a[r] * b[c];
        }
    }
}

#[must_use]
fn col(m: &Mat3, c: usize) -> Vec3 {
    [m[0][c], m[1][c], m[2][c]]
}

fn set_col(m: &mut Mat3, c: usize, v: Vec3) {
    m[0][c] = v[0];
    m[1][c] = v[1];
    m[2][c] = v[2];
}

/// Symmetric `3×3` eigendecomposition by **cyclic Jacobi** with a fixed sweep count → deterministic.
/// Returns `(eigenvalues, V)` with **eigenvalues sorted descending** (index tiebreak stable) and `V`'s
/// **columns** the matching eigenvectors, so `a == V · diag(eval) · Vᵀ`.
#[must_use]
pub fn jacobi_eigen_sym(a_in: &Mat3) -> (Vec3, Mat3) {
    let mut a = *a_in;
    let mut v = I3;
    for _ in 0..JACOBI_SWEEPS {
        for &(p, q) in &[(0usize, 1usize), (0, 2), (1, 2)] {
            let apq = a[p][q];
            if apq.abs() < 1e-300 {
                continue;
            }
            // Stable Jacobi rotation (Numerical Recipes): t = sign(θ)/(|θ| + √(θ²+1)).
            let theta = (a[q][q] - a[p][p]) / (2.0 * apq);
            let t = if theta == 0.0 {
                1.0
            } else {
                theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt())
            };
            let c = 1.0 / (t * t + 1.0).sqrt();
            let s = t * c;
            // Rotate A (symmetric update).
            let app = a[p][p];
            let aqq = a[q][q];
            a[p][p] = c * c * app - 2.0 * s * c * apq + s * s * aqq;
            a[q][q] = s * s * app + 2.0 * s * c * apq + c * c * aqq;
            a[p][q] = 0.0;
            a[q][p] = 0.0;
            for k in 0..3 {
                if k != p && k != q {
                    let akp = a[k][p];
                    let akq = a[k][q];
                    a[k][p] = c * akp - s * akq;
                    a[p][k] = a[k][p];
                    a[k][q] = s * akp + c * akq;
                    a[q][k] = a[k][q];
                }
            }
            // Accumulate the rotation into V.
            for vrow in &mut v {
                let vp = vrow[p];
                let vq = vrow[q];
                vrow[p] = c * vp - s * vq;
                vrow[q] = s * vp + c * vq;
            }
        }
    }
    let mut eval = [a[0][0], a[1][1], a[2][2]];
    // Sort eigenpairs descending (stable, index tiebreak) so the smallest-σ column is index 2 — the
    // pinned target of the reflection fix.
    let mut order = [0usize, 1, 2];
    order.sort_by(|&i, &j| {
        eval[j]
            .partial_cmp(&eval[i])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(i.cmp(&j))
    });
    let sorted_eval = [eval[order[0]], eval[order[1]], eval[order[2]]];
    let mut sorted_v = [[0.0; 3]; 3];
    for (new_c, &old_c) in order.iter().enumerate() {
        set_col(&mut sorted_v, new_c, col(&v, old_c));
    }
    eval = sorted_eval;
    (eval, sorted_v)
}

/// The ARAP **local step**: the rotation `R` that best aligns rest edges to deformed edges, i.e.
/// `argmax_R tr(R · S)` over proper rotations, where `S = Σ wᵢⱼ (restᵢ−restⱼ)(p'ᵢ−p'ⱼ)ᵀ`.
/// Computed as `R = V · Uᵀ` from the SVD `S = U Σ Vᵀ`, with the **reflection fix pinned**: if
/// `det(R) < 0`, negate the singular vector of the **smallest** singular value (index 2 after the
/// descending sort) — a fixed, deterministic choice (never "whichever the library picks").
#[must_use]
pub fn best_rotation(s: &Mat3) -> Mat3 {
    // SVD via the eigendecomposition of SᵀS = V Σ² Vᵀ.
    let ata = mat_mul(&transpose(s), s);
    let (eval, vmat) = jacobi_eigen_sym(&ata);
    let sigma = [
        eval[0].max(0.0).sqrt(),
        eval[1].max(0.0).sqrt(),
        eval[2].max(0.0).sqrt(),
    ];
    // Fully degenerate covariance (a vertex that barely constrains) → identity rotation (rigid).
    if sigma[0] < SVD_EPS {
        return I3;
    }
    let v0 = col(&vmat, 0);
    let v1 = col(&vmat, 1);
    // U columns: uᵢ = S vᵢ / σᵢ. Build an orthonormal U deterministically: u0 from σ0 (always valid
    // here), u1 Gram-Schmidt'd against u0, u2 = u0 × u1 (so its sign is what the reflection fix decides).
    let u0 = normalize_or(mat_vec(s, v0), [1.0, 0.0, 0.0]);
    let mut u1 = mat_vec(s, v1);
    if sigma[1] < SVD_EPS {
        // Rank-deficient: pick any vector orthogonal to u0 (deterministic).
        u1 = orthogonal_to(u0);
    }
    // Gram-Schmidt u1 against u0.
    u1 = sub(u1, scale(u0, dot(u1, u0)));
    let u1 = normalize_or(u1, orthogonal_to(u0));
    let u2 = cross(u0, u1);
    let mut u = I3;
    set_col(&mut u, 0, u0);
    set_col(&mut u, 1, u1);
    set_col(&mut u, 2, u2);

    let mut r = mat_mul(&vmat, &transpose(&u));
    if det(&r) < 0.0 {
        // Reflection → flip the smallest-σ singular vector (column 2) and recompute. Pinned convention.
        set_col(&mut u, 2, scale(u2, -1.0));
        r = mat_mul(&vmat, &transpose(&u));
    }
    r
}

/// Any unit vector orthogonal to `a` (deterministic — cross with whichever axis is least aligned).
#[must_use]
fn orthogonal_to(a: Vec3) -> Vec3 {
    let axis = if a[0].abs() <= a[1].abs() && a[0].abs() <= a[2].abs() {
        [1.0, 0.0, 0.0]
    } else if a[1].abs() <= a[2].abs() {
        [0.0, 1.0, 0.0]
    } else {
        [0.0, 0.0, 1.0]
    };
    normalize_or(cross(a, axis), [0.0, 1.0, 0.0])
}

/// A **sequential dense Cholesky** factorization `A = L Lᵀ` of a symmetric positive-definite `n×n`
/// matrix (row-major flattened). `None` if a non-positive pivot appears (not PD). No threads → the
/// reduction order is fixed → deterministic (the audited alternative to `baby_shark`'s `rayon` solve).
#[must_use]
pub fn cholesky(a: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut l = vec![0.0f64; n * n];
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= l[j * n + k] * l[j * n + k];
        }
        if d <= 0.0 {
            return None;
        }
        let ljj = d.sqrt();
        l[j * n + j] = ljj;
        for i in (j + 1)..n {
            let mut acc = a[i * n + j];
            for k in 0..j {
                acc -= l[i * n + k] * l[j * n + k];
            }
            l[i * n + j] = acc / ljj;
        }
    }
    Some(l)
}

/// Solve `L Lᵀ x = b` in place (`b` becomes `x`), given the lower factor from [`cholesky`]. Sequential
/// forward + back substitution → deterministic.
pub fn cholesky_solve(l: &[f64], n: usize, b: &mut [f64]) {
    // Forward: L y = b.
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= l[i * n + k] * b[k];
        }
        b[i] = s / l[i * n + i];
    }
    // Backward: Lᵀ x = y.
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in (i + 1)..n {
            s -= l[k * n + i] * b[k];
        }
        b[i] = s / l[i * n + i];
    }
}
