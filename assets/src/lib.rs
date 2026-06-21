//! `metrocalk-assets` — the local asset + import substrate (M4 / Phase-2 asset gate).
//!
//! The describe-to-create promise is a working, **visible** object. Until now every entity rendered
//! as an M2.2 placeholder cube; this crate is the substrate that lets a described (and later,
//! marketplace) object look like *itself*: import a real glTF/glb → the project's internal
//! [`mesh::MeshAsset`] (trait-wrapped, [`source::MeshSource`]) → a content-addressed
//! [`store::AssetStore`] beside the scene document → GPU-ready [`gpu::MeshGpu`] the native renderer
//! draws. An entity references an asset only by lightweight handle ([`store::AssetId`]); geometry
//! never enters the ECS or the Loro doc (invariants 1 & 2).
//!
//! **No foreign decoder type crosses the public surface** (invariant 5): `gltf::` / `image::` live
//! only in [`gltf_import`], exactly as `flecs_ecs` lives only in `/ecs` (CI grep-gated). And the whole
//! crate is `wasm32-unknown-unknown`-clean — no `/core` (Flecs), no Loro, no C FFI — so import +
//! mesh-data prep reach the browser (ADR-006). KTX2/basis-universal GPU-texture compression is a
//! native-only normalization step (basis-universal is C++ FFI) — documented in the asset ADR, not
//! built here.

pub mod demo;
pub mod gltf_import;
pub mod gpu;
pub mod mesh;
pub mod source;
pub mod store;

pub use gltf_import::GltfImporter;
pub use gpu::{MeshGpu, MeshVertex};
pub use mesh::{Bounds, Material, MeshAsset, Primitive, Texture};
pub use source::{ImportError, MeshSource, MAX_ELEMENTS, MAX_IMPORT_BYTES};
pub use store::{AssetId, AssetStore};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_healthbar_geometry_and_materials() {
        let bytes = demo::healthbar_glb();
        let asset = GltfImporter::new()
            .import(&bytes)
            .expect("import healthbar");
        // Two boxes → two primitives, two materials.
        assert_eq!(asset.primitives.len(), 2, "frame + fill primitives");
        assert_eq!(asset.materials.len(), 2);
        // Each box is 24 verts / 36 indices.
        assert_eq!(asset.vertex_count(), 48);
        assert_eq!(asset.index_count(), 72);
        assert_eq!(asset.triangle_count(), 24);
        // Bar bounds: wide (≈2.1) and short (≈0.56) — clearly not a unit cube.
        let b = asset.bounds();
        assert!((b.max[0] - b.min[0]) > 2.0, "bar is wide");
        assert!((b.max[1] - b.min[1]) < 0.6, "bar is short");
        // The fill material is reddish.
        assert!(asset
            .materials
            .iter()
            .any(|m| m.base_color[0] > 0.8 && m.base_color[1] < 0.3));
    }

    #[test]
    fn imports_prop_and_derives_normals_when_absent() {
        let bytes = demo::prop_glb();
        let asset = GltfImporter::new().import(&bytes).expect("import prop");
        assert_eq!(asset.primitives.len(), 1);
        assert_eq!(asset.vertex_count(), 6, "octahedron has 6 verts");
        assert_eq!(asset.triangle_count(), 8, "octahedron has 8 faces");
        assert!(
            asset.primitives[0].normals.is_empty(),
            "authored without normals"
        );
        // The packer derives them.
        let gpu = MeshGpu::from_asset(&asset);
        assert_eq!(gpu.vertex_count(), 6);
        assert_eq!(gpu.index_count(), 24);
        for v in &gpu.vertices {
            let len = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "derived normal is unit-length");
        }
    }

    #[test]
    fn imports_sphere_ball_test_mesh() {
        // The M8.2 physics test mesh: a smooth UV sphere (radius 0.5), authored with normals.
        let bytes = demo::sphere_glb();
        let asset = GltfImporter::new().import(&bytes).expect("import sphere");
        assert_eq!(asset.primitives.len(), 1);
        assert_eq!(asset.vertex_count(), 13 * 17, "13 stacks × 17 slices grid");
        assert_eq!(
            asset.triangle_count(),
            12 * 16 * 2,
            "two tris per stack×slice quad"
        );
        // Roughly a unit-diameter ball: ~1.0 extent on every axis, centred at the origin.
        let b = asset.bounds();
        for axis in 0..3 {
            assert!(
                (b.max[axis] - b.min[axis] - 1.0).abs() < 0.05,
                "axis {axis} spans ~1.0 (diameter)"
            );
            assert!(
                (b.max[axis] + b.min[axis]).abs() < 0.05,
                "centred on origin"
            );
        }
        // Authored normals survive packing and are unit-length (smooth shading).
        let gpu = MeshGpu::from_asset(&asset);
        for v in &gpu.vertices {
            let len = (v.normal[0].powi(2) + v.normal[1].powi(2) + v.normal[2].powi(2)).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "sphere normal is unit-length");
        }
    }

    #[test]
    fn imports_embedded_png_texture() {
        let bytes = demo::textured_quad_glb();
        let asset = GltfImporter::new().import(&bytes).expect("import textured");
        assert_eq!(
            asset.textures.len(),
            1,
            "the embedded base-color png decodes"
        );
        assert_eq!((asset.textures[0].width, asset.textures[0].height), (2, 2));
        assert_eq!(asset.textures[0].rgba8.len(), 2 * 2 * 4);
        assert_eq!(asset.materials[0].base_color_texture, Some(0));
    }

    #[test]
    fn rejects_oversize_input() {
        let big = vec![0u8; MAX_IMPORT_BYTES + 1];
        let err = GltfImporter::new().import(&big).unwrap_err();
        assert!(matches!(err, ImportError::TooLarge { .. }));
    }

    #[test]
    fn rejects_malformed_bytes() {
        let err = GltfImporter::new()
            .import(b"not a gltf at all")
            .unwrap_err();
        assert!(matches!(err, ImportError::Malformed(_)));
    }

    #[test]
    fn rejects_non_triangle_index_count() {
        // A triangle-list primitive with a 5-index buffer is malformed — the importer must reject it
        // fail-fast, never silently drop the trailing partial triangle (adversarial-review finding).
        let err = GltfImporter::new()
            .import(&demo::malformed_indices_glb())
            .unwrap_err();
        assert!(matches!(err, ImportError::Malformed(_)), "got {err:?}");
    }

    #[test]
    fn content_address_is_stable_and_distinct() {
        let a = AssetId::of_bytes(&demo::healthbar_glb());
        let a2 = AssetId::of_bytes(&demo::healthbar_glb());
        let p = AssetId::of_bytes(&demo::prop_glb());
        assert_eq!(
            a, a2,
            "same bytes → same handle (deterministic across reloads)"
        );
        assert_ne!(a, p, "different assets → different handles");
        assert!(a.as_str().starts_with("mtkasset:"));
    }

    #[test]
    fn store_imports_idempotently_and_resolves_by_handle() {
        let importer = GltfImporter::new();
        let mut store = AssetStore::new();
        let bytes = demo::healthbar_glb();
        let id1 = store.import(&importer, &bytes).expect("import");
        let id2 = store.import(&importer, &bytes).expect("re-import");
        assert_eq!(id1, id2);
        assert_eq!(
            store.len(),
            1,
            "re-importing identical bytes does not duplicate"
        );
        assert!(store.contains(id1.as_str()));
        assert!(store.get_str(id1.as_str()).is_some());
        assert!(store.get_str("mtkasset:deadbeef").is_none());
    }

    #[test]
    fn gpu_pack_merges_primitives_and_bakes_color() {
        let asset = GltfImporter::new()
            .import(&demo::healthbar_glb())
            .expect("import");
        let gpu = MeshGpu::from_asset(&asset);
        assert_eq!(gpu.vertex_count(), 48);
        assert_eq!(gpu.index_count(), 72);
        // Indices stay in range after re-basing into the merged buffer.
        assert!(gpu
            .indices
            .iter()
            .all(|&i| (i as usize) < gpu.vertices.len()));
        // Both materials' colors appear baked into vertices.
        assert!(
            gpu.vertices.iter().any(|v| v.color[0] > 0.8),
            "red fill baked"
        );
        assert!(
            gpu.vertices.iter().any(|v| v.color[0] < 0.2),
            "dark frame baked"
        );
    }

    #[test]
    fn imports_a_skinned_rig_with_a_topo_sorted_skeleton() {
        // M9.3 / G3: a glTF `skin` loads through the importer as our `skeleton::Skeleton` (no `gltf::`
        // leak — the mapping lives in the wrapper), and each vertex carries `JOINTS_0`/`WEIGHTS_0`
        // remapped to the skeleton's topological order.
        use metrocalk_skeleton::{skin_position, Pose};
        let asset = GltfImporter::new()
            .import(&demo::skinned_quad_glb())
            .expect("import skinned");
        let skel = asset.skeleton.as_ref().expect("a glTF skin → a skeleton");
        assert_eq!(skel.joints.len(), 2, "root + child");
        assert_eq!(skel.joints[0].parent, None, "root joint");
        assert_eq!(
            skel.joints[1].parent,
            Some(0),
            "child parented to root (parent < child — topological order)"
        );
        // Bind: the child joint sits at y=1, and the skinning matrices are identity (a bound vertex is
        // unmoved in the rest pose).
        assert!(
            (skel.joint_position(&Pose::new(), 1)[1] - 1.0).abs() < 1e-4,
            "child joint at y=1 in bind"
        );
        for m in skel.skinning_matrices(&Pose::new()) {
            for (c, col) in m.iter().enumerate() {
                for (r, &val) in col.iter().enumerate() {
                    let id = if c == r { 1.0 } else { 0.0 };
                    assert!((val - id).abs() < 1e-4, "skinning matrix identity at bind");
                }
            }
        }
        // The primitive carries per-vertex JOINTS_0/WEIGHTS_0 (remapped to topo order — identity here).
        let p = &asset.primitives[0];
        assert_eq!((p.joints.len(), p.weights.len()), (4, 4));
        assert_eq!(p.joints[0], [0, 0, 0, 0], "bottom vertex → root joint");
        assert_eq!(p.joints[2], [1, 0, 0, 0], "top vertex → child joint");
        assert!((p.weights[0][0] - 1.0).abs() < 1e-6, "fully weighted");
        // FK pose: bend the child 90° about Z → a top vertex (bound to the child) swings toward -X.
        let mut pose = Pose::new();
        let mut bent = skel.joints[1].local_bind;
        bent.rotation = [0.0, 0.0, 0.707_106_77, 0.707_106_77]; // 90° about +Z
        pose.set(1, bent);
        let skin = skel.skinning_matrices(&pose);
        let out = skin_position([0.2, 2.0, 0.0], [1, 0, 0, 0], [1.0, 0.0, 0.0, 0.0], &skin);
        assert!(
            out[0] < -0.5,
            "the child bend swung the top vertex toward -X (FK over the loaded rig), got {out:?}"
        );
    }

    #[test]
    fn a_static_mesh_carries_no_skeleton_or_skin_attrs() {
        // The existing un-rigged fixtures must stay skeleton-free (additive skin fields don't disturb them).
        let asset = GltfImporter::new()
            .import(&demo::prop_glb())
            .expect("import prop");
        assert!(
            asset.skeleton.is_none(),
            "an un-rigged mesh carries no skeleton"
        );
        assert!(asset
            .primitives
            .iter()
            .all(|p| p.joints.is_empty() && p.weights.is_empty()));
    }
}
