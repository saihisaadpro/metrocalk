//! M13.2 — CSG carve intent: the editor-shell seam that drives a **robust exact-predicate boolean**
//! ([`metrocalk_csg`]) over the asset store and lands the result as a **content-addressed
//! [`MeshAsset`]** plus a `place_mesh`-style **undoable commit** (ADR-051).
//!
//! `/core` stays mesh-agnostic — geometry lives in the store **by handle**, never in the Loro doc
//! (invariant 2) — so the CSG compute happens *here*, exactly like the M6 generation stream-in: read the
//! two input meshes by handle, run the boolean, validate it is watertight, store the result by its content
//! address, and (the caller) `place_mesh` the handle in one undoable transaction.
//!
//! The [`metrocalk_csg::Csg`] trait is the boundary (invariant 5): no `robust::`/`csgrs::` geometry type
//! crosses into the shell — the public surface here is plain handles + [`BoolOp`].

use std::fmt;

use metrocalk_assets::{AssetId, AssetStore, Material, MeshAsset, Primitive};
use metrocalk_csg::{validate, BoolOp, Csg, ExactBspCsg, TriMesh};

/// A carve failed in a way the user must see (ADR-016) — never a silent crack or a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CarveError {
    /// An input handle isn't in the store.
    UnknownInput(String),
    /// The robust boolean refused to produce a clean solid (the Blocked-explained reason).
    Blocked(String),
}

impl fmt::Display for CarveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownInput(h) => write!(f, "no such mesh to carve: {h}"),
            Self::Blocked(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for CarveError {}

/// Parse a boolean-op verb (the form a UI / command carries) into a [`BoolOp`].
#[must_use]
pub fn parse_op(verb: &str) -> Option<BoolOp> {
    match verb {
        "union" => Some(BoolOp::Union),
        "difference" | "subtract" | "carve" => Some(BoolOp::Difference),
        "intersection" | "intersect" => Some(BoolOp::Intersection),
        "xor" => Some(BoolOp::Xor),
        _ => None,
    }
}

/// Flatten a [`MeshAsset`]'s primitives into one `f64` triangle soup the CSG engine speaks.
#[must_use]
pub fn mesh_asset_to_trimesh(asset: &MeshAsset) -> TriMesh {
    let mut positions: Vec<[f64; 3]> = Vec::with_capacity(asset.vertex_count());
    let mut triangles: Vec<[u32; 3]> = Vec::with_capacity(asset.triangle_count());
    for prim in &asset.primitives {
        let base = u32::try_from(positions.len()).unwrap_or(u32::MAX);
        for p in &prim.positions {
            positions.push([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])]);
        }
        let n = u32::try_from(prim.positions.len()).unwrap_or(u32::MAX);
        for t in prim.indices.chunks_exact(3) {
            if t[0] < n && t[1] < n && t[2] < n {
                triangles.push([base + t[0], base + t[1], base + t[2]]);
            }
        }
    }
    TriMesh {
        positions,
        triangles,
    }
}

/// Convert a CSG result back into a single-primitive, static [`MeshAsset`] (the packer derives crease-aware
/// smooth normals — `normals`/`uvs` left empty; default/CAD material).
#[must_use]
pub fn trimesh_to_mesh_asset(mesh: &TriMesh, name: &str) -> MeshAsset {
    trimesh_to_mesh_asset_colored(mesh, name, None)
}

/// As [`trimesh_to_mesh_asset`], but with an explicit authored display colour (linear RGB) for the CAD path —
/// so an imported part renders in its real STEP/3DXML colour. `None` ⇒ the neutral machined-metal default.
///
/// Appearance policy: imported CAD (`"cad"`/`"step"`) gets a **satin machined-metal** look — glossier +
/// slightly metallic vs the matte-plastic engine default — so parts catch a form-revealing specular highlight
/// off the IBL and read as a machine, not a flat white clay model. Kept majority-diffuse (a moderate
/// `metallic`) so an authored colour (or the neutral default) reads clearly and the outdoor env doesn't mirror
/// onto every surface. The material is a **display** value on the `MeshAsset`, downstream of the
/// content-address hash (`content_bytes` is geometry only), so it does not change asset ids or import
/// dedup/determinism. Carve/SDF results keep the matte engine default.
#[must_use]
pub fn trimesh_to_mesh_asset_colored(
    mesh: &TriMesh,
    name: &str,
    color: Option<[f32; 3]>,
) -> MeshAsset {
    #[allow(clippy::cast_possible_truncation)] // f64→f32 is the asset layer's storage precision
    let positions: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|p| [p[0] as f32, p[1] as f32, p[2] as f32])
        .collect();
    let indices: Vec<u32> = mesh
        .triangles
        .iter()
        .flat_map(|t| t.iter().copied())
        .collect();
    let material = if matches!(name, "cad" | "step") {
        let base = color.map_or([0.58, 0.59, 0.61, 1.0], |c| [c[0], c[1], c[2], 1.0]);
        Material {
            base_color: base,
            metallic: 0.30,
            roughness: 0.38,
            ..Material::default()
        }
    } else {
        Material::default()
    };
    MeshAsset {
        name: name.to_string(),
        primitives: vec![Primitive {
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
            material: 0,
            joints: Vec::new(),
            weights: Vec::new(),
        }],
        materials: vec![material],
        textures: Vec::new(),
        skeleton: None,
    }
}

/// Store a mesh under its **content address** and return the handle (the path a carve result — and a
/// synthetic source mesh — takes into the store). Same geometry ⇒ same bytes ⇒ same handle (dedup + a
/// stable reload handle).
#[must_use]
pub fn store_mesh(store: &mut AssetStore, mesh: &TriMesh, name: &str) -> String {
    let asset = trimesh_to_mesh_asset(mesh, name);
    let id = AssetId::of_bytes(&content_bytes(mesh));
    let handle = id.as_str().to_string();
    store.insert(id, asset);
    handle
}

/// Deterministic content bytes for a result mesh (a length-prefixed LE encoding of the `f32` positions +
/// `u32` indices) → the content address. Same boolean on the same inputs ⇒ same bytes ⇒ same handle, so a
/// carved mesh **re-resolves to the same handle after a reload** (ADR-013/014) and identical carves dedup.
fn content_bytes(mesh: &TriMesh) -> Vec<u8> {
    let mut b = Vec::with_capacity(mesh.positions.len() * 12 + mesh.triangles.len() * 12 + 16);
    b.extend_from_slice(&(mesh.positions.len() as u64).to_le_bytes());
    b.extend_from_slice(&(mesh.triangles.len() as u64).to_le_bytes());
    #[allow(clippy::cast_possible_truncation)]
    // the stored mesh is f32; the content hash matches it
    for p in &mesh.positions {
        for c in p {
            b.extend_from_slice(&(*c as f32).to_le_bytes());
        }
    }
    for t in &mesh.triangles {
        for i in t {
            b.extend_from_slice(&i.to_le_bytes());
        }
    }
    b
}

/// Run a robust boolean on two stored meshes, validate the result is a clean watertight solid, store it by
/// its **content address**, and return the new handle. Deterministic (content-addressed) and crack-free
/// (the always-on validator gates the output; a degenerate result is [`CarveError::Blocked`], never a
/// silent crack). The caller `place_mesh`es the returned handle in one undoable transaction.
///
/// # Errors
/// [`CarveError::UnknownInput`] if a handle isn't in the store; [`CarveError::Blocked`] if the boolean does
/// not yield a clean solid.
pub fn carve(
    store: &mut AssetStore,
    op: BoolOp,
    a_handle: &str,
    b_handle: &str,
) -> Result<String, CarveError> {
    let ta = {
        let a = store
            .get_str(a_handle)
            .ok_or_else(|| CarveError::UnknownInput(a_handle.to_string()))?;
        mesh_asset_to_trimesh(a)
    };
    let tb = {
        let b = store
            .get_str(b_handle)
            .ok_or_else(|| CarveError::UnknownInput(b_handle.to_string()))?;
        mesh_asset_to_trimesh(b)
    };

    // The asset store is f32, so a carve of a *previous carve result* sees f32-quantised coordinates. Use a
    // weld tolerance matched to f32 precision (~6e-8 relative) so the BSP's split points still fuse on a
    // re-carved mesh (the destructible-wall chain) — the f64 default (1e-9) is below f32 noise.
    let engine_csg = ExactBspCsg { weld_rel: 1e-6 };
    let out = engine_csg
        .boolean(&ta, &tb, op)
        .map_err(|e| CarveError::Blocked(e.to_string()))?;
    debug_assert!(
        validate(&out).is_clean(),
        "carve() returns only validator-clean meshes"
    );

    Ok(store_mesh(store, &out, &format!("csg-{}", op.verb())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_asset(store: &mut AssetStore, center: [f64; 3], half: [f64; 3]) -> String {
        store_mesh(store, &metrocalk_csg::box_mesh(center, half), "box")
    }

    #[test]
    fn carve_stores_a_watertight_result_by_content_address() {
        let mut store = AssetStore::new();
        let wall = box_asset(&mut store, [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
        let cut = box_asset(&mut store, [0.0, 0.5, 0.0], [0.5, 0.5, 1.0]); // coplanar-top carve

        let handle = carve(&mut store, BoolOp::Difference, &wall, &cut).expect("clean carve");
        assert!(
            store.contains(&handle),
            "the result is content-addressed in the store"
        );

        // The stored result is a clean watertight solid.
        let result = store.get_str(&handle).expect("present");
        let r = validate(&mesh_asset_to_trimesh(result));
        assert!(r.watertight && r.oriented, "{}", r.explain());
    }

    #[test]
    fn carve_is_deterministic_same_inputs_same_handle() {
        let mut s1 = AssetStore::new();
        let a1 = box_asset(&mut s1, [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
        let b1 = box_asset(&mut s1, [0.3, 0.2, 0.0], [0.4, 0.4, 1.0]);
        let h1 = carve(&mut s1, BoolOp::Difference, &a1, &b1).unwrap();

        // A fresh store (a "reload"): the same inputs reproduce the SAME content-addressed handle.
        let mut s2 = AssetStore::new();
        let a2 = box_asset(&mut s2, [0.0, 0.0, 0.0], [2.0, 1.0, 0.5]);
        let b2 = box_asset(&mut s2, [0.3, 0.2, 0.0], [0.4, 0.4, 1.0]);
        let h2 = carve(&mut s2, BoolOp::Difference, &a2, &b2).unwrap();
        assert_eq!(
            h1, h2,
            "a carve re-resolves to the same handle after a reload (content-addressed)"
        );
    }

    #[test]
    fn an_unknown_input_is_explained() {
        let mut store = AssetStore::new();
        let a = box_asset(&mut store, [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let err = carve(&mut store, BoolOp::Difference, &a, "mtkasset:deadbeef").unwrap_err();
        assert!(matches!(err, CarveError::UnknownInput(_)), "{err}");
    }

    #[test]
    fn parse_op_accepts_the_verbs() {
        assert_eq!(parse_op("carve"), Some(BoolOp::Difference));
        assert_eq!(parse_op("union"), Some(BoolOp::Union));
        assert_eq!(parse_op("intersect"), Some(BoolOp::Intersection));
        assert_eq!(parse_op("xor"), Some(BoolOp::Xor));
        assert_eq!(parse_op("frobnicate"), None);
    }
}
