#![cfg(feature = "fbx")]
//! Import REAL `.fbx` files, not a synthetic string.
//!
//! The FBX importer's own unit test builds an 8-vertex ASCII cube in a Rust string literal. That proves the
//! parser can read a cube it was handed; it says nothing about binary FBX, deflate-compressed arrays, real
//! material graphs, rigs, or the pivot/mirroring conventions DCC tools actually emit — which is where FBX
//! importers actually break. These fixtures are real files from assimp's licence-clean test suite; see
//! `fixtures/fbx/PROVENANCE.md`.
//!
//! Every assertion here is a property that must hold for ANY importable mesh, so the suite does not have to
//! be rewritten when a fixture is added: geometry is non-empty, indices are in range, triangle counts are
//! consistent, bounds are finite and non-degenerate, and repeated imports are byte-identical.

use metrocalk_assets::{FbxImporter, MeshSource};

/// Every fixture, with whether it is binary or ASCII — the split is the reason the set exists.
const FIXTURES: &[(&str, &[u8], bool)] = &[
    ("box.fbx", include_bytes!("fixtures/fbx/box.fbx"), true),
    (
        "phong_cube.fbx",
        include_bytes!("fixtures/fbx/phong_cube.fbx"),
        true,
    ),
    (
        "spider.fbx",
        include_bytes!("fixtures/fbx/spider.fbx"),
        true,
    ),
    (
        "boxWithCompressedCTypeArray.FBX",
        include_bytes!("fixtures/fbx/boxWithCompressedCTypeArray.FBX"),
        true,
    ),
    (
        "cubes_with_mirroring_and_pivot.fbx",
        include_bytes!("fixtures/fbx/cubes_with_mirroring_and_pivot.fbx"),
        false,
    ),
    (
        "maxPbrMaterial_metalRough.fbx",
        include_bytes!("fixtures/fbx/maxPbrMaterial_metalRough.fbx"),
        false,
    ),
    (
        "animation_with_skeleton.fbx",
        include_bytes!("fixtures/fbx/animation_with_skeleton.fbx"),
        true,
    ),
];

const BINARY_MAGIC: &[u8] = b"Kaydara FBX Binary";

#[test]
fn every_real_fbx_fixture_imports_to_usable_geometry() {
    let importer = FbxImporter::new();
    let mut exercised = 0;
    for (name, bytes, binary) in FIXTURES {
        // The fixture must be what it claims: a corrupted or truncated download would otherwise show up as
        // a confusing importer failure rather than a broken fixture.
        assert_eq!(
            bytes.starts_with(BINARY_MAGIC),
            *binary,
            "{name}: fixture encoding does not match its recorded kind"
        );

        let asset = importer
            .import(bytes)
            .unwrap_or_else(|error| panic!("{name}: real-world FBX failed to import: {error:?}"));

        assert!(
            !asset.primitives.is_empty(),
            "{name}: imported with no primitives"
        );
        assert!(
            asset.triangle_count() > 0,
            "{name}: imported with no triangles"
        );

        for (index, primitive) in asset.primitives.iter().enumerate() {
            assert!(
                !primitive.positions.is_empty(),
                "{name}[{index}]: primitive has no positions"
            );
            assert!(
                primitive.indices.len() % 3 == 0,
                "{name}[{index}]: index count {} is not a whole number of triangles",
                primitive.indices.len()
            );
            // An out-of-range index is the failure that turns into a GPU crash or silent garbage later,
            // so it is checked on every primitive of every fixture rather than sampled.
            let vertices = primitive.positions.len();
            assert!(
                primitive
                    .indices
                    .iter()
                    .all(|index| (*index as usize) < vertices),
                "{name}[{index}]: an index points outside the {vertices}-vertex buffer"
            );
        }

        let bounds = asset.bounds();
        for axis in 0..3 {
            assert!(
                bounds.min[axis].is_finite() && bounds.max[axis].is_finite(),
                "{name}: non-finite bounds on axis {axis} - a NaN vertex would poison camera framing and LOD"
            );
            assert!(
                bounds.max[axis] >= bounds.min[axis],
                "{name}: inverted bounds on axis {axis}"
            );
        }
        assert!(
            (0..3).any(|axis| bounds.max[axis] > bounds.min[axis]),
            "{name}: fully degenerate bounds - the mesh has no extent on any axis"
        );
        exercised += 1;
    }
    assert_eq!(exercised, FIXTURES.len(), "every fixture must be exercised");
}

#[test]
fn importing_the_same_real_file_twice_is_byte_identical() {
    // Determinism is the property the whole engine is built on (ADR-020/091). An importer that returned
    // even slightly different geometry between runs would break content hashing and the content-addressed
    // asset store, and would do it silently.
    let importer = FbxImporter::new();
    for (name, bytes, _) in FIXTURES {
        let first = importer.import(bytes).expect("first import");
        let second = importer.import(bytes).expect("second import");
        assert_eq!(
            first.primitives.len(),
            second.primitives.len(),
            "{name}: primitive count differed between imports"
        );
        for (index, (a, b)) in first
            .primitives
            .iter()
            .zip(second.primitives.iter())
            .enumerate()
        {
            assert_eq!(
                a.positions, b.positions,
                "{name}[{index}]: positions differed between two imports of the same bytes"
            );
            assert_eq!(
                a.indices, b.indices,
                "{name}[{index}]: indices differed between two imports of the same bytes"
            );
        }
    }
}

#[test]
fn a_rigged_fbx_imports_as_geometry_rather_than_being_refused() {
    // ADR-040 states a rigged mesh imports AS GEOMETRY (the skeleton is a later tier). That is a real
    // product promise - a user dropping a rigged character must see it, not an error - so it is pinned to
    // a genuinely rigged file rather than left as prose.
    let asset = FbxImporter::new()
        .import(include_bytes!("fixtures/fbx/animation_with_skeleton.fbx"))
        .expect("a rigged FBX must import as geometry, not be refused");
    assert!(asset.triangle_count() > 0);
}

#[test]
fn a_truncated_real_fbx_is_explained_rather_than_panicking() {
    // Untrusted-asset safety (ADR-031): a malformed file must produce an explained error, never a panic.
    // Truncating a REAL file is a far better probe than a random byte string, because it stays valid right
    // up to the point it stops.
    let importer = FbxImporter::new();
    let full: &[u8] = include_bytes!("fixtures/fbx/box.fbx");
    for fraction in [2, 3, 4, 8] {
        let truncated = &full[..full.len() / fraction];
        // Either outcome is acceptable; a panic is not, and that is what this test actually guards.
        let _ = importer.import(truncated);
    }
}
