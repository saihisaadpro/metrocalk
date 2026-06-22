//! Audio asset import (M10.2) — **WAV / OGG → an internal [`AudioAsset`] stored by handle**, the second
//! user-content format (beside the M4 glTF mesh path). **Playback is the audio milestone, NOT here**: we
//! VALIDATE the file + parse its metadata (format · sample rate · channels · duration) and keep the bytes
//! **content-addressed** ([`crate::store::AssetId`]), so a placed audio entity reload-survives. Same
//! untrusted-asset discipline as the mesh path (deliverable 8): the input is **size-capped**, a malformed
//! header is **rejected with an explained error** (never a panic/hang), and — unlike glTF — an audio file
//! carries **no external URIs to fetch**, so there is no external-reference attack surface.
//!
//! The header parse is **hand-rolled** (a bounds-checked little-endian cursor) — **no foreign decoder type**
//! crosses the boundary (so the CI grep-gate that confines `gltf::`/`image::` to the importer wrapper has
//! nothing to police here; invariant 5 holds by construction). Pure Rust → `wasm32`-clean.

// Header arithmetic: u64→f64→f32 for the duration is intentional + bounded by the size cap; the
// single-char le-cursor names read clearest.
#![allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]

use std::collections::{HashMap, HashSet};

use crate::source::{ImportError, MAX_IMPORT_BYTES};
use crate::store::AssetId;

/// The container format an [`AudioAsset`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioFormat {
    /// RIFF/WAVE PCM.
    Wav,
    /// Ogg (Vorbis).
    Ogg,
}

/// A decoded-enough audio asset: the parsed metadata + the **original bytes** (stored content-addressed;
/// decode-to-samples is the audio-playback milestone). The entity references it by [`AssetId`] handle.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioAsset {
    /// The container format.
    pub format: AudioFormat,
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Channel count (1 = mono, 2 = stereo, …).
    pub channels: u16,
    /// Playback length in seconds (best-effort for Ogg from the last page's granule; 0 if unknown).
    pub duration_secs: f32,
    /// The original file bytes — stored by handle so the placed entity reload-survives (invariant 2:
    /// only the handle enters the doc, never the samples).
    pub bytes: Vec<u8>,
}

/// A source of importable audio — the trait wrapping the (hand-rolled) parsers, mirroring
/// [`crate::source::MeshSource`]. A second backend (a real decoder, a server transcode) slots in by
/// implementing this; no foreign type crosses out.
pub trait AudioSource {
    /// A short identifier for the formats this source accepts (e.g. `"wav/ogg"`).
    fn format(&self) -> &'static str;

    /// Import a self-contained audio file from in-memory bytes.
    ///
    /// # Errors
    /// [`ImportError`] on an oversized input, an unrecognized container, or a malformed/truncated header.
    fn import(&self, bytes: &[u8]) -> Result<AudioAsset, ImportError>;
}

/// The built-in WAV/OGG importer. Stateless.
#[derive(Debug, Default, Clone, Copy)]
pub struct AudioImporter;

impl AudioImporter {
    /// Construct the importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl AudioSource for AudioImporter {
    fn format(&self) -> &'static str {
        "wav/ogg"
    }

    fn import(&self, bytes: &[u8]) -> Result<AudioAsset, ImportError> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }
        // Dispatch by magic (the first bytes of the container) — never by a caller-supplied extension.
        if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
            parse_wav(bytes)
        } else if bytes.starts_with(b"OggS") {
            parse_ogg(bytes)
        } else {
            Err(ImportError::Malformed(
                "not a WAV (RIFF/WAVE) or OGG (OggS) audio file".into(),
            ))
        }
    }
}

/// Parse a RIFF/WAVE PCM header: walk the chunk list for `fmt ` (format params) + `data` (sample bytes →
/// duration). Every read is bounds-checked → a truncated/hostile file is a [`ImportError::Malformed`],
/// never a panic or over-read.
fn parse_wav(bytes: &[u8]) -> Result<AudioAsset, ImportError> {
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut byte_rate = 0u32;
    let mut data_len = 0u64;
    let mut found_fmt = false;
    let mut found_data = false;

    let mut pos = 12usize; // after "RIFF"<size>"WAVE"
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let size = u32_le(bytes, pos + 4)? as usize;
        let body = pos + 8;
        let end = body
            .checked_add(size)
            .ok_or_else(malformed("chunk size overflow"))?;
        if end > bytes.len() {
            // a chunk that claims more than the file holds → malformed
            return Err(ImportError::Malformed(
                "WAV chunk runs past end of file".into(),
            ));
        }
        if id == b"fmt " {
            if size < 16 {
                return Err(ImportError::Malformed("WAV fmt chunk too short".into()));
            }
            // fmt: audioFormat u16 | channels u16 | sampleRate u32 | byteRate u32 | blockAlign u16 | bits u16
            channels = u16_le(bytes, body + 2)?;
            sample_rate = u32_le(bytes, body + 4)?;
            byte_rate = u32_le(bytes, body + 8)?;
            found_fmt = true;
        } else if id == b"data" {
            data_len = size as u64;
            found_data = true;
        }
        // chunks are word-aligned: a body of odd size has a pad byte.
        pos = end + (size & 1);
    }

    if !found_fmt || channels == 0 || sample_rate == 0 {
        return Err(ImportError::Malformed(
            "WAV missing/invalid fmt chunk".into(),
        ));
    }
    let duration = if found_data && byte_rate > 0 {
        (data_len as f64 / f64::from(byte_rate)) as f32
    } else {
        0.0
    };
    Ok(AudioAsset {
        format: AudioFormat::Wav,
        sample_rate,
        channels,
        duration_secs: duration,
        bytes: bytes.to_vec(),
    })
}

/// Parse an Ogg(Vorbis) container's metadata: validate the `OggS` capture pattern, read the **Vorbis
/// identification header** (channels + sample rate) from the first packet, and take the **last page's
/// granule position** as the total sample count → duration (best-effort; 0 if absent/unknown). Bounds-
/// checked throughout. We do NOT decode audio (playback milestone) and we never fetch anything.
fn parse_ogg(bytes: &[u8]) -> Result<AudioAsset, ImportError> {
    // The Vorbis ID header packet: [0x01]"vorbis" | version u32 | channels u8 | sampleRate u32 | …
    let marker = b"\x01vorbis";
    let id = find_subslice(bytes, marker)
        .ok_or_else(malformed("OGG: no Vorbis identification header"))?;
    let channels_at = id + 11;
    let rate_at = id + 12;
    let channels = u16::from(
        *bytes
            .get(channels_at)
            .ok_or_else(malformed("OGG: truncated id header"))?,
    );
    let sample_rate = u32_le(bytes, rate_at)?;
    if channels == 0 || sample_rate == 0 {
        return Err(ImportError::Malformed(
            "OGG: zero channels/sample-rate".into(),
        ));
    }

    // Last page's granule position (u64 LE at offset 6 of an "OggS" page header) = total samples.
    let duration =
        last_ogg_granule(bytes).map_or(0.0, |g| g as f64 / f64::from(sample_rate)) as f32;

    Ok(AudioAsset {
        format: AudioFormat::Ogg,
        sample_rate,
        channels,
        duration_secs: duration,
        bytes: bytes.to_vec(),
    })
}

/// The granule position of the LAST `OggS` page, if readable (and not the `u64::MAX` "unknown" sentinel).
fn last_ogg_granule(bytes: &[u8]) -> Option<u64> {
    let pat = b"OggS";
    let mut last = None;
    let mut i = 0usize;
    while let Some(rel) = find_subslice(&bytes[i..], pat) {
        last = Some(i + rel);
        i += rel + 1;
    }
    let page = last?;
    let g = u64::from_le_bytes(bytes.get(page + 6..page + 14)?.try_into().ok()?);
    if g == u64::MAX {
        None
    } else {
        Some(g)
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn u16_le(b: &[u8], o: usize) -> Result<u16, ImportError> {
    Ok(u16::from_le_bytes(
        b.get(o..o + 2)
            .ok_or_else(malformed("truncated u16"))?
            .try_into()
            .map_err(|_| ImportError::Malformed("bad u16".into()))?,
    ))
}
fn u32_le(b: &[u8], o: usize) -> Result<u32, ImportError> {
    Ok(u32::from_le_bytes(
        b.get(o..o + 4)
            .ok_or_else(malformed("truncated u32"))?
            .try_into()
            .map_err(|_| ImportError::Malformed("bad u32".into()))?,
    ))
}
fn malformed(why: &'static str) -> impl Fn() -> ImportError {
    move || ImportError::Malformed(why.into())
}

/// A content-addressed audio store beside the scene doc — the audio analog of [`crate::store::AssetStore`]
/// (identical handle space: same bytes → same [`AssetId`]). Imported audio dedups for free; an entity
/// carries only the handle (invariant 2). (Mesh + audio stores are parallel today — a unified
/// `Store<Asset>` is the documented refactor seam.)
#[derive(Default)]
pub struct AudioStore {
    assets: HashMap<AssetId, AudioAsset>,
}

impl AudioStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Import `bytes` through `source`, store under the content address, return the handle. Re-importing
    /// identical bytes is idempotent (dedup).
    ///
    /// # Errors
    /// Propagates the [`AudioSource`]'s [`ImportError`].
    pub fn import<S: AudioSource>(
        &mut self,
        source: &S,
        bytes: &[u8],
    ) -> Result<AssetId, ImportError> {
        let id = AssetId::of_bytes(bytes);
        if !self.assets.contains_key(&id) {
            let asset = source.import(bytes)?;
            self.assets.insert(id.clone(), asset);
        }
        Ok(id)
    }

    /// Look up an audio asset by raw handle string.
    #[must_use]
    pub fn get_str(&self, handle: &str) -> Option<&AudioAsset> {
        self.assets.get(&AssetId::from_handle(handle))
    }

    /// Distinct assets held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    /// Whether the store is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    /// **The unreferenced-cleanup (GC) seam** (deliverable 9): drop every asset whose handle is not in the
    /// live set (the scene's referenced handles). Returns how many were collected. Cheap, explicit, and
    /// caller-driven — the engine passes the set of handles still referenced by entities.
    pub fn gc(&mut self, live: &HashSet<AssetId>) -> usize {
        let before = self.assets.len();
        self.assets.retain(|id, _| live.contains(id));
        before - self.assets.len()
    }
}

#[cfg(test)]
mod tests;
