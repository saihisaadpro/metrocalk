//! Level-of-detail generation (M11.1) — an imported mesh is conditioned into ≥2 coarser LODs the renderer
//! selects by distance, so a dense imported asset holds the frame budget on **min-spec** (product
//! principle 3). The LOD count + coarseness is the **cost knob** ([`LodConfig`]).
//!
//! **Deterministic + dependency-free by construction** (the M9.5 / `baby_shark` lesson — audit every dep for
//! determinism/wasm/min-spec, and *own it* when a dep can't promise all three). Rather than take an FFI or
//! rayon-threaded simplifier, this is a project-owned **vertex-clustering** decimator: quantize positions to
//! a uniform grid (the cell size = the cost knob), merge co-cell vertices to their centroid, remap indices,
//! and drop the triangles whose corners collapsed together. New indices are assigned in **first-encounter
//! order** (not from any map's iteration order), and the cell map is a [`BTreeMap`] (no RNG, unlike a
//! `HashMap`'s seed source — which also keeps the crate `wasm32`-clean), so the output is **bit-identical
//! across runs and machines**. Quality is below a quadric-error-metric (QEM) collapse; a `meshopt`/QEM
//! implementation can slot in **behind this same [`LodGenerator`] trait** as a quality upgrade (a named
//! seam) without touching the store or the renderer.

// This module is inherently numeric-conversion code: positions are `f32`, grid cell keys are integer
// quantizations, and centroids divide f64 sums back to f32 — all deliberate, bounded conversions.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::many_single_char_names
)]

use std::collections::BTreeMap;

use crate::mesh::{MeshAsset, Primitive};

/// How many LODs to generate and how coarse the first one is. The coarseness **doubles** per level, so the
/// triangle count is monotonically non-increasing. `base_fraction` is the LOD-1 grid cell as a fraction of
/// the mesh's bounding diagonal — larger ⇒ coarser ⇒ cheaper (the min-spec knob).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LodConfig {
    /// Number of reduced LODs to produce (LOD-0 = the full asset, not stored here). ≥1.
    pub levels: u8,
    /// LOD-1 grid cell as a fraction of the bounding diagonal. LOD-`n` uses `base_fraction · 2^(n-1)`.
    pub base_fraction: f32,
}

impl Default for LodConfig {
    fn default() -> Self {
        Self {
            levels: 2,
            base_fraction: 0.02,
        }
    }
}

/// One generated level of detail — a decimated copy of the asset's geometry. Reuses [`Primitive`] (with
/// empty normals/uvs/skin: normals are re-derived by the GPU packer, and LODs aren't skinned) so a LOD
/// packs through the **same** [`crate::gpu::MeshGpu::from_asset`]-style path as the full mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshLod {
    /// LOD level (1 = the first reduction; higher = coarser).
    pub level: u8,
    /// The decimated primitives (material index preserved; normals/uvs/skin dropped).
    pub primitives: Vec<Primitive>,
}

impl MeshLod {
    /// Total triangle count across this LOD's primitives.
    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.primitives.iter().map(|p| p.indices.len() / 3).sum()
    }

    /// Total vertex count across this LOD's primitives.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.primitives.iter().map(|p| p.positions.len()).sum()
    }
}

/// A project-owned LOD generator. A different algorithm (e.g. a QEM/`meshopt` collapse) slots in behind
/// this trait without touching the store or the renderer.
pub trait LodGenerator {
    /// Generate `cfg.levels` reduced LODs for `asset`, coarsest last. Deterministic: same input → same LODs.
    fn generate(&self, asset: &MeshAsset, cfg: &LodConfig) -> Vec<MeshLod>;
}

/// The default deterministic vertex-clustering decimator (uniform grid, centroid merge).
#[derive(Clone, Copy, Debug, Default)]
pub struct GridClusterLod;

impl LodGenerator for GridClusterLod {
    fn generate(&self, asset: &MeshAsset, cfg: &LodConfig) -> Vec<MeshLod> {
        let b = asset.bounds();
        // Bounding diagonal; a degenerate (empty / zero-size) asset yields no LODs (nothing to reduce).
        let diag = {
            let d = [
                b.max[0] - b.min[0],
                b.max[1] - b.min[1],
                b.max[2] - b.min[2],
            ];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        // `diag > 0.0` is false for both 0 and NaN (a degenerate asset) — extract to a bool so the guard
        // isn't a negated comparison on a partially-ordered type (clippy `neg_cmp_op_on_partial_ord`).
        let diag_positive = diag > 0.0;
        if cfg.levels == 0 || !diag_positive || asset.primitives.is_empty() {
            return Vec::new();
        }
        let mut lods = Vec::with_capacity(cfg.levels as usize);
        for level in 1..=cfg.levels {
            // Cell doubles per level → monotonically coarser.
            let cell = diag * cfg.base_fraction * 2.0_f32.powi(i32::from(level) - 1);
            let cell_positive = cell > 0.0;
            if !cell_positive {
                continue;
            }
            let primitives: Vec<Primitive> = asset
                .primitives
                .iter()
                .map(|p| cluster_decimate(p, cell))
                .filter(|p| !p.indices.is_empty())
                .collect();
            lods.push(MeshLod { level, primitives });
        }
        lods
    }
}

/// Vertex-cluster one primitive at grid `cell`: positions in the same cell merge to their centroid; indices
/// remap; triangles whose corners collapsed together are dropped. Deterministic (first-encounter index
/// assignment; `BTreeMap` cell map → no RNG, `wasm32`-clean).
fn cluster_decimate(prim: &Primitive, cell: f32) -> Primitive {
    let cell = f64::from(cell);
    let mut cell_to_new: BTreeMap<[i64; 3], u32> = BTreeMap::new();
    // Per cluster: running centroid sum + count (f64 — order within a cell is the fixed vertex order, but
    // f64 keeps the merge robust across very dense cells).
    let mut sums: Vec<[f64; 3]> = Vec::new();
    let mut counts: Vec<u32> = Vec::new();
    let mut remap: Vec<u32> = Vec::with_capacity(prim.positions.len());
    for p in &prim.positions {
        let key = [
            (f64::from(p[0]) / cell).floor() as i64,
            (f64::from(p[1]) / cell).floor() as i64,
            (f64::from(p[2]) / cell).floor() as i64,
        ];
        let idx = *cell_to_new.entry(key).or_insert_with(|| {
            let i = u32::try_from(sums.len()).unwrap_or(u32::MAX);
            sums.push([0.0; 3]);
            counts.push(0);
            i
        });
        let s = &mut sums[idx as usize];
        s[0] += f64::from(p[0]);
        s[1] += f64::from(p[1]);
        s[2] += f64::from(p[2]);
        counts[idx as usize] += 1;
        remap.push(idx);
    }
    let positions: Vec<[f32; 3]> = sums
        .iter()
        .zip(&counts)
        .map(|(s, &c)| {
            let c = f64::from(c.max(1));
            [(s[0] / c) as f32, (s[1] / c) as f32, (s[2] / c) as f32]
        })
        .collect();
    let mut indices: Vec<u32> = Vec::new();
    for tri in prim.indices.chunks_exact(3) {
        let (a, b, c) = (
            remap[tri[0] as usize],
            remap[tri[1] as usize],
            remap[tri[2] as usize],
        );
        // Drop a triangle whose corners collapsed into the same cluster(s) — it's degenerate (zero area).
        if a != b && b != c && a != c {
            indices.extend_from_slice(&[a, b, c]);
        }
    }
    Primitive {
        positions,
        normals: Vec::new(), // re-derived by the packer (flat normals), like a normal-less import
        uvs: Vec::new(),
        indices,
        material: prim.material,
        joints: Vec::new(),
        weights: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{Material, Primitive};

    /// A tessellated unit plane: `n`×`n` vertices, `2·(n-1)²` triangles. Dense enough that clustering
    /// genuinely reduces it.
    fn grid_plane(n: usize) -> MeshAsset {
        let mut positions = Vec::new();
        for j in 0..n {
            for i in 0..n {
                let x = i as f32 / (n - 1) as f32 - 0.5;
                let z = j as f32 / (n - 1) as f32 - 0.5;
                positions.push([x, 0.0, z]);
            }
        }
        let mut indices = Vec::new();
        for j in 0..n - 1 {
            for i in 0..n - 1 {
                let a = (j * n + i) as u32;
                let b = a + 1;
                let c = a + n as u32;
                let d = c + 1;
                indices.extend_from_slice(&[a, b, c, b, d, c]);
            }
        }
        MeshAsset {
            name: "grid".into(),
            primitives: vec![Primitive {
                positions,
                normals: Vec::new(),
                uvs: Vec::new(),
                indices,
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            }],
            materials: vec![Material {
                base_color: [1.0; 4],
                base_color_texture: None,
            }],
            textures: Vec::new(),
            skeleton: None,
        }
    }

    #[test]
    fn generates_at_least_two_lods_that_monotonically_reduce() {
        let asset = grid_plane(33); // 1089 verts, 2048 tris
        let cfg = LodConfig {
            levels: 2,
            base_fraction: 0.06,
        };
        let lods = GridClusterLod.generate(&asset, &cfg);
        assert_eq!(lods.len(), 2, "two LODs requested → two produced");
        assert_eq!(lods[0].level, 1);
        assert_eq!(lods[1].level, 2);
        let full = asset.triangle_count();
        assert!(
            lods[0].triangle_count() < full,
            "LOD1 reduces the full mesh"
        );
        assert!(
            lods[1].triangle_count() <= lods[0].triangle_count(),
            "LOD2 (coarser) ≤ LOD1: {} vs {}",
            lods[1].triangle_count(),
            lods[0].triangle_count()
        );
        // Both LODs are non-empty and index-valid.
        for lod in &lods {
            for p in &lod.primitives {
                assert!(!p.indices.is_empty());
                assert!(p.indices.iter().all(|&i| (i as usize) < p.positions.len()));
                assert_eq!(p.indices.len() % 3, 0, "still a triangle list");
            }
        }
    }

    #[test]
    fn lod_generation_is_deterministic() {
        // The reload contract: re-conditioning the same asset reproduces byte-identical LODs.
        let asset = grid_plane(20);
        let cfg = LodConfig::default();
        let a = GridClusterLod.generate(&asset, &cfg);
        let b = GridClusterLod.generate(&asset, &cfg);
        assert_eq!(a, b, "LOD generation is deterministic across runs");
    }

    #[test]
    fn a_coarser_knob_yields_fewer_triangles() {
        // The cost knob: a larger base_fraction (coarser) produces a cheaper LOD1 — the min-spec lever.
        let asset = grid_plane(33);
        let fine = GridClusterLod.generate(
            &asset,
            &LodConfig {
                levels: 1,
                base_fraction: 0.03,
            },
        );
        let coarse = GridClusterLod.generate(
            &asset,
            &LodConfig {
                levels: 1,
                base_fraction: 0.12,
            },
        );
        assert!(
            coarse[0].triangle_count() < fine[0].triangle_count(),
            "a coarser knob is cheaper: {} < {}",
            coarse[0].triangle_count(),
            fine[0].triangle_count()
        );
    }

    #[test]
    fn a_degenerate_asset_yields_no_lods() {
        // A zero-size (single-point) asset has nothing to reduce — no panic, no LODs.
        let mut asset = grid_plane(2);
        for p in &mut asset.primitives[0].positions {
            *p = [0.0, 0.0, 0.0];
        }
        assert!(GridClusterLod
            .generate(&asset, &LodConfig::default())
            .is_empty());
    }
}
