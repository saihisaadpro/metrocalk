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

pub mod audio;
pub mod autorig;
pub mod demo;
pub mod env_import;
/// FBX import (M11.1, ADR-040) — native-only (`ufbx` C-FFI), behind the `fbx` feature so the default crate
/// stays wasm32-clean. The browser funnel converts FBX server-side (the explicit wasm seam).
#[cfg(feature = "fbx")]
pub mod fbx_import;
pub mod gltf_import;
pub mod gpu;
pub mod image_import;
pub mod import;
/// KTX2/basis texture transcode (M11.1, ADR-040) — native-only (`basis-universal` C++ FFI), behind the
/// `ktx2` feature so the default crate stays wasm32-clean. The browser funnel transcodes server-side.
#[cfg(feature = "ktx2")]
pub mod ktx2_import;
pub mod lod;
pub mod mesh;
pub mod obj_import;
/// M11.5 (ADR-044) — asset identity: a provenance record + perceptual-hash near-dup detection, riding the
/// content-addressed store. Pure-Rust; the C2PA backing + offline auto-rig are seams behind it.
pub mod provenance;
/// M11.5 (ADR-044) — the cryptographic provenance signing backing (Ed25519, the C2PA trust model). Behind
/// the `signing` feature so the default crate stays minimal + wasm-tripwire-clean.
#[cfg(feature = "signing")]
pub mod signed;
pub mod source;
pub mod store;

pub use audio::{AudioAsset, AudioFormat, AudioImporter, AudioSource, AudioStore};
pub use autorig::{bake_standard_lbs, AutoRig, AutoRigJoint, NeuralRigImporter};
#[cfg(feature = "fbx")]
pub use fbx_import::FbxImporter;
pub use gltf_import::GltfImporter;
pub use gpu::{MeshGpu, MeshVertex};
pub use image_import::{ImageImporter, MAX_TEXELS};
pub use import::{detect, import_any, Detected, ImportedAsset};
#[cfg(feature = "ktx2")]
pub use ktx2_import::{transcode_to_rgba8, KtxImporter};
pub use lod::{GridClusterLod, LodConfig, LodGenerator, MeshLod};
pub use mesh::{Bounds, Material, MeshAsset, Primitive, Texture};
pub use obj_import::ObjImporter;
pub use provenance::{
    hamming_distance, is_near_duplicate, perceptual_hash, AssetKind, ContentAddressTrust,
    Provenance, ProvenanceVerifier, TamperError,
};
#[cfg(feature = "signing")]
pub use signed::SignedProvenanceTrust;
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
        // The packer derives crease-aware smooth normals. An octahedron's faces meet at ~109° — far above the
        // 30° crease angle — so every welded vertex SPLITS per-face: 8 faces × 3 corners = 24 flat-shaded
        // vertices (correctly faceted, not wrongly smoothed into a blob). The triangle count is unchanged.
        let gpu = MeshGpu::from_asset(&asset);
        assert_eq!(gpu.vertex_count(), 24, "sharp facets crease-split per-face");
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

    // ── M9.5 / G5: neural auto-rig as OFFLINE asset-prep → baked standard LBS (never runtime NN) ─────

    #[test]
    fn neural_autorig_bakes_a_topo_sorted_standard_lbs_rig() {
        // A neural rigger's (reversed-order, un-normalized) prediction imports through the M4 trait and
        // BAKES a standard rig: joints topologically sorted (parent < child), skinning matrices identity
        // at bind, every vertex normalized to a partition of unity with ≤ 4 influences.
        let asset = NeuralRigImporter::new()
            .import(&demo::neural_rigged_blob())
            .expect("bake neural rig");
        let skel = asset.skeleton.as_ref().expect("the bake emits a skeleton");
        assert_eq!(skel.joints.len(), 5, "5-joint chain");
        for (i, j) in skel.joints.iter().enumerate() {
            if let Some(p) = j.parent {
                assert!(p < i, "topologically sorted (parent {p} < child {i})");
            }
        }
        // Identity skinning matrices at bind (a bound vertex is unmoved in the rest pose).
        for m in skel.skinning_matrices(&metrocalk_skeleton::Pose::new()) {
            for (c, col) in m.iter().enumerate() {
                for (r, &val) in col.iter().enumerate() {
                    let id = if c == r { 1.0 } else { 0.0 };
                    assert!((val - id).abs() < 1e-4, "skinning matrix identity at bind");
                }
            }
        }
        let prim = &asset.primitives[0];
        assert_eq!(prim.joints.len(), prim.positions.len());
        for w in &prim.weights {
            let sum: f32 = w.iter().sum();
            assert!(
                (sum - 1.0).abs() < 1e-5,
                "weights normalized to a partition of unity"
            );
        }
    }

    #[test]
    fn neural_autorig_reduces_over_four_influences_to_top_four() {
        // Vertex 0 was predicted with 5 influences (weights 1..5). The bake keeps the top 4 (drops the
        // weight-1 influence → input joint "L4") and renormalizes. So vertex 0 has exactly 4 non-zero
        // weights summing to 1, and the dropped joint is absent.
        let asset = NeuralRigImporter::new()
            .import(&demo::neural_rigged_blob())
            .expect("bake");
        let prim = &asset.primitives[0];
        let w0 = prim.weights[0];
        let nonzero = w0.iter().filter(|&&x| x > 0.0).count();
        assert_eq!(nonzero, 4, "5 influences reduced to the top 4");
        assert!((w0.iter().sum::<f32>() - 1.0).abs() < 1e-5, "renormalized");
        // The strongest predicted influence (weight 5 → logical root L0, baked to joint 0) survives.
        assert!(
            prim.joints[0]
                .iter()
                .zip(w0)
                .any(|(&j, x)| j == 0 && x > 0.0),
            "the strongest influence (the root) is kept"
        );
    }

    #[test]
    fn a_neural_rigged_character_poses_via_g3_lbs() {
        // The success criterion: a neural-rigged import poses via G3's deterministic LBS — bend an upper
        // joint, a top vertex (bound high on the chain) swings; the bottom (root-bound) holds.
        use metrocalk_skeleton::{skin_position, Pose};
        let asset = NeuralRigImporter::new()
            .import(&demo::neural_rigged_blob())
            .expect("bake");
        let skel = asset.skeleton.as_ref().unwrap();
        let prim = &asset.primitives[0];

        let mut pose = Pose::new();
        let mut bent = skel.joints[1].local_bind; // bend joint 1 (low on the chain) about +Z
        bent.rotation = [0.0, 0.0, (0.7f32).sin(), (0.7f32).cos()];
        pose.set(1, bent);
        let skin = skel.skinning_matrices(&pose);
        let moved = |v: usize| {
            let p = prim.positions[v];
            let d = skin_position(p, prim.joints[v], prim.weights[v], &skin);
            ((d[0] - p[0]).powi(2) + (d[1] - p[1]).powi(2) + (d[2] - p[2]).powi(2)).sqrt()
        };
        // Vertex index = row*2 + col; the top row (y=6) is verts 12,13, the bottom (y=0) is 0,1.
        // (Vertex 0 has the synthetic 5-influence weights, so use vertex 1 for the "bottom holds" check.)
        let bottom = moved(1);
        let top = moved(12);
        assert!(bottom < 0.05, "the root-bound bottom holds: {bottom}");
        assert!(
            top > 1.0,
            "the chain-bound top swings under the bend: {top}"
        );
    }

    #[test]
    fn neural_autorig_rejects_a_malformed_blob() {
        let imp = NeuralRigImporter::new();
        assert!(matches!(
            imp.import(b"not an mtkrig blob").unwrap_err(),
            ImportError::Malformed(_)
        ));
        // A truncated-but-correct-magic blob is also rejected (the bounds-checked reader, no panic).
        let mut good = demo::neural_rigged_blob();
        good.truncate(20);
        assert!(matches!(
            imp.import(&good).unwrap_err(),
            ImportError::Malformed(_)
        ));
    }
}
