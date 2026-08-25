#![cfg(feature = "fbx")]
//! Regressions for the FBX import audit.
//!
//! `real_fbx_import.rs` next door asserts *shape* properties — non-empty, in-range indices, finite bounds,
//! byte-identical repeats. Every one of those passed while the importer was stacking every cube at the
//! origin, discarding every material, and delivering 3ds Max content lying on its side. A test that cannot
//! fail on a visibly wrong mesh is not protecting anyone, so these pin the things that were actually wrong.
//!
//! Each test names the defect it guards, because the value of the file is that it fails if any of them
//! comes back.

use metrocalk_assets::{FbxImporter, MeshSource, Primitive};

const FIXTURES: &[(&str, &[u8])] = &[
    ("box.fbx", include_bytes!("fixtures/fbx/box.fbx")),
    (
        "phong_cube.fbx",
        include_bytes!("fixtures/fbx/phong_cube.fbx"),
    ),
    ("spider.fbx", include_bytes!("fixtures/fbx/spider.fbx")),
    (
        "boxWithCompressedCTypeArray.FBX",
        include_bytes!("fixtures/fbx/boxWithCompressedCTypeArray.FBX"),
    ),
    (
        "cubes_with_mirroring_and_pivot.fbx",
        include_bytes!("fixtures/fbx/cubes_with_mirroring_and_pivot.fbx"),
    ),
    (
        "maxPbrMaterial_metalRough.fbx",
        include_bytes!("fixtures/fbx/maxPbrMaterial_metalRough.fbx"),
    ),
    (
        "animation_with_skeleton.fbx",
        include_bytes!("fixtures/fbx/animation_with_skeleton.fbx"),
    ),
];

const MIRRORED_CUBES: &[u8] = include_bytes!("fixtures/fbx/cubes_with_mirroring_and_pivot.fbx");

/// Where a primitive's geometry actually sits once transforms have been applied.
fn centroid(primitive: &Primitive) -> [f32; 3] {
    // Vertex counts here are in the thousands; f32 represents them exactly.
    #[allow(clippy::cast_precision_loss)]
    let n = primitive.positions.len() as f32;
    primitive.positions.iter().fold([0.0_f32; 3], |mut acc, v| {
        for axis in 0..3 {
            acc[axis] += v[axis] / n;
        }
        acc
    })
}

/// Signed volume of a closed triangle soup (divergence theorem). Positive means the winding is
/// consistently outward; a mirrored instance whose winding was never flipped comes out negative.
fn signed_volume(primitive: &Primitive) -> f32 {
    let p = &primitive.positions;
    primitive
        .indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let (a, b, c) = (p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]);
            let cross = [
                b[1] * c[2] - b[2] * c[1],
                b[2] * c[0] - b[0] * c[2],
                b[0] * c[1] - b[1] * c[0],
            ];
            (a[0] * cross[0] + a[1] * cross[1] + a[2] * cross[2]) / 6.0
        })
        .sum()
}

#[test]
fn node_transforms_place_each_instance_where_the_file_puts_it() {
    // THE regression. This fixture is four cubes at four different places, with pivots and mirroring,
    // which is why it was chosen in the first place. Reading only mesh-local vertex positions imported
    // all four on top of each other at the origin, and every shape-only assertion still passed.
    let asset = FbxImporter::new().import(MIRRORED_CUBES).expect("import");
    assert!(
        asset.primitives.len() >= 4,
        "expected the fixture's four placed cubes, got {}",
        asset.primitives.len()
    );

    let centroids: Vec<[f32; 3]> = asset.primitives.iter().map(centroid).collect();
    for (i, a) in centroids.iter().enumerate() {
        for (j, b) in centroids.iter().enumerate().skip(i + 1) {
            let apart = (0..3).map(|k| (a[k] - b[k]).abs()).fold(0.0_f32, f32::max);
            assert!(
                apart > 1e-4,
                "primitives {i} and {j} share a centroid ({a:?}) - node transforms are being dropped"
            );
        }
    }
}

#[test]
fn a_mirrored_instance_is_not_imported_inside_out() {
    // A negative-determinant transform reverses triangle winding. Left alone the mesh renders as a hollow
    // shell under backface culling - it looks *almost* right, which is what makes it dangerous. Signed
    // volume is the direct test: an inside-out closed box has negative volume.
    let asset = FbxImporter::new().import(MIRRORED_CUBES).expect("import");
    for (index, primitive) in asset.primitives.iter().enumerate() {
        let volume = signed_volume(primitive);
        assert!(
            volume > 0.0,
            "primitive {index} has signed volume {volume} - its winding is inside-out, so a mirrored \
             instance was imported without reversing its triangles"
        );
    }
}

#[test]
fn a_z_up_file_is_converted_to_the_editors_y_up_convention() {
    // 3ds Max writes Z-up. This box rests ON the ground plane in its source file, so after conversion the
    // vertical axis must be the one starting at zero and the two horizontal axes must straddle zero.
    let asset = FbxImporter::new()
        .import(include_bytes!("fixtures/fbx/maxPbrMaterial_metalRough.fbx"))
        .expect("import");
    let b = asset.bounds();
    assert!(
        b.min[1].abs() < 0.05 * (b.max[1] - b.min[1]),
        "the box should rest on y=0 after Z-up conversion, but y spans {}..{}",
        b.min[1],
        b.max[1]
    );
    for axis in [0, 2] {
        assert!(
            b.min[axis] < 0.0 && b.max[axis] > 0.0,
            "axis {axis} should straddle the origin after conversion, but spans {}..{}",
            b.min[axis],
            b.max[axis]
        );
    }
}

#[test]
fn real_materials_are_read_rather_than_replaced_with_a_grey() {
    // These two fixtures exist to cover the two material worlds FBX has. Before the audit they imported
    // identically, because every material was discarded and replaced with one mid-grey.
    let phong = FbxImporter::new()
        .import(include_bytes!("fixtures/fbx/phong_cube.fbx"))
        .expect("phong");
    assert!(
        phong
            .materials
            .iter()
            .any(|m| (m.base_color[0] - m.base_color[1]).abs() > 0.05),
        "the Phong fixture's material is coloured, but every imported material is grey: {:?}",
        phong.materials
    );

    let pbr = FbxImporter::new()
        .import(include_bytes!("fixtures/fbx/maxPbrMaterial_metalRough.fbx"))
        .expect("pbr");
    assert!(
        pbr.materials.iter().any(|m| m.metallic > 0.0),
        "a metal-rough fixture must import a non-zero metalness: {:?}",
        pbr.materials
    );

    // Alpha is never zero: ufbx reports RGB-only colours with w = 0, which taken verbatim would make an
    // imported mesh invisible rather than visibly wrong - the hardest kind of import bug to diagnose.
    for asset in [&phong, &pbr] {
        for material in &asset.materials {
            assert!(
                material.base_color[3] > 0.0,
                "material imported fully transparent: {:?}",
                material.base_color
            );
        }
    }
}

#[test]
fn a_multi_material_mesh_keeps_its_materials_apart() {
    // Every primitive used to carry `material: 0`, so a model with distinct materials rendered in one
    // colour. The spider has several, and the mirrored-cubes fixture has a green one and a red one.
    for (name, bytes) in [
        ("spider.fbx", &include_bytes!("fixtures/fbx/spider.fbx")[..]),
        ("cubes_with_mirroring_and_pivot.fbx", MIRRORED_CUBES),
    ] {
        let asset = FbxImporter::new().import(bytes).expect("import");
        let mut used: Vec<usize> = asset.primitives.iter().map(|p| p.material).collect();
        used.sort_unstable();
        used.dedup();
        assert!(
            used.len() > 1,
            "{name} references several materials but everything imported as material {used:?}"
        );
    }
}

#[test]
fn an_imported_asset_is_named_after_its_contents() {
    // The literal "fbx" for every file made the inspector and the outliner useless for telling imports
    // apart once a project had more than one.
    for (name, bytes) in FIXTURES {
        let asset = FbxImporter::new().import(bytes).expect("import");
        assert!(
            asset.name != "fbx" && !asset.name.is_empty(),
            "{name}: imported with the placeholder name {:?}",
            asset.name
        );
    }
}

#[test]
fn every_primitive_references_a_material_that_exists() {
    // The fallback slot is dropped when unused, so an off-by-one there would produce an out-of-range
    // material index - a silent wrong colour, or a panic in the renderer.
    for (name, bytes) in FIXTURES {
        let asset = FbxImporter::new().import(bytes).expect("import");
        assert!(!asset.materials.is_empty(), "{name}: no materials at all");
        for (index, primitive) in asset.primitives.iter().enumerate() {
            assert!(
                primitive.material < asset.materials.len(),
                "{name}[{index}]: material {} is out of range of {} materials",
                primitive.material,
                asset.materials.len()
            );
        }
    }
}

#[test]
fn per_vertex_streams_stay_aligned_and_normals_stay_unit_length() {
    // Positions, normals and UVs are emitted per corner in one pass. A mismatch would misalign shading
    // across a whole mesh and is invisible to a bounds or index-range check. Normal length is the
    // non-uniform-scale guard: transforming through the inverse-transpose changes a normal's length, so
    // failing to re-normalise leaves shading subtly wrong everywhere the model is squashed.
    for (name, bytes) in FIXTURES {
        let asset = FbxImporter::new().import(bytes).expect("import");
        for (index, p) in asset.primitives.iter().enumerate() {
            assert!(
                p.normals.is_empty() || p.normals.len() == p.positions.len(),
                "{name}[{index}]: {} normals for {} positions",
                p.normals.len(),
                p.positions.len()
            );
            assert!(
                p.uvs.is_empty() || p.uvs.len() == p.positions.len(),
                "{name}[{index}]: {} uvs for {} positions",
                p.uvs.len(),
                p.positions.len()
            );
            for n in &p.normals {
                let len = n[0].mul_add(n[0], n[1].mul_add(n[1], n[2] * n[2])).sqrt();
                assert!(
                    (len - 1.0).abs() < 1e-2 || len < 1e-6,
                    "{name}[{index}]: normal length {len} - re-normalisation after a non-uniform scale \
                     is missing"
                );
            }
        }
    }
}

#[test]
fn a_hostile_or_truncated_file_is_refused_rather_than_crashing() {
    // Untrusted-asset safety (ADR-031). Truncating a real file at many points is a far better probe than
    // random bytes, because it stays structurally valid right up to where it stops - which is exactly the
    // shape of a partial download or a corrupted upload.
    let importer = FbxImporter::new();
    for (name, bytes) in FIXTURES {
        for numerator in 1..16_usize {
            let cut = bytes.len() * numerator / 16;
            // Either outcome is acceptable. A panic, a hang or an out-of-memory is not, and that is what
            // this actually guards.
            let _ = importer.import(&bytes[..cut]);
        }
        // Bit-flips in the middle of an otherwise valid file: the other common corruption.
        for offset in [bytes.len() / 3, bytes.len() / 2, bytes.len() * 2 / 3] {
            let mut corrupted = bytes.to_vec();
            corrupted[offset] ^= 0xFF;
            let _ = importer.import(&corrupted);
        }
        // And a file whose header claims one thing while the body is garbage.
        let mut lying = bytes[..64.min(bytes.len())].to_vec();
        lying.extend(std::iter::repeat_n(0xAB, 4096));
        let _ = importer.import(&lying);
        assert!(!name.is_empty());
    }
}
