//! `metrocalk-deform` — the high-fidelity mesh-deformation tier (M9.5 / G5, ADR-029): the rung above
//! rigid part editing (G2) and bone-LBS (G3). **Drag a handle, the surface flows** — As-Rigid-As-Possible
//! deformation, plus an optional **auto-skin-weights** path (`skin_weights`, added next) that drives
//! G3's LBS.
//!
//! **Why we reimplement rather than depend on `baby_shark`** (the ADR-029 determinism audit): a
//! deformation that feeds gameplay must be deterministic and cross-platform bit-identical (the M8.1 /
//! ADR-020 boundary). `baby_shark` is the one ready-made pure-Rust deformation primitive, but (a) it runs
//! its solve on `rayon`, whose reduction order is **not** deterministic by default — the exact hazard the
//! audit flags; (b) its ARAP branding / `prepare_deform` API / wasm support are single-source, unverified;
//! (c) it is single-maintainer. So G5 reimplements the ARAP local/global loop on our **own deterministic
//! primitives** ([`linalg`]: a fixed-sweep Jacobi SVD with a pinned reflection-fix, a sequential dense
//! Cholesky — no threads). The determinism is **ours by construction**, not assumed of a dependency.
//!
//! The universal pattern (B.3): **front-load a one-time precompute** (factor the region's Laplacian once)
//! **then deform cheaply per frame** ([`ArapDeformer::prepare`] → [`Deformer::deform`]). The
//! **region-of-interest is the cost knob** (ARAP degrades on dense full meshes → restrict it). `f64`
//! throughout; pure Rust (no foreign math type on the public surface — invariant 5) → `wasm32`-clean.
//!
//! **Cage / lattice deformation** (Green / Harmonic / MVC coordinates) is the cheapest high-quality
//! per-frame option, but there is **no Rust cage library** (libigl/C++ territory) — it is a documented
//! **port-or-reimplement seam** (ADR-029), not built here. The [`Deformer`] trait is the seam: a future
//! cage deformer slots in behind the same per-frame contract.

// Math-heavy crate: short names (a/b/c/u/v/s/t/p/q) are canonical; the precise float constants in tests
// read clearer un-separated; index→f64 loses no precision at these counts.
#![allow(
    clippy::many_single_char_names,
    clippy::similar_names,
    clippy::module_name_repetitions,
    // Fixed 3×3 / n×n index loops over flattened matrices read clearer than iterator chains here.
    clippy::needless_range_loop
)]

pub mod arap;
pub mod linalg;

pub use arap::{ArapConfig, ArapDeformer, DeformMesh, Deformer, Region};
pub use linalg::Vec3;

#[cfg(test)]
mod tests;
