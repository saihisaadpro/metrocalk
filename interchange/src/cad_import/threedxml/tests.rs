//! Synthetic-3DXML tests (behind the `3dxml` feature) — reproduce the exact Unreal failure modes on a small,
//! CI-runnable fixture: a proprietary CATIA rep (→ proxy + kernel-seam diagnosis), an open XML tessellation
//! cache (→ rendered instantly), real per-instance transforms, and dedup/instancing. The REAL 222 MB bar
//! file is the local-only headline proof (editor-shell's `universal_cad_import_spike`).

#![allow(clippy::float_cmp, clippy::cast_possible_truncation)] // exact-literal transforms + i64-binned checks

use super::*;
use crate::{translation_of, CadReader, PartFidelity};
use std::io::{Cursor, Write};
use zip::write::{SimpleFileOptions, ZipWriter};

/// Build a ZIP (the 3DXML container) from named entries — deflate-compressed, exactly like a real 3DXML.
fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut cur = Cursor::new(Vec::new());
    {
        let mut zw = ZipWriter::new(&mut cur);
        let opts =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in entries {
            zw.start_file(*name, opts).expect("start_file");
            zw.write_all(data).expect("write");
        }
        zw.finish().expect("finish");
    }
    cur.into_inner()
}

const MANIFEST: &str = "<?xml version='1.0'?><Manifest><Root>PRODUCT.3dxml</Root></Manifest>";

/// An OPEN XML 3DRep (the 3DXML PolygonalRepType tessellation) — a quad (4 verts, 2 tris).
const OPEN_REP: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<XMLRepresentation><Root><Rep xsi:type="PolygonalRepType">
  <Faces><Face triangles="0 1 2 0 2 3"/></Faces>
  <VertexBuffer><Positions>0 0 0 1 0 0 1 1 0 0 1 0</Positions></VertexBuffer>
</Rep></Root></XMLRepresentation>"#;

/// A PROPRIETARY CATIA rep (the V5_CFV3/CB0001 magic the real bar file carries) — binary, unreadable here.
fn proprietary_rep() -> Vec<u8> {
    let mut v = b"V5_CFV3\x00\x00\x00\x03CATIA_V5 CB0001".to_vec();
    v.extend_from_slice(&[0u8; 64]); // some binary body
    v
}

/// A product structure: root(1) aggregates two open-rep bolts (inst 10 @origin, inst 11 @x=100, both →ref 2)
/// and one proprietary native part (inst 12 @y=200 →ref 4).
const PRODUCT: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<Model_3dxml xmlns="http://www.3ds.com/xsd/3DXML">
 <ProductStructure root="1">
  <Reference3D id="1" name="root"><V_Name>Skid Test Assembly</V_Name></Reference3D>
  <Reference3D id="2" name="bolt"><V_Name>Weld Bolt</V_Name></Reference3D>
  <Reference3D id="4" name="native"><V_Name>Weld Boom Base</V_Name></Reference3D>
  <ReferenceRep id="3" associatedFile="urn:3DXML:bolt.3DRep"><V_Name>Bolt Rep</V_Name></ReferenceRep>
  <ReferenceRep id="5" associatedFile="urn:3DXML:native.3DRep"><V_Name>Boom Base Rep</V_Name></ReferenceRep>
  <InstanceRep id="30"><IsAggregatedBy>2</IsAggregatedBy><IsInstanceOf>3</IsInstanceOf></InstanceRep>
  <InstanceRep id="50"><IsAggregatedBy>4</IsAggregatedBy><IsInstanceOf>5</IsInstanceOf></InstanceRep>
  <Instance3D id="10" name=""><IsAggregatedBy>1</IsAggregatedBy><IsInstanceOf>2</IsInstanceOf><RelativeMatrix>1 0 0 0 1 0 0 0 1 0 0 0</RelativeMatrix></Instance3D>
  <Instance3D id="11" name=""><IsAggregatedBy>1</IsAggregatedBy><IsInstanceOf>2</IsInstanceOf><RelativeMatrix>1 0 0 0 1 0 0 0 1 100 0 0</RelativeMatrix></Instance3D>
  <Instance3D id="12" name=""><IsAggregatedBy>1</IsAggregatedBy><IsInstanceOf>4</IsInstanceOf><RelativeMatrix>1 0 0 0 1 0 0 0 1 0 200 0</RelativeMatrix></Instance3D>
 </ProductStructure>
</Model_3dxml>"#;

fn skid_3dxml() -> Vec<u8> {
    build_zip(&[
        ("Manifest.xml", MANIFEST.as_bytes()),
        ("PRODUCT.3dxml", PRODUCT.as_bytes()),
        ("bolt.3DRep", OPEN_REP.as_bytes()),
        ("native.3DRep", &proprietary_rep()),
    ])
}

#[test]
fn the_3dxml_imports_never_empty_never_silent_with_real_transforms_and_dedup() {
    let bytes = skid_3dxml();
    assert!(ThreeDxmlReader.can_read(&bytes), "sniffs the ZIP container");
    let imp = ThreeDxmlReader.read(&bytes).expect("import the 3dxml");

    // NEVER-EMPTY + NEVER-SILENT — the two headline guarantees.
    assert!(
        imp.never_empty(),
        "every occurrence is placed (open mesh or proxy)"
    );
    assert!(
        imp.never_silent(),
        "every part has a reason (+ fix below exact)"
    );
    assert_eq!(imp.source_format, "CATIA-3DXML");
    assert_eq!(imp.name, "Skid Test Assembly");

    // Three occurrences: 2 open-tessellation bolts + 1 proprietary native part.
    assert_eq!(imp.part_count(), 3, "all three occurrences accounted for");
    let c = imp.fidelity_counts();
    assert_eq!(c.tessellation_only, 2, "the two open-rep bolts render");
    assert_eq!(
        c.proxy, 1,
        "the proprietary native part is a diagnosed proxy"
    );
    assert_eq!(c.failed, 0, "nothing failed silently");

    // DEDUP / INSTANCING: the two bolts share ONE tessellated mesh; the proxy is a second mesh.
    let bolts: Vec<_> = imp
        .parts
        .iter()
        .filter(|p| p.fidelity == PartFidelity::TessellationOnly)
        .collect();
    assert_eq!(bolts.len(), 2);
    assert_eq!(
        bolts[0].mesh, bolts[1].mesh,
        "identical geometry → shared mesh (instanced)"
    );
    assert_eq!(imp.instancing(), (2, 3), "2 unique meshes for 3 instances");

    // REAL TRANSFORMS (not the assembly-origin collapse): the bolts at x=0 and x=100; native at y=200.
    let tx: std::collections::BTreeSet<[i64; 3]> = imp
        .parts
        .iter()
        .map(|p| {
            let t = translation_of(&p.transform);
            [t[0] as i64, t[1] as i64, t[2] as i64]
        })
        .collect();
    assert!(tx.contains(&[0, 0, 0]), "bolt A at origin");
    assert!(
        tx.contains(&[100, 0, 0]),
        "bolt B at x=100 (real placement)"
    );
    assert!(tx.contains(&[0, 200, 0]), "native part at y=200");

    // The proprietary part is DIAGNOSED with the kernel/re-export fix path (the honest boundary).
    let native = imp
        .parts
        .iter()
        .find(|p| p.fidelity == PartFidelity::Proxy)
        .unwrap();
    assert!(
        native.reason.contains("V5_CFV3"),
        "the encoding is named honestly"
    );
    assert!(
        native.fix.as_ref().unwrap().contains("STEP AP242")
            || native.fix.as_ref().unwrap().contains("kernel"),
        "the fix path is offered"
    );
    assert_eq!(native.name, "Boom Base Rep", "the human name survives");
}

#[test]
fn the_3dxml_import_is_deterministic_across_runs() {
    // Same file → identical meshes (hashes) + identical part ids ×3 (the regression-corpus property, on the
    // container reader — single-threaded, exact-coordinate weld, deterministic assembly walk).
    let bytes = skid_3dxml();
    let a = ThreeDxmlReader.read(&bytes).unwrap();
    let b = ThreeDxmlReader.read(&bytes).unwrap();
    let c = ThreeDxmlReader.read(&bytes).unwrap();
    let sig = |imp: &crate::CadImport| -> Vec<(u64, u64)> {
        let mut v: Vec<(u64, u64)> = imp
            .parts
            .iter()
            .map(|p| (p.id, p.mesh.map_or(0, |i| imp.meshes[i].hash)))
            .collect();
        v.sort_unstable();
        v
    };
    assert_eq!(sig(&a), sig(&b), "run 1 == run 2");
    assert_eq!(sig(&b), sig(&c), "run 2 == run 3");
    assert_eq!(
        a.source_hash, b.source_hash,
        "provenance source hash stable"
    );
}

#[test]
fn the_source_hierarchy_is_preserved_as_a_named_group_tree() {
    // The user's ask: imported CAD keeps its original hierarchy / grouping / names, not a flat pile of hex ids.
    let bytes = skid_3dxml();
    let imp = ThreeDxmlReader.read(&bytes).expect("import the 3dxml");

    // The root assembly is preserved as ONE named GROUP container (the source `V_Name`), at the top of the tree.
    assert_eq!(
        imp.groups.len(),
        1,
        "the root assembly is one group container"
    );
    assert_eq!(
        imp.structural_nodes,
        imp.groups.len(),
        "the structural-node count mirrors the emitted tree"
    );
    let root = &imp.groups[0];
    assert_eq!(
        root.name, "Skid Test Assembly",
        "the source product name survives on the group (not a bare id)"
    );
    assert_eq!(root.parent, None, "the product root is a forest root");

    // Every leaf part nests UNDER a real named group (its source assembly occurrence) — grouping preserved.
    let group_ids: std::collections::BTreeSet<u64> = imp.groups.iter().map(|g| g.id).collect();
    assert!(
        imp.parts
            .iter()
            .all(|p| p.parent.is_some_and(|pid| group_ids.contains(&pid))),
        "every part is parented under a named group (the source tree), never orphaned/flat"
    );
    assert!(
        imp.parts.iter().all(|p| p.parent == Some(root.id)),
        "the three occurrences group under the root assembly exactly as the source nests them"
    );
}

#[test]
fn nested_subassemblies_are_preserved_as_a_multi_level_group_tree() {
    // root(1 "Weld Line") aggregates sub(2 "Robot Cell") via inst 10; the cell aggregates part(3 "Weld Bolt")
    // via inst 20 (at x=100); the part carries an open rep. Expect a 2-level tree: Weld Line › Robot Cell ›
    // (bolt leaf), with the leaf's transform WORLD-composed down the tree.
    let product = r#"<?xml version="1.0"?>
<Model_3dxml xmlns="http://www.3ds.com/xsd/3DXML">
 <ProductStructure root="1">
  <Reference3D id="1" name="line"><V_Name>Weld Line</V_Name></Reference3D>
  <Reference3D id="2" name="cell"><V_Name>Robot Cell</V_Name></Reference3D>
  <Reference3D id="3" name="bolt"><V_Name>Weld Bolt</V_Name></Reference3D>
  <ReferenceRep id="4" associatedFile="urn:3DXML:bolt.3DRep"><V_Name>Bolt Rep</V_Name></ReferenceRep>
  <InstanceRep id="40"><IsAggregatedBy>3</IsAggregatedBy><IsInstanceOf>4</IsInstanceOf></InstanceRep>
  <Instance3D id="10"><IsAggregatedBy>1</IsAggregatedBy><IsInstanceOf>2</IsInstanceOf><RelativeMatrix>1 0 0 0 1 0 0 0 1 0 0 0</RelativeMatrix></Instance3D>
  <Instance3D id="20"><IsAggregatedBy>2</IsAggregatedBy><IsInstanceOf>3</IsInstanceOf><RelativeMatrix>1 0 0 0 1 0 0 0 1 100 0 0</RelativeMatrix></Instance3D>
 </ProductStructure>
</Model_3dxml>"#;
    let bytes = build_zip(&[
        ("Manifest.xml", MANIFEST.as_bytes()),
        ("PRODUCT.3dxml", product.as_bytes()),
        ("bolt.3DRep", OPEN_REP.as_bytes()),
    ]);
    let imp = ThreeDxmlReader.read(&bytes).expect("import");
    assert!(imp.never_empty() && imp.never_silent());

    // TWO groups (Weld Line, Robot Cell) — the two assembly levels; the bolt is a leaf under the cell.
    assert_eq!(imp.groups.len(), 2, "two assembly levels → two group nodes");
    let by_name = |n: &str| {
        imp.groups
            .iter()
            .find(|g| g.name == n)
            .expect("group present")
    };
    let line = by_name("Weld Line");
    let cell = by_name("Robot Cell");
    assert_eq!(line.parent, None, "the line is the forest root");
    assert_eq!(
        cell.parent,
        Some(line.id),
        "the cell nests under the line (real multi-level nesting, not flattened)"
    );

    // The single bolt part nests under the CELL (the deepest assembly), exactly as the source nests it.
    assert_eq!(imp.part_count(), 1);
    assert_eq!(
        imp.parts[0].parent,
        Some(cell.id),
        "the part nests under its immediate sub-assembly"
    );
    assert_eq!(imp.parts[0].name, "Bolt Rep", "the human name survives");
    // Placement stays world-composed down the tree (identity cell + x=100 bolt → world x=100) — the leaf keeps
    // its true world transform, so the identity group containers never perturb placement.
    assert_eq!(
        translation_of(&imp.parts[0].transform)[0],
        100.0,
        "the leaf transform is world-composed down the assembly tree"
    );
}

#[test]
fn a_missing_rep_is_a_diagnosed_proxy_never_silent() {
    // The ZIP references native.3DRep but doesn't include it → the part is placed + diagnosed, never dropped.
    let bytes = build_zip(&[
        ("Manifest.xml", MANIFEST.as_bytes()),
        ("PRODUCT.3dxml", PRODUCT.as_bytes()),
        ("bolt.3DRep", OPEN_REP.as_bytes()),
        // native.3DRep intentionally absent
    ]);
    let imp = ThreeDxmlReader.read(&bytes).expect("import");
    assert!(imp.never_empty() && imp.never_silent());
    let native = imp.parts.iter().find(|p| p.reference == "4").unwrap();
    assert_eq!(
        native.fidelity,
        PartFidelity::Failed,
        "missing rep → a diagnosed failure"
    );
    assert!(
        native.mesh.is_some(),
        "still placed as a proxy (never a black hole)"
    );
    assert!(native.reason.contains("missing") || native.reason.contains("unresolved"));
}

#[test]
fn malformed_containers_are_explained_never_a_panic() {
    // Not a ZIP.
    assert!(matches!(
        ThreeDxmlReader.read(b"just some bytes"),
        Err(CadError::Malformed(_) | CadError::Unrecognized(_))
    ));
    // A valid ZIP with no product structure.
    let empty = build_zip(&[("readme.txt", b"hello")]);
    assert!(matches!(
        ThreeDxmlReader.read(&empty),
        Err(CadError::Unrecognized(_))
    ));
    // can_read is false for a non-ZIP.
    assert!(!ThreeDxmlReader.can_read(b"ISO-10303-21;"));
}

#[test]
fn multi_rep_and_dangling_reps_are_all_placed_never_silently_dropped() {
    // A Reference3D with TWO ReferenceReps (a multi-body/LOD part) — BOTH must be placed, not just the last;
    // and a dangling rep (an InstanceRep pointing at a ReferenceRep id that was never defined) must be a
    // diagnosed Missing proxy, never a silent skip. (The adversarial-review MINOR/NIT fixes.)
    let product = r#"<?xml version="1.0"?>
<Model_3dxml xmlns="http://www.3ds.com/xsd/3DXML">
 <ProductStructure root="1">
  <Reference3D id="1" name="root"><V_Name>Multi Rep Asm</V_Name></Reference3D>
  <Reference3D id="2" name="part"><V_Name>Two Body Part</V_Name></Reference3D>
  <ReferenceRep id="3" associatedFile="urn:3DXML:a.3DRep"><V_Name>Body A</V_Name></ReferenceRep>
  <ReferenceRep id="4" associatedFile="urn:3DXML:b.3DRep"><V_Name>Body B</V_Name></ReferenceRep>
  <InstanceRep id="30"><IsAggregatedBy>2</IsAggregatedBy><IsInstanceOf>3</IsInstanceOf></InstanceRep>
  <InstanceRep id="31"><IsAggregatedBy>2</IsAggregatedBy><IsInstanceOf>4</IsInstanceOf></InstanceRep>
  <InstanceRep id="32"><IsAggregatedBy>2</IsAggregatedBy><IsInstanceOf>999</IsInstanceOf></InstanceRep>
  <Instance3D id="10"><IsAggregatedBy>1</IsAggregatedBy><IsInstanceOf>2</IsInstanceOf><RelativeMatrix>1 0 0 0 1 0 0 0 1 0 0 0</RelativeMatrix></Instance3D>
 </ProductStructure>
</Model_3dxml>"#;
    let bytes = build_zip(&[
        ("Manifest.xml", MANIFEST.as_bytes()),
        ("PRODUCT.3dxml", product.as_bytes()),
        ("a.3DRep", OPEN_REP.as_bytes()),
        ("b.3DRep", &proprietary_rep()),
    ]);
    let imp = ThreeDxmlReader.read(&bytes).expect("import");
    assert!(imp.never_empty() && imp.never_silent());
    // ONE occurrence of ref 2, but THREE reps (open + proprietary + dangling) → 3 parts, none dropped.
    assert_eq!(
        imp.part_count(),
        3,
        "all three reps placed (multi-rep + dangling)"
    );
    let c = imp.fidelity_counts();
    assert_eq!(c.tessellation_only, 1, "the open body renders");
    assert_eq!(c.proxy, 1, "the proprietary body is a diagnosed proxy");
    assert_eq!(
        c.failed, 1,
        "the dangling rep is a diagnosed Missing, not a silent skip"
    );
    let dangling = imp
        .parts
        .iter()
        .find(|p| p.fidelity == PartFidelity::Failed)
        .unwrap();
    assert!(dangling.reason.contains("missing") || dangling.reason.contains("dangling"));
    assert!(
        dangling.mesh.is_some(),
        "even the dangling rep is placed (never a black hole)"
    );
}

#[test]
fn parse_open_3drep_reads_positions_and_triangles() {
    let mesh = parse_open_3drep(OPEN_REP.as_bytes()).expect("parse a valid open rep");
    assert_eq!(mesh.positions.len(), 4, "a quad has 4 vertices");
    assert_eq!(mesh.triangle_count(), 2, "two triangles");
    // A malformed rep (index out of range) is dropped, never a panic.
    let bad = r#"<XMLRepresentation><Rep><Faces><Face triangles="0 1 9"/></Faces>
      <VertexBuffer><Positions>0 0 0 1 0 0</Positions></VertexBuffer></Rep></XMLRepresentation>"#;
    assert!(
        parse_open_3drep(bad.as_bytes()).is_none(),
        "out-of-range index → dropped"
    );
}

#[test]
fn relative_matrix_parses_to_column_major_with_translation() {
    let m = parse_matrix("1 0 0 0 1 0 0 0 1 10 20 30").unwrap();
    assert_eq!(
        [m[12], m[13], m[14]],
        [10.0, 20.0, 30.0],
        "origin in the last column"
    );
    assert_eq!(m[0], 1.0, "x-axis first");
    assert!(
        parse_matrix("1 2 3").is_none(),
        "not 12 numbers → None (never a panic)"
    );
}
