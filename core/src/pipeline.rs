//! Single transactional commit pipeline (invariant 3).
//!
//! Every mutation — editor, plugin, AI — enters through [`Engine::commit`]. The ECS [`World`] stays
//! authoritative at runtime; each committed transaction mirrors to the Loro document as **deltas**
//! (invariant 2). Every op is pre-validated before any is applied, so a commit is all-or-nothing —
//! a single invalid op rejects the whole batch with no partial mutation. One transaction ↔ one Loro
//! commit; per-op inverses are captured for the engine-side undo stack ([`crate::undo`]).
//!
//! Loro types are internal — nothing from `loro::` appears in the public API.

use crate::entity_id::{EntityId, IdGenerator};
use crate::merge::{self, MergeReport};
use crate::undo::{InverseOp, InverseTransaction};
use loro::{
    Container, ExportMode, LoroDoc, LoroMap, LoroValue, TreeID, TreeParentId, ValueOrContainer,
};
use metrocalk_ecs::{Entity, World};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

// ── public value type (no Loro leak) ───────────────────────────────────────

/// A component field value. Maps 1:1 to JSON-Schema scalar types and to Loro value variants.
#[derive(Clone, Debug, PartialEq)]
pub enum FieldValue {
    Integer(i64),
    Number(f64),
    Bool(bool),
    Str(String),
}

impl FieldValue {
    pub(crate) fn to_loro(&self) -> LoroValue {
        match self {
            Self::Integer(i) => LoroValue::I64(*i),
            Self::Number(n) => LoroValue::Double(*n),
            Self::Bool(b) => LoroValue::Bool(*b),
            Self::Str(s) => LoroValue::from(s.as_str()),
        }
    }

    pub(crate) fn from_loro(v: &LoroValue) -> Option<Self> {
        match v {
            LoroValue::I64(i) => Some(Self::Integer(*i)),
            LoroValue::Double(n) => Some(Self::Number(*n)),
            LoroValue::Bool(b) => Some(Self::Bool(*b)),
            LoroValue::String(s) => Some(Self::Str(s.to_string())),
            _ => None,
        }
    }
}

// ── operations ─────────────────────────────────────────────────────────────

/// One atomic operation in the commit pipeline. A [`Engine::commit`] call takes a `Vec<Op>` that
/// forms a single undoable transaction.
#[derive(Clone, Debug)]
pub enum Op {
    /// Create a scene entity (both ECS + Loro tree node).
    CreateEntity {
        id: EntityId,
        parent: Option<EntityId>,
    },
    /// Delete a scene entity and its descendants (cascade).
    DeleteEntity { id: EntityId },
    /// Set a component field value (Loro-side data).
    SetField {
        entity: EntityId,
        component: String,
        field: String,
        value: FieldValue,
    },
    /// Remove an entire component record from an entity.
    RemoveComponent { entity: EntityId, component: String },
    /// Remove a single component field, leaving sibling fields intact (and dropping the component
    /// record if it becomes empty). The precise inverse of an additive [`Op::SetField`].
    RemoveField {
        entity: EntityId,
        component: String,
        field: String,
    },
    /// Add a tag to an entity (ECS-only, for query support).
    AddTag { entity: EntityId, tag: Entity },
    /// Remove a tag from an entity.
    RemoveTag { entity: EntityId, tag: Entity },
    /// Add a relationship pair (ECS-only, for query support).
    AddPair {
        entity: EntityId,
        rel: Entity,
        target: Entity,
    },
    /// Remove a relationship pair.
    RemovePair {
        entity: EntityId,
        rel: Entity,
        target: Entity,
    },
    /// Reparent an entity in the Loro tree.
    Reparent {
        entity: EntityId,
        new_parent: Option<EntityId>,
    },
    /// Add a binding edge (Loro-side).
    AddBinding {
        from: EntityId,
        kind: String,
        to: EntityId,
    },
    /// Remove a binding edge (Loro-side).
    RemoveBinding {
        from: EntityId,
        kind: String,
        to: EntityId,
    },
}

// ── errors ─────────────────────────────────────────────────────────────────

#[derive(Error, Debug)]
pub enum PipelineError {
    #[error("unknown entity: {0}")]
    UnknownEntity(EntityId),
    #[error("entity already exists: {0}")]
    DuplicateEntity(EntityId),
    #[error("loro operation failed: {0}")]
    Loro(String),
}

// ── engine ─────────────────────────────────────────────────────────────────

/// The single transactional core. Owns the ECS world (authoritative) and Loro document (mirror).
/// All scene mutations flow through [`commit`](Self::commit). Undo/redo is an in-memory
/// inverse-op stack, not Loro `checkout` (F2).
pub struct Engine<W: World> {
    world: W,
    doc: LoroDoc,
    id_gen: IdGenerator,

    // entity mapping: logical ↔ ECS ↔ Loro
    eid_to_ecs: HashMap<EntityId, Entity>,
    ecs_to_eid: HashMap<Entity, EntityId>,
    eid_to_tid: HashMap<EntityId, TreeID>,
    tid_to_eid_map: HashMap<TreeID, EntityId>,

    // ECS-side tracking (needed for delete-inverse capture)
    entity_tags: HashMap<EntityId, HashSet<Entity>>,
    entity_pairs: HashMap<EntityId, HashSet<(Entity, Entity)>>,

    // undo / redo
    undo_stack: Vec<InverseTransaction>,
    redo_stack: Vec<InverseTransaction>,
}

impl<W: World> Engine<W> {
    /// Create a new engine with the given world backend and peer id.
    pub fn new(world: W, peer_id: u64) -> Self {
        let doc = LoroDoc::new();
        doc.set_peer_id(peer_id).unwrap();
        // Touch the three top-level containers so they exist before any commit.
        let _ = doc.get_tree("hierarchy");
        let _ = doc.get_map("components");
        let _ = doc.get_map("bindings");

        Self {
            world,
            doc,
            id_gen: IdGenerator::new(peer_id),
            eid_to_ecs: HashMap::new(),
            ecs_to_eid: HashMap::new(),
            eid_to_tid: HashMap::new(),
            tid_to_eid_map: HashMap::new(),
            entity_tags: HashMap::new(),
            entity_pairs: HashMap::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    // ── public reads ───────────────────────────────────────────────────

    /// Read-only world access for queries. Scene mutations go through [`commit`](Self::commit).
    pub fn world(&self) -> &W {
        &self.world
    }

    /// Allocate a peer-namespaced entity id (does not create the entity — submit a
    /// [`Op::CreateEntity`] to actually create it).
    pub fn alloc_entity_id(&mut self) -> EntityId {
        self.id_gen.next_id()
    }

    pub fn entity_exists(&self, id: EntityId) -> bool {
        self.eid_to_ecs.contains_key(&id)
    }

    /// Read a component field value from the Loro document.
    pub fn get_field(&self, entity: EntityId, component: &str, field: &str) -> Option<FieldValue> {
        let key = entity.to_loro_key();
        let components = self.doc.get_map("components");
        let rec = get_child_map(&components, &key)?;
        let cmap = get_child_map(&rec, component)?;
        let val = cmap.get(field)?;
        match val {
            ValueOrContainer::Value(v) => FieldValue::from_loro(&v),
            ValueOrContainer::Container(_) => None,
        }
    }

    /// How many scene entities exist.
    pub fn entity_count(&self) -> usize {
        self.eid_to_ecs.len()
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    pub fn peer_id(&self) -> u64 {
        self.id_gen.peer()
    }

    // ── projection reads (M2.6: feed the editor's ProjectionDelta) ──────

    /// All live scene entity ids (order not guaranteed).
    #[must_use]
    pub fn entity_ids(&self) -> Vec<EntityId> {
        self.eid_to_ecs.keys().copied().collect()
    }

    /// The parent of `id` in the scene hierarchy (`None` = root).
    #[must_use]
    pub fn parent_of(&self, id: EntityId) -> Option<EntityId> {
        let tid = *self.eid_to_tid.get(&id)?;
        match self.doc.get_tree("hierarchy").parent(tid) {
            Some(TreeParentId::Node(ptid)) => self.tid_to_eid(ptid),
            _ => None,
        }
    }

    /// All component field values for `id` as `component → field → value`.
    #[must_use]
    pub fn components_of(&self, id: EntityId) -> HashMap<String, HashMap<String, FieldValue>> {
        self.capture_components(id)
    }

    /// The ECS handle backing a live scene entity id — the bridge from the projection/edit world
    /// (logical [`EntityId`]) to the relational query world (raw [`Entity`]), so a query that takes a
    /// raw handle (e.g. the M3.1 reveal / compatibility query) can be run for a selected entity.
    /// `None` if `id` is not a live scene entity.
    #[must_use]
    pub fn ecs_entity(&self, id: EntityId) -> Option<Entity> {
        self.eid_to_ecs.get(&id).copied()
    }

    /// The scene entity id backing an ECS handle — the inverse of [`ecs_entity`](Self::ecs_entity),
    /// mapping a query result back to a projectable/bindable id. `None` for non-scene handles (e.g. the
    /// interned relationship/capability entities the reveal query matches against).
    #[must_use]
    pub fn entity_id_of(&self, e: Entity) -> Option<EntityId> {
        self.ecs_to_eid.get(&e).copied()
    }

    /// All binding edges in the scene as `(from, kind, to)`.
    #[must_use]
    pub fn bindings(&self) -> Vec<(EntityId, String, EntityId)> {
        let bindings = self.doc.get_map("bindings");
        let mut out = Vec::new();
        for k in bindings.keys() {
            let parts: Vec<&str> = k.split('|').collect();
            if parts.len() == 3 {
                if let (Some(f), Some(t)) = (
                    EntityId::from_loro_key(parts[0]),
                    EntityId::from_loro_key(parts[2]),
                ) {
                    out.push((f, parts[1].to_string(), t));
                }
            }
        }
        out
    }

    // ── commit (the sole mutation path) ────────────────────────────────

    /// Apply a transaction. This is the **sole** entry point for scene mutations (invariant 3).
    ///
    /// **Atomic / all-or-nothing:** every op is pre-validated (entity/parent existence, no duplicate
    /// ids) against the transaction's own running state *before* any op is applied; a single invalid
    /// op rejects the whole batch — no partial mutation, no undo entry. Valid ops are then applied to
    /// the authoritative ECS world and mirrored to the Loro document as deltas (invariant 2), with
    /// per-op inverses captured for undo, sealed as one Loro commit.
    ///
    /// The only non-atomic residual is a Loro-internal error mid-apply (bug-class, not caller-
    /// controllable); it surfaces as [`PipelineError::Loro`] and is intentionally loud.
    // Takes the op batch by value: `commit` owns the transaction the caller hands it
    // (ergonomic `commit("label", vec![..])` at every call site); not consumed today but the
    // ownership is the intended contract.
    #[allow(clippy::needless_pass_by_value)]
    pub fn commit(&mut self, label: &str, ops: Vec<Op>) -> Result<(), PipelineError> {
        if ops.is_empty() {
            return Ok(());
        }

        // 1. Pre-validate the WHOLE batch — all-or-nothing (no partial mutation on a bad op).
        self.validate_transaction(&ops)?;

        // 2. Apply, capturing per-op inverses (interleaved; see apply_transaction).
        let inverse_ops = self.apply_transaction(&ops)?;

        // 3. One transaction ↔ one Loro commit.
        self.doc.commit();

        // 4. Push undo, clear redo.
        self.undo_stack.push(InverseTransaction {
            label: label.to_string(),
            ops: inverse_ops,
        });
        self.redo_stack.clear();

        Ok(())
    }

    /// Undo the last transaction. Returns `true` if there was something to undo.
    pub fn undo(&mut self) -> bool {
        let Some(inv) = self.undo_stack.pop() else {
            return false;
        };
        let forward_ops = inv.to_forward_ops();
        // Undo/redo replay the inverse of an already-valid transaction, so they are trusted: a
        // failure here is a logic bug (not caller input), and we surface it loudly rather than
        // silently corrupt the undo/redo stacks.
        let redo_inverse = self
            .apply_transaction(&forward_ops)
            .expect("undo: replaying the inverse of a valid transaction must not fail");
        self.doc.commit();
        self.redo_stack.push(InverseTransaction {
            label: inv.label,
            ops: redo_inverse,
        });
        true
    }

    /// Redo the last undone transaction. Returns `true` if there was something to redo.
    pub fn redo(&mut self) -> bool {
        let Some(inv) = self.redo_stack.pop() else {
            return false;
        };
        let forward_ops = inv.to_forward_ops();
        // See `undo`: replaying a trusted, already-valid transaction — fail loudly on a logic bug.
        let undo_inverse = self
            .apply_transaction(&forward_ops)
            .expect("redo: replaying the inverse of a valid transaction must not fail");
        self.doc.commit();
        self.undo_stack.push(InverseTransaction {
            label: inv.label,
            ops: undo_inverse,
        });
        true
    }

    // ── merge ──────────────────────────────────────────────────────────

    /// Import remote Loro updates, run merge validation (detect + repair all 8 invalid-state
    /// classes), and rebuild the ECS world from the merged Loro document.
    pub fn merge(&mut self, updates: &[u8]) -> Result<MergeReport, PipelineError> {
        self.doc
            .import(updates)
            .map_err(|e| PipelineError::Loro(e.to_string()))?;
        self.doc.commit();

        let report = merge::validate_and_repair(&self.doc);
        if report.total_repairs > 0 {
            self.doc.commit();
        }

        self.rebuild_ecs_from_loro();

        // Undo stack is invalidated by merge (ECS handles changed, interleaved ops — see
        // adversarial analysis in merge.rs).
        self.undo_stack.clear();
        self.redo_stack.clear();

        Ok(report)
    }

    /// Export Loro updates since a given version vector (for transport).
    pub fn export_updates(&self) -> Vec<u8> {
        self.doc.export(ExportMode::all_updates()).unwrap()
    }

    /// Export only updates since a specific version vector snapshot.
    pub fn export_updates_since(&self, vv_bytes: &[u8]) -> Vec<u8> {
        let vv = loro::VersionVector::decode(vv_bytes).unwrap();
        self.doc.export(ExportMode::updates(&vv)).unwrap()
    }

    /// Snapshot the current version vector (opaque bytes for `export_updates_since`).
    pub fn version_vector(&self) -> Vec<u8> {
        self.doc.oplog_vv().encode()
    }

    /// Fork this engine's Loro document (for testing merge scenarios).
    pub fn fork_doc(&self) -> Vec<u8> {
        self.doc.export(ExportMode::Snapshot).unwrap()
    }

    // ── internals ──────────────────────────────────────────────────────

    /// Pre-validate the whole batch against a running "alive" set (existing entities + in-batch
    /// creates − in-batch deletes) so an op may legally reference an entity created earlier in the
    /// *same* transaction. Any failed precondition rejects the transaction before a single op is
    /// applied — this is what makes [`commit`](Self::commit) all-or-nothing.
    fn validate_transaction(&self, ops: &[Op]) -> Result<(), PipelineError> {
        let mut alive: HashSet<EntityId> = self.eid_to_ecs.keys().copied().collect();
        let mut in_tx_children: HashMap<EntityId, Vec<EntityId>> = HashMap::new();

        for op in ops {
            match op {
                Op::CreateEntity { id, parent } => {
                    if alive.contains(id) {
                        return Err(PipelineError::DuplicateEntity(*id));
                    }
                    if let Some(p) = parent {
                        if !alive.contains(p) {
                            return Err(PipelineError::UnknownEntity(*p));
                        }
                        in_tx_children.entry(*p).or_default().push(*id);
                    }
                    alive.insert(*id);
                }
                Op::DeleteEntity { id } => {
                    if !alive.contains(id) {
                        return Err(PipelineError::UnknownEntity(*id));
                    }
                    // A delete cascades; drop the whole subtree from `alive` so a later op that
                    // references a cascade-deleted descendant is correctly rejected.
                    for e in self.validation_subtree(*id, &in_tx_children) {
                        alive.remove(&e);
                    }
                }
                Op::SetField { entity, .. }
                | Op::RemoveComponent { entity, .. }
                | Op::RemoveField { entity, .. }
                | Op::AddTag { entity, .. }
                | Op::RemoveTag { entity, .. }
                | Op::AddPair { entity, .. }
                | Op::RemovePair { entity, .. } => {
                    if !alive.contains(entity) {
                        return Err(PipelineError::UnknownEntity(*entity));
                    }
                }
                Op::Reparent { entity, new_parent } => {
                    if !alive.contains(entity) {
                        return Err(PipelineError::UnknownEntity(*entity));
                    }
                    if let Some(p) = new_parent {
                        if !alive.contains(p) {
                            return Err(PipelineError::UnknownEntity(*p));
                        }
                    }
                }
                Op::AddBinding { from, to, .. } => {
                    if !alive.contains(from) {
                        return Err(PipelineError::UnknownEntity(*from));
                    }
                    if !alive.contains(to) {
                        return Err(PipelineError::UnknownEntity(*to));
                    }
                }
                // `apply_remove_binding` tolerates a missing edge / endpoints (no-op if absent), so
                // it can never fail — nothing to pre-validate.
                Op::RemoveBinding { .. } => {}
            }
        }
        Ok(())
    }

    /// The set of entities a `DeleteEntity` would cascade-remove: the real subtree (already-applied
    /// tree nodes) unioned with descendants created earlier in this same (not-yet-applied)
    /// transaction. Used by [`validate_transaction`](Self::validate_transaction) only.
    ///
    /// Note: an in-batch `Reparent` is not reflected here, so a delete that targets an entity whose
    /// in-batch-moved child should cascade may under-approximate — a pathological self-contradicting
    /// batch. It stays *sound* (never rejects a valid batch); the worst case is that such a batch is
    /// caught at apply time (loud Loro/unknown-entity error) rather than at validation.
    fn validation_subtree(
        &self,
        root: EntityId,
        in_tx_children: &HashMap<EntityId, Vec<EntityId>>,
    ) -> HashSet<EntityId> {
        let tree = self.doc.get_tree("hierarchy");
        let mut result = HashSet::new();
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if !result.insert(e) {
                continue;
            }
            if let Some(&tid) = self.eid_to_tid.get(&e) {
                if let Some(children) = tree.children(TreeParentId::Node(tid)) {
                    for ctid in children {
                        if let Some(ceid) = self.tid_to_eid(ctid) {
                            stack.push(ceid);
                        }
                    }
                }
            }
            if let Some(kids) = in_tx_children.get(&e) {
                stack.extend(kids.iter().copied());
            }
        }
        result
    }

    /// Apply each op in order, capturing its inverse against the live state *just before* it runs.
    /// Interleaving compute+apply (vs. compute-all-then-apply-all) is what lets an op reference an
    /// entity created earlier in the same transaction — e.g. resurrection's create-then-set-field,
    /// or a create-then-delete batch. [`commit`](Self::commit) pre-validates, so no caller-
    /// controllable op fails here; the only possible error is a Loro-internal failure (loud).
    fn apply_transaction(&mut self, ops: &[Op]) -> Result<Vec<InverseOp>, PipelineError> {
        let mut inverses = Vec::with_capacity(ops.len());
        for op in ops {
            let inv = self.inverse_of(op)?;
            self.apply_one(op)?;
            inverses.push(inv);
        }
        Ok(inverses)
    }

    fn apply_one(&mut self, op: &Op) -> Result<(), PipelineError> {
        match op {
            Op::CreateEntity { id, parent } => self.apply_create(*id, *parent),
            Op::DeleteEntity { id } => self.apply_delete(*id),
            Op::SetField {
                entity,
                component,
                field,
                value,
            } => self.apply_set_field(*entity, component, field, value),
            Op::AddTag { entity, tag } => self.apply_add_tag(*entity, *tag),
            Op::RemoveTag { entity, tag } => self.apply_remove_tag(*entity, *tag),
            Op::AddPair {
                entity,
                rel,
                target,
            } => self.apply_add_pair(*entity, *rel, *target),
            Op::RemovePair {
                entity,
                rel,
                target,
            } => self.apply_remove_pair(*entity, *rel, *target),
            Op::RemoveComponent { entity, component } => {
                self.apply_remove_component(*entity, component)
            }
            Op::RemoveField {
                entity,
                component,
                field,
            } => self.apply_remove_field(*entity, component, field),
            Op::Reparent { entity, new_parent } => self.apply_reparent(*entity, *new_parent),
            Op::AddBinding { from, kind, to } => self.apply_add_binding(*from, kind, *to),
            Op::RemoveBinding { from, kind, to } => self.apply_remove_binding(*from, kind, *to),
        }
    }

    fn apply_create(
        &mut self,
        id: EntityId,
        parent: Option<EntityId>,
    ) -> Result<(), PipelineError> {
        if self.eid_to_ecs.contains_key(&id) {
            return Err(PipelineError::DuplicateEntity(id));
        }

        // ECS
        let ecs_entity = self.world.create_entity();

        // Loro tree
        let tree = self.doc.get_tree("hierarchy");
        let parent_tid = match parent {
            Some(pid) => {
                let tid = self
                    .eid_to_tid
                    .get(&pid)
                    .ok_or(PipelineError::UnknownEntity(pid))?;
                TreeParentId::Node(*tid)
            }
            None => TreeParentId::Root,
        };
        let tid = tree.create(parent_tid).map_err(loro_err)?;
        tree.get_meta(tid)
            .map_err(loro_err)?
            .insert("eid", id.to_loro_key().as_str())
            .map_err(loro_err)?;

        // Loro components placeholder
        let components = self.doc.get_map("components");
        let _ = try_child_map(&components, &id.to_loro_key())?;

        // Mapping
        self.eid_to_ecs.insert(id, ecs_entity);
        self.ecs_to_eid.insert(ecs_entity, id);
        self.eid_to_tid.insert(id, tid);
        self.tid_to_eid_map.insert(tid, id);
        self.entity_tags.insert(id, HashSet::new());
        self.entity_pairs.insert(id, HashSet::new());

        Ok(())
    }

    fn apply_delete(&mut self, id: EntityId) -> Result<(), PipelineError> {
        let ecs_entity = *self
            .eid_to_ecs
            .get(&id)
            .ok_or(PipelineError::UnknownEntity(id))?;
        let tid = *self
            .eid_to_tid
            .get(&id)
            .ok_or(PipelineError::UnknownEntity(id))?;

        // Collect subtree (the entity + all descendants)
        let subtree = self.collect_subtree(id);

        // Loro: delete tree node (children auto-cascade to "under deleted ancestor")
        let tree = self.doc.get_tree("hierarchy");
        tree.delete(tid).map_err(loro_err)?;

        // Loro: clean up component records + bindings for all subtree entities
        let components = self.doc.get_map("components");
        let bindings = self.doc.get_map("bindings");
        let subtree_keys: HashSet<String> = subtree.iter().map(EntityId::to_loro_key).collect();
        for eid in &subtree {
            let key = eid.to_loro_key();
            if components.get(&key).is_some() {
                components.delete(&key).map_err(loro_err)?;
            }
        }
        // Remove bindings that reference any entity in the subtree
        let binding_keys: Vec<String> = bindings
            .keys()
            .filter(|k| {
                let parts: Vec<&str> = k.split('|').collect();
                parts.len() == 3
                    && (subtree_keys.contains(parts[0]) || subtree_keys.contains(parts[2]))
            })
            .map(|k| k.to_string())
            .collect();
        for k in &binding_keys {
            bindings.delete(k).map_err(loro_err)?;
        }

        // ECS: delete all entities in subtree
        for eid in &subtree {
            if let Some(e) = self.eid_to_ecs.remove(eid) {
                self.world.delete_entity(e);
                self.ecs_to_eid.remove(&e);
            }
            if let Some(t) = self.eid_to_tid.remove(eid) {
                self.tid_to_eid_map.remove(&t);
            }
            self.entity_tags.remove(eid);
            self.entity_pairs.remove(eid);
        }

        let _ = ecs_entity; // used above through mapping removal
        Ok(())
    }

    fn apply_set_field(
        &mut self,
        entity: EntityId,
        component: &str,
        field: &str,
        value: &FieldValue,
    ) -> Result<(), PipelineError> {
        if !self.eid_to_ecs.contains_key(&entity) {
            return Err(PipelineError::UnknownEntity(entity));
        }
        let key = entity.to_loro_key();
        let components = self.doc.get_map("components");
        let rec = try_child_map(&components, &key)?;
        let cmap = try_child_map(&rec, component)?;
        cmap.insert(field, value.to_loro()).map_err(loro_err)?;
        Ok(())
    }

    fn apply_remove_component(
        &mut self,
        entity: EntityId,
        component: &str,
    ) -> Result<(), PipelineError> {
        if !self.eid_to_ecs.contains_key(&entity) {
            return Err(PipelineError::UnknownEntity(entity));
        }
        let key = entity.to_loro_key();
        let components = self.doc.get_map("components");
        if let Some(ValueOrContainer::Container(Container::Map(rec))) = components.get(&key) {
            if rec.get(component).is_some() {
                rec.delete(component).map_err(loro_err)?;
            }
        }
        Ok(())
    }

    /// Remove a single field, leaving sibling fields intact. If the component record is left with no
    /// fields, drop it too — so undoing the creation of a component's first field restores the exact
    /// prior state (no lingering empty component). This is the precise inverse of an additive
    /// [`Op::SetField`]; the old over-broad [`Op::RemoveComponent`] inverse destroyed sibling fields
    /// written by earlier transactions (the M1 audit bug).
    fn apply_remove_field(
        &mut self,
        entity: EntityId,
        component: &str,
        field: &str,
    ) -> Result<(), PipelineError> {
        if !self.eid_to_ecs.contains_key(&entity) {
            return Err(PipelineError::UnknownEntity(entity));
        }
        let key = entity.to_loro_key();
        let components = self.doc.get_map("components");
        if let Some(ValueOrContainer::Container(Container::Map(rec))) = components.get(&key) {
            if let Some(ValueOrContainer::Container(Container::Map(cmap))) = rec.get(component) {
                if cmap.get(field).is_some() {
                    cmap.delete(field).map_err(loro_err)?;
                }
                if cmap.is_empty() {
                    rec.delete(component).map_err(loro_err)?;
                }
            }
        }
        Ok(())
    }

    fn apply_add_tag(&mut self, entity: EntityId, tag: Entity) -> Result<(), PipelineError> {
        let ecs = *self
            .eid_to_ecs
            .get(&entity)
            .ok_or(PipelineError::UnknownEntity(entity))?;
        self.world.add_tag(ecs, tag);
        self.entity_tags.entry(entity).or_default().insert(tag);
        Ok(())
    }

    fn apply_remove_tag(&mut self, entity: EntityId, tag: Entity) -> Result<(), PipelineError> {
        let ecs = *self
            .eid_to_ecs
            .get(&entity)
            .ok_or(PipelineError::UnknownEntity(entity))?;
        self.world.remove_tag(ecs, tag);
        self.entity_tags.entry(entity).or_default().remove(&tag);
        Ok(())
    }

    fn apply_add_pair(
        &mut self,
        entity: EntityId,
        rel: Entity,
        target: Entity,
    ) -> Result<(), PipelineError> {
        let ecs = *self
            .eid_to_ecs
            .get(&entity)
            .ok_or(PipelineError::UnknownEntity(entity))?;
        self.world.add_pair(ecs, rel, target);
        self.entity_pairs
            .entry(entity)
            .or_default()
            .insert((rel, target));
        Ok(())
    }

    fn apply_remove_pair(
        &mut self,
        entity: EntityId,
        rel: Entity,
        target: Entity,
    ) -> Result<(), PipelineError> {
        let ecs = *self
            .eid_to_ecs
            .get(&entity)
            .ok_or(PipelineError::UnknownEntity(entity))?;
        self.world.remove_pair(ecs, rel, target);
        self.entity_pairs
            .entry(entity)
            .or_default()
            .remove(&(rel, target));
        Ok(())
    }

    fn apply_reparent(
        &mut self,
        entity: EntityId,
        new_parent: Option<EntityId>,
    ) -> Result<(), PipelineError> {
        let tid = *self
            .eid_to_tid
            .get(&entity)
            .ok_or(PipelineError::UnknownEntity(entity))?;
        let tree = self.doc.get_tree("hierarchy");
        match new_parent {
            Some(pid) => {
                let parent_tid = *self
                    .eid_to_tid
                    .get(&pid)
                    .ok_or(PipelineError::UnknownEntity(pid))?;
                tree.mov(tid, parent_tid).map_err(loro_err)?;
            }
            None => {
                tree.mov_to(tid, TreeParentId::Root, 0).map_err(loro_err)?;
            }
        }
        Ok(())
    }

    fn apply_add_binding(
        &mut self,
        from: EntityId,
        kind: &str,
        to: EntityId,
    ) -> Result<(), PipelineError> {
        if !self.eid_to_ecs.contains_key(&from) {
            return Err(PipelineError::UnknownEntity(from));
        }
        if !self.eid_to_ecs.contains_key(&to) {
            return Err(PipelineError::UnknownEntity(to));
        }
        let key = binding_key(&from, kind, &to);
        let bindings = self.doc.get_map("bindings");
        let em = try_child_map(&bindings, &key)?;
        em.insert("from", from.to_loro_key().as_str())
            .map_err(loro_err)?;
        em.insert("to", to.to_loro_key().as_str())
            .map_err(loro_err)?;
        em.insert("kind", kind).map_err(loro_err)?;
        Ok(())
    }

    fn apply_remove_binding(
        &mut self,
        from: EntityId,
        kind: &str,
        to: EntityId,
    ) -> Result<(), PipelineError> {
        let key = binding_key(&from, kind, &to);
        let bindings = self.doc.get_map("bindings");
        if bindings.get(&key).is_some() {
            bindings.delete(&key).map_err(loro_err)?;
        }
        Ok(())
    }

    // ── inverse computation (reads current state) ──────────────────────

    #[allow(clippy::too_many_lines)] // one match arm per Op variant; splitting would fragment the inverse logic
    fn inverse_of(&self, op: &Op) -> Result<InverseOp, PipelineError> {
        match op {
            Op::CreateEntity { id, .. } => Ok(InverseOp::DestroyEntity { id: *id }),

            Op::DeleteEntity { id } => {
                let tid = *self
                    .eid_to_tid
                    .get(id)
                    .ok_or(PipelineError::UnknownEntity(*id))?;

                // Capture parent
                let tree = self.doc.get_tree("hierarchy");
                let parent = match tree.parent(tid) {
                    Some(TreeParentId::Node(ptid)) => self.tid_to_eid(ptid),
                    _ => None,
                };

                // Capture subtree
                let subtree = self.collect_subtree(*id);

                // For each entity in subtree, capture its full state
                let mut entities = Vec::new();
                for eid in &subtree {
                    let comps = self.capture_components(*eid);
                    let tags: Vec<Entity> = self
                        .entity_tags
                        .get(eid)
                        .map(|s| s.iter().copied().collect())
                        .unwrap_or_default();
                    let pairs: Vec<(Entity, Entity)> = self
                        .entity_pairs
                        .get(eid)
                        .map(|s| s.iter().copied().collect())
                        .unwrap_or_default();
                    let e_parent = if *eid == *id {
                        parent
                    } else {
                        let e_tid = self.eid_to_tid.get(eid);
                        e_tid.and_then(|t| match tree.parent(*t) {
                            Some(TreeParentId::Node(ptid)) => self.tid_to_eid(ptid),
                            _ => None,
                        })
                    };
                    entities.push(crate::undo::CapturedEntity {
                        id: *eid,
                        parent: e_parent,
                        components: comps,
                        tags,
                        pairs,
                    });
                }

                // Capture bindings involving any subtree entity
                let subtree_keys: HashSet<String> =
                    subtree.iter().map(EntityId::to_loro_key).collect();
                let bindings_map = self.doc.get_map("bindings");
                let mut bindings = Vec::new();
                for k in bindings_map.keys() {
                    let parts: Vec<&str> = k.split('|').collect();
                    if parts.len() == 3
                        && (subtree_keys.contains(parts[0]) || subtree_keys.contains(parts[2]))
                    {
                        if let (Some(from), Some(to)) = (
                            EntityId::from_loro_key(parts[0]),
                            EntityId::from_loro_key(parts[2]),
                        ) {
                            bindings.push((from, parts[1].to_string(), to));
                        }
                    }
                }

                Ok(InverseOp::ResurrectSubtree { entities, bindings })
            }

            // Setting a field and removing a field share one inverse: restore the field's prior
            // value (`SetField{old:Some}`), or — if it had none — `SetField{old:None}`, whose
            // forward form is a precise single-field `RemoveField`.
            Op::SetField {
                entity,
                component,
                field,
                ..
            }
            | Op::RemoveField {
                entity,
                component,
                field,
            } => {
                let old = self.get_field(*entity, component, field);
                Ok(InverseOp::SetField {
                    entity: *entity,
                    component: component.clone(),
                    field: field.clone(),
                    old_value: old,
                })
            }

            Op::RemoveComponent { entity, component } => {
                let fields = self.capture_component_fields(*entity, component);
                Ok(InverseOp::RestoreComponent {
                    entity: *entity,
                    component: component.clone(),
                    fields,
                })
            }

            Op::AddTag { entity, tag } => Ok(InverseOp::RemoveTag {
                entity: *entity,
                tag: *tag,
            }),
            Op::RemoveTag { entity, tag } => Ok(InverseOp::AddTag {
                entity: *entity,
                tag: *tag,
            }),
            Op::AddPair {
                entity,
                rel,
                target,
            } => Ok(InverseOp::RemovePair {
                entity: *entity,
                rel: *rel,
                target: *target,
            }),
            Op::RemovePair {
                entity,
                rel,
                target,
            } => Ok(InverseOp::AddPair {
                entity: *entity,
                rel: *rel,
                target: *target,
            }),

            Op::Reparent { entity, .. } => {
                let tid = *self
                    .eid_to_tid
                    .get(entity)
                    .ok_or(PipelineError::UnknownEntity(*entity))?;
                let tree = self.doc.get_tree("hierarchy");
                let old_parent = match tree.parent(tid) {
                    Some(TreeParentId::Node(ptid)) => self.tid_to_eid(ptid),
                    _ => None,
                };
                Ok(InverseOp::Reparent {
                    entity: *entity,
                    old_parent,
                })
            }

            Op::AddBinding { from, kind, to } => Ok(InverseOp::RemoveBinding {
                from: *from,
                kind: kind.clone(),
                to: *to,
            }),
            Op::RemoveBinding { from, kind, to } => Ok(InverseOp::AddBinding {
                from: *from,
                kind: kind.clone(),
                to: *to,
            }),
        }
    }

    // ── helpers ─────────────────────────────────────────────────────────

    fn tid_to_eid(&self, tid: TreeID) -> Option<EntityId> {
        self.tid_to_eid_map.get(&tid).copied()
    }

    fn collect_subtree(&self, root: EntityId) -> Vec<EntityId> {
        let Some(&root_tid) = self.eid_to_tid.get(&root) else {
            return vec![root];
        };
        let tree = self.doc.get_tree("hierarchy");
        let mut result = vec![root];
        let mut queue = vec![root_tid];
        while let Some(tid) = queue.pop() {
            if let Some(children) = tree.children(TreeParentId::Node(tid)) {
                for child_tid in children {
                    if let Some(eid) = self.tid_to_eid(child_tid) {
                        result.push(eid);
                    }
                    queue.push(child_tid);
                }
            }
        }
        result
    }

    fn capture_components(&self, entity: EntityId) -> HashMap<String, HashMap<String, FieldValue>> {
        let key = entity.to_loro_key();
        let components = self.doc.get_map("components");
        let mut result = HashMap::new();
        if let Some(ValueOrContainer::Container(Container::Map(rec))) = components.get(&key) {
            if let LoroValue::Map(m) = rec.get_deep_value() {
                for (comp_name, comp_val) in m.iter() {
                    if let LoroValue::Map(fields) = comp_val {
                        let mut fmap = HashMap::new();
                        for (fname, fval) in fields.iter() {
                            if let Some(fv) = FieldValue::from_loro(fval) {
                                fmap.insert(fname.clone(), fv);
                            }
                        }
                        result.insert(comp_name.clone(), fmap);
                    }
                }
            }
        }
        result
    }

    fn capture_component_fields(
        &self,
        entity: EntityId,
        component: &str,
    ) -> HashMap<String, FieldValue> {
        let key = entity.to_loro_key();
        let components = self.doc.get_map("components");
        let mut result = HashMap::new();
        if let Some(ValueOrContainer::Container(Container::Map(rec))) = components.get(&key) {
            if let Some(ValueOrContainer::Container(Container::Map(cmap))) = rec.get(component) {
                if let LoroValue::Map(fields) = cmap.get_deep_value() {
                    for (fname, fval) in fields.iter() {
                        if let Some(fv) = FieldValue::from_loro(fval) {
                            result.insert(fname.clone(), fv);
                        }
                    }
                }
            }
        }
        result
    }

    /// Rebuild the ECS world from the Loro document (used after merge).
    fn rebuild_ecs_from_loro(&mut self) {
        // Clear all scene entities from ECS
        for (_, ecs_entity) in self.eid_to_ecs.drain() {
            self.world.delete_entity(ecs_entity);
        }
        self.ecs_to_eid.clear();
        self.eid_to_tid.clear();
        self.tid_to_eid_map.clear();
        self.entity_tags.clear();
        self.entity_pairs.clear();

        let tree = self.doc.get_tree("hierarchy");
        let all_nodes = tree.nodes();

        // Collect alive nodes with their eids
        for tid in &all_nodes {
            let Ok(false) = tree.is_node_deleted(tid) else {
                continue;
            };
            let Ok(meta) = tree.get_meta(*tid) else {
                continue;
            };
            let eid_str = meta
                .get("eid")
                .and_then(|v| v.as_value().cloned())
                .and_then(|v| match v {
                    LoroValue::String(s) => Some(s.to_string()),
                    _ => None,
                });
            let Some(eid_str) = eid_str else { continue };
            let Some(eid) = EntityId::from_loro_key(&eid_str) else {
                continue;
            };

            let ecs_entity = self.world.create_entity();
            self.eid_to_ecs.insert(eid, ecs_entity);
            self.ecs_to_eid.insert(ecs_entity, eid);
            self.eid_to_tid.insert(eid, *tid);
            self.tid_to_eid_map.insert(*tid, eid);
            self.entity_tags.insert(eid, HashSet::new());
            self.entity_pairs.insert(eid, HashSet::new());
        }

        // Update id generator to avoid future collisions
        let max_counter = self
            .eid_to_ecs
            .keys()
            .filter(|eid| eid.peer == self.id_gen.peer())
            .map(|eid| eid.counter)
            .max();
        if let Some(mc) = max_counter {
            while self.id_gen.next_id().counter <= mc {}
        }
    }
}

// ── Loro helpers (crate-internal, no public leak) ──────────────────────────

/// Get-or-create a child map, propagating a Loro failure as [`PipelineError::Loro`]. Used on the
/// `apply_*` mutation path (deliverable 4: no `.unwrap()` on a fallible Loro op there).
pub(crate) fn try_child_map(parent: &LoroMap, key: &str) -> Result<LoroMap, PipelineError> {
    match parent.get(key) {
        Some(ValueOrContainer::Container(Container::Map(m))) => Ok(m),
        _ => parent
            .insert_container(key, LoroMap::new())
            .map_err(loro_err),
    }
}

/// Infallible get-or-create for the merge-repair path (merge.rs), where a get-or-create failure
/// would be a Loro-internal bug rather than a recoverable condition. The `apply_*` mutation path
/// uses [`try_child_map`] and propagates instead.
pub(crate) fn child_map(parent: &LoroMap, key: &str) -> LoroMap {
    try_child_map(parent, key).expect("loro get-or-create container (repair path)")
}

/// Map any Loro error into [`PipelineError::Loro`]. Keeps the `apply_*` call sites terse.
fn loro_err<E: std::fmt::Display>(e: E) -> PipelineError {
    PipelineError::Loro(e.to_string())
}

fn get_child_map(parent: &LoroMap, key: &str) -> Option<LoroMap> {
    match parent.get(key) {
        Some(ValueOrContainer::Container(Container::Map(m))) => Some(m),
        _ => None,
    }
}

pub(crate) fn binding_key(from: &EntityId, kind: &str, to: &EntityId) -> String {
    format!("{}|{}|{}", from.to_loro_key(), kind, to.to_loro_key())
}
