//! M8.1 physics determinism + f64 spike (P1 ≡ NET-4) — a THROWAWAY spike, not production code.
//!
//! A standalone, seeded rigid-body harness that hashes the serialized world after a fixed number of
//! fixed-`dt` steps, in **f64** (default) or **f32** (`--no-default-features --features f32`), with
//! `enhanced-determinism`. It proves (or disproves) **bit-identical world state across IEEE-754 targets**
//! (P1), quantifies the **f64 cost** (deliverable 2), captures **solver/energy/contact diagnostics**
//! (deliverable 3), checks **input-replay** (P2) and the **snapshot/restore #910 risk + the
//! `BvhOptimizationStrategy::None` mitigation** (P3), and emits the **bake/replay provenance envelope**
//! (deliverable 6 — the one standard physics replay (M8.4) and rollback netcode (NET-4) both consume).
//!
//! Status: harness logic + the provenance/diagnostics standard are defined here; the rapier-0.33 scene/
//! step bodies (`scene`, `step`, snapshot) are completed against the compiler (the glam-migrated math API
//! + the new `BroadPhaseBvh`). No measurement is recorded until run cross-platform — see `RESULTS.md`.

// ── Determinism-path verification (the "enhanced-determinism silently inactive" adversarial guard) ──
// We enable `rapier3d-f64/enhanced-determinism` in Cargo.toml, which forces `simba/libm_force` +
// `parry3d/enhanced-determinism`. rapier itself REFUSES to compile enhanced-determinism together with
// simd/parallel (they are mutually exclusive — the dual-config), so a SUCCESSFUL build of this crate is
// itself the proof that simd/parallel are OFF and libm is in. `cargo tree -e features` confirms `libm`
// is present (recorded in RESULTS.md), and the ULTIMATE arbiter is cross-platform hash equality (a stray
// SIMD/FMA path would diverge across targets). The precision guards below are checked on THIS crate's
// own features (so they are meaningful, unlike a dep-feature cfg).
#[cfg(all(feature = "f64", feature = "f32"))]
compile_error!("pick exactly one precision: f64 (default) or --features f32");
#[cfg(not(any(feature = "f64", feature = "f32")))]
compile_error!("pick a precision: f64 (default) or --features f32");

use serde::Serialize;

/// The active precision label ("f64" | "f32") — recorded in the provenance envelope.
#[cfg(feature = "f64")]
pub const PRECISION: &str = "f64";
#[cfg(feature = "f32")]
pub const PRECISION: &str = "f32";

/// The fixed simulation timestep (Hz → seconds). A FIXED dt is non-negotiable for determinism.
pub const FIXED_DT: f64 = 1.0 / 60.0;
/// Steps per run (≥10k per the P1 gate).
pub const STEPS: u32 = 10_000;
/// The deterministic scene seed (same seed → byte-identical scene across runs/targets, M0 discipline).
pub const SEED: u64 = 0x4D45_5452_4F43_414C; // "METROCAL"

/// The **bake/replay provenance envelope** (deliverable 6) — the metadata EVERY future bake/replay
/// carries so a replay is reproducible and version-locked, serving BOTH physics scrub/resume (M8.4) and
/// rollback netcode (NET-4). Built once. Serialized into `RESULTS.md` and emitted by the harness.
#[derive(Debug, Clone, Serialize)]
pub struct Provenance {
    /// Solver backend + version (e.g. "rapier3d-f64 0.33.0 / parry3d 0.28").
    pub backend: String,
    /// "f64" | "f32".
    pub precision: String,
    /// Whether the deterministic math path is active (enhanced-determinism / libm on, simd/parallel off).
    pub enhanced_determinism: bool,
    /// Fixed timestep, seconds.
    pub fixed_dt: f64,
    /// Substep policy — MUST be a deterministic, recorded policy (same inputs → same substep decisions);
    /// runtime-adaptive substepping belongs only to the non-authoritative SIMD config (incompatible here).
    pub substep_policy: String,
    /// RNG seed for the seeded scene.
    pub seed: u64,
    /// Body-creation order policy (here: deterministic index order from the seeded generator).
    pub body_creation_order: String,
    /// Contact-ordering mode (rapier's deterministic contact ordering under enhanced-determinism).
    pub contact_ordering: String,
    /// Units / world scale (meters, gravity).
    pub units: String,
    /// The broad-phase mode used (default vs the `None` optimization-strategy snapshot-determinism fix).
    pub broad_phase: String,
    /// Per-collision-shape content hashes (so a shape change invalidates a replay).
    pub collider_shape_hashes: Vec<String>,
    /// Sampled per-frame world-state hashes (the replay/scrub checkpoints).
    pub frame_hashes: Vec<FrameHash>,
    /// The final serialized-world hash — the P1 cross-platform equality key.
    pub final_world_hash: String,
    /// Toolchain version-lock (rustc + crate versions) — replays are invalid across bumps.
    pub toolchain: String,
    /// Step count + body/joint counts (the scene shape).
    pub steps: u32,
    pub body_count: usize,
    pub joint_count: usize,
    /// Per-step wall time p50/p99 in microseconds (native only; `None` on wasm — no monotonic clock in
    /// `core`). The f64-vs-f32 cost comparison (deliverable 2).
    pub step_us_p50: Option<f64>,
    pub step_us_p99: Option<f64>,
}

/// A sampled per-frame checkpoint: the frame index, the world-state hash, and the quality diagnostics
/// (deliverable 3 — determinism is *quality*, not just bit-equality; this payload feeds the M8.4 contact
/// debugger + the bake provenance).
#[derive(Debug, Clone, Serialize)]
pub struct FrameHash {
    pub frame: u32,
    pub world_hash: String,
    /// Total mechanical energy (kinetic + gravitational potential) — drift over the run is a quality
    /// signal (a stable deterministic solver should not gain energy).
    pub energy: f64,
    /// Active contact-manifold count (the contact graph size).
    pub contacts: usize,
    /// Max contact penetration depth (a constraint-residual proxy — the deeper the worse the solve).
    pub max_penetration: f64,
}

/// Blake3 content hash of arbitrary bytes → hex (the deterministic world-hash primitive).
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// A tiny deterministic LCG — seeded, portable, identical across targets (no `rand`/`getrandom`, no
/// platform entropy). Drives the seeded scene + the recorded input stream (P2).
pub struct Lcg(pub u64);
impl Lcg {
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 1
    }
    /// A deterministic f64 in `[lo, hi)` — built from the integer LCG (precision-independent bits).
    pub fn range(&mut self, lo: f64, hi: f64) -> f64 {
        let frac = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        lo + frac * (hi - lo)
    }
}

pub mod harness;
