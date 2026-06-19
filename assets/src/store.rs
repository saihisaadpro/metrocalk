//! The asset store — **id-keyed, content-addressed, and beside the scene document** (never inside the
//! Loro doc). An entity references an asset by its [`AssetId`] handle (invariant 2: the doc + every
//! projection delta carries only the lightweight string, never geometry); the store maps that handle
//! back to the [`MeshAsset`]. Content-addressing (a hash of the import bytes) means the same file
//! always gets the same handle, so a persisted handle re-resolves after a reload (deterministic id
//! space, ADR-013) and identical assets de-duplicate for free.

use std::collections::HashMap;

use crate::mesh::MeshAsset;
use crate::source::{ImportError, MeshSource};

/// A stable, content-derived asset handle — the lightweight string an entity carries (invariant 2).
/// Rendered as `mtkasset:<32-hex>` so it is self-describing in the inspector and a saved scene log.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AssetId(String);

impl AssetId {
    /// The content-address of `bytes` — FNV-1a 128-bit, hex. Pure, allocation-light, and `wasm32`
    /// trivial (no crate, no C); a non-cryptographic content id is all the store needs.
    #[must_use]
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(format!("mtkasset:{:032x}", fnv1a_128(bytes)))
    }

    /// The handle string (what lands in a `MeshRenderer.mesh` field / the scene doc).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reconstruct a handle from a stored string (e.g. read back off a `MeshRenderer.mesh` field).
    #[must_use]
    pub fn from_handle(s: impl Into<String>) -> Self {
        Self(s.into())
    }
}

impl std::fmt::Display for AssetId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// FNV-1a 128-bit over `bytes`.
#[must_use]
fn fnv1a_128(bytes: &[u8]) -> u128 {
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u128::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// The in-memory asset store. Lives beside the engine (not in the Loro doc); rebuilt at launch by
/// re-importing the same source bytes, which (content-addressing) reproduces the same handles.
#[derive(Default)]
pub struct AssetStore {
    assets: HashMap<AssetId, MeshAsset>,
}

impl AssetStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Import `bytes` through `source` and store the result under its content address, returning the
    /// handle. Re-importing identical bytes is idempotent (same handle, no duplicate stored).
    ///
    /// # Errors
    /// Propagates the [`MeshSource`]'s [`ImportError`].
    pub fn import<S: MeshSource>(
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

    /// Insert an already-imported asset under an explicit id (e.g. a synthetic/test asset). Returns
    /// the previous asset at that id, if any.
    pub fn insert(&mut self, id: AssetId, asset: MeshAsset) -> Option<MeshAsset> {
        self.assets.insert(id, asset)
    }

    /// Look up an asset by handle.
    #[must_use]
    pub fn get(&self, id: &AssetId) -> Option<&MeshAsset> {
        self.assets.get(id)
    }

    /// Look up by raw handle string (the form carried on a `MeshRenderer.mesh` field).
    #[must_use]
    pub fn get_str(&self, handle: &str) -> Option<&MeshAsset> {
        self.assets.get(&AssetId::from_handle(handle))
    }

    /// Whether a handle is known to the store.
    #[must_use]
    pub fn contains(&self, handle: &str) -> bool {
        self.assets.contains_key(&AssetId::from_handle(handle))
    }

    /// Number of distinct assets held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.assets.len()
    }

    /// Whether the store holds no assets.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty()
    }

    /// Iterate `(handle, asset)` pairs (unordered).
    pub fn iter(&self) -> impl Iterator<Item = (&AssetId, &MeshAsset)> {
        self.assets.iter()
    }
}
