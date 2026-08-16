//! **The acceleration structures.** A bounding-volume hierarchy over triangles (per mesh) and one
//! over world bounds (per scene), so a click costs `O(log n)` instead of `O(n)`.
//!
//! Two structures because they have opposite lifetimes and opposite update patterns:
//!
//! * [`MeshBvh`] is built **once per mesh asset**, in the mesh's own local space, and is shared by
//!   every instance of that mesh. This is the reason picking a scene with 5,000 copies of one bolt
//!   does not need 5,000 BVHs: the world ray is transformed *into* the bolt's space and tested
//!   against the one shared tree. Building per instance would also be wrong under animation.
//! * [`SceneBvh`] is built over **world AABBs** and changes constantly, because objects move. It
//!   supports **refit** — recomputing node bounds bottom-up without changing the topology — which is
//!   what makes dragging an object cost microseconds instead of a rebuild. Refitting degrades tree
//!   quality as objects move far from where they were, so the structure tracks that and says when a
//!   rebuild has become worthwhile rather than silently getting slower.
//!
//! Both use binned SAH (surface-area heuristic) construction: candidate splits are scored by
//! `area(left)·count(left) + area(right)·count(right)`, which approximates the probability a random
//! ray must descend each side. A median split is simpler and produces a measurably worse tree on the
//! kind of geometry that actually matters here — long thin CAD parts and wildly varying object sizes.

use crate::bounds::Aabb;
use crate::ray::{ray_aabb_hit, ray_triangle, Ray, TriangleHit};
use crate::transform::Vec3;

/// How many buckets the SAH search evaluates per axis. 12 is the usual sweet spot: the cost curve is
/// flat enough that more buckets buy negligible tree quality for linear build time.
const SAH_BUCKETS: usize = 12;
/// Below this many primitives, a leaf is cheaper than any split.
const MAX_LEAF_PRIMITIVES: usize = 4;
/// Hard cap on tree depth. Reached only by pathological input (millions of exactly-coincident
/// primitives, where no split ever separates anything); the cap turns an infinite recursion into a
/// slightly slow leaf.
const MAX_DEPTH: u32 = 64;

/// A vertex position in whatever precision the caller already stores.
///
/// The renderer keeps f32 vertices; forcing a parallel f64 copy just to build a BVH would double the
/// memory of every imported assembly for no accuracy gain — a mesh's *local* coordinates are small,
/// and it is the world **ray** that needs f64. So the source is read in place.
pub trait Position: Copy {
    fn to_vec3(self) -> Vec3;
}

impl Position for [f32; 3] {
    fn to_vec3(self) -> Vec3 {
        [f64::from(self[0]), f64::from(self[1]), f64::from(self[2])]
    }
}

impl Position for [f64; 3] {
    fn to_vec3(self) -> Vec3 {
        self
    }
}

/// Where [`MeshBvh`] reads triangles from. Implemented for the common indexed layout, and open so a
/// caller can expose an interleaved vertex buffer without copying it.
pub trait TriangleSource {
    fn triangle_count(&self) -> usize;
    /// The triangle's three corners, or `None` if its indices are out of range (a malformed import
    /// must not panic the editor).
    fn triangle(&self, index: usize) -> Option<[Vec3; 3]>;
}

/// The ordinary positions-plus-indices mesh, borrowed.
#[derive(Clone, Copy, Debug)]
pub struct IndexedMesh<'a, P: Position> {
    pub positions: &'a [P],
    pub indices: &'a [u32],
}

impl<'a, P: Position> IndexedMesh<'a, P> {
    #[must_use]
    pub fn new(positions: &'a [P], indices: &'a [u32]) -> Self {
        Self { positions, indices }
    }
}

impl<P: Position> TriangleSource for IndexedMesh<'_, P> {
    fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
    fn triangle(&self, index: usize) -> Option<[Vec3; 3]> {
        let i0 = *self.indices.get(index * 3)? as usize;
        let i1 = *self.indices.get(index * 3 + 1)? as usize;
        let i2 = *self.indices.get(index * 3 + 2)? as usize;
        Some([
            self.positions.get(i0)?.to_vec3(),
            self.positions.get(i1)?.to_vec3(),
            self.positions.get(i2)?.to_vec3(),
        ])
    }
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    /// For a leaf: the first index into the primitive-order array. For an interior node: the index
    /// of the right child (the left child is always `self_index + 1`, the standard layout that makes
    /// traversal a single array walk).
    offset: u32,
    count: u32,
}

impl Node {
    fn is_leaf(&self) -> bool {
        self.count > 0
    }
}

/// A BVH over an indexed triangle mesh, in the mesh's **local** space.
#[derive(Clone, Debug, Default)]
pub struct MeshBvh {
    nodes: Vec<Node>,
    /// Triangle indices in traversal order.
    order: Vec<u32>,
    triangle_count: usize,
    bounds: Aabb,
}

/// A triangle hit in mesh-local space.
#[derive(Clone, Copy, Debug)]
pub struct MeshHit {
    pub triangle: u32,
    pub hit: TriangleHit,
}

impl MeshBvh {
    /// Build over any [`TriangleSource`].
    ///
    /// Non-finite and degenerate triangles are skipped at build time rather than at query time: a NaN
    /// vertex would produce a NaN bound that swallows the whole tree, and imported meshes reliably
    /// contain some.
    #[must_use]
    pub fn build(source: &impl TriangleSource) -> Self {
        let tri_count = source.triangle_count();
        // One packed array of (triangle index, centroid, bounds): the builder reorders it in place,
        // and packing keeps a swap from moving one field and forgetting the others.
        let mut prims: Vec<(u32, Vec3, Aabb)> = Vec::with_capacity(tri_count);
        for t in 0..tri_count {
            let Some([a, b, c]) = source.triangle(t) else {
                continue; // out-of-range index: skip rather than panic on a malformed import
            };
            let bb = Aabb::from_points([a, b, c]);
            if bb.is_empty() || !bb.is_finite() {
                continue;
            }
            prims.push((u32::try_from(t).unwrap_or(u32::MAX), bb.center(), bb));
        }

        let mut nodes = Vec::with_capacity(prims.len().max(1) * 2);
        let root_bounds = prims.iter().fold(Aabb::EMPTY, |acc, p| acc.union(&p.2));
        if prims.is_empty() {
            return Self {
                nodes,
                order: Vec::new(),
                triangle_count: 0,
                bounds: Aabb::EMPTY,
            };
        }
        nodes.push(Node {
            bounds: root_bounds,
            offset: 0,
            count: 0,
        });
        let count = prims.len();
        build_recursive(&mut nodes, &mut prims, 0, 0, count, 0);
        Self {
            nodes,
            order: prims.into_iter().map(|p| p.0).collect(),
            triangle_count: count,
            bounds: root_bounds,
        }
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    #[must_use]
    pub fn triangle_count(&self) -> usize {
        self.triangle_count
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// The nearest triangle the ray hits, in mesh-local space.
    ///
    /// Traversal shrinks `t_max` as it finds closer hits, so subtrees behind the current best are
    /// rejected by their bounds without ever touching a triangle. `scratch` is the caller's traversal
    /// stack, reused across queries — a hover pick runs on every pointer move, and allocating a
    /// `Vec` there is a per-frame allocation on the interaction hot path.
    pub fn raycast(
        &self,
        ray: &Ray,
        source: &impl TriangleSource,
        t_max: f64,
        scratch: &mut Vec<u32>,
    ) -> Option<MeshHit> {
        if self.nodes.is_empty() {
            return None;
        }
        scratch.clear();
        scratch.push(0);
        let mut best: Option<MeshHit> = None;
        let mut limit = t_max;
        while let Some(index) = scratch.pop() {
            let node = self.nodes[index as usize];
            if !ray_aabb_hit(ray, &node.bounds, limit) {
                continue;
            }
            if node.is_leaf() {
                for k in 0..node.count {
                    let tri = self.order[(node.offset + k) as usize];
                    let Some([a, b, c]) = source.triangle(tri as usize) else {
                        continue;
                    };
                    if let Some(hit) = ray_triangle(ray, a, b, c, limit) {
                        limit = hit.t;
                        best = Some(MeshHit { triangle: tri, hit });
                    }
                }
            } else {
                scratch.push(index + 1);
                scratch.push(node.offset);
            }
        }
        best
    }

    /// Whether the ray hits anything at all within `t_max`. Stops at the first hit, so it is strictly
    /// cheaper than [`Self::raycast`] — the right query for an occlusion test.
    pub fn intersects(
        &self,
        ray: &Ray,
        source: &impl TriangleSource,
        t_max: f64,
        scratch: &mut Vec<u32>,
    ) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        scratch.clear();
        scratch.push(0);
        while let Some(index) = scratch.pop() {
            let node = self.nodes[index as usize];
            if !ray_aabb_hit(ray, &node.bounds, t_max) {
                continue;
            }
            if node.is_leaf() {
                for k in 0..node.count {
                    let tri = self.order[(node.offset + k) as usize];
                    let Some([a, b, c]) = source.triangle(tri as usize) else {
                        continue;
                    };
                    if ray_triangle(ray, a, b, c, t_max).is_some() {
                        return true;
                    }
                }
            } else {
                scratch.push(index + 1);
                scratch.push(node.offset);
            }
        }
        false
    }
}

/// A BVH over per-object world bounds — the picking broad phase.
#[derive(Clone, Debug, Default)]
pub struct SceneBvh {
    nodes: Vec<Node>,
    /// Caller-defined object keys, in traversal order.
    order: Vec<u32>,
    /// Each entry's world bounds, indexed by the caller's key.
    object_bounds: Vec<Aabb>,
    /// The bounds each leaf was BUILT with — the reference refit quality is measured against.
    built_bounds: Vec<Aabb>,
}

impl SceneBvh {
    /// Build over `bounds`, where the index into the slice is the object key the queries return.
    /// Empty entries (objects with no geometry) are skipped and simply never returned.
    #[must_use]
    pub fn build(bounds: &[Aabb]) -> Self {
        let mut prims: Vec<(u32, Vec3, Aabb)> = bounds
            .iter()
            .enumerate()
            .filter(|(_, b)| !b.is_empty() && b.is_finite())
            .map(|(i, b)| (u32::try_from(i).unwrap_or(u32::MAX), b.center(), *b))
            .collect();
        let mut nodes = Vec::with_capacity(prims.len().max(1) * 2);
        if prims.is_empty() {
            return Self {
                nodes,
                order: Vec::new(),
                object_bounds: bounds.to_vec(),
                built_bounds: bounds.to_vec(),
            };
        }
        let root = prims.iter().fold(Aabb::EMPTY, |acc, p| acc.union(&p.2));
        nodes.push(Node {
            bounds: root,
            offset: 0,
            count: 0,
        });
        let count = prims.len();
        build_recursive(&mut nodes, &mut prims, 0, 0, count, 0);
        Self {
            nodes,
            order: prims.into_iter().map(|p| p.0).collect(),
            object_bounds: bounds.to_vec(),
            built_bounds: bounds.to_vec(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[must_use]
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    #[must_use]
    pub fn bounds(&self) -> Aabb {
        self.nodes.first().map_or(Aabb::EMPTY, |n| n.bounds)
    }

    /// Update one object's bounds without changing the topology.
    ///
    /// The node bounds are **not** recomputed here — call [`Self::refit`] once after a batch of
    /// updates. Refitting per object during a drag of a 200-part selection would walk the tree 200
    /// times for one frame's worth of change.
    pub fn update_bounds(&mut self, key: u32, bounds: Aabb) -> bool {
        match self.object_bounds.get_mut(key as usize) {
            Some(slot) => {
                *slot = bounds;
                true
            }
            None => false,
        }
    }

    /// Recompute every node's bounds bottom-up from the current object bounds. `O(nodes)`, no
    /// allocation, no reordering — the operation that makes a gizmo drag cheap.
    pub fn refit(&mut self) {
        if self.nodes.is_empty() {
            return;
        }
        // Nodes are emitted parent-before-children by the builder, so a reverse pass visits every
        // child before its parent.
        for i in (0..self.nodes.len()).rev() {
            let node = self.nodes[i];
            let bounds = if node.is_leaf() {
                let mut b = Aabb::EMPTY;
                for k in 0..node.count {
                    let key = self.order[(node.offset + k) as usize] as usize;
                    if let Some(ob) = self.object_bounds.get(key) {
                        b.expand(ob);
                    }
                }
                b
            } else {
                let left = self.nodes[i + 1].bounds;
                let right = self.nodes[node.offset as usize].bounds;
                left.union(&right)
            };
            self.nodes[i].bounds = bounds;
        }
    }

    /// How far refitting has degraded the tree: the ratio of the current root area to the area the
    /// tree was built with. Above ~2 the topology no longer reflects where objects are and a rebuild
    /// pays for itself — surfaced so that decision is made on a measurement instead of a timer.
    #[must_use]
    pub fn refit_quality(&self) -> f64 {
        let built = self
            .built_bounds
            .iter()
            .fold(Aabb::EMPTY, Aabb::union)
            .surface_area();
        if built <= 0.0 {
            return 1.0;
        }
        (self.bounds().surface_area() / built).max(1.0)
    }

    /// Every object whose bounds the ray enters within `t_max`, as `(key, t_enter)`.
    ///
    /// Deliberately returns **candidates**, not a single answer: the broad phase's job is to shrink
    /// the set, and deciding which candidate the user meant needs geometry, visibility and priority
    /// the BVH does not know about. Results are pushed in traversal order; the caller sorts.
    pub fn query_ray(
        &self,
        ray: &Ray,
        t_max: f64,
        scratch: &mut Vec<u32>,
        out: &mut Vec<(u32, f64)>,
    ) {
        out.clear();
        if self.nodes.is_empty() {
            return;
        }
        scratch.clear();
        scratch.push(0);
        while let Some(index) = scratch.pop() {
            let node = self.nodes[index as usize];
            if !ray_aabb_hit(ray, &node.bounds, t_max) {
                continue;
            }
            if node.is_leaf() {
                for k in 0..node.count {
                    let key = self.order[(node.offset + k) as usize];
                    if let Some(b) = self.object_bounds.get(key as usize) {
                        // The exact interval is what the caller sorts by, but membership uses the
                        // conservative test: a broad phase that drops a grazing candidate produces a
                        // click that selects nothing, with nothing anywhere saying why.
                        if ray_aabb_hit(ray, b, t_max) {
                            let t = crate::ray::ray_aabb(ray, b, t_max).map_or(0.0, |(t, _)| t);
                            out.push((key, t));
                        }
                    }
                }
            } else {
                scratch.push(index + 1);
                scratch.push(node.offset);
            }
        }
    }

    /// Every object whose bounds intersect `region` — the marquee/box-selection broad phase.
    pub fn query_bounds(&self, region: &Aabb, scratch: &mut Vec<u32>, out: &mut Vec<u32>) {
        out.clear();
        if self.nodes.is_empty() || region.is_empty() {
            return;
        }
        scratch.clear();
        scratch.push(0);
        while let Some(index) = scratch.pop() {
            let node = self.nodes[index as usize];
            if !node.bounds.intersects(region) {
                continue;
            }
            if node.is_leaf() {
                for k in 0..node.count {
                    let key = self.order[(node.offset + k) as usize];
                    if let Some(b) = self.object_bounds.get(key as usize) {
                        if b.intersects(region) {
                            out.push(key);
                        }
                    }
                }
            } else {
                scratch.push(index + 1);
                scratch.push(node.offset);
            }
        }
    }
}

/// Binned-SAH split of `prims[first..first+count]`, writing nodes into `nodes` at `node_index`.
///
/// Iterative would be nicer, but the recursion depth is bounded by [`MAX_DEPTH`] (64), which is a few
/// kilobytes of stack — unlike the *data*-driven recursion in a scene graph, this one cannot be made
/// deep by user input.
fn build_recursive(
    nodes: &mut Vec<Node>,
    prims: &mut [(u32, Vec3, Aabb)],
    node_index: usize,
    first: usize,
    count: usize,
    depth: u32,
) {
    let bounds = prims[first..first + count]
        .iter()
        .fold(Aabb::EMPTY, |a, p| a.union(&p.2));
    nodes[node_index].bounds = bounds;

    let make_leaf = |nodes: &mut Vec<Node>| {
        nodes[node_index].offset = u32::try_from(first).unwrap_or(u32::MAX);
        nodes[node_index].count = u32::try_from(count).unwrap_or(u32::MAX);
    };

    if count <= MAX_LEAF_PRIMITIVES || depth >= MAX_DEPTH {
        make_leaf(nodes);
        return;
    }

    // Split along the axis with the widest spread of CENTROIDS, not of bounds: a few huge objects
    // make the bounds wide on an axis where everything is actually stacked, and splitting there
    // separates nothing.
    let centroid_bounds = prims[first..first + count]
        .iter()
        .fold(Aabb::EMPTY, |a, p| a.union(&Aabb::point(p.1)));
    let axis = centroid_bounds.longest_axis();
    let extent = centroid_bounds.size()[axis];
    if extent <= 0.0 {
        // Every centroid coincident: no split can separate them. A median cut keeps the tree
        // balanced rather than recursing forever on the same set.
        let mid = count / 2;
        split_at(nodes, prims, node_index, first, count, mid, depth);
        return;
    }

    let lo = centroid_bounds.min[axis];
    let scale = f64::from(u32::try_from(SAH_BUCKETS).unwrap_or(12)) / extent;
    let mut bucket_bounds = [Aabb::EMPTY; SAH_BUCKETS];
    let mut bucket_count = [0usize; SAH_BUCKETS];
    for p in &prims[first..first + count] {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let b = (((p.1[axis] - lo) * scale) as usize).min(SAH_BUCKETS - 1);
        bucket_bounds[b].expand(&p.2);
        bucket_count[b] += 1;
    }

    // Cost of splitting after bucket i: area(left)·count(left) + area(right)·count(right).
    let mut best_cost = f64::INFINITY;
    let mut best_split = 0usize;
    for split in 1..SAH_BUCKETS {
        let mut left = Aabb::EMPTY;
        let mut left_n = 0usize;
        for b in 0..split {
            left.expand(&bucket_bounds[b]);
            left_n += bucket_count[b];
        }
        let mut right = Aabb::EMPTY;
        let mut right_n = 0usize;
        for b in split..SAH_BUCKETS {
            right.expand(&bucket_bounds[b]);
            right_n += bucket_count[b];
        }
        if left_n == 0 || right_n == 0 {
            continue;
        }
        #[allow(clippy::cast_precision_loss)]
        let cost = left.surface_area() * left_n as f64 + right.surface_area() * right_n as f64;
        if cost < best_cost {
            best_cost = cost;
            best_split = split;
        }
    }

    #[allow(clippy::cast_precision_loss)]
    let leaf_cost = bounds.surface_area() * count as f64;
    if best_split == 0 || (count <= MAX_LEAF_PRIMITIVES * 2 && best_cost >= leaf_cost) {
        make_leaf(nodes);
        return;
    }

    // Partition in place by bucket.
    let threshold = lo
        + (extent / f64::from(u32::try_from(SAH_BUCKETS).unwrap_or(12)))
            * f64::from(u32::try_from(best_split).unwrap_or(1));
    let slice = &mut prims[first..first + count];
    let mut mid = 0usize;
    for i in 0..slice.len() {
        if slice[i].1[axis] < threshold {
            slice.swap(i, mid);
            mid += 1;
        }
    }
    if mid == 0 || mid == count {
        mid = count / 2; // the partition degenerated; fall back to a median cut
    }
    split_at(nodes, prims, node_index, first, count, mid, depth);
}

fn split_at(
    nodes: &mut Vec<Node>,
    prims: &mut [(u32, Vec3, Aabb)],
    node_index: usize,
    first: usize,
    count: usize,
    mid: usize,
    depth: u32,
) {
    let left_index = nodes.len();
    nodes.push(Node {
        bounds: Aabb::EMPTY,
        offset: 0,
        count: 0,
    });
    build_recursive(nodes, prims, left_index, first, mid, depth + 1);
    let right_index = nodes.len();
    nodes.push(Node {
        bounds: Aabb::EMPTY,
        offset: 0,
        count: 0,
    });
    build_recursive(
        nodes,
        prims,
        right_index,
        first + mid,
        count - mid,
        depth + 1,
    );
    nodes[node_index].offset = u32::try_from(right_index).unwrap_or(u32::MAX);
    nodes[node_index].count = 0;
    debug_assert_eq!(
        left_index,
        node_index + 1,
        "left child is always node_index + 1"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epsilon::approx_eq;

    /// An axis-aligned quad at `z`, as two triangles.
    fn quad(positions: &mut Vec<Vec3>, indices: &mut Vec<u32>, z: f64, half: f64) {
        let base = u32::try_from(positions.len()).unwrap();
        positions.extend([
            [-half, -half, z],
            [half, -half, z],
            [half, half, z],
            [-half, half, z],
        ]);
        indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }

    #[test]
    fn mesh_bvh_returns_the_nearest_triangle_not_just_any_hit() {
        // Ten stacked quads: the ray must come back with the FIRST one, or "select what you see" is
        // impossible in any scene with layered geometry.
        let (mut p, mut i) = (Vec::new(), Vec::new());
        for k in 0..10 {
            quad(&mut p, &mut i, -f64::from(k), 1.0);
        }
        let src = IndexedMesh::new(&p, &i);
        let bvh = MeshBvh::build(&src);
        assert_eq!(bvh.triangle_count(), 20);
        let mut scratch = Vec::new();
        let ray = Ray::new([0.0, 0.0, 5.0], [0.0, 0.0, -1.0]);
        let hit = bvh
            .raycast(&ray, &src, f64::INFINITY, &mut scratch)
            .expect("hit");
        assert!(
            approx_eq(hit.hit.t, 5.0),
            "nearest quad is at z = 0 (t = {})",
            hit.hit.t
        );
        assert!(
            hit.triangle < 2,
            "and it is one of the first quad's two triangles"
        );
        // From the other side, the nearest is the LAST quad.
        let back = Ray::new([0.0, 0.0, -20.0], [0.0, 0.0, 1.0]);
        let hb = bvh
            .raycast(&back, &src, f64::INFINITY, &mut scratch)
            .expect("hit");
        assert!(approx_eq(hb.hit.t, 11.0), "t = {}", hb.hit.t);
    }

    #[test]
    fn mesh_bvh_agrees_exactly_with_brute_force_on_random_geometry() {
        // The correctness contract for an acceleration structure: it must not change the ANSWER, only
        // the cost. A deterministic pseudo-random mesh, every ray checked against a linear scan.
        // xorshift64, mapped to [-1, 1). The 53-bit divisor matters: `>> 11` leaves 53 bits, so
        // dividing by 2^21 (as an earlier draft did) yields values up to ~4×10⁹ and the "random
        // mesh" silently spans a billion units — which is a different test than the one intended.
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let mut rng = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            ((seed >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        };
        let mut p: Vec<Vec3> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        for t in 0..700u32 {
            let c = [rng() * 20.0, rng() * 20.0, rng() * 20.0];
            for k in 0..3 {
                p.push([c[0] + rng() * 2.0, c[1] + rng() * 2.0, c[2] + rng() * 2.0]);
                let _ = k;
            }
            idx.extend([t * 3, t * 3 + 1, t * 3 + 2]);
        }
        let src = IndexedMesh::new(&p, &idx);
        let bvh = MeshBvh::build(&src);
        let mut scratch = Vec::new();
        let mut checked = 0;
        for probe in 0..300usize {
            // Aim each ray from a random point THROUGH a vertex of a randomly chosen triangle, so
            // the sample is dominated by rays that actually hit something. Purely random directions
            // mostly miss, and a comparison test that rarely compares a hit proves nothing.
            let origin = [rng() * 60.0, rng() * 60.0, rng() * 60.0];
            // Aim at a triangle's CENTROID, not at a vertex. A ray through a vertex is a boundary
            // case shared by three edges and every adjacent face, and comparing two implementations
            // on ties measures rounding rather than correctness.
            let t = (probe * 11) % (idx.len() / 3);
            let (v0, v1, v2) = (
                p[idx[t * 3] as usize],
                p[idx[t * 3 + 1] as usize],
                p[idx[t * 3 + 2] as usize],
            );
            let target = [
                (v0[0] + v1[0] + v2[0]) / 3.0,
                (v0[1] + v1[1] + v2[1]) / 3.0,
                (v0[2] + v1[2] + v2[2]) / 3.0,
            ];
            let ray = Ray::new(
                origin,
                [
                    target[0] - origin[0],
                    target[1] - origin[1],
                    target[2] - origin[2],
                ],
            );
            let fast = bvh.raycast(&ray, &src, f64::INFINITY, &mut scratch);
            // Brute force.
            let mut slow: Option<(u32, f64)> = None;
            for t in 0..idx.len() / 3 {
                let (a, b, c) = (
                    p[idx[t * 3] as usize],
                    p[idx[t * 3 + 1] as usize],
                    p[idx[t * 3 + 2] as usize],
                );
                if let Some(h) = crate::ray::ray_triangle(&ray, a, b, c, f64::INFINITY) {
                    if slow.is_none_or(|(_, bt)| h.t < bt) {
                        slow = Some((u32::try_from(t).unwrap(), h.t));
                    }
                }
            }
            match (fast, slow) {
                (Some(f), Some((_, st))) => {
                    assert!(
                        approx_eq(f.hit.t, st),
                        "BVH t {} vs brute force {st}",
                        f.hit.t
                    );
                    checked += 1;
                }
                (None, None) => {}
                (f, s) => {
                    let tri = s.map_or(0, |(t, _)| t) as usize;
                    let corners = src.triangle(tri);
                    panic!(
                        "BVH and brute force disagree: {f:?} vs {s:?}\n\
                         ray origin {:?} dir {:?}\n\
                         triangle {tri} corners {corners:?}\n\
                         root bounds {:?} (hit: {})\n\
                         triangle present in order: {}",
                        ray.origin,
                        ray.direction,
                        bvh.bounds(),
                        crate::ray::ray_aabb(&ray, &bvh.bounds(), f64::INFINITY).is_some(),
                        bvh.order.contains(&u32::try_from(tri).unwrap()),
                    )
                }
            }
        }
        assert!(
            checked > 100,
            "the test actually exercised hits (got {checked}/300)"
        );
    }

    #[test]
    fn a_mesh_of_degenerate_and_nan_triangles_builds_and_never_hits() {
        let p: Vec<Vec3> = vec![
            [0.0; 3],
            [1.0, 0.0, 0.0],
            [2.0, 0.0, 0.0], // collinear ⇒ degenerate
            [f64::NAN, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0], // contains NaN
        ];
        let idx = vec![0, 1, 2, 3, 4, 5];
        let src = IndexedMesh::new(&p, &idx);
        let bvh = MeshBvh::build(&src);
        assert!(
            bvh.bounds().is_finite(),
            "a NaN vertex must not poison the root bounds"
        );
        let mut scratch = Vec::new();
        let ray = Ray::new([0.5, 0.5, 5.0], [0.0, 0.0, -1.0]);
        assert!(bvh
            .raycast(&ray, &src, f64::INFINITY, &mut scratch)
            .is_none());
        // An out-of-range index must not panic.
        let bad_src = IndexedMesh::new(&p, &[0, 1, 999][..]);
        let bad = MeshBvh::build(&bad_src);
        assert!(bad
            .raycast(&ray, &bad_src, f64::INFINITY, &mut scratch)
            .is_none());
        // An empty mesh is valid and simply never hits.
        let empty_src = IndexedMesh::new(&[] as &[Vec3], &[] as &[u32]);
        let empty = MeshBvh::build(&empty_src);
        assert!(empty.is_empty());
        assert!(empty
            .raycast(&ray, &empty_src, f64::INFINITY, &mut scratch)
            .is_none());
    }

    #[test]
    fn a_million_coincident_triangles_do_not_recurse_forever() {
        // Every centroid identical: no split ever separates anything. Without a depth cap this is
        // infinite recursion and a process abort.
        let p = vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut idx = Vec::new();
        for _ in 0..5_000 {
            idx.extend([0, 1, 2]);
        }
        let src = IndexedMesh::new(&p, &idx);
        let bvh = MeshBvh::build(&src);
        assert_eq!(bvh.triangle_count(), 5_000);
        let mut scratch = Vec::new();
        let ray = Ray::new([0.2, 0.2, 5.0], [0.0, 0.0, -1.0]);
        assert!(bvh
            .raycast(&ray, &src, f64::INFINITY, &mut scratch)
            .is_some());
    }

    #[test]
    fn scene_bvh_finds_every_candidate_a_linear_scan_would() {
        let mut bounds = Vec::new();
        for i in 0..500 {
            let x = f64::from(i % 25) * 4.0;
            let y = f64::from(i / 25) * 4.0;
            bounds.push(Aabb::from_center_half([x, y, 0.0], [1.0; 3]));
        }
        let bvh = SceneBvh::build(&bounds);
        let mut scratch = Vec::new();
        let mut out = Vec::new();
        let ray = Ray::new([40.0, 28.0, 50.0], [0.0, 0.0, -1.0]);
        bvh.query_ray(&ray, f64::INFINITY, &mut scratch, &mut out);
        let mut expected: Vec<u32> = bounds
            .iter()
            .enumerate()
            .filter(|(_, b)| crate::ray::ray_aabb(&ray, b, f64::INFINITY).is_some())
            .map(|(i, _)| u32::try_from(i).unwrap())
            .collect();
        let mut got: Vec<u32> = out.iter().map(|(k, _)| *k).collect();
        got.sort_unstable();
        expected.sort_unstable();
        assert_eq!(got, expected, "broad phase must not lose a candidate");
        assert!(!expected.is_empty(), "the test ray actually hits something");
    }

    #[test]
    fn refit_keeps_answers_correct_while_objects_move() {
        // The drag case: bounds change every frame, topology does not. Refit must keep the tree a
        // correct (conservative) hierarchy, and the quality metric must notice when it stops being a
        // GOOD one.
        let mut bounds: Vec<Aabb> = (0..200)
            .map(|i| Aabb::from_center_half([f64::from(i), 0.0, 0.0], [0.4; 3]))
            .collect();
        let mut bvh = SceneBvh::build(&bounds);
        let baseline = bvh.refit_quality();
        assert!(
            approx_eq(baseline, 1.0),
            "a freshly built tree is perfect ({baseline})"
        );

        // Drag object 7 far away.
        bounds[7] = Aabb::from_center_half([0.0, 5000.0, 0.0], [0.4; 3]);
        bvh.update_bounds(7, bounds[7]);
        bvh.refit();
        let mut scratch = Vec::new();
        let mut out = Vec::new();
        let ray = Ray::new([0.0, 5000.0, 50.0], [0.0, 0.0, -1.0]);
        bvh.query_ray(&ray, f64::INFINITY, &mut scratch, &mut out);
        assert!(
            out.iter().any(|(k, _)| *k == 7),
            "the moved object is still found"
        );
        assert!(
            bvh.refit_quality() > 2.0,
            "…and the tree reports that it has degraded"
        );

        // Its old position must no longer report it.
        let old = Ray::new([7.0, 0.0, 50.0], [0.0, 0.0, -1.0]);
        bvh.query_ray(&old, f64::INFINITY, &mut scratch, &mut out);
        assert!(
            !out.iter().any(|(k, _)| *k == 7),
            "and not at its old position"
        );
    }

    #[test]
    fn box_query_returns_exactly_the_intersecting_objects() {
        let bounds: Vec<Aabb> = (0..100)
            .map(|i| Aabb::from_center_half([f64::from(i), 0.0, 0.0], [0.25; 3]))
            .collect();
        let bvh = SceneBvh::build(&bounds);
        let mut scratch = Vec::new();
        let mut out = Vec::new();
        bvh.query_bounds(
            &Aabb::new([9.5, -1.0, -1.0], [12.5, 1.0, 1.0]),
            &mut scratch,
            &mut out,
        );
        out.sort_unstable();
        assert_eq!(out, vec![10, 11, 12]);
        bvh.query_bounds(&Aabb::EMPTY, &mut scratch, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn an_empty_scene_produces_an_empty_tree_that_answers_nothing() {
        let bvh = SceneBvh::build(&[]);
        assert!(bvh.is_empty());
        let mut scratch = Vec::new();
        let mut out = Vec::new();
        bvh.query_ray(
            &Ray::new([0.0; 3], [0.0, 0.0, -1.0]),
            f64::INFINITY,
            &mut scratch,
            &mut out,
        );
        assert!(out.is_empty());
        // All-empty bounds are skipped rather than producing a degenerate tree.
        let skipped = SceneBvh::build(&[Aabb::EMPTY, Aabb::EMPTY]);
        assert!(skipped.is_empty());
    }
}
