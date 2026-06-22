//! Real **project save / open** — the `.mtk` Loro-document file (M10.3, ADR-033). The format envelope +
//! versioning/migration is pure bytes in `metrocalk_core::project`; this is the **native file IO** half:
//! an **atomic, crash-safe** save and a validating open that re-derives capabilities (ADR-032).
//!
//! "Save your work and reopen it" — table stakes the seed+replay scaffold (ADR-013) deferred. A project
//! IS the Loro document (snapshot + oplog, ADR-002); save exports the snapshot and writes it atomically,
//! open reads + migrates + `merge`s it back, which (because caps are mirrored into the document, ADR-032)
//! restores the reveal/bind compatibility query, not just the entities and edges.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use metrocalk_core::project::{self, ProjectError};
use metrocalk_core::{Engine, MergeReport, PipelineError};
use metrocalk_ecs::FlecsWorld;

/// Why opening a `.mtk` project failed — IO, an explained format/version problem, or a document that
/// imported as invalid. Opening a bad file is **never** a crash (the adversarial guard).
#[derive(Debug)]
pub enum OpenError {
    /// The file couldn't be read.
    Io(std::io::Error),
    /// The envelope is corrupt, truncated, or from a newer build (carries the explained reason).
    Format(ProjectError),
    /// The bytes parsed as a project but the Loro document failed to import (corrupt payload).
    Load(PipelineError),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "couldn't read the project file: {e}"),
            Self::Format(e) => write!(f, "{e}"),
            Self::Load(e) => write!(f, "the project document is unreadable: {e}"),
        }
    }
}

impl std::error::Error for OpenError {}

/// The temp path an [`atomic_save`] writes to before renaming over `path` — a sibling
/// `"<path>.tmp"` (same directory ⇒ same volume ⇒ the rename is atomic).
fn tmp_path(path: &Path) -> PathBuf {
    let mut s: OsString = path.as_os_str().to_os_string();
    s.push(".tmp");
    PathBuf::from(s)
}

/// **Atomically** save the engine's document to `path` as a `.mtk` project: write the versioned envelope
/// to a sibling temp file, **fsync** it, then **rename** over the target. A crash mid-save leaves either
/// the old project intact or the temp file behind — **never a half-written, corrupt project** (the
/// crash-safe-save adversarial guard). Two saves of the same scene are byte-identical (no timestamp in
/// the envelope), so a save is deterministic.
///
/// # Errors
/// Any IO failure (create / write / fsync / rename); the temp file is cleaned up on a rename failure.
pub fn save(engine: &Engine<FlecsWorld>, path: &Path) -> std::io::Result<()> {
    atomic_write(path, &project::build(&engine.snapshot()))
}

/// Atomic write-temp → fsync → rename of arbitrary bytes (the crash-safe primitive `save` uses; also the
/// autosave substrate). Public so the autosave/recovery wiring (deliverable 8) reuses one implementation.
///
/// # Errors
/// Propagates any IO failure; removes the temp file if the final rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = tmp_path(path);
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?; // durable on disk BEFORE the rename, so the rename can't expose a partial file
    }
    // `std::fs::rename` replaces the destination atomically (Windows: MoveFileEx REPLACE_EXISTING).
    if let Err(e) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

/// Open a `.mtk` project **into** `engine` — which must already have its capability resolver set (so
/// caps restore, ADR-032). Reads the file, parses + **migrates** the envelope forward (or refuses a
/// newer/corrupt one with an explained error), and `merge`s the Loro snapshot, which rebuilds the ECS
/// **with** its capability pairs. Returns the merge report.
///
/// `engine` should be a freshly-constructed engine for the project (a clean world + scene), so the
/// merged document defines the whole scene.
///
/// # Errors
/// [`OpenError`] — IO, an explained format/version problem, or a corrupt document payload. Never panics.
pub fn open_into(engine: &mut Engine<FlecsWorld>, path: &Path) -> Result<MergeReport, OpenError> {
    let bytes = fs::read(path).map_err(OpenError::Io)?;
    let snapshot = project::parse(&bytes).map_err(OpenError::Format)?;
    engine.merge(&snapshot).map_err(OpenError::Load)
}
