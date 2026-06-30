//! M15.1 (ADR-071) — **BOM-as-a-query**: the bill-of-materials is a relational query over the live product
//! structure, **always in sync by construction** — not an Excel export.
//!
//! A CAD BOM rolls up an assembly into its distinct parts, their quantities, and where each is used. In
//! Metrocalk the assembly tree **is** the entity hierarchy ([`Engine::parent_of`]/[`Engine::children_of`]),
//! a **part number** is the content-addressed mesh handle (identical geometry → identical handle → the same
//! part — the content-addressed-store property, ADR-014), and where-used is the inverse of the hierarchy
//! edge. So the BOM is a pure **read** over the engine — recomputed fresh on every call, never persisted —
//! which is exactly the always-in-sync property file-CAD's exported BOMs lack (this is the M14.2 C6
//! relational-projection pattern, ADR-058, applied to the product structure).
//!
//! It is a read/render projection (invariant 1) with **zero determinism impact** — it never commits, never
//! authors to Loro. Off the per-frame hot path (invariant 4): a roll-up is O(entities + edges), run on the
//! discrete projection path, never the play/sim tick.

use crate::capscene::MESH_FIELD;
use metrocalk_core::{Engine, EntityId, FieldValue};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// One bill-of-materials line: a distinct part, its total quantity, and the assemblies it is used in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BomLine {
    /// The **part number** — the content-addressed mesh handle (the natural CAD part identity: identical
    /// geometry is the same part by construction).
    pub part: String,
    /// A human label for the part (the first instance's display name, else the part number).
    pub label: String,
    /// How many instances of this part the product contains (the BOM quantity).
    pub quantity: usize,
    /// The assemblies this part is used in, by display label (the where-used query) — sorted, de-duplicated.
    pub where_used: Vec<String>,
}

/// A bill-of-materials: distinct parts with quantities + where-used, **always in sync** (a live query, not
/// a stored table). Deterministic: lines are sorted by part number.
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bom {
    /// The BOM lines, sorted by part number (deterministic).
    pub lines: Vec<BomLine>,
    /// Total part instances across the product (the sum of all line quantities).
    pub total_instances: usize,
    /// The number of distinct parts (`lines.len()`).
    pub distinct_parts: usize,
}

/// The entity's display label — its user-set `__meta__.name` (the M10.6 rename) if present, else its loro
/// key. (Mirrors `bridge::entity_label`; kept local so the BOM query doesn't widen that module's surface.)
fn label_of<W: World>(engine: &Engine<W>, id: EntityId) -> String {
    engine
        .get_field(id, "__meta__", "name")
        .and_then(|v| match v {
            FieldValue::Str(s) => Some(s),
            _ => None,
        })
        .unwrap_or_else(|| id.to_loro_key())
}

/// The entity's **part number** — the content-addressed mesh handle on any of its components (the geometry
/// IS the part identity). `None` for an entity carrying no geometry (a group / assembly node / light).
fn part_of<W: World>(engine: &Engine<W>, id: EntityId) -> Option<String> {
    engine
        .components_of(id)
        .values()
        .find_map(|fields| match fields.get(MESH_FIELD) {
            Some(FieldValue::Str(handle)) if !handle.is_empty() => Some(handle.clone()),
            _ => None,
        })
}

/// Roll the live product structure up into a bill-of-materials — a pure relational query over the engine,
/// recomputed every call so it is **always in sync** (an edit is reflected with no re-export step). Each
/// geometry-bearing entity is one part instance; its part number is the content-addressed mesh handle, and
/// its where-used is its parent assembly's label (`(top level)` for a root part).
#[must_use]
pub fn rollup<W: World>(engine: &Engine<W>) -> Bom {
    // part number -> (quantity, label-of-first-instance, set of where-used assembly labels)
    let mut parts: BTreeMap<String, (usize, String, BTreeSet<String>)> = BTreeMap::new();

    // Sort the ids so the roll-up (and any hash of it) is deterministic — entity_ids() order is not
    // guaranteed (pipeline.rs), and a content-addressed revision may hash a BOM digest.
    let mut ids = engine.entity_ids();
    ids.sort_by_key(EntityId::to_loro_key);

    let mut total_instances = 0usize;
    for id in ids {
        let Some(part) = part_of(engine, id) else {
            continue; // an assembly/group node, not a BOM line item itself
        };
        total_instances += 1;
        let where_used = match engine.parent_of(id) {
            Some(parent) => label_of(engine, parent),
            None => "(top level)".to_string(),
        };
        let entry = parts
            .entry(part)
            .or_insert_with(|| (0, label_of(engine, id), BTreeSet::new()));
        entry.0 += 1;
        entry.2.insert(where_used);
    }

    let lines: Vec<BomLine> = parts
        .into_iter()
        .map(|(part, (quantity, label, where_used))| BomLine {
            part,
            label,
            quantity,
            where_used: where_used.into_iter().collect(),
        })
        .collect();

    Bom {
        distinct_parts: lines.len(),
        total_instances,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_core::Op;
    use metrocalk_ecs::FlecsWorld;

    fn engine() -> Engine<FlecsWorld> {
        Engine::new(FlecsWorld::new(), 1)
    }

    /// Place a geometry-bearing part entity under an optional parent assembly, carrying a content-addressed
    /// mesh handle and an optional display name (mirrors `place_mesh`'s MeshRenderer.mesh).
    fn place_part(
        e: &mut Engine<FlecsWorld>,
        handle: &str,
        parent: Option<EntityId>,
        name: Option<&str>,
    ) -> EntityId {
        let id = e.alloc_entity_id();
        let mut ops = vec![
            Op::CreateEntity { id, parent },
            Op::SetField {
                entity: id,
                component: "MeshRenderer".into(),
                field: MESH_FIELD.into(),
                value: FieldValue::Str(handle.into()),
            },
        ];
        if let Some(n) = name {
            ops.push(Op::SetField {
                entity: id,
                component: "__meta__".into(),
                field: "name".into(),
                value: FieldValue::Str(n.into()),
            });
        }
        e.commit("place-part", ops).expect("place a part");
        id
    }

    fn assembly(e: &mut Engine<FlecsWorld>, name: &str) -> EntityId {
        let id = e.alloc_entity_id();
        e.commit(
            "assembly",
            vec![
                Op::CreateEntity { id, parent: None },
                Op::SetField {
                    entity: id,
                    component: "__meta__".into(),
                    field: "name".into(),
                    value: FieldValue::Str(name.into()),
                },
            ],
        )
        .expect("create assembly");
        id
    }

    #[test]
    fn bom_rolls_up_quantities_and_where_used() {
        let mut e = engine();
        let chassis = assembly(&mut e, "Chassis");
        let wheels = assembly(&mut e, "WheelSet");
        // 4 identical wheels (same content-addressed handle) under WheelSet + 1 bolt under Chassis.
        for _ in 0..4 {
            place_part(&mut e, "mtkasset:wheel", Some(wheels), Some("wheel"));
        }
        place_part(&mut e, "mtkasset:bolt", Some(chassis), Some("bolt"));

        let bom = rollup(&e);
        assert_eq!(bom.distinct_parts, 2, "two distinct part numbers");
        assert_eq!(bom.total_instances, 5, "5 part instances total");

        let wheel = bom
            .lines
            .iter()
            .find(|l| l.part == "mtkasset:wheel")
            .unwrap();
        assert_eq!(
            wheel.quantity, 4,
            "quantity is the instance count, by construction"
        );
        assert_eq!(wheel.where_used, vec!["WheelSet".to_string()]);

        let bolt = bom
            .lines
            .iter()
            .find(|l| l.part == "mtkasset:bolt")
            .unwrap();
        assert_eq!(bolt.quantity, 1);
        assert_eq!(bolt.where_used, vec!["Chassis".to_string()]);
    }

    #[test]
    fn bom_stays_in_sync_after_an_edit_with_no_export_step() {
        let mut e = engine();
        let asm = assembly(&mut e, "Frame");
        place_part(&mut e, "mtkasset:rail", Some(asm), Some("rail"));
        let before = rollup(&e);
        assert_eq!(before.total_instances, 1);

        // Add another instance of the SAME part — the live query reflects it with NO re-export step.
        place_part(&mut e, "mtkasset:rail", Some(asm), Some("rail"));
        let after = rollup(&e);
        assert_eq!(after.total_instances, 2);
        assert_eq!(
            after.distinct_parts, 1,
            "same part number -> same line, quantity bumped"
        );
        assert_eq!(after.lines[0].quantity, 2);

        // Add a NEW distinct part — a new line appears, still no export.
        place_part(&mut e, "mtkasset:plate", Some(asm), Some("plate"));
        let grown = rollup(&e);
        assert_eq!(grown.distinct_parts, 2);
        assert_eq!(grown.total_instances, 3);
    }

    #[test]
    fn a_where_used_part_under_two_assemblies_lists_both() {
        let mut e = engine();
        let a = assembly(&mut e, "AssemblyA");
        let b = assembly(&mut e, "AssemblyB");
        place_part(&mut e, "mtkasset:screw", Some(a), Some("screw"));
        place_part(&mut e, "mtkasset:screw", Some(b), Some("screw"));
        let bom = rollup(&e);
        let screw = bom
            .lines
            .iter()
            .find(|l| l.part == "mtkasset:screw")
            .unwrap();
        assert_eq!(screw.quantity, 2);
        // The where-used query lists BOTH parent assemblies (sorted, de-duplicated).
        assert_eq!(
            screw.where_used,
            vec!["AssemblyA".to_string(), "AssemblyB".to_string()]
        );
    }

    #[test]
    fn rollup_is_deterministic() {
        let mut e = engine();
        let asm = assembly(&mut e, "Frame");
        place_part(&mut e, "mtkasset:b", Some(asm), Some("b"));
        place_part(&mut e, "mtkasset:a", Some(asm), Some("a"));
        // Two roll-ups of the same state are byte-identical (sorted lines) — required for a BOM digest in a
        // content-addressed revision.
        assert_eq!(rollup(&e), rollup(&e));
        assert_eq!(
            rollup(&e).lines[0].part,
            "mtkasset:a",
            "lines are sorted by part number"
        );
    }
}
