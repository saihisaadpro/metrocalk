//! **The hierarchy evaluator.** One place that answers "where is this object in the world", with
//! targeted invalidation so that moving one object costs one object's worth of work.
//!
//! Three design decisions, each fixing a specific failure:
//!
//! 1. **Compose matrices; decompose at most once.** `world = parent_world · local` is exact matrix
//!    arithmetic. An evaluator that decomposes to TRS at every level (as a naive recursive
//!    `global_transform` does) throws away shear at each step and accumulates error with depth — a
//!    part six levels down under rotated, non-uniformly-scaled parents ends up visibly off its
//!    parent. Here a TRS is produced only when someone asks for one, from the fully composed matrix.
//!
//! 2. **Version-stamped caches, not eager propagation.** Every node carries a `world_version` that
//!    only advances when its world matrix actually *changes*. Setting one object's X marks that
//!    object dirty; evaluation walks its subtree and stops descending the moment it reaches a node
//!    whose inputs are unchanged. Unrelated branches are never touched, and downstream consumers
//!    (GPU upload, world bounds, the spatial index) compare a `u64` instead of re-deriving.
//!
//! 3. **Iteration, not recursion.** Every traversal here uses an explicit stack. A recursive
//!    evaluator overflows the stack on a deep imported assembly, and a stack overflow aborts the
//!    process — there is no catching it.

use crate::bounds::Aabb;
use crate::epsilon;
use crate::transform::{mat_mul, Decomposed, Mat4, Quat, Transform, Vec3};

/// A handle to a node. Carries a generation so a stale handle to a removed node is *detected*
/// rather than silently addressing whatever was allocated in its place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

impl NodeId {
    /// The raw slot index — for building parallel arrays (render instances, BVH primitives).
    #[must_use]
    pub const fn index(self) -> u32 {
        self.index
    }
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// What happens to a node's world placement when its parent changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReparentMode {
    /// The object does not move on screen: its local transform is recomputed against the new parent.
    /// This is what dragging a row in an outliner should do.
    PreserveWorld,
    /// The local transform is kept verbatim, so the object jumps to the same offset relative to its
    /// new parent. Correct when assembling a rig from parts authored in a shared local space.
    PreserveLocal,
}

/// Per-node policy the picker and the renderer both read, so they cannot disagree about whether
/// something is visible or selectable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeFlags {
    /// Drawn, and (subject to `selectable`) pickable. Inherited: hiding a parent hides the subtree.
    pub visible: bool,
    /// Rendered and pickable, but refuses transform edits. Inherited.
    pub locked: bool,
    /// Participates in viewport picking at all. A helper/annotation sets this false while staying
    /// visible. NOT inherited — a selectable child of a non-selectable group is still selectable.
    pub selectable: bool,
}

impl Default for NodeFlags {
    fn default() -> Self {
        Self {
            visible: true,
            locked: false,
            selectable: true,
        }
    }
}

#[derive(Clone, Debug)]
struct Slot {
    generation: u32,
    alive: bool,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    depth: u32,
    local: Transform,
    flags: NodeFlags,
    /// Geometry bounds in the node's own local space (`None` = no geometry of its own).
    local_bounds: Option<Aabb>,

    // ── caches ────────────────────────────────────────────────────────────────────────────────────
    world: Mat4,
    /// Advances only when `world` actually changes. Consumers compare this instead of re-deriving.
    world_version: u64,
    /// The parent's `world_version` this node's cache was built against.
    seen_parent_version: u64,
    dirty: bool,
    cached_world_bounds: Option<Aabb>,
    bounds_version: u64,
}

const IDENTITY_MATRIX: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// The scene's transform hierarchy.
#[derive(Clone, Debug, Default)]
pub struct TransformGraph {
    slots: Vec<Slot>,
    free: Vec<u32>,
    roots: Vec<NodeId>,
    dirty: Vec<NodeId>,
    clock: u64,
    /// Advances on any structural change (create/remove/reparent) — lets a consumer notice that its
    /// node list is stale without diffing it.
    structure_version: u64,
}

impl TransformGraph {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    // ── structure ─────────────────────────────────────────────────────────────────────────────────

    /// Add a node. A non-finite `local` is rejected and replaced by identity — see
    /// [`Transform::sanitized`]; the alternative is a NaN that silently deletes a subtree from view.
    pub fn create(&mut self, parent: Option<NodeId>, local: Transform) -> NodeId {
        let local = local.sanitized().unwrap_or(Transform::IDENTITY);
        let parent = parent.filter(|p| self.is_alive(*p));
        let depth = parent.map_or(0, |p| self.slots[p.index as usize].depth + 1);
        let id = if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.generation = slot.generation.wrapping_add(1);
            slot.alive = true;
            slot.parent = parent;
            slot.children.clear();
            slot.depth = depth;
            slot.local = local;
            slot.flags = NodeFlags::default();
            slot.local_bounds = None;
            slot.world = IDENTITY_MATRIX;
            slot.world_version = 0;
            slot.seen_parent_version = u64::MAX; // force the first evaluation
            slot.dirty = true;
            slot.cached_world_bounds = None;
            slot.bounds_version = u64::MAX;
            NodeId {
                index,
                generation: slot.generation,
            }
        } else {
            let index = u32::try_from(self.slots.len()).expect("node count fits in u32");
            self.slots.push(Slot {
                generation: 0,
                alive: true,
                parent,
                children: Vec::new(),
                depth,
                local,
                flags: NodeFlags::default(),
                local_bounds: None,
                world: IDENTITY_MATRIX,
                world_version: 0,
                seen_parent_version: u64::MAX,
                dirty: true,
                cached_world_bounds: None,
                bounds_version: u64::MAX,
            });
            NodeId {
                index,
                generation: 0,
            }
        };
        match parent {
            Some(p) => self.slots[p.index as usize].children.push(id),
            None => self.roots.push(id),
        }
        self.dirty.push(id);
        self.structure_version += 1;
        id
    }

    /// Remove a node and its whole subtree. Returns the ids removed (so a caller can drop the
    /// matching render/BVH entries without re-scanning).
    pub fn remove(&mut self, id: NodeId) -> Vec<NodeId> {
        if !self.is_alive(id) {
            return Vec::new();
        }
        self.detach_from_parent(id);
        let mut removed = Vec::new();
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let i = n.index as usize;
            if !self.slots[i].alive || self.slots[i].generation != n.generation {
                continue;
            }
            stack.extend(self.slots[i].children.iter().copied());
            self.slots[i].alive = false;
            self.slots[i].children.clear();
            self.slots[i].parent = None;
            self.slots[i].local_bounds = None;
            self.free.push(n.index);
            removed.push(n);
        }
        self.structure_version += 1;
        removed
    }

    /// Move `id` under `new_parent`.
    ///
    /// Refuses to create a cycle (parenting a node to its own descendant, or to itself) — a cycle in
    /// a transform graph is not a wrong answer, it is a hang. Returns `false` if refused.
    pub fn reparent(&mut self, id: NodeId, new_parent: Option<NodeId>, mode: ReparentMode) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        if let Some(p) = new_parent {
            if !self.is_alive(p) || p == id || self.is_ancestor_of(id, p) {
                return false;
            }
        }
        let world_before =
            matches!(mode, ReparentMode::PreserveWorld).then(|| self.world_matrix(id));
        self.detach_from_parent(id);
        self.slots[id.index as usize].parent = new_parent;
        match new_parent {
            Some(p) => self.slots[p.index as usize].children.push(id),
            None => self.roots.push(id),
        }
        self.refresh_depths(id);
        self.mark_dirty(id);
        self.structure_version += 1;
        if let Some(world) = world_before {
            // Recompute the local transform so the object does not move. The parent chain above `id`
            // is already evaluated (the world read above forced it), so this is exact.
            let parent_world = new_parent.map_or(IDENTITY_MATRIX, |p| self.world_matrix(p));
            let world_t = Transform::decompose(world).transform;
            if let Some(local) = Transform::to_local(&world_t, parent_world) {
                self.set_local(id, local.transform);
            }
        }
        true
    }

    fn detach_from_parent(&mut self, id: NodeId) {
        match self.slots[id.index as usize].parent {
            Some(p) => {
                let kids = &mut self.slots[p.index as usize].children;
                if let Some(pos) = kids.iter().position(|c| *c == id) {
                    kids.remove(pos);
                }
            }
            None => {
                if let Some(pos) = self.roots.iter().position(|c| *c == id) {
                    self.roots.remove(pos);
                }
            }
        }
    }

    fn refresh_depths(&mut self, root: NodeId) {
        let base = self.slots[root.index as usize]
            .parent
            .map_or(0, |p| self.slots[p.index as usize].depth + 1);
        self.slots[root.index as usize].depth = base;
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            let d = self.slots[n.index as usize].depth;
            let kids = self.slots[n.index as usize].children.clone();
            for k in kids {
                self.slots[k.index as usize].depth = d + 1;
                stack.push(k);
            }
        }
    }

    // ── queries ───────────────────────────────────────────────────────────────────────────────────

    #[must_use]
    pub fn is_alive(&self, id: NodeId) -> bool {
        self.slots
            .get(id.index as usize)
            .is_some_and(|s| s.alive && s.generation == id.generation)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.alive).count()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.slots.get(id.index as usize).and_then(|s| s.parent)
    }

    #[must_use]
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        self.slots
            .get(id.index as usize)
            .map_or(&[], |s| s.children.as_slice())
    }

    #[must_use]
    pub fn roots(&self) -> &[NodeId] {
        &self.roots
    }

    #[must_use]
    pub fn depth(&self, id: NodeId) -> u32 {
        self.slots.get(id.index as usize).map_or(0, |s| s.depth)
    }

    #[must_use]
    pub fn structure_version(&self) -> u64 {
        self.structure_version
    }

    /// Every live node, in slot order.
    pub fn iter(&self) -> impl Iterator<Item = NodeId> + '_ {
        self.slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.alive)
            .map(|(i, s)| NodeId {
                index: u32::try_from(i).unwrap_or(u32::MAX),
                generation: s.generation,
            })
    }

    /// Whether `ancestor` is on `node`'s parent chain (used to refuse cycles, and to test whether a
    /// selection change should highlight a subtree).
    #[must_use]
    pub fn is_ancestor_of(&self, ancestor: NodeId, node: NodeId) -> bool {
        let mut cur = self.parent(node);
        // Bounded by the node count so a corrupted graph cannot hang the editor.
        for _ in 0..self.slots.len() {
            match cur {
                Some(p) if p == ancestor => return true,
                Some(p) => cur = self.parent(p),
                None => return false,
            }
        }
        false
    }

    /// The node's subtree, pre-order (a node immediately followed by its whole subtree).
    #[must_use]
    pub fn subtree(&self, root: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        if !self.is_alive(root) {
            return out;
        }
        let mut stack = vec![root];
        while let Some(n) = stack.pop() {
            out.push(n);
            let kids = &self.slots[n.index as usize].children;
            stack.extend(kids.iter().rev().copied());
        }
        out
    }

    // ── local transform ───────────────────────────────────────────────────────────────────────────

    #[must_use]
    pub fn local(&self, id: NodeId) -> Transform {
        self.slots
            .get(id.index as usize)
            .map_or(Transform::IDENTITY, |s| s.local)
    }

    /// Set the local transform. Non-finite input is **refused** (returns `false`) rather than written.
    pub fn set_local(&mut self, id: NodeId, local: Transform) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        let Some(clean) = local.sanitized() else {
            return false;
        };
        if self.slots[id.index as usize].local == clean {
            return true; // no-op: do not dirty the subtree for a write that changes nothing
        }
        self.slots[id.index as usize].local = clean;
        self.mark_dirty(id);
        true
    }

    pub fn set_local_position(&mut self, id: NodeId, p: Vec3) -> bool {
        let mut t = self.local(id);
        t.translation = p;
        self.set_local(id, t)
    }

    pub fn set_local_rotation(&mut self, id: NodeId, r: Quat) -> bool {
        let mut t = self.local(id);
        t.rotation = r;
        self.set_local(id, t)
    }

    pub fn set_local_scale(&mut self, id: NodeId, s: Vec3) -> bool {
        let mut t = self.local(id);
        t.scale = s;
        self.set_local(id, t)
    }

    // ── world transform ───────────────────────────────────────────────────────────────────────────

    /// The node's world matrix. Evaluates any pending work first, so a read is never stale — the
    /// caller cannot forget to flush.
    pub fn world_matrix(&mut self, id: NodeId) -> Mat4 {
        self.evaluate();
        self.slots
            .get(id.index as usize)
            .map_or(IDENTITY_MATRIX, |s| s.world)
    }

    /// The node's world transform as a TRS, with the fidelity of that conversion attached (shear from
    /// a rotated + non-uniformly-scaled ancestor is reported, not hidden).
    pub fn world_transform(&mut self, id: NodeId) -> Decomposed {
        Transform::decompose(self.world_matrix(id))
    }

    /// The node's world-space position — the number the inspector's world X/Y/Z shows.
    pub fn world_position(&mut self, id: NodeId) -> Vec3 {
        let w = self.world_matrix(id);
        [w[3][0], w[3][1], w[3][2]]
    }

    /// The parent's world matrix (identity for a root) — the input to every world→local conversion.
    pub fn parent_world_matrix(&mut self, id: NodeId) -> Mat4 {
        match self.parent(id) {
            Some(p) => self.world_matrix(p),
            None => IDENTITY_MATRIX,
        }
    }

    /// A monotonic stamp that advances only when this node's world matrix changes.
    pub fn world_version(&mut self, id: NodeId) -> u64 {
        self.evaluate();
        self.slots
            .get(id.index as usize)
            .map_or(0, |s| s.world_version)
    }

    /// **Set the world transform**, converting to parent space internally.
    ///
    /// This is the API the gizmo and the world-space numeric fields use, and the reason callers never
    /// hand-invert a parent matrix. Returns `false` when the value is non-finite or the parent chain
    /// is singular — in which case nothing is written.
    pub fn set_world_transform(&mut self, id: NodeId, world: Transform) -> bool {
        if !self.is_alive(id) || world.sanitized().is_none() {
            return false;
        }
        let parent_world = self.parent_world_matrix(id);
        let Some(local) = Transform::to_local(&world, parent_world) else {
            return false;
        };
        self.set_local(id, local.transform)
    }

    /// Set the world matrix directly (used when the source really is a matrix, e.g. an importer).
    pub fn set_world_matrix(&mut self, id: NodeId, world: Mat4) -> bool {
        self.set_world_transform(id, Transform::decompose(world).transform)
    }

    /// Move the node so its world position is exactly `p`, leaving its world rotation and scale
    /// alone. **World X means world X**, at any hierarchy depth, under any parent transform.
    pub fn set_world_position(&mut self, id: NodeId, p: Vec3) -> bool {
        if !self.is_alive(id) || !epsilon::all_finite(&p) {
            return false;
        }
        let mut w = self.world_transform(id).transform;
        w.translation = p;
        self.set_world_transform(id, w)
    }

    /// Set the node's world orientation, leaving its world position and scale alone.
    pub fn set_world_rotation(&mut self, id: NodeId, r: Quat) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        let mut w = self.world_transform(id).transform;
        w.rotation = r;
        self.set_world_transform(id, w)
    }

    /// Set the node's world scale, leaving its world position and orientation alone.
    pub fn set_world_scale(&mut self, id: NodeId, s: Vec3) -> bool {
        if !self.is_alive(id) {
            return false;
        }
        let mut w = self.world_transform(id).transform;
        w.scale = s;
        self.set_world_transform(id, w)
    }

    // ── flags ─────────────────────────────────────────────────────────────────────────────────────

    #[must_use]
    pub fn flags(&self, id: NodeId) -> NodeFlags {
        self.slots
            .get(id.index as usize)
            .map_or(NodeFlags::default(), |s| s.flags)
    }

    pub fn set_flags(&mut self, id: NodeId, flags: NodeFlags) {
        if let Some(s) = self.slots.get_mut(id.index as usize) {
            s.flags = flags;
        }
    }

    /// Visible after inheritance — a node inside a hidden group is not visible even if its own flag
    /// says so. The renderer and the picker both ask this, which is what keeps "I can click something
    /// I cannot see" from happening.
    #[must_use]
    pub fn is_visible(&self, id: NodeId) -> bool {
        self.inherited(id, |f| f.visible)
    }

    /// Locked after inheritance — locking a group locks its parts against transform edits.
    #[must_use]
    pub fn is_locked(&self, id: NodeId) -> bool {
        !self.inherited(id, |f| !f.locked)
    }

    /// Pickable: visible (inherited) and selectable (own flag only — a helper group containing real
    /// geometry should not make that geometry unpickable).
    #[must_use]
    pub fn is_pickable(&self, id: NodeId) -> bool {
        self.is_visible(id) && self.flags(id).selectable
    }

    fn inherited(&self, id: NodeId, pred: impl Fn(NodeFlags) -> bool) -> bool {
        let mut cur = Some(id);
        for _ in 0..=self.slots.len() {
            let Some(n) = cur else { return true };
            let Some(s) = self.slots.get(n.index as usize) else {
                return true;
            };
            if !pred(s.flags) {
                return false;
            }
            cur = s.parent;
        }
        true
    }

    // ── bounds ────────────────────────────────────────────────────────────────────────────────────

    #[must_use]
    pub fn local_bounds(&self, id: NodeId) -> Option<Aabb> {
        self.slots
            .get(id.index as usize)
            .and_then(|s| s.local_bounds)
    }

    pub fn set_local_bounds(&mut self, id: NodeId, bounds: Option<Aabb>) {
        if let Some(s) = self.slots.get_mut(id.index as usize) {
            s.local_bounds = bounds.filter(|b| b.is_finite() && !b.is_empty());
            s.bounds_version = u64::MAX; // force recompute
        }
    }

    /// The node's own geometry bounds in world space, cached against `world_version` so a static
    /// object's bounds are computed once no matter how many times they are queried.
    pub fn world_bounds(&mut self, id: NodeId) -> Option<Aabb> {
        self.evaluate();
        let i = id.index as usize;
        let (wv, local) = {
            let s = self.slots.get(i)?;
            if !s.alive {
                return None;
            }
            (s.world_version, s.local_bounds?)
        };
        if self.slots[i].bounds_version == wv {
            return self.slots[i].cached_world_bounds;
        }
        let world = local.transformed(self.slots[i].world);
        let out = (!world.is_empty()).then_some(world);
        self.slots[i].cached_world_bounds = out;
        self.slots[i].bounds_version = wv;
        out
    }

    /// Bounds enclosing the node and everything under it — what "frame selected" on a group needs.
    pub fn subtree_bounds(&mut self, id: NodeId) -> Aabb {
        self.evaluate();
        let mut out = Aabb::EMPTY;
        for n in self.subtree(id) {
            if let Some(b) = self.world_bounds(n) {
                out.expand(&b);
            }
        }
        out
    }

    /// Bounds enclosing every visible node with geometry — the "frame all" input.
    pub fn scene_bounds(&mut self) -> Aabb {
        self.evaluate();
        let ids: Vec<NodeId> = self.iter().collect();
        let mut out = Aabb::EMPTY;
        for id in ids {
            if !self.is_visible(id) {
                continue;
            }
            if let Some(b) = self.world_bounds(id) {
                out.expand(&b);
            }
        }
        out
    }

    // ── evaluation ────────────────────────────────────────────────────────────────────────────────

    /// Whether any world matrix is stale. Exposed so a profiler/diagnostic can report how much work a
    /// frame's evaluation had queued.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.dirty.len()
    }

    fn mark_dirty(&mut self, id: NodeId) {
        if let Some(s) = self.slots.get_mut(id.index as usize) {
            if !s.dirty {
                s.dirty = true;
                self.dirty.push(id);
            }
        }
    }

    /// Bring every world matrix up to date. Cheap and idempotent when nothing changed.
    ///
    /// Dirty seeds are processed shallowest-first so a node's parent is always current before the
    /// node is recomputed, and a subtree walk stops descending as soon as it meets a node whose
    /// inputs are unchanged — which is why editing a leaf does not cost a scene-wide sweep.
    pub fn evaluate(&mut self) {
        if self.dirty.is_empty() {
            return;
        }
        let mut seeds = std::mem::take(&mut self.dirty);
        seeds.sort_by_key(|id| self.slots.get(id.index as usize).map_or(0, |s| s.depth));
        let mut stack: Vec<NodeId> = Vec::new();
        for seed in seeds {
            if !self.is_alive(seed) {
                continue;
            }
            stack.push(seed);
            while let Some(n) = stack.pop() {
                let i = n.index as usize;
                if !self.slots[i].alive {
                    continue;
                }
                let parent = self.slots[i].parent;
                let (parent_world, parent_version) = match parent {
                    Some(p) => {
                        let ps = &self.slots[p.index as usize];
                        (ps.world, ps.world_version)
                    }
                    None => (IDENTITY_MATRIX, 0),
                };
                let unchanged =
                    !self.slots[i].dirty && self.slots[i].seen_parent_version == parent_version;
                if unchanged {
                    continue; // this node and therefore its whole subtree are already current
                }
                let world = mat_mul(parent_world, self.slots[i].local.to_matrix());
                let changed = world != self.slots[i].world;
                let s = &mut self.slots[i];
                s.world = world;
                s.seen_parent_version = parent_version;
                s.dirty = false;
                if changed {
                    self.clock += 1;
                    self.slots[i].world_version = self.clock;
                }
                stack.extend(self.slots[i].children.iter().copied());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::{approx_eq, approx_eq_vec3};
    use crate::transform::quat_approx_eq;

    fn deg(d: f64) -> f64 {
        d.to_radians()
    }

    /// An awkward parent: translated far out, arbitrarily rotated, and scaled. Every hierarchy test
    /// below runs against this rather than an axis-aligned unit parent, because an identity-ish
    /// parent hides exactly the bugs these tests exist to catch.
    ///
    /// Its scale is **uniform** on purpose. A rotated parent with non-uniform scale composes to a
    /// matrix containing shear, and shear has no TRS representation at all — so "the child's world
    /// transform is exactly X" is not a claim any TRS scene graph can make there. That case has its
    /// own test ([`shear_from_a_rotated_non_uniform_parent_is_reported_not_hidden`]) which asserts
    /// what *is* true rather than pretending the limitation away.
    fn awkward_parent() -> Transform {
        Transform {
            translation: [100.0, 20.0, -80.0],
            rotation: Transform::from_axis_angle([0.3, 0.8, -0.5], deg(57.0)),
            scale: [2.5, 2.5, 2.5],
        }
    }

    #[test]
    fn editing_a_deep_childs_world_x_places_it_at_that_world_x() {
        // The brief's headline hierarchy case. Four levels deep, under a rotated + non-uniformly
        // scaled parent: setting world X must mean world X, not "local X of something rotated".
        let mut g = TransformGraph::new();
        let a = g.create(None, awkward_parent());
        let b = g.create(
            Some(a),
            Transform {
                translation: [3.0, -1.0, 2.0],
                rotation: Transform::from_axis_angle([1.0, 0.0, 0.0], deg(31.0)),
                scale: [1.5, 1.5, 1.5],
            },
        );
        let c = g.create(
            Some(b),
            Transform {
                translation: [-2.0, 5.0, 0.5],
                rotation: Transform::from_axis_angle([0.0, 1.0, 1.0], deg(-80.0)),
                scale: [0.25, 2.0, 1.0],
            },
        );
        let d = g.create(Some(c), Transform::from_translation([7.0, 7.0, 7.0]));

        let before = g.world_position(d);
        assert!(g.set_world_position(d, [42.0, before[1], before[2]]));
        let after = g.world_position(d);
        assert!(
            approx_eq(after[0], 42.0),
            "world X must land exactly on 42 (got {})",
            after[0]
        );
        assert!(
            approx_eq(after[1], before[1]) && approx_eq(after[2], before[2]),
            "the untouched axes must not move ({before:?} -> {after:?})"
        );
    }

    #[test]
    fn world_rotation_and_scale_setters_leave_the_other_channels_alone() {
        let mut g = TransformGraph::new();
        let p = g.create(None, awkward_parent());
        let c = g.create(
            Some(p),
            Transform {
                translation: [1.0, 2.0, 3.0],
                rotation: Transform::from_axis_angle([1.0, 1.0, 0.0], deg(20.0)),
                scale: [1.0; 3],
            },
        );
        let pos = g.world_position(c);
        let want = Transform::from_axis_angle([0.0, 0.0, 1.0], deg(90.0));
        assert!(g.set_world_rotation(c, want));
        let got = g.world_transform(c);
        assert!(
            quat_approx_eq(got.transform.rotation, want),
            "world rotation applied exactly"
        );
        assert!(
            approx_eq_vec3(g.world_position(c), pos),
            "position untouched by a rotation edit"
        );

        assert!(g.set_world_scale(c, [4.0, 4.0, 4.0]));
        let got = g.world_transform(c);
        for i in 0..3 {
            assert!(
                approx_eq(got.transform.scale[i].abs(), 4.0),
                "world scale applied exactly"
            );
        }
        assert!(
            approx_eq_vec3(g.world_position(c), pos),
            "position untouched by a scale edit"
        );
    }

    #[test]
    fn reparent_preserve_world_does_not_move_the_object() {
        let mut g = TransformGraph::new();
        let a = g.create(None, awkward_parent());
        let b = g.create(
            None,
            Transform {
                translation: [-40.0, 12.0, 6.0],
                rotation: Transform::from_axis_angle([0.1, -0.9, 0.4], deg(-120.0)),
                scale: [0.3, 0.3, 0.3],
            },
        );
        let child = g.create(Some(a), Transform::from_translation([2.0, 0.0, 1.0]));

        let world_before = g.world_matrix(child);
        assert!(g.reparent(child, Some(b), ReparentMode::PreserveWorld));
        let world_after = g.world_matrix(child);
        for col in 0..4 {
            for row in 0..4 {
                assert!(
                    approx_eq(world_before[col][row], world_after[col][row]),
                    "preserve-world must be a no-op on screen at [{col}][{row}]: {} vs {}",
                    world_before[col][row],
                    world_after[col][row]
                );
            }
        }
        assert_eq!(g.parent(child), Some(b));
    }

    #[test]
    fn shear_from_a_rotated_non_uniform_parent_is_reported_not_hidden() {
        // The one hierarchy case a TRS scene graph genuinely cannot represent exactly. A parent that
        // is BOTH rotated and non-uniformly scaled composes to a matrix with shear; TRS has no shear
        // component, so a child's world transform can only be approximated.
        //
        // Every mainstream engine has this limitation. What is not universal — and is the point of
        // this test — is *saying so*: the position stays exact, and the decomposition reports the
        // shear it discarded so a caller can warn instead of silently drifting.
        let mut g = TransformGraph::new();
        let parent = g.create(
            None,
            Transform {
                translation: [10.0, 0.0, 0.0],
                rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], deg(45.0)),
                scale: [4.0, 1.0, 1.0], // rotated AND non-uniform ⇒ shear
            },
        );
        let child = g.create(
            Some(parent),
            Transform {
                translation: [1.0, 2.0, 0.0],
                rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], deg(30.0)),
                scale: [1.0; 3],
            },
        );

        let world = g.world_transform(child);
        assert!(
            !world.is_exact(),
            "the composition really does shear, and decompose says so ({})",
            world.shear
        );

        // Position is still exactly settable — the translation column is unaffected by shear, which
        // is why world X/Y/Z numeric editing stays trustworthy even here.
        assert!(g.set_world_position(child, [42.0, -7.0, 3.0]));
        let p = g.world_position(child);
        assert!(
            approx_eq_vec3(p, [42.0, -7.0, 3.0]),
            "world position is exact even under shear (got {p:?})"
        );
        // And nothing became non-finite.
        let m = g.world_matrix(child);
        assert!(m.iter().all(|c| epsilon::all_finite(c)));
    }

    #[test]
    fn reparent_preserve_local_keeps_the_local_and_moves_the_object() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::from_translation([10.0, 0.0, 0.0]));
        let b = g.create(None, Transform::from_translation([-10.0, 0.0, 0.0]));
        let child = g.create(Some(a), Transform::from_translation([1.0, 0.0, 0.0]));
        assert_eq!(g.world_position(child), [11.0, 0.0, 0.0]);
        assert!(g.reparent(child, Some(b), ReparentMode::PreserveLocal));
        assert_eq!(
            g.local(child).translation,
            [1.0, 0.0, 0.0],
            "local kept verbatim"
        );
        assert_eq!(
            g.world_position(child),
            [-9.0, 0.0, 0.0],
            "so it moves with the new parent"
        );
    }

    #[test]
    fn reparenting_into_a_cycle_is_refused_not_hung() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::IDENTITY);
        let b = g.create(Some(a), Transform::IDENTITY);
        let c = g.create(Some(b), Transform::IDENTITY);
        assert!(
            !g.reparent(a, Some(c), ReparentMode::PreserveWorld),
            "a under its own grandchild"
        );
        assert!(
            !g.reparent(a, Some(a), ReparentMode::PreserveWorld),
            "a under itself"
        );
        assert_eq!(g.parent(a), None, "and the graph is unchanged");
        assert!(
            g.reparent(c, None, ReparentMode::PreserveWorld),
            "a legal move still works"
        );
    }

    #[test]
    fn descendants_follow_a_parent_edit() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::IDENTITY);
        let b = g.create(Some(a), Transform::from_translation([1.0, 0.0, 0.0]));
        let c = g.create(Some(b), Transform::from_translation([1.0, 0.0, 0.0]));
        assert_eq!(g.world_position(c), [2.0, 0.0, 0.0]);
        g.set_local_position(a, [10.0, 0.0, 0.0]);
        assert_eq!(
            g.world_position(c),
            [12.0, 0.0, 0.0],
            "grandchild moved with the root"
        );
        // And a rotation of the root swings the grandchild around it.
        g.set_local_rotation(a, Transform::from_axis_angle([0.0, 1.0, 0.0], deg(90.0)));
        let p = g.world_position(c);
        assert!(
            approx_eq(p[0], 10.0) && approx_eq(p[2], -2.0),
            "swung to {p:?}"
        );
    }

    #[test]
    fn dirty_propagation_touches_only_the_affected_branch() {
        // The performance contract as a behavioural test: editing one branch must not advance the
        // world version of an unrelated branch. Version stamps are what downstream consumers (GPU
        // upload, bounds, the spatial index) key their own work off, so a version that moves when it
        // should not is a scene-wide rebuild nobody asked for.
        let mut g = TransformGraph::new();
        let left = g.create(None, Transform::IDENTITY);
        let left_child = g.create(Some(left), Transform::IDENTITY);
        let right = g.create(None, Transform::IDENTITY);
        let right_child = g.create(Some(right), Transform::IDENTITY);
        g.evaluate();

        let (lv, lcv) = (g.world_version(left), g.world_version(left_child));
        let (rv, rcv) = (g.world_version(right), g.world_version(right_child));

        g.set_local_position(left, [5.0, 0.0, 0.0]);
        g.evaluate();

        assert!(g.world_version(left) > lv, "the edited node updated");
        assert!(g.world_version(left_child) > lcv, "and its descendant");
        assert_eq!(g.world_version(right), rv, "the other branch did NOT");
        assert_eq!(g.world_version(right_child), rcv, "nor its descendant");
    }

    #[test]
    fn a_write_that_changes_nothing_does_not_dirty_anything() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::from_translation([1.0, 2.0, 3.0]));
        let b = g.create(Some(a), Transform::IDENTITY);
        g.evaluate();
        let v = g.world_version(b);
        g.set_local_position(a, [1.0, 2.0, 3.0]); // same value
        assert_eq!(g.pending(), 0, "an idempotent write queues no work");
        assert_eq!(g.world_version(b), v);
    }

    #[test]
    fn deep_chains_evaluate_without_recursion() {
        // 20,000 levels. A recursive evaluator overflows the stack here, and a stack overflow aborts
        // the process rather than raising an error anyone can handle.
        let mut g = TransformGraph::new();
        let mut parent = None;
        let mut last = None;
        for _ in 0..20_000 {
            let id = g.create(parent, Transform::from_translation([1.0, 0.0, 0.0]));
            parent = Some(id);
            last = Some(id);
        }
        let leaf = last.expect("chain built");
        assert_eq!(g.depth(leaf), 19_999);
        assert!(approx_eq(g.world_position(leaf)[0], 20_000.0));
        assert_eq!(g.subtree(g.roots()[0]).len(), 20_000);
    }

    #[test]
    fn hierarchy_composition_does_not_drift_with_depth() {
        // Matrix composition, decomposed ONCE at the end. A per-level decompose (the pattern this
        // module exists to replace) visibly drifts by 60 levels under rotated non-uniform parents.
        let mut g = TransformGraph::new();
        let mut parent = None;
        for i in 0..60 {
            let t = Transform {
                translation: [1.0, 0.5, -0.25],
                rotation: Transform::from_axis_angle([0.3, 1.0, 0.7], deg(7.0 + f64::from(i))),
                scale: [1.0, 1.0, 1.0],
            };
            parent = Some(g.create(parent, t));
        }
        let leaf = parent.expect("chain");
        let evaluated = g.world_matrix(leaf);
        // Independent reference: compose the chain by hand, root-down, in one matrix product.
        let mut reference = super::IDENTITY_MATRIX;
        let mut chain = Vec::new();
        let mut cur = Some(leaf);
        while let Some(n) = cur {
            chain.push(n);
            cur = g.parent(n);
        }
        for n in chain.iter().rev() {
            reference = mat_mul(reference, g.local(*n).to_matrix());
        }
        for c in 0..4 {
            for r in 0..4 {
                assert!(
                    approx_eq(evaluated[c][r], reference[c][r]),
                    "60-deep composition matches the reference at [{c}][{r}]: {} vs {}",
                    evaluated[c][r],
                    reference[c][r]
                );
            }
        }
    }

    #[test]
    fn non_finite_writes_are_refused_and_leave_the_graph_intact() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::from_translation([1.0, 2.0, 3.0]));
        let b = g.create(Some(a), Transform::IDENTITY);
        assert!(!g.set_local_position(a, [f64::NAN, 0.0, 0.0]));
        assert!(!g.set_world_position(b, [0.0, f64::INFINITY, 0.0]));
        assert_eq!(
            g.local(a).translation,
            [1.0, 2.0, 3.0],
            "the old value stands"
        );
        assert!(
            epsilon::all_finite(&g.world_position(b)),
            "no NaN reached the world matrix"
        );
    }

    #[test]
    fn a_zero_scaled_ancestor_is_clamped_so_the_subtree_stays_editable() {
        let mut g = TransformGraph::new();
        // `create` clamps a literal zero to MIN_SCALE, so build the degenerate case the way it really
        // arises: a legal tiny scale that is still invertible, and then an explicitly singular matrix.
        let p = g.create(
            None,
            Transform {
                scale: [0.0, 1.0, 1.0],
                ..Transform::IDENTITY
            },
        );
        let c = g.create(Some(p), Transform::IDENTITY);
        assert!(
            g.local(p).scale[0].abs() >= epsilon::MIN_SCALE,
            "the clamp keeps the parent invertible rather than poisoning the subtree"
        );
        assert!(
            g.set_world_position(c, [1.0, 2.0, 3.0]),
            "and the edit therefore succeeds"
        );
        assert!(epsilon::all_finite(&g.world_position(c)));
    }

    #[test]
    fn visibility_and_lock_inherit_but_selectable_does_not() {
        let mut g = TransformGraph::new();
        let group = g.create(None, Transform::IDENTITY);
        let part = g.create(Some(group), Transform::IDENTITY);

        g.set_flags(
            group,
            NodeFlags {
                visible: false,
                ..NodeFlags::default()
            },
        );
        assert!(!g.is_visible(part), "hiding a group hides its parts");
        assert!(!g.is_pickable(part), "and a hidden part is not pickable");

        g.set_flags(
            group,
            NodeFlags {
                locked: true,
                visible: true,
                selectable: false,
            },
        );
        assert!(g.is_locked(part), "locking a group locks its parts");
        assert!(g.is_visible(part));
        assert!(
            g.is_pickable(part),
            "a non-selectable CONTAINER must not make its real geometry unpickable"
        );
    }

    #[test]
    fn world_bounds_are_cached_against_the_world_version() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::IDENTITY);
        g.set_local_bounds(a, Some(Aabb::new([-1.0; 3], [1.0; 3])));
        let first = g.world_bounds(a).expect("bounds");
        assert_eq!(first, Aabb::new([-1.0; 3], [1.0; 3]));
        g.set_local_position(a, [10.0, 0.0, 0.0]);
        let moved = g.world_bounds(a).expect("bounds");
        assert_eq!(moved, Aabb::new([9.0, -1.0, -1.0], [11.0, 1.0, 1.0]));
        // Subtree bounds gather descendants.
        let b = g.create(Some(a), Transform::from_translation([5.0, 0.0, 0.0]));
        g.set_local_bounds(b, Some(Aabb::new([-1.0; 3], [1.0; 3])));
        assert_eq!(g.subtree_bounds(a).max[0], 16.0);
    }

    #[test]
    fn removing_a_subtree_invalidates_its_handles() {
        let mut g = TransformGraph::new();
        let a = g.create(None, Transform::IDENTITY);
        let b = g.create(Some(a), Transform::IDENTITY);
        let c = g.create(Some(b), Transform::IDENTITY);
        let removed = g.remove(b);
        assert_eq!(removed.len(), 2, "b and c");
        assert!(!g.is_alive(b) && !g.is_alive(c));
        assert!(g.is_alive(a));
        assert!(g.children(a).is_empty());
        // A recycled slot gets a new generation, so the stale handle stays dead.
        let fresh = g.create(None, Transform::IDENTITY);
        assert!(g.is_alive(fresh));
        assert!(
            !g.is_alive(b),
            "a stale handle never resurrects as the new node"
        );
    }
}
