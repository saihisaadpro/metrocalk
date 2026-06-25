//! FBX import (M11.1, ADR-040) — Autodesk `.fbx` → the project's internal [`MeshAsset`], behind the same
//! [`MeshSource`] trait as glTF/OBJ. The crate decision is **MEASURED, not assumed** (`tests/fbx_bakeoff.rs`):
//! `ufbx` over `fbxcel-dom` for ASCII+binary coverage (ufbx parsed an ASCII cube — 8 verts, 12 tris,
//! deterministic, ~141 µs on the dev box — where `fbxcel-dom` rejected it as binary-only).
//!
//! **Native-only:** `ufbx` is a C library via FFI (`cc`), so this whole module is behind the **`fbx`
//! feature** — the default crate stays `wasm32`-clean (the wasm-tripwire builds the default), and the
//! browser funnel converts FBX **server-side** (the explicit wasm boundary, ADR-040). `ufbx::` stays behind
//! THIS module (CI grep-gated, exactly like `gltf::`/`image::`/`tobj::`).
//!
//! The untrusted-asset safety gate (ADR-031) holds: a size cap before parsing, an element cap before the
//! mesh is accepted, and `ufbx`'s own robust error path → a malformed/partial FBX is an explained
//! [`ImportError`], never a panic.

use crate::mesh::{Material, MeshAsset, Primitive};
use crate::source::{ImportError, MeshSource, MAX_ELEMENTS, MAX_IMPORT_BYTES};

/// The Autodesk FBX importer (M11.1) — `ufbx` C-FFI, native-only (behind the `fbx` feature).
#[derive(Clone, Copy, Debug, Default)]
pub struct FbxImporter;

impl FbxImporter {
    /// A new importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// A neutral default material (FBX material/texture extraction is a follow-up — a mid-grey so an imported
/// mesh is visible).
fn default_fbx_material() -> Material {
    Material {
        base_color: [0.8, 0.8, 0.8, 1.0],
        base_color_texture: None,
    }
}

/// Reject before allocating a ruinously large mesh (the decode-bomb guard, mirrored from glTF/OBJ).
fn guard_count(count: usize) -> Result<(), ImportError> {
    if count > MAX_ELEMENTS {
        Err(ImportError::TooManyElements {
            count,
            limit: MAX_ELEMENTS,
        })
    } else {
        Ok(())
    }
}

impl MeshSource for FbxImporter {
    fn format(&self) -> &'static str {
        "fbx"
    }

    #[allow(clippy::cast_possible_truncation)] // f64 (ufbx Real) → f32 positions; corner index → u32: bounded
    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError> {
        // Size cap BEFORE parsing — a hostile multi-GB file is refused, not buffered.
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }

        // ufbx is robust (ASCII + binary); a malformed/partial FBX returns Err → an explained error, never a
        // panic. No external-URI fetch surface (we parse in-memory bytes only).
        let scene = ufbx::load_memory(bytes, ufbx::LoadOpts::default())
            .map_err(|e| ImportError::Malformed(format!("FBX parse failed: {e:?}")))?;

        let mut primitives = Vec::new();
        let mut corner_idx: Vec<u32> = Vec::new();
        let mut rigged = false;
        for mesh in &scene.meshes {
            guard_count(mesh.num_vertices)?;
            if !mesh.skin_deformers.is_empty() {
                rigged = true;
            }
            let has_n = mesh.vertex_normal.exists;
            let has_uv = mesh.vertex_uv.exists;
            let mut positions: Vec<[f32; 3]> = Vec::new();
            let mut normals: Vec<[f32; 3]> = Vec::new();
            let mut uvs: Vec<[f32; 2]> = Vec::new();
            let mut indices: Vec<u32> = Vec::new();
            // Triangulate each (possibly n-gon) face into corner indices, then emit per-corner vertices
            // (positions/normals/uvs), so the output is always a clean triangle list.
            for face in &mesh.faces {
                let ntri = ufbx::triangulate_face_vec(&mut corner_idx, mesh, *face);
                if ntri == 0 {
                    continue; // a degenerate/point/line face — skip, not an error
                }
                for &c in &corner_idx {
                    let ci = c as usize;
                    let p = mesh.vertex_position[ci];
                    indices.push(positions.len() as u32);
                    positions.push([p.x as f32, p.y as f32, p.z as f32]);
                    if has_n {
                        let n = mesh.vertex_normal[ci];
                        normals.push([n.x as f32, n.y as f32, n.z as f32]);
                    }
                    if has_uv {
                        let u = mesh.vertex_uv[ci];
                        uvs.push([u.x as f32, u.y as f32]);
                    }
                }
                guard_count(positions.len())?;
            }
            if positions.is_empty() {
                continue;
            }
            guard_count(indices.len())?;
            primitives.push(Primitive {
                positions,
                normals,
                uvs,
                indices,
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            });
        }

        if primitives.is_empty() {
            return Err(ImportError::NoGeometry);
        }

        // RIG DETECTION (M11.1): an FBX with skin deformers is RIGGED. Extracting the full `Skeleton`
        // (ufbx skin clusters → joints + inverse-bind matrices + per-vertex weights → topo-sort → the M9.3
        // `Skeleton` type, generalizing the glTF skin→Skeleton path) is a NAMED next increment — the headline
        // here is the §1.5 "drop a .fbx → a working mesh". A rigged mesh imports as geometry; `skeleton`
        // stays `None` until that extraction lands (so a Mixamo FBX renders now, and poses after).
        let _ = rigged;
        Ok(MeshAsset {
            name: "fbx".to_string(),
            primitives,
            materials: vec![default_fbx_material()],
            textures: Vec::new(),
            skeleton: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same minimal ASCII FBX cube the bake-off uses (8 verts, 6 quads). ufbx parses ASCII.
    fn ascii_cube() -> &'static [u8] {
        b"; FBX 7.4.0 project file\n\
FBXHeaderExtension:  {\n\
\tFBXHeaderVersion: 1003\n\
\tFBXVersion: 7400\n\
}\n\
Objects:  {\n\
\tGeometry: 100, \"Geometry::Cube\", \"Mesh\" {\n\
\t\tVertices: *24 {\n\
\t\t\ta: -0.5,-0.5,-0.5,0.5,-0.5,-0.5,0.5,0.5,-0.5,-0.5,0.5,-0.5,-0.5,-0.5,0.5,0.5,-0.5,0.5,0.5,0.5,0.5,-0.5,0.5,0.5\n\
\t\t}\n\
\t\tPolygonVertexIndex: *24 {\n\
\t\t\ta: 0,1,2,-4,4,5,6,-8,0,1,5,-5,1,2,6,-6,2,3,7,-7,3,0,4,-8\n\
\t\t}\n\
\t}\n\
}\n"
    }

    #[test]
    fn imports_an_fbx_cube_to_a_mesh_asset() {
        let asset = FbxImporter::new()
            .import(ascii_cube())
            .expect("import cube.fbx");
        assert_eq!(asset.primitives.len(), 1, "one mesh → one primitive");
        assert_eq!(asset.triangle_count(), 12, "6 quads triangulated → 12 tris");
        assert!(asset.index_count() >= 36);
        let p = &asset.primitives[0];
        assert!(
            p.indices.iter().all(|&i| (i as usize) < p.positions.len()),
            "indices in range"
        );
        // A unit cube: ~1.0 extent, centred at the origin.
        let b = asset.bounds();
        for axis in 0..3 {
            assert!(
                (b.max[axis] - b.min[axis] - 1.0).abs() < 1e-4,
                "axis {axis} spans 1.0"
            );
        }
        assert_eq!(asset.materials.len(), 1);
    }

    #[test]
    fn fbx_import_is_deterministic() {
        let a = FbxImporter::new().import(ascii_cube()).expect("a");
        let b = FbxImporter::new().import(ascii_cube()).expect("b");
        assert_eq!(
            a, b,
            "FBX import is deterministic (no rayon/RNG in the ufbx path)"
        );
    }

    #[test]
    fn malformed_fbx_is_an_explained_error_not_a_panic() {
        let err = FbxImporter::new()
            .import(b"\x00\x01 not an fbx \xff\xfe garbage")
            .unwrap_err();
        assert!(
            matches!(err, ImportError::Malformed(_) | ImportError::NoGeometry),
            "got {err:?}"
        );
    }

    #[test]
    fn rejects_oversize_input() {
        let big = vec![b';'; MAX_IMPORT_BYTES + 1];
        assert!(matches!(
            FbxImporter::new().import(&big).unwrap_err(),
            ImportError::TooLarge { .. }
        ));
    }
}
