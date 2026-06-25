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
use crate::obj_import::ObjImporter;
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
    /// Wavefront OBJ (no magic — sniffed by its leading text directives). M11.1.
    Obj,
    /// Autodesk FBX (binary `Kaydara FBX Binary` magic, or an ASCII `; FBX` header). M11.1 — recognized;
    /// the importer is the **native `ufbx` FFI seam** (ADR-040), so a dropped `.fbx` is *explained*, not
    /// silently "unrecognized".
    Fbx,
    /// A KTX2 GPU-texture container (`«KTX 20»` magic). M11.1 — recognized; the basis transcode is the
    /// **native C++ FFI seam** (ADR-040), explained until built.
    Ktx2,
}

/// OBJ has **no magic bytes** — it is line-oriented ASCII. Sniff a bounded leading prefix for its
/// characteristic directives: a vertex (`v `) plus a face (`f `) or another OBJ-only directive
/// (`vn`/`vt`/`mtllib`/`usemtl`). Conservative (both a vertex AND a face/attribute directive) so an
/// arbitrary text file doesn't get mis-routed — and it runs only AFTER the magic formats are ruled out.
fn looks_like_obj(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(8192)];
    if head.contains(&0) {
        return false; // a binary file — not OBJ text
    }
    let Ok(text) = std::str::from_utf8(head) else {
        return false;
    };
    let (mut has_vertex, mut has_face_or_attr) = (false, false);
    for line in text.lines() {
        let l = line.trim_start();
        if l.starts_with("v ") || l.starts_with("v\t") {
            has_vertex = true;
        }
        if l.starts_with("f ")
            || l.starts_with("f\t")
            || l.starts_with("vn ")
            || l.starts_with("vt ")
            || l.starts_with("mtllib")
            || l.starts_with("usemtl")
        {
            has_face_or_attr = true;
        }
        if has_vertex && has_face_or_attr {
            return true;
        }
    }
    false
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
    // KTX2 texture container: the `«KTX 20»\r\n\x1A\n` identifier.
    if bytes.starts_with(&[
        0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A,
    ]) {
        return Some(Detected::Ktx2);
    }
    // FBX: binary (`Kaydara FBX Binary`) or ASCII (a leading `; FBX` comment).
    if bytes.starts_with(b"Kaydara FBX Binary") || bytes.starts_with(b"; FBX") {
        return Some(Detected::Fbx);
    }
    // A `.gltf` JSON (text) — the first non-space char is `{`.
    if bytes
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|&b| b == b'{')
    {
        return Some(Detected::Gltf);
    }
    // OBJ last (no magic — a text heuristic, after the magic formats are ruled out).
    if looks_like_obj(bytes) {
        return Some(Detected::Obj);
    }
    None
}

/// FBX route — the native `ufbx` importer (feature `fbx`), else an explained native-seam error.
#[cfg(feature = "fbx")]
fn fbx_route(bytes: &[u8]) -> Result<ImportedAsset, ImportError> {
    crate::fbx_import::FbxImporter::new()
        .import(bytes)
        .map(ImportedAsset::Mesh)
}
#[cfg(not(feature = "fbx"))]
fn fbx_route(_bytes: &[u8]) -> Result<ImportedAsset, ImportError> {
    Err(ImportError::Malformed(
        "FBX recognized — its importer is the native `ufbx` path (build with the `fbx` feature). \
         Export to glTF/glb or OBJ for now."
            .into(),
    ))
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
        Some(Detected::Obj) => ObjImporter::new().import(bytes).map(ImportedAsset::Mesh),
        Some(Detected::Audio) => AudioImporter::new().import(bytes).map(ImportedAsset::Audio),
        // FBX → the native `ufbx` importer when the `fbx` feature is built (ADR-040); otherwise recognized
        // + explained (never a silent "unrecognized" / panic — the browser funnel converts server-side).
        Some(Detected::Fbx) => fbx_route(bytes),
        Some(Detected::Ktx2) => Err(ImportError::Malformed(
            "KTX2 recognized — its basis transcode is the native C++ FFI path (not built in this slice). \
             Use PNG/JPG, or pre-transcode to RGBA8."
                .into(),
        )),
        None => Err(ImportError::Malformed(
            "unrecognized file — supported: glTF/glb, OBJ, PNG/JPG, WAV/OGG (FBX/KTX2 recognized, native seam)".into(),
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

    #[test]
    fn routes_obj_by_its_text_directives() {
        // OBJ has no magic — it's routed by sniffing its `v`/`f` directives, then imports to a mesh.
        let obj = b"o tri\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n";
        assert_eq!(detect(obj), Some(Detected::Obj));
        match import_any(obj).expect("obj") {
            ImportedAsset::Mesh(m) => {
                assert_eq!(m.triangle_count(), 1, "the triangle imported");
                assert!(m.skeleton.is_none());
            }
            ImportedAsset::Audio(_) => panic!("an OBJ must route to a mesh"),
        }
        // A plain prose text file is NOT mis-routed as OBJ (the conservative both-directives heuristic).
        assert_eq!(detect(b"the quick brown fox\njumps over\n"), None);
    }

    #[test]
    fn recognizes_fbx_and_ktx2_as_explained_seams_not_unknown() {
        // FBX/KTX2 are DECIDED native-FFI formats (ADR-040): a dropped file is recognized + explained
        // (so the user is told what to do), never a silent "unrecognized" and never a panic.
        let fbx = b"Kaydara FBX Binary  \x00\x1a\x00 ...rest...";
        assert_eq!(detect(fbx), Some(Detected::Fbx));
        let err = import_any(fbx).unwrap_err();
        assert!(matches!(err, ImportError::Malformed(m) if m.contains("FBX")));

        let ktx2 = [
            0xAB, 0x4B, 0x54, 0x58, 0x20, 0x32, 0x30, 0xBB, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00,
        ];
        assert_eq!(detect(&ktx2), Some(Detected::Ktx2));
        assert!(
            matches!(import_any(&ktx2).unwrap_err(), ImportError::Malformed(m) if m.contains("KTX2"))
        );
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
