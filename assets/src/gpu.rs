//! GPU-ready vertex packing — the `wasm32`-portable bridge between an imported [`MeshAsset`] and the
//! renderer (deliverable 6: this render-data prep compiles to `wasm32`; the wgpu calls that consume it
//! are native but use only already-web-proven primitives — an indexed vertex buffer + an instanced
//! draw, no bindless). Pure data, no `wgpu` dependency: `bytemuck` (pure Rust, wasm-clean) makes the
//! vertex `Pod` so the native renderer can `cast_slice` it straight into a buffer.
//!
//! Packing merges an asset's primitives into one interleaved vertex buffer plus one index buffer,
//! partitioned into per-primitive [`SubMesh`] index ranges (a multi-material mesh draws one sub-draw per
//! submesh), bakes each primitive's material base-color/metallic-roughness into the vertex stream, carries
//! the per-vertex UV alongside each submesh's own base-color/metallic-roughness/normal textures for the
//! renderer to sample (M11.2 follow-up — non-bindless: one texture bind group per submesh on the
//! already-per-mesh instance group), and derives smooth normals when the source ships none.

// Index offsets are bounded by MAX_ELEMENTS; the f32 color baking is a display value. The fixed [_;3]
// component loops read clearest as `0..3` (the iterator rewrite is noisier for a 3-vector), and the tests
// compare exact, unmodified float coordinates.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::needless_range_loop,
    clippy::float_cmp
)]

use crate::mesh::MeshAsset;

/// One packed vertex — position, normal (for lighting), a baked RGB base color, the baked
/// metallic-roughness PBR factors (M11.2, ADR-041), and the **UV** for base-color texture sampling (M11.2
/// follow-up). 48 bytes, `std430`/vertex-attribute clean. Matches the renderer's WGSL (`vs_mesh` reads all
/// six attributes; `fs_mesh` samples the base-color texture × the baked factor + a Cook-Torrance BRDF).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Object-space normal (unit-length).
    pub normal: [f32; 3],
    /// Baked base color (linear RGB).
    pub color: [f32; 3],
    /// Baked metalness `[0,1]` (from the primitive's [`crate::mesh::Material`]).
    pub metallic: f32,
    /// Baked perceptual roughness `[0,1]`.
    pub roughness: f32,
    /// Texture coordinate (0 when the source ships none → samples the renderer's 1×1 white dummy = the
    /// baked factor renders unchanged, so an untextured mesh looks exactly as before).
    pub uv: [f32; 2],
}

/// One drawable **submesh** — a contiguous index range with its own material's textures. M11.2 follow-up:
/// a multi-material mesh draws **one submesh per source primitive**, each binding its own base-color /
/// metallic-roughness / normal textures (non-bindless — a separate bind group per submesh), so a model
/// whose parts use different textures no longer renders with just the first one. `None` in a slot ⇒ the
/// renderer binds the matching dummy (1×1 white / flat normal), so the baked vertex factor renders unchanged.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SubMesh {
    /// Offset of this submesh's first index into [`MeshGpu::indices`].
    pub index_offset: u32,
    /// Number of indices this submesh draws.
    pub index_count: u32,
    /// Base-color (albedo) texture (RGBA8, sampled sRGB), if this submesh's material ships one.
    pub base_color_texture: Option<crate::mesh::Texture>,
    /// Metallic-roughness texture (RGBA8 LINEAR; glTF packing roughness=G, metalness=B), if any.
    pub metallic_roughness_texture: Option<crate::mesh::Texture>,
    /// Tangent-space normal map (RGBA8 LINEAR), if any.
    pub normal_texture: Option<crate::mesh::Texture>,
}

/// A mesh ready to upload: one interleaved vertex buffer + one `u32` index buffer, partitioned into
/// [`SubMesh`]es (M11.2 follow-up) — one per source primitive, each carrying its own material textures so a
/// multi-material model renders every part's texture, not just the first. An untextured submesh binds the
/// renderer's dummies (white × the baked factor = the factor — looks exactly as an untextured mesh did).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshGpu {
    /// Interleaved vertices.
    pub vertices: Vec<MeshVertex>,
    /// Triangle-list indices into `vertices`.
    pub indices: Vec<u32>,
    /// Drawable submeshes (one per source primitive): an index range + that primitive's material textures.
    pub submeshes: Vec<SubMesh>,
}

impl MeshGpu {
    /// Pack `asset` (all primitives merged, materials baked to vertex color, smooth normals derived
    /// when absent).
    #[must_use]
    pub fn from_asset(asset: &MeshAsset) -> Self {
        let mut vertices = Vec::with_capacity(asset.vertex_count());
        let mut indices = Vec::with_capacity(asset.index_count());
        let mut submeshes = Vec::with_capacity(asset.primitives.len());

        // The texture a material references in `slot`, cloned (so the packed mesh is self-contained).
        let tex = |mat: Option<&crate::mesh::Material>,
                   pick: fn(&crate::mesh::Material) -> Option<usize>| {
            mat.and_then(pick)
                .and_then(|ti| asset.textures.get(ti).cloned())
        };

        for prim in &asset.primitives {
            let base = vertices.len() as u32;
            let index_offset = indices.len() as u32;
            let mat = asset.materials.get(prim.material);
            let color = mat.map_or([0.8, 0.8, 0.8], |m| {
                [m.base_color[0], m.base_color[1], m.base_color[2]]
            });
            // Bake the primitive's PBR factors per-vertex (matte-dielectric default when material-less),
            // clamped to the valid [0,1] range the BRDF assumes.
            let (metallic, roughness) = mat.map_or((0.0, 0.7), |m| {
                (m.metallic.clamp(0.0, 1.0), m.roughness.clamp(0.0, 1.0))
            });

            let normals = if prim.normals.len() == prim.positions.len() {
                prim.normals.clone()
            } else {
                derive_normals(&prim.positions, &prim.indices)
            };

            for (i, &position) in prim.positions.iter().enumerate() {
                vertices.push(MeshVertex {
                    position,
                    normal: normals.get(i).copied().unwrap_or([0.0, 1.0, 0.0]),
                    color,
                    metallic,
                    roughness,
                    // UV when the source ships one; 0 otherwise (→ the 1×1 white dummy = factor unchanged).
                    uv: prim.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
                });
            }
            // Re-base this primitive's indices into the merged vertex buffer; drop any out-of-range
            // index (a malformed primitive) rather than emitting a bad draw.
            let n = prim.positions.len() as u32;
            for tri in prim.indices.chunks_exact(3) {
                if tri.iter().all(|&i| i < n) {
                    indices.push(base + tri[0]);
                    indices.push(base + tri[1]);
                    indices.push(base + tri[2]);
                }
            }
            // One submesh per primitive: its index range + its own material textures. Skip a primitive that
            // contributed no valid triangles (nothing to draw).
            let index_count = indices.len() as u32 - index_offset;
            if index_count > 0 {
                submeshes.push(SubMesh {
                    index_offset,
                    index_count,
                    base_color_texture: tex(mat, |m| m.base_color_texture),
                    metallic_roughness_texture: tex(mat, |m| m.metallic_roughness_texture),
                    normal_texture: tex(mat, |m| m.normal_texture),
                });
            }
        }

        Self {
            vertices,
            indices,
            submeshes,
        }
    }

    /// Vertex count.
    #[must_use]
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Index count.
    #[must_use]
    pub fn index_count(&self) -> usize {
        self.indices.len()
    }

    /// Recenter to the bounding-box centre and scale to **unit max-extent**, in place. An imported asset's
    /// raw vertices can span hundreds of units (FBX is often authored in cm); the renderer applies the
    /// entity's `Transform.scale` directly to these positions (`v.position * scale` in the shader), so a
    /// "normal-looking" scale like `1.0` would blow a 200-unit mesh up to 200 units. Normalising here makes
    /// the stored geometry ~1 unit so the `scale` field is an intuitive world-size multiplier (`1.0` ≈ one
    /// unit, `2.0` ≈ double) AND the derived collider — which reads these same vertices — stays centred on
    /// the entity and matched to the render. No-op for an empty or degenerate (zero-extent) mesh.
    pub fn normalize_to_unit(&mut self) {
        if self.vertices.is_empty() {
            return;
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for v in &self.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.position[k]);
                hi[k] = hi[k].max(v.position[k]);
            }
        }
        let ext = (0..3).map(|k| hi[k] - lo[k]).fold(0.0_f32, f32::max);
        // Positive guard (NaN- and zero-extent-safe): only normalize a real, non-degenerate mesh.
        if ext > 0.0 {
            let center = [
                (lo[0] + hi[0]) * 0.5,
                (lo[1] + hi[1]) * 0.5,
                (lo[2] + hi[2]) * 0.5,
            ];
            let inv = 1.0 / ext;
            for v in &mut self.vertices {
                for k in 0..3 {
                    v.position[k] = (v.position[k] - center[k]) * inv;
                }
            }
        }
    }

    /// M11.1 (ADR-040) — generate coarser LOD copies of this (already-normalized, ~1-unit) mesh by uniform
    /// **vertex clustering**: vertices sharing a grid cell merge to their centroid, triangles are remapped,
    /// and any that collapse to a line/point are dropped. Each LOD is a SINGLE submesh carrying this mesh's
    /// PRIMARY (first-submesh) textures — texture detail matters little at the distance a LOD kicks in.
    /// `levels` coarsenings, the cell doubling per level, so triangle counts are monotonically
    /// non-increasing. A level that fails to reduce (or empties) is dropped. Deterministic.
    #[must_use]
    pub fn lods(&self, levels: u8) -> Vec<MeshGpu> {
        if self.vertices.is_empty() || self.indices.is_empty() || levels == 0 {
            return Vec::new();
        }
        let primary = self.submeshes.first();
        let base = primary.and_then(|s| s.base_color_texture.clone());
        let mr = primary.and_then(|s| s.metallic_roughness_texture.clone());
        let normal = primary.and_then(|s| s.normal_texture.clone());
        let mut out = Vec::with_capacity(levels as usize);
        for level in 1..=levels {
            // The normalized mesh spans ~1 unit; base cell 0.06, doubling per level → coarser + cheaper.
            let cell = 0.06_f32 * 2.0_f32.powi(i32::from(level) - 1);
            let g = self.clustered(cell, base.clone(), mr.clone(), normal.clone());
            if !g.indices.is_empty() && g.vertices.len() < self.vertices.len() {
                out.push(g);
            }
        }
        out
    }

    /// One vertex-clustered copy at grid `cell` — a single submesh with the given textures (the LOD helper).
    fn clustered(
        &self,
        cell: f32,
        base: Option<crate::mesh::Texture>,
        mr: Option<crate::mesh::Texture>,
        normal: Option<crate::mesh::Texture>,
    ) -> MeshGpu {
        use std::collections::HashMap;
        let key = |p: [f32; 3]| -> (i32, i32, i32) {
            (
                (p[0] / cell).floor() as i32,
                (p[1] / cell).floor() as i32,
                (p[2] / cell).floor() as i32,
            )
        };
        let mut cell_to_new: HashMap<(i32, i32, i32), u32> = HashMap::new();
        let mut verts: Vec<MeshVertex> = Vec::new();
        let mut counts: Vec<f32> = Vec::new();
        let mut old_to_new: Vec<u32> = Vec::with_capacity(self.vertices.len());
        for v in &self.vertices {
            let ni = *cell_to_new.entry(key(v.position)).or_insert_with(|| {
                verts.push(*v);
                counts.push(0.0);
                (verts.len() - 1) as u32
            });
            // Centroid-accumulate position + average the shading attrs (the pushed initial value is folded in
            // by the n==0 step, so no double-count).
            let i = ni as usize;
            let (n, nn) = (counts[i], counts[i] + 1.0);
            for c in 0..3 {
                verts[i].position[c] = (verts[i].position[c] * n + v.position[c]) / nn;
                verts[i].normal[c] = (verts[i].normal[c] * n + v.normal[c]) / nn;
                verts[i].color[c] = (verts[i].color[c] * n + v.color[c]) / nn;
            }
            verts[i].uv[0] = (verts[i].uv[0] * n + v.uv[0]) / nn;
            verts[i].uv[1] = (verts[i].uv[1] * n + v.uv[1]) / nn;
            verts[i].metallic = (verts[i].metallic * n + v.metallic) / nn;
            verts[i].roughness = (verts[i].roughness * n + v.roughness) / nn;
            counts[i] = nn;
            old_to_new.push(ni);
        }
        let mut indices = Vec::new();
        for tri in self.indices.chunks_exact(3) {
            let (a, b, c) = (
                old_to_new[tri[0] as usize],
                old_to_new[tri[1] as usize],
                old_to_new[tri[2] as usize],
            );
            if a != b && b != c && a != c {
                indices.extend_from_slice(&[a, b, c]); // drop triangles that collapsed to a line/point
            }
        }
        let submeshes = if indices.is_empty() {
            Vec::new()
        } else {
            vec![SubMesh {
                index_offset: 0,
                index_count: indices.len() as u32,
                base_color_texture: base,
                metallic_roughness_texture: mr,
                normal_texture: normal,
            }]
        };
        MeshGpu {
            vertices: verts,
            indices,
            submeshes,
        }
    }
}

/// Smooth per-vertex normals: accumulate each triangle's face normal onto its vertices, then
/// normalize. Used when a primitive ships no normals.
fn derive_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut acc = vec![[0.0f32; 3]; positions.len()];
    for tri in indices.chunks_exact(3) {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let (Some(&p0), Some(&p1), Some(&p2)) =
            (positions.get(i0), positions.get(i1), positions.get(i2))
        else {
            continue;
        };
        let face = cross(sub(p1, p0), sub(p2, p0));
        for &vi in &[i0, i1, i2] {
            acc[vi] = add(acc[vi], face);
        }
    }
    for n in &mut acc {
        *n = normalize(*n, [0.0, 1.0, 0.0]);
    }
    acc
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normalize(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len > 1e-8 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vtx(p: [f32; 3]) -> MeshVertex {
        MeshVertex {
            position: p,
            normal: [0.0, 1.0, 0.0],
            color: [0.8, 0.8, 0.8],
            metallic: 0.0,
            roughness: 0.7,
            uv: [0.0, 0.0],
        }
    }

    #[test]
    fn normalize_to_unit_recenters_and_unit_scales() {
        // A big, off-centre box (like an FBX in cm): x spans 200, centred at (100, 50, 10).
        let mut m = MeshGpu {
            vertices: vec![vtx([0.0, 0.0, 0.0]), vtx([200.0, 100.0, 20.0])],
            indices: vec![],
            ..Default::default()
        };
        m.normalize_to_unit();
        // Recentred about the bbox centre, scaled so the max axis (x, span 200) becomes 1.0.
        let (a, b) = (m.vertices[0].position, m.vertices[1].position);
        assert!(
            (a[0] - (-0.5)).abs() < 1e-5 && (b[0] - 0.5).abs() < 1e-5,
            "x spans [-0.5,0.5]"
        );
        // Aspect preserved: y span 100 → 0.5, z span 20 → 0.1 (same divisor as x).
        assert!((b[1] - a[1] - 0.5).abs() < 1e-5, "y span 0.5");
        assert!((b[2] - a[2] - 0.1).abs() < 1e-5, "z span 0.1 (aspect kept)");
        // Centred on the origin.
        assert!(
            a.iter().zip(&b).all(|(lo, hi)| (lo + hi).abs() < 1e-5),
            "centred"
        );
    }

    #[test]
    fn normalize_to_unit_is_a_noop_for_degenerate_or_empty() {
        let mut empty = MeshGpu::default();
        empty.normalize_to_unit(); // no panic
        assert!(empty.vertices.is_empty());

        let mut point = MeshGpu {
            vertices: vec![vtx([3.0, 3.0, 3.0]), vtx([3.0, 3.0, 3.0])],
            indices: vec![],
            ..Default::default()
        };
        point.normalize_to_unit(); // zero extent → unchanged (no divide-by-zero)
        assert_eq!(point.vertices[0].position, [3.0, 3.0, 3.0]);
    }

    #[test]
    fn from_asset_bakes_metallic_roughness_per_vertex() {
        use crate::mesh::{Material, MeshAsset, Primitive};
        let tri = Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2],
            material: 0,
            joints: Vec::new(),
            weights: Vec::new(),
        };
        // A polished metal (the glTF PBR factors the importer now keeps) bakes onto every vertex.
        let asset = MeshAsset {
            name: "metal".into(),
            primitives: vec![tri],
            materials: vec![Material {
                base_color: [0.9, 0.8, 0.2, 1.0],
                metallic: 0.95,
                roughness: 0.15,
                base_color_texture: None,
                metallic_roughness_texture: None,
                normal_texture: None,
            }],
            textures: Vec::new(),
            skeleton: None,
        };
        let gpu = MeshGpu::from_asset(&asset);
        assert!(!gpu.vertices.is_empty());
        assert!(gpu
            .vertices
            .iter()
            .all(|v| (v.metallic - 0.95).abs() < 1e-6 && (v.roughness - 0.15).abs() < 1e-6));
    }

    #[test]
    fn from_asset_material_less_primitive_is_matte_dielectric() {
        use crate::mesh::{MeshAsset, Primitive};
        // No materials → the matte default (non-metal, fairly rough) so it reads like the prior shading.
        let asset = MeshAsset {
            name: "bare".into(),
            primitives: vec![Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: Vec::new(),
                uvs: Vec::new(),
                indices: vec![0, 1, 2],
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            }],
            materials: Vec::new(),
            textures: Vec::new(),
            skeleton: None,
        };
        let v = MeshGpu::from_asset(&asset).vertices[0];
        assert_eq!(v.metallic, 0.0);
        assert!((v.roughness - 0.7).abs() < 1e-6);
    }

    #[test]
    fn from_asset_carries_uv_and_the_base_color_mr_and_normal_textures() {
        use crate::mesh::{Material, MeshAsset, Primitive, Texture};
        let tex = |w: u32, h: u32| Texture {
            width: w,
            height: h,
            rgba8: vec![255; (w * h * 4) as usize],
        };
        let asset = MeshAsset {
            name: "tex".into(),
            primitives: vec![Primitive {
                positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                normals: Vec::new(),
                uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
                indices: vec![0, 1, 2],
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            }],
            materials: vec![Material {
                base_color: [1.0, 1.0, 1.0, 1.0],
                metallic: 0.0,
                roughness: 0.7,
                base_color_texture: Some(0),
                metallic_roughness_texture: Some(1),
                normal_texture: Some(2),
            }],
            // Distinct sizes so we can assert each slot maps to the right texture.
            textures: vec![tex(2, 2), tex(4, 1), tex(1, 4)],
            skeleton: None,
        };
        let gpu = MeshGpu::from_asset(&asset);
        // The per-vertex UV flows through (so the fragment shader can sample the textures).
        assert_eq!(gpu.vertices[1].uv, [1.0, 0.0], "the second vertex's UV");
        // One primitive → one submesh, carrying all three texture slots mapped to the right textures.
        assert_eq!(gpu.submeshes.len(), 1, "one submesh for the one primitive");
        let sm = &gpu.submeshes[0];
        assert_eq!(sm.index_count, 3, "the triangle's three indices");
        let b = sm.base_color_texture.as_ref().expect("base-color carried");
        assert_eq!((b.width, b.height), (2, 2));
        let mr = sm
            .metallic_roughness_texture
            .as_ref()
            .expect("metallic-roughness carried");
        assert_eq!((mr.width, mr.height), (4, 1));
        let nrm = sm.normal_texture.as_ref().expect("normal map carried");
        assert_eq!((nrm.width, nrm.height), (1, 4));
    }

    #[test]
    fn from_asset_keeps_distinct_textures_per_submesh() {
        use crate::mesh::{Material, MeshAsset, Primitive, Texture};
        // M11.2 follow-up — a MULTI-MATERIAL mesh: two primitives, each with its OWN base-color texture.
        // Each must become its own submesh binding its own texture (the prior code used only the first).
        let tex = |w: u32, h: u32| Texture {
            width: w,
            height: h,
            rgba8: vec![255; (w * h * 4) as usize],
        };
        let prim = |mat: usize| Primitive {
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            normals: Vec::new(),
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            indices: vec![0, 1, 2],
            material: mat,
            joints: Vec::new(),
            weights: Vec::new(),
        };
        let mat = |slot: usize| Material {
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 0.7,
            base_color_texture: Some(slot),
            metallic_roughness_texture: None,
            normal_texture: None,
        };
        let asset = MeshAsset {
            name: "multi".into(),
            primitives: vec![prim(0), prim(1)],
            materials: vec![mat(0), mat(1)],
            // Distinct sizes so we can tell which texture each submesh kept.
            textures: vec![tex(2, 2), tex(8, 8)],
            skeleton: None,
        };
        let gpu = MeshGpu::from_asset(&asset);
        assert_eq!(gpu.submeshes.len(), 2, "one submesh per primitive");
        // Contiguous, non-overlapping index ranges covering the whole buffer.
        assert_eq!(gpu.submeshes[0].index_offset, 0);
        assert_eq!(gpu.submeshes[0].index_count, 3);
        assert_eq!(gpu.submeshes[1].index_offset, 3);
        assert_eq!(gpu.submeshes[1].index_count, 3);
        // Each submesh kept ITS OWN base-color texture (2×2 vs 8×8), not just the first.
        let a = gpu.submeshes[0]
            .base_color_texture
            .as_ref()
            .expect("sm0 tex");
        let b = gpu.submeshes[1]
            .base_color_texture
            .as_ref()
            .expect("sm1 tex");
        assert_eq!((a.width, a.height), (2, 2));
        assert_eq!((b.width, b.height), (8, 8));
    }

    #[test]
    fn lods_cluster_decimate_and_reduce_triangles() {
        // A dense flat grid in [0,1]² (finer than the LOD-1 cell): clustering must merge vertices + reduce
        // the triangle count, monotonically, and emit a single textured submesh per LOD.
        const N: usize = 40;
        let mut m = MeshGpu::default();
        for j in 0..N {
            for i in 0..N {
                m.vertices.push(MeshVertex {
                    position: [i as f32 / (N - 1) as f32, j as f32 / (N - 1) as f32, 0.0],
                    normal: [0.0, 0.0, 1.0],
                    color: [0.8, 0.8, 0.8],
                    metallic: 0.0,
                    roughness: 0.7,
                    uv: [0.0, 0.0],
                });
            }
        }
        let idx = |i: usize, j: usize| (j * N + i) as u32;
        for j in 0..N - 1 {
            for i in 0..N - 1 {
                m.indices
                    .extend_from_slice(&[idx(i, j), idx(i + 1, j), idx(i, j + 1)]);
                m.indices
                    .extend_from_slice(&[idx(i + 1, j), idx(i + 1, j + 1), idx(i, j + 1)]);
            }
        }
        m.submeshes.push(SubMesh {
            index_offset: 0,
            index_count: m.indices.len() as u32,
            ..Default::default()
        });
        let full_tris = m.indices.len() / 3;

        let lods = m.lods(2);
        assert_eq!(lods.len(), 2, "two LOD levels generated");
        assert!(
            lods[0].vertices.len() < m.vertices.len(),
            "LOD-1 merged vertices"
        );
        let l0 = lods[0].indices.len() / 3;
        let l1 = lods[1].indices.len() / 3;
        assert!(
            l0 < full_tris,
            "LOD-1 has fewer triangles than the full mesh"
        );
        assert!(l1 <= l0, "LOD-2 is no finer than LOD-1 (monotonic)");
        assert_eq!(lods[0].submeshes.len(), 1, "a LOD is a single submesh");
    }
}
