//! Demo asset generators — small, **own** (no third-party licensing) glTF/glb meshes, built by a
//! dependency-free in-memory `.glb` encoder. These are the provenance for the checked-in fixtures the
//! shell imports (`editor-shell/assets/*.glb`) and the substrate the importer round-trip tests run
//! against. Real glTF bytes through the real importer — just authored by us rather than sourced.
//!
//! `healthbar` ships explicit normals + two materials (a dark frame + a red fill — multi-primitive,
//! multi-material); `prop` is an octahedron with **no** normals (exercising the packer's smooth-normal
//! derivation); `textured_quad` embeds a PNG base-color texture (exercising the importer's
//! texture-decode path). Geometry is tiny (tens–hundreds of verts) — these prove the mechanism, not an
//! art library.

// Demo geometry: literal coordinates + index lists, and f32→LE byte casts for the buffer. Truncation
// of small counts to u16 indices is intentional and bounded by the hand-authored geometry.
#![allow(clippy::cast_possible_truncation)]

use std::fmt::Write as _;

/// One primitive to encode: positions, optional normals, triangle indices, a base color, and an
/// optional base-color texture (index into the builder's textures).
struct PrimSpec {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    indices: Vec<u16>,
    base_color: [f32; 4],
    texture: Option<usize>,
}

// glTF component / target / type constants.
const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// Accumulates the BIN buffer + the bufferView/accessor JSON fragments as primitives are added.
#[derive(Default)]
struct GlbBuilder {
    bin: Vec<u8>,
    views: Vec<String>,
    accessors: Vec<String>,
}

impl GlbBuilder {
    fn align4(&mut self) {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
    }

    fn add_view(&mut self, data: &[u8], target: Option<u32>) -> usize {
        self.align4();
        let offset = self.bin.len();
        self.bin.extend_from_slice(data);
        let idx = self.views.len();
        let target = target.map_or(String::new(), |t| format!(",\"target\":{t}"));
        self.views.push(format!(
            "{{\"buffer\":0,\"byteOffset\":{offset},\"byteLength\":{}{target}}}",
            data.len()
        ));
        idx
    }

    fn add_accessor(
        &mut self,
        view: usize,
        component_type: u32,
        count: usize,
        ty: &str,
        minmax: Option<(String, String)>,
    ) -> usize {
        let idx = self.accessors.len();
        let mm = minmax.map_or(String::new(), |(lo, hi)| {
            format!(",\"min\":{lo},\"max\":{hi}")
        });
        self.accessors.push(format!(
            "{{\"bufferView\":{view},\"componentType\":{component_type},\"count\":{count},\"type\":\"{ty}\"{mm}}}"
        ));
        idx
    }
}

fn f32_le(values: impl IntoIterator<Item = f32>) -> Vec<u8> {
    let mut out = Vec::new();
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

fn vec3_minmax(positions: &[[f32; 3]]) -> (String, String) {
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for p in positions {
        for i in 0..3 {
            lo[i] = lo[i].min(p[i]);
            hi[i] = hi[i].max(p[i]);
        }
    }
    let fmt = |a: [f32; 3]| format!("[{},{},{}]", a[0], a[1], a[2]);
    (fmt(lo), fmt(hi))
}

/// Build a `.glb` from primitives (+ optional embedded PNG textures). `textures[i]` is `(png_bytes)`;
/// a primitive's `texture` field indexes into it.
fn build_glb(name: &str, prims: &[PrimSpec], textures: &[Vec<u8>]) -> Vec<u8> {
    let mut b = GlbBuilder::default();
    let mut prim_json = Vec::new();
    let mut material_json = Vec::new();

    for (mi, prim) in prims.iter().enumerate() {
        // POSITION
        let pos_bytes = f32_le(prim.positions.iter().flat_map(|p| p.iter().copied()));
        let pos_view = b.add_view(&pos_bytes, Some(ARRAY_BUFFER));
        let pos_acc = b.add_accessor(
            pos_view,
            FLOAT,
            prim.positions.len(),
            "VEC3",
            Some(vec3_minmax(&prim.positions)),
        );
        let mut attrs = format!("\"POSITION\":{pos_acc}");
        // NORMAL (optional)
        if let Some(normals) = &prim.normals {
            let n_bytes = f32_le(normals.iter().flat_map(|p| p.iter().copied()));
            let n_view = b.add_view(&n_bytes, Some(ARRAY_BUFFER));
            let n_acc = b.add_accessor(n_view, FLOAT, normals.len(), "VEC3", None);
            let _ = write!(attrs, ",\"NORMAL\":{n_acc}");
        }
        // indices (u16)
        let mut idx_bytes = Vec::new();
        for &i in &prim.indices {
            idx_bytes.extend_from_slice(&i.to_le_bytes());
        }
        let idx_view = b.add_view(&idx_bytes, Some(ELEMENT_ARRAY_BUFFER));
        let idx_acc = b.add_accessor(idx_view, UNSIGNED_SHORT, prim.indices.len(), "SCALAR", None);

        prim_json.push(format!(
            "{{\"attributes\":{{{attrs}}},\"indices\":{idx_acc},\"material\":{mi},\"mode\":4}}"
        ));

        let [r, g, bl, a] = prim.base_color;
        let tex = prim.texture.map_or(String::new(), |t| {
            format!(",\"baseColorTexture\":{{\"index\":{t}}}")
        });
        material_json.push(format!(
            "{{\"pbrMetallicRoughness\":{{\"baseColorFactor\":[{r},{g},{bl},{a}]{tex}}}}}"
        ));
    }

    // Embedded PNG textures (as bufferView images).
    let mut images_json = Vec::new();
    let mut textures_json = Vec::new();
    let mut samplers_json = Vec::new();
    if !textures.is_empty() {
        samplers_json.push("{}".to_string());
        for (ti, png) in textures.iter().enumerate() {
            let view = b.add_view(png, None);
            images_json.push(format!(
                "{{\"bufferView\":{view},\"mimeType\":\"image/png\"}}"
            ));
            textures_json.push(format!("{{\"sampler\":0,\"source\":{ti}}}"));
        }
    }

    b.align4();
    let buffer_len = b.bin.len();

    let mut json = String::from("{\"asset\":{\"version\":\"2.0\"}");
    let _ = write!(json, ",\"buffers\":[{{\"byteLength\":{buffer_len}}}]");
    let _ = write!(json, ",\"bufferViews\":[{}]", b.views.join(","));
    let _ = write!(json, ",\"accessors\":[{}]", b.accessors.join(","));
    let _ = write!(json, ",\"materials\":[{}]", material_json.join(","));
    if !images_json.is_empty() {
        let _ = write!(json, ",\"images\":[{}]", images_json.join(","));
        let _ = write!(json, ",\"samplers\":[{}]", samplers_json.join(","));
        let _ = write!(json, ",\"textures\":[{}]", textures_json.join(","));
    }
    let _ = write!(
        json,
        ",\"meshes\":[{{\"name\":\"{name}\",\"primitives\":[{}]}}]",
        prim_json.join(",")
    );
    json.push_str(",\"nodes\":[{\"mesh\":0}],\"scenes\":[{\"nodes\":[0]}],\"scene\":0}");

    assemble_glb(&json, &b.bin)
}

/// Wrap a JSON string + a BIN buffer into a binary glTF container.
fn assemble_glb(json: &str, bin: &[u8]) -> Vec<u8> {
    let mut json_chunk = json.as_bytes().to_vec();
    while !json_chunk.len().is_multiple_of(4) {
        json_chunk.push(b' ');
    }
    let mut bin_chunk = bin.to_vec();
    while !bin_chunk.len().is_multiple_of(4) {
        bin_chunk.push(0);
    }
    let total = 12 + 8 + json_chunk.len() + 8 + bin_chunk.len();

    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67u32.to_le_bytes()); // "glTF"
    out.extend_from_slice(&2u32.to_le_bytes()); // version
    out.extend_from_slice(&(total as u32).to_le_bytes());
    // JSON chunk
    out.extend_from_slice(&(json_chunk.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json_chunk);
    // BIN chunk
    out.extend_from_slice(&(bin_chunk.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes()); // "BIN\0"
    out.extend_from_slice(&bin_chunk);
    out
}

/// An axis-aligned box [min,max] as a 24-vertex / 36-index primitive with outward per-face normals.
fn box_prim(min: [f32; 3], max: [f32; 3], color: [f32; 4]) -> PrimSpec {
    // 6 faces, each [normal, 4 corners CCW-from-outside].
    let faces: [([f32; 3], [[f32; 3]; 4]); 6] = [
        // +X
        (
            [1.0, 0.0, 0.0],
            [
                [max[0], min[1], min[2]],
                [max[0], max[1], min[2]],
                [max[0], max[1], max[2]],
                [max[0], min[1], max[2]],
            ],
        ),
        // -X
        (
            [-1.0, 0.0, 0.0],
            [
                [min[0], min[1], max[2]],
                [min[0], max[1], max[2]],
                [min[0], max[1], min[2]],
                [min[0], min[1], min[2]],
            ],
        ),
        // +Y
        (
            [0.0, 1.0, 0.0],
            [
                [min[0], max[1], min[2]],
                [min[0], max[1], max[2]],
                [max[0], max[1], max[2]],
                [max[0], max[1], min[2]],
            ],
        ),
        // -Y
        (
            [0.0, -1.0, 0.0],
            [
                [min[0], min[1], max[2]],
                [min[0], min[1], min[2]],
                [max[0], min[1], min[2]],
                [max[0], min[1], max[2]],
            ],
        ),
        // +Z
        (
            [0.0, 0.0, 1.0],
            [
                [min[0], min[1], max[2]],
                [max[0], min[1], max[2]],
                [max[0], max[1], max[2]],
                [min[0], max[1], max[2]],
            ],
        ),
        // -Z
        (
            [0.0, 0.0, -1.0],
            [
                [max[0], min[1], min[2]],
                [min[0], min[1], min[2]],
                [min[0], max[1], min[2]],
                [max[0], max[1], min[2]],
            ],
        ),
    ];
    let mut positions = Vec::with_capacity(24);
    let mut normals = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    for (normal, corners) in faces {
        let base = positions.len() as u16;
        for c in corners {
            positions.push(c);
            normals.push(normal);
        }
        indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    PrimSpec {
        positions,
        normals: Some(normals),
        indices,
        base_color: color,
        texture: None,
    }
}

/// The health-bar mesh: a dark frame box behind a red fill box — a recognizable bar, not a cube.
/// Two primitives / two materials (multi-material import + per-material vertex-color baking).
#[must_use]
pub fn healthbar_glb() -> Vec<u8> {
    let frame = box_prim(
        [-1.05, -0.28, -0.06],
        [1.05, 0.28, 0.0],
        [0.10, 0.10, 0.12, 1.0],
    );
    let fill = box_prim(
        [-1.0, -0.22, -0.05],
        [0.55, 0.22, 0.06],
        [0.85, 0.18, 0.20, 1.0],
    );
    build_glb("healthbar", &[frame, fill], &[])
}

/// A faceted octahedron prop — distinctly non-cube, and authored with **no** normals so the GPU
/// packer derives them (smooth-normal derivation path).
#[must_use]
pub fn prop_glb() -> Vec<u8> {
    let s = 0.7f32;
    let positions = vec![
        [s, 0.0, 0.0],
        [-s, 0.0, 0.0],
        [0.0, s, 0.0],
        [0.0, -s, 0.0],
        [0.0, 0.0, s],
        [0.0, 0.0, -s],
    ];
    // 8 triangular faces (top fan to +/-Y, then bottom).
    let indices = vec![
        0, 2, 4, 4, 2, 1, 1, 2, 5, 5, 2, 0, // top four (around +Y)
        0, 4, 3, 4, 1, 3, 1, 5, 3, 5, 0, 3, // bottom four (around -Y)
    ];
    let prim = PrimSpec {
        positions,
        normals: None,
        indices,
        base_color: [0.20, 0.70, 0.75, 1.0],
        texture: None,
    };
    build_glb("prop", &[prim], &[])
}

/// A unit quad carrying an embedded PNG base-color texture — for the importer's texture-decode test.
#[must_use]
pub fn textured_quad_glb() -> Vec<u8> {
    let positions = vec![
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let normals = vec![[0.0, 0.0, 1.0]; 4];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let prim = PrimSpec {
        positions,
        normals: Some(normals),
        indices,
        base_color: [1.0, 1.0, 1.0, 1.0],
        texture: Some(0),
    };
    build_glb("textured", &[prim], &[checker_png()])
}

/// A tiny 2×2 RGBA PNG (a checker), encoded with our pinned pure-Rust `image`.
fn checker_png() -> Vec<u8> {
    let mut img = image::RgbaImage::new(2, 2);
    img.put_pixel(0, 0, image::Rgba([220, 40, 40, 255]));
    img.put_pixel(1, 0, image::Rgba([40, 220, 40, 255]));
    img.put_pixel(0, 1, image::Rgba([40, 40, 220, 255]));
    img.put_pixel(1, 1, image::Rgba([220, 220, 40, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode demo png");
    out.into_inner()
}
