//! M15.0 (ADR-070, Leg B) — SDF bake intent: the editor-shell seam that drives the **SDF/implicit
//! canonical rep** ([`metrocalk_sdf`]) and lands the compiled mesh as a **content-addressed
//! [`metrocalk_assets::MeshAsset`]** plus a `place_mesh`-style **undoable commit** — exactly the
//! [`crate::csg_intent`] path, one representation over.
//!
//! An SDF op is "geometry is a program": a [`Sdf`] tree (`box − cylinder`) is compiled **down to a
//! deterministic watertight mesh** (Marching Tetrahedra), validated by the always-on M13.2 validator, and
//! stored by its **content address** — same program + grid ⇒ same bytes ⇒ same handle (dedup + a stable
//! reload handle, ADR-013/014). `/core` stays mesh-agnostic: the geometry lives in the store **by handle**,
//! never in the Loro doc (invariant 2). Deterministic by construction (the SDF field + Marching Tetrahedra
//! are f64, no RNG/rayon — ADR-020 native path); the placement is one undoable transaction (invariant 3).
//! There is **no runtime raymarcher** — SDF is an authoring/baked rep (FF-T8 honest-limit, ADR-070).

use crate::csg_intent::store_mesh;
use metrocalk_assets::AssetStore;
use metrocalk_sdf::{compile, validate, Grid, Sdf};
use std::fmt;

/// A bake failed in a way the user must see (ADR-016) — never a silent crack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SdfBakeError {
    /// The compiled mesh isn't a clean watertight solid (the Blocked-explained reason).
    Blocked(String),
}

impl fmt::Display for SdfBakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Blocked(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for SdfBakeError {}

/// Compile an SDF program on `grid`, validate it is a clean watertight solid, store it by its **content
/// address**, and return the handle. Deterministic (content-addressed) and crack-free (the always-on
/// validator gates the output; a non-watertight compile is [`SdfBakeError::Blocked`], never silent). The
/// caller `place_mesh`es the returned handle in one undoable transaction.
///
/// # Errors
/// [`SdfBakeError::Blocked`] if the field does not compile to a watertight, manifold mesh.
pub fn bake(store: &mut AssetStore, sdf: &Sdf, grid: &Grid) -> Result<String, SdfBakeError> {
    let mesh = compile(sdf, grid);
    let r = validate(&mesh);
    if !(r.watertight && r.manifold) {
        return Err(SdfBakeError::Blocked(format!(
            "SDF compiled to a non-watertight mesh: {}",
            r.explain()
        )));
    }
    Ok(store_mesh(store, &mesh, "sdf"))
}

/// [`bake`] with an auto-sized grid around the field's solid at `res` cells/axis — the ergonomic path a
/// `bake-sdf` command drives.
///
/// # Errors
/// As [`bake`].
pub fn bake_auto(store: &mut AssetStore, sdf: &Sdf, res: usize) -> Result<String, SdfBakeError> {
    bake(store, sdf, &Grid::around(sdf, res, 0.06))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::csg_intent::mesh_asset_to_trimesh;
    use metrocalk_sdf::Axis;

    fn box_minus_cylinder() -> Sdf {
        Sdf::cuboid([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]).difference(Sdf::cylinder(
            [0.0, 0.0, 0.0],
            0.5,
            2.0,
            Axis::Y,
        ))
    }

    #[test]
    fn bake_stores_a_watertight_result_by_content_address() {
        let mut store = AssetStore::new();
        let handle = bake_auto(&mut store, &box_minus_cylinder(), 40).expect("clean bake");
        assert!(store.contains(&handle), "content-addressed in the store");
        let r = validate(&mesh_asset_to_trimesh(store.get_str(&handle).unwrap()));
        assert!(r.watertight && r.manifold, "{}", r.explain());
    }

    #[test]
    fn bake_is_deterministic_same_program_same_handle() {
        // Two independent "sessions": the same SDF program + grid reproduce the SAME content-addressed
        // handle, so a baked mesh re-resolves after a reload (ADR-013/014 + the ADR-020 native-f64 path).
        let mut s1 = AssetStore::new();
        let h1 = bake_auto(&mut s1, &box_minus_cylinder(), 40).unwrap();
        let mut s2 = AssetStore::new();
        let h2 = bake_auto(&mut s2, &box_minus_cylinder(), 40).unwrap();
        assert_eq!(
            h1, h2,
            "a bake re-resolves to the same handle (deterministic)"
        );
    }
}
