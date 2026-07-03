//! M15.7 (ADR-077) — **THE SPIKE** (deliverable #1): the go/no-go gate for the universal CAD import pipeline
//! that imports CAD "like importing a texture" — never a black screen, never a silently-dropped part.
//!
//! The bar to beat, exactly: a real CATIA 3DEXPERIENCE **3DXML** (221 MB) and its STEP AP242 re-export **both
//! defeated Unreal/Datasmith** — the 3DXML imported **1 of ~1,280 parts** (the rest 0-triangle shells); the
//! STEP re-export imported **nothing** (a black screen). This spike proves Metrocalk's pipeline is the
//! opposite on the same failure modes:
//!
//! - **STEP AP242 leg (CI-runnable):** a real ADVANCED_BREP part imports **exact, never-empty, never-silent,
//!   deterministic**, and lands as ONE undoable transaction of queryable entities (the re-export that
//!   black-screened Unreal — here it renders).
//! - **CATIA 3DXML leg (local-only gate):** the REAL 221 MB bar file — if present — imports **never-empty +
//!   never-silent**: every part placed at its real assembly transform + diagnosed (the proprietary V5_CFV3
//!   geometry is the licensed-kernel seam, ADR-070), deterministic across runs, deduped for instancing. The
//!   file is not in the repo/CI (it's the owner's proprietary assembly) → this leg is a **documented
//!   local-only gate** (like the `.exe` E2E), run when the file is on disk (set `MTK_3DXML_BAR_FILE` or the
//!   default path). The synthetic-3DXML failure modes ARE gated in CI (interchange's `threedxml` tests).
//!
//! A headless gate — the never-empty/never-silent/deterministic guarantees are asserted as **structured
//! signals** (part counts, fidelity tokens, mesh hashes, ECS field reads), never drifting UI copy.

use metrocalk_assets::AssetStore;
use metrocalk_core::{Engine, FieldValue};
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::cad_import::{import_cad, read_cad, CAD_PART};
use metrocalk_editor_shell::{CapResolver, CapScene, MESH_FIELD};

const CUBE_STEP: &str = include_str!("../../interchange/tests/fixtures/cube_ap242.step");

fn engine() -> (Engine<FlecsWorld>, CapScene) {
    let mut world = FlecsWorld::new();
    let scene = CapScene::intern(&mut world);
    let mut engine = Engine::new(world, 1);
    engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
    (engine, scene)
}

/// A stable signature over an import (part ids + mesh hashes + transforms) — the determinism check.
fn signature(imp: &metrocalk_interchange::CadImport) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for p in &imp.parts {
        mix(p.id);
        mix(p.mesh.map_or(0, |i| imp.meshes[i].hash));
        for v in &p.transform {
            mix(v.to_bits());
        }
    }
    h
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// LEG 1 — STEP AP242 (CI-runnable): the re-export that black-screened Unreal, here exact + landed + undoable
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

#[test]
fn step_ap242_imports_never_empty_never_silent_exact_and_lands_undoable() {
    let (mut engine, scene) = engine();
    let mut store = AssetStore::new();

    // Read + report.
    let report = read_cad(CUBE_STEP.as_bytes()).expect("STEP imports (no black screen)");
    assert!(
        report.never_empty(),
        "the STEP part is on screen, not black"
    );
    assert!(report.never_silent(), "every part is diagnosed");
    assert_eq!(report.part_count(), 1);
    assert_eq!(report.fidelity_counts().exact_brep, 1, "exact B-rep");
    assert_eq!(report.fidelity_counts().failed, 0, "0 silent failures");

    // Determinism: same file → identical signature ×3.
    let s = signature(&report);
    assert_eq!(s, signature(&read_cad(CUBE_STEP.as_bytes()).unwrap()));
    assert_eq!(s, signature(&read_cad(CUBE_STEP.as_bytes()).unwrap()));

    // Lands as ONE undoable transaction of queryable entities.
    assert_eq!(engine.entity_count(), 0);
    let landing = import_cad(&mut engine, &scene, &mut store, CUBE_STEP.as_bytes()).expect("land");
    assert_eq!(engine.entity_count(), 1, "one part entity");
    assert_eq!(
        engine.get_field(landing.entities[0], CAD_PART, "fidelity"),
        Some(FieldValue::Str("exact-brep".into()))
    );
    assert!(engine
        .get_field(landing.entities[0], "MeshRenderer", MESH_FIELD)
        .is_some());
    assert!(engine.undo(), "one Ctrl-Z peels the import");
    assert_eq!(engine.entity_count(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────────────────────────────────
// LEG 2 — the REAL CATIA 3DXML (local-only gate): the exact file that black-screened Unreal, head-to-head
// ─────────────────────────────────────────────────────────────────────────────────────────────────────────

/// The bar file, if present on disk (owner's proprietary assembly; not in CI). Env override + default path.
fn bar_file() -> Option<Vec<u8>> {
    let path = std::env::var("MTK_3DXML_BAR_FILE").unwrap_or_else(|_| {
        "X:/Work/Metrocalk/Games Projects/Unreal/Skid Weld Line A.1/Skid Weld Line A.1.3dxml".into()
    });
    std::fs::read(&path).ok()
}

#[test]
#[allow(clippy::too_many_lines)] // one end-to-end proof over the real bar file — cohesive, reads top-to-bottom
fn real_catia_3dxml_beats_the_documented_unreal_failure() {
    let Some(bytes) = bar_file() else {
        eprintln!(
            "[skip] real-3DXML leg: bar file not on disk (documented local-only gate; set \
             MTK_3DXML_BAR_FILE). The synthetic-3DXML failure modes are gated in CI (interchange threedxml)."
        );
        return;
    };

    // The exact file Unreal imported 1 part of, then black-screened.
    let report = read_cad(&bytes).expect("the 3DXML imports (never a black screen)");

    // NEVER-EMPTY + NEVER-SILENT on the real file — the go/no-go.
    assert!(report.never_empty(), "every part placed (never black)");
    assert!(report.never_silent(), "every part diagnosed (never silent)");

    // The full factory cell: far more than Unreal's 1 part; a forest of products; all deduped for instancing.
    assert!(
        report.part_count() > 500,
        "placed {} parts (Unreal placed 1)",
        report.part_count()
    );
    assert!(
        report.products > 1,
        "a forest of products ({})",
        report.products
    );
    assert!(
        report.unique_geometry_count() < report.part_count(),
        "dedup: {} unique geometries serve {} placements (instancing)",
        report.unique_geometry_count(),
        report.part_count()
    );
    // Nothing failed/dropped silently; the proprietary CATIA geometry is the diagnosed licensed-kernel seam.
    let c = report.fidelity_counts();
    assert_eq!(c.failed, 0, "0 silent failures");
    assert!(c.proxy > 0, "proprietary parts placed as diagnosed proxies");
    assert!(
        report.parts.iter().all(|p| !p.reason.trim().is_empty()),
        "every part has a plain-language diagnosis"
    );
    assert!(
        report
            .parts
            .iter()
            .filter(|p| p.fidelity == metrocalk_interchange::PartFidelity::Proxy)
            .all(|p| p
                .fix
                .as_deref()
                .is_some_and(|f| f.contains("STEP AP242") || f.contains("kernel"))),
        "every proprietary part offers a one-click fix path"
    );

    // Real placement (not the assembly-origin collapse): the parts span a real mm extent (a factory cell).
    let spread = {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for p in &report.parts {
            let t = metrocalk_interchange::translation_of(&p.transform);
            for k in 0..3 {
                lo[k] = lo[k].min(t[k]);
                hi[k] = hi[k].max(t[k]);
            }
        }
        (hi[0] - lo[0]).max(hi[1] - lo[1]).max(hi[2] - lo[2])
    };
    assert!(
        spread > 1000.0,
        "parts spread over a real mm extent ({spread:.0} mm), not the origin"
    );

    // Determinism: same file → bit-identical signature ×3.
    let s = signature(&report);
    assert_eq!(
        s,
        signature(&read_cad(&bytes).unwrap()),
        "deterministic run 2"
    );
    assert_eq!(
        s,
        signature(&read_cad(&bytes).unwrap()),
        "deterministic run 3"
    );

    // Substrate-native at scale: the whole assembly lands as ONE undoable transaction, then one Ctrl-Z peels
    // it. (The proprietary geometry is proxies; the ECS mapping + dedup + queryable report is the win.)
    let (mut engine, scene) = engine();
    let mut store = AssetStore::new();
    let landing = import_cad(&mut engine, &scene, &mut store, &bytes).expect("land the real file");
    assert_eq!(
        engine.entity_count(),
        report.part_count(),
        "every part is a queryable entity"
    );
    assert!(
        landing.unique_meshes <= report.unique_geometry_count() + 1,
        "meshes deduped (proxy shared)"
    );
    // The report is ECS-queryable — a proxy part names its fidelity.
    let a_proxy = landing
        .report
        .parts
        .iter()
        .position(|p| p.fidelity == metrocalk_interchange::PartFidelity::Proxy)
        .expect("a proxy part");
    assert_eq!(
        engine.get_field(landing.entities[a_proxy], CAD_PART, "fidelity"),
        Some(FieldValue::Str("proxy".into()))
    );
    assert!(
        engine.undo(),
        "one Ctrl-Z peels the whole factory-cell import"
    );
    assert_eq!(engine.entity_count(), 0);

    eprintln!(
        "[GO] real CATIA 3DXML: {} part placements · {} unique geometries · {} products — never-empty, \
         never-silent, deterministic. Unreal: 1 part, then black screen.",
        report.part_count(),
        report.unique_geometry_count(),
        report.products,
    );
}
