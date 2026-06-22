//! Audio import (M10.2) headless spine: WAV + OGG parse to the right metadata, the content-addressed
//! store dedups + GC-collects, and the untrusted-asset guards hold (oversize / bad-magic / truncated →
//! an EXPLAINED rejection, never a panic).

// Test fixtures: literal byte layouts + small-count casts are intentional + bounded.
#![allow(clippy::cast_possible_truncation)]

use super::*;
use crate::source::MAX_IMPORT_BYTES;
use std::collections::HashSet;

/// A valid RIFF/WAVE PCM file of `frames` silent frames.
fn wav(sample_rate: u32, channels: u16, bits: u16, frames: u32) -> Vec<u8> {
    let block_align = channels * (bits / 8);
    let byte_rate = sample_rate * u32::from(block_align);
    let data_len = frames * u32::from(block_align);
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data_len).to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&channels.to_le_bytes());
    b.extend_from_slice(&sample_rate.to_le_bytes());
    b.extend_from_slice(&byte_rate.to_le_bytes());
    b.extend_from_slice(&block_align.to_le_bytes());
    b.extend_from_slice(&bits.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data_len.to_le_bytes());
    b.resize(b.len() + data_len as usize, 0); // silence
    b
}

/// A minimal single-page Ogg/Vorbis file: the BOS page carrying the 30-byte Vorbis identification
/// header, with `granule` as the page's granule position (= total samples).
fn ogg(sample_rate: u32, channels: u8, granule: u64) -> Vec<u8> {
    let mut packet = Vec::new();
    packet.push(0x01);
    packet.extend_from_slice(b"vorbis");
    packet.extend_from_slice(&0u32.to_le_bytes()); // vorbis_version
    packet.push(channels);
    packet.extend_from_slice(&sample_rate.to_le_bytes());
    packet.extend_from_slice(&0u32.to_le_bytes()); // bitrate_max
    packet.extend_from_slice(&0u32.to_le_bytes()); // bitrate_nominal
    packet.extend_from_slice(&0u32.to_le_bytes()); // bitrate_min
    packet.push(0u8); // blocksizes
    packet.push(0x01); // framing
    debug_assert_eq!(packet.len(), 30);

    let mut b = Vec::new();
    b.extend_from_slice(b"OggS");
    b.push(0); // stream structure version
    b.push(0x02); // header_type: BOS
    b.extend_from_slice(&granule.to_le_bytes());
    b.extend_from_slice(&1u32.to_le_bytes()); // bitstream serial
    b.extend_from_slice(&0u32.to_le_bytes()); // page sequence
    b.extend_from_slice(&0u32.to_le_bytes()); // crc (unchecked)
    b.push(1u8); // page_segments
    b.push(packet.len() as u8); // segment table
    b.extend_from_slice(&packet);
    b
}

#[test]
fn imports_a_wav_with_its_metadata_and_duration() {
    let a = AudioImporter::new()
        .import(&wav(44_100, 2, 16, 44_100))
        .expect("wav");
    assert_eq!(a.format, AudioFormat::Wav);
    assert_eq!(a.sample_rate, 44_100);
    assert_eq!(a.channels, 2);
    assert!(
        (a.duration_secs - 1.0).abs() < 1e-3,
        "1s of 44.1k stereo, got {}",
        a.duration_secs
    );
    assert!(
        !a.bytes.is_empty(),
        "the original bytes are kept (stored by handle)"
    );
}

#[test]
fn imports_an_ogg_with_channels_rate_and_duration() {
    let a = AudioImporter::new()
        .import(&ogg(48_000, 1, 48_000))
        .expect("ogg");
    assert_eq!(a.format, AudioFormat::Ogg);
    assert_eq!(a.sample_rate, 48_000);
    assert_eq!(a.channels, 1);
    assert!(
        (a.duration_secs - 1.0).abs() < 1e-3,
        "granule 48000 / 48k = 1s, got {}",
        a.duration_secs
    );
}

#[test]
fn rejects_an_oversized_file_explained() {
    let big = vec![0u8; MAX_IMPORT_BYTES + 1];
    let err = AudioImporter::new().import(&big).unwrap_err();
    assert!(matches!(err, ImportError::TooLarge { .. }), "got {err:?}");
}

#[test]
fn rejects_an_unrecognized_container_explained() {
    let err = AudioImporter::new()
        .import(b"not an audio file at all")
        .unwrap_err();
    assert!(matches!(err, ImportError::Malformed(_)), "got {err:?}");
}

#[test]
fn rejects_a_truncated_or_lying_wav_without_panicking() {
    // A WAV whose fmt chunk claims a size that runs past EOF → rejected, never an over-read/panic.
    let mut b = Vec::new();
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&100u32.to_le_bytes());
    b.extend_from_slice(b"WAVE");
    b.extend_from_slice(b"fmt ");
    b.extend_from_slice(&9_999u32.to_le_bytes()); // lies: 9999 bytes that aren't there
    let err = AudioImporter::new().import(&b).unwrap_err();
    assert!(matches!(err, ImportError::Malformed(_)), "got {err:?}");

    // A WAV with no fmt chunk at all → malformed (missing format).
    let mut nofmt = Vec::new();
    nofmt.extend_from_slice(b"RIFF");
    nofmt.extend_from_slice(&4u32.to_le_bytes());
    nofmt.extend_from_slice(b"WAVE");
    assert!(matches!(
        AudioImporter::new().import(&nofmt).unwrap_err(),
        ImportError::Malformed(_)
    ));
}

#[test]
fn an_ogg_without_a_vorbis_header_is_rejected() {
    // "OggS" magic but no Vorbis identification packet → malformed (not a crash).
    let mut b = Vec::new();
    b.extend_from_slice(b"OggS");
    b.resize(40, 0);
    assert!(matches!(
        AudioImporter::new().import(&b).unwrap_err(),
        ImportError::Malformed(_)
    ));
}

#[test]
fn the_store_dedups_by_content_and_gc_collects_the_unreferenced() {
    let imp = AudioImporter::new();
    let mut store = AudioStore::new();
    let bytes = wav(22_050, 1, 16, 100);

    let h1 = store.import(&imp, &bytes).expect("import");
    let h2 = store.import(&imp, &bytes).expect("re-import");
    assert_eq!(h1, h2, "same bytes → same handle");
    assert_eq!(store.len(), 1, "identical audio dedups");
    assert!(store.get_str(h1.as_str()).is_some());

    // GC with an empty live set collects the unreferenced asset (the unreferenced-cleanup seam).
    let collected = store.gc(&HashSet::new());
    assert_eq!(collected, 1);
    assert!(store.is_empty());

    // GC keeps a still-referenced asset.
    let h = store.import(&imp, &bytes).expect("re-add");
    let mut live = HashSet::new();
    live.insert(h.clone());
    assert_eq!(store.gc(&live), 0, "a referenced asset is kept");
    assert_eq!(store.len(), 1);
}
