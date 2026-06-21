//! The override / variant model on the CRDT (M9.2 / ADR-026) — the save-&-reuse layer.
//!
//! A "part" is a child node with a local transform (a Movable-Tree node). Editing a part stores a
//! **sparse per-field override** on the part's own *mergeable* slot (the [`Op::SetOverride`] /
//! [`Op::SetActive`] ops in [`crate::pipeline`]) — never a destructive rewrite. This module adds the
//! reuse layer on top of that:
//!
//! - **[`Composition`]** — "save this character for reuse" = snapshot the subtree's **resolved**
//!   state (base ⊕ overrides baked in) into a portable, pre-componentized asset
//!   ([`Engine::save_composition`]). (`doc.fork()` / [`Engine::fork_doc`] is the doc-level Loro
//!   alternative; a subtree snapshot is the precise "save *this* character" form.) Re-instantiating it
//!   ([`Engine::instantiate_composition`]) drops a fresh, independently-id'd instance that arrives
//!   already as entities + components — and keeps a link back to the composition it came from.
//! - **[`Variant`]** — a *named bundle of override ops* keyed by **structural rel-path** (not entity
//!   id), so it re-applies to **any** instance of a composition ([`Engine::apply_variant`] /
//!   [`Engine::capture_variant`]). This is the seed of the G3 pose library and the USD-variant model.
//! - **Resolution** — [`Engine::resolved_instance`] reads each part as base ⊕ override,
//!   override-wins-by-structure, with deactivated parts *flagged*, not dropped.
//!
//! Everything routes through the single commit pipeline ([`Engine::commit`]) — so a part edit, a
//! re-instantiate, and a variant application are each ONE undoable, reload-surviving transaction
//! (invariant 3). Pure Loro/metadata: no per-frame cost, no GPU, portable (ADR-006).

use crate::entity_id::EntityId;
use crate::pipeline::{Engine, FieldValue, Op, PipelineError};
use metrocalk_ecs::World;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// The component that records an instance root's link to the [`Composition`] it was instantiated from
/// — the *reference that is never lost* (deliverable 4: save-for-reuse keeps the source link).
/// Excluded from a re-snapshot so provenance doesn't compound.
pub const INSTANCE_META: &str = "__meta__";

/// A reusable, pre-componentized asset: a snapshot of a composed character's subtree with its
/// **resolved** components (the edited state baked in), addressable by id. The nodes are in pre-order
/// (parent before child) so [`Engine::instantiate_composition`] can recreate the tree in one pass.
/// This is the marketplace-able "edited variant" asset (ADR-015 / deliverable 4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Composition {
    /// A logical id for the asset (content/author-assigned; the marketplace index keys off it).
    pub id: String,
    /// The subtree's nodes, pre-order (index 0 = the root).
    pub nodes: Vec<CompositionNode>,
}

/// One node of a [`Composition`]: its structural rel-path, its parent's index in
/// [`Composition::nodes`] (`None` for the root), its resolved components, and whether it is active.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CompositionNode {
    /// Structural rel-path from the instance root (`""` = root, `"0"` = first child, `"0/1"` = …).
    pub path: String,
    /// Index into [`Composition::nodes`] of this node's parent (`None` = the root).
    pub parent: Option<usize>,
    /// `component → field → value`, deterministically ordered (the resolved base for a fresh instance).
    pub components: BTreeMap<String, BTreeMap<String, FieldValue>>,
    /// Whether this part is active (deactivated parts are preserved, not dropped).
    pub active: bool,
}

/// A named bundle of override ops re-applyable to any instance of a composition (USD variant /
/// Blender pose library). Keyed by **structural rel-path**, so it maps onto a fresh instance's parts.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Variant {
    pub name: String,
    pub ops: Vec<VariantOp>,
    /// The one "active-selection" rel-path a variant may pin (the seed of the G3 pose-library's
    /// "applied to the selection" key). `None` = no pinned selection.
    pub active_selection: Option<String>,
}

/// One override in a [`Variant`], addressed by structural rel-path within the instance.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum VariantOp {
    /// Override a single field on the part at `path`.
    SetField {
        path: String,
        component: String,
        field: String,
        value: FieldValue,
    },
    /// Deactivate the part at `path` (deactivate-not-delete).
    Deactivate { path: String },
}

/// A part's fully-resolved view: base ⊕ override, plus its active flag and structural path.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedNode {
    pub entity: EntityId,
    pub path: String,
    pub active: bool,
    pub components: HashMap<String, HashMap<String, FieldValue>>,
}

/// The parent rel-path of `path`: `"0/1" → "0"`, a top-level `"0" → ""` (root), root `"" → None`.
fn parent_path(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    Some(match path.rsplit_once('/') {
        Some((head, _)) => head.to_string(),
        None => String::new(),
    })
}

impl<W: World> Engine<W> {
    /// Every node of the instance rooted at `root`, in pre-order (parent before child), paired with
    /// its structural rel-path. The rel-path encodes the child-index chain (fractional-index order
    /// from [`Engine::children_of`]) — stable across instances of the same structure, which is what
    /// lets a [`Variant`] re-apply to a fresh instance.
    #[must_use]
    pub fn instance_nodes(&self, root: EntityId) -> Vec<(EntityId, String)> {
        let mut out = Vec::new();
        let mut stack = vec![(root, String::new())];
        while let Some((eid, path)) = stack.pop() {
            out.push((eid, path.clone()));
            let children = self.children_of(eid);
            // Push reversed so child 0 is processed first → stable pre-order in `out`.
            for (i, c) in children.iter().enumerate().rev() {
                let cpath = if path.is_empty() {
                    i.to_string()
                } else {
                    format!("{path}/{i}")
                };
                stack.push((*c, cpath));
            }
        }
        out
    }

    /// The entity at a structural rel-path within the instance rooted at `root`, or `None` if the
    /// path doesn't resolve (a structurally different instance — the variant op is then skipped).
    #[must_use]
    pub fn entity_at_path(&self, root: EntityId, path: &str) -> Option<EntityId> {
        if path.is_empty() {
            return Some(root);
        }
        let mut cur = root;
        for seg in path.split('/') {
            let idx: usize = seg.parse().ok()?;
            cur = *self.children_of(cur).get(idx)?;
        }
        Some(cur)
    }

    /// **Save this character for reuse** (deliverable 3): snapshot the subtree rooted at `root` into a
    /// reusable [`Composition`], baking in the **resolved** state (base ⊕ current overrides) so the
    /// *edited* composition is what gets saved. The provenance [`INSTANCE_META`] component is excluded
    /// so a re-snapshot doesn't compound source links. Pure read — no commit.
    #[must_use]
    pub fn save_composition(&self, root: EntityId, id: &str) -> Composition {
        let nodes = self.instance_nodes(root);
        let path_to_index: HashMap<&str, usize> = nodes
            .iter()
            .enumerate()
            .map(|(i, (_, p))| (p.as_str(), i))
            .collect();
        let comp_nodes = nodes
            .iter()
            .map(|(eid, path)| {
                let parent =
                    parent_path(path).and_then(|pp| path_to_index.get(pp.as_str()).copied());
                let mut components: BTreeMap<String, BTreeMap<String, FieldValue>> =
                    BTreeMap::new();
                for (comp, fields) in self.resolved_components(*eid) {
                    if comp == INSTANCE_META {
                        continue;
                    }
                    components.insert(comp, fields.into_iter().collect());
                }
                CompositionNode {
                    path: path.clone(),
                    parent,
                    components,
                    active: self.is_active(*eid),
                }
            })
            .collect();
        Composition {
            id: id.to_string(),
            nodes: comp_nodes,
        }
    }

    /// Re-instantiate a [`Composition`] as a fresh, independently-id'd instance subtree — **pre-
    /// componentized** (the composition's resolved components become the new instance's base) and
    /// linked back via [`INSTANCE_META`] — in ONE undoable transaction. Returns the instance root.
    ///
    /// # Errors
    /// [`PipelineError`] if the create transaction fails, or if the composition has no root node.
    pub fn instantiate_composition(
        &mut self,
        comp: &Composition,
    ) -> Result<EntityId, PipelineError> {
        if comp.nodes.is_empty() {
            return Err(PipelineError::Loro(
                "instantiate of an empty composition".into(),
            ));
        }
        let mut ids: Vec<EntityId> = Vec::with_capacity(comp.nodes.len());
        let mut ops: Vec<Op> = Vec::new();
        for node in &comp.nodes {
            let id = self.alloc_entity_id();
            ids.push(id);
            let parent = node.parent.map(|pi| ids[pi]);
            ops.push(Op::CreateEntity { id, parent });
            for (component, fields) in &node.components {
                for (field, value) in fields {
                    ops.push(Op::SetField {
                        entity: id,
                        component: component.clone(),
                        field: field.clone(),
                        value: value.clone(),
                    });
                }
            }
            if !node.active {
                ops.push(Op::SetActive {
                    entity: id,
                    active: false,
                });
            }
        }
        let root = ids[0];
        ops.push(Op::SetField {
            entity: root,
            component: INSTANCE_META.into(),
            field: "composition".into(),
            value: FieldValue::Str(comp.id.clone()),
        });
        self.commit("instantiate-composition", ops)?;
        Ok(root)
    }

    /// The composition id an instance root was instantiated from (the preserved link), or `None`.
    #[must_use]
    pub fn composition_of(&self, instance_root: EntityId) -> Option<String> {
        match self.get_field(instance_root, INSTANCE_META, "composition") {
            Some(FieldValue::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// Apply a named [`Variant`] onto the instance rooted at `instance_root` — each op overlaid as a
    /// sparse override (override-wins) or a deactivation, mapped from rel-path to the concrete part.
    /// ONE undoable transaction. Ops whose rel-path doesn't resolve in this instance are skipped.
    ///
    /// # Errors
    /// [`PipelineError`] if the override transaction fails.
    pub fn apply_variant(
        &mut self,
        instance_root: EntityId,
        variant: &Variant,
    ) -> Result<(), PipelineError> {
        let mut ops = Vec::new();
        for vop in &variant.ops {
            match vop {
                VariantOp::SetField {
                    path,
                    component,
                    field,
                    value,
                } => {
                    if let Some(eid) = self.entity_at_path(instance_root, path) {
                        ops.push(Op::SetOverride {
                            entity: eid,
                            component: component.clone(),
                            field: field.clone(),
                            value: value.clone(),
                        });
                    }
                }
                VariantOp::Deactivate { path } => {
                    if let Some(eid) = self.entity_at_path(instance_root, path) {
                        ops.push(Op::SetActive {
                            entity: eid,
                            active: false,
                        });
                    }
                }
            }
        }
        self.commit(&format!("apply-variant:{}", variant.name), ops)
    }

    /// Capture the instance's **current** override layer as a portable, re-applyable [`Variant`]
    /// (the "save this edit as a named variant" path) — every field override + every deactivation,
    /// keyed by structural rel-path, deterministically ordered.
    #[must_use]
    pub fn capture_variant(&self, instance_root: EntityId, name: &str) -> Variant {
        let mut ops = Vec::new();
        for (entity, path) in self.instance_nodes(instance_root) {
            if !self.is_active(entity) {
                ops.push(VariantOp::Deactivate { path: path.clone() });
            }
            let mut keys: Vec<String> = self.overrides_of(entity).into_keys().collect();
            keys.sort();
            for key in keys {
                if let Some((comp, field)) = key.split_once('\u{1f}') {
                    if let Some(value) = self.get_override(entity, comp, field) {
                        ops.push(VariantOp::SetField {
                            path: path.clone(),
                            component: comp.to_string(),
                            field: field.to_string(),
                            value,
                        });
                    }
                }
            }
        }
        Variant {
            name: name.to_string(),
            ops,
            active_selection: None,
        }
    }

    /// The fully-resolved view of every part in the instance — base ⊕ override (override-wins) + the
    /// active flag — what a renderer/inspector/projection should read for an instanced character.
    #[must_use]
    pub fn resolved_instance(&self, root: EntityId) -> Vec<ResolvedNode> {
        self.instance_nodes(root)
            .into_iter()
            .map(|(entity, path)| ResolvedNode {
                entity,
                active: self.is_active(entity),
                components: self.resolved_components(entity),
                path,
            })
            .collect()
    }
}
