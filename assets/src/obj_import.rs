//! OBJ import (M11.1) — Wavefront `.obj` → the project's internal [`MeshAsset`], behind the same
//! [`MeshSource`] trait as glTF. OBJ is the other format half the asset world ships in (DCC exports,
//! Sketchfab, procedural tools); `tobj` is a clean **pure-Rust, wasm32-clean** decoder, so this front-end
//! stays in the crate's default (browser-reachable) feature set — no FFI, no rayon, no foreign type past
//! the boundary (`tobj::` lives only here, like `gltf::` in [`crate::gltf_import`]).
//!
//! **Self-contained bytes only.** An `.obj` references its materials by an external `mtllib` filename; a
//! drag-a-single-file import has no filesystem to resolve it against, so geometry imports with a neutral
//! default material and the `.mtl` sidecar is a **named seam** (the shell's File→Import can pass a
//! companion `.mtl`'s bytes when it has them — a follow-up, not a fork). Normals/UVs are carried when
//! present; absent normals are derived by the GPU packer (same as glTF).
//!
//! The untrusted-asset safety gate (ADR-031) applies unchanged: a size cap before parsing, an element
//! cap before the mesh is accepted, every index range-checked, and a malformed/truncated `.obj` → an
//! explained [`ImportError`], never a panic.

use std::io::Cursor;

use crate::mesh::{Material, MeshAsset, Primitive};
use crate::source::{ImportError, MeshSource, MAX_ELEMENTS, MAX_IMPORT_BYTES};

/// The Wavefront `.obj` importer (M11.1). Pure-Rust (`tobj`), wasm32-clean.
#[derive(Clone, Copy, Debug, Default)]
pub struct ObjImporter;

impl ObjImporter {
    /// A new importer.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// A neutral default material for OBJ geometry (the `.mtl` sidecar is a named seam) — a mid-grey so an
/// imported mesh is visible without an authored material.
fn default_obj_material() -> Material {
    Material {
        base_color: [0.8, 0.8, 0.8, 1.0],
        base_color_texture: None,
    }
}

/// Reject before allocating a ruinously large mesh (the decode-bomb guard, mirrored from the glTF path).
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

impl MeshSource for ObjImporter {
    fn format(&self) -> &'static str {
        "obj"
    }

    fn import(&self, bytes: &[u8]) -> Result<MeshAsset, ImportError> {
        // Size cap BEFORE parsing — a hostile multi-GB file is refused, not buffered.
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(ImportError::TooLarge {
                bytes: bytes.len(),
                limit: MAX_IMPORT_BYTES,
            });
        }

        // single_index → positions/normals/texcoords share ONE index buffer (our `Primitive` layout);
        // triangulate → n-gon faces become triangles, so `indices` is always a clean triangle list.
        let opts = tobj::LoadOptions {
            single_index: true,
            triangulate: true,
            ..Default::default()
        };
        let mut reader = Cursor::new(bytes);
        // No external `.mtl` resolution from raw bytes — the material loader declines; geometry still
        // loads (tobj returns the material result separately). The `.mtl` sidecar is the named seam.
        let (models, _materials) = tobj::load_obj_buf(&mut reader, &opts, |_p| {
            Err(tobj::LoadError::GenericFailure)
        })
        .map_err(|e| ImportError::Malformed(format!("OBJ parse failed: {e}")))?;

        let mut primitives = Vec::new();
        for model in &models {
            let m = &model.mesh;
            if m.positions.is_empty() || m.indices.is_empty() {
                continue; // a group with no geometry (e.g. lines/points stripped) — skip, not an error
            }
            // Decode-bomb guards before we accept the buffers.
            guard_count(m.positions.len() / 3)?;
            guard_count(m.indices.len())?;
            // Triangulated → indices must be a whole number of triangles. A malformed buffer is rejected
            // fail-fast (never silently drop a partial triangle — the glTF path's adversarial-review rule).
            if m.indices.len() % 3 != 0 {
                return Err(ImportError::Malformed(
                    "OBJ mesh indices are not a triangle list".into(),
                ));
            }
            if m.positions.len() % 3 != 0 {
                return Err(ImportError::Malformed(
                    "OBJ vertex positions are not 3-component".into(),
                ));
            }
            let positions: Vec<[f32; 3]> = m
                .positions
                .chunks_exact(3)
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            let normals: Vec<[f32; 3]> = m
                .normals
                .chunks_exact(3)
                .map(|c| [c[0], c[1], c[2]])
                .collect();
            let uvs: Vec<[f32; 2]> = m.texcoords.chunks_exact(2).map(|c| [c[0], c[1]]).collect();
            // Every index must be in range — a lying index is an explained rejection, never an over-read.
            let nverts = positions.len();
            if m.indices.iter().any(|&i| (i as usize) >= nverts) {
                return Err(ImportError::Malformed(
                    "OBJ face references an out-of-range vertex".into(),
                ));
            }
            primitives.push(Primitive {
                positions,
                normals,
                uvs,
                indices: m.indices.clone(),
                material: 0,
                joints: Vec::new(),
                weights: Vec::new(),
            });
        }

        if primitives.is_empty() {
            return Err(ImportError::NoGeometry);
        }

        let name = models
            .first()
            .map(|m| m.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "obj".into());

        Ok(MeshAsset {
            name,
            primitives,
            materials: vec![default_obj_material()],
            textures: Vec::new(),
            skeleton: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube as 6 quad faces (tobj triangulates → 12 tris; single_index dedups to 8 positions).
    fn cube_obj() -> &'static [u8] {
        b"# unit cube\n\
o cube\n\
v -0.5 -0.5 -0.5\n\
v  0.5 -0.5 -0.5\n\
v  0.5  0.5 -0.5\n\
v -0.5  0.5 -0.5\n\
v -0.5 -0.5  0.5\n\
v  0.5 -0.5  0.5\n\
v  0.5  0.5  0.5\n\
v -0.5  0.5  0.5\n\
f 1 2 3 4\n\
f 5 6 7 8\n\
f 1 2 6 5\n\
f 2 3 7 6\n\
f 3 4 8 7\n\
f 4 1 5 8\n"
    }

    #[test]
    fn imports_a_cube_obj_to_a_mesh_asset() {
        let asset = ObjImporter::new()
            .import(cube_obj())
            .expect("import cube.obj");
        assert_eq!(asset.primitives.len(), 1, "one object → one primitive");
        assert_eq!(
            asset.vertex_count(),
            8,
            "8 unique positions (single_index dedup)"
        );
        assert_eq!(asset.triangle_count(), 12, "6 quads triangulated → 12 tris");
        assert_eq!(asset.index_count(), 36);
        // Indices stay in range.
        let p = &asset.primitives[0];
        assert!(p.indices.iter().all(|&i| (i as usize) < p.positions.len()));
        // A unit cube: ~1.0 extent on every axis, centred at the origin.
        let b = asset.bounds();
        for axis in 0..3 {
            assert!(
                (b.max[axis] - b.min[axis] - 1.0).abs() < 1e-5,
                "axis {axis} spans 1.0"
            );
            assert!(
                (b.max[axis] + b.min[axis]).abs() < 1e-5,
                "centred on origin"
            );
        }
        assert_eq!(asset.materials.len(), 1, "a neutral default material");
        assert!(asset.skeleton.is_none(), "OBJ carries no rig");
    }

    #[test]
    fn import_is_deterministic() {
        // Same bytes → byte-identical geometry, twice (the content-address / reload contract).
        let a = ObjImporter::new().import(cube_obj()).expect("a");
        let b = ObjImporter::new().import(cube_obj()).expect("b");
        assert_eq!(a, b, "OBJ import is deterministic");
    }

    #[test]
    fn rejects_oversize_input() {
        let big = vec![b'v'; MAX_IMPORT_BYTES + 1];
        let err = ObjImporter::new().import(&big).unwrap_err();
        assert!(matches!(err, ImportError::TooLarge { .. }));
    }

    #[test]
    fn malformed_obj_is_an_explained_error_not_a_panic() {
        // Garbage that isn't an OBJ → either a parse error or no geometry, but NEVER a panic.
        let err = ObjImporter::new()
            .import(b"\x00\x01\x02 not an obj at all \xff")
            .unwrap_err();
        assert!(matches!(
            err,
            ImportError::Malformed(_) | ImportError::NoGeometry
        ));
    }

    #[test]
    fn an_obj_with_no_faces_has_no_geometry() {
        // Vertices but no faces → no drawable geometry, an explained rejection.
        let err = ObjImporter::new()
            .import(b"v 0 0 0\nv 1 0 0\nv 0 1 0\n")
            .unwrap_err();
        assert!(matches!(
            err,
            ImportError::NoGeometry | ImportError::Malformed(_)
        ));
    }
}
