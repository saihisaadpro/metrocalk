//! Core pipeline tests (wasm-clean — no `3dxml` feature). The 3DXML-container tests live behind the feature
//! in `threedxml.rs`'s own test module + the editor-shell spike (which imports the REAL bar file).
#![allow(clippy::float_cmp)] // transforms here are exact literals / exact integer arithmetic — exact compare

use super::*;
use crate::step::{CadEdge, CadFace, CadInterchange, FaceKind};
use crate::StepInterchange;
use metrocalk_csg::validate;

/// A real ADVANCED_BREP cube (2×2×2 mm) — the same fixture the STEP reader tests use.
const CUBE_STEP: &str = include_str!("../../tests/fixtures/cube_ap242.step");

// ── The STEP AP242 leg: exact B-rep → deterministic tessellation → never-empty + never-silent ────────────

#[test]
fn step_reader_is_never_empty_and_never_silent_with_exact_brep() {
    let imp = StepAssemblyReader
        .read(CUBE_STEP.as_bytes())
        .expect("import");
    assert!(imp.never_empty(), "every part has a placed mesh");
    assert!(
        imp.never_silent(),
        "every part has a reason (+ fix if below exact)"
    );
    assert_eq!(imp.part_count(), 1, "the cube is one solid → one part");
    let p = &imp.parts[0];
    assert_eq!(p.strategy, ImportStrategy::ExactBrep);
    assert_eq!(p.fidelity, PartFidelity::ExactBrep);
    assert!(p.fix.is_none(), "exact needs no fix path");
    // The mesh is the real, watertight cube tessellation (not a proxy).
    let m = &imp.meshes[p.mesh.unwrap()];
    assert!(!m.is_proxy);
    assert_eq!(m.tris.triangle_count(), 12, "6 quads → 12 triangles");
    assert!(
        validate(&m.tris).watertight,
        "the exact tessellation is watertight"
    );
}

// ── The multi-strategy cascade: proprietary / encrypted / missing all render + report, never silent ──────

fn raw(id: u64, source: PartSource) -> RawPart {
    RawPart {
        id,
        name: format!("part {id}"),
        reference: format!("ref-{id}"),
        transform: IDENTITY_4X4,
        source,
    }
}

#[test]
fn the_cascade_places_and_diagnoses_every_hard_case_never_a_black_hole() {
    // A part per hard case: a proprietary CATIA rep (the crane case), an encrypted part, a missing reference.
    let parts = vec![
        raw(
            1,
            PartSource::ProprietaryRep {
                encoding: "CATIA V5_CFV3/CB0001".into(),
            },
        ),
        raw(2, PartSource::Encrypted),
        raw(
            3,
            PartSource::Missing {
                detail: "3DRep urn:...:missing".into(),
            },
        ),
    ];
    let imp = build_import(
        "hard cases".into(),
        "TEST".into(),
        Units::SI,
        123,
        parts,
        0,
        vec![],
    );
    // NEVER-EMPTY: all three render (as the shared proxy) — a part that would 0-triangle-fail in Datasmith
    // still appears.
    assert!(
        imp.never_empty(),
        "even undecodable parts are placed (proxy)"
    );
    // NEVER-SILENT: each carries a reason + a fix path.
    assert!(imp.never_silent());
    let by_id = |id: u64| imp.parts.iter().find(|p| p.id == id).unwrap();
    assert_eq!(by_id(1).fidelity, PartFidelity::Proxy);
    assert!(by_id(1).reason.contains("V5_CFV3"), "the encoding is named");
    assert!(by_id(1).fix.as_ref().unwrap().contains("STEP AP242"));
    assert_eq!(by_id(2).fidelity, PartFidelity::AccessDenied);
    assert!(by_id(2).reason.contains("DRM") || by_id(2).reason.contains("encrypted"));
    assert_eq!(by_id(3).fidelity, PartFidelity::Failed);
    assert!(by_id(3).fix.is_some());
    // All three proxies share ONE mesh (the never-empty floor is cheap → instanced).
    assert_eq!(imp.meshes.len(), 1, "one shared proxy box for all three");
    assert!(imp.meshes[0].is_proxy);
    assert_eq!(imp.instancing(), (1, 3), "1 unique mesh, 3 instances");
}

#[test]
fn a_zero_triangle_tessellation_cache_is_diagnosed_never_a_silent_success() {
    // The exact Datasmith failure mode: a part resolves to 0 triangles. It must NOT silently "succeed" as an
    // empty shell — it falls to a diagnosed proxy.
    let empty = TriMesh::new(vec![], vec![]);
    let imp = build_import(
        "zero-tri".into(),
        "TEST".into(),
        Units::SI,
        1,
        vec![raw(1, PartSource::Tessellation(empty))],
        0,
        vec![],
    );
    assert!(imp.never_empty(), "the 0-tri part still gets a proxy");
    assert_eq!(imp.parts[0].fidelity, PartFidelity::Failed);
    assert!(imp.parts[0].mesh.is_some());
}

// ── Dedup / instancing (CAD is bolt-heavy) ──────────────────────────────────────────────────────────────

#[test]
fn identical_geometry_dedups_to_one_mesh_and_instances() {
    // Two parts with the SAME B-rep faces (a repeated bolt) → tessellate once, instance twice.
    let scene = StepInterchange.import(CUBE_STEP.as_bytes()).unwrap();
    let faces = scene.solids[0].faces.clone();
    let mut t2 = IDENTITY_4X4;
    t2[12] = 100.0; // the second bolt is 100mm away
    let parts = vec![
        RawPart {
            id: 1,
            name: "bolt A".into(),
            reference: "bolt".into(),
            transform: IDENTITY_4X4,
            source: PartSource::ExactBrep(faces.clone()),
        },
        RawPart {
            id: 2,
            name: "bolt B".into(),
            reference: "bolt".into(),
            transform: t2,
            source: PartSource::ExactBrep(faces),
        },
    ];
    let imp = build_import(
        "bolts".into(),
        "TEST".into(),
        Units::SI,
        9,
        parts,
        0,
        vec![],
    );
    assert_eq!(imp.meshes.len(), 1, "identical geometry → ONE unique mesh");
    assert_eq!(imp.instancing(), (1, 2), "1 unique mesh, 2 instances");
    // Both parts reference the same mesh, at different transforms (real placement).
    assert_eq!(imp.parts[0].mesh, imp.parts[1].mesh);
    assert_ne!(imp.parts[0].transform, imp.parts[1].transform);
    assert_eq!(translation_of(&imp.parts[1].transform), [100.0, 0.0, 0.0]);
}

// ── Determinism: same geometry → bit-identical mesh hash (the regression-corpus property) ────────────────

#[test]
fn mesh_hash_is_deterministic_across_runs() {
    let scene = StepInterchange.import(CUBE_STEP.as_bytes()).unwrap();
    let faces = &scene.solids[0].faces;
    let h1 = mesh_hash(&tessellate_faces(faces));
    let h2 = mesh_hash(&tessellate_faces(faces));
    let h3 = mesh_hash(&tessellate_faces(faces));
    assert_eq!(h1, h2, "same faces → same hash");
    assert_eq!(
        h2, h3,
        "…every time (single-threaded, exact-coordinate weld)"
    );
    // The whole-import path is stable too (the corpus value the CI mesh-hash gate pins).
    let a = StepAssemblyReader.read(CUBE_STEP.as_bytes()).unwrap();
    let b = StepAssemblyReader.read(CUBE_STEP.as_bytes()).unwrap();
    assert_eq!(a.meshes[0].hash, b.meshes[0].hash);
}

#[test]
fn a_reordered_vertex_list_changes_the_hash_the_corpus_catches_drift() {
    // The hash keys on vertex ORDER (what a non-deterministic parallel mesher would perturb) — reordering
    // two vertices flips the hash, so the corpus catches a drift a triangle-count check would miss.
    let a = TriMesh::new(
        vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        vec![[0, 1, 2]],
    );
    let b = TriMesh::new(
        vec![[1.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        vec![[1, 0, 2]],
    );
    assert_ne!(
        mesh_hash(&a),
        mesh_hash(&b),
        "reordered vertices → different hash"
    );
}

// ── O(1) content-addressed re-import diff ────────────────────────────────────────────────────────────────

#[test]
fn reimport_of_unchanged_parts_is_all_unchanged_not_a_full_retessellation() {
    let a = StepAssemblyReader.read(CUBE_STEP.as_bytes()).unwrap();
    let b = StepAssemblyReader.read(CUBE_STEP.as_bytes()).unwrap();
    let d = diff(&a, &b);
    assert!(
        d.iter().all(|e| e.change == PartChange::Unchanged),
        "re-importing the same file → every part Unchanged (no re-tessellation)"
    );
}

#[test]
fn reimport_diff_detects_moved_changed_added_removed() {
    let scene = StepInterchange.import(CUBE_STEP.as_bytes()).unwrap();
    let faces = scene.solids[0].faces.clone();
    let base = || {
        vec![
            RawPart {
                id: 1,
                name: "keep".into(),
                reference: "a".into(),
                transform: IDENTITY_4X4,
                source: PartSource::ExactBrep(faces.clone()),
            },
            RawPart {
                id: 2,
                name: "move-me".into(),
                reference: "b".into(),
                transform: IDENTITY_4X4,
                source: PartSource::ExactBrep(faces.clone()),
            },
            RawPart {
                id: 3,
                name: "remove-me".into(),
                reference: "c".into(),
                transform: IDENTITY_4X4,
                source: PartSource::Encrypted,
            },
        ]
    };
    let before = build_import("v1".into(), "TEST".into(), Units::SI, 1, base(), 0, vec![]);

    // v2: part 2 moved; part 3 removed; part 4 added; part 1 unchanged.
    let mut moved = IDENTITY_4X4;
    moved[13] = 50.0;
    let mut after_parts = vec![
        RawPart {
            id: 1,
            name: "keep".into(),
            reference: "a".into(),
            transform: IDENTITY_4X4,
            source: PartSource::ExactBrep(faces.clone()),
        },
        RawPart {
            id: 2,
            name: "moved".into(),
            reference: "b".into(),
            transform: moved,
            source: PartSource::ExactBrep(faces.clone()),
        },
    ];
    after_parts.push(RawPart {
        id: 4,
        name: "new".into(),
        reference: "d".into(),
        transform: IDENTITY_4X4,
        source: PartSource::ExactBrep(faces),
    });
    let after = build_import(
        "v2".into(),
        "TEST".into(),
        Units::SI,
        2,
        after_parts,
        0,
        vec![],
    );

    let d = diff(&before, &after);
    let change = |id: u64| d.iter().find(|e| e.id == id).unwrap().change;
    assert_eq!(change(1), PartChange::Unchanged);
    assert_eq!(change(2), PartChange::Moved, "same geometry, new transform");
    assert_eq!(change(3), PartChange::Removed);
    assert_eq!(change(4), PartChange::Added);
}

// ── Small transform math (assembly-tree composition + pivots) ────────────────────────────────────────────

#[test]
fn mat4_mul_composes_translations_down_the_tree() {
    let mut parent = IDENTITY_4X4;
    parent[12] = 10.0; // parent at x=10
    let mut child = IDENTITY_4X4;
    child[13] = 5.0; // child at y=5 in the parent frame
    let world = mat4_mul(&parent, &child);
    assert_eq!(
        translation_of(&world),
        [10.0, 5.0, 0.0],
        "child world = parent ∘ local"
    );
}

#[test]
fn tessellate_faces_skips_curved_and_matches_the_scene_tessellation_for_one_solid() {
    // A single-solid part's tessellate_faces must agree with CadScene::tessellate (same weld, same hash) — the
    // per-part path is a refactor, not a behavior change.
    let scene = StepInterchange.import(CUBE_STEP.as_bytes()).unwrap();
    let per_part = tessellate_faces(&scene.solids[0].faces);
    let whole = scene.tessellate();
    assert_eq!(
        mesh_hash(&per_part),
        mesh_hash(&whole),
        "single-solid per-part tessellation == whole-scene tessellation"
    );
    // A curved face contributes no triangles here (the OCCT seam).
    let curved = vec![CadFace {
        id: 1,
        kind: FaceKind::Curved,
        outer: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
        edges: vec![CadEdge {
            id: 1,
            ends: [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
        }],
    }];
    assert_eq!(tessellate_faces(&curved).triangle_count(), 0);
}

/// The real AP242 tessellated-assembly shape (what commercial CAD exports, and what the M15.7 bar file uses):
/// a nested `REPOSITIONED_TESSELLATED_ITEM` / `TESSELLATED_GEOMETRIC_SET` hierarchy, each node carrying its own
/// `AXIS2_PLACEMENT_3D` reposition, down to `TESSELLATED_SOLID`s of `COMPLEX_TRIANGULATED_FACE`s. The reader
/// must (a) find the leaf solid's mesh from its shared `COORDINATES_LIST`, and (b) place it at the COMPOSITION
/// of every ancestor reposition (here z+10 over x+5 → (5,0,10)).
const TESS_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
FILE_NAME('tess','',(''),(''),'','','');\nFILE_SCHEMA(('AP242'));\nENDSEC;\nDATA;\n\
#5=COORDINATES_LIST('',3,((0.,0.,0.),(1.,0.,0.),(0.,1.,0.)));\n\
#6=COMPLEX_TRIANGULATED_FACE('',#5,3,((0.,0.,1.)),$,(1,2,3),((1,2,3)),());\n\
#10=TESSELLATED_SOLID('',(#6),$);\n\
#20=DIRECTION('',(0.,0.,1.));\n\
#21=DIRECTION('',(1.,0.,0.));\n\
#22=CARTESIAN_POINT('',(5.,0.,0.));\n\
#23=AXIS2_PLACEMENT_3D('',#22,#20,#21);\n\
#30=CARTESIAN_POINT('',(0.,0.,10.));\n\
#31=AXIS2_PLACEMENT_3D('',#30,#20,#21);\n\
#100=(GEOMETRIC_REPRESENTATION_ITEM()REPOSITIONED_TESSELLATED_ITEM(#23)REPRESENTATION_ITEM('')\
TESSELLATED_GEOMETRIC_SET((#10))TESSELLATED_ITEM());\n\
#200=(GEOMETRIC_REPRESENTATION_ITEM()REPOSITIONED_TESSELLATED_ITEM(#31)REPRESENTATION_ITEM('')\
TESSELLATED_GEOMETRIC_SET((#100))TESSELLATED_ITEM());\n\
ENDSEC;\nEND-ISO-10303-21;\n";

#[test]
fn tessellated_assembly_places_leaf_at_composed_reposition() {
    let entities = crate::step::parse_entities(TESS_STEP).unwrap();
    let parts = crate::step::parse_tessellated_assembly(&entities);
    assert_eq!(
        parts.len(),
        1,
        "one leaf TESSELLATED_SOLID → one placed part"
    );
    let p = &parts[0];
    assert_eq!(
        p.mesh.triangle_count(),
        1,
        "the COMPLEX_TRIANGULATED_FACE's single triangle survives the pnindex→coords remap"
    );
    // World = reposition(z+10) ∘ reposition(x+5) — both pure translations → (5,0,10).
    assert_eq!(
        translation_of(&p.transform),
        [5.0, 0.0, 10.0],
        "leaf placed at the COMPOSITION of both ancestor repositions"
    );
    assert_eq!(
        p.reference, "10",
        "reference keyed on the solid id (dedup key)"
    );
}

#[test]
fn no_tessellation_falls_through_to_the_planar_interpreter() {
    // A file with zero tessellation → parse_tessellated_assembly is empty so the reader takes the B-rep leg.
    let entities = crate::step::parse_entities(CUBE_STEP).unwrap();
    assert!(
        crate::step::parse_tessellated_assembly(&entities).is_empty(),
        "no TESSELLATED_* entities → empty, so StepAssemblyReader falls back to the planar interpret path"
    );
}

/// A `COMPLEX_TRIANGULATED_FACE`'s `triangle_fans` attribute (the LAST connectivity arg) must triangulate with
/// FAN topology — every triangle shares the first vertex — NOT the alternating-winding strip topology of the
/// `triangle_strips` attribute. The two attributes are the identical list-of-int-lists shape, so the reader
/// distinguishes them by POSITION (fans = last), not shape. Here the fan `(1,2,3,4)` → (0,1,2),(0,2,3); a strip
/// would instead give (0,1,2),(2,1,3) — the silently-wrong geometry this locks out.
const FAN_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
FILE_NAME('fan','',(''),(''),'','','');\nFILE_SCHEMA(('AP242'));\nENDSEC;\nDATA;\n\
#5=COORDINATES_LIST('',4,((0.,0.,0.),(1.,0.,0.),(1.,1.,0.),(0.,1.,0.)));\n\
#6=COMPLEX_TRIANGULATED_FACE('',#5,4,((0.,0.,1.)),$,(1,2,3,4),(),((1,2,3,4)));\n\
#10=TESSELLATED_SOLID('',(#6),$);\n\
ENDSEC;\nEND-ISO-10303-21;\n";

#[test]
fn complex_triangulated_face_fans_use_fan_topology_not_strip() {
    let entities = crate::step::parse_entities(FAN_STEP).unwrap();
    let parts = crate::step::parse_tessellated_assembly(&entities);
    assert_eq!(
        parts.len(),
        1,
        "the one tessellated solid → one placed part"
    );
    assert_eq!(
        parts[0].mesh.triangles,
        vec![[0u32, 1, 2], [0, 2, 3]],
        "triangle_fans triangulate as a fan (shared apex vertex 0), not a strip"
    );
}

/// A tessellated leaf whose faces yield NO usable triangles must STILL be reported (never-silent) — the reader
/// emits it with an empty mesh so `build_import` routes it to a diagnosed bounding proxy, never dropping it.
const EMPTY_LEAF_STEP: &str = "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\n\
FILE_NAME('empty','',(''),(''),'','','');\nFILE_SCHEMA(('AP242'));\nENDSEC;\nDATA;\n\
#10=TESSELLATED_SOLID('',(),$);\n\
ENDSEC;\nEND-ISO-10303-21;\n";

#[test]
fn a_leaf_with_no_decodable_geometry_is_reported_not_silently_dropped() {
    let entities = crate::step::parse_entities(EMPTY_LEAF_STEP).unwrap();
    let parts = crate::step::parse_tessellated_assembly(&entities);
    assert_eq!(
        parts.len(),
        1,
        "the geometry-less leaf is still emitted (never-silent) — build_import will diagnose + proxy it"
    );
    assert_eq!(parts[0].mesh.triangle_count(), 0, "…with an empty mesh");
}
