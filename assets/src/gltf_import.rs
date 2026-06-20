//! The glTF/glb import backend — **the one file that touches `gltf::` / `image::`** (the wrapper
//! boundary, invariant 5; CI grep-gated, mirroring `flecs_ecs`-in-`/ecs`). Everything it returns is
//! the project's internal [`MeshAsset`]; no foreign type crosses out.
//!
//! Scope: self-contained `.glb` (single binary blob) and `.gltf` with embedded data — read via
//! `Gltf::from_slice`, with geometry pulled from the blob and base-color PNG textures decoded from
//! their bufferViews by our pinned pure-Rust `image`. We deliberately do **not** enable `gltf`'s
//! `import` feature (it pulls a rayon'd decoder path that breaks `wasm32`) — so external-file/base64
//! buffer references are unresolved and reported as [`ImportError::Malformed`] rather than touching a
//! filesystem. That keeps the whole crate `wasm32-unknown-unknown`-clean (ADR-006, deliverable 6).

// glTF positions/uvs are f32 already; index casts are bounded by MAX_ELEMENTS (checked before read).
#![allow(clippy::cast_possible_truncation)]

use gltf::image::Source as ImageSource;
use gltf::Gltf;

use crate::mesh::{Material, MeshAsset, Primitive, Texture};
use crate::source::{ImportError, MeshSource, MAX_ELEMENTS, MAX_IMPORT_BYTES};

/// The glTF/glb importer. Stateless — construct with [`GltfImporter::new`] and call
/// [`MeshSource::import`].
#[derive(Debug, Default, Clone, Copy)]
pub struct GltfImporter;

impl GltfImporter {
    /// Construct the importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MeshSource for GltfImporter {
    fn format(&self) -> &'static str {
        "gltf/glb"
    }

    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }
        // Parse (validates the container) — flatten the decoder's error to a string, never leak its type.
        let doc = Gltf::from_slice(bytes).map_err(|e| ImportError::Malformed(e.to_string()))?;
        let blob: &[u8] = doc.blob.as_deref().unwrap_or(&[]);

        // Materials first (so primitives can index them). glTF's implicit "default material" maps to
        // index `materials.len()` — appended last if any primitive uses it.
        let mut materials: Vec<Material> = doc
            .materials()
            .map(|m| {
                let pbr = m.pbr_metallic_roughness();
                Material {
                    base_color: pbr.base_color_factor(),
                    base_color_texture: pbr
                        .base_color_texture()
                        .map(|t| t.texture().source().index()),
                }
            })
            .collect();
        let default_material_index = materials.len();
        let mut used_default = false;

        let mut primitives: Vec<Primitive> = Vec::new();
        for mesh in doc.meshes() {
            for prim in mesh.primitives() {
                if prim.mode() != gltf::mesh::Mode::Triangles {
                    continue; // we render triangle lists; skip line/point primitives
                }
                // Fail-fast on a decode bomb: refuse before allocating, using the declared accessor
                // counts (an attacker controls these in the JSON; reading would allocate first).
                if let Some(acc) = prim.get(&gltf::Semantic::Positions) {
                    guard_count(acc.count())?;
                }
                if let Some(acc) = prim.indices() {
                    guard_count(acc.count())?;
                }

                let reader = prim.reader(|buffer| match buffer.source() {
                    gltf::buffer::Source::Bin => Some(blob),
                    gltf::buffer::Source::Uri(_) => None, // external/base64 buffer — unsupported tier
                });

                let Some(positions) = reader.read_positions() else {
                    continue; // no positions (or an unresolved external buffer) — not drawable
                };
                let positions: Vec<[f32; 3]> = positions.collect();
                if positions.is_empty() {
                    continue;
                }
                let normals: Vec<[f32; 3]> = reader
                    .read_normals()
                    .map(Iterator::collect)
                    .unwrap_or_default();
                let uvs: Vec<[f32; 2]> = reader
                    .read_tex_coords(0)
                    .map(|tc| tc.into_f32().collect())
                    .unwrap_or_default();
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(idx) => idx.into_u32().collect(),
                    // A primitive with no index buffer is an implicit sequential triangle list.
                    None => (0..positions.len() as u32).collect(),
                };
                guard_count(positions.len())?;
                guard_count(indices.len())?;
                // Triangle lists only (mode is already constrained to Triangles): a count not divisible
                // by 3 is a malformed primitive — reject it fail-fast rather than let the downstream
                // `chunks_exact(3)` silently drop the trailing partial triangle.
                if !indices.len().is_multiple_of(3) {
                    return Err(ImportError::Malformed(format!(
                        "triangle-list index count {} is not a multiple of 3",
                        indices.len()
                    )));
                }

                let material = prim.material().index().unwrap_or_else(|| {
                    used_default = true;
                    default_material_index
                });

                primitives.push(Primitive {
                    positions,
                    normals,
                    uvs,
                    indices,
                    material,
                });
            }
        }

        if primitives.is_empty() {
            return Err(ImportError::NoGeometry);
        }
        if used_default {
            materials.push(Material::default());
        }

        // Decode the base-color textures actually referenced by a material (RGBA8, from their
        // bufferView bytes). A texture we can't resolve/decode is dropped to `None` on the material —
        // never fatal (the render path bakes the base-color factor regardless).
        let textures = decode_textures(&doc, blob, &mut materials);

        let name = doc
            .meshes()
            .next()
            .and_then(|m| m.name().map(str::to_string))
            .unwrap_or_else(|| "mesh".to_string());

        Ok(MeshAsset {
            name,
            primitives,
            materials,
            textures,
        })
    }
}

/// Reject a count over [`MAX_ELEMENTS`] before it can allocate.
fn guard_count(count: usize) -> Result<(), ImportError> {
    if count > MAX_ELEMENTS {
        Err(ImportError::TooManyElements {
            count,
            limit: MAX_ELEMENTS,
        })
    } else {
        Ok(())
    }
}

/// Decode every base-color texture referenced by `materials` into RGBA8, rewriting each material's
/// `base_color_texture` to index into the returned (compact) texture list. Source images that aren't
/// resolvable PNG-in-bufferView are dropped (`None`).
fn decode_textures(doc: &Gltf, blob: &[u8], materials: &mut [Material]) -> Vec<Texture> {
    use std::collections::HashMap;
    let mut out: Vec<Texture> = Vec::new();
    let mut remap: HashMap<usize, usize> = HashMap::new(); // glTF image index → compact texture index

    for mat in materials.iter_mut() {
        let Some(img_idx) = mat.base_color_texture else {
            continue;
        };
        if let Some(&compact) = remap.get(&img_idx) {
            mat.base_color_texture = Some(compact);
            continue;
        }
        let decoded = doc
            .images()
            .nth(img_idx)
            .and_then(|img| decode_image(&img, blob));
        if let Some(tex) = decoded {
            let compact = out.len();
            out.push(tex);
            remap.insert(img_idx, compact);
            mat.base_color_texture = Some(compact);
        } else {
            mat.base_color_texture = None; // unresolved — fall back to the base-color factor
        }
    }
    out
}

/// Decode one glTF image from its bufferView (PNG only, RGBA8). `None` if the source is external/URI
/// or not decodable as PNG.
fn decode_image(img: &gltf::Image, blob: &[u8]) -> Option<Texture> {
    let ImageSource::View { view, mime_type } = img.source() else {
        return None; // a URI image — unsupported in this self-contained tier
    };
    if mime_type != "image/png" {
        return None;
    }
    let start = view.offset();
    let end = start.checked_add(view.length())?;
    let bytes = blob.get(start..end)?;
    let decoded = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
    let rgba = decoded.to_rgba8();
    let (width, height) = (rgba.width(), rgba.height());
    Some(Texture {
        width,
        height,
        rgba8: rgba.into_raw(),
    })
}
