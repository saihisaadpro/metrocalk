//! **Picking**: pointer ray in, a canonical [`HitResult`] out.
//!
//! The pipeline, and why each stage exists:
//!
//! ```text
//! ray ─► SceneBvh (broad) ─► per-object narrow test ─► filter ─► deterministic sort ─► HitResult
//! ```
//!
//! * **Broad phase.** A BVH over world AABBs. Without it, every click is a linear scan of the scene
//!   and hover becomes unaffordable at any interesting size.
//! * **Narrow phase.** The world ray is transformed *into each candidate's local space* and tested
//!   against real geometry through the mesh's own BVH. Transforming one ray per candidate is the
//!   whole trick — the alternative, transforming vertices into world space, is thousands of times
//!   more work and has to be redone whenever anything moves.
//! * **Filter.** Visibility, lock state, selectability and kind are applied *after* geometry, so the
//!   result can honestly answer "there was something there, but it is hidden" rather than silently
//!   returning nothing.
//! * **Deterministic sort.** Two objects under the cursor must resolve the same way every time. Ties
//!   fall through to the stable entity key, never to container iteration order.
//!
//! ## What this replaces
//!
//! The previous implementation ranked instances by the squared NDC distance from the cursor to each
//! instance's projected **pivot**. That has no ray, no geometry, no bounds and no occlusion, and it
//! returns a hit for any non-empty scene — so clicking empty space always selected something, a big
//! object could not be clicked anywhere except near its origin, and a mesh whose modelling origin sat
//! away from its geometry (routine in imported CAD) was unclickable where it was drawn.

use crate::bounds::Aabb;
use crate::bvh::{IndexedMesh, MeshBvh, SceneBvh};
use crate::camera::{Camera, Viewport};
use crate::epsilon;
use crate::ray::{ray_aabb, ray_sphere, Ray};
use crate::transform::{mat_inverse, transform_dir, Mat4, Vec3};

/// What kind of thing was hit — drives filtering, priority, and the icon the UI shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum HitKind {
    Mesh,
    Light,
    Camera,
    Helper,
    Group,
    Terrain,
    Curve,
    Volume,
    Bone,
}

impl HitKind {
    /// The bit this kind occupies in a [`PickFilter`] mask.
    #[must_use]
    pub fn bit(self) -> u32 {
        1 << (self as u32)
    }

    /// Default interaction priority. Icon-like objects sit above scene geometry because they are
    /// drawn as overlays and are physically tiny: a light inside a wall is still the thing a user
    /// clicking its icon meant. Scene meshes are the baseline and resolve purely by depth.
    #[must_use]
    pub fn default_priority(self) -> i32 {
        match self {
            Self::Light | Self::Camera | Self::Helper | Self::Bone => 10,
            Self::Curve => 5,
            Self::Mesh | Self::Group | Self::Terrain | Self::Volume => 0,
        }
    }
}

/// How an object is tested once the broad phase has offered it as a candidate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickGeometry {
    /// Real triangles, via the shared per-asset [`MeshBvh`] at this index.
    Mesh(u32),
    /// The object's local AABB as a solid box — placeholder cubes, volumes, and groups that are
    /// selectable as a whole.
    Bounds,
    /// A point proxy with a radius measured in **pixels**, so it stays equally clickable at any zoom.
    /// Lights, cameras, empties and joints have no surface to hit; requiring a one-pixel intersection
    /// with an icon would make them unselectable in practice.
    Point { screen_radius_px: f64 },
    /// A polyline (index into [`PickScene`]'s polyline table) with a pixel thickness — wires, curves,
    /// bones, edges.
    Polyline { index: u32, screen_radius_px: f64 },
}

/// Where a hit came from. Useful in diagnostics: "the click resolved against a proxy, not geometry"
/// explains a surprising selection immediately.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitSource {
    Triangle,
    Bounds,
    Proxy,
}

/// **The canonical intersection result.** One shape, returned by every picking entry point.
///
/// Deliberately richer than selection needs: the same query answers material painting, object
/// placement, measurement, surface snapping, decals and dimensioning. Throwing the hit point, normal
/// and primitive away at the picking boundary is what forces each of those features to grow its own
/// parallel raycast later.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitResult {
    /// The caller's stable entity key.
    pub key: u64,
    /// Which instance of that entity — `0` when the entity is not instanced. An instanced forest
    /// returns *which tree*, not "the source mesh".
    pub instance: u32,
    /// Index into [`PickScene::objects`], for callers that want the full record back.
    pub object: u32,
    /// Triangle index within the mesh, when the hit came from real geometry.
    pub primitive: Option<u32>,
    /// Distance along the ray, in world units. Directly comparable between hits.
    pub distance: f64,
    pub world_position: Vec3,
    pub local_position: Vec3,
    /// Unit normal in world space, oriented against the ray so it always faces the viewer.
    pub world_normal: Vec3,
    pub local_normal: Vec3,
    /// Barycentric weights `[a, b, c]` of the hit within its triangle.
    pub barycentric: Option<[f64; 3]>,
    pub kind: HitKind,
    pub source: HitSource,
    /// How far the ray passed from the object in **pixels**. Zero for a real geometric hit; non-zero
    /// only for proxy hits resolved within a tolerance.
    pub screen_distance_px: f64,
    /// Whether the surface was struck from behind.
    pub back_face: bool,
    /// Interaction priority used in the ordering.
    pub priority: i32,
    pub locked: bool,
}

impl HitResult {
    /// The deterministic sort key. Ordering, in words: **higher priority first, then nearer, then
    /// closer to the cursor in pixels, then the stable key, then the instance.**
    ///
    /// Every level is a total order on values the caller controls, so the same click always resolves
    /// the same way. The last two levels exist purely so exact geometric ties (coincident faces,
    /// duplicated geometry) cannot fall through to hash-map or vector iteration order.
    fn sort_key(&self) -> (i32, u64, u64, u64, u32) {
        (
            -self.priority,
            self.distance.max(0.0).to_bits(),
            self.screen_distance_px.max(0.0).to_bits(),
            self.key,
            self.instance,
        )
    }
}

/// How transparent surfaces participate in picking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TransparencyPolicy {
    /// A transparent surface is selectable like any other — you can click the glass.
    #[default]
    Selectable,
    /// The ray passes through transparent surfaces to whatever is behind them, unless nothing is.
    /// Chosen deliberately rather than left accidental: an editor full of glass panels is unusable
    /// under the first policy, and an editor where glass cannot be selected at all is unusable under
    /// a strict version of this one.
    PreferOpaque,
    /// Transparent surfaces are ignored entirely.
    Ignore,
}

/// Restricts what a query may return, so tools can narrow the candidate set without every input path
/// hard-coding "meshes only".
#[derive(Clone, Copy, Debug)]
pub struct PickFilter {
    /// Bit mask of allowed [`HitKind`]s. Zero means "no kinds", not "all kinds" — an empty filter
    /// that silently matched everything would be a trap.
    pub kinds: u32,
    /// Include objects that are locked (selectable for inspection, refused for transform).
    pub include_locked: bool,
    /// Include objects hidden by their own or an ancestor's visibility.
    pub include_hidden: bool,
    /// Include objects flagged non-selectable (annotation geometry, gizmo-only helpers).
    pub include_unselectable: bool,
    /// Skip this key — used by "select what is behind the thing I already have".
    pub exclude_key: Option<u64>,
    pub max_distance: f64,
    pub transparency: TransparencyPolicy,
    /// Extra pixel tolerance applied to proxy geometry, on top of each proxy's own radius. Scales
    /// with pointer type: a touch or pen contact deserves more than a mouse.
    pub extra_tolerance_px: f64,
}

impl Default for PickFilter {
    fn default() -> Self {
        Self {
            kinds: u32::MAX,
            include_locked: true,
            include_hidden: false,
            include_unselectable: false,
            exclude_key: None,
            max_distance: f64::INFINITY,
            transparency: TransparencyPolicy::default(),
            extra_tolerance_px: 0.0,
        }
    }
}

impl PickFilter {
    /// Only these kinds.
    #[must_use]
    pub fn only(kinds: &[HitKind]) -> Self {
        Self {
            kinds: kinds.iter().fold(0, |m, k| m | k.bit()),
            ..Self::default()
        }
    }

    #[must_use]
    pub fn allows(&self, kind: HitKind) -> bool {
        self.kinds & kind.bit() != 0
    }
}

/// One pickable thing. An entity with several instances contributes one of these per instance, which
/// is what makes "instance 5,284" an answer the system can give rather than one it has thrown away.
// Four independent policy flags, each with its own meaning and its own filter. Packing them into a
// bitfield would make every read site do bit arithmetic to answer a question the field name already
// answers, and the renderer and the picker read these constantly.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug)]
pub struct PickObject {
    pub key: u64,
    pub instance: u32,
    pub world: Mat4,
    /// Cached `world⁻¹`. `None` for a singular (zero-scaled) transform, which is simply never hit
    /// rather than producing NaN.
    pub inv_world: Option<Mat4>,
    /// Bounds of the object's geometry in its own local space.
    pub local_bounds: Aabb,
    pub geometry: PickGeometry,
    pub kind: HitKind,
    pub visible: bool,
    pub selectable: bool,
    pub locked: bool,
    pub transparent: bool,
    pub priority: i32,
    /// How far, in **world units**, the broad-phase bounds are grown beyond the geometry.
    ///
    /// A light or an empty has zero-volume bounds, so an un-padded broad phase only offers it as a
    /// candidate when the ray passes exactly through the point — and the narrow phase's pixel
    /// tolerance never gets a chance to run. The padding is the world-space size of that pixel
    /// tolerance at the object's own depth, refreshed by
    /// [`PickScene::refresh_screen_padding`] when the camera changes.
    pub pick_padding: f64,
}

impl PickObject {
    /// Build an entry, caching the inverse and defaulting the priority from the kind.
    #[must_use]
    pub fn new(
        key: u64,
        world: Mat4,
        local_bounds: Aabb,
        geometry: PickGeometry,
        kind: HitKind,
    ) -> Self {
        Self {
            key,
            instance: 0,
            world,
            inv_world: mat_inverse(world),
            local_bounds,
            geometry,
            kind,
            visible: true,
            selectable: true,
            locked: false,
            transparent: false,
            priority: kind.default_priority(),
            pick_padding: 0.0,
        }
    }

    /// The pixel tolerance this object's geometry wants, or `None` for exact surfaces.
    #[must_use]
    pub fn screen_tolerance_px(&self) -> Option<f64> {
        match self.geometry {
            PickGeometry::Point { screen_radius_px }
            | PickGeometry::Polyline {
                screen_radius_px, ..
            } => Some(screen_radius_px),
            PickGeometry::Mesh(_) | PickGeometry::Bounds => None,
        }
    }

    /// The object's world-space bounds — what the broad phase indexes, padded so a proxy is reachable
    /// by a near-miss ray.
    #[must_use]
    pub fn world_bounds(&self) -> Aabb {
        let b = self.local_bounds.transformed(self.world);
        if self.pick_padding > 0.0 {
            b.expanded_by(self.pick_padding)
        } else {
            b
        }
    }
}

/// A mesh available for narrow-phase testing, owned by the pick scene.
///
/// Positions are kept in **f32**: they are mesh-*local*, so their magnitudes are small and f32 costs
/// nothing in accuracy there, while the world ray — the part that genuinely needs range — stays f64.
/// Storing a parallel f64 copy would double the memory of every imported assembly for no benefit.
#[derive(Clone, Debug, Default)]
pub struct PickMesh {
    pub positions: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
    pub bvh: MeshBvh,
}

impl PickMesh {
    #[must_use]
    pub fn build(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        let bvh = MeshBvh::build(&IndexedMesh::new(&positions, &indices));
        Self {
            positions,
            indices,
            bvh,
        }
    }

    #[must_use]
    pub fn source(&self) -> IndexedMesh<'_, [f32; 3]> {
        IndexedMesh::new(&self.positions, &self.indices)
    }

    #[must_use]
    pub fn local_bounds(&self) -> Aabb {
        self.bvh.bounds()
    }
}

/// Everything a pick needs to know about the scene.
#[derive(Clone, Debug, Default)]
pub struct PickScene {
    pub objects: Vec<PickObject>,
    pub meshes: Vec<PickMesh>,
    /// Local-space polylines, referenced by [`PickGeometry::Polyline`].
    pub polylines: Vec<Vec<Vec3>>,
    bvh: SceneBvh,
    dirty: bool,
    /// Indices of objects whose bounds depend on the camera (pixel-sized proxies). Kept separately so
    /// the per-pick padding refresh is `O(proxies)` rather than `O(scene)` — a scene of 100,000
    /// meshes and 20 lights should not walk 100,020 objects on every pointer move.
    proxies: Vec<u32>,
    /// The camera the padding was last computed for; a repeated pick from the same view skips the
    /// refresh entirely.
    padding_camera: Option<([f64; 3], f64, f64)>,
}

impl PickScene {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the object list. The broad phase is rebuilt on the next query.
    pub fn set_objects(&mut self, objects: Vec<PickObject>) {
        self.objects = objects;
        self.reindex_proxies();
        self.dirty = true;
        self.padding_camera = None;
    }

    pub fn push_object(&mut self, object: PickObject) {
        if object.screen_tolerance_px().is_some() {
            self.proxies
                .push(u32::try_from(self.objects.len()).unwrap_or(u32::MAX));
        }
        self.objects.push(object);
        self.dirty = true;
        self.padding_camera = None;
    }

    fn reindex_proxies(&mut self) {
        self.proxies.clear();
        for (i, o) in self.objects.iter().enumerate() {
            if o.screen_tolerance_px().is_some() {
                self.proxies.push(u32::try_from(i).unwrap_or(u32::MAX));
            }
        }
    }

    /// Move one object without rebuilding the tree — the gizmo-drag path.
    ///
    /// Refit rather than rebuild: `O(nodes)` with no allocation and no reordering. Rebuilding on
    /// every pointer move during a drag is the difference between a responsive viewport and a
    /// stuttering one on a large scene.
    pub fn update_object_transform(&mut self, index: usize, world: Mat4) -> bool {
        let Some(object) = self.objects.get_mut(index) else {
            return false;
        };
        object.world = world;
        object.inv_world = mat_inverse(world);
        let bounds = object.world_bounds();
        if self.dirty {
            return true; // a rebuild is already pending; it will pick this up
        }
        let key = u32::try_from(index).unwrap_or(u32::MAX);
        self.bvh.update_bounds(key, bounds);
        self.bvh.refit();
        true
    }

    /// Recompute each proxy object's broad-phase padding for the current camera.
    ///
    /// Proxy geometry is sized in pixels, so its world extent depends on where the camera is. This is
    /// an `O(objects)` pass plus a refit — no rebuild, no allocation — and is only worth running when
    /// the camera has actually moved. Returns the number of objects whose padding changed.
    pub fn refresh_screen_padding(&mut self, camera: &Camera, viewport: &Viewport) -> usize {
        let stamp = (camera.eye, viewport.aspect(), viewport.surface_height);
        if self.padding_camera == Some(stamp) {
            return 0; // same view as last time; the padding is already right
        }
        self.padding_camera = Some(stamp);
        let mut changed = 0;
        for slot in 0..self.proxies.len() {
            let index = self.proxies[slot] as usize;
            let Some(px) = self
                .objects
                .get(index)
                .and_then(PickObject::screen_tolerance_px)
            else {
                continue;
            };
            let object = &self.objects[index];
            let Some(center) =
                crate::transform::transform_point4(object.world, object.local_bounds.center())
            else {
                continue;
            };
            // A little slack above the exact tolerance: the broad phase must be conservative, and a
            // padding that exactly equals the narrow-phase radius can drop a hit to rounding.
            let padding = camera.world_units_per_pixel(viewport, center) * (px + 2.0);
            if !padding.is_finite() || (padding - self.objects[index].pick_padding).abs() < 1.0e-12
            {
                continue;
            }
            self.objects[index].pick_padding = padding;
            changed += 1;
            if !self.dirty {
                let bounds = self.objects[index].world_bounds();
                self.bvh
                    .update_bounds(u32::try_from(index).unwrap_or(u32::MAX), bounds);
            }
        }
        if changed > 0 && !self.dirty {
            self.bvh.refit();
        }
        changed
    }

    /// Rebuild the broad phase if anything structural changed. Idempotent.
    pub fn rebuild(&mut self) {
        if !self.dirty {
            return;
        }
        let bounds: Vec<Aabb> = self.objects.iter().map(PickObject::world_bounds).collect();
        self.bvh = SceneBvh::build(&bounds);
        self.dirty = false;
    }

    /// How degraded the refitted tree is; above ~2, a [`PickScene::force_rebuild`] pays for itself.
    #[must_use]
    pub fn broad_phase_quality(&self) -> f64 {
        self.bvh.refit_quality()
    }

    pub fn force_rebuild(&mut self) {
        self.dirty = true;
        self.rebuild();
    }

    #[must_use]
    pub fn scene_bounds(&self) -> Aabb {
        self.objects
            .iter()
            .filter(|o| o.visible)
            .fold(Aabb::EMPTY, |acc, o| acc.union(&o.world_bounds()))
    }
}

/// Reusable scratch buffers plus the query entry points.
///
/// Held across queries on purpose: hover runs on every pointer move, and allocating traversal stacks
/// there would put a heap allocation on the interaction hot path.
#[derive(Debug, Default)]
pub struct Picker {
    node_stack: Vec<u32>,
    mesh_stack: Vec<u32>,
    candidates: Vec<(u32, f64)>,
    hits: Vec<HitResult>,
    keys: Vec<u32>,
}

impl Picker {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Every hit under the ray, ordered by [`HitResult::sort_key`].
    ///
    /// Returns the whole ordered list rather than one answer, because "what else is under here" is
    /// what alt-click cycling, a candidate menu, and click-through all need — and recomputing the
    /// query to answer it is both slower and liable to give a different answer.
    pub fn pick_all(
        &mut self,
        scene: &mut PickScene,
        camera: &Camera,
        viewport: &Viewport,
        ray: &Ray,
        filter: &PickFilter,
    ) -> &[HitResult] {
        // Pixel-sized proxies need world-space padding for the current view before the broad phase
        // can offer them. Doing it here rather than trusting every call site to remember is the
        // difference between "lights are selectable" and "lights are selectable if you called the
        // right function first".
        scene.refresh_screen_padding(camera, viewport);
        scene.rebuild();
        self.hits.clear();
        scene.bvh.query_ray(
            ray,
            filter.max_distance,
            &mut self.node_stack,
            &mut self.candidates,
        );

        // Take the candidate list out so the borrow checker allows `&scene` reads in the loop.
        let candidates = std::mem::take(&mut self.candidates);
        for &(key, _t_enter) in &candidates {
            let index = key as usize;
            let Some(object) = scene.objects.get(index) else {
                continue;
            };
            if !passes(object, filter) {
                continue;
            }
            if let Some(hit) = narrow_phase(
                object,
                index,
                scene,
                camera,
                viewport,
                ray,
                filter,
                &mut self.mesh_stack,
            ) {
                self.hits.push(hit);
            }
        }
        self.candidates = candidates;

        self.hits.sort_by_key(HitResult::sort_key);

        // `PreferOpaque` is applied after ordering so it can fall back honestly: transparent hits are
        // dropped only when an opaque one survives, so clicking a lone glass panel still selects it.
        if matches!(filter.transparency, TransparencyPolicy::PreferOpaque) {
            let has_opaque = self.hits.iter().any(|h| !is_transparent(scene, h.object));
            if has_opaque {
                let objects = &scene.objects;
                self.hits.retain(|h| {
                    objects
                        .get(h.object as usize)
                        .is_none_or(|o| !o.transparent)
                });
            }
        }
        &self.hits
    }

    /// The single hit a plain click resolves to.
    pub fn pick_nearest(
        &mut self,
        scene: &mut PickScene,
        camera: &Camera,
        viewport: &Viewport,
        ray: &Ray,
        filter: &PickFilter,
    ) -> Option<HitResult> {
        self.pick_all(scene, camera, viewport, ray, filter)
            .first()
            .copied()
    }

    /// Click-through: the hit **after** `previous` in the ordered list, wrapping around.
    ///
    /// This is how obscured geometry stays reachable. Wrapping (rather than stopping at the end)
    /// means repeated clicks cycle forever instead of getting stuck on the last candidate, which is
    /// what makes the gesture learnable without a manual.
    pub fn pick_cycle(
        &mut self,
        scene: &mut PickScene,
        camera: &Camera,
        viewport: &Viewport,
        ray: &Ray,
        filter: &PickFilter,
        previous: Option<(u64, u32)>,
    ) -> Option<HitResult> {
        let hits = self.pick_all(scene, camera, viewport, ray, filter);
        if hits.is_empty() {
            return None;
        }
        let Some((key, instance)) = previous else {
            return hits.first().copied();
        };
        match hits
            .iter()
            .position(|h| h.key == key && h.instance == instance)
        {
            Some(i) => hits.get((i + 1) % hits.len()).copied(),
            None => hits.first().copied(),
        }
    }

    /// Marquee selection: every object whose projected bounds meet `rect`.
    ///
    /// The touch-versus-enclose policy comes from the rectangle itself ([`ScreenRect::mode`]), so it
    /// is decided once at the input boundary rather than by each caller passing a flag it may or may
    /// not have thought about. Leaving that to chance is why box selection feels different in every
    /// tool that has one.
    pub fn pick_region(
        &mut self,
        scene: &mut PickScene,
        camera: &Camera,
        viewport: &Viewport,
        rect: ScreenRect,
        filter: &PickFilter,
    ) -> Vec<(u64, u32)> {
        let (rect_min_ndc, rect_max_ndc) = (rect.min, rect.max);
        let require_enclosed = matches!(rect.mode(), RegionMode::Enclose);
        scene.rebuild();
        let aspect = viewport.aspect();
        let mut out = Vec::new();
        self.keys.clear();
        for object in &scene.objects {
            if !passes(object, filter) {
                continue;
            }
            let world = object.world_bounds();
            if world.is_empty() {
                continue;
            }
            // Project the eight corners. An object with any corner behind the eye is treated as
            // touching (it straddles the camera plane), which is the conservative answer: a marquee
            // that silently skips geometry crossing the near plane looks like a broken selection.
            let mut lo = [f64::INFINITY; 2];
            let mut hi = [f64::NEG_INFINITY; 2];
            let mut behind = false;
            for c in world.corners() {
                match camera.world_to_ndc(c, aspect) {
                    Some(n) => {
                        lo[0] = lo[0].min(n[0]);
                        lo[1] = lo[1].min(n[1]);
                        hi[0] = hi[0].max(n[0]);
                        hi[1] = hi[1].max(n[1]);
                    }
                    None => behind = true,
                }
            }
            if lo[0] > hi[0] {
                continue; // entirely behind the camera
            }
            let touches = lo[0] <= rect_max_ndc[0]
                && hi[0] >= rect_min_ndc[0]
                && lo[1] <= rect_max_ndc[1]
                && hi[1] >= rect_min_ndc[1];
            let enclosed = !behind
                && lo[0] >= rect_min_ndc[0]
                && hi[0] <= rect_max_ndc[0]
                && lo[1] >= rect_min_ndc[1]
                && hi[1] <= rect_max_ndc[1];
            let selected = if require_enclosed { enclosed } else { touches };
            if selected {
                out.push((object.key, object.instance));
            }
        }
        out.sort_unstable(); // deterministic order, independent of object storage order
        out.dedup();
        out
    }

    /// The hits from the last query, for a caller that wants the candidate list after picking.
    #[must_use]
    pub fn last_hits(&self) -> &[HitResult] {
        &self.hits
    }
}

fn is_transparent(scene: &PickScene, object: u32) -> bool {
    scene
        .objects
        .get(object as usize)
        .is_some_and(|o| o.transparent)
}

fn passes(object: &PickObject, filter: &PickFilter) -> bool {
    if !filter.allows(object.kind) {
        return false;
    }
    if !object.visible && !filter.include_hidden {
        return false;
    }
    if !object.selectable && !filter.include_unselectable {
        return false;
    }
    if object.locked && !filter.include_locked {
        return false;
    }
    if matches!(filter.transparency, TransparencyPolicy::Ignore) && object.transparent {
        return false;
    }
    if filter.exclude_key == Some(object.key) {
        return false;
    }
    true
}

// One dispatch over the geometry kinds, sharing the local-space ray and the result assembly. Split
// into four functions, each would need the same eight parameters plus a way to build the shared
// `HitResult`, which is more moving parts for the same work.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn narrow_phase(
    object: &PickObject,
    index: usize,
    scene: &PickScene,
    camera: &Camera,
    viewport: &Viewport,
    ray: &Ray,
    filter: &PickFilter,
    stack: &mut Vec<u32>,
) -> Option<HitResult> {
    let inv = object.inv_world?;
    let (local_ray, dir_scale) = ray.transformed(inv)?;
    let object_index = u32::try_from(index).unwrap_or(u32::MAX);
    let base = |distance: f64,
                local_position: Vec3,
                local_normal: Vec3,
                primitive: Option<u32>,
                barycentric: Option<[f64; 3]>,
                source: HitSource,
                screen_distance_px: f64,
                back_face: bool|
     -> HitResult {
        let world_position = ray.at(distance);
        // Normals transform by the inverse-transpose; using the matrix directly skews them under
        // non-uniform scale, which is exactly when a normal matters most.
        let world_normal =
            normalize_or(transform_dir(transpose(inv), local_normal), [0.0, 1.0, 0.0]);
        let facing = dot(world_normal, ray.direction);
        HitResult {
            key: object.key,
            instance: object.instance,
            object: object_index,
            primitive,
            distance,
            world_position,
            local_position,
            world_normal: if facing > 0.0 {
                [-world_normal[0], -world_normal[1], -world_normal[2]]
            } else {
                world_normal
            },
            local_normal,
            barycentric,
            kind: object.kind,
            source,
            screen_distance_px,
            back_face,
            priority: object.priority,
            locked: object.locked,
        }
    };

    match object.geometry {
        PickGeometry::Mesh(mesh_index) => {
            let mesh = scene.meshes.get(mesh_index as usize)?;
            // `t_max` is in LOCAL units: a world limit must be scaled by how much the transform
            // stretched the ray, or a 0.001-scaled CAD part rejects every hit as too far.
            let local_max = if filter.max_distance.is_finite() {
                filter.max_distance * dir_scale
            } else {
                f64::INFINITY
            };
            let hit = mesh
                .bvh
                .raycast(&local_ray, &mesh.source(), local_max, stack)?;
            let world_t = hit.hit.t / dir_scale;
            if world_t > filter.max_distance {
                return None;
            }
            let local_position = local_ray.at(hit.hit.t);
            let tri = mesh.source_triangle(hit.triangle)?;
            let local_normal = crate::ray::triangle_normal(tri[0], tri[1], tri[2]);
            Some(base(
                world_t,
                local_position,
                local_normal,
                Some(hit.triangle),
                Some([1.0 - hit.hit.u - hit.hit.v, hit.hit.u, hit.hit.v]),
                HitSource::Triangle,
                0.0,
                hit.hit.back_face,
            ))
        }
        PickGeometry::Bounds => {
            let local_max = if filter.max_distance.is_finite() {
                filter.max_distance * dir_scale
            } else {
                f64::INFINITY
            };
            let (t_enter, t_exit) = ray_aabb(&local_ray, &object.local_bounds, local_max)?;
            // Inside the box, `t_enter` is clamped to 0; use the exit face so "the camera is inside
            // the object" still resolves to a real surface rather than to the ray's own origin.
            let t = if t_enter > epsilon::RAY_T_MIN {
                t_enter
            } else {
                t_exit
            };
            let world_t = t / dir_scale;
            if world_t > filter.max_distance {
                return None;
            }
            let local_position = local_ray.at(t);
            let local_normal = box_normal(&object.local_bounds, local_position);
            Some(base(
                world_t,
                local_position,
                local_normal,
                None,
                None,
                HitSource::Bounds,
                0.0,
                t_enter <= epsilon::RAY_T_MIN,
            ))
        }
        PickGeometry::Point { screen_radius_px } => {
            let center_world =
                crate::transform::transform_point4(object.world, object.local_bounds.center())?;
            let radius_px = (screen_radius_px + filter.extra_tolerance_px).max(1.0);
            let world_radius = camera.world_units_per_pixel(viewport, center_world) * radius_px;
            let t = ray_sphere(ray, center_world, world_radius, filter.max_distance)?;
            let screen_px = ray.distance_to_point(center_world)
                / camera
                    .world_units_per_pixel(viewport, center_world)
                    .max(1.0e-12);
            // Report the distance to the proxy's CENTRE, not to its sphere surface: a light's depth
            // is where the light is, and sorting by a sphere-surface distance would make a bigger
            // icon read as nearer.
            let center_t = ray.project_parameter(center_world).max(t);
            let local_center = object.local_bounds.center();
            Some(base(
                center_t,
                local_center,
                [0.0, 1.0, 0.0],
                None,
                None,
                HitSource::Proxy,
                screen_px,
                false,
            ))
        }
        PickGeometry::Polyline {
            index: poly,
            screen_radius_px,
        } => {
            let points = scene.polylines.get(poly as usize)?;
            if points.len() < 2 {
                return None;
            }
            let radius_px = (screen_radius_px + filter.extra_tolerance_px).max(1.0);
            let mut best: Option<(f64, f64, Vec3)> = None;
            for w in points.windows(2) {
                let a = crate::transform::transform_point4(object.world, w[0])?;
                let b = crate::transform::transform_point4(object.world, w[1])?;
                let distance = crate::ray::ray_segment_distance(ray, a, b);
                let mid = [
                    f64::midpoint(a[0], b[0]),
                    f64::midpoint(a[1], b[1]),
                    f64::midpoint(a[2], b[2]),
                ];
                let upp = camera.world_units_per_pixel(viewport, mid).max(1.0e-12);
                let px = distance / upp;
                if px > radius_px {
                    continue;
                }
                let t = ray.project_parameter(mid);
                if t < 0.0 || t > filter.max_distance {
                    continue;
                }
                if best.is_none_or(|(bt, _, _)| t < bt) {
                    best = Some((t, px, mid));
                }
            }
            let (t, px, _) = best?;
            let local_position = crate::transform::transform_point4(inv, ray.at(t))?;
            Some(base(
                t,
                local_position,
                [0.0, 1.0, 0.0],
                None,
                None,
                HitSource::Proxy,
                px,
                false,
            ))
        }
    }
}

impl PickMesh {
    fn source_triangle(&self, index: u32) -> Option<[Vec3; 3]> {
        use crate::bvh::TriangleSource;
        self.source().triangle(index as usize)
    }
}

fn box_normal(bounds: &Aabb, p: Vec3) -> Vec3 {
    // The face whose plane the point is nearest to, scaled by the box's own size so a flat box does
    // not always report its thin axis.
    let center = bounds.center();
    let half = bounds.half_size();
    let mut best = 0usize;
    let mut best_score = f64::NEG_INFINITY;
    for i in 0..3 {
        let h = half[i].max(1.0e-12);
        let score = ((p[i] - center[i]) / h).abs();
        if score > best_score {
            best_score = score;
            best = i;
        }
    }
    let mut n = [0.0; 3];
    n[best] = if p[best] >= center[best] { 1.0 } else { -1.0 };
    n
}

fn transpose(m: Mat4) -> Mat4 {
    let mut out = [[0.0; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            out[c][r] = m[r][c];
        }
    }
    out
}

fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize_or(v: Vec3, fallback: Vec3) -> Vec3 {
    let len_sq = dot(v, v);
    if !len_sq.is_finite() || len_sq < epsilon::DIRECTION_LEN_SQ {
        return fallback;
    }
    let inv = len_sq.sqrt().recip();
    [v[0] * inv, v[1] * inv, v[2] * inv]
}

// ── selection state ──────────────────────────────────────────────────────────────────────────────

/// **The one selection.** Ordered, with an explicit active object.
///
/// Existing as a type at all is the point: the viewport, the outliner and the inspector previously
/// each kept their own copy of "what is selected" — one a render-list index, one a string id, one a
/// React state — and they drifted apart whenever the scene changed underneath them. One model with
/// one mutation surface makes disagreement impossible rather than merely unlikely.
///
/// Order is preserved because it carries meaning: the **active** object is the one the inspector
/// shows and the one an "active object" pivot mode uses, and it is the most recently added, not an
/// arbitrary member of a set.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionModel {
    entries: Vec<(u64, u32)>,
}

impl SelectionModel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Selection order, oldest first.
    #[must_use]
    pub fn entries(&self) -> &[(u64, u32)] {
        &self.entries
    }

    /// The primary object: the most recently added. `None` when nothing is selected.
    #[must_use]
    pub fn active(&self) -> Option<(u64, u32)> {
        self.entries.last().copied()
    }

    #[must_use]
    pub fn contains(&self, key: u64, instance: u32) -> bool {
        self.entries.contains(&(key, instance))
    }

    /// Whether any instance of `key` is selected — what the outliner highlights a row on.
    #[must_use]
    pub fn contains_key(&self, key: u64) -> bool {
        self.entries.iter().any(|(k, _)| *k == key)
    }

    /// Replace the selection with one object.
    pub fn set(&mut self, key: u64, instance: u32) {
        self.entries.clear();
        self.entries.push((key, instance));
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Add, promoting an already-selected object to active — Shift-click.
    pub fn add(&mut self, key: u64, instance: u32) {
        self.entries.retain(|e| *e != (key, instance));
        self.entries.push((key, instance));
    }

    /// Add or remove — Ctrl/Cmd-click. Returns whether the object ended up selected.
    pub fn toggle(&mut self, key: u64, instance: u32) -> bool {
        if let Some(pos) = self.entries.iter().position(|e| *e == (key, instance)) {
            self.entries.remove(pos);
            false
        } else {
            self.entries.push((key, instance));
            true
        }
    }

    pub fn remove(&mut self, key: u64, instance: u32) {
        self.entries.retain(|e| *e != (key, instance));
    }

    /// Drop every entry whose key is no longer in the scene.
    ///
    /// Called after any structural change. Without it a selection outlives the object it names, and
    /// the inspector shows a phantom while the gizmo edits nothing — the exact failure of an
    /// index-based selection after an undo.
    pub fn retain_live(&mut self, is_live: impl Fn(u64, u32) -> bool) -> bool {
        let before = self.entries.len();
        self.entries.retain(|(k, i)| is_live(*k, *i));
        before != self.entries.len()
    }

    /// Apply a click with the platform's modifier conventions.
    ///
    /// `hit` is `None` for a click on empty space, which **clears** the selection unless a modifier
    /// says otherwise — the behaviour that was impossible before, because the old picker could never
    /// report empty space.
    pub fn apply_click(&mut self, hit: Option<(u64, u32)>, modifiers: ClickModifiers) -> bool {
        let before = self.entries.clone();
        match hit {
            Some((key, instance)) => {
                if modifiers.toggle {
                    self.toggle(key, instance);
                } else if modifiers.extend {
                    self.add(key, instance);
                } else {
                    self.set(key, instance);
                }
            }
            None => {
                if !modifiers.toggle && !modifiers.extend {
                    self.clear();
                }
            }
        }
        before != self.entries
    }
}

/// Modifier state for a selection click, named by intent rather than by key so the platform mapping
/// lives at the input boundary (Cmd on macOS, Ctrl elsewhere) instead of in the selection logic.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClickModifiers {
    /// Shift: add to the selection.
    pub extend: bool,
    /// Ctrl/Cmd: toggle membership.
    pub toggle: bool,
}

/// A single-shot request, for callers that would rather pass one struct than six arguments.
#[derive(Clone, Copy, Debug)]
pub struct PickRequest {
    pub ray: Ray,
    pub filter: PickFilter,
}

/// Which objects a marquee takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionMode {
    /// Anything the rectangle overlaps at all — forgiving, and what most people expect by default.
    Touch,
    /// Only objects entirely inside the rectangle — precise, and the convention CAD tools give to a
    /// right-to-left drag.
    Enclose,
}

/// A marquee rectangle in NDC, carrying the drag direction it was made from.
///
/// The direction is kept because it *is* the policy: dragging left-to-right means "take what I
/// enclosed", right-to-left means "take what I touched". Deriving the mode here means every caller
/// gets the same convention instead of each one choosing a flag.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenRect {
    pub min: [f64; 2],
    pub max: [f64; 2],
    /// `true` when the drag ran right-to-left.
    pub reversed: bool,
}

impl ScreenRect {
    /// From the two NDC corners of a drag, in the order they happened.
    #[must_use]
    pub fn from_drag(start: [f64; 2], end: [f64; 2]) -> Self {
        Self {
            min: [start[0].min(end[0]), start[1].min(end[1])],
            max: [start[0].max(end[0]), start[1].max(end[1])],
            reversed: end[0] < start[0],
        }
    }

    /// A rectangle with an explicit mode, for callers that are not driven by a drag.
    #[must_use]
    pub fn with_mode(min: [f64; 2], max: [f64; 2], mode: RegionMode) -> Self {
        Self {
            min: [min[0].min(max[0]), min[1].min(max[1])],
            max: [min[0].max(max[0]), min[1].max(max[1])],
            reversed: matches!(mode, RegionMode::Touch),
        }
    }

    #[must_use]
    pub fn mode(&self) -> RegionMode {
        if self.reversed {
            RegionMode::Touch
        } else {
            RegionMode::Enclose
        }
    }

    /// Whether the drag covered enough ground to be a marquee rather than a click.
    #[must_use]
    pub fn is_degenerate(&self) -> bool {
        (self.max[0] - self.min[0]).abs() < 1.0e-9 && (self.max[1] - self.min[1]).abs() < 1.0e-9
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Projection;
    use crate::epsilon::approx_eq;
    use crate::transform::Transform;

    fn camera() -> Camera {
        Camera {
            eye: [0.0, 0.0, 20.0],
            target: [0.0; 3],
            up: [0.0, 1.0, 0.0],
            projection: Projection::Perspective {
                fov_y: 55.0_f64.to_radians(),
            },
            near: 0.01,
            far: 1000.0,
        }
    }

    fn viewport() -> Viewport {
        Viewport::from_physical(1920.0, 1080.0, 1.0)
    }

    /// A unit cube centred on the origin, as 12 triangles.
    fn cube_mesh() -> PickMesh {
        let p: Vec<[f32; 3]> = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let i: Vec<u32> = vec![
            0, 2, 1, 0, 3, 2, // -Z
            4, 5, 6, 4, 6, 7, // +Z
            0, 1, 5, 0, 5, 4, // -Y
            3, 7, 6, 3, 6, 2, // +Y
            0, 4, 7, 0, 7, 3, // -X
            1, 2, 6, 1, 6, 5, // +X
        ];
        PickMesh::build(p, i)
    }

    fn scene_with_cubes(positions: &[Vec3]) -> PickScene {
        let mut scene = PickScene::new();
        scene.meshes.push(cube_mesh());
        let local = scene.meshes[0].local_bounds();
        for (i, p) in positions.iter().enumerate() {
            let world = Transform::from_translation(*p).to_matrix();
            let mut o =
                PickObject::new(i as u64, world, local, PickGeometry::Mesh(0), HitKind::Mesh);
            o.instance = 0;
            scene.push_object(o);
        }
        scene
    }

    #[test]
    fn clicking_empty_space_returns_nothing() {
        // The headline regression. The previous picker returned a hit for ANY non-empty scene, which
        // made click-to-deselect literally unreachable code.
        let mut scene = scene_with_cubes(&[[0.0; 3]]);
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let miss = c.ray_through_ndc(-0.98, 0.98, vp.aspect());
        assert!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &miss, &PickFilter::default())
                .is_none(),
            "a click in the corner of an almost-empty viewport must hit nothing"
        );
        // …and a click on the object still hits.
        let on = c.ray_through_ndc(0.0, 0.0, vp.aspect());
        assert!(picker
            .pick_nearest(&mut scene, &c, &vp, &on, &PickFilter::default())
            .is_some());
    }

    #[test]
    fn a_click_anywhere_on_a_large_object_selects_it() {
        // The old metric ranked by distance to the projected PIVOT, so a big object could only be
        // clicked near its centre and a small neighbour stole clicks on the big one's face.
        let mut scene = PickScene::new();
        scene.meshes.push(cube_mesh());
        let local = scene.meshes[0].local_bounds();
        // A large cube at the origin...
        scene.push_object(PickObject::new(
            1,
            Transform {
                scale: [8.0, 8.0, 1.0],
                ..Transform::IDENTITY
            }
            .to_matrix(),
            local,
            PickGeometry::Mesh(0),
            HitKind::Mesh,
        ));
        // ...and a small one just off to the side, whose pivot is nearer the cursor in screen space.
        scene.push_object(PickObject::new(
            2,
            Transform {
                translation: [6.0, 6.0, -6.0],
                scale: [0.2; 3],
                ..Transform::IDENTITY
            }
            .to_matrix(),
            local,
            PickGeometry::Mesh(0),
            HitKind::Mesh,
        ));
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        // Aim at the big cube's upper-right FACE, far from its pivot and near the small cube's pivot.
        let ray = Ray::new([6.0, 6.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("hits the face");
        assert_eq!(
            hit.key, 1,
            "the surface under the cursor wins, not the nearest pivot"
        );
        assert_eq!(hit.source, HitSource::Triangle);
        assert!(
            approx_eq(hit.world_position[2], 1.0),
            "on the +Z face: {:?}",
            hit.world_position
        );
    }

    #[test]
    fn occlusion_is_respected_the_nearest_visible_surface_wins() {
        let mut scene = scene_with_cubes(&[[0.0, 0.0, 5.0], [0.0, 0.0, 0.0], [0.0, 0.0, -5.0]]);
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        let hits = picker
            .pick_all(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .to_vec();
        assert_eq!(hits.len(), 3, "all three are candidates");
        assert_eq!(hits[0].key, 0, "the nearest is first");
        assert!(hits[0].distance < hits[1].distance && hits[1].distance < hits[2].distance);
        // The hidden ones are still REACHABLE, which is what makes click-through possible.
        let second = picker
            .pick_cycle(
                &mut scene,
                &c,
                &vp,
                &ray,
                &PickFilter::default(),
                Some((0, 0)),
            )
            .expect("cycles");
        assert_eq!(second.key, 1);
        let third = picker
            .pick_cycle(
                &mut scene,
                &c,
                &vp,
                &ray,
                &PickFilter::default(),
                Some((1, 0)),
            )
            .expect("cycles");
        assert_eq!(third.key, 2);
        // …and it wraps rather than getting stuck.
        let wrapped = picker
            .pick_cycle(
                &mut scene,
                &c,
                &vp,
                &ray,
                &PickFilter::default(),
                Some((2, 0)),
            )
            .expect("wraps");
        assert_eq!(wrapped.key, 0);
    }

    #[test]
    fn selection_is_deterministic_for_coincident_geometry() {
        // Two identical cubes in exactly the same place. Repeated identical clicks must not
        // alternate — a selection that flickers between two objects is worse than picking either.
        let mut scene = scene_with_cubes(&[[0.0; 3], [0.0; 3]]);
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        let first = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("hit");
        for _ in 0..50 {
            let again = picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
                .expect("hit");
            assert_eq!(
                again.key, first.key,
                "the same click always resolves the same way"
            );
        }
        assert_eq!(first.key, 0, "and the tie breaks on the stable key");
    }

    #[test]
    fn hidden_and_unselectable_objects_obey_the_stated_policy() {
        let mut scene = scene_with_cubes(&[[0.0, 0.0, 5.0], [0.0, 0.0, 0.0]]);
        scene.objects[0].visible = false;
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("the visible one behind it");
        assert_eq!(hit.key, 1, "a hidden object does not intercept the click");

        // Explicitly asking for hidden objects finds it again — the policy is a filter, not a hole.
        let show_hidden = PickFilter {
            include_hidden: true,
            ..PickFilter::default()
        };
        assert_eq!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &show_hidden)
                .unwrap()
                .key,
            0
        );

        // Non-selectable annotation geometry is skipped but still drawn (visible stays true).
        scene.objects[0].visible = true;
        scene.objects[0].selectable = false;
        assert_eq!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
                .unwrap()
                .key,
            1
        );

        // Locked objects ARE selectable by default (you can inspect them) and report it.
        scene.objects[0].selectable = true;
        scene.objects[0].locked = true;
        let locked = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .unwrap();
        assert_eq!(locked.key, 0);
        assert!(
            locked.locked,
            "the hit carries the lock so the caller can refuse a transform"
        );
    }

    #[test]
    fn point_proxies_make_lights_and_cameras_clickable_without_pixel_precision() {
        // A light has no geometry. Requiring an exact intersection with a 16-pixel icon is what made
        // lights and cameras unselectable in the viewport — they were not even in the pick list.
        let mut scene = PickScene::new();
        scene.push_object(PickObject::new(
            42,
            Transform::from_translation([2.0, 1.0, 0.0]).to_matrix(),
            Aabb::point([0.0; 3]),
            PickGeometry::Point {
                screen_radius_px: 12.0,
            },
            HitKind::Light,
        ));
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        // Aim a few pixels away from the light's exact centre.
        let centre_ndc = c
            .world_to_ndc([2.0, 1.0, 0.0], vp.aspect())
            .expect("in front");
        let (px, py) = vp.pixel_in_ndc();
        let near_miss = c.ray_through_ndc(
            centre_ndc[0] + px * 6.0,
            centre_ndc[1] + py * 6.0,
            vp.aspect(),
        );
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &near_miss, &PickFilter::default())
            .expect("a near miss still selects the icon");
        assert_eq!(hit.key, 42);
        assert_eq!(hit.source, HitSource::Proxy);
        assert!(
            hit.screen_distance_px > 1.0 && hit.screen_distance_px < 12.0,
            "{}",
            hit.screen_distance_px
        );

        // Far outside the icon, nothing.
        let far = c.ray_through_ndc(centre_ndc[0] + px * 60.0, centre_ndc[1], vp.aspect());
        assert!(picker
            .pick_nearest(&mut scene, &c, &vp, &far, &PickFilter::default())
            .is_none());
    }

    #[test]
    fn a_light_icon_outranks_the_wall_behind_it() {
        let mut scene = scene_with_cubes(&[[0.0, 0.0, 0.0]]);
        scene.push_object(PickObject::new(
            99,
            Transform::from_translation([0.0, 0.0, -3.0]).to_matrix(),
            Aabb::point([0.0; 3]),
            PickGeometry::Point {
                screen_radius_px: 14.0,
            },
            HitKind::Light,
        ));
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("hit");
        assert_eq!(
            hit.key, 99,
            "clicking an overlay icon selects the light, even though a cube is nearer"
        );
        // The cube is still reachable by cycling, and by filtering to meshes only.
        let meshes_only = PickFilter::only(&[HitKind::Mesh]);
        assert_eq!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &meshes_only)
                .unwrap()
                .key,
            0
        );
    }

    #[test]
    fn instanced_objects_report_which_instance_was_hit() {
        let mut scene = PickScene::new();
        scene.meshes.push(cube_mesh());
        let local = scene.meshes[0].local_bounds();
        // 6,000 instances of ONE mesh — the case that must not need 6,000 BVHs.
        for i in 0..6_000u32 {
            let mut o = PickObject::new(
                7, // the same entity key throughout
                Transform::from_translation([f64::from(i) * 3.0, 0.0, 0.0]).to_matrix(),
                local,
                PickGeometry::Mesh(0),
                HitKind::Mesh,
            );
            o.instance = i;
            scene.push_object(o);
        }
        assert_eq!(
            scene.meshes.len(),
            1,
            "one shared mesh BVH for 6,000 instances"
        );
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([5_284.0 * 3.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("hit");
        assert_eq!(hit.key, 7);
        assert_eq!(
            hit.instance, 5_284,
            "the INSTANCE is the answer, not the source mesh"
        );
    }

    #[test]
    fn hits_carry_the_geometry_a_downstream_tool_needs() {
        let mut scene = scene_with_cubes(&[[0.0; 3]]);
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.3, -0.2, 20.0], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("hit");
        assert!(approx_eq(hit.world_position[2], 1.0), "on the +Z face");
        assert!(
            approx_eq(hit.local_position[2], 1.0),
            "local == world for an untransformed cube"
        );
        // The normal faces the viewer, which is what a decal or a placement tool needs.
        assert!(
            hit.world_normal[2] > 0.9,
            "outward normal: {:?}",
            hit.world_normal
        );
        assert!(hit.primitive.is_some(), "the triangle is identified");
        let b = hit.barycentric.expect("barycentrics");
        assert!(
            approx_eq(b[0] + b[1] + b[2], 1.0),
            "barycentrics sum to 1: {b:?}"
        );
        assert!(b.iter().all(|w| *w >= -1e-9), "and are inside the triangle");
    }

    #[test]
    fn normals_are_correct_under_non_uniform_scale() {
        // Normals transform by the inverse-transpose. Using the matrix directly skews them, and a
        // skewed normal is invisible in the pick itself but wrong for every tool that consumes it.
        let mut scene = PickScene::new();
        scene.meshes.push(cube_mesh());
        let local = scene.meshes[0].local_bounds();
        let world = Transform {
            rotation: Transform::from_axis_angle([0.0, 0.0, 1.0], 45f64.to_radians()),
            scale: [4.0, 0.25, 1.0],
            ..Transform::IDENTITY
        }
        .to_matrix();
        scene.push_object(PickObject::new(
            1,
            world,
            local,
            PickGeometry::Mesh(0),
            HitKind::Mesh,
        ));
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("hit");
        // The +Z face is unaffected by the X/Y scale and the Z rotation, so its world normal is +Z.
        assert!(
            hit.world_normal[2] > 0.999,
            "inverse-transpose keeps the normal perpendicular: {:?}",
            hit.world_normal
        );
        let len = (hit.world_normal.iter().map(|c| c * c).sum::<f64>()).sqrt();
        assert!(approx_eq(len, 1.0), "and unit length");
    }

    #[test]
    fn a_singular_transform_is_skipped_rather_than_producing_nan() {
        let mut scene = scene_with_cubes(&[[0.0; 3]]);
        scene.objects[0].world = Transform {
            scale: [0.0; 3],
            ..Transform::IDENTITY
        }
        .to_matrix();
        scene.objects[0].inv_world = mat_inverse(scene.objects[0].world);
        scene.force_rebuild();
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        assert!(picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .is_none());
    }

    #[test]
    fn a_tiny_part_far_from_the_origin_is_still_pickable_at_its_true_scale() {
        // A millimetre CAD part ten kilometres out, with a 0.001 instance scale — the combination
        // that breaks an f32 picker and a picker that forgets to rescale its distance limit.
        let mut scene = PickScene::new();
        scene.meshes.push(cube_mesh());
        let local = scene.meshes[0].local_bounds();
        let world = Transform {
            translation: [10_000.0, 0.0, 0.0],
            scale: [0.001; 3],
            ..Transform::IDENTITY
        }
        .to_matrix();
        scene.push_object(PickObject::new(
            5,
            world,
            local,
            PickGeometry::Mesh(0),
            HitKind::Mesh,
        ));
        let c = Camera {
            eye: [10_000.0, 0.0, 0.05],
            target: [10_000.0, 0.0, 0.0],
            ..camera()
        };
        let vp = viewport();
        let mut picker = Picker::new();
        let ray = Ray::new([10_000.0, 0.0, 0.05], [0.0, 0.0, -1.0]);
        let hit = picker
            .pick_nearest(&mut scene, &c, &vp, &ray, &PickFilter::default())
            .expect("the millimetre part is hit");
        assert_eq!(hit.key, 5);
        // The reported distance is in WORLD units: the cube's +Z face is at z = 0.001.
        assert!(
            approx_eq(hit.distance, 0.049),
            "world distance, got {}",
            hit.distance
        );
        assert!(
            approx_eq(hit.world_position[0], 10_000.0),
            "and the hit point keeps its full magnitude: {:?}",
            hit.world_position
        );
    }

    #[test]
    fn transparency_policy_is_explicit_and_falls_back_honestly() {
        let mut scene = scene_with_cubes(&[[0.0, 0.0, 5.0], [0.0, 0.0, 0.0]]);
        scene.objects[0].transparent = true;
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);

        let selectable = PickFilter::default();
        assert_eq!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &selectable)
                .unwrap()
                .key,
            0,
            "by default you can click the glass"
        );
        let prefer_opaque = PickFilter {
            transparency: TransparencyPolicy::PreferOpaque,
            ..PickFilter::default()
        };
        assert_eq!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &prefer_opaque)
                .unwrap()
                .key,
            1,
            "prefer-opaque reaches through to the solid behind"
        );
        // …but a LONE transparent object is still selectable — the fallback is what keeps the policy
        // from turning glass into a hole in the scene.
        scene.objects[1].visible = false;
        scene.force_rebuild();
        assert_eq!(
            picker
                .pick_nearest(&mut scene, &c, &vp, &ray, &prefer_opaque)
                .unwrap()
                .key,
            0
        );
    }

    #[test]
    fn marquee_selection_supports_both_touch_and_enclose() {
        let mut scene = scene_with_cubes(&[[-6.0, 0.0, 0.0], [0.0, 0.0, 0.0], [6.0, 0.0, 0.0]]);
        let (c, vp) = (camera(), viewport());
        let mut picker = Picker::new();
        // A rectangle around the centre cube that only clips the edges of its neighbours.
        let centre = c.world_to_ndc([0.0; 3], vp.aspect()).unwrap();
        let left_edge = c.world_to_ndc([-5.0, 0.0, 0.0], vp.aspect()).unwrap();
        let right_edge = c.world_to_ndc([5.0, 0.0, 0.0], vp.aspect()).unwrap();
        let rect_min = [left_edge[0], centre[1] - 0.5];
        let rect_max = [right_edge[0], centre[1] + 0.5];

        // A right-to-left drag means TOUCH: everything the rectangle clips is taken.
        let touching = picker.pick_region(
            &mut scene,
            &c,
            &vp,
            ScreenRect::from_drag([rect_max[0], rect_max[1]], [rect_min[0], rect_min[1]]),
            &PickFilter::default(),
        );
        assert!(
            touching.len() >= 3,
            "touch mode catches the clipped neighbours: {touching:?}"
        );

        // Left-to-right means ENCLOSE: only what is fully inside.
        let enclosed = picker.pick_region(
            &mut scene,
            &c,
            &vp,
            ScreenRect::from_drag([rect_min[0], rect_min[1]], [rect_max[0], rect_max[1]]),
            &PickFilter::default(),
        );
        assert_eq!(
            enclosed,
            vec![(1, 0)],
            "enclose mode takes only the fully-inside cube"
        );

        // The direction IS the policy, and a rectangle knows which it is.
        assert_eq!(
            ScreenRect::from_drag([0.0, 0.0], [0.5, 0.5]).mode(),
            RegionMode::Enclose
        );
        assert_eq!(
            ScreenRect::from_drag([0.5, 0.5], [0.0, 0.0]).mode(),
            RegionMode::Touch
        );
        assert!(ScreenRect::from_drag([0.2, 0.2], [0.2, 0.2]).is_degenerate());
    }

    #[test]
    fn selection_model_is_one_ordered_source_of_truth() {
        let mut s = SelectionModel::new();
        assert!(s.is_empty() && s.active().is_none());

        s.apply_click(Some((1, 0)), ClickModifiers::default());
        assert_eq!(s.active(), Some((1, 0)));

        // Shift adds and PROMOTES to active — the inspector follows the last thing you touched.
        s.apply_click(
            Some((2, 0)),
            ClickModifiers {
                extend: true,
                toggle: false,
            },
        );
        assert_eq!(s.len(), 2);
        assert_eq!(s.active(), Some((2, 0)));
        s.apply_click(
            Some((1, 0)),
            ClickModifiers {
                extend: true,
                toggle: false,
            },
        );
        assert_eq!(s.len(), 2, "re-adding does not duplicate");
        assert_eq!(s.active(), Some((1, 0)), "…it promotes");

        // Ctrl toggles.
        s.apply_click(
            Some((1, 0)),
            ClickModifiers {
                extend: false,
                toggle: true,
            },
        );
        assert_eq!(s.len(), 1);
        assert!(!s.contains(1, 0));

        // A plain click on empty space clears; a modified one does not.
        s.apply_click(
            None,
            ClickModifiers {
                extend: true,
                toggle: false,
            },
        );
        assert_eq!(s.len(), 1, "shift-click on empty space keeps the selection");
        assert!(
            s.apply_click(None, ClickModifiers::default()),
            "…a plain one clears it"
        );
        assert!(s.is_empty());

        // A no-op click reports no change, so callers can skip an event/undo entry.
        assert!(!s.apply_click(None, ClickModifiers::default()));
    }

    #[test]
    fn selection_drops_entries_whose_objects_are_gone() {
        // The dangling-selection bug: after an undo or a delete, an index- or id-based selection
        // outlives its object and the inspector shows a phantom.
        let mut s = SelectionModel::new();
        s.add(1, 0);
        s.add(2, 0);
        s.add(3, 0);
        assert!(s.retain_live(|k, _| k != 2), "reports that it changed");
        assert_eq!(s.entries(), &[(1, 0), (3, 0)]);
        assert_eq!(
            s.active(),
            Some((3, 0)),
            "the active object survives if it is still live"
        );
        assert!(
            !s.retain_live(|_, _| true),
            "an idempotent pass reports no change"
        );
        s.retain_live(|_, _| false);
        assert!(s.is_empty() && s.active().is_none());
    }
}
