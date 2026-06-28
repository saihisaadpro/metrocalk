//! M11.5 (ADR-044) — asset IDENTITY: a provenance record + perceptual-hash near-duplicate detection.
//!
//! This RIDES the content-addressed store (ADR-014/031) — it does **not** rebuild hashing. The store
//! already hashes bytes for exact-dedup + the content address; this adds (a) a [`Provenance`] record
//! ("what is this, where from, AI-generated?") co-located by that content hash, and (b) a **perceptual**
//! hash (dHash) over a texture for *near*-duplicate hints (robust to scale/recompression), on top of the
//! exact dedup. Pure-Rust + `wasm32`-clean (no new dependency). The C2PA-manifest signing/validation
//! backing + the offline auto-rig bake are named seams behind a `Provenance` trait (see ADR-044); this is
//! the dependency-free first backing of the SA-34 trust layer (classed UNIQUELY-ENABLED by *integration*).

// Pixel/grid value casts (luma in 0..255, grid-cell sampling, brightness → u8) are intentional + bounded;
// the perceptual hash is a fingerprint, not a precise measurement.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]

use crate::mesh::Texture;

/// How an asset entered the project — drives the trust surface (an AI-generated asset is honestly flagged).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKind {
    /// Imported from a file (glTF/FBX/OBJ/image/…).
    Imported,
    /// Produced by the generation tier (carries its prompt + provider + the AI-generated flag).
    Generated,
}

/// An asset's provenance record — co-located with the asset by content address. A STABLE, inspectable field
/// (never cosmetic copy). The C2PA manifest (cryptographic sign/validate) is a named seam behind a
/// `Provenance` trait (ADR-044); this struct is the dependency-free first backing.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Provenance {
    /// Imported vs generated (`None` = unknown / legacy asset).
    pub kind: Option<AssetKind>,
    /// Where it came from — a file name, a marketplace entry id, or a provider tag.
    pub source: String,
    /// Whether the asset was AI-generated (honestly surfaced in the UI; EU-AI-Act-relevant context).
    pub ai_generated: bool,
    /// The generation prompt (generated assets only).
    pub prompt: Option<String>,
    /// The generation provider (generated assets only).
    pub provider: Option<String>,
    /// The exact content-address hash (the store's existing hash — referenced, not rebuilt).
    pub content_hash: String,
    /// A perceptual (dHash) fingerprint of the asset's primary texture, for near-duplicate detection.
    pub perceptual_hash: u64,
}

impl Provenance {
    /// An imported asset's provenance (not AI-generated). `content_hash` is the store's existing hash.
    #[must_use]
    pub fn imported(
        source: impl Into<String>,
        content_hash: impl Into<String>,
        perceptual_hash: u64,
    ) -> Self {
        Self {
            kind: Some(AssetKind::Imported),
            source: source.into(),
            content_hash: content_hash.into(),
            perceptual_hash,
            ..Self::default()
        }
    }

    /// A generated asset's provenance — carries its prompt + provider + the honestly-surfaced AI flag (the
    /// inherited M6 residual). Ties to the M7 economy at the call site (a paid/generated asset).
    #[must_use]
    pub fn generated(
        prompt: impl Into<String>,
        provider: impl Into<String>,
        content_hash: impl Into<String>,
        perceptual_hash: u64,
    ) -> Self {
        let provider = provider.into();
        Self {
            kind: Some(AssetKind::Generated),
            source: provider.clone(),
            ai_generated: true,
            prompt: Some(prompt.into()),
            provider: Some(provider),
            content_hash: content_hash.into(),
            perceptual_hash,
        }
    }
}

/// A **dHash** (difference hash) perceptual fingerprint of `tex`: box-sample to a 9×8 grayscale grid, then
/// emit one bit per adjacent horizontal pair (left brighter than right) → a 64-bit hash. Two images with a
/// small Hamming distance are near-duplicates (robust to scale / minor edits / recompression). `0` for an
/// empty texture. Deterministic + pure-Rust.
#[must_use]
pub fn perceptual_hash(tex: &Texture) -> u64 {
    const W: usize = 9; // 9 columns → 8 horizontal comparisons per row
    const H: usize = 8; // 8 rows → 64 bits
    if tex.width == 0 || tex.height == 0 || tex.rgba8.len() < (tex.width * tex.height * 4) as usize
    {
        return 0;
    }
    let mut gray = [[0.0f32; W]; H];
    for (gy, row) in gray.iter_mut().enumerate() {
        for (gx, g) in row.iter_mut().enumerate() {
            // Centre-sample the source pixel for this grid cell.
            let sx = (((gx as u32) * 2 + 1) * tex.width / (2 * W as u32)).min(tex.width - 1);
            let sy = (((gy as u32) * 2 + 1) * tex.height / (2 * H as u32)).min(tex.height - 1);
            let i = ((sy * tex.width + sx) * 4) as usize;
            let (r, gg, b) = (
                f32::from(tex.rgba8[i]),
                f32::from(tex.rgba8[i + 1]),
                f32::from(tex.rgba8[i + 2]),
            );
            *g = 0.299 * r + 0.587 * gg + 0.114 * b; // luma
        }
    }
    let mut hash = 0u64;
    let mut bit = 0u32;
    for row in &gray {
        for x in 0..W - 1 {
            if row[x] > row[x + 1] {
                hash |= 1u64 << bit;
            }
            bit += 1;
        }
    }
    hash
}

/// Hamming distance between two perceptual hashes (differing bits) — smaller = more similar.
#[must_use]
pub fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// Whether two perceptual hashes are near-duplicates (Hamming distance ≤ `threshold`; ~10 of 64 is a common
/// cutoff). A HINT for the import/marketplace path on top of the store's exact dedup — never a silent merge.
#[must_use]
pub fn is_near_duplicate(a: u64, b: u64, threshold: u32) -> bool {
    hamming_distance(a, b) <= threshold
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grayscale horizontal "triangle" (dark edges, bright centre) — a structured, scale-robust pattern.
    fn peak(w: u32, h: u32) -> Texture {
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for _y in 0..h {
            for x in 0..w {
                let t = if w > 1 {
                    x as f32 / (w - 1) as f32
                } else {
                    0.5
                };
                let v = (255.0 * (1.0 - (2.0 * t - 1.0).abs())) as u8;
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        Texture {
            width: w,
            height: h,
            rgba8: rgba,
        }
    }

    fn solid(w: u32, h: u32) -> Texture {
        Texture {
            width: w,
            height: h,
            rgba8: (0..w * h).flat_map(|_| [128, 128, 128, 255]).collect(),
        }
    }

    #[test]
    fn identical_textures_hash_identically() {
        let h = perceptual_hash(&peak(64, 64));
        assert_eq!(h, perceptual_hash(&peak(64, 64)), "deterministic");
        assert_eq!(hamming_distance(h, h), 0);
        assert_ne!(h, 0, "a structured texture has a non-trivial hash");
    }

    #[test]
    fn a_rescaled_copy_is_a_near_duplicate() {
        // The same pattern at very different resolutions → a small Hamming distance (perceptual = scale-robust).
        let a = perceptual_hash(&peak(160, 120));
        let b = perceptual_hash(&peak(48, 24));
        let d = hamming_distance(a, b);
        assert!(
            is_near_duplicate(a, b, 8),
            "a rescaled copy is a near-duplicate (dist {d})"
        );
    }

    #[test]
    fn a_different_texture_is_not_a_near_duplicate() {
        // A structured pattern vs a flat fill → a large Hamming distance (not a dup at the dedup threshold).
        let a = perceptual_hash(&peak(64, 64));
        let b = perceptual_hash(&solid(64, 64));
        let d = hamming_distance(a, b);
        assert!(
            !is_near_duplicate(a, b, 8),
            "a pattern and a flat fill are not near-duplicates (dist {d})"
        );
    }

    #[test]
    fn empty_texture_hashes_to_zero() {
        assert_eq!(
            perceptual_hash(&Texture {
                width: 0,
                height: 0,
                rgba8: vec![]
            }),
            0
        );
    }

    #[test]
    fn provenance_records_carry_identity() {
        let imp = Provenance::imported("spider.fbx", "mtkasset:abc", 42);
        assert_eq!(imp.kind, Some(AssetKind::Imported));
        assert!(!imp.ai_generated);
        assert_eq!(imp.content_hash, "mtkasset:abc");

        let gen = Provenance::generated("a glowing health bar", "stub", "mtkasset:def", 7);
        assert_eq!(gen.kind, Some(AssetKind::Generated));
        assert!(gen.ai_generated, "a generated asset is honestly flagged");
        assert_eq!(gen.prompt.as_deref(), Some("a glowing health bar"));
        assert_eq!(gen.provider.as_deref(), Some("stub"));
    }
}
