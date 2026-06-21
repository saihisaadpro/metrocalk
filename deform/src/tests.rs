//! `metrocalk-deform` headless spine (M9.5 / G5). Proves the determinism gate (same input → bit-identical
//! output), the ARAP correctness invariants (rigid motions reproduced exactly; handles land on targets;
//! anchors stay; a localized lift makes a smooth bump), the deterministic linear-algebra primitives
//! (Jacobi eigen, `best_rotation`, Cholesky), and the per-frame budget on a representative region.

#![allow(
    clippy::float_cmp,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::arap::{ArapConfig, ArapDeformer, DeformMesh, Deformer, Region};
use crate::linalg::{
    best_rotation, cholesky, cholesky_solve, jacobi_eigen_sym, mat_mul, mat_vec, sub, transpose,
    Mat3, Vec3,
};
use crate::skin_weights::{auto_skin_weights, SkinWeightConfig};

fn dist(a: Vec3, b: Vec3) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Rodrigues rotation about a unit `axis` by `angle` (row-major).
fn rot_axis(axis: Vec3, angle: f64) -> Mat3 {
    let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let [x, y, z] = [axis[0] / n, axis[1] / n, axis[2] / n];
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// An `nx × ny` grid in the XY plane with a small deterministic z-bump (keeps per-vertex covariances
/// full-rank so the rigid-rotation test isn't a planar-degenerate special case). Returns the mesh + the
/// boundary vertex indices + an interior vertex index near the center.
fn grid(nx: usize, ny: usize) -> (DeformMesh, Vec<u32>, u32) {
    let idx = |i: usize, j: usize| (j * nx + i) as u32;
    let mut positions = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let z = 0.05 * (((i * 3 + j * 5) % 7) as f64);
            positions.push([i as f64, j as f64, z]);
        }
    }
    let mut triangles = Vec::new();
    for j in 0..ny - 1 {
        for i in 0..nx - 1 {
            triangles.push([idx(i, j), idx(i + 1, j), idx(i + 1, j + 1)]);
            triangles.push([idx(i, j), idx(i + 1, j + 1), idx(i, j + 1)]);
        }
    }
    let mut boundary = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            if i == 0 || j == 0 || i == nx - 1 || j == ny - 1 {
                boundary.push(idx(i, j));
            }
        }
    }
    let center = idx(nx / 2, ny / 2);
    (
        DeformMesh {
            positions,
            triangles,
        },
        boundary,
        center,
    )
}

// ── deterministic linear-algebra primitives ──────────────────────────────────

#[test]
fn jacobi_eigen_reconstructs_a_symmetric_matrix() {
    // A symmetric matrix → V diag(λ) Vᵀ recovers it, V orthonormal, eigenvalues descending.
    let a: Mat3 = [[4.0, 1.0, 0.5], [1.0, 3.0, 0.2], [0.5, 0.2, 2.0]];
    let (eval, v) = jacobi_eigen_sym(&a);
    assert!(
        eval[0] >= eval[1] && eval[1] >= eval[2],
        "eigenvalues descending"
    );
    // V diag Vᵀ ≈ A.
    let d: Mat3 = [
        [eval[0], 0.0, 0.0],
        [0.0, eval[1], 0.0],
        [0.0, 0.0, eval[2]],
    ];
    let recon = mat_mul(&mat_mul(&v, &d), &transpose(&v));
    for r in 0..3 {
        for c in 0..3 {
            assert!(
                (recon[r][c] - a[r][c]).abs() < 1e-9,
                "reconstruction at {r},{c}"
            );
        }
    }
    // V orthonormal: Vᵀ V ≈ I.
    let vtv = mat_mul(&transpose(&v), &v);
    for r in 0..3 {
        for c in 0..3 {
            let id = if r == c { 1.0 } else { 0.0 };
            assert!((vtv[r][c] - id).abs() < 1e-9, "V orthonormal");
        }
    }
}

#[test]
fn best_rotation_recovers_a_known_rotation_and_is_proper() {
    // Build S = Σ e (R e)ᵀ for a known rotation R → best_rotation(S) == R, det == +1 (never a reflection).
    let r = rot_axis([0.3, 1.0, 0.2], 0.9);
    let edges = [
        [1.0, 0.0, 0.0],
        [0.0, 2.0, 0.0],
        [0.0, 0.0, 1.5],
        [1.0, 1.0, 1.0],
    ];
    let mut s: Mat3 = [[0.0; 3]; 3];
    for &e in &edges {
        let re = mat_vec(&r, e);
        for (row, scell) in s.iter_mut().enumerate() {
            for (col, c) in scell.iter_mut().enumerate() {
                *c += e[row] * re[col];
            }
        }
    }
    let got = best_rotation(&s);
    for row in 0..3 {
        for col in 0..3 {
            assert!(
                (got[row][col] - r[row][col]).abs() < 1e-9,
                "recovered rotation at {row},{col}: {} vs {}",
                got[row][col],
                r[row][col]
            );
        }
    }
    // Proper rotation (the reflection fix held).
    let d = crate::linalg::det(&got);
    assert!((d - 1.0).abs() < 1e-9, "det(R) == +1, got {d}");
}

#[test]
fn cholesky_solves_an_spd_system() {
    // A small SPD system A x = b, solved via the factor → A x ≈ b.
    let n = 3;
    let a = vec![4.0, 1.0, 0.5, 1.0, 3.0, 0.2, 0.5, 0.2, 2.0];
    let l = cholesky(&a, n).expect("SPD");
    let mut x = vec![1.0, 2.0, 3.0]; // = b on input, x on output
    let b = x.clone();
    cholesky_solve(&l, n, &mut x);
    for i in 0..n {
        let ax: f64 = (0..n).map(|j| a[i * n + j] * x[j]).sum();
        assert!((ax - b[i]).abs() < 1e-9, "A x == b at row {i}");
    }
}

// ── ARAP correctness: rigid motions are reproduced exactly ────────────────────

#[test]
fn pure_translation_is_reproduced() {
    // Move every boundary vertex by t (no anchors) → the free interior translates by t (ARAP energy is
    // zero for a rigid motion, regardless of weights). The defining sanity check. The local/global
    // iteration converges *linearly* from a rest init — the grid center is the slowest point — so we
    // assert (a) more iterations strictly shrink the residual (it IS converging to the exact rigid
    // solution, not a biased fixed point) and (b) it reaches a tight bound.
    let (mesh, boundary, _center) = grid(7, 7);
    let region = Region {
        handles: boundary.clone(),
        anchors: Vec::new(),
    };
    let t = [0.7, -0.4, 0.9];
    let targets: Vec<Vec3> = boundary
        .iter()
        .map(|&h| {
            let p = mesh.positions[h as usize];
            [p[0] + t[0], p[1] + t[1], p[2] + t[2]]
        })
        .collect();
    let max_residual = |iters: usize| {
        let arap = ArapDeformer::prepare(&mesh, &region, ArapConfig { iters }).expect("prepare");
        let out = arap.deform(&targets);
        mesh.positions
            .iter()
            .enumerate()
            .fold(0.0f64, |m, (i, &p)| {
                m.max(dist(out[i], [p[0] + t[0], p[1] + t[1], p[2] + t[2]]))
            })
    };
    let coarse = max_residual(20);
    let fine = max_residual(800);
    assert!(
        fine < coarse,
        "more iterations strictly shrink the residual (converging to the exact rigid solution): {fine:e} < {coarse:e}"
    );
    assert!(
        fine < 1e-4,
        "rigid translation reproduced (max residual {fine:e})"
    );
}

#[test]
fn pure_rotation_is_reproduced() {
    // Rotate every boundary vertex rigidly about the grid center → the free interior follows the same
    // rotation (tests the SVD rotation extraction end-to-end). Energy-zero rigid motion.
    let (mesh, boundary, _center) = grid(7, 7);
    let pivot = [3.0, 3.0, 0.0];
    let r = rot_axis([0.2, 0.3, 1.0], 0.5);
    let apply = |p: Vec3| {
        let d = sub(p, pivot);
        let rd = mat_vec(&r, d);
        [rd[0] + pivot[0], rd[1] + pivot[1], rd[2] + pivot[2]]
    };
    let region = Region {
        handles: boundary.clone(),
        anchors: Vec::new(),
    };
    let arap = ArapDeformer::prepare(&mesh, &region, ArapConfig { iters: 30 }).expect("prepare");
    let targets: Vec<Vec3> = boundary
        .iter()
        .map(|&h| apply(mesh.positions[h as usize]))
        .collect();
    let out = arap.deform(&targets);
    for (i, &p) in mesh.positions.iter().enumerate() {
        assert!(
            dist(out[i], apply(p)) < 1e-3,
            "vertex {i} follows the rigid rotation"
        );
    }
}

// ── ARAP handle behavior: handles land on targets, anchors stay, smooth falloff ─

#[test]
fn handles_land_on_targets_anchors_stay_and_the_bump_is_smooth() {
    // Anchor the boundary at rest, lift the center handle +z → the handle lands exactly, the boundary is
    // unmoved, and the deformation is a smooth bump (an interior vertex rises, less than the handle).
    let (mesh, boundary, center) = grid(9, 9);
    let region = Region {
        handles: vec![center],
        anchors: boundary.clone(),
    };
    let arap = ArapDeformer::prepare(&mesh, &region, ArapConfig::default()).expect("prepare");
    let rest_c = mesh.positions[center as usize];
    let target = [rest_c[0], rest_c[1], rest_c[2] + 1.0];
    let out = arap.deform(&[target]);

    assert!(
        dist(out[center as usize], target) < 1e-9,
        "handle lands exactly on its target"
    );
    for &b in &boundary {
        assert!(
            dist(out[b as usize], mesh.positions[b as usize]) < 1e-9,
            "anchor {b} stays put"
        );
    }
    // A neighbor of the center rose, but less than the handle (smooth falloff, no rigid jump).
    let neighbor = center - 1;
    let lift = out[neighbor as usize][2] - mesh.positions[neighbor as usize][2];
    assert!(
        lift > 0.05,
        "the bump propagated to the neighbor (z rose by {lift})"
    );
    assert!(lift < 1.0, "but less than the handle's lift (smooth)");
}

#[test]
fn deform_is_deterministic() {
    // The determinism gate: identical inputs → bit-identical output (no rayon reduction-order drift).
    let (mesh, boundary, center) = grid(9, 9);
    let region = Region {
        handles: vec![center],
        anchors: boundary,
    };
    let arap = ArapDeformer::prepare(&mesh, &region, ArapConfig::default()).expect("prepare");
    let rest_c = mesh.positions[center as usize];
    let target = [rest_c[0] + 0.3, rest_c[1] - 0.2, rest_c[2] + 0.8];
    let a = arap.deform(&[target]);
    let b = arap.deform(&[target]);
    assert_eq!(
        a, b,
        "same input → identical f64 output (deterministic; gameplay-safe, the M8.1/ADR-020 path)"
    );
}

#[test]
fn all_constrained_region_returns_the_placement() {
    // Degenerate region: every vertex is a handle/anchor (no free vertices) → deform is just placement,
    // no solve. (Adversarial: the empty free-block must not panic.)
    let (mesh, _b, _c) = grid(3, 3);
    let all: Vec<u32> = (0..mesh.positions.len() as u32).collect();
    let region = Region {
        handles: all.clone(),
        anchors: Vec::new(),
    };
    let arap = ArapDeformer::prepare(&mesh, &region, ArapConfig::default()).expect("prepare");
    let targets: Vec<Vec3> = all.iter().map(|&h| [f64::from(h), 0.0, 0.0]).collect();
    let out = arap.deform(&targets);
    assert_eq!(arap.vertex_count(), mesh.positions.len());
    for (k, &h) in all.iter().enumerate() {
        assert_eq!(
            out[h as usize], targets[k],
            "constrained vertex placed at its target"
        );
    }
}

// ── benchmark discipline: precompute (one-shot) + per-frame deform on a region ─

#[test]
#[cfg_attr(
    debug_assertions,
    ignore = "release-only timing measurement (run --release)"
)]
fn arap_precompute_and_per_frame_cost_across_region_sizes() {
    // The region-of-interest is the COST KNOB (B.3: ARAP degrades on dense full meshes → restrict the
    // region). We sweep several localized region sizes and report the ONE-SHOT precompute (dense Cholesky
    // factorization) + the PER-FRAME deform (4 ARAP iterations), so the knob is explicit (no silent cap).
    // Pure f64 CPU → host-stable + identical on min-spec (the deterministic path); the GPU is uninvolved.
    // The localized region (a handle edit on a leg/arm patch) must hold the frame budget WITH headroom for
    // min-spec (we can only measure high-end here — see the ADR's honest min-spec note). The dense solve is
    // O(n²) per frame; the documented seam for large regions is a faer SPARSE Cholesky (the Laplacian is
    // sparse). Release-gated (debug timing is noise).
    let mut localized_per_ms = f64::INFINITY;
    for n in [12usize, 16, 20, 30] {
        let (mesh, boundary, center) = grid(n, n);
        let nfree = mesh.positions.len() - boundary.len();
        let region = Region {
            handles: vec![center],
            anchors: boundary,
        };

        let t_prep = std::time::Instant::now();
        let arap = ArapDeformer::prepare(&mesh, &region, ArapConfig::default()).expect("prepare");
        let prep_ms = t_prep.elapsed().as_secs_f64() * 1e3;

        let rest_c = mesh.positions[center as usize];
        let runs = 200u32;
        let _ = arap.deform(&[[rest_c[0], rest_c[1], rest_c[2] + 0.5]]); // warm up
        let t0 = std::time::Instant::now();
        let mut acc = 0.0f64;
        for i in 0..runs {
            let z = 0.5 + 0.001 * f64::from(i);
            let out = arap.deform(&[[rest_c[0], rest_c[1], rest_c[2] + z]]);
            acc += out[0][2];
        }
        let per_ms = t0.elapsed().as_secs_f64() * 1e3 / f64::from(runs);
        std::hint::black_box(acc);
        eprintln!(
            "[M9.5] ARAP region {:>4}v ({:>3} free): precompute {:>8.3} ms (one-shot), per-frame deform {:>8.4} ms ({} iters)",
            mesh.positions.len(),
            nfree,
            prep_ms,
            per_ms,
            ArapConfig::default().iters
        );
        // A genuinely localized handle-edit region (≤ ~256 verts) is the budget gate.
        if mesh.positions.len() <= 256 {
            localized_per_ms = localized_per_ms.min(per_ms);
        }
    }
    // The localized region must hold the 60 Hz budget with comfortable headroom (min-spec is slower and is
    // not measured here — the ADR records that boundary honestly).
    assert!(
        localized_per_ms < 8.0,
        "a localized region's per-frame deform (got {localized_per_ms} ms) must hold the frame budget with min-spec headroom"
    );
}

// ── auto skin weights (deliverable 2): biharmonic-inpainted weights → drive G3 LBS ────────────────

/// A vertical ribbon (3 × 11 grid, x∈{0,1,2}, y∈{0..10}) with a 2-bone chain through its center column:
/// joint 0 (root) at the bottom (1,0,0), joint 1 at the middle (1,5,0). Bone 0 covers the lower half,
/// bone 1 the upper half — so the upper vertices weight to joint 1, the lower to joint 0.
fn ribbon_and_skeleton() -> (DeformMesh, metrocalk_skeleton::Skeleton, [(Vec3, Vec3); 2]) {
    use metrocalk_skeleton::{Joint, Skeleton, Transform};
    let (mesh, _b, _c) = grid(3, 11);
    let tf = |t: [f32; 3]| Transform {
        translation: t,
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
    };
    let mut skel = Skeleton {
        joints: vec![
            Joint {
                parent: None,
                local_bind: tf([1.0, 0.0, 0.0]), // root at the bottom-center
                inverse_bind: [[0.0; 4]; 4],
            },
            Joint {
                parent: Some(0),
                local_bind: tf([0.0, 5.0, 0.0]), // child 5 up → global (1,5,0)
                inverse_bind: [[0.0; 4]; 4],
            },
        ],
    };
    skel.recompute_inverse_binds();
    // Bone segments (seeding hints), parallel to the joints: lower half → joint 0, upper half → joint 1.
    let bones = [
        ([1.0, 0.0, 0.0], [1.0, 5.0, 0.0]),
        ([1.0, 5.0, 0.0], [1.0, 10.0, 0.0]),
    ];
    (mesh, skel, bones)
}

#[test]
fn auto_skin_weights_are_a_partition_of_unity_and_localized() {
    let (mesh, _skel, bones) = ribbon_and_skeleton();
    let bind = auto_skin_weights(&mesh, &bones, SkinWeightConfig::default()).expect("weights");
    assert_eq!(bind.weights.len(), mesh.positions.len());

    for (v, w) in bind.weights.iter().enumerate() {
        let sum: f32 = w.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-5,
            "vertex {v} weights sum to 1 (partition of unity)"
        );
        assert!(
            w.iter().all(|&x| (-1e-6..=1.000_001).contains(&x)),
            "weights bounded [0,1]"
        );
    }

    // Weight of joint `j` at a vertex, summed over its slots.
    let wj = |v: usize, j: u16| -> f32 {
        bind.joints[v]
            .iter()
            .zip(bind.weights[v])
            .filter(|&(&jj, _)| jj == j)
            .map(|(_, x)| x)
            .sum()
    };
    let idx = |i: usize, j: usize| j * 3 + i;
    // Bottom-center vertex → mostly joint 0; top-center → mostly joint 1; middle → blended (both > 0.15).
    assert!(wj(idx(1, 0), 0) > 0.8, "bottom is owned by the root bone");
    assert!(wj(idx(1, 10), 1) > 0.8, "top is owned by the child bone");
    let mid = idx(1, 5);
    assert!(
        wj(mid, 0) > 0.15 && wj(mid, 1) > 0.15,
        "the middle blends both bones (smooth): j0={}, j1={}",
        wj(mid, 0),
        wj(mid, 1)
    );
}

#[test]
fn a_posed_bone_deforms_the_surface_smoothly_via_g3_lbs() {
    // The end-to-end success criterion: auto-weights → G3 LBS. Bend joint 1 (about +Z at its origin
    // y=5) → the upper vertices (weighted to joint 1) swing in +X, the bottom (weighted to the unposed
    // root) barely moves, and the middle moves partially — a smooth bone-driven deformation.
    use metrocalk_skeleton::{skin_position, Pose};
    let (mesh, skel, bones) = ribbon_and_skeleton();
    let bind = auto_skin_weights(&mesh, &bones, SkinWeightConfig::default()).expect("weights");

    let mut pose = Pose::new();
    let mut bent = skel.joints[1].local_bind;
    bent.rotation = [0.0, 0.0, (0.6f32).sin(), (0.6f32).cos()]; // ~69° about +Z
    pose.set(1, bent);
    let skin = skel.skinning_matrices(&pose);

    let idx = |i: usize, j: usize| j * 3 + i;
    let deformed = |v: usize| {
        let p32 = [
            mesh.positions[v][0] as f32,
            mesh.positions[v][1] as f32,
            mesh.positions[v][2] as f32,
        ];
        skin_position(p32, bind.joints[v], bind.weights[v], &skin)
    };
    let rest = |v: usize| {
        [
            mesh.positions[v][0] as f32,
            mesh.positions[v][1] as f32,
            mesh.positions[v][2] as f32,
        ]
    };
    let moved = |v: usize| {
        let d = deformed(v);
        let r = rest(v);
        ((d[0] - r[0]).powi(2) + (d[1] - r[1]).powi(2) + (d[2] - r[2]).powi(2)).sqrt()
    };

    // Sample up the bar: the lower half (joint 0, unposed) holds; above the pivot the displacement grows
    // smoothly with height (longer rotation arm) — the surface flows, no rigid jump. (The vertex exactly
    // at the pivot can't move — it IS the rotation center — so we sample y=6,8,10.)
    let bottom = moved(idx(1, 0));
    let up6 = moved(idx(1, 6));
    let up8 = moved(idx(1, 8));
    let top = moved(idx(1, 10));
    assert!(
        bottom < 0.05,
        "the bottom (root-weighted) barely moves: {bottom}"
    );
    assert!(
        top > 1.0,
        "the top (child-weighted) swings substantially: {top}"
    );
    assert!(
        bottom < up6 && up6 < up8 && up8 < top,
        "smooth monotone falloff up the bar: bottom {bottom} < y6 {up6} < y8 {up8} < top {top}"
    );
    // A +Z rotation of a point above the pivot swings it toward −X (x' = −r·sinθ).
    assert!(
        deformed(idx(1, 10))[0] < rest(idx(1, 10))[0] - 0.5,
        "top swung −X under the +Z bend"
    );
}
