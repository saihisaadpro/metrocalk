//! Automatic material blending, baked per chunk.
//!
//! ## The choice: bake, don't splat
//!
//! The textbook approach is a splat map plus N tiling textures sampled and blended in the fragment shader.
//! It is also the approach that forces a bespoke terrain shader, a bespoke vertex format, a bespoke bind
//! group layout, and a permanent fork between "terrain" and "everything else" in the renderer.
//!
//! This bakes instead: the blend is resolved on the CPU into one albedo texture and one detail normal map
//! per chunk, at a resolution that halves per LOD. The consequences are worth being explicit about, because
//! this is the single largest architectural decision in the subsystem:
//!
//! * **A terrain chunk is an ordinary textured mesh.** It goes through the existing PBR mesh pipeline, the
//!   existing texture upload with mip generation, the existing instanced draw. No shader variant, no fourth
//!   bind group, nothing to keep in sync — and it works unchanged on the WebGPU path, which has no binding
//!   arrays to lean on.
//! * **Blending is unlimited in complexity and free at run time.** Eight materials with soft biome
//!   transitions, road overlays, patchiness and per-material mottling all resolve to the same one texture
//!   fetch a rock does.
//! * **The cost is texel density and memory.** 4 texels per metre at LOD 0 is comparable to a production
//!   landscape weightmap, and the budget evicts distant chunks; but a bake cannot give the infinite detail
//!   that shader-side tiling gives at grazing angles. Adding a tiling detail sample to the mesh shader later
//!   is a strictly additive upgrade — the bake stays as the macro layer.
//!
//! ## Two-rate evaluation
//!
//! The expensive inputs (moisture, biome scoring, spline lookup) vary over tens of metres; the cheap ones
//! (per-material mottling) vary over centimetres. So the expensive ones are evaluated on a coarse lattice
//! and interpolated, and the cheap one runs per texel. That is a 16× saving on a 256² bake with no visible
//! difference, and it is the difference between a chunk baking in one millisecond and in twenty.

use crate::field::Terrain;
use crate::mesh::ChunkSamples;
use crate::noise;
use crate::recipe::{MaterialLayer, TerrainRecipe};
use crate::spline::SplineIndex;

/// Texels between coarse lattice samples of the expensive inputs.
const COARSE_STEP: u32 = 4;

/// Up to four material layers blended at a point.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct SplatSample {
    /// Material layer indices.
    pub layer: [u16; 4],
    /// Weights, normalized to sum to 1 over the used slots.
    pub weight: [f32; 4],
    /// Used slots.
    pub count: u8,
}

impl SplatSample {
    /// Iterate `(layer, weight)`.
    pub fn iter(&self) -> impl Iterator<Item = (usize, f32)> + '_ {
        (0..self.count as usize).map(move |i| (self.layer[i] as usize, self.weight[i]))
    }

    /// The heaviest layer, if any.
    #[must_use]
    pub fn dominant(&self) -> Option<usize> {
        (self.count > 0).then(|| self.layer[0] as usize)
    }

    fn add(&mut self, layer: u16, w: f32) {
        if w <= 0.0 {
            return;
        }
        for i in 0..self.count as usize {
            if self.layer[i] == layer {
                self.weight[i] += w;
                return;
            }
        }
        if (self.count as usize) < 4 {
            self.layer[self.count as usize] = layer;
            self.weight[self.count as usize] = w;
            self.count += 1;
        } else {
            // Replace the weakest slot if this is heavier — keeps the four strongest materials.
            let (mut wi, mut wv) = (0usize, self.weight[0]);
            for i in 1..4 {
                if self.weight[i] < wv {
                    wi = i;
                    wv = self.weight[i];
                }
            }
            if w > wv {
                self.layer[wi] = layer;
                self.weight[wi] = w;
            }
        }
    }

    fn finish(&mut self) {
        let total: f32 = self.weight[..self.count as usize].iter().sum();
        if total > 0.0 {
            for i in 0..self.count as usize {
                self.weight[i] /= total;
            }
        }
        // Strongest first, so `dominant` and the mottling pick are stable.
        for i in 1..self.count as usize {
            let mut k = i;
            while k > 0 && self.weight[k] > self.weight[k - 1] {
                self.weight.swap(k, k - 1);
                self.layer.swap(k, k - 1);
                k -= 1;
            }
        }
    }
}

/// Resolve which ground materials cover a point, and how much.
///
/// Biome weights select materials; a road/pad corridor overrides them within its width, because a road that
/// is 70 % grass is not a road.
#[must_use]
pub fn splat_at(
    recipe: &TerrainRecipe,
    splines: &SplineIndex,
    height: f32,
    slope: f32,
    moisture: f32,
    x: f32,
    z: f32,
) -> SplatSample {
    let mut out = SplatSample::default();
    let biomes = crate::biome::weights_at(recipe, height, slope, moisture, x, z);
    for (bi, w) in biomes.iter() {
        if let Some(rule) = recipe.biomes.get(bi) {
            let layer = rule
                .material_layer
                .min(recipe.materials.len().saturating_sub(1));
            out.add(layer as u16, w);
        }
    }
    if out.count == 0 && !recipe.materials.is_empty() {
        out.add(0, 1.0);
    }
    if let Some(road) = splines.material_at(x, z) {
        if road < recipe.materials.len() {
            // Full override: the corridor is paved, and the falloff already softened the geometry.
            out = SplatSample::default();
            out.add(road as u16, 1.0);
        }
    }
    out.finish();
    out
}

/// Baked per-chunk textures.
#[derive(Clone, Debug, PartialEq)]
pub struct ChunkTextures {
    /// Edge resolution in texels.
    pub res: u32,
    /// `res² * 4` sRGB-encoded RGBA8 albedo bytes.
    pub albedo_rgba8: Vec<u8>,
    /// `res² * 4` linear RGBA8 tangent-space detail normals, when requested.
    pub normal_rgba8: Option<Vec<u8>>,
    /// Representative roughness for the chunk's PBR factor.
    pub roughness: f32,
    /// Representative metalness.
    pub metallic: f32,
}

impl ChunkTextures {
    /// Heap bytes — what the texture budget accounts.
    #[must_use]
    pub fn bytes(&self) -> usize {
        self.albedo_rgba8.len() + self.normal_rgba8.as_ref().map_or(0, Vec::len)
    }
}

/// sRGB transfer, tabulated over the *fourth root* of the linear value.
///
/// The obvious `powf(l, 1/2.4)` is a libm call whose last bit legitimately differs between platforms, which
/// would make a baked texture's content address platform-dependent — the same terrain would produce
/// different asset handles on a colleague's machine and defeat dedup. Substituting the fourth root as the
/// interpolation domain turns the transfer into a gently curved, bounded-derivative function, and 32
/// intervals then reproduce the exact encode to within 0.2 of an 8-bit step: below one quantization level,
/// so the emitted byte is the same byte, computed with nothing but multiplication and `sqrt`.
#[allow(clippy::excessive_precision)] // generated at f64 precision on purpose; the extra digits are harmless and keep the table auditable against the generator
const SRGB_TABLE: [f32; 33] = [
    0.0,
    0.000_012_32,
    0.000_197_14,
    0.000_998_04,
    0.003_154_3,
    0.007_700_92,
    0.015_968_63,
    0.029_583_85,
    0.049_669_26,
    0.072_371_82,
    0.096_822_39,
    0.122_960_51,
    0.150_733_29,
    0.180_093_93,
    0.211_000_56,
    0.243_415_42,
    0.277_304_18,
    0.312_635_43,
    0.349_380_31,
    0.387_512_11,
    0.427_006_05,
    0.467_839_01,
    0.509_989_4,
    0.553_436_92,
    0.598_162_48,
    0.644_148_09,
    0.691_376_7,
    0.739_832_17,
    0.789_499_15,
    0.840_363_03,
    0.892_409_9,
    0.945_626_47,
    1.0,
];

/// Encode a linear value to an sRGB byte, deterministically.
#[must_use]
pub fn srgb_byte(linear: f32) -> u8 {
    let l = linear.clamp(0.0, 1.0);
    let u = l.sqrt().sqrt();
    let t = u * 32.0;
    let i = (t.floor() as usize).min(31);
    let f = t - i as f32;
    let s = SRGB_TABLE[i] + (SRGB_TABLE[i + 1] - SRGB_TABLE[i]) * f;
    (s * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Per-material mottling: a value-noise field that keeps a flat colour from reading as plastic.
fn mottle(mat: &MaterialLayer, x: f32, z: f32, seed: u64) -> f32 {
    if mat.detail_strength <= 0.0 {
        return 0.0;
    }
    let wl = mat.detail_wavelength_m.max(0.05);
    noise::value_2d(x / wl, z / wl, seed) * mat.detail_strength
}

/// Bake a chunk's albedo (and optionally its detail normal map).
///
/// Reads heights and slopes from the already-computed [`ChunkSamples`] rather than re-evaluating the field,
/// so a bake costs a few thousand cheap lattice evaluations rather than a full second field pass.
#[must_use]
pub fn bake_chunk_textures(
    terrain: &Terrain,
    samples: &ChunkSamples,
    res: u32,
    bake_normals: bool,
) -> ChunkTextures {
    let recipe = terrain.recipe();
    let res = res.clamp(4, 4096);
    let size = samples.size_m();
    let coarse_n = (res / COARSE_STEP).max(1) + 1;

    // Coarse lattice: the expensive inputs, plus the resolved colour and the dominant material.
    let mut coarse_rgb = vec![[0.0f32; 3]; (coarse_n * coarse_n) as usize];
    let mut coarse_dom = vec![0u16; (coarse_n * coarse_n) as usize];
    let mut rough_sum = 0.0f32;
    let mut metal_sum = 0.0f32;
    for cz in 0..coarse_n {
        for cx in 0..coarse_n {
            let u = cx as f32 / (coarse_n - 1).max(1) as f32;
            let v = cz as f32 / (coarse_n - 1).max(1) as f32;
            let wx = samples.origin[0] + u * size;
            let wz = samples.origin[1] + v * size;
            let gx = u * (samples.verts - 1) as f32;
            let gz = v * (samples.verts - 1) as f32;
            let height = samples.height_bilinear(gx, gz);
            let slope = samples.slope_at_grid(gx, gz);
            let moisture = terrain.moisture_at(wx, wz, height);
            let splat = splat_at(recipe, terrain.splines(), height, slope, moisture, wx, wz);
            let mut rgb = [0.0f32; 3];
            let mut rough = 0.0f32;
            let mut metal = 0.0f32;
            for (li, w) in splat.iter() {
                if let Some(m) = recipe.materials.get(li) {
                    for (dst, src) in rgb.iter_mut().zip(&m.albedo) {
                        *dst += src * w;
                    }
                    rough += m.roughness * w;
                    metal += m.metallic * w;
                }
            }
            let idx = (cz * coarse_n + cx) as usize;
            coarse_rgb[idx] = rgb;
            coarse_dom[idx] = splat.dominant().unwrap_or(0) as u16;
            rough_sum += rough;
            metal_sum += metal;
        }
    }
    let coarse_count = (coarse_n * coarse_n) as f32;

    // Fine pass: interpolate the colour, add the per-material mottling.
    let mut albedo = vec![0u8; (res * res * 4) as usize];
    let mut normals = if bake_normals {
        Some(vec![0u8; (res * res * 4) as usize])
    } else {
        None
    };
    let texel_m = size / res as f32;
    for tz in 0..res {
        for tx in 0..res {
            let u = (tx as f32 + 0.5) / res as f32;
            let v = (tz as f32 + 0.5) / res as f32;
            let wx = samples.origin[0] + u * size;
            let wz = samples.origin[1] + v * size;
            let rgb = bilinear_rgb(&coarse_rgb, coarse_n, u, v);
            let dom = nearest_dom(&coarse_dom, coarse_n, u, v) as usize;
            let mat = recipe.materials.get(dom);
            let m = mat.map_or(0.0, |mm| mottle(mm, wx, wz, recipe.seed ^ 0x3A71_C0DE));
            let o = ((tz * res + tx) * 4) as usize;
            for k in 0..3 {
                albedo[o + k] = srgb_byte((rgb[k] * (1.0 + m)).clamp(0.0, 1.0));
            }
            albedo[o + 3] = 255;

            if let Some(nrm) = normals.as_mut() {
                // Detail-only normals: the mesh already carries the macro shape, so encoding it again here
                // would double-shade every slope.
                let (bx, bz) = match mat {
                    None => (0.0, 0.0),
                    Some(mm) => {
                        let e = texel_m.max(0.01);
                        let s = mm.detail_strength * 2.0;
                        let seed = recipe.seed ^ 0x3A71_C0DE;
                        let wl = mm.detail_wavelength_m.max(0.05);
                        let h = |px: f32, pz: f32| noise::value_2d(px / wl, pz / wl, seed) * s;
                        (
                            (h(wx + e, wz) - h(wx - e, wz)) / (2.0 * e),
                            (h(wx, wz + e) - h(wx, wz - e)) / (2.0 * e),
                        )
                    }
                };
                let n = normalize3([-bx, 1.0, -bz]);
                // Tangent space with U along +X and V along +Z, matching the chunk's planar UVs.
                nrm[o] = encode_unit(n[0]);
                nrm[o + 1] = encode_unit(n[2]);
                nrm[o + 2] = encode_unit(n[1]);
                nrm[o + 3] = 255;
            }
        }
    }

    ChunkTextures {
        res,
        albedo_rgba8: albedo,
        normal_rgba8: normals,
        roughness: (rough_sum / coarse_count).clamp(0.0, 1.0),
        metallic: (metal_sum / coarse_count).clamp(0.0, 1.0),
    }
}

fn bilinear_rgb(grid: &[[f32; 3]], n: u32, u: f32, v: f32) -> [f32; 3] {
    let fx = u * (n - 1) as f32;
    let fz = v * (n - 1) as f32;
    let ix = (fx.floor() as u32).min(n - 1);
    let iz = (fz.floor() as u32).min(n - 1);
    let ix1 = (ix + 1).min(n - 1);
    let iz1 = (iz + 1).min(n - 1);
    let tx = fx - ix as f32;
    let tz = fz - iz as f32;
    let g = |x: u32, z: u32| grid[(z * n + x) as usize];
    let (a, b, c, d) = (g(ix, iz), g(ix1, iz), g(ix, iz1), g(ix1, iz1));
    let mut out = [0.0f32; 3];
    for k in 0..3 {
        let top = a[k] + (b[k] - a[k]) * tx;
        let bot = c[k] + (d[k] - c[k]) * tx;
        out[k] = top + (bot - top) * tz;
    }
    out
}

fn nearest_dom(grid: &[u16], n: u32, u: f32, v: f32) -> u16 {
    let ix = ((u * (n - 1) as f32).round() as u32).min(n - 1);
    let iz = ((v * (n - 1) as f32).round() as u32).min(n - 1);
    grid[(iz * n + ix) as usize]
}

fn normalize3(v: [f32; 3]) -> [f32; 3] {
    let l2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if l2 <= 1e-20 {
        return [0.0, 1.0, 0.0];
    }
    let inv = 1.0 / l2.sqrt();
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

fn encode_unit(v: f32) -> u8 {
    ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0 + 0.5) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recipe::{
        BiomeRule, ChunkCoord, Layer, LayerKind, MaterialLayer, SplineDef, SplineKind,
    };
    use std::collections::BTreeMap;

    fn banded_terrain() -> Terrain {
        let mut r = TerrainRecipe {
            world_size_m: 256.0,
            chunk_size_m: 64.0,
            chunk_verts: 33,
            ..TerrainRecipe::default()
        };
        r.layers.push(Layer::new(
            "Hills",
            LayerKind::Fbm {
                amplitude: 40.0,
                wavelength_m: 120.0,
                octaves: 4,
                lacunarity: 2.0,
                gain: 0.5,
                warp_m: 0.0,
                warp_wavelength_m: 1.0,
            },
        ));
        r.materials = vec![
            MaterialLayer::new("Sand", [0.76, 0.70, 0.50], 0.9),
            MaterialLayer::new("Grass", [0.24, 0.36, 0.16], 0.85),
            MaterialLayer::new("Rock", [0.42, 0.40, 0.38], 0.7),
            MaterialLayer::new("Road", [0.12, 0.12, 0.13], 0.6),
        ];
        r.biomes = vec![
            BiomeRule::by_height("Beach", [-100.0, -90.0, 1.0, 6.0], 0),
            BiomeRule::by_height("Meadow", [1.0, 6.0, 25.0, 34.0], 1),
            BiomeRule::by_height("Highland", [25.0, 34.0, 9000.0, 9001.0], 2),
        ];
        Terrain::compile(r, BTreeMap::new()).expect("compile")
    }

    #[test]
    fn srgb_encoding_is_exact_enough_and_monotone() {
        assert_eq!(srgb_byte(0.0), 0);
        assert_eq!(srgb_byte(1.0), 255);
        // Middle grey: linear 0.2140 encodes to sRGB 0.5 → 128.
        assert!((i32::from(srgb_byte(0.2140)) - 128).abs() <= 1);
        let mut prev = 0u8;
        for i in 0..=1000 {
            let b = srgb_byte(i as f32 / 1000.0);
            assert!(b >= prev, "sRGB encode is not monotone at {i}");
            prev = b;
        }
    }

    #[test]
    fn srgb_encoding_matches_the_reference_within_a_quantization_step() {
        // The determinism substitution must not cost accuracy.
        for i in 0..=2000 {
            let l = i as f32 / 2000.0;
            let exact = if l <= 0.003_130_8 {
                12.92 * l
            } else {
                1.055 * l.powf(1.0 / 2.4) - 0.055
            };
            let reference = (exact * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
            let ours = srgb_byte(l);
            assert!(
                i32::from(ours).abs_diff(i32::from(reference)) <= 1,
                "linear {l}: ours {ours} vs reference {reference}"
            );
        }
    }

    #[test]
    fn splat_blends_biome_materials_and_normalizes() {
        let t = banded_terrain();
        let s = splat_at(t.recipe(), t.splines(), 3.5, 0.1, 0.5, 10.0, 10.0);
        assert!(s.count >= 2, "the beach/meadow overlap should blend");
        let total: f32 = s.iter().map(|(_, w)| w).sum();
        assert!((total - 1.0).abs() < 1e-5);
        // Sorted strongest first.
        for i in 1..s.count as usize {
            assert!(s.weight[i - 1] >= s.weight[i]);
        }
    }

    #[test]
    fn a_road_overrides_the_biome_material_completely() {
        let mut r = banded_terrain().recipe().clone();
        r.splines.push(SplineDef {
            name: "Road".into(),
            kind: SplineKind::Road,
            points: vec![[0.0, 0.0, 100.0], [256.0, 0.0, 100.0]],
            width_m: 10.0,
            falloff_m: 4.0,
            material_layer: Some(3),
            ..SplineDef::default()
        });
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let on = splat_at(t.recipe(), t.splines(), 10.0, 0.05, 0.5, 128.0, 100.0);
        assert_eq!(on.count, 1, "a road is not 70 % grass");
        assert_eq!(on.dominant(), Some(3));
        let off = splat_at(t.recipe(), t.splines(), 10.0, 0.05, 0.5, 128.0, 160.0);
        assert_ne!(off.dominant(), Some(3));
    }

    #[test]
    fn bake_produces_the_right_shape_and_plausible_colour() {
        let t = banded_terrain();
        let s = t.sample_chunk(ChunkCoord::new(1, 1)).expect("chunk");
        let tex = bake_chunk_textures(&t, &s, 64, true);
        assert_eq!(tex.res, 64);
        assert_eq!(tex.albedo_rgba8.len(), 64 * 64 * 4);
        assert_eq!(tex.normal_rgba8.as_ref().unwrap().len(), 64 * 64 * 4);
        assert_eq!(tex.bytes(), 64 * 64 * 8);
        // Alpha is opaque everywhere, and no texel is pure black (which would mean an unassigned material).
        for p in tex.albedo_rgba8.as_chunks::<4>().0 {
            assert_eq!(p[3], 255);
            assert!(
                u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]) > 12,
                "black texel"
            );
        }
        assert!(tex.roughness > 0.5 && tex.roughness <= 1.0);
    }

    #[test]
    fn the_bake_is_bit_reproducible() {
        let t = banded_terrain();
        let s = t.sample_chunk(ChunkCoord::new(2, 0)).expect("chunk");
        let a = bake_chunk_textures(&t, &s, 32, true);
        let b = bake_chunk_textures(&t, &s, 32, true);
        assert_eq!(a, b);
    }

    #[test]
    fn detail_normals_are_flat_when_a_material_has_no_mottling() {
        let mut r = banded_terrain().recipe().clone();
        for m in &mut r.materials {
            m.detail_strength = 0.0;
        }
        let t = Terrain::compile(r, BTreeMap::new()).expect("compile");
        let s = t.sample_chunk(ChunkCoord::new(0, 0)).expect("chunk");
        let tex = bake_chunk_textures(&t, &s, 16, true);
        for p in tex.normal_rgba8.as_ref().unwrap().as_chunks::<4>().0 {
            assert_eq!(p[0], 128, "x should be flat");
            assert_eq!(p[1], 128, "y should be flat");
            assert_eq!(p[2], 255, "z should point straight out");
        }
    }

    #[test]
    fn resolution_halves_cleanly_for_distant_chunks() {
        let t = banded_terrain();
        let s = t.sample_chunk(ChunkCoord::new(0, 1)).expect("chunk");
        let hi = bake_chunk_textures(&t, &s, 64, false);
        let lo = bake_chunk_textures(&t, &s, 16, false);
        assert!(
            lo.bytes() * 8 <= hi.bytes(),
            "coarse bake must be far cheaper"
        );
        assert!(lo.normal_rgba8.is_none(), "normals are opt-in");
    }
}
