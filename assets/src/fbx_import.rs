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
//!
//! ## What an FBX actually needs, and what an earlier version of this importer got wrong
//!
//! An audit against the real fixtures (`tests/fixtures/fbx/`) measured three defects that the previous
//! shape-only tests could not see, because a mesh can be non-empty, in-range and deterministic while still
//! being completely wrong on screen:
//!
//! 1. **Node transforms were dropped.** Only `mesh.vertex_position` (mesh-local) was read, so
//!    `cubes_with_mirroring_and_pivot.fbx` imported all four of its cubes stacked at the origin. Everything
//!    FBX expresses through the node — parent inheritance, pre/post-rotation, rotation order, the separate
//!    *geometric* transform, pivots, mirroring and negative scale — was silently discarded. Geometry is now
//!    emitted **per instance** through `node.geometry_to_world`, which is the one place ufbx has already
//!    composed all of that correctly.
//! 2. **No coordinate-system or unit conversion.** A 3ds Max file arrived Z-up, and `box.fbx` arrived 200
//!    units wide. Conversion is now asked of ufbx up front, so a file that *declares* its axes and units —
//!    which every DCC exporter does — lands in the editor's convention (right-handed, Y-up, metres). A file
//!    that declares nothing is left alone rather than guessed at, which is the honest behaviour: inventing a
//!    scale factor for an undeclared file would be wrong more often than right.
//! 3. **Materials were a hard-coded grey.** The Phong and the PBR fixture imported identically. ufbx
//!    normalises Phong *and* PBR into one `pbr` map set, so both are now read, and faces are split by
//!    material so a multi-material mesh no longer collapses onto one.
//!
//! **Mirroring is a winding problem, not only a position problem.** A negative-determinant transform turns
//! a counter-clockwise triangle clockwise; left alone the mesh renders inside-out under backface culling.
//! Winding is reversed per instance when the determinant is negative.
//!
//! **Still not extracted, and deliberately named rather than hidden:** the skeleton (a rigged FBX imports
//! as geometry — see [`MeshAsset::skeleton`]), animation curves, morph targets, vertex colours, and
//! embedded/external textures. Tangents are left to the conditioning tier, which generates them with
//! MikkTSpace rather than trusting an exporter's.

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

/// A neutral default material, used only when geometry references no FBX material at all — a mid-grey so
/// an imported mesh is visible rather than invisible.
fn default_fbx_material() -> Material {
    Material {
        base_color: [0.8, 0.8, 0.8, 1.0],
        ..Default::default()
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

/// Load options that put every FBX into the editor's own convention.
///
/// FBX files carry their authoring tool's axes and units in the header — 3ds Max writes Z-up, Maya writes
/// Y-up, and centimetres are overwhelmingly the common unit. Asking ufbx to convert means the importer
/// never has to guess, and two files from different tools land on top of each other instead of one arriving
/// rotated and a hundred times too large.
///
/// `AdjustTransforms` puts the conversion into the node transforms rather than into a synthetic root node,
/// which is what makes it correct to read `geometry_to_world` per instance below. `generate_missing_normals`
/// covers exporters that omit them (`boxWithCompressedCTypeArray.FBX` is one) so downstream shading does not
/// have to special-case a normal-less mesh.
fn load_options<'a>() -> ufbx::LoadOpts<'a> {
    ufbx::LoadOpts {
        target_axes: ufbx::CoordinateAxes::right_handed_y_up(),
        target_unit_meters: 1.0,
        space_conversion: ufbx::SpaceConversion::AdjustTransforms,
        generate_missing_normals: true,
        ..ufbx::LoadOpts::default()
    }
}

/// Map one ufbx material onto the project's PBR material.
///
/// ufbx resolves Phong (`lambert`/`phong`) and modern PBR shaders into the same `pbr` map set, so a 1998
/// Phong cube and a 3ds Max metal-rough material both arrive here already normalised. Only factors are
/// read: textures are referenced by paths this importer never fetches (there is no external-URI surface
/// here by design, ADR-031), so a textured FBX imports with its factors and no texture rather than with a
/// broken path.
// ufbx works in `Real` (f64); the project's material factors are f32. Every value here is a colour or
// a 0..1 factor, so the precision lost is far below what a shader or a colour picker can represent.
#[allow(clippy::cast_possible_truncation)]
fn convert_material(material: &ufbx::Material) -> Material {
    let pbr = &material.pbr;
    let mut out = default_fbx_material();
    if pbr.base_color.has_value {
        let c = pbr.base_color.value_vec4;
        out.base_color = [c.x as f32, c.y as f32, c.z as f32, c.w as f32];
        // A material with an authored colour but no authored alpha would otherwise read as fully
        // transparent: ufbx leaves `w` at 0 when the source carried only RGB.
        if pbr.base_color.value_components < 4 {
            out.base_color[3] = 1.0;
        }
    }
    if pbr.metalness.has_value {
        out.metallic = pbr.metalness.value_vec4.x as f32;
    }
    if pbr.roughness.has_value {
        out.roughness = pbr.roughness.value_vec4.x as f32;
    }
    // Clamp rather than trust: an exporter writing a percentage (0..100) into a 0..1 slot would otherwise
    // reach the shader and blow out the lighting.
    out.base_color = out.base_color.map(|c| c.clamp(0.0, 1.0));
    out.metallic = out.metallic.clamp(0.0, 1.0);
    out.roughness = out.roughness.clamp(0.0, 1.0);
    out
}

/// One instance's geometry for one material, accumulated before it becomes a [`Primitive`].
#[derive(Default)]
struct Bucket {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

impl MeshSource for FbxImporter {
    fn format(&self) -> &'static str {
        "fbx"
    }

    #[allow(clippy::cast_possible_truncation)] // f64 (ufbx Real) → f32 positions; corner index → u32: bounded
    #[allow(clippy::too_many_lines)] // one linear walk of the scene; splitting it hides the order
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
        let scene = ufbx::load_memory(bytes, load_options())
            .map_err(|e| ImportError::Malformed(format!("FBX parse failed: {e:?}")))?;

        // Materials first, so a face can name one by index. `typed_id` is ufbx's own index into
        // `scene.materials`, which is what the per-mesh material entries carry.
        guard_count(scene.materials.len())?;
        let mut materials: Vec<Material> = scene
            .materials
            .iter()
            .map(|m| convert_material(m))
            .collect();
        // The last slot is the fallback for geometry that references no material at all.
        let fallback = materials.len();
        materials.push(default_fbx_material());

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

            // A mesh instanced twice must be emitted twice, at both of its placements — that is what
            // reading `instances` buys. But geometry with NO node at all still has to import: hand-written
            // and tool-exported-with-orphans files exist, and refusing a file that visibly contains a cube
            // with "no geometry" is the worst possible answer. Un-instanced geometry is imported in its own
            // mesh-local space, which is the only space it has.
            let placements: Vec<Option<&ufbx::Node>> = if mesh.element.instances.is_empty() {
                vec![None]
            } else {
                mesh.element.instances.iter().map(|n| Some(&**n)).collect()
            };
            for node in placements {
                let to_world = node.map_or_else(ufbx::Matrix::identity, |n| n.geometry_to_world);
                let for_normals = node.map_or_else(ufbx::Matrix::identity, |n| {
                    ufbx::get_compatible_matrix_for_normals(n)
                });
                // A mirrored (negative-determinant) instance reverses triangle winding. Without this the
                // mesh is inside-out under backface culling — visible as a hollow shell, not as an error.
                let mirrored = ufbx::matrix_determinant(&to_world) < 0.0;

                let mut buckets: std::collections::BTreeMap<usize, Bucket> =
                    std::collections::BTreeMap::new();

                for (face_index, face) in mesh.faces.iter().enumerate() {
                    let ntri = ufbx::triangulate_face_vec(&mut corner_idx, mesh, *face);
                    if ntri == 0 {
                        continue; // a degenerate/point/line face — skip, not an error
                    }
                    // Which material this face belongs to. A single-material mesh often carries an empty
                    // `face_material`, so a missing entry means "the mesh's only material".
                    let slot = mesh
                        .face_material
                        .get(face_index)
                        .and_then(|local| mesh.materials.get(*local as usize))
                        .map_or(fallback, |material| material.element.typed_id as usize);
                    let slot = if slot < materials.len() {
                        slot
                    } else {
                        fallback
                    };
                    let bucket = buckets.entry(slot).or_default();

                    // Emit per-corner vertices so the output is always a clean triangle list. Corners are
                    // walked in reverse for a mirrored instance, which flips every triangle's winding.
                    for tri in 0..usize::try_from(ntri).unwrap_or(0) {
                        let base = tri * 3;
                        for corner in 0_usize..3 {
                            let corner = if mirrored { 2 - corner } else { corner };
                            let Some(&c) = corner_idx.get(base + corner) else {
                                continue;
                            };
                            let ci = c as usize;
                            let p = ufbx::transform_position(&to_world, mesh.vertex_position[ci]);
                            bucket.indices.push(bucket.positions.len() as u32);
                            bucket.positions.push([p.x as f32, p.y as f32, p.z as f32]);
                            if has_n {
                                let n =
                                    ufbx::transform_direction(&for_normals, mesh.vertex_normal[ci]);
                                // Re-normalise: a non-uniform scale changes a normal's length even through
                                // the correct inverse-transpose matrix.
                                let len = n.x.mul_add(n.x, n.y.mul_add(n.y, n.z * n.z)).sqrt();
                                let inv = if len > 1e-12 { 1.0 / len } else { 0.0 };
                                bucket.normals.push([
                                    (n.x * inv) as f32,
                                    (n.y * inv) as f32,
                                    (n.z * inv) as f32,
                                ]);
                            }
                            if has_uv {
                                let u = mesh.vertex_uv[ci];
                                bucket.uvs.push([u.x as f32, u.y as f32]);
                            }
                        }
                    }
                    guard_count(bucket.positions.len())?;
                }

                for (slot, bucket) in buckets {
                    if bucket.positions.is_empty() {
                        continue;
                    }
                    guard_count(bucket.indices.len())?;
                    // The struct update is deliberate: `Primitive` gains fields over time (tangents,
                    // and more to come from the conditioning tier), and an importer that has nothing to
                    // say about a new field should not have to be edited every time one appears. It also
                    // keeps this module compiling against either side of an in-flight field addition.
                    #[allow(clippy::needless_update)]
                    primitives.push(Primitive {
                        positions: bucket.positions,
                        normals: bucket.normals,
                        uvs: bucket.uvs,
                        indices: bucket.indices,
                        material: slot,
                        joints: Vec::new(),
                        weights: Vec::new(),
                        ..Default::default()
                    });
                }
            }
        }

        if primitives.is_empty() {
            return Err(ImportError::NoGeometry);
        }
        // Drop the fallback slot unless geometry actually landed in it. It is the LAST slot, so removing
        // it cannot renumber any real material.
        if !primitives.iter().any(|p| p.material == fallback) {
            materials.pop();
        }

        // RIG DETECTION (M11.1): an FBX with skin deformers is RIGGED. Extracting the full `Skeleton`
        // (ufbx skin clusters → joints + inverse-bind matrices + per-vertex weights → topo-sort → the M9.3
        // `Skeleton` type, generalizing the glTF skin→Skeleton path) is a NAMED next increment — the headline
        // here is the §1.5 "drop a .fbx → a working mesh". A rigged mesh imports as geometry; `skeleton`
        // stays `None` until that extraction lands (so a Mixamo FBX renders now, and poses after).
        let _ = rigged;
        // The file's own name, so the inspector shows what was imported rather than the literal "fbx". The
        // root node is unnamed in every FBX, so the first named node is the useful label.
        let name = scene
            .nodes
            .iter()
            .find(|node| !node.is_root && node.element.name.length > 0)
            .map_or_else(
                || "fbx".to_string(),
                |node| AsRef::<str>::as_ref(&node.element.name).to_owned(),
            );
        Ok(MeshAsset {
            name,
            primitives,
            materials,
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
        assert!(!asset.materials.is_empty());
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

    #[test]
    fn geometry_with_no_node_instancing_it_still_imports() {
        // Emitting geometry per node instance is what applies transforms — but it also means geometry
        // that no node references would vanish. This ASCII cube is exactly that shape: a `Geometry` with
        // no `Model` and no `Connections`. Refusing a file that visibly contains a cube with "no geometry"
        // is the worst possible answer, so un-instanced geometry imports in its own mesh-local space.
        let asset = FbxImporter::new()
            .import(ascii_cube())
            .expect("un-instanced geometry must still import");
        assert_eq!(asset.triangle_count(), 12);
        let b = asset.bounds();
        // Mesh-local space: the cube is where the file put it, untransformed.
        for axis in 0..3 {
            assert!((b.min[axis] + 0.5).abs() < 1e-4 && (b.max[axis] - 0.5).abs() < 1e-4);
        }
    }

    #[test]
    fn an_imported_material_is_never_fully_transparent() {
        // ufbx leaves the alpha component at 0 when the source carried only RGB. Taking `value_vec4`
        // verbatim would make every such material fully transparent — a mesh that imports "successfully"
        // and then cannot be seen, which is the worst kind of import bug to diagnose.
        let asset = FbxImporter::new().import(ascii_cube()).expect("cube");
        for material in &asset.materials {
            assert!(
                material.base_color[3] > 0.0,
                "an imported material must not be fully transparent: {:?}",
                material.base_color
            );
        }
    }
}
