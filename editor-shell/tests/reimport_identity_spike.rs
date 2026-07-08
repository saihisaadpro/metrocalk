//! M15.10 (ADR-080) — THE SPIKE (deliverable #1): the measured go/no-go for **persistent re-import identity**.
//!
//! The claim: a designer imports an assembly, assigns overrides (a material, a physics collider, the M15.9
//! joint animation binding) to several parts, then **re-imports an EDITED version** — and KEEPS everything on
//! the parts that survived, **re-bound automatically** onto the geometrically-matched part, with added/removed
//! parts diffed, the **deleted part's overrides preserved + flagged (never silently lost)**, and a
//! **low-confidence match surfaced for adjudication, never auto-applied**.
//!
//! The gate (a wrong match that silently corrupts an override, or any override silently lost, is a FAIL):
//! - **overrides survive on the matched part** — assert the re-bound op targets the MATCHED new entity;
//! - **the added part is new + empty** — no override leaks onto it;
//! - **the deleted part's overrides are FLAGGED** (an orphan), not dropped;
//! - **prefer-miss-over-wrong** — a heavily-edited part MISSES rather than wrong-binding;
//! - **a low-confidence match is HELD for adjudication**, not auto-applied to a load-bearing override.
//!
//! Real `Engine` + real ops (the uniquely-enabled op-stream re-bind), headless, CI-gated (no dark test).

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_csg::TriMesh;
use metrocalk_ecs::FlecsWorld;
use metrocalk_editor_shell::{
    capture_overrides, match_scene_against, plan_rebind, set_reimport_id_ops, Adjudication,
    REIMPORT_ID,
};
use metrocalk_interchange::{
    fingerprint, MatchKind, PartFingerprint, PartIdentity, PartMatch, ReimportPlan,
};
use std::collections::BTreeMap;

fn engine() -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), 1)
}

/// An axis-aligned box centred at the origin → a closed 12-triangle mesh (the spike's synthetic geometry).
fn box_mesh(hx: f64, hy: f64, hz: f64) -> TriMesh {
    let p = [
        [-hx, -hy, -hz],
        [hx, -hy, -hz],
        [hx, hy, -hz],
        [-hx, hy, -hz],
        [-hx, -hy, hz],
        [hx, -hy, hz],
        [hx, hy, hz],
        [-hx, hy, hz],
    ];
    let t = [
        [0u32, 3, 2],
        [0, 2, 1],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [2, 3, 7],
        [2, 7, 6],
        [1, 2, 6],
        [1, 6, 5],
        [0, 4, 7],
        [0, 7, 3],
    ];
    TriMesh {
        positions: p.to_vec(),
        triangles: t.to_vec(),
    }
}

/// Create a CAD part entity carrying its `ReimportId` (the stable geometric identity) — as the importer would.
fn place_part(
    e: &mut Engine<FlecsWorld>,
    pid: u64,
    reference: &str,
    mesh: &TriMesh,
    centroid: [f64; 3],
) -> EntityId {
    let ent = e.alloc_entity_id();
    let fp = fingerprint(mesh, None);
    let mesh_hash = metrocalk_interchange::mesh_hash(mesh);
    let mut ops = vec![Op::CreateEntity {
        id: ent,
        parent: None,
    }];
    ops.push(Op::SetField {
        entity: ent,
        component: "MeshRenderer".into(),
        field: "mesh".into(),
        value: FieldValue::Str(format!("mtkcad:{mesh_hash:016x}")),
    });
    ops.extend(set_reimport_id_ops(
        ent,
        pid,
        reference,
        Some(mesh_hash),
        centroid,
        &fp,
    ));
    e.commit("import-part", ops).expect("import part");
    ent
}

/// Assign a material override to an entity (a user edit — the thing that must survive re-import).
fn set_material(e: &mut Engine<FlecsWorld>, ent: EntityId, preset: &str) {
    e.commit(
        "set-material",
        vec![Op::SetField {
            entity: ent,
            component: "MeshRenderer".into(),
            field: "material".into(),
            value: FieldValue::Str(preset.into()),
        }],
    )
    .expect("set material");
}

/// Author a joint (the M15.9 animation binding) on an entity — the override the prompt names explicitly.
fn set_joint(e: &mut Engine<FlecsWorld>, ent: EntityId) {
    let ops = metrocalk_editor_shell::set_joint_ops(
        ent,
        true,
        [0.0, 0.0, 1.0],
        [0.04, 0.0, 0.0],
        (-10.0, 10.0),
        "manual",
    );
    e.commit("set-joint", ops).expect("set joint");
}

fn material_of(e: &Engine<FlecsWorld>, ent: EntityId) -> Option<String> {
    e.components_of(ent)
        .get("MeshRenderer")
        .and_then(|m| m.get("material"))
        .and_then(|v| match v {
            FieldValue::Str(s) => Some(s.clone()),
            _ => None,
        })
}

#[test]
fn reimport_keeps_my_work_matched_survives_deleted_flagged_added_bare() {
    let mut e = engine();

    // ── First import: a Bracket (cube) and a Plate (flat box). ───────────────────────────────────────────
    let bracket = place_part(
        &mut e,
        1,
        "bracket",
        &box_mesh(1.0, 1.0, 1.0),
        [0.0, 0.0, 0.0],
    );
    let plate = place_part(
        &mut e,
        2,
        "plate",
        &box_mesh(2.0, 2.0, 0.15),
        [5.0, 0.0, 0.0],
    );

    // The user does work: chrome + a joint on the bracket; gold on the plate.
    set_material(&mut e, bracket, "chrome");
    set_joint(&mut e, bracket);
    set_material(&mut e, plate, "gold");

    let old_entities: BTreeMap<u64, EntityId> = [(1u64, bracket), (2, plate)].into();
    let names: BTreeMap<u64, String> = [(1u64, "Bracket".into()), (2, "Plate".into())].into();

    // ── Re-import an EDITED file: the bracket is FILLETED (a small shrink, same place) → matches; a NEW
    // gusset is added; the PLATE IS DELETED. Create the new entities as the importer would. ───────────────
    let bracket_edited = box_mesh(0.96, 0.96, 0.96); // the fillet
    let gusset = box_mesh(0.4, 0.1, 0.7); // a brand-new part
    let ent_bracket2 = place_part(&mut e, 10, "bracket", &bracket_edited, [0.0, 0.0, 0.0]);
    let ent_gusset = place_part(&mut e, 11, "gusset", &gusset, [9.0, 0.0, 0.0]);
    let new_entities: BTreeMap<u64, EntityId> = [(10u64, ent_bracket2), (11, ent_gusset)].into();
    let new_ids: Vec<PartIdentity> = [ent_bracket2, ent_gusset]
        .iter()
        .map(|&ent| metrocalk_editor_shell::reimport_identity_of(&e, ent).unwrap())
        .collect();

    // ── Match the LIVE scene (the old parts) against the re-import. ───────────────────────────────────────
    let plan = match_scene_against(&e, &old_entities, &new_ids);

    let bracket_match = plan.matches.iter().find(|m| m.old_id == 1).unwrap();
    assert_eq!(
        bracket_match.kind,
        MatchKind::Strong,
        "the filleted bracket matches: {bracket_match:?}"
    );
    assert_eq!(
        bracket_match.new_id,
        Some(10),
        "matched to the edited bracket, not the gusset"
    );
    let plate_match = plan.matches.iter().find(|m| m.old_id == 2).unwrap();
    assert_eq!(
        plate_match.kind,
        MatchKind::Miss,
        "the deleted plate misses (prefer-miss): {plate_match:?}"
    );
    assert!(
        plan.added.contains(&11),
        "the gusset is a new part: {:?}",
        plan.added
    );

    // ── Plan + commit the override re-bind. ──────────────────────────────────────────────────────────────
    let outcome = plan_rebind(&e, &old_entities, &new_entities, &plan, &names);
    assert_eq!(outcome.rebound, 1, "one part's overrides auto-re-bound");
    // The deleted plate's gold material is PRESERVED + FLAGGED, never lost.
    let orphan = outcome
        .orphans
        .iter()
        .find(|o| o.old_id == 2)
        .expect("plate override flagged");
    assert_eq!(orphan.name, "Plate");
    assert_eq!(
        orphan.overrides.material.as_deref(),
        Some("gold"),
        "the flagged override is preserved"
    );

    e.commit("reimport-rebind", outcome.ops)
        .expect("commit rebind");

    // ── THE GATE: the bracket's work survived onto the MATCHED new entity. ────────────────────────────────
    assert_eq!(
        material_of(&e, ent_bracket2).as_deref(),
        Some("chrome"),
        "material re-bound onto the matched part"
    );
    assert!(
        metrocalk_editor_shell::joint_of(&e, ent_bracket2).is_some(),
        "the M15.9 joint animation binding re-bound onto the matched part"
    );
    // The added gusset is BARE — nothing leaked onto it.
    assert_eq!(material_of(&e, ent_gusset), None, "the new part is empty");
    assert!(
        metrocalk_editor_shell::joint_of(&e, ent_gusset).is_none(),
        "no joint leaked onto the new part"
    );
    // The old bracket entity's ReimportId still exists (the scene isn't corrupted by the plan step).
    assert!(e.components_of(bracket).contains_key(REIMPORT_ID));
}

#[test]
fn a_low_confidence_match_is_held_for_adjudication_never_auto_applied() {
    // The honest fallback: a middle-confidence match must NOT auto-re-bind a load-bearing override — it is
    // surfaced for the user to confirm/reject. Build a plan with a LowConfidence match directly (decoupled
    // from confidence tuning) and prove plan_rebind HOLDS it (adjudicate), emitting NO re-bind op.
    let mut e = engine();
    let old_ent = place_part(&mut e, 1, "part", &box_mesh(1.0, 1.0, 1.0), [0.0, 0.0, 0.0]);
    set_material(&mut e, old_ent, "chrome"); // a load-bearing override
    let new_ent = place_part(&mut e, 5, "part", &box_mesh(0.7, 1.3, 1.0), [0.1, 0.0, 0.0]);

    let old_entities: BTreeMap<u64, EntityId> = [(1u64, old_ent)].into();
    let new_entities: BTreeMap<u64, EntityId> = [(5u64, new_ent)].into();
    let plan = ReimportPlan {
        matches: vec![PartMatch {
            old_id: 1,
            new_id: Some(5),
            confidence: 0.66, // in the [LOW, STRONG) band
            kind: MatchKind::LowConfidence,
        }],
        added: vec![],
    };
    let outcome = plan_rebind(&e, &old_entities, &new_entities, &plan, &BTreeMap::new());

    assert!(
        outcome.ops.is_empty(),
        "a low-confidence match auto-binds NOTHING"
    );
    assert_eq!(outcome.rebound, 0);
    let adj: &Adjudication = outcome
        .adjudicate
        .first()
        .expect("surfaced for adjudication");
    assert_eq!((adj.old_id, adj.new_id), (1, 5));
    assert_eq!(
        adj.overrides.material.as_deref(),
        Some("chrome"),
        "the override is held, not applied"
    );
    // The new entity did NOT silently receive the override.
    assert_eq!(
        material_of(&e, new_ent),
        None,
        "no silent wrong-bind onto an unconfirmed match"
    );
}

#[test]
fn reimport_over_scene_rebinds_deactivates_old_and_reports_every_part() {
    // D1 (ADR-080 convergence): the LIVE re-import orchestration end-to-end (headless). Import A (bracket +
    // plate with overrides), then re-import an edited B over the live scene: reimport_over_scene must produce
    // ONE undoable batch that re-binds the matched overrides onto the new entities AND deactivates the old
    // ones, plus a never-silent per-part diff. Commit it and assert the live result off structured signals.
    let mut e = engine();
    let bracket = place_part(
        &mut e,
        1,
        "bracket",
        &box_mesh(1.0, 1.0, 1.0),
        [0.0, 0.0, 0.0],
    );
    let plate = place_part(
        &mut e,
        2,
        "plate",
        &box_mesh(2.0, 2.0, 0.15),
        [5.0, 0.0, 0.0],
    );
    set_material(&mut e, bracket, "chrome");
    set_joint(&mut e, bracket);
    set_material(&mut e, plate, "gold");

    let old_entities: BTreeMap<u64, EntityId> = [(1u64, bracket), (2, plate)].into();
    let old_names: BTreeMap<u64, String> = [(1u64, "Bracket".into()), (2, "Plate".into())].into();

    // Re-import B: bracket filleted (matches), gusset added, plate deleted.
    let ent_b2 = place_part(
        &mut e,
        10,
        "bracket",
        &box_mesh(0.96, 0.96, 0.96),
        [0.0, 0.0, 0.0],
    );
    let ent_g = place_part(
        &mut e,
        11,
        "gusset",
        &box_mesh(0.4, 0.1, 0.7),
        [9.0, 0.0, 0.0],
    );
    let new_entities: BTreeMap<u64, EntityId> = [(10u64, ent_b2), (11, ent_g)].into();
    let new_ids: Vec<PartIdentity> = [ent_b2, ent_g]
        .iter()
        .map(|&ent| metrocalk_editor_shell::reimport_identity_of(&e, ent).unwrap())
        .collect();

    let session = metrocalk_editor_shell::reimport_over_scene(
        &e,
        &old_entities,
        &old_names,
        &new_ids,
        &new_entities,
    );

    // The report accounts for EVERY part (never-silent): bracket matched, plate removed, gusset added.
    let by_old = |oid: u64| session.report.iter().find(|r| r.old_id == oid).cloned();
    assert_eq!(
        by_old(1).unwrap().kind,
        "matched",
        "the bracket is reported matched"
    );
    assert_eq!(by_old(1).unwrap().new_id, Some(10));
    assert!(
        by_old(1).unwrap().had_overrides,
        "the bracket's overrides are noted"
    );
    assert_eq!(
        by_old(2).unwrap().kind,
        "removed",
        "the plate is reported removed"
    );
    assert!(
        session
            .report
            .iter()
            .any(|r| r.kind == "added" && r.new_id == Some(11)),
        "the gusset is reported added"
    );
    assert_eq!(session.rebound, 1);
    assert_eq!(
        session
            .orphans
            .iter()
            .find(|o| o.old_id == 2)
            .unwrap()
            .overrides
            .material
            .as_deref(),
        Some("gold")
    );

    // Apply the whole re-import as ONE undoable commit.
    e.commit("reimport", session.commit_ops)
        .expect("commit reimport");

    // The bracket's work survived onto the NEW entity; the old entities are deactivated (deactivate-not-delete).
    assert_eq!(
        material_of(&e, ent_b2).as_deref(),
        Some("chrome"),
        "material re-bound onto the new bracket"
    );
    assert!(
        metrocalk_editor_shell::joint_of(&e, ent_b2).is_some(),
        "joint re-bound onto the new bracket"
    );
    assert!(
        !e.is_active(bracket),
        "the previous bracket entity is deactivated"
    );
    assert!(
        !e.is_active(plate),
        "the previous plate entity is deactivated"
    );
    assert!(
        e.is_active(ent_b2) && e.is_active(ent_g),
        "the new entities stay active"
    );
}

#[test]
fn capture_overrides_reads_material_and_joint_but_not_import_authored_fields() {
    // capture_overrides must grab the user's work (material, joint) and NOT the import-authored mesh handle
    // (which the new import re-authors) — else the re-bind would clobber the new geometry with the old.
    let mut e = engine();
    let ent = place_part(&mut e, 1, "p", &box_mesh(1.0, 1.0, 1.0), [0.0, 0.0, 0.0]);
    set_material(&mut e, ent, "gold");
    set_joint(&mut e, ent);
    let ov = capture_overrides(&e, ent);
    assert_eq!(ov.material.as_deref(), Some("gold"));
    assert!(ov.components.contains_key("Joint"), "the joint is captured");
    // The mesh handle is import-authored — it must NOT be in the re-bind payload.
    assert!(
        !ov.components.values().any(|f| f.contains_key("mesh")),
        "the import-authored mesh handle is not re-bound"
    );
    let _ = PartFingerprint::degenerate(); // (type is used)
}
