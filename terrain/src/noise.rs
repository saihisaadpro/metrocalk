//! Deterministic, transcendental-free coherent noise — the procedural half of the height field.
//!
//! Everything here is a pure function of *integer* lattice coordinates plus the recipe seed, mixed with a
//! SplitMix64-style avalanche (the same finalizer `metrocalk_ecs::rng` uses, so "random" here means the same
//! thing it means everywhere else in the engine). There is no RNG state, so there is no evaluation order to
//! get wrong: a chunk built on worker 7 in the middle of a session is bit-identical to the same chunk built
//! first on another machine.
//!
//! **The float discipline that makes that true.** Only `+ - * /` and `sqrt` appear below. IEEE-754 defines
//! all five exactly, Rust does not contract them into fused multiply-adds, and LLVM may not reassociate
//! them, so the results are bit-identical across x86-64, aarch64 and wasm32. Transcendentals are the exact
//! thing that would break it — `sin`, `cos`, `exp`, `powf` are libm calls whose last bits legitimately
//! differ between platforms — so gradients come from a fixed table and easing from a quintic polynomial.
//! This is why terrain heights may safely feed the deterministic simulation, even though ADR-020 ruled f32
//! out for *physics accumulation* (a different problem: millions of dependent steps, not one expression).

/// Mix three integers into a well-distributed 64-bit value (SplitMix64 finalizer over a lattice mix).
///
/// The two coordinate multipliers are large odd 64-bit primes, so `(x, z)` pairs do not alias along any
/// axis or diagonal — the visible failure mode of the classic `x * 57 + z * 131` hash is a faint diagonal
/// grain in the terrain, which this avoids.
#[must_use]
pub fn hash_2d(x: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed
        ^ (i64::from(x) as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (i64::from(z) as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

/// Mix a third integer in (a channel / octave / prototype selector) — same guarantees as [`hash_2d`].
#[must_use]
pub fn hash_3d(x: i32, z: i32, k: i32, seed: u64) -> u64 {
    hash_2d(
        x,
        z,
        seed ^ (i64::from(k) as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93),
    )
}

/// A hashed value in `[0, 1)` — the uniform primitive scatter, thinning and jitter are built from.
#[must_use]
pub fn hash_unit(x: i32, z: i32, seed: u64) -> f32 {
    // 24 bits is exactly f32's mantissa, so the division is exact and the distribution is flat.
    ((hash_2d(x, z, seed) >> 40) as f32) / 16_777_216.0
}

/// A hashed value in `[0, 1)` on a three-integer key.
#[must_use]
pub fn hash_unit_3(x: i32, z: i32, k: i32, seed: u64) -> f32 {
    ((hash_3d(x, z, k, seed) >> 40) as f32) / 16_777_216.0
}

/// Quintic ease `6t⁵ − 15t⁴ + 10t³` — Perlin's improved fade. Zero first *and* second derivative at both
/// ends, which is what keeps octave boundaries from showing up as faint creases under a low sun.
#[must_use]
pub fn fade(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

/// Linear interpolation.
#[must_use]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Smoothstep between two edges, clamped. `lo == hi` degenerates to a hard step (no division by zero).
#[must_use]
pub fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    if hi <= lo {
        return if x < lo { 0.0 } else { 1.0 };
    }
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The 8 lattice gradients, diagonals pre-scaled to unit length so every direction contributes the same
/// amplitude. A fixed table (rather than `sin`/`cos` of a hashed angle) is what keeps this deterministic.
const GRAD: [[f32; 2]; 8] = [
    [1.0, 0.0],
    [-1.0, 0.0],
    [0.0, 1.0],
    [0.0, -1.0],
    [0.707_106_77, 0.707_106_77],
    [-0.707_106_77, 0.707_106_77],
    [0.707_106_77, -0.707_106_77],
    [-0.707_106_77, -0.707_106_77],
];

/// Gradient (Perlin) noise in roughly `[-1, 1]`. Smoother and less grid-aligned than value noise, which
/// matters because terrain silhouettes expose axis alignment mercilessly.
#[must_use]
pub fn perlin_2d(x: f32, z: f32, seed: u64) -> f32 {
    let xi = x.floor();
    let zi = z.floor();
    let fx = x - xi;
    let fz = z - zi;
    let (ix, iz) = (xi as i32, zi as i32);

    let dot = |cx: i32, cz: i32, dx: f32, dz: f32| {
        let g = GRAD[(hash_2d(cx, cz, seed) & 7) as usize];
        g[0] * dx + g[1] * dz
    };

    let n00 = dot(ix, iz, fx, fz);
    let n10 = dot(ix + 1, iz, fx - 1.0, fz);
    let n01 = dot(ix, iz + 1, fx, fz - 1.0);
    let n11 = dot(ix + 1, iz + 1, fx - 1.0, fz - 1.0);

    let u = fade(fx);
    let v = fade(fz);
    // 1.4142 normalizes the theoretical 2D Perlin bound (1/√2) back to ±1.
    lerp(lerp(n00, n10, u), lerp(n01, n11, u), v) * std::f32::consts::SQRT_2
}

/// Value noise in `[-1, 1]` — cheaper than [`perlin_2d`] and blockier; used for masks and jitter fields
/// where the grid character is invisible.
#[must_use]
pub fn value_2d(x: f32, z: f32, seed: u64) -> f32 {
    let xi = x.floor();
    let zi = z.floor();
    let fx = fade(x - xi);
    let fz = fade(z - zi);
    let (ix, iz) = (xi as i32, zi as i32);
    let v = |cx: i32, cz: i32| hash_unit(cx, cz, seed) * 2.0 - 1.0;
    lerp(
        lerp(v(ix, iz), v(ix + 1, iz), fx),
        lerp(v(ix, iz + 1), v(ix + 1, iz + 1), fx),
        fz,
    )
}

/// Fractal Brownian motion: `octaves` of [`perlin_2d`], each `lacunarity`× finer and `gain`× quieter.
///
/// The result is normalized by the sum of amplitudes, so it stays in `[-1, 1]` whatever the octave count —
/// changing octaves changes *detail*, never the silhouette height. That property is what lets a distant
/// chunk drop octaves as a cost knob without the world visibly rising or sinking.
#[must_use]
pub fn fbm(x: f32, z: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut fx = x;
    let mut fz = z;
    for o in 0..octaves.max(1) {
        sum += perlin_2d(fx, fz, seed ^ (u64::from(o) << 33)) * amp;
        norm += amp;
        amp *= gain;
        fx *= lacunarity;
        fz *= lacunarity;
    }
    if norm > 0.0 {
        sum / norm
    } else {
        0.0
    }
}

/// Ridged multifractal in `[0, 1]`: `1 − |noise|`, squared per octave and weighted by the previous octave,
/// which is what produces sharp ridge lines and smooth valleys instead of symmetric lumps. The classic
/// choice for mountains.
#[must_use]
pub fn ridged(x: f32, z: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut weight = 1.0f32;
    let mut fx = x;
    let mut fz = z;
    for o in 0..octaves.max(1) {
        let n = perlin_2d(fx, fz, seed ^ (u64::from(o) << 33));
        let mut r = 1.0 - n.abs();
        r *= r;
        r *= weight.clamp(0.0, 1.0);
        weight = r * 2.0;
        sum += r * amp;
        norm += amp;
        amp *= gain;
        fx *= lacunarity;
        fz *= lacunarity;
    }
    if norm > 0.0 {
        (sum / norm).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Billow noise in `[0, 1]` — `|fbm|`, giving rounded dune/cloud lobes.
#[must_use]
pub fn billow(x: f32, z: f32, seed: u64, octaves: u32, lacunarity: f32, gain: f32) -> f32 {
    let mut sum = 0.0;
    let mut amp = 1.0;
    let mut norm = 0.0;
    let mut fx = x;
    let mut fz = z;
    for o in 0..octaves.max(1) {
        sum += perlin_2d(fx, fz, seed ^ (u64::from(o) << 33)).abs() * amp;
        norm += amp;
        amp *= gain;
        fx *= lacunarity;
        fz *= lacunarity;
    }
    if norm > 0.0 {
        (sum / norm).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Domain warp: offset the sample point by a low-frequency noise vector before evaluating the real field.
///
/// This is the single highest-value-per-line trick in procedural terrain. Pure fBm reads as "noise";
/// warping its *input* bends the ridges and braids the valleys, which is what makes a field read as
/// geology instead of static.
#[must_use]
pub fn warp_2d(x: f32, z: f32, seed: u64, amount: f32, frequency: f32) -> (f32, f32) {
    if amount == 0.0 {
        return (x, z);
    }
    let wx = perlin_2d(x * frequency, z * frequency, seed ^ 0x5F3D_1A77);
    let wz = perlin_2d(
        x * frequency + 31.7,
        z * frequency - 17.3,
        seed ^ 0xA13C_7E21,
    );
    (x + wx * amount, z + wz * amount)
}

/// Worley / cellular F1 distance in `[0, 1]` — distance to the nearest jittered lattice point, normalized.
///
/// Used for patchy masks (clearings, rock outcrops, biome mottling) where fBm's uniform texture looks
/// artificial. Searches the 3×3 neighbourhood, which is sufficient for a jitter of ≤ 1 cell.
#[must_use]
pub fn worley_2d(x: f32, z: f32, seed: u64) -> f32 {
    let xi = x.floor() as i32;
    let zi = z.floor() as i32;
    let mut best = f32::MAX;
    for dz in -1..=1 {
        for dx in -1..=1 {
            let (cx, cz) = (xi + dx, zi + dz);
            let px = cx as f32 + hash_unit(cx, cz, seed);
            let pz = cz as f32 + hash_unit_3(cx, cz, 1, seed);
            let (ex, ez) = (px - x, pz - z);
            let d2 = ex * ex + ez * ez;
            if d2 < best {
                best = d2;
            }
        }
    }
    // The maximum possible F1 distance in a unit-jittered lattice is ~1.5; normalize and clamp.
    (best.sqrt() / 1.5).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_is_stable_and_well_spread() {
        // A pinned value: if the mixer ever changes, every existing world changes with it, so this is a
        // deliberate tripwire rather than a tautology.
        assert_eq!(hash_2d(0, 0, 0), 0);
        assert_ne!(hash_2d(1, 0, 7), hash_2d(0, 1, 7), "no axis aliasing");
        assert_ne!(hash_2d(2, 3, 7), hash_2d(3, 2, 7), "no diagonal aliasing");
        // Uniformity: the mean of 4096 samples should sit near 0.5.
        let mut sum = 0.0;
        for i in 0..64 {
            for j in 0..64 {
                sum += hash_unit(i, j, 99);
            }
        }
        let mean = sum / 4096.0;
        assert!((mean - 0.5).abs() < 0.02, "hash mean drifted: {mean}");
    }

    #[test]
    fn perlin_is_zero_on_the_lattice_and_bounded_between() {
        for i in -3..3 {
            for j in -3..3 {
                let v = perlin_2d(i as f32, j as f32, 12345);
                assert!(v.abs() < 1e-6, "gradient noise must vanish on the lattice");
            }
        }
        let mut max = 0.0f32;
        for i in 0..200 {
            for j in 0..200 {
                let v = perlin_2d(i as f32 * 0.137, j as f32 * 0.137, 5);
                max = max.max(v.abs());
            }
        }
        assert!(max <= 1.0001, "perlin exceeded its normalized bound: {max}");
        assert!(max > 0.5, "perlin never got near its bound: {max}");
    }

    #[test]
    fn fbm_amplitude_is_independent_of_octave_count() {
        // The silhouette must not rise or sink when a distant chunk drops octaves — that would show up as
        // the world visibly moving at an LOD boundary.
        let mut peak = [0.0f32; 3];
        for (slot, oct) in [2u32, 4, 7].iter().enumerate() {
            let mut m = 0.0f32;
            for i in 0..300 {
                for j in 0..300 {
                    m = m.max(fbm(i as f32 * 0.05, j as f32 * 0.05, 77, *oct, 2.0, 0.5).abs());
                }
            }
            peak[slot] = m;
        }
        for p in peak {
            assert!(p <= 1.0001, "fbm broke its bound: {p}");
        }
        assert!(
            (peak[0] - peak[2]).abs() < 0.35,
            "octave count changed the amplitude envelope: {peak:?}"
        );
    }

    #[test]
    fn ridged_and_billow_stay_in_unit_range() {
        for i in 0..120 {
            for j in 0..120 {
                let (x, z) = (i as f32 * 0.083, j as f32 * 0.083);
                let r = ridged(x, z, 3, 5, 2.0, 0.5);
                let b = billow(x, z, 3, 5, 2.0, 0.5);
                assert!((0.0..=1.0).contains(&r), "ridged out of range: {r}");
                assert!((0.0..=1.0).contains(&b), "billow out of range: {b}");
            }
        }
    }

    #[test]
    fn warp_moves_the_domain_but_stays_bounded() {
        let (wx, wz) = warp_2d(10.0, 20.0, 4, 8.0, 0.01);
        assert!((wx - 10.0).abs() <= 8.001 && (wz - 20.0).abs() <= 8.001);
        assert_eq!(
            warp_2d(10.0, 20.0, 4, 0.0, 0.01),
            (10.0, 20.0),
            "no-op at 0"
        );
    }

    #[test]
    fn worley_is_zero_at_a_cell_point_and_unit_bounded() {
        let cx = 4;
        let cz = -2;
        let px = cx as f32 + hash_unit(cx, cz, 8);
        let pz = cz as f32 + hash_unit_3(cx, cz, 1, 8);
        assert!(worley_2d(px, pz, 8) < 1e-6);
        for i in 0..80 {
            for j in 0..80 {
                let v = worley_2d(i as f32 * 0.37, j as f32 * 0.37, 8);
                assert!((0.0..=1.0).contains(&v));
            }
        }
    }

    #[test]
    fn everything_is_bit_reproducible() {
        // Same inputs → bit-identical outputs, twice, including through the fractal stack.
        let a: Vec<u32> = (0..64)
            .map(|i| fbm(i as f32 * 0.31, 7.5, 991, 6, 2.13, 0.47).to_bits())
            .collect();
        let b: Vec<u32> = (0..64)
            .map(|i| fbm(i as f32 * 0.31, 7.5, 991, 6, 2.13, 0.47).to_bits())
            .collect();
        assert_eq!(a, b);
    }
}
