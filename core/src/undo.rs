//! Engine-side in-memory inverse-op undo/redo stack (F2 from loro spike).
//!
//! Loro `checkout` costs 50–62 ms for bulk undo — too slow for interactive Ctrl-Z. This module
//! provides O(change) undo by storing the inverse of each committed transaction. Loro `checkout`
//! is reserved for deep history / time-travel, never per-Ctrl-Z.
//!
//! Entity resurrection: deleting an entity captures its full state (components, tags, pairs,
//! bindings, children) so undo can fully restore it.

use crate::entity_id::EntityId;
use crate::pipeline::{FieldValue, Op};
use metrocalk_ecs::Entity;
use std::collections::HashMap;

/// The full state of one entity, captured before deletion for resurrection on undo.
#[derive(Clone, Debug)]
pub struct CapturedEntity {
    pub id: EntityId,
    pub parent: Option<EntityId>,
    pub components: HashMap<String, HashMap<String, FieldValue>>,
    pub tags: Vec<Entity>,
    pub pairs: Vec<(Entity, Entity)>,
}

/// The inverse of a single [`Op`]. Applied in reverse order to undo a transaction.
#[derive(Clone, Debug)]
pub enum InverseOp {
    /// Inverse of CreateEntity — destroy the entity.
    DestroyEntity {
        id: EntityId,
    },

    /// Inverse of DeleteEntity — resurrect the full subtree with all state.
    ResurrectSubtree {
        entities: Vec<CapturedEntity>,
        bindings: Vec<(EntityId, String, EntityId)>,
    },

    /// Inverse of SetField — restore the old value (or remove if it didn't exist).
    SetField {
        entity: EntityId,
        component: String,
        field: String,
        old_value: Option<FieldValue>,
    },

    /// Inverse of RemoveComponent — restore the component with all its fields.
    RestoreComponent {
        entity: EntityId,
        component: String,
        fields: HashMap<String, FieldValue>,
    },

    AddTag {
        entity: EntityId,
        tag: Entity,
    },
    RemoveTag {
        entity: EntityId,
        tag: Entity,
    },
    AddPair {
        entity: EntityId,
        rel: Entity,
        target: Entity,
    },
    RemovePair {
        entity: EntityId,
        rel: Entity,
        target: Entity,
    },
    Reparent {
        entity: EntityId,
        old_parent: Option<EntityId>,
    },
    AddBinding {
        from: EntityId,
        kind: String,
        to: EntityId,
    },
    RemoveBinding {
        from: EntityId,
        kind: String,
        to: EntityId,
    },
}

impl InverseOp {
    /// Convert this inverse op back into a forward [`Op`] for application.
    pub fn to_forward_op(&self) -> Op {
        match self {
            Self::DestroyEntity { id } => Op::DeleteEntity { id: *id },

            Self::ResurrectSubtree { .. } => {
                // Handled specially — expanded into multiple ops by InverseTransaction
                unreachable!("ResurrectSubtree is expanded by InverseTransaction::to_forward_ops")
            }

            Self::SetField {
                entity,
                component,
                field,
                old_value,
            } => match old_value {
                Some(v) => Op::SetField {
                    entity: *entity,
                    component: component.clone(),
                    field: field.clone(),
                    value: v.clone(),
                },
                // Precise inverse of an additive set: remove ONLY this field (not the whole
                // component — that was the M1 over-removal bug that destroyed sibling fields).
                None => Op::RemoveField {
                    entity: *entity,
                    component: component.clone(),
                    field: field.clone(),
                },
            },

            Self::RestoreComponent {
                entity, component, ..
            } => {
                // The fields are restored as individual SetField ops by InverseTransaction
                unreachable!(
                    "RestoreComponent is expanded by InverseTransaction::to_forward_ops for {}::{}",
                    entity, component
                )
            }

            Self::AddTag { entity, tag } => Op::AddTag {
                entity: *entity,
                tag: *tag,
            },
            Self::RemoveTag { entity, tag } => Op::RemoveTag {
                entity: *entity,
                tag: *tag,
            },
            Self::AddPair {
                entity,
                rel,
                target,
            } => Op::AddPair {
                entity: *entity,
                rel: *rel,
                target: *target,
            },
            Self::RemovePair {
                entity,
                rel,
                target,
            } => Op::RemovePair {
                entity: *entity,
                rel: *rel,
                target: *target,
            },
            Self::Reparent { entity, old_parent } => Op::Reparent {
                entity: *entity,
                new_parent: *old_parent,
            },
            Self::AddBinding { from, kind, to } => Op::AddBinding {
                from: *from,
                kind: kind.clone(),
                to: *to,
            },
            Self::RemoveBinding { from, kind, to } => Op::RemoveBinding {
                from: *from,
                kind: kind.clone(),
                to: *to,
            },
        }
    }
}

/// A complete inverse transaction — the inverse of one [`Engine::commit`] call.
#[derive(Clone, Debug)]
pub struct InverseTransaction {
    pub label: String,
    pub ops: Vec<InverseOp>,
}

impl InverseTransaction {
    /// Expand into forward [`Op`]s for undo/redo application. Complex inverse ops
    /// (ResurrectSubtree, RestoreComponent) are expanded into sequences of primitive ops.
    /// The ops are returned in reverse order (last op undone first).
    pub fn to_forward_ops(&self) -> Vec<Op> {
        let mut result = Vec::new();
        for inv in self.ops.iter().rev() {
            match inv {
                InverseOp::ResurrectSubtree { entities, bindings } => {
                    // Recreate entities (parents before children — the Vec is already in
                    // parent-first order from collect_subtree)
                    for ce in entities {
                        result.push(Op::CreateEntity {
                            id: ce.id,
                            parent: ce.parent,
                        });
                        // Restore components
                        for (comp_name, fields) in &ce.components {
                            for (fname, fval) in fields {
                                result.push(Op::SetField {
                                    entity: ce.id,
                                    component: comp_name.clone(),
                                    field: fname.clone(),
                                    value: fval.clone(),
                                });
                            }
                        }
                        // Restore tags
                        for tag in &ce.tags {
                            result.push(Op::AddTag {
                                entity: ce.id,
                                tag: *tag,
                            });
                        }
                        // Restore pairs
                        for (rel, target) in &ce.pairs {
                            result.push(Op::AddPair {
                                entity: ce.id,
                                rel: *rel,
                                target: *target,
                            });
                        }
                    }
                    // Restore bindings
                    for (from, kind, to) in bindings {
                        result.push(Op::AddBinding {
                            from: *from,
                            kind: kind.clone(),
                            to: *to,
                        });
                    }
                }

                InverseOp::RestoreComponent {
                    entity,
                    component,
                    fields,
                } => {
                    for (fname, fval) in fields {
                        result.push(Op::SetField {
                            entity: *entity,
                            component: component.clone(),
                            field: fname.clone(),
                            value: fval.clone(),
                        });
                    }
                }

                // Primitive inverses (incl. SetField{old:None}, whose forward form is a precise
                // single-field RemoveField — see `to_forward_op`) map 1:1.
                other => {
                    result.push(other.to_forward_op());
                }
            }
        }
        result
    }
}
