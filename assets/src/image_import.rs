//! Standalone **image import** (M10.2) — a user's PNG/JPG → the M4 asset model, as a **placeable textured
//! quad** (`MeshAsset` = a unit quad + the decoded texture), so dropping an image into the editor gives a
//! real, renderable, undoable entity (deliverable 2 + 4). This is the **second file that touches `image::`**
//! (the first is the glTF embedded-texture path in `gltf_import.rs`); the CI grep-gate confines the decoder
//! type here, behind the [`MeshSource`] trait (invariant 5) — no `image::` type crosses out.
//!
//! Untrusted-asset safety (deliverable 8): the **dimensions are read from the header and capped BEFORE the
//! full decode** (a decode-bomb guard — a tiny file claiming a gigapixel image is rejected, never decoded),
//! the input bytes are already size-capped upstream, and a malformed image is an explained
//! [`ImportError::Malformed`] — never a panic, never a fetch (images carry no external URIs). Pure-Rust
//! `image` (png + jpeg, no rayon/C) → `wasm32`-clean (ADR-006).

use std::io::Cursor;

use crate::mesh::{Material, MeshAsset, Primitive, Texture};
use crate::source::{ImportError, MeshSource, MAX_ELEMENTS, MAX_IMPORT_BYTES};

/// The maximum decoded image size in texels (a decode-bomb cap). 8192×8192 ≈ 67 M texels comfortably
/// covers a real user texture while refusing a gigapixel bomb that would exhaust memory on decode.
pub const MAX_TEXELS: u64 = 8192 * 8192;

/// Imports a standalone raster image (PNG/JPG) into a placeable textured-quad [`MeshAsset`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ImageImporter;

impl ImageImporter {
    /// Construct the importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MeshSource for ImageImporter {
    fn format(&self) -> &'static str {
        "png/jpg"
    }

    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }
        // Read the dimensions from the HEADER first and cap them BEFORE decoding (the decode-bomb guard).
        let dims = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| ImportError::Malformed(e.to_string()))?
            .into_dimensions()
            .map_err(|e| ImportError::Malformed(e.to_string()))?;
        let texels = u64::from(dims.0) * u64::from(dims.1);
        if texels > MAX_TEXELS {
            return Err(ImportError::TooManyElements {
                count: usize::try_from(texels).unwrap_or(usize::MAX),
                limit: usize::try_from(MAX_TEXELS).unwrap_or(usize::MAX),
            });
        }

        // Now decode (bounded), to RGBA8.
        let decoded = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| ImportError::Malformed(e.to_string()))?
            .decode()
            .map_err(|e| ImportError::Malformed(e.to_string()))?;
        let rgba = decoded.to_rgba8();
        let (width, height) = (rgba.width(), rgba.height());
        let texture = Texture {
            width,
            height,
            rgba8: rgba.into_raw(),
        };

        // A unit quad in the XY plane facing +Z, UVs flipped in V (image origin is top-left). This is the
        // placeable, renderable form — the same mesh path a glTF plane would take.
        let primitive = Primitive {
            positions: vec![
                [-0.5, -0.5, 0.0],
                [0.5, -0.5, 0.0],
                [0.5, 0.5, 0.0],
                [-0.5, 0.5, 0.0],
            ],
            normals: vec![[0.0, 0.0, 1.0]; 4],
            uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
            indices: vec![0, 1, 2, 0, 2, 3],
            material: 0,
            joints: Vec::new(),
            weights: Vec::new(),
        };
        guard_count(primitive.positions.len())?;

        Ok(MeshAsset {
            name: "image".to_string(),
            primitives: vec![primitive],
            materials: vec![Material {
                base_color: [1.0, 1.0, 1.0, 1.0],
                base_color_texture: Some(0),
                ..Default::default()
            }],
            textures: vec![texture],
            skeleton: None,
        })
    }
}

/// Reject a count over [`MAX_ELEMENTS`] (shares the mesh guard).
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::MeshSource;

    #[test]
    fn imports_a_standalone_png_as_a_textured_quad() {
        // The checked-in 2×2 checker PNG → a unit quad carrying that texture (placeable + renderable).
        let asset = ImageImporter::new()
            .import(&crate::demo::checker_png())
            .expect("png import");
        assert_eq!(asset.primitives.len(), 1);
        assert_eq!(asset.vertex_count(), 4, "a unit quad");
        assert_eq!(asset.triangle_count(), 2);
        assert_eq!(asset.textures.len(), 1);
        assert_eq!((asset.textures[0].width, asset.textures[0].height), (2, 2));
        assert_eq!(asset.textures[0].rgba8.len(), 2 * 2 * 4);
        assert_eq!(asset.materials[0].base_color_texture, Some(0));
        assert!(!asset.primitives[0].uvs.is_empty(), "uvs for texturing");
    }

    #[test]
    fn imports_a_jpg_round_trip() {
        // Encode a 3×2 image to JPEG (the `jpeg` feature), then import it back → a 3×2 textured quad.
        // (image:: stays confined to this wrapper file — the grep-gate boundary.)
        let img = image::RgbImage::from_pixel(3, 2, image::Rgb([200, 120, 60]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
            .expect("encode jpg");
        let asset = ImageImporter::new().import(&bytes).expect("jpg import");
        assert_eq!((asset.textures[0].width, asset.textures[0].height), (3, 2));
        assert_eq!(asset.vertex_count(), 4);
    }

    #[test]
    fn rejects_a_non_image_explained() {
        let err = ImageImporter::new()
            .import(b"definitely not an image")
            .unwrap_err();
        assert!(matches!(err, ImportError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn rejects_an_oversized_input() {
        let big = vec![0u8; MAX_IMPORT_BYTES + 1];
        let err = ImageImporter::new().import(&big).unwrap_err();
        assert!(matches!(err, ImportError::TooLarge { .. }));
    }
}
