//! The **unified user-import router** (M10.2, deliverable 1) — "drag a file onto the editor → it imports
//! through the M4 pipeline." Detects the format from the file's **magic bytes** (never a caller-supplied
//! extension, which an attacker controls) and routes to the right trait-backed importer: glTF/glb + PNG/JPG
//! → a [`MeshAsset`] (a real, placeable, renderable entity), WAV/OGG → an [`AudioAsset`] (stored by handle,
//! playback deferred). An unrecognized container is an explained [`ImportError`] — the rejection-as-UX the
//! drag-drop / File→Import affordance surfaces. Pure routing over the existing importers; `wasm32`-clean.

use crate::audio::{AudioAsset, AudioImporter, AudioSource};
use crate::gltf_import::GltfImporter;
use crate::image_import::ImageImporter;
use crate::mesh::MeshAsset;
use crate::source::{ImportError, MeshSource};

/// What a user file imported to. Mesh-shaped assets (glTF, image-as-quad) are placeable + renderable;
/// audio is stored by handle (playback is the audio milestone).
#[derive(Clone, Debug, PartialEq)]
pub enum ImportedAsset {
    /// A glTF/glb model, or a PNG/JPG decoded to a textured quad.
    Mesh(MeshAsset),
    /// A WAV/OGG audio asset (metadata + bytes; playback deferred).
    Audio(AudioAsset),
}

/// The recognized container, sniffed from the leading bytes. `None` ⇒ unrecognized.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detected {
    /// glTF binary (`glTF` magic) or a `.gltf` JSON document.
    Gltf,
    /// PNG or JPEG.
    Image,
    /// WAV (RIFF/WAVE) or Ogg.
    Audio,
}

/// Sniff the container from the file's magic bytes alone (extension-independent — a renamed `.png` that is
/// really a glTF is routed by what it *is*).
#[must_use]
pub fn detect(bytes: &[u8]) -> Option<Detected> {
    // glTF binary: "glTF" magic. A `.gltf` JSON: the first non-whitespace byte is `{`.
    if bytes.starts_with(b"glTF") {
        return Some(Detected::Gltf);
    }
    // Images.
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) || bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Detected::Image);
    }
    // Audio.
    if (bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE")
        || bytes.starts_with(b"OggS")
    {
        return Some(Detected::Audio);
    }
    // A `.gltf` JSON (text) — the first non-space char is `{`.
    if bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'{')
    {
        return Some(Detected::Gltf);
    }
    None
}

/// Import a user file through the right backend, chosen by [`detect`].
///
/// # Errors
/// [`ImportError::Malformed`] for an unrecognized container, or the chosen importer's error (oversize,
/// malformed, decode-bomb, …).
pub fn import_any(bytes: &[u8]) -> Result<ImportedAsset, ImportError> {
    match detect(bytes) {
        Some(Detected::Gltf) => GltfImporter::new().import(bytes).map(ImportedAsset::Mesh),
        Some(Detected::Image) => ImageImporter::new().import(bytes).map(ImportedAsset::Mesh),
        Some(Detected::Audio) => AudioImporter::new().import(bytes).map(ImportedAsset::Audio),
        None => Err(ImportError::Malformed(
            "unrecognized file — supported: glTF/glb, PNG/JPG, WAV/OGG".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_each_format_by_magic_to_the_right_importer() {
        // glb → a mesh.
        assert!(matches!(
            import_any(&crate::demo::healthbar_glb()).expect("glb"),
            ImportedAsset::Mesh(_)
        ));
        // PNG → a mesh (textured quad).
        let png = import_any(&crate::demo::checker_png()).expect("png");
        match png {
            ImportedAsset::Mesh(m) => {
                assert_eq!(m.textures.len(), 1, "the image became a textured quad");
            }
            ImportedAsset::Audio(_) => panic!("a PNG must route to a mesh"),
        }
        // WAV → audio.
        let wav = wav_1s();
        match import_any(&wav).expect("wav") {
            ImportedAsset::Audio(a) => assert_eq!(a.sample_rate, 8000),
            ImportedAsset::Mesh(_) => panic!("a WAV must route to audio"),
        }
    }

    #[test]
    fn detect_is_extension_independent_and_rejects_unknown() {
        assert_eq!(detect(&crate::demo::checker_png()), Some(Detected::Image));
        assert_eq!(detect(&crate::demo::healthbar_glb()), Some(Detected::Gltf));
        assert_eq!(detect(b"   { \"asset\": {} }"), Some(Detected::Gltf)); // .gltf JSON
        assert_eq!(detect(b"random bytes that are nothing"), None);
        assert!(matches!(
            import_any(b"not any known asset").unwrap_err(),
            ImportError::Malformed(_)
        ));
    }

    /// A 1-second 8 kHz mono 16-bit WAV (small, valid RIFF/WAVE).
    fn wav_1s() -> Vec<u8> {
        let (rate, ch, bits, frames) = (8000u32, 1u16, 16u16, 8000u32);
        let block = ch * (bits / 8);
        let byte_rate = rate * u32::from(block);
        let data_len = frames * u32::from(block);
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&(36 + data_len).to_le_bytes());
        b.extend_from_slice(b"WAVE");
        b.extend_from_slice(b"fmt ");
        b.extend_from_slice(&16u32.to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&ch.to_le_bytes());
        b.extend_from_slice(&rate.to_le_bytes());
        b.extend_from_slice(&byte_rate.to_le_bytes());
        b.extend_from_slice(&block.to_le_bytes());
        b.extend_from_slice(&bits.to_le_bytes());
        b.extend_from_slice(b"data");
        b.extend_from_slice(&data_len.to_le_bytes());
        b.resize(b.len() + data_len as usize, 0);
        b
    }
}
