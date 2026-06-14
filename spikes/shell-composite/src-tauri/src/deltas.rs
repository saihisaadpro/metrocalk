//! Real coalesced-delta source for sub-gate 1a.
//!
//! A scene is mutated like a 60 Hz transform drag, each step a real `commit` through the
//! `metrocalk-core` pipeline. `step()` returns the Loro update bytes *since the previous step* —
//! exactly the invariant-2 "deltas only" payload a transport would carry. This is the real
//! encoding the IPC wire is measured against, not a synthetic blob.

use metrocalk_core::{Engine, EntityId, FieldValue, Op};
use metrocalk_ecs::FlecsWorld;

pub struct DeltaGen {
    engine: Engine<FlecsWorld>,
    entity: EntityId,
    vv: Vec<u8>,
    t: f64,
}

impl DeltaGen {
    pub fn new() -> Self {
        let mut engine = Engine::new(FlecsWorld::new(), 1);
        let entity = engine.alloc_entity_id();
        engine
            .commit(
                "create",
                vec![
                    Op::CreateEntity { id: entity, parent: None },
                    Op::SetField {
                        entity,
                        component: "Transform".into(),
                        field: "px".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity,
                        component: "Transform".into(),
                        field: "py".into(),
                        value: FieldValue::Number(0.0),
                    },
                    Op::SetField {
                        entity,
                        component: "Transform".into(),
                        field: "pz".into(),
                        value: FieldValue::Number(0.0),
                    },
                ],
            )
            .expect("seed commit");
        let vv = engine.version_vector();
        Self { engine, entity, vv, t: 0.0 }
    }

    /// One drag step → the coalesced delta bytes since the last step.
    pub fn step(&mut self) -> Vec<u8> {
        self.t += 1.0;
        let x = (self.t * 0.05).sin();
        let y = (self.t * 0.05).cos();
        self.engine
            .commit(
                "drag",
                vec![
                    Op::SetField {
                        entity: self.entity,
                        component: "Transform".into(),
                        field: "px".into(),
                        value: FieldValue::Number(x),
                    },
                    Op::SetField {
                        entity: self.entity,
                        component: "Transform".into(),
                        field: "py".into(),
                        value: FieldValue::Number(y),
                    },
                ],
            )
            .expect("drag commit");
        let delta = self.engine.export_updates_since(&self.vv);
        self.vv = self.engine.version_vector();
        delta
    }
}
