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
// of small counts to u16 indices is intentional and bounded by the hand-authored geometry; likewise the
// sphere's `usize`→`f32` segment ratios are over tiny constant counts (≤16), so the precision loss is
// nil in practice.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::fmt::Write as _;

/// One primitive to encode: positions, optional normals + UVs, triangle indices, a base color, and
/// optional PBR textures (each an index into the builder's textures). `texture` = base-color,
/// `mr_texture` = metallic-roughness (G=roughness, B=metallic), `normal_texture` = tangent-space normal.
#[derive(Default)]
struct PrimSpec {
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Option<Vec<[f32; 2]>>,
    indices: Vec<u16>,
    base_color: [f32; 4],
    texture: Option<usize>,
    mr_texture: Option<usize>,
    normal_texture: Option<usize>,
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
        // TEXCOORD_0 (optional) — required for textures to sample spatially (without it the importer
        // reads no UVs and every vertex samples texel 0, i.e. a flat color).
        if let Some(uvs) = &prim.uvs {
            let uv_bytes = f32_le(uvs.iter().flat_map(|p| p.iter().copied()));
            let uv_view = b.add_view(&uv_bytes, Some(ARRAY_BUFFER));
            let uv_acc = b.add_accessor(uv_view, FLOAT, uvs.len(), "VEC2", None);
            let _ = write!(attrs, ",\"TEXCOORD_0\":{uv_acc}");
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
        let mut pbr = format!("\"baseColorFactor\":[{r},{g},{bl},{a}]");
        if let Some(t) = prim.texture {
            let _ = write!(pbr, ",\"baseColorTexture\":{{\"index\":{t}}}");
        }
        if let Some(t) = prim.mr_texture {
            let _ = write!(pbr, ",\"metallicRoughnessTexture\":{{\"index\":{t}}}");
        }
        // `normalTexture` is a material-level sibling of pbrMetallicRoughness (not inside it).
        let normal = prim.normal_texture.map_or(String::new(), |t| {
            format!(",\"normalTexture\":{{\"index\":{t}}}")
        });
        material_json.push(format!("{{\"pbrMetallicRoughness\":{{{pbr}}}{normal}}}"));
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
        ..Default::default()
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
        ..Default::default()
    };
    build_glb("prop", &[prim], &[])
}

/// A smooth UV sphere (radius 0.5) — the canonical **physics** test mesh (M8.2): unmistakably not a
/// cube, it pairs with a ball collider and visibly falls, rolls, and rests under gravity. Smooth
/// per-vertex normals (a unit sphere's outward normal is its position direction). Own geometry — no
/// third-party asset, deterministic bytes, wasm-safe. ~221 verts / 1152 indices (u16-safe).
#[must_use]
pub fn sphere_glb() -> Vec<u8> {
    const R: f32 = 0.5;
    const STACKS: usize = 12; // latitude bands (north pole → south)
    const SLICES: usize = 16; // longitude segments
    let mut positions = Vec::with_capacity((STACKS + 1) * (SLICES + 1));
    let mut normals = Vec::with_capacity((STACKS + 1) * (SLICES + 1));
    for i in 0..=STACKS {
        let lat = (i as f32 / STACKS as f32) * std::f32::consts::PI; // 0..π
        let (sin_lat, cos_lat) = lat.sin_cos();
        for j in 0..=SLICES {
            let lon = (j as f32 / SLICES as f32) * std::f32::consts::TAU; // 0..2π
            let (sin_lon, cos_lon) = lon.sin_cos();
            let n = [sin_lat * cos_lon, cos_lat, sin_lat * sin_lon];
            positions.push([R * n[0], R * n[1], R * n[2]]);
            normals.push(n); // already unit-length
        }
    }
    let mut indices = Vec::with_capacity(STACKS * SLICES * 6);
    let row = (SLICES + 1) as u16;
    for i in 0..STACKS as u16 {
        for j in 0..SLICES as u16 {
            let a = i * row + j;
            let b = a + row;
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    let prim = PrimSpec {
        positions,
        normals: Some(normals),
        indices,
        base_color: [0.95, 0.55, 0.20, 1.0], // amber — distinct from the scene cubes/props
        ..Default::default()
    };
    build_glb("sphere", &[prim], &[])
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
        ..Default::default()
    };
    build_glb("textured", &[prim], &[checker_png()])
}

/// A unit quad (XY plane, +Z normal, UVs spanning [0,1]) carrying a **full PBR texture set**: a solid
/// light base color, a metallic-roughness map split left|right (smooth metal | rough dielectric), and a
/// tangent-space **normal map** encoding a sinusoidal ripple relief. This is the M11.2 follow-up fixture
/// — on a flat quad under a grazing light it shows the normal-mapped relief + the metallic split, the
/// positive MR/normal *visual*, and it exercises the importer's 3-slot texture-decode path.
#[must_use]
pub fn normal_mapped_quad_glb() -> Vec<u8> {
    let positions = vec![
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let normals = vec![[0.0, 0.0, 1.0]; 4];
    let uvs = vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let prim = PrimSpec {
        positions,
        normals: Some(normals),
        uvs: Some(uvs),
        indices,
        base_color: [1.0, 1.0, 1.0, 1.0],
        texture: Some(0),
        mr_texture: Some(1),
        normal_texture: Some(2),
    };
    build_glb(
        "normal_mapped",
        &[prim],
        &[gray_base_png(), mr_split_png(), ripple_normal_png()],
    )
}

/// Encode an RGBA8 buffer as PNG bytes (via the crate's pinned, wasm-clean `image`).
fn encode_png_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Vec<u8> {
    let img = image::RgbaImage::from_raw(width, height, rgba).expect("rgba dims match");
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("encode png");
    out.into_inner()
}

/// A solid light-grey base-color PNG (so the normal/MR relief dominates the look, not a busy albedo).
fn gray_base_png() -> Vec<u8> {
    const N: u32 = 8;
    let rgba: Vec<u8> = (0..N * N).flat_map(|_| [200u8, 200, 205, 255]).collect();
    encode_png_rgba(N, N, rgba)
}

/// A metallic-roughness PNG split left|right: left half = smooth metal (B=metal high, G=rough low),
/// right half = rough dielectric (B=0, G high). Sampled in `fs_mesh` as `metallic *= B`, `roughness *= G`.
fn mr_split_png() -> Vec<u8> {
    const N: u32 = 16;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for _y in 0..N {
        for x in 0..N {
            let metal_half = x < N / 2;
            let g = if metal_half { 40 } else { 235 }; // roughness (G)
            let b = if metal_half { 255 } else { 0 }; // metalness (B)
            rgba.extend_from_slice(&[0, g, b, 255]);
        }
    }
    encode_png_rgba(N, N, rgba)
}

/// A tangent-space normal map encoding a sinusoidal **ripple** relief: a height field h = sin·sin, its
/// surface normal = normalize(-∂h/∂u, -∂h/∂v, 1), RGB-encoded (xyz·0.5+0.5, B≈1 at the flats). On a flat
/// quad this reads as a quilted relief under a grazing light — proof the normal map drives the shading.
fn ripple_normal_png() -> Vec<u8> {
    const N: u32 = 64;
    const F: f32 = 4.0; // ripple cycles across the tile
    const G: f32 = 0.7; // peak relief gradient (≈ tan of the steepest tilt)
    let tau_f = std::f32::consts::TAU * F;
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let u = x as f32 / N as f32;
            let v = y as f32 / N as f32;
            // ∂h/∂u, ∂h/∂v of the height field h = sin(τF u)·sin(τF v).
            let grad = [
                G * (tau_f * u).cos() * (tau_f * v).sin(),
                G * (tau_f * u).sin() * (tau_f * v).cos(),
            ];
            let len = (grad[0] * grad[0] + grad[1] * grad[1] + 1.0).sqrt();
            let n = [-grad[0] / len, -grad[1] / len, 1.0 / len];
            let enc = |c: f32| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgba.extend_from_slice(&[enc(n[0]), enc(n[1]), enc(n[2]), 255]);
        }
    }
    encode_png_rgba(N, N, rgba)
}

/// Two side-by-side quads as **two primitives / two materials**, each with its OWN base-color texture (a
/// checker on the left, a solid colour on the right). The M11.2 multi-texture-per-mesh fixture: a correct
/// renderer shows BOTH textures; the prior single-texture path showed only the first across the whole mesh.
#[must_use]
pub fn multi_material_quad_glb() -> Vec<u8> {
    let quad = |x0: f32, x1: f32, tex: usize| PrimSpec {
        positions: vec![
            [x0, -0.5, 0.0],
            [x1, -0.5, 0.0],
            [x1, 0.5, 0.0],
            [x0, 0.5, 0.0],
        ],
        normals: Some(vec![[0.0, 0.0, 1.0]; 4]),
        uvs: Some(vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]),
        indices: vec![0, 1, 2, 0, 2, 3],
        base_color: [1.0, 1.0, 1.0, 1.0],
        texture: Some(tex),
        ..Default::default()
    };
    build_glb(
        "multi_material",
        &[quad(-1.0, 0.0, 0), quad(0.0, 1.0, 1)],
        &[checker_png(), solid_png([40, 220, 90, 255])],
    )
}

/// A small solid-colour RGBA PNG (the right tile's base texture — vivid so the per-submesh split is obvious).
fn solid_png(rgba: [u8; 4]) -> Vec<u8> {
    const N: u32 = 4;
    let pixels: Vec<u8> = (0..N * N).flat_map(|_| rgba).collect();
    encode_png_rgba(N, N, pixels)
}

/// A quad whose triangle-list index count is **not** a multiple of 3 (5 indices) — a deliberately
/// malformed primitive, for the importer's fail-fast strictness guard test.
#[must_use]
pub fn malformed_indices_glb() -> Vec<u8> {
    let positions = vec![
        [-0.5, -0.5, 0.0],
        [0.5, -0.5, 0.0],
        [0.5, 0.5, 0.0],
        [-0.5, 0.5, 0.0],
    ];
    let prim = PrimSpec {
        positions,
        normals: None,
        indices: vec![0, 1, 2, 0, 3], // 5 — not a multiple of 3
        base_color: [0.5, 0.5, 0.5, 1.0],
        ..Default::default()
    };
    build_glb("malformed", &[prim], &[])
}

/// A minimal **skinned** mesh (M9.3 / G3): a tall quad bound to a 2-joint chain — a **root** joint at the
/// origin and a **child** joint 1 unit up (parented to the root). The bottom edge (y=0) binds fully to the
/// root, the top edge (y=2) to the child, so an FK pose of the child bends the top. The
/// `inverseBindMatrices` are the inverse of each joint's bind global (root → identity; child → translate
/// (0,-1,0)), so the skinning matrices are **identity at bind** (a bound vertex is unmoved in the rest
/// pose). Own geometry, deterministic bytes, wasm-safe — the provenance for the importer's skin-load test.
#[must_use]
pub fn skinned_quad_glb() -> Vec<u8> {
    const MAT4: &str = "MAT4";
    const VEC4: &str = "VEC4";
    let mut b = GlbBuilder::default();

    // Geometry: a tall quad in XY. Bottom edge y=0 (→ root joint 0), top edge y=2 (→ child joint 1).
    let positions = [
        [-0.2f32, 0.0, 0.0],
        [0.2, 0.0, 0.0],
        [0.2, 2.0, 0.0],
        [-0.2, 2.0, 0.0],
    ];
    let pos_view = b.add_view(
        &f32_le(positions.iter().flat_map(|p| p.iter().copied())),
        Some(ARRAY_BUFFER),
    );
    let pos_acc = b.add_accessor(pos_view, FLOAT, 4, "VEC3", Some(vec3_minmax(&positions)));

    // JOINTS_0 (VEC4 u16): bottom verts → root (0); top verts → child (1).
    let joints: [[u16; 4]; 4] = [[0, 0, 0, 0], [0, 0, 0, 0], [1, 0, 0, 0], [1, 0, 0, 0]];
    let mut joint_bytes = Vec::new();
    for j in joints {
        for v in j {
            joint_bytes.extend_from_slice(&v.to_le_bytes());
        }
    }
    let j_view = b.add_view(&joint_bytes, Some(ARRAY_BUFFER));
    let j_acc = b.add_accessor(j_view, UNSIGNED_SHORT, 4, VEC4, None);

    // WEIGHTS_0 (VEC4 f32): fully weighted to the first influence.
    let weights = [[1.0f32, 0.0, 0.0, 0.0]; 4];
    let w_view = b.add_view(
        &f32_le(weights.iter().flat_map(|w| w.iter().copied())),
        Some(ARRAY_BUFFER),
    );
    let w_acc = b.add_accessor(w_view, FLOAT, 4, VEC4, None);

    // Indices.
    let mut idx_bytes = Vec::new();
    for i in [0u16, 1, 2, 0, 2, 3] {
        idx_bytes.extend_from_slice(&i.to_le_bytes());
    }
    let idx_view = b.add_view(&idx_bytes, Some(ELEMENT_ARRAY_BUFFER));
    let idx_acc = b.add_accessor(idx_view, UNSIGNED_SHORT, 6, "SCALAR", None);

    // inverseBindMatrices (MAT4 f32, column-major, 2 joints): root = identity; child = translate (0,-1,0).
    let ibm: [f32; 32] = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
        1.0, // joint 0
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0,
        1.0, // joint 1
    ];
    let ibm_view = b.add_view(&f32_le(ibm), None); // non-vertex data → no bufferView target
    let ibm_acc = b.add_accessor(ibm_view, FLOAT, 2, MAT4, None);

    b.align4();
    let buffer_len = b.bin.len();

    let mut json = String::from("{\"asset\":{\"version\":\"2.0\"}");
    let _ = write!(json, ",\"buffers\":[{{\"byteLength\":{buffer_len}}}]");
    let _ = write!(json, ",\"bufferViews\":[{}]", b.views.join(","));
    let _ = write!(json, ",\"accessors\":[{}]", b.accessors.join(","));
    let _ = write!(
        json,
        ",\"materials\":[{{\"pbrMetallicRoughness\":{{\"baseColorFactor\":[0.7,0.6,0.3,1.0]}}}}]"
    );
    let _ = write!(
        json,
        ",\"meshes\":[{{\"name\":\"skinned\",\"primitives\":[{{\"attributes\":{{\"POSITION\":{pos_acc},\"JOINTS_0\":{j_acc},\"WEIGHTS_0\":{w_acc}}},\"indices\":{idx_acc},\"material\":0,\"mode\":4}}]}}]"
    );
    // nodes: 0 = the skinned mesh instance; 1 = root joint (child 2); 2 = child joint at +Y 1.
    let _ = write!(
        json,
        ",\"nodes\":[{{\"mesh\":0,\"skin\":0}},{{\"name\":\"root\",\"translation\":[0,0,0],\"children\":[2]}},{{\"name\":\"child\",\"translation\":[0,1,0]}}]"
    );
    let _ = write!(
        json,
        ",\"skins\":[{{\"joints\":[1,2],\"inverseBindMatrices\":{ibm_acc},\"skeleton\":1}}]"
    );
    json.push_str(",\"scenes\":[{\"nodes\":[0,1]}],\"scene\":0}");

    assemble_glb(&json, &b.bin)
}

/// A minimal **neural-auto-rig** fixture in the `MTKRIG` blob format (M9.5 / G5) — the structured output
/// a neural auto-rigger (UniRig / RigAnything / Make-It-Animatable) emits, ingested by the offline-bake
/// importer ([`crate::autorig::NeuralRigImporter`]). Deliberately exercises every bake fix-up: the
/// 5-joint chain is given in **reversed** order (each parent index > self → the bake must topo-sort),
/// the weights are **un-normalized** (weight 2.0 → renormalized to 1), and vertex 0 carries **5
/// influences** (> 4 → the bake keeps the top 4). A short 2-column bar so a posed bone visibly bends it.
/// NOT a real network — the deterministic structured prediction, authored by hand.
#[must_use]
pub fn neural_rigged_blob() -> Vec<u8> {
    // 5-joint chain along +Y at y = 0, 1.5, 3, 4.5, 6, given REVERSED (input[0] = the tip). Each entry:
    // (parent input index, or -1 for the root; local-bind translation 1.5 above its parent).
    // input order: 0 = L4 (tip), 1 = L3, 2 = L2, 3 = L1, 4 = L0 (root).
    let joints: [(i32, [f32; 3]); 5] = [
        (1, [0.0, 1.5, 0.0]),  // L4, parent L3 (input 1)
        (2, [0.0, 1.5, 0.0]),  // L3, parent L2 (input 2)
        (3, [0.0, 1.5, 0.0]),  // L2, parent L1 (input 3)
        (4, [0.0, 1.5, 0.0]),  // L1, parent L0 (input 4)
        (-1, [0.0, 0.0, 0.0]), // L0, root
    ];
    // Bar mesh: 2 columns (x = ±0.4) × 7 rows (y = 0..6) → 14 verts; index = row*2 + col.
    let cols = [-0.4f32, 0.4];
    let mut positions: Vec<[f32; 3]> = Vec::new();
    for r in 0..7 {
        for &x in &cols {
            positions.push([x, r as f32, 0.0]);
        }
    }
    let mut tris: Vec<[u32; 3]> = Vec::new();
    for r in 0..6u32 {
        let (bl, br, tl, tr) = (r * 2, r * 2 + 1, (r + 1) * 2, (r + 1) * 2 + 1);
        tris.push([bl, br, tr]);
        tris.push([bl, tr, tl]);
    }
    // Influences (un-normalized): each vertex → its nearest logical joint by height (weight 2.0), except
    // vertex 0, which gets all 5 (weights 1..5) to exercise the > 4 → top-4 reduction. logical→input map.
    let logical_to_input = [4u32, 3, 2, 1, 0]; // L0..L4 → their input slots
    let mut infl: Vec<Vec<(u32, f32)>> = Vec::new();
    for (vi, p) in positions.iter().enumerate() {
        if vi == 0 {
            infl.push((0u32..5).map(|j| (j, (j + 1) as f32)).collect());
        } else {
            let li = ((p[1] / 1.5).round() as i32).clamp(0, 4) as usize;
            infl.push(vec![(logical_to_input[li], 2.0)]);
        }
    }
    encode_mtkrig(&positions, &tris, &joints, &infl)
}

/// Encode the `MTKRIG` blob (little-endian) the [`neural_rigged_blob`] fixture + the offline-bake importer
/// share. Layout: magic, nverts + positions, ntris + indices, njoints + (parent i32, TRS 10×f32), nverts
/// + per-vertex (ninf u16, (joint u32, weight f32)×).
fn encode_mtkrig(
    positions: &[[f32; 3]],
    tris: &[[u32; 3]],
    joints: &[(i32, [f32; 3])],
    infl: &[Vec<(u32, f32)>],
) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(b"MTKRIG01");
    b.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    for p in positions {
        for &c in p {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    b.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in tris {
        for &i in t {
            b.extend_from_slice(&i.to_le_bytes());
        }
    }
    b.extend_from_slice(&(joints.len() as u32).to_le_bytes());
    for &(parent, t) in joints {
        b.extend_from_slice(&parent.to_le_bytes());
        for &c in &t {
            b.extend_from_slice(&c.to_le_bytes());
        }
        for &c in &[0.0f32, 0.0, 0.0, 1.0] {
            b.extend_from_slice(&c.to_le_bytes()); // rotation identity (xyzw)
        }
        for &c in &[1.0f32, 1.0, 1.0] {
            b.extend_from_slice(&c.to_le_bytes()); // scale
        }
    }
    b.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    for list in infl {
        b.extend_from_slice(&(list.len() as u16).to_le_bytes());
        for &(j, w) in list {
            b.extend_from_slice(&j.to_le_bytes());
            b.extend_from_slice(&w.to_le_bytes());
        }
    }
    b
}

/// A tiny 2×2 RGBA checker PNG (red/green/blue/yellow). Hardcoded so the demo generator pulls in **no**
/// `image::`/decoder type — keeping foreign decoder types confined to the importer wrapper
/// (`gltf_import.rs`), the boundary the CI grep-gate enforces. (Provenance: `examples/dump_png.rs`,
/// our pure-Rust `image` PNG encoder — removed after generating these bytes.)
pub fn checker_png() -> Vec<u8> {
    const CHECKER_PNG: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 29, 73, 68, 65, 84, 120, 1, 1, 18, 0, 237, 255, 0, 220,
        40, 40, 255, 40, 220, 40, 255, 0, 40, 40, 220, 255, 220, 220, 40, 255, 77, 76, 9, 97, 40,
        218, 95, 228, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
    ];
    CHECKER_PNG.to_vec()
}
