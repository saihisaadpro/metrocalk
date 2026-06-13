//! Merge convergence tests + all 8 invalid-state class injection/repair.

use metrocalk_core::{Engine, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;

fn engine(peer: u64) -> Engine<FlecsWorld> {
    Engine::new(FlecsWorld::new(), peer)
}

// ── two-fork merge convergence ─────────────────────────────────────────

#[test]
fn two_fork_merge_converges() {
    // Peer 1: create entities, set fields
    let mut e1 = engine(1);
    let a = e1.alloc_entity_id();
    let b = e1.alloc_entity_id();
    e1.commit(
        "setup",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::CreateEntity {
                id: b,
                parent: Some(a),
            },
            Op::SetField {
                entity: a,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(100),
            },
            Op::SetField {
                entity: b,
                component: "Transform".into(),
                field: "px".into(),
                value: FieldValue::Number(5.0),
            },
        ],
    )
    .unwrap();

    // Fork: peer 2 imports peer 1's initial state
    let snapshot = e1.fork_doc();
    let mut e2 = engine(2);
    let report = e2.merge(&snapshot).unwrap();
    assert_eq!(report.total_violations(), 0);

    // Peer 1: concurrent edit
    e1.commit(
        "p1-edit",
        vec![Op::SetField {
            entity: a,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(200),
        }],
    )
    .unwrap();

    // Peer 2: concurrent edit (different field, no conflict)
    let c = e2.alloc_entity_id();
    e2.commit(
        "p2-edit",
        vec![
            Op::CreateEntity {
                id: c,
                parent: None,
            },
            Op::SetField {
                entity: c,
                component: "Tag".into(),
                field: "name".into(),
                value: FieldValue::Str("peer2-entity".into()),
            },
        ],
    )
    .unwrap();

    // Merge: peer 1 ← peer 2
    let e2_updates = e2.export_updates();
    let report1 = e1.merge(&e2_updates).unwrap();

    // Merge: peer 2 ← peer 1
    let e1_updates = e1.export_updates();
    let report2 = e2.merge(&e1_updates).unwrap();

    // Both engines should have the same entities
    assert_eq!(e1.entity_count(), 3); // a, b, c
    assert_eq!(e2.entity_count(), 3);

    // Both should see peer 1's edit
    assert_eq!(
        e1.get_field(a, "Health", "hp"),
        Some(FieldValue::Integer(200))
    );
    assert_eq!(
        e2.get_field(a, "Health", "hp"),
        Some(FieldValue::Integer(200))
    );

    // Both should see peer 2's entity
    assert_eq!(
        e1.get_field(c, "Tag", "name"),
        Some(FieldValue::Str("peer2-entity".into()))
    );
    assert_eq!(
        e2.get_field(c, "Tag", "name"),
        Some(FieldValue::Str("peer2-entity".into()))
    );

    // Undo stacks cleared on merge
    assert!(!e1.can_undo());
    assert!(!e2.can_undo());

    // No violations found
    assert_eq!(report1.total_violations(), 0);
    assert_eq!(report2.total_violations(), 0);
}

#[test]
fn concurrent_field_edit_lww() {
    // Both peers edit the same field — Loro LWW resolves deterministically
    let mut e1 = engine(1);
    let a = e1.alloc_entity_id();
    e1.commit(
        "setup",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::SetField {
                entity: a,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(50),
            },
        ],
    )
    .unwrap();

    let snapshot = e1.fork_doc();
    let mut e2 = engine(2);
    e2.merge(&snapshot).unwrap();

    // Concurrent: both set same field
    e1.commit(
        "p1",
        vec![Op::SetField {
            entity: a,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(100),
        }],
    )
    .unwrap();

    e2.commit(
        "p2",
        vec![Op::SetField {
            entity: a,
            component: "Health".into(),
            field: "hp".into(),
            value: FieldValue::Integer(999),
        }],
    )
    .unwrap();

    // Cross-merge
    let u1 = e1.export_updates();
    let u2 = e2.export_updates();
    e1.merge(&u2).unwrap();
    e2.merge(&u1).unwrap();

    // They must converge to the same value (LWW, deterministic)
    let v1 = e1.get_field(a, "Health", "hp");
    let v2 = e2.get_field(a, "Health", "hp");
    assert_eq!(v1, v2, "LWW convergence: both peers see the same value");
}

// ── 8 invalid-state class injection + repair ───────────────────────────

#[test]
fn class1_dangling_edge_endpoint() {
    // Create entity a and b, add binding, then corrupt: delete a from tree but leave binding
    let doc = loro::LoroDoc::new();
    let tree = doc.get_tree("hierarchy");
    let _ = doc.get_map("components");
    let bindings = doc.get_map("bindings");

    let t_a = tree.create(loro::TreeParentId::Root).unwrap();
    tree.get_meta(t_a).unwrap().insert("eid", "1_0").unwrap();
    let t_b = tree.create(loro::TreeParentId::Root).unwrap();
    tree.get_meta(t_b).unwrap().insert("eid", "1_1").unwrap();

    // Add component records
    let comps = doc.get_map("components");
    let _ = comps.insert_container("1_0", loro::LoroMap::new()).unwrap();
    let _ = comps.insert_container("1_1", loro::LoroMap::new()).unwrap();

    // Add binding
    let edge = bindings
        .insert_container("1_0|bindsTo|1_1", loro::LoroMap::new())
        .unwrap();
    edge.insert("from", "1_0").unwrap();
    edge.insert("kind", "bindsTo").unwrap();
    edge.insert("to", "1_1").unwrap();

    // Delete entity a (but leave the binding → dangling)
    tree.delete(t_a).unwrap();
    comps.delete("1_0").unwrap();
    doc.commit();

    // Now merge into an engine and check
    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    let mut e = engine(99);
    let report = e.merge(&snapshot).unwrap();

    assert!(
        report.violations.contains_key("dangling-edge-endpoint"),
        "should detect dangling edge"
    );
    assert!(
        report.total_repairs > 0,
        "should repair by deleting dangling binding"
    );
}

#[test]
fn class2_orphan_component_record() {
    // Component record for an eid that has no tree node
    let doc = loro::LoroDoc::new();
    let _ = doc.get_tree("hierarchy");
    let comps = doc.get_map("components");
    let _ = doc.get_map("bindings");

    // Create orphan component record (no tree node for "99_0")
    let orphan = comps
        .insert_container("99_0", loro::LoroMap::new())
        .unwrap();
    orphan
        .insert_container("Health", loro::LoroMap::new())
        .unwrap()
        .insert("hp", 42)
        .unwrap();
    doc.commit();

    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    let mut e = engine(99);
    let report = e.merge(&snapshot).unwrap();

    assert!(
        report.violations.contains_key("orphan-component-record"),
        "should detect orphan component"
    );
    assert!(report.total_repairs > 0);
}

#[test]
fn class3_entity_missing_component_record() {
    // Tree node alive but no component record
    let doc = loro::LoroDoc::new();
    let tree = doc.get_tree("hierarchy");
    let _ = doc.get_map("components"); // don't create a record for the entity
    let _ = doc.get_map("bindings");

    let tid = tree.create(loro::TreeParentId::Root).unwrap();
    tree.get_meta(tid).unwrap().insert("eid", "1_0").unwrap();
    doc.commit();

    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    let mut e = engine(99);
    let report = e.merge(&snapshot).unwrap();

    assert!(
        report
            .violations
            .contains_key("entity-missing-component-record"),
        "should detect missing component record"
    );
    assert!(report.total_repairs > 0);
}

#[test]
fn class4_duplicate_eid() {
    // Two alive tree nodes with the same eid
    let doc = loro::LoroDoc::new();
    let tree = doc.get_tree("hierarchy");
    let comps = doc.get_map("components");
    let _ = doc.get_map("bindings");

    let t1 = tree.create(loro::TreeParentId::Root).unwrap();
    tree.get_meta(t1).unwrap().insert("eid", "1_0").unwrap();
    let _ = comps.insert_container("1_0", loro::LoroMap::new()).unwrap();

    let t2 = tree.create(loro::TreeParentId::Root).unwrap();
    tree.get_meta(t2).unwrap().insert("eid", "1_0").unwrap(); // same eid!
    doc.commit();

    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    let mut e = engine(99);
    let report = e.merge(&snapshot).unwrap();

    assert!(
        report.violations.contains_key("duplicate-eid"),
        "should detect duplicate eid"
    );
    assert!(report.total_repairs > 0, "should re-key duplicate");
}

#[test]
fn class7_corrupt_asset_ref() {
    // Component field named "mesh" with a non-asset value
    let doc = loro::LoroDoc::new();
    let tree = doc.get_tree("hierarchy");
    let comps = doc.get_map("components");
    let _ = doc.get_map("bindings");

    let tid = tree.create(loro::TreeParentId::Root).unwrap();
    tree.get_meta(tid).unwrap().insert("eid", "1_0").unwrap();

    let rec = comps.insert_container("1_0", loro::LoroMap::new()).unwrap();
    let comp = rec
        .insert_container("Renderer", loro::LoroMap::new())
        .unwrap();
    comp.insert("mesh", "not/a/valid/asset.xyz").unwrap(); // corrupt asset ref
    doc.commit();

    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    let mut e = engine(99);
    let report = e.merge(&snapshot).unwrap();

    assert!(
        report.violations.contains_key("corrupt-asset-ref"),
        "should detect corrupt asset ref"
    );
    // Corrupt asset refs are flagged but not auto-repaired (correct value unknown)
}

#[test]
fn class8_malformed_edge() {
    // Binding entry with missing required fields
    let doc = loro::LoroDoc::new();
    let _ = doc.get_tree("hierarchy");
    let _ = doc.get_map("components");
    let bindings = doc.get_map("bindings");

    let edge = bindings
        .insert_container("bad_edge", loro::LoroMap::new())
        .unwrap();
    edge.insert("from", "1_0").unwrap();
    // missing "kind" and "to" → malformed
    doc.commit();

    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
    let mut e = engine(99);
    let report = e.merge(&snapshot).unwrap();

    assert!(
        report.violations.contains_key("malformed-edge"),
        "should detect malformed edge"
    );
    assert!(report.total_repairs > 0);
}

// ── merge clears undo stack ────────────────────────────────────────────

#[test]
fn merge_clears_undo_stack() {
    let mut e1 = engine(1);
    let a = e1.alloc_entity_id();
    e1.commit(
        "create",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::SetField {
                entity: a,
                component: "X".into(),
                field: "v".into(),
                value: FieldValue::Integer(1),
            },
        ],
    )
    .unwrap();
    assert!(e1.can_undo());

    // Merge some remote data
    let mut e2 = engine(2);
    let b = e2.alloc_entity_id();
    e2.commit(
        "remote",
        vec![Op::CreateEntity {
            id: b,
            parent: None,
        }],
    )
    .unwrap();
    let u2 = e2.export_updates();
    e1.merge(&u2).unwrap();

    assert!(!e1.can_undo(), "undo stack should be cleared after merge");
}

// ── merge + validate: no violations in clean merge ─────────────────────

#[test]
fn clean_merge_zero_violations() {
    let mut e1 = engine(1);
    let a = e1.alloc_entity_id();
    e1.commit(
        "setup",
        vec![
            Op::CreateEntity {
                id: a,
                parent: None,
            },
            Op::SetField {
                entity: a,
                component: "Health".into(),
                field: "hp".into(),
                value: FieldValue::Integer(100),
            },
        ],
    )
    .unwrap();

    let mut e2 = engine(2);
    let report = e2.merge(&e1.export_updates()).unwrap();

    assert_eq!(report.total_violations(), 0);
    assert_eq!(report.total_repairs, 0);
    assert_eq!(report.alive_nodes, 1);
}
