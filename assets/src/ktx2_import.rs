//! KTX2 / basis texture transcode (M11.1, ADR-040) — a `.basis`/`.ktx2` texture → RGBA8 → a placeable
//! textured quad, behind the [`MeshSource`] trait. **Native-only:** `basis-universal` wraps Binomial's C++
//! codec via FFI, so this whole module is behind the **`ktx2` feature** — the default crate stays
//! `wasm32`-clean (the browser funnel gets pre-transcoded-server-side / uncompressed RGBA8, the explicit
//! wasm seam, ADR-040). `basis_universal::` stays behind THIS module (grep-gated).
//!
//! **Determinism-audited (the M9.5 / `baby_shark` rule):** the TRANSCODE (decode → RGBA8) is the import
//! path, and it is **deterministic** — the test transcodes the same `.basis` twice and asserts byte-identical
//! output. (Encoding is a server-side step, not on the import path.) The decode-bomb guard reads the declared
//! dims from the header BEFORE transcoding.

use basis_universal::{transcoder_init, TranscodeParameters, Transcoder, TranscoderTextureFormat};

use crate::image_import::MAX_TEXELS;
use crate::mesh::{Material, MeshAsset, Primitive, Texture};
use crate::source::{ImportError, MeshSource, MAX_IMPORT_BYTES};

/// Transcode a `.basis`/`.ktx2` to an RGBA8 [`Texture`]. Deterministic; decode-bomb-guarded (the dims are
/// capped from the header before the transcode allocates).
///
/// # Errors
/// [`ImportError::Malformed`] on an unparseable container / unsupported transcode, or
/// [`ImportError::TooManyElements`] if the declared image exceeds the texel cap.
#[allow(clippy::cast_possible_truncation)]
pub fn transcode_to_rgba8(bytes: &[u8]) -> Result<Texture, ImportError> {
    transcoder_init();
    let mut transcoder = Transcoder::new();
    transcoder
        .prepare_transcoding(bytes)
        .map_err(|()| ImportError::Malformed("KTX2/basis: not a transcodable container".into()))?;
    let desc = transcoder
        .image_level_description(bytes, 0, 0)
        .ok_or_else(|| ImportError::Malformed("KTX2/basis: no image level 0".into()))?;
    let (w, h) = (desc.original_width, desc.original_height);
    // Decode-bomb guard: reject a header claiming a ruinously large image BEFORE transcoding.
    if u64::from(w) * u64::from(h) > MAX_TEXELS {
        return Err(ImportError::TooManyElements {
            count: (w as usize).saturating_mul(h as usize),
            limit: MAX_TEXELS as usize,
        });
    }
    let rgba = transcoder
        .transcode_image_level(
            bytes,
            TranscoderTextureFormat::RGBA32,
            TranscodeParameters {
                image_index: 0,
                level_index: 0,
                ..Default::default()
            },
        )
        .map_err(|e| ImportError::Malformed(format!("KTX2/basis transcode failed: {e:?}")))?;
    Ok(Texture {
        width: w,
        height: h,
        rgba8: rgba,
    })
}

/// The KTX2/basis texture importer (M11.1) — transcodes to RGBA8 and wraps it as a placeable textured quad
/// (the same shape the PNG/JPG `ImageImporter` produces). Native-only (`ktx2` feature).
#[derive(Clone, Copy, Debug, Default)]
pub struct KtxImporter;

impl KtxImporter {
    /// A new importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl MeshSource for KtxImporter {
    fn format(&self) -> &'static str {
        "ktx2/basis"
    }

    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }
        let texture = transcode_to_rgba8(bytes)?;
        // A unit quad in the XY plane (UVs 0..1) carrying the transcoded texture — placeable + renderable.
        Ok(MeshAsset {
            name: "ktx2".into(),
            primitives: vec![Primitive {
                positions: vec![
                    [-0.5, -0.5, 0.0],
                    [0.5, -0.5, 0.0],
                    [0.5, 0.5, 0.0],
                    [-0.5, 0.5, 0.0],
                ],
                normals: Vec::new(),
                uvs: vec![[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]],
                indices: vec![0, 1, 2, 0, 2, 3],
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            }],
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A real `.basis` container: a 64×64 ETC1S checker, encoded ONCE out-of-crate (the encoder is the
    /// `basis_universal` C++ `Compressor`, an `unsafe` FFI call — and this crate `forbid`s unsafe, by
    /// design: encoding is the server-side step, never on the import path). Committed as a fixture so the
    /// in-crate test only exercises the SAFE transcode (the actual import path) it means to measure.
    const CHECKER_BASIS: &[u8] = include_bytes!("../tests/fixtures/checker_64.basis");

    #[test]
    fn ktx2_basis_transcode_is_deterministic_and_measured() {
        let basis = CHECKER_BASIS;
        let a = transcode_to_rgba8(basis).expect("transcode a");
        let b = transcode_to_rgba8(basis).expect("transcode b");
        assert_eq!((a.width, a.height), (64, 64));
        assert_eq!(a.rgba8.len(), 64 * 64 * 4, "RGBA8 = w*h*4");
        // THE AUDIT: the transcode (the import path) is DETERMINISTIC — same bytes twice.
        assert_eq!(a.rgba8, b.rgba8, "basis/KTX2 transcode is deterministic");

        // Timing on this box (the min-spec transcode number).
        let n = 50;
        let t0 = Instant::now();
        for _ in 0..n {
            let _ = transcode_to_rgba8(basis);
        }
        let us = t0.elapsed().as_secs_f64() * 1.0e6 / f64::from(n);
        eprintln!(
            "KTX2/basis transcode (64x64 ETC1S → RGBA8): deterministic ✓ · {us:.1} µs/transcode on this box"
        );

        // The full importer wraps it as a placeable textured quad.
        let asset = KtxImporter::new().import(basis).expect("import ktx2");
        assert_eq!(asset.textures.len(), 1);
        assert_eq!(asset.primitives.len(), 1, "a textured quad");
        assert_eq!(asset.materials[0].base_color_texture, Some(0));
    }

    #[test]
    fn malformed_basis_is_an_explained_error_not_a_panic() {
        let err = transcode_to_rgba8(b"\x00\x01 not a basis container").unwrap_err();
        assert!(
            matches!(
                err,
                ImportError::Malformed(_) | ImportError::TooManyElements { .. }
            ),
            "got {err:?}"
        );
    }
}
