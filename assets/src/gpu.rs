//! GPU-ready vertex packing — the `wasm32`-portable bridge between an imported [`MeshAsset`] and the
//! renderer (deliverable 6: this render-data prep compiles to `wasm32`; the wgpu calls that consume it
//! are native but use only already-web-proven primitives — an indexed vertex buffer + an instanced
//! draw, no bindless). Pure data, no `wgpu` dependency: `bytemuck` (pure Rust, wasm-clean) makes the
//! vertex `Pod` so the native renderer can `cast_slice` it straight into a buffer.
//!
//! Packing merges an asset's primitives into one interleaved vertex buffer + one index buffer (a mesh
//! draws as a single indexed call), bakes each primitive's material base-color/metallic-roughness into the
//! vertex stream, carries the per-vertex UV + the primary base-color texture for the renderer to sample
//! (M11.2 follow-up — non-bindless: one texture bind group per mesh on the already-per-mesh instance group),
//! and derives smooth normals when the source ships none.

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

/// A mesh ready to upload: one interleaved vertex buffer + one `u32` index buffer, plus the optional
/// base-color texture the renderer uploads + samples (M11.2 follow-up — single primary texture per mesh;
/// multi-texture meshes use the first, a documented limitation). `None` ⇒ the renderer binds a 1×1 white
/// dummy so `fs_mesh` can always sample (white × the baked factor = the factor — untextured looks as before).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshGpu {
    /// Interleaved vertices.
    pub vertices: Vec<MeshVertex>,
    /// Triangle-list indices into `vertices`.
    pub indices: Vec<u32>,
    /// The primary base-color (albedo) texture (RGBA8), if the asset ships one.
    pub base_color_texture: Option<crate::mesh::Texture>,
    /// The primary metallic-roughness texture (RGBA8; glTF packing roughness=G, metalness=B), if any.
    pub metallic_roughness_texture: Option<crate::mesh::Texture>,
    /// The primary tangent-space normal map (RGBA8), if any.
    pub normal_texture: Option<crate::mesh::Texture>,
}

impl MeshGpu {
    /// Pack `asset` (all primitives merged, materials baked to vertex color, smooth normals derived
    /// when absent).
    #[must_use]
    pub fn from_asset(asset: &MeshAsset) -> Self {
        let mut vertices = Vec::with_capacity(asset.vertex_count());
        let mut indices = Vec::with_capacity(asset.index_count());

        for prim in &asset.primitives {
            let base = vertices.len() as u32;
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
        }

        // The primary textures: the first primitive whose material references each slot (single texture
        // per mesh — a multi-texture mesh uses the first, a documented limitation). Cloned so the packed
        // mesh is self-contained for the renderer to upload.
        let tex_of = |pick: fn(&crate::mesh::Material) -> Option<usize>| {
            asset.primitives.iter().find_map(|p| {
                asset
                    .materials
                    .get(p.material)
                    .and_then(pick)
                    .and_then(|ti| asset.textures.get(ti).cloned())
            })
        };

        Self {
            vertices,
            indices,
            base_color_texture: tex_of(|m| m.base_color_texture),
            metallic_roughness_texture: tex_of(|m| m.metallic_roughness_texture),
            normal_texture: tex_of(|m| m.normal_texture),
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
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
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
            base_color_texture: None,
            metallic_roughness_texture: None,
            normal_texture: None,
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
        // Each slot is carried for the renderer to upload + sample, mapped to the right texture.
        let b = gpu.base_color_texture.expect("base-color carried");
        assert_eq!((b.width, b.height), (2, 2));
        let mr = gpu
            .metallic_roughness_texture
            .expect("metallic-roughness carried");
        assert_eq!((mr.width, mr.height), (4, 1));
        let nrm = gpu.normal_texture.expect("normal map carried");
        assert_eq!((nrm.width, nrm.height), (1, 4));
    }
}
