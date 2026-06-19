//! The project-owned import boundary (invariant 5). [`MeshSource`] is the trait every importer
//! implements; its signatures speak only [`MeshAsset`] + [`ImportError`] — never a `gltf::` or
//! `image::` type. The glTF backend lives behind it ([`crate::gltf_import`]), exactly as Flecs lives
//! behind `/ecs` and Loro behind `/core`'s commit pipeline. A second backend (FBX, OBJ, a server
//! decoder) slots in by implementing this same trait, with no change to the store or the renderer.

use crate::mesh::MeshAsset;

/// The maximum input blob we will attempt to import, in bytes. A guard against a pathologically-large
/// (or hostile) file exhausting memory before we even parse it — the import is a one-shot heavy op,
/// not frame-budgeted, but it must still be bounded. 64 MiB comfortably covers a real game asset while
/// refusing a multi-gigabyte bomb. Callers that genuinely need more import via [`MeshSource::import`]
/// after their own size policy.
pub const MAX_IMPORT_BYTES: usize = 64 * 1024 * 1024;

/// The maximum vertex / index count we accept from a single asset — a second guard, against a small
/// file that decodes to a ruinously large mesh (e.g. a crafted accessor count). 8M of each is far past
/// any hand-placed editor asset while still refusing a decode bomb.
pub const MAX_ELEMENTS: usize = 8_000_000;

/// Why an import failed — actionable, foreign-type-free (no `gltf::Error` leaks across the boundary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    /// The input exceeded [`MAX_IMPORT_BYTES`].
    TooLarge {
        /// The offending input size.
        bytes: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The decoded mesh exceeded [`MAX_ELEMENTS`] vertices or indices.
    TooManyElements {
        /// The offending count.
        count: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The bytes are not a parseable glTF/glb, or reference external resources we don't load
    /// (this importer accepts only self-contained `.glb` / embedded buffers). The string is a
    /// human-readable reason (the underlying decoder's message, flattened — never its type).
    Malformed(String),
    /// The asset parsed but carried no drawable geometry (no positions on any primitive).
    NoGeometry,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { bytes, limit } => {
                write!(f, "asset too large: {bytes} bytes (limit {limit})")
            }
            Self::TooManyElements { count, limit } => {
                write!(f, "asset has too many elements: {count} (limit {limit})")
            }
            Self::Malformed(why) => write!(f, "malformed asset: {why}"),
            Self::NoGeometry => write!(f, "asset has no drawable geometry"),
        }
    }
}

impl std::error::Error for ImportError {}

/// A source of importable meshes — the trait wrapping a concrete decoder. Implementors validate size
/// + element limits and return the project's internal [`MeshAsset`]; no foreign type crosses out.
pub trait MeshSource {
    /// A short identifier for the formats this source accepts (e.g. `"gltf/glb"`) — for logs/UX.
    fn format(&self) -> &'static str;

    /// Import a self-contained asset from in-memory bytes.
    ///
    /// # Errors
    /// [`ImportError`] on an oversized input, an over-large decoded mesh, malformed bytes, or an asset
    /// with no geometry.
    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError>;
}
