//! **The determinism mechanism** — witness-config-in-the-op + the quantized canonical state (ADR-076).
//!
//! `ezpz`'s raw f64 solve is a derivation: bit-reproducible same-machine (measured), but its `faer`/`gemm`
//! backend does runtime SIMD dispatch, so cross-ISA bit-identity is UNVERIFIED. We do not trust it — we
//! make determinism OURS in two moves:
//!
//! 1. **The witness is stored in the op** and grid-snapped ([`snap_witness`]), so every peer re-solves
//!    from a *bit-identical* initial configuration → the same solution **branch** (the "flip" fix). A
//!    two-branch system (an "elbow up/down" triangle) can't diverge between peers, because they share the
//!    start.
//! 2. **The solved state is quantized to i64** at [`CANON_QUANTUM_MM`] (1 nm) — far below any CAD tolerance
//!    and ~10⁶× above `faer`'s cross-ISA ULP noise — so the *stored* solve is portable + bit-reproducible
//!    even where the raw f64 would differ in its last bits. This is the M13.1/ADR-050 "store integer/exact
//!    state, never raw floats" discipline, applied to a constraint solve.
//!
//! A [`SketchSolve`] is the lossless "a sketch solve is a file" artifact: the canonical sketch (its
//! constraints and its snapped witness) together with the canonical solved state. It
//! [`SketchSolve::verify_replay`]s bit-for-bit and carries a content-addressed [`SketchSolve::identity`].

use serde::{Deserialize, Serialize};

use crate::sketch::{CircleDef, Point, Sketch};
use crate::Solver;

/// The canonical grid: solved coordinates and the witness snap to integer multiples of this (millimetres).
/// 1e-6 mm = 1 nanometre — below any real CAD tolerance, above `faer`'s cross-ISA rounding noise, so the
/// canonical state is portable.
pub const CANON_QUANTUM_MM: f64 = 1e-6;

/// Quantize a coordinate to the canonical i64 grid. Total + deterministic: a non-finite value maps to 0
/// (a diverged solve is reported `!satisfied` and never committed, but this stays total).
#[must_use]
pub fn canon_i64(v: f64) -> i64 {
    if !v.is_finite() {
        return 0;
    }
    // round-half-away-from-zero is a deterministic IEEE op; post-round the value is integral, and any
    // sane sketch coordinate in mm is far inside i64 (±9.2e12 mm at 1 nm resolution).
    #[allow(clippy::cast_possible_truncation)]
    {
        (v / CANON_QUANTUM_MM).round() as i64
    }
}

/// De-quantize a canonical i64 back to the (grid-snapped) f64 coordinate.
#[must_use]
pub fn dequant(i: i64) -> f64 {
    // i64→f64 is exact for any coordinate inside ±2^53 grid units; deterministic everywhere.
    #[allow(clippy::cast_precision_loss)]
    {
        (i as f64) * CANON_QUANTUM_MM
    }
}

/// Return a copy of `sketch` whose witness (point positions + circle radii) is snapped to the canonical
/// grid. The snapped witness is bit-identical across peers (same i64 → same f64 product → same bincode
/// bytes), so it is a *canonical* witness — the guard against "a non-canonical witness changes the branch".
#[must_use]
pub fn snap_witness(sketch: &Sketch) -> Sketch {
    Sketch {
        points: sketch
            .points
            .iter()
            .map(|p| Point::new(dequant(canon_i64(p.x)), dequant(canon_i64(p.y))))
            .collect(),
        circles: sketch
            .circles
            .iter()
            .map(|c| CircleDef {
                center: c.center,
                radius: dequant(canon_i64(c.radius)),
            })
            .collect(),
        constraints: sketch.constraints.clone(),
    }
}

/// A solved sketch, canonical + reproducible — **"a sketch solve is a file"**. Self-contained: it carries
/// the constraints + the snapped witness (so it re-solves) and the canonical solved state (portable + the
/// identity basis).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SketchSolve {
    /// The canonical sketch: the constraints + the grid-snapped witness (the initial configuration).
    pub sketch: Sketch,
    /// The canonical solved point coordinates (i64 grid units), one `[x, y]` per point.
    pub solved_points: Vec<[i64; 2]>,
    /// The canonical solved circle radii (i64 grid units).
    pub solved_radii: Vec<i64>,
    /// Did the solve converge?
    pub converged: bool,
    /// Were all constraints satisfied?
    pub satisfied: bool,
    /// The backend that produced this (recorded for the audit).
    pub solver: String,
}

impl SketchSolve {
    /// Solve `sketch` and capture the canonical result. The witness is snapped first (so the stored sketch
    /// is canonical) and the raw f64 solve is quantized.
    #[must_use]
    pub fn compute<S: Solver>(sketch: &Sketch, solver: &S) -> Self {
        let canon = snap_witness(sketch);
        let res = solver.solve(&canon);
        Self {
            sketch: canon,
            solved_points: res
                .points
                .iter()
                .map(|p| [canon_i64(p.x), canon_i64(p.y)])
                .collect(),
            solved_radii: res.radii.iter().map(|r| canon_i64(*r)).collect(),
            converged: res.converged,
            satisfied: res.satisfied,
            solver: solver.name().to_string(),
        }
    }

    /// The LOSSLESS artifact bytes — bincode of the canonical (i64/structure) state, so the bytes ARE the
    /// identity. No JSON shortest-float is ever in the path.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        bincode::serialize(self).expect("a SketchSolve serializes (pure canonical data)")
    }

    /// Reload a "sketch solve is a file" artifact.
    ///
    /// # Errors
    /// The bincode error if the artifact is malformed / out of format.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, bincode::Error> {
        bincode::deserialize(bytes)
    }

    /// The content-addressed identity of this solve (FNV-1a 128 over the canonical bytes). Bit-identical
    /// across peers/runs whenever the canonical state is — the reproducible witness/solve identity.
    #[must_use]
    pub fn identity(&self) -> String {
        format!("mtksketch:{:032x}", fnv1a_128(&self.to_bytes()))
    }

    /// **Replay** — re-solve the stored (canonical-witness) sketch and confirm the canonical solved state
    /// matches bit-for-bit. Meaningful only for a deterministic backend; an honest `false` is returned for
    /// a non-deterministic one (the same discipline as the co-sim audit — never a faked replay).
    #[must_use]
    pub fn verify_replay<S: Solver>(&self, solver: &S) -> bool {
        if !solver.deterministic() {
            return false;
        }
        let again = Self::compute(&self.sketch, solver);
        again.solved_points == self.solved_points
            && again.solved_radii == self.solved_radii
            && again.converged == self.converged
            && again.satisfied == self.satisfied
    }

    /// A solved point, de-quantized for readout (mm).
    ///
    /// # Panics
    /// If `i` is out of range.
    #[must_use]
    pub fn solved_point(&self, i: usize) -> Point {
        let [x, y] = self.solved_points[i];
        Point::new(dequant(x), dequant(y))
    }
}

// FNV-1a, 128-bit — the same content-addressing family as `metrocalk_assets::AssetId`, kept dependency-free
// so this crate stays lean (no `assets` pull). Deterministic on every target.
const FNV_OFFSET_128: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME_128: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

fn fnv1a_128(bytes: &[u8]) -> u128 {
    let mut h = FNV_OFFSET_128;
    for &b in bytes {
        h ^= u128::from(b);
        h = h.wrapping_mul(FNV_PRIME_128);
    }
    h
}
