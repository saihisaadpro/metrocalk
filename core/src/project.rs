//! The `.mtk` **project document** format — a versioned envelope around a Loro snapshot (ADR-033,
//! superseding the seed+replay scaffold of ADR-013 for real projects).
//!
//! A project IS the Loro document (snapshot + oplog — ADR-002: "its oplog *is* the WAL"); this module
//! is only the thin, **versioned** wrapper that lets the format evolve without stranding a user's work:
//!
//! ```text
//! bytes 0..4   MAGIC  = b"MTKP"
//! bytes 4..8   format version : u32 little-endian
//! bytes 8..    the Loro snapshot (ExportMode::Snapshot — carries history)
//! ```
//!
//! [`build`] wraps a snapshot for saving; [`parse`] validates + **migrates an older version forward**
//! (or refuses, with an explained error, a file from a newer build or a truncated one) and returns the
//! Loro snapshot bytes ready to `import`/`merge`. Capabilities ride along because they are mirrored into
//! the document (ADR-032), so opening restores the reveal/bind compat query.
//!
//! Pure bytes in / bytes out — no ECS, no Loro, no file IO (those live in `editor-shell::project`), so
//! the envelope is **wasm-portable**: the browser funnel (ADR-006) opens the *same* `.mtk` format over
//! the same Loro snapshot once its pure-Rust query backend lands (the funnel carry-forward).

use std::cmp::Ordering;
use thiserror::Error;

/// The current on-disk format version a fresh save writes. Bump when the **envelope or the persisted
/// document schema** changes in a way an older build couldn't read; add a [`migrate`] step in the same
/// change so an older project still opens (deliverable 7 — never strand a user's work).
pub const FORMAT_VERSION: u32 = 1;

/// The magic prefix identifying a versioned `.mtk` envelope. Bytes that don't start with it are treated
/// as a **legacy bare Loro snapshot** (the pre-versioned "v0" form) and migrated forward.
const MAGIC: &[u8; 4] = b"MTKP";

/// Envelope header length: `MAGIC` (4) + version (4, little-endian).
const HEADER_LEN: usize = 8;

/// A project-document load error — every variant carries an explained, user-facing message; opening a
/// bad file is **never** a crash (the adversarial guard: a corrupt/truncated `.mtk` must not panic Open).
#[derive(Error, Debug)]
pub enum ProjectError {
    /// The file is truncated or not a Metrocalk project.
    #[error("not a Metrocalk project, or the file is truncated/corrupt ({0})")]
    Corrupt(String),
    /// The project was saved by a newer build than this one can read.
    #[error(
        "this project was saved by a newer Metrocalk (format v{found}); this build reads up to v{supported} — please update to open it"
    )]
    TooNew {
        /// The format version found in the file.
        found: u32,
        /// The highest format version this build supports ([`FORMAT_VERSION`]).
        supported: u32,
    },
    /// No migration path is registered from the file's (older) version to the current one.
    #[error("can't open this project: no migration path from format v{0} to v{FORMAT_VERSION}")]
    UnsupportedVersion(u32),
}

/// Wrap a Loro snapshot in the current `.mtk` envelope, ready to write to disk.
#[must_use]
pub fn build(snapshot: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + snapshot.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(snapshot);
    out
}

/// Validate + migrate a `.mtk` file's bytes and return the Loro snapshot to `import`/`merge`.
///
/// - Current version → the snapshot as-is.
/// - Older version (or a legacy header-less bare snapshot, "v0") → [`migrate`] forward.
/// - Newer version → [`ProjectError::TooNew`] (refuse, don't guess).
/// - A short/truncated `MTKP` file → [`ProjectError::Corrupt`].
///
/// (A bare-snapshot's bytes that aren't actually a valid Loro document are caught downstream when the
/// caller `merge`s them — surfaced as an explained error there, still never a panic.)
///
/// # Errors
/// See [`ProjectError`].
pub fn parse(bytes: &[u8]) -> Result<Vec<u8>, ProjectError> {
    if bytes.len() >= 4 && &bytes[0..4] == MAGIC {
        if bytes.len() < HEADER_LEN {
            return Err(ProjectError::Corrupt(format!(
                "header is {} bytes, need {HEADER_LEN}",
                bytes.len()
            )));
        }
        let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        let payload = &bytes[HEADER_LEN..];
        match version.cmp(&FORMAT_VERSION) {
            Ordering::Equal => Ok(payload.to_vec()),
            Ordering::Less => migrate(version, payload),
            Ordering::Greater => Err(ProjectError::TooNew {
                found: version,
                supported: FORMAT_VERSION,
            }),
        }
    } else {
        // No MTKP header → a legacy bare Loro snapshot (the pre-versioned export). Migrate forward from
        // "v0" so an early/foreign export still opens rather than being rejected.
        migrate(0, bytes)
    }
}

/// Upgrade a `from`-version payload to the current [`FORMAT_VERSION`] — the **migration seam**. Today
/// the document schema is stable from the legacy bare-snapshot ("v0") through v1, so the payload (a Loro
/// snapshot) is carried forward unchanged; a future schema bump adds a real step here (e.g. `1 => { ..
/// transform .. ; migrate(2, &upgraded) }`), keeping migration a single chained function so every older
/// project opens in every newer build.
fn migrate(from: u32, payload: &[u8]) -> Result<Vec<u8>, ProjectError> {
    match from {
        // v0 (legacy bare snapshot) and v1 share the same Loro document schema → identity payload.
        0 | 1 => Ok(payload.to_vec()),
        other => Err(ProjectError::UnsupportedVersion(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_the_current_version() {
        let snapshot = b"\x01\x02\x03 loro snapshot bytes".to_vec();
        let file = build(&snapshot);
        assert_eq!(&file[0..4], MAGIC);
        assert_eq!(
            u32::from_le_bytes([file[4], file[5], file[6], file[7]]),
            FORMAT_VERSION
        );
        assert_eq!(
            parse(&file).unwrap(),
            snapshot,
            "the snapshot survives the envelope round-trip"
        );
    }

    #[test]
    fn a_newer_version_is_refused_with_an_explained_error() {
        let mut file = build(b"snapshot");
        // bump the version field past what this build supports
        file[4..8].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        match parse(&file) {
            Err(ProjectError::TooNew { found, supported }) => {
                assert_eq!(found, FORMAT_VERSION + 1);
                assert_eq!(supported, FORMAT_VERSION);
            }
            other => panic!("expected TooNew, got {other:?}"),
        }
    }

    #[test]
    fn a_legacy_bare_snapshot_migrates_forward() {
        // The pre-versioned format was a bare Loro snapshot with no MTKP header — it must still open
        // (migrate v0 → current), never be rejected.
        let bare = b"a legacy bare loro snapshot".to_vec();
        assert_eq!(
            parse(&bare).unwrap(),
            bare,
            "a header-less legacy snapshot migrates to the current version (payload carried forward)"
        );
    }

    #[test]
    fn a_truncated_mtk_header_is_corrupt_not_a_panic() {
        let truncated = b"MTKP\x01\x00".to_vec(); // MAGIC + only 2 of the 4 version bytes
        match parse(&truncated) {
            Err(ProjectError::Corrupt(_)) => {}
            other => panic!("expected Corrupt, got {other:?}"),
        }
    }
}
