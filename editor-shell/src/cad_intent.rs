//! M15.0 (ADR-070, Leg A) — STEP import intent: the editor-shell seam that maps an imported STEP
//! [`CadScene`] onto **registry entities** as **one undoable transaction** (invariant 3).
//!
//! The imported part's planar B-rep is tessellated into a **content-addressed [`metrocalk_assets::MeshAsset`]**
//! (`/core` stays mesh-agnostic — geometry by handle, never in Loro, invariant 2). The solid becomes a
//! renderable entity, and **every referenceable face becomes a child entity carrying its stable STEP #id** —
//! the hook M15.3 semantic-PMI / GD&T attaches a feature-control-frame to. Curved faces are referenced +
//! explained (the OCCT seam), never silently dropped. STEP import here is **display / annotate / exchange,
//! not in-engine B-rep editing** (ADR-070). No `opencascade`/STEP-lib type crosses — the boundary is the
//! neutral [`CadScene`] behind the `CadInterchange` trait (invariant 5).

use crate::capscene::{CapScene, MESH_FIELD};
use crate::csg_intent::store_mesh;
use metrocalk_assets::AssetStore;
use metrocalk_core::caps::canonical;
use metrocalk_core::{Engine, EntityId, FieldValue, Op, PipelineError};
use metrocalk_ecs::FlecsWorld;
use metrocalk_interchange::{CadScene, FaceKind};
use std::collections::BTreeSet;

/// What a STEP import produced: the solid entity, the referenceable face + edge entities (the M15.3 PMI /
/// GD&T-datum attach points), the content-addressed mesh handle, and the count of curved faces routed to the
/// OCCT seam.
pub struct StepImport {
    /// The solid (renderable) entity.
    pub solid: EntityId,
    /// One child entity per referenceable face (planar + curved), each carrying its STEP `#id`.
    pub faces: Vec<EntityId>,
    /// One child entity per UNIQUE referenceable edge (deduped by STEP `#id`), each carrying its `#id`.
    pub edges: Vec<EntityId>,
    /// The content-addressed handle of the tessellated mesh.
    pub mesh: String,
    /// How many faces were curved (referenced but tessellation deferred to the OCCT seam).
    pub curved_faces: usize,
}

/// Import a STEP [`CadScene`] as ONE undoable transaction: tessellate → a content-addressed mesh on a
/// renderable solid entity, plus a child entity per referenceable face (the STEP `#id` + kind, so M15.3 PMI
/// can attach). Everything commits atomically (invariant 3); one Ctrl-Z peels the whole import.
///
/// # Errors
/// Propagates a [`PipelineError`] if the commit is rejected.
pub fn import_step(
    engine: &mut Engine<FlecsWorld>,
    scene: &CapScene,
    store: &mut AssetStore,
    cad: &CadScene,
) -> Result<StepImport, PipelineError> {
    // Tessellate the planar B-rep → a content-addressed MeshAsset (geometry by handle, invariant 2).
    let mesh = cad.tessellate();
    let handle = store_mesh(store, &mesh, "step");

    let solid = engine.alloc_entity_id();
    let mut ops = vec![Op::CreateEntity {
        id: solid,
        parent: None,
    }];
    for (f, v) in [("x", 0.0), ("y", 0.0), ("z", 0.0)] {
        ops.push(Op::SetField {
            entity: solid,
            component: "Transform".into(),
            field: f.into(),
            value: FieldValue::Number(v),
        });
    }
    ops.push(Op::SetField {
        entity: solid,
        component: "MeshRenderer".into(),
        field: MESH_FIELD.into(),
        value: FieldValue::Str(handle.clone()),
    });
    if let Some(&c) = scene.caps.get(&canonical("Renderable")) {
        ops.push(Op::AddPair {
            entity: solid,
            rel: scene.rels.provides,
            target: c,
        });
    }

    // Every referenceable face → a child entity carrying its stable STEP #id (the M15.3 PMI attach point).
    let mut faces = Vec::new();
    let mut curved_faces = 0usize;
    for face in cad.solids.iter().flat_map(|s| &s.faces) {
        let fid = engine.alloc_entity_id();
        ops.push(Op::CreateEntity {
            id: fid,
            parent: Some(solid),
        });
        ops.push(Op::SetField {
            entity: fid,
            component: "CadFace".into(),
            field: "step_id".into(),
            value: FieldValue::Integer(i64::try_from(face.id).unwrap_or(i64::MAX)),
        });
        let kind = match face.kind {
            FaceKind::Planar => "planar",
            FaceKind::Curved => {
                curved_faces += 1;
                "curved"
            }
        };
        ops.push(Op::SetField {
            entity: fid,
            component: "CadFace".into(),
            field: "kind".into(),
            value: FieldValue::Str(kind.into()),
        });
        faces.push(fid);
    }

    // Every UNIQUE referenceable edge → a child entity carrying its stable STEP #id. A shared edge appears on
    // two faces but materializes once (deduped by #id). The M15.3 GD&T datum/edge-tolerance attach point.
    let mut edges = Vec::new();
    let mut seen_edges: BTreeSet<u64> = BTreeSet::new();
    for edge in cad
        .solids
        .iter()
        .flat_map(|s| &s.faces)
        .flat_map(|f| &f.edges)
    {
        if !seen_edges.insert(edge.id) {
            continue;
        }
        let eid = engine.alloc_entity_id();
        ops.push(Op::CreateEntity {
            id: eid,
            parent: Some(solid),
        });
        ops.push(Op::SetField {
            entity: eid,
            component: "CadEdge".into(),
            field: "step_id".into(),
            value: FieldValue::Integer(i64::try_from(edge.id).unwrap_or(i64::MAX)),
        });
        edges.push(eid);
    }

    engine.commit("import-step", ops)?;
    Ok(StepImport {
        solid,
        faces,
        edges,
        mesh: handle,
        curved_faces,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capscene::{CapResolver, CapScene};
    use metrocalk_interchange::{CadInterchange, StepInterchange};

    const CUBE_STEP: &str = include_str!("../../interchange/tests/fixtures/cube_ap242.step");

    fn engine() -> (Engine<FlecsWorld>, CapScene) {
        let mut world = FlecsWorld::new();
        let scene = CapScene::intern(&mut world);
        let mut engine = Engine::new(world, 1);
        engine.set_capability_resolver(Box::new(CapResolver::from_scene(&scene)));
        (engine, scene)
    }

    #[test]
    fn a_step_import_is_one_undoable_transaction_with_referenceable_faces() {
        let (mut engine, scene) = engine();
        let mut store = AssetStore::new();
        let cad = StepInterchange
            .import(CUBE_STEP.as_bytes())
            .expect("import");

        assert_eq!(engine.entity_count(), 0);
        let imported = import_step(&mut engine, &scene, &mut store, &cad).expect("map to entities");

        // The solid + 6 referenceable face entities + 24 unique referenceable edge entities (1 + 6 + 24 = 31).
        assert_eq!(imported.faces.len(), 6, "6 referenceable faces");
        assert_eq!(imported.edges.len(), 24, "24 unique referenceable edges");
        assert_eq!(
            engine.entity_count(),
            31,
            "solid + 6 face + 24 edge entities"
        );
        assert!(store.contains(&imported.mesh), "mesh is content-addressed");

        // The solid renders the tessellated mesh by handle (geometry stays out of Loro).
        assert_eq!(
            engine.get_field(imported.solid, "MeshRenderer", MESH_FIELD),
            Some(FieldValue::Str(imported.mesh.clone()))
        );
        // A face + an edge each carry a stable STEP #id — the M15.3 PMI / GD&T-datum attach points.
        let fsid = engine.get_field(imported.faces[0], "CadFace", "step_id");
        assert!(matches!(fsid, Some(FieldValue::Integer(n)) if n > 0));
        let esid = engine.get_field(imported.edges[0], "CadEdge", "step_id");
        assert!(matches!(esid, Some(FieldValue::Integer(n)) if n > 0));

        // ONE Ctrl-Z peels the whole import (solid + all faces + all edges).
        assert!(engine.undo(), "undo the import");
        assert_eq!(
            engine.entity_count(),
            0,
            "one undo removed the whole import"
        );
    }
}
