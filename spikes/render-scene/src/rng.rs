//! SplitMix64 — byte-identical to `ecs/src/rng.rs` and the M0 spikes, so the render scene is built
//! from the **same seeded sequence** as M1.4's stress scene. The spike is excluded from the
//! workspace (it can't depend on `/ecs` — that pulls the native-only Flecs C core), so the PRNG is
//! reproduced here rather than imported; same seed ⇒ same draw order ⇒ same scene across runs/OS.
#![allow(clippy::cast_precision_loss, clippy::should_implement_trait)]

/// A small, deterministic PRNG. Same seed → same sequence everywhere.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f32 in [0, 1).
    pub fn f32(&mut self) -> f32 {
        (self.f64()) as f32
    }

    fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform f32 in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
}
