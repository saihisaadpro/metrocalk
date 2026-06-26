//! GPU-ready vertex packing — the `wasm32`-portable bridge between an imported [`MeshAsset`] and the
//! renderer (deliverable 6: this render-data prep compiles to `wasm32`; the wgpu calls that consume it
//! are native but use only already-web-proven primitives — an indexed vertex buffer + an instanced
//! draw, no bindless). Pure data, no `wgpu` dependency: `bytemuck` (pure Rust, wasm-clean) makes the
//! vertex `Pod` so the native renderer can `cast_slice` it straight into a buffer.
//!
//! Packing merges an asset's primitives into one interleaved vertex buffer + one index buffer (a mesh
//! draws as a single indexed call), bakes each primitive's material base-color into the vertex color
//! (so the non-bindless path needs no per-material bind group this milestone — texture sampling is the
//! next render increment), and derives smooth normals when the source ships none.

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

/// One packed vertex — position, normal (for lighting), and a baked RGB color (the source material's
/// base-color factor). 36 bytes, `std430`/vertex-attribute clean. Matches the renderer's WGSL.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MeshVertex {
    /// Object-space position.
    pub position: [f32; 3],
    /// Object-space normal (unit-length).
    pub normal: [f32; 3],
    /// Baked base color (linear RGB).
    pub color: [f32; 3],
}

/// A mesh ready to upload: one interleaved vertex buffer + one `u32` index buffer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeshGpu {
    /// Interleaved vertices.
    pub vertices: Vec<MeshVertex>,
    /// Triangle-list indices into `vertices`.
    pub indices: Vec<u32>,
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
            let color = asset
                .materials
                .get(prim.material)
                .map_or([0.8, 0.8, 0.8], |m| {
                    [m.base_color[0], m.base_color[1], m.base_color[2]]
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

        Self { vertices, indices }
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
        }
    }

    #[test]
    fn normalize_to_unit_recenters_and_unit_scales() {
        // A big, off-centre box (like an FBX in cm): x spans 200, centred at (100, 50, 10).
        let mut m = MeshGpu {
            vertices: vec![vtx([0.0, 0.0, 0.0]), vtx([200.0, 100.0, 20.0])],
            indices: vec![],
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
        };
        point.normalize_to_unit(); // zero extent → unchanged (no divide-by-zero)
        assert_eq!(point.vertices[0].position, [3.0, 3.0, 3.0]);
    }
}
