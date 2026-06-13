//! SplitMix64 — identical to the M0 spikes, so the seeded scene built through the wrapper has the
//! exact same structure as `spikes/flecs` (and thus the same 211-match compat result), giving a
//! direct wrapper-vs-raw cross-check.
// a PRNG: truncation/precision loss is intentional, and `next` is the conventional name (not Iterator).
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::should_implement_trait
)]

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

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn f64(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.f64() < p
    }
}
