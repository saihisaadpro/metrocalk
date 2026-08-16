//! Biome rules — soft, weighted, overlapping. Never a classification.
//!
//! The instinctive implementation is a decision tree: "if height > 400 and slope > 0.7 then rock". It is
//! also the reason procedural terrain looks procedural, because the *boundaries* of that decision are
//! visible as jagged contour lines wherever two rules meet, and no amount of texture work hides them.
//!
//! So a biome here produces a **weight**, not a verdict. Every enabled rule scores the point through soft
//! four-stop bands over height, slope and moisture, optionally modulated by a patchiness field; the top four
//! scores are normalized and blended. Transitions come out soft for free, a point is never unassigned
//! (there is an explicit fallback), and an author can overlap rules deliberately — "alpine meadow" sitting
//! half inside "scree" is a feature, not a conflict to resolve.
//!
//! Four is the blend width because it is what a four-channel splat can express, and because a point
//! genuinely influenced by more than four biomes is an authoring problem rather than a rendering one.

use crate::field::band_weight;
use crate::noise;
use crate::recipe::TerrainRecipe;

/// How many biomes can influence one point.
pub const MAX_BLEND: usize = 4;

/// The blended biome influence at a point: up to [`MAX_BLEND`] rule indices with weights summing to 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiomeWeights {
    /// Rule indices into [`TerrainRecipe::biomes`], strongest first.
    pub index: [u16; MAX_BLEND],
    /// Normalized weights, parallel to `index`. Unused slots are 0.
    pub weight: [f32; MAX_BLEND],
    /// How many slots are in use.
    pub count: u8,
}

impl Default for BiomeWeights {
    fn default() -> Self {
        Self {
            index: [0; MAX_BLEND],
            weight: [0.0; MAX_BLEND],
            count: 0,
        }
    }
}

impl BiomeWeights {
    /// The strongest biome, if any.
    #[must_use]
    pub fn dominant(&self) -> Option<u16> {
        if self.count == 0 {
            None
        } else {
            Some(self.index[0])
        }
    }

    /// Weight of a specific biome (0 when it is not in the blend).
    #[must_use]
    pub fn weight_of(&self, biome: usize) -> f32 {
        let biome = biome as u16;
        (0..self.count as usize)
            .find(|&i| self.index[i] == biome)
            .map_or(0.0, |i| self.weight[i])
    }

    /// Iterate `(index, weight)` over the used slots.
    pub fn iter(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        (0..self.count as usize).map(move |i| (self.index[i] as usize, self.weight[i]))
    }
}

/// The inputs a biome rule scores against, gathered once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiomeSample {
    /// Height in metres.
    pub height: f32,
    /// Slope as rise over run.
    pub slope: f32,
    /// Moisture `0..1`.
    pub moisture: f32,
}

/// Score every rule at a point and return the normalized top-[`MAX_BLEND`] blend.
///
/// Falls back to biome 0 at full weight when no rule matches, so there is no such thing as a point with no
/// material — an unpainted hole in a terrain is far worse than a slightly wrong biome.
#[must_use]
pub fn weights_at(
    recipe: &TerrainRecipe,
    height: f32,
    slope: f32,
    moisture: f32,
    x: f32,
    z: f32,
) -> BiomeWeights {
    let mut best: [(u16, f32); MAX_BLEND] = [(0, 0.0); MAX_BLEND];
    let mut used = 0usize;

    for (i, rule) in recipe.biomes.iter().enumerate() {
        if !rule.enabled || rule.weight <= 0.0 {
            continue;
        }
        let mut w = band_weight(rule.height_band, height)
            * band_weight(rule.slope_band, slope)
            * band_weight(rule.moisture_band, moisture)
            * rule.weight;
        if w <= 1e-4 {
            continue;
        }
        if let Some([wl, threshold, softness]) = rule.patchiness {
            let wl = wl.max(0.01);
            let n = 0.5
                + 0.5
                    * noise::fbm(
                        x / wl,
                        z / wl,
                        recipe.seed ^ (0x81A3_5CD1u64.wrapping_add(i as u64)),
                        3,
                        2.0,
                        0.5,
                    );
            w *= noise::smoothstep(
                threshold - softness.max(1e-4),
                threshold + softness.max(1e-4),
                n,
            );
            if w <= 1e-4 {
                continue;
            }
        }
        // Insertion sort into the top-N, which is cheaper than a heap at N = 4 and keeps the order stable
        // (equal weights resolve by rule index, so the blend is deterministic).
        let mut slot = used.min(MAX_BLEND);
        if used == MAX_BLEND {
            if w <= best[MAX_BLEND - 1].1 {
                continue;
            }
            slot = MAX_BLEND - 1;
        } else {
            used += 1;
        }
        best[slot] = (i as u16, w);
        let mut k = slot;
        while k > 0 && best[k].1 > best[k - 1].1 {
            best.swap(k, k - 1);
            k -= 1;
        }
    }

    if used == 0 {
        // The explicit fallback: an unpainted hole in a terrain is far worse than a slightly wrong biome.
        return if recipe.biomes.is_empty() {
            BiomeWeights::default()
        } else {
            BiomeWeights {
                index: [0; MAX_BLEND],
                weight: [1.0, 0.0, 0.0, 0.0],
                count: 1,
            }
        };
    }

    let total: f32 = best[..used].iter().map(|(_, w)| *w).sum();
    let mut out = BiomeWeights {
        count: used as u8,
        ..BiomeWeights::default()
    };
    for (slot, (index, weight)) in best[..used].iter().enumerate() {
        out.index[slot] = *index;
        out.weight[slot] = if total > 0.0 { *weight / total } else { 0.0 };
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::BiomeRule;

    fn three_band_recipe() -> TerrainRecipe {
        TerrainRecipe {
            biomes: vec![
                BiomeRule::by_height("Sand", [-100.0, -90.0, 2.0, 8.0], 0),
                BiomeRule::by_height("Grass", [2.0, 8.0, 60.0, 90.0], 1),
                BiomeRule::by_height("Rock", [60.0, 90.0, 9000.0, 9001.0], 2),
            ],
            ..TerrainRecipe::default()
        }
    }

    #[test]
    fn a_point_in_one_band_is_that_biome_alone() {
        let r = three_band_recipe();
        let w = weights_at(&r, 30.0, 0.1, 0.5, 0.0, 0.0);
        assert_eq!(w.count, 1);
        assert_eq!(w.dominant(), Some(1));
        assert!((w.weight[0] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn overlapping_bands_blend_and_sum_to_one() {
        let r = three_band_recipe();
        // 5 m sits in the Sand fade-out and the Grass fade-in.
        let w = weights_at(&r, 5.0, 0.1, 0.5, 0.0, 0.0);
        assert_eq!(w.count, 2, "both bands should contribute");
        let sum: f32 = w.iter().map(|(_, x)| x).sum();
        assert!((sum - 1.0).abs() < 1e-5, "weights must normalize: {sum}");
        assert!(w.weight_of(0) > 0.0 && w.weight_of(1) > 0.0);
    }

    #[test]
    fn the_transition_is_continuous_not_a_step() {
        // The property this module exists for: sweeping height must not produce a jump in the weights.
        let r = three_band_recipe();
        // The step must be finer than the bands' own slope, or this measures the sampling rate rather than
        // the function: a 6 m fade has a maximum slope of 0.25 per metre, so a 0.05 m step can move a weight
        // by at most 0.0125 legitimately.
        let mut prev = weights_at(&r, 0.0, 0.1, 0.5, 0.0, 0.0);
        let mut worst_jump = 0.0f32;
        for i in 1..=2000 {
            let h = i as f32 * 0.05;
            let cur = weights_at(&r, h, 0.1, 0.5, 0.0, 0.0);
            for b in 0..3 {
                worst_jump = worst_jump.max((cur.weight_of(b) - prev.weight_of(b)).abs());
            }
            prev = cur;
        }
        assert!(
            worst_jump < 0.02,
            "biome weights stepped by {worst_jump} — that is a visible contour line"
        );
    }

    #[test]
    fn slope_and_moisture_gates_work() {
        let mut r = three_band_recipe();
        r.biomes.push(
            BiomeRule::by_height("Cliff", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 3)
                .with_slope([0.7, 0.9, 9.0, 9.1]),
        );
        // Flat ground: the cliff rule contributes nothing.
        assert_eq!(weights_at(&r, 30.0, 0.05, 0.5, 0.0, 0.0).weight_of(3), 0.0);
        // Steep ground: it dominates.
        let steep = weights_at(&r, 30.0, 2.0, 0.5, 0.0, 0.0);
        assert!(
            steep.weight_of(3) > 0.4,
            "cliff rule did not engage: {steep:?}"
        );

        let mut wet = three_band_recipe();
        wet.biomes.push(
            BiomeRule::by_height("Swamp", [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 3)
                .with_moisture([0.7, 0.8, 1.1, 1.2]),
        );
        assert_eq!(weights_at(&wet, 30.0, 0.1, 0.2, 0.0, 0.0).weight_of(3), 0.0);
        assert!(weights_at(&wet, 30.0, 0.1, 0.95, 0.0, 0.0).weight_of(3) > 0.3);
    }

    #[test]
    fn a_point_matching_nothing_still_gets_a_material() {
        let mut r = three_band_recipe();
        for b in &mut r.biomes {
            b.height_band = [1000.0, 1001.0, 1002.0, 1003.0];
        }
        let w = weights_at(&r, 0.0, 0.0, 0.5, 0.0, 0.0);
        assert_eq!(w.count, 1, "must fall back rather than leave a hole");
        assert_eq!(w.dominant(), Some(0));
    }

    #[test]
    fn more_than_four_overlapping_biomes_keep_the_strongest_four() {
        let r = TerrainRecipe {
            biomes: (0..7)
                .map(|i| {
                    let mut b =
                        BiomeRule::by_height(format!("B{i}"), [-1.0e6, -1.0e6, 1.0e6, 1.0e6], 0);
                    b.weight = 1.0 + i as f32;
                    b
                })
                .collect(),
            ..TerrainRecipe::default()
        };
        let w = weights_at(&r, 0.0, 0.0, 0.5, 0.0, 0.0);
        assert_eq!(w.count as usize, MAX_BLEND);
        // Strongest first, and the four kept are the four heaviest rules (indices 6,5,4,3).
        assert_eq!(w.index[0], 6);
        assert_eq!(w.index[3], 3);
        let sum: f32 = w.iter().map(|(_, x)| x).sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn patchiness_breaks_a_biome_up_without_leaving_holes() {
        let mut r = three_band_recipe();
        r.biomes[1].patchiness = Some([60.0, 0.5, 0.08]);
        let mut on = 0;
        let mut off = 0;
        for i in 0..400 {
            let x = i as f32 * 3.0;
            let w = weights_at(&r, 30.0, 0.1, 0.5, x, 0.0);
            if w.weight_of(1) > 0.5 {
                on += 1;
            } else {
                off += 1;
            }
            // Even where the patch is absent, something is assigned.
            assert!(w.count >= 1);
        }
        assert!(
            on > 20 && off > 20,
            "patchiness produced no variation: {on}/{off}"
        );
    }
}
