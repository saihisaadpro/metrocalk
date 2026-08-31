//! **The viewport's picking bridge** — the seam between the renderer's state and
//! `metrocalk-spatial`'s picking pipeline.
//!
//! What this replaces, and why it matters:
//!
//! `render::pick_nearest` ranked instances by the squared NDC distance from the cursor to each
//! instance's projected **pivot**. That is not a hit test. It has no ray, no geometry, no bounds, no
//! occlusion and no aspect correction, and — because a nearest-of-a-non-empty-set always exists — it
//! returned a hit for *any* click on *any* non-empty scene. The consequences were all visible in the
//! product and all read as unrelated bugs:
//!
//! * Clicking empty space selected whatever object happened to project nearest, so click-to-deselect
//!   was unreachable code.
//! * A large object could only be clicked near its centre; a small neighbour stole clicks on the big
//!   one's face.
//! * A mesh whose modelling origin sits away from its geometry — routine in imported CAD — was
//!   unclickable *where it was drawn* and clickable in empty space where its pivot happened to be.
//! * A fully hidden object outranked the wall in front of it.
//! * Lights, cameras, terrain and CAD group nodes are not in the instance list at all, so clicking
//!   one silently selected an unrelated entity.
//!
//! This module builds a [`PickScene`] from the same data the renderer draws — so what you click is
//! what you see, by construction rather than by coincidence — and answers with the canonical
//! [`HitResult`].

use metrocalk_assets::MeshGpu;
use metrocalk_spatial::bvh::TriangleSource;
use metrocalk_spatial::{
    Aabb, Camera, ClickModifiers, HitKind, HitResult, Orbit, PickFilter, PickGeometry, PickMesh,
    PickObject, PickScene, Picker, Projection, Ray, ScreenRect, SelectionModel, Transform, Vec3,
    Viewport,
};

use crate::render::{self, Instance, SceneState};

/// Pixel radius of a light/camera/helper icon's grab region.
///
/// 14 px is the icon's own drawn size plus a little: these objects have no surface to hit, and
/// requiring an exact intersection with a glyph makes them unselectable in practice.
const MARKER_PICK_RADIUS_PX: f64 = 14.0;

/// A zero-copy [`TriangleSource`] over the renderer's interleaved vertex buffer.
///
/// The renderer already holds every vertex; reading positions in place means the picker adds a BVH
/// and nothing else. A parallel position array would add 25% to the memory of every imported
/// assembly to store numbers that are already resident.
struct GpuMesh<'a>(&'a MeshGpu);

impl TriangleSource for GpuMesh<'_> {
    fn triangle_count(&self) -> usize {
        self.0.indices.len() / 3
    }
    fn triangle(&self, index: usize) -> Option<[Vec3; 3]> {
        let read = |slot: usize| -> Option<Vec3> {
            let i = *self.0.indices.get(slot)? as usize;
            let p = self.0.vertices.get(i)?.position;
            Some([f64::from(p[0]), f64::from(p[1]), f64::from(p[2])])
        };
        Some([read(index * 3)?, read(index * 3 + 1)?, read(index * 3 + 2)?])
    }
}

/// The picking view of the scene, kept beside the render state and refreshed from it.
///
/// Two revisions rather than one because the two inputs change at wildly different rates: the mesh
/// set is loaded once and the BVHs over it are expensive, while instance transforms change on every
/// drag frame and only need a refit.
#[derive(Default)]
pub struct PickCache {
    scene: PickScene,
    picker: Picker,
    /// `SceneState::meshes_revision` the mesh BVHs were built from.
    meshes_revision: u64,
    /// `SceneState::revision` the object list was built from.
    scene_revision: u64,
    /// The instance transforms the object list was built from, so a revision bump that only *moved*
    /// things can be applied as a refit instead of a rebuild.
    ///
    /// Every scene change bumps one revision counter, so without this a gizmo drag — which changes
    /// nothing but a transform — pays for a full BVH rebuild on release, and a physics tick pays for
    /// one every frame.
    instance_transforms: Vec<Instance>,
    /// Milliseconds the last full rebuild took — surfaced by the diagnostics command.
    pub last_build_ms: f64,
    /// Objects in the last built scene.
    pub last_object_count: usize,
    /// Triangles across all built mesh BVHs.
    pub last_triangle_count: usize,
}

impl PickCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Bring the pick scene up to date with the render state. Cheap and idempotent when nothing
    /// changed; rebuilds mesh BVHs only when the mesh set itself changed.
    pub fn sync(&mut self, state: &SceneState) {
        let started = std::time::Instant::now();
        let mut rebuilt = false;

        if self.meshes_revision != state.meshes_revision || self.scene.meshes.is_empty() {
            self.scene.meshes.clear();
            self.last_triangle_count = 0;
            for mesh in &state.meshes {
                // Positions are extracted once per mesh-set revision (rare) so the pick scene owns a
                // compact f32 copy; the BVH build reads the renderer's buffer in place.
                let positions: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| v.position).collect();
                let bvh = metrocalk_spatial::MeshBvh::build(&GpuMesh(mesh));
                self.last_triangle_count += bvh.triangle_count();
                self.scene.meshes.push(PickMesh {
                    positions,
                    indices: mesh.indices.clone(),
                    bvh,
                });
            }
            self.meshes_revision = state.meshes_revision;
            rebuilt = true;
        }

        if self.scene_revision != state.revision || rebuilt {
            // A revision bump that only moved existing instances is a REFIT, not a rebuild: same
            // topology, new bounds, `O(nodes)` and no allocation. This is the difference between a
            // gizmo drag that costs microseconds and one that rebuilds the scene's BVH on release.
            let transform_only = !rebuilt
                && self.instance_transforms.len() == state.instances.len()
                && !state.instances.is_empty()
                && self.scene.objects.len() == state.instances.len() + state.marker_entities.len();
            if transform_only {
                for (index, instance) in state.instances.iter().enumerate() {
                    if instance_moved(&self.instance_transforms[index], instance) {
                        self.update_transform(index, instance_matrix(instance));
                    }
                }
            } else {
                self.scene
                    .set_objects(build_objects(state, &self.scene.meshes));
                self.scene.force_rebuild();
                self.last_object_count = self.scene.objects.len();
                rebuilt = true;
            }
            self.instance_transforms = state.instances.clone();
            self.scene_revision = state.revision;
            self.last_build_ms = started.elapsed().as_secs_f64() * 1e3;
            // Refitting degrades tree quality as objects move away from where they were indexed.
            // Rebuild when the measurement says it has stopped paying, rather than on a timer.
            if !rebuilt && self.scene.broad_phase_quality() > 2.5 {
                self.scene.force_rebuild();
                self.last_build_ms = started.elapsed().as_secs_f64() * 1e3;
            }
        } else if rebuilt {
            self.last_build_ms = started.elapsed().as_secs_f64() * 1e3;
        }
    }

    /// Move one object's transform without a rebuild — the drag/simulation path.
    pub fn update_transform(&mut self, index: usize, world: metrocalk_spatial::Mat4) {
        self.scene.update_object_transform(index, world);
    }

    /// The ordered hits under `ray`.
    pub fn hits(
        &mut self,
        camera: &Camera,
        viewport: &Viewport,
        ray: &Ray,
        filter: &PickFilter,
    ) -> &[HitResult] {
        self.picker
            .pick_all(&mut self.scene, camera, viewport, ray, filter)
    }

    /// The single hit a click resolves to.
    pub fn nearest(
        &mut self,
        camera: &Camera,
        viewport: &Viewport,
        ray: &Ray,
        filter: &PickFilter,
    ) -> Option<HitResult> {
        self.picker
            .pick_nearest(&mut self.scene, camera, viewport, ray, filter)
    }

    /// The hit after `previous` in the ordered list — repeated clicks reach obscured geometry.
    pub fn cycle(
        &mut self,
        camera: &Camera,
        viewport: &Viewport,
        ray: &Ray,
        filter: &PickFilter,
        previous: Option<(u64, u32)>,
    ) -> Option<HitResult> {
        self.picker
            .pick_cycle(&mut self.scene, camera, viewport, ray, filter, previous)
    }

    /// Marquee selection.
    pub fn region(
        &mut self,
        camera: &Camera,
        viewport: &Viewport,
        rect: ScreenRect,
        filter: &PickFilter,
    ) -> Vec<(u64, u32)> {
        self.picker
            .pick_region(&mut self.scene, camera, viewport, rect, filter)
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.scene.objects.len()
    }

    #[must_use]
    pub fn broad_phase_quality(&self) -> f64 {
        self.scene.broad_phase_quality()
    }
}

/// The pick object for one renderable instance.
///
/// The key is the instance's index into `SceneState::ids`, so a hit maps back to an entity through
/// exactly the array the renderer already maintains — no second identity scheme to drift.
fn build_objects(state: &SceneState, meshes: &[PickMesh]) -> Vec<PickObject> {
    let mut out = Vec::with_capacity(state.instances.len() + state.marker_entities.len());
    for (index, instance) in state.instances.iter().enumerate() {
        let world = instance_matrix(instance);
        let slot = state.mesh_slots.get(index).copied().unwrap_or(-1);
        let (geometry, local_bounds) = match usize::try_from(slot)
            .ok()
            .and_then(|s| meshes.get(s).map(|m| (s, m)))
        {
            // A real imported mesh: test its triangles, and bound it by the mesh's OWN local bounds.
            // Using a unit cube here is what made geometry offset from its modelling origin
            // unclickable where it is drawn.
            Some((s, mesh)) => (
                PickGeometry::Mesh(u32::try_from(s).unwrap_or(0)),
                mesh.local_bounds(),
            ),
            // The placeholder cube the renderer draws for an entity with no mesh.
            None => (PickGeometry::Bounds, Aabb::new([-1.0; 3], [1.0; 3])),
        };
        let mut object =
            PickObject::new(index as u64, world, local_bounds, geometry, HitKind::Mesh);
        object.instance = 0;
        out.push(object);
    }

    // Lights, cameras and other markers are DRAWN (as glyphs) but were never in `instances`, so they
    // could not be clicked at all and a click on one selected whatever mesh happened to project
    // nearest. They get a pixel-sized point proxy, which is what makes an icon selectable without
    // demanding pixel-exact aim.
    for (offset, marker) in state.marker_entities.iter().enumerate() {
        let key = (state.instances.len() + offset) as u64;
        let world = Transform::from_translation([
            f64::from(marker.position[0]),
            f64::from(marker.position[1]),
            f64::from(marker.position[2]),
        ])
        .to_matrix();
        let mut object = PickObject::new(
            key,
            world,
            Aabb::point([0.0; 3]),
            PickGeometry::Point {
                screen_radius_px: MARKER_PICK_RADIUS_PX,
            },
            marker.kind,
        );
        object.instance = 0;
        out.push(object);
    }
    out
}

/// The world matrix the renderer actually draws an instance with.
///
/// Mirrors `scene.wgsl`'s `center + quat_rotate(rotation, local * scale)` exactly. That equality is
/// the whole point: a picker that reconstructs the transform its own way will drift from the
/// rasterizer the first time either changes, and the symptom is a click that lands next to the object
/// rather than on it.
fn instance_matrix(instance: &Instance) -> metrocalk_spatial::Mat4 {
    let scale = if instance.scale.is_finite() && instance.scale.abs() > 1.0e-9 {
        f64::from(instance.scale)
    } else {
        1.0e-9
    };
    Transform {
        translation: [
            f64::from(instance.center[0]),
            f64::from(instance.center[1]),
            f64::from(instance.center[2]),
        ],
        rotation: [
            f64::from(instance.rotation[0]),
            f64::from(instance.rotation[1]),
            f64::from(instance.rotation[2]),
            f64::from(instance.rotation[3]),
        ],
        scale: [scale; 3],
    }
    .sanitized()
    .unwrap_or(Transform::IDENTITY)
    .to_matrix()
}

/// The camera the pixels were drawn with, as a `metrocalk-spatial` camera.
///
/// Built from the same orbit parameters the render loop uses, so the picking ray and the rasterized
/// image describe one frustum. The previous code built its projection separately in three places,
/// which is how a click during a resize could unproject against a lens that was never rasterized.
#[must_use]
pub fn camera_for(state: &SceneState) -> Camera {
    let (distance, elevation) = if state.distance == 0.0 {
        (60.0, 0.4) // mirror the render loop's lazy init without mutating shared state
    } else {
        (state.distance, state.elevation)
    };
    let orbit = Orbit {
        yaw: f64::from(state.orbit),
        pitch: f64::from(elevation),
        distance: f64::from(distance),
        target: [
            f64::from(state.cam_target[0]),
            f64::from(state.cam_target[1]),
            f64::from(state.cam_target[2]),
        ],
    };
    let mut camera = orbit.to_camera(
        f64::from(render::CAMERA_FOV_DEG).to_radians(),
        matches!(state.projection, render::Projection::Orthographic),
    );
    // An authored scene camera the viewport is looking through replaces the orbit rig entirely; a
    // pick that ignored it tested against a camera that was not on screen.
    if let Some(over) = state.cam_override {
        camera.eye = [
            f64::from(over.pos[0]),
            f64::from(over.pos[1]),
            f64::from(over.pos[2]),
        ];
        if let Some(look) = over.look_at {
            camera.target = [f64::from(look[0]), f64::from(look[1]), f64::from(look[2])];
        }
        // Authored fields come from the document and can be anything; a zero near plane collapses the
        // scene onto the far plane and a zero field of view produces a NaN matrix, so they are
        // clamped to a usable range here rather than trusted.
        let fov = f64::from(over.fov_deg).to_radians().clamp(0.01, 3.0);
        camera.projection = Projection::Perspective { fov_y: fov };
        camera.near = f64::from(over.near).max(1.0e-5);
        camera.far = f64::from(over.far).max(camera.near * 1.001);
    }
    camera
}

/// The viewport the pixels were drawn into.
///
/// `surface_aspect` is the aspect the frame was actually rasterized with — reading the window size
/// again here is what let a pick during a resize disagree with the image on screen.
#[must_use]
pub fn viewport_for(state: &SceneState, dpi_scale: f64) -> Viewport {
    // The COMPOSED frame's aspect, not the window's. The projection is sheared to the rectangle the
    // viewer can see (`render::framed_projection`), so NDC spans that rectangle: a ray built against
    // the window's shape describes a frustum that was never rasterised. Fractions handed to this
    // viewport must therefore be frame fractions - see [`frame_fraction`].
    let aspect = f64::from(render::frame_aspect(
        state.surface_aspect,
        state.composition_rect(),
    ));
    // Height is nominal; only the ratio and the pixel count matter, and the pixel count is what turns
    // a pixel tolerance into world units.
    let height = 1080.0;
    Viewport {
        surface_width: height * aspect,
        surface_height: height,
        origin_x: 0.0,
        origin_y: 0.0,
        dpi_scale,
    }
}

/// Remap a SURFACE fraction - what the shell reports, the pointer's position over the whole window -
/// into a fraction of the rectangle the picture is composed in.
///
/// The identity while the composed frame is the whole surface, which is every case that existed before
/// a viewport rectangle was reported. With a dock open it is the difference between the object under
/// the cursor and the object a click selects.
#[must_use]
pub fn frame_fraction(state: &SceneState, x: f64, y: f64) -> (f64, f64) {
    let [fx, fy, fw, fh] = state.composition_rect();
    (
        (x - f64::from(fx)) / f64::from(fw).max(1.0e-6),
        (y - f64::from(fy)) / f64::from(fh).max(1.0e-6),
    )
}

/// Resolve a hit back to the entity id the rest of the editor speaks.
#[must_use]
pub fn entity_of(state: &SceneState, hit: &HitResult) -> Option<String> {
    let key = hit.key as usize;
    if key < state.ids.len() {
        return state.ids.get(key).cloned();
    }
    state
        .marker_entities
        .get(key - state.instances.len())
        .map(|m| m.id.clone())
}

/// Whether a pick key refers to a renderable instance (as opposed to a marker), and its index.
#[must_use]
pub fn instance_index_of(state: &SceneState, key: u64) -> Option<usize> {
    usize::try_from(key)
        .ok()
        .filter(|index| *index < state.instances.len())
}

/// Whether two instances describe a different placement — the refit trigger.
fn instance_moved(a: &Instance, b: &Instance) -> bool {
    a.center != b.center || a.scale != b.scale || a.rotation != b.rotation
}

/// **Project the canonical selection onto the renderer's per-instance highlight flag.**
///
/// The flag is a *view* of the selection, never a second copy of it. Both selection entry points call
/// this, so the viewport outline, the gizmo target and the reported ids cannot disagree — the failure
/// where a structural change left an outline lit on an object that was no longer selected came from
/// each of those keeping its own idea of what was selected.
///
/// Returns the primary (active) instance index, which is what the gizmo attaches to.
pub fn apply_selection_highlight(
    state: &mut SceneState,
    selection: &SelectionModel,
) -> Option<usize> {
    for instance in &mut state.instances {
        instance.highlight =
            render::highlight_with(instance.highlight, render::HIGHLIGHT_SELECTED, false);
    }
    let mut primary = None;
    for (key, _) in selection.entries() {
        if let Some(index) = instance_index_of(state, *key) {
            state.instances[index].highlight = render::highlight_with(
                state.instances[index].highlight,
                render::HIGHLIGHT_SELECTED,
                true,
            );
            primary = Some(index);
        }
    }
    state.selected = primary;
    state.revision = state.revision.wrapping_add(1);
    primary
}

/// The entity ids of everything currently selected, in selection order.
#[must_use]
pub fn selected_ids(state: &SceneState, selection: &SelectionModel) -> Vec<String> {
    selection
        .entries()
        .iter()
        .filter_map(|(key, _)| {
            instance_index_of(state, *key).map_or_else(
                || {
                    state
                        .marker_entities
                        .get((*key as usize).saturating_sub(state.instances.len()))
                        .map(|m| m.id.clone())
                },
                |index| state.ids.get(index).cloned(),
            )
        })
        .collect()
}

/// The default filter for a viewport click: everything visible and selectable, opaque preferred.
#[must_use]
pub fn click_filter() -> PickFilter {
    PickFilter {
        transparency: metrocalk_spatial::TransparencyPolicy::PreferOpaque,
        ..PickFilter::default()
    }
}

/// Turn the modifier flags the front end sends into the selection model's vocabulary.
#[must_use]
pub fn modifiers(shift: bool, ctrl: bool) -> ClickModifiers {
    ClickModifiers {
        extend: shift,
        toggle: ctrl,
    }
}

/// Apply a click to the canonical selection and report whether it changed anything.
pub fn apply_click(
    selection: &mut SelectionModel,
    hit: Option<&HitResult>,
    modifiers: ClickModifiers,
) -> bool {
    selection.apply_click(hit.map(|h| (h.key, h.instance)), modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrocalk_assets::MeshVertex;

    /// The x and w components of a 90° rotation about Y: `sin(45°) = cos(45°) = 1/√2`.
    const HALF_TURN_Y: f32 = std::f32::consts::FRAC_1_SQRT_2;

    fn vertex(p: [f32; 3]) -> MeshVertex {
        MeshVertex {
            position: p,
            normal: [0.0, 1.0, 0.0],
            color: [1.0; 3],
            metallic: 0.0,
            roughness: 1.0,
            uv: [0.0; 2],
            tangent: [0.0; 4],
        }
    }

    /// A unit cube centred on the origin.
    fn cube() -> MeshGpu {
        let v = [
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        MeshGpu {
            vertices: v.into_iter().map(vertex).collect(),
            indices: vec![
                0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 3, 7, 6, 3, 6, 2, 0, 4, 7, 0,
                7, 3, 1, 2, 6, 1, 6, 5,
            ],
            submeshes: Vec::new(),
        }
    }

    #[test]
    fn the_triangle_source_reads_the_renderers_buffer_without_copying_it() {
        let mesh = cube();
        let src = GpuMesh(&mesh);
        assert_eq!(src.triangle_count(), 12);
        let t = src.triangle(0).expect("first triangle");
        assert_eq!(t[0], [-1.0, -1.0, -1.0]);
        // An out-of-range index reports None rather than panicking — a malformed import must not take
        // the editor down.
        assert!(src.triangle(999).is_none());
        let broken = MeshGpu {
            vertices: vec![vertex([0.0; 3])],
            indices: vec![0, 1, 2],
            submeshes: Vec::new(),
        };
        assert!(GpuMesh(&broken).triangle(0).is_none());
    }

    #[test]
    fn the_instance_matrix_matches_what_the_shader_draws() {
        // `scene.wgsl` computes `center + quat_rotate(rotation, local * scale)`. If this drifts from
        // that, the click lands beside the object instead of on it.
        let instance = Instance {
            center: [3.0, -2.0, 5.0],
            scale: 2.0,
            color: [1.0; 3],
            highlight: 0.0,
            rotation: [0.0, HALF_TURN_Y, 0.0, HALF_TURN_Y], // 90° about Y
            material: [0.0; 4],
        };
        let m = instance_matrix(&instance);
        let t = Transform::decompose(m).transform;
        assert_eq!(t.translation, [3.0, -2.0, 5.0]);
        for s in t.scale {
            assert!(
                (s - 2.0).abs() < 1.0e-6,
                "uniform scale carried: {:?}",
                t.scale
            );
        }
        // A local +X corner maps to world −Z (times the scale), plus the centre.
        let corner = t.transform_point([1.0, 0.0, 0.0]);
        assert!((corner[0] - 3.0).abs() < 1.0e-6, "{corner:?}");
        assert!((corner[2] - 3.0).abs() < 1.0e-6, "{corner:?}");
    }

    /// A scene state with `count` unit cubes laid out by `place`, all sharing one mesh.
    fn scene_with(place: impl Fn(usize) -> ([f32; 3], f32)) -> SceneState {
        let mut state = SceneState {
            meshes: vec![cube()],
            meshes_revision: 1,
            revision: 1,
            surface_aspect: 16.0 / 9.0,
            distance: 20.0,
            elevation: 0.0,
            orbit: 0.0,
            cam_target: [0.0; 3],
            ..SceneState::default()
        };
        for i in 0..3 {
            let (center, scale) = place(i);
            state.instances.push(Instance {
                center,
                scale,
                color: [1.0; 3],
                highlight: 0.0,
                rotation: [0.0, 0.0, 0.0, 1.0],
                material: [0.0; 4],
            });
            state.mesh_slots.push(0);
            state.ids.push(format!("entity:{i}"));
        }
        state
    }

    /// The ray a click at the given surface fraction produces, plus the camera/viewport it used.
    fn click(state: &SceneState, x: f64, y: f64) -> (Camera, Viewport, Ray) {
        let camera = camera_for(state);
        let viewport = viewport_for(state, 1.0);
        let ray = camera.ray_through_fraction(&viewport, x, y);
        (camera, viewport, ray)
    }

    #[test]
    fn a_click_is_measured_against_the_frame_the_picture_is_composed_in() {
        // The seam that keeps the ray and the image describing one frustum. The projection is sheared
        // to the composed rectangle (`render::framed_projection`), so a fraction of the WINDOW is not a
        // fraction of the picture. The rectangle here is the bottom-right quarter — a deliberately
        // large offset, because the point of the second half is to SEPARATE two rays, and a hole near
        // the window's own centre separates them by less than a cube is wide.
        let mut state = scene_with(|i| ([0.0, 0.0, f32::from(i as u8) * 4.0 - 4.0], 1.0));
        state.surface_aspect = 1296.0 / 839.0;
        state.visible_rect = [0.55, 0.55, 0.40, 0.40];
        state.frame_all();

        let mut cache = PickCache::new();
        cache.sync(&state);
        let centre = state.composition_rect();
        let (cx, cy) = (
            f64::from(centre[2].mul_add(0.5, centre[0])),
            f64::from(centre[3].mul_add(0.5, centre[1])),
        );

        // Through the seam: the centre of the visible stage hits the scene that was framed into it.
        let camera = camera_for(&state);
        let viewport = viewport_for(&state, 1.0);
        let (fx, fy) = frame_fraction(&state, cx, cy);
        assert!((fx - 0.5).abs() < 1.0e-6 && (fy - 0.5).abs() < 1.0e-6);
        let ray = camera.ray_through_fraction(&viewport, fx, fy);
        assert!(
            cache
                .nearest(&camera, &viewport, &ray, &click_filter())
                .is_some(),
            "the centre of the composed frame must hit the framed scene"
        );

        // And WITHOUT it — the window fraction handed straight to the viewport, which is what this
        // file did before the projection carried the frame — the same click misses entirely. That is
        // the defect itself: the cursor and the thing under it drift apart by the distance between the
        // window's centre and the picture's.
        let stale = camera.ray_through_fraction(&viewport, cx, cy);
        assert!(
            cache
                .nearest(&camera, &viewport, &stale, &click_filter())
                .is_none(),
            "a ray built from the window's fraction is aimed somewhere the picture is not"
        );

        // The identity when the picture owns the whole surface, which is every case that existed
        // before a viewport rectangle was reported.
        let mut whole = scene_with(|i| ([0.0, 0.0, f32::from(i as u8) * 4.0 - 4.0], 1.0));
        whole.surface_aspect = 1296.0 / 839.0;
        assert_eq!(frame_fraction(&whole, 0.37, 0.62), (0.37, 0.62));
    }

    #[test]
    fn clicking_empty_space_now_returns_nothing() {
        // The headline behaviour change. `pick_nearest` returned its nearest projected pivot for ANY
        // click on any non-empty scene, so the "nothing here" branch in the front end was unreachable
        // and click-to-deselect could not work.
        // The orbit rig at yaw 0 / pitch 0 puts the eye on +X looking at the origin, so the cubes are
        // spread across the view along Z — laying them along X would stack them in DEPTH and the
        // centre click would (correctly) hit the nearest, which is a different test.
        let state = scene_with(|i| ([0.0, 0.0, f32::from(i as u8) * 4.0 - 4.0], 1.0));
        let mut cache = PickCache::new();
        cache.sync(&state);
        let (camera, viewport, ray) = click(&state, 0.02, 0.02); // top-left corner
        assert!(
            cache
                .nearest(&camera, &viewport, &ray, &click_filter())
                .is_none(),
            "a click in an empty corner must hit nothing"
        );
        // …and a click at the screen centre hits the cube that is actually at the screen centre.
        let (camera, viewport, ray) = click(&state, 0.5, 0.5);
        let hit = cache
            .nearest(&camera, &viewport, &ray, &click_filter())
            .expect("the centre cube");
        assert_eq!(entity_of(&state, &hit).as_deref(), Some("entity:1"));
        assert_eq!(
            hit.source,
            metrocalk_spatial::HitSource::Triangle,
            "real geometry, not a proxy"
        );
    }

    #[test]
    fn a_click_on_a_large_objects_face_selects_it_not_the_nearer_pivot() {
        // The pivot metric's worst everyday failure: a big object can only be clicked near its
        // centre, and a small neighbour whose pivot happens to be nearer the cursor steals the click.
        let mut state = scene_with(|_| ([0.0; 3], 1.0));
        state.instances.clear();
        state.mesh_slots.clear();
        state.ids.clear();
        // A large cube at the origin.
        state.instances.push(Instance {
            center: [0.0; 3],
            scale: 5.0,
            color: [1.0; 3],
            highlight: 0.0,
            rotation: [0.0, 0.0, 0.0, 1.0],
            material: [0.0; 4],
        });
        state.mesh_slots.push(0);
        state.ids.push("big".into());
        // A small one just beyond its upper-right corner, whose PIVOT projects nearer the cursor.
        state.instances.push(Instance {
            center: [4.2, 4.2, 0.0],
            scale: 0.15,
            color: [1.0; 3],
            highlight: 0.0,
            rotation: [0.0, 0.0, 0.0, 1.0],
            material: [0.0; 4],
        });
        state.mesh_slots.push(0);
        state.ids.push("small".into());

        let mut cache = PickCache::new();
        cache.sync(&state);
        // Aim at a point ON the big cube's front face, near that corner.
        let camera = camera_for(&state);
        let viewport = viewport_for(&state, 1.0);
        let ray = Ray::new([4.0, 4.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = cache
            .nearest(&camera, &viewport, &ray, &click_filter())
            .expect("the face under the cursor");
        assert_eq!(
            entity_of(&state, &hit).as_deref(),
            Some("big"),
            "the surface under the cursor wins, not the nearest pivot"
        );
        assert!(
            (hit.world_position[2] - 5.0).abs() < 1.0e-6,
            "and the hit is on the +Z face: {:?}",
            hit.world_position
        );
    }

    #[test]
    fn occlusion_is_respected_and_obscured_geometry_stays_reachable() {
        let state = scene_with(|i| ([0.0, 0.0, 4.0 - f32::from(i as u8) * 4.0], 1.0));
        let mut cache = PickCache::new();
        cache.sync(&state);
        let camera = camera_for(&state);
        let viewport = viewport_for(&state, 1.0);
        let ray = Ray::new([0.0, 0.0, 20.0], [0.0, 0.0, -1.0]);

        let nearest = cache
            .nearest(&camera, &viewport, &ray, &click_filter())
            .expect("hit");
        assert_eq!(
            entity_of(&state, &nearest).as_deref(),
            Some("entity:0"),
            "the front one"
        );

        // Clicking again cycles to what is behind — obscured geometry is not unreachable.
        let second = cache
            .cycle(
                &camera,
                &viewport,
                &ray,
                &click_filter(),
                Some((nearest.key, nearest.instance)),
            )
            .expect("cycles");
        assert_eq!(entity_of(&state, &second).as_deref(), Some("entity:1"));
        assert!(second.distance > nearest.distance);
    }

    #[test]
    fn a_light_marker_is_selectable_and_answers_with_its_own_entity() {
        // Lights and cameras are drawn as glyphs and were absent from every pick candidate list, so
        // clicking one selected whatever mesh projected nearest.
        let mut state = scene_with(|_| ([0.0, 0.0, -30.0], 1.0));
        state.marker_entities.push(render::MarkerEntity {
            id: "light:key".into(),
            position: [2.0, 1.0, 0.0],
            kind: HitKind::Light,
        });
        let mut cache = PickCache::new();
        cache.sync(&state);
        let camera = camera_for(&state);
        let viewport = viewport_for(&state, 1.0);
        // Aim a few pixels off the light's exact centre — an icon must not demand pixel-exact aim.
        let centre = camera
            .world_to_ndc([2.0, 1.0, 0.0], viewport.aspect())
            .expect("in front");
        let (px, py) = viewport.pixel_in_ndc();
        let ray = camera.ray_through_ndc(
            centre[0] + px * 5.0,
            centre[1] + py * 5.0,
            viewport.aspect(),
        );
        let hit = cache
            .nearest(&camera, &viewport, &ray, &click_filter())
            .expect("the light icon");
        assert_eq!(entity_of(&state, &hit).as_deref(), Some("light:key"));
        assert_eq!(hit.kind, HitKind::Light);
        assert_eq!(hit.source, metrocalk_spatial::HitSource::Proxy);
    }

    #[test]
    fn moving_an_instance_refits_the_broad_phase_instead_of_rebuilding_it() {
        // The drag path. A revision bump that only changed a transform must not pay for a rebuild —
        // and the picker must still find the object where it moved to, not where it was indexed.
        let mut state = scene_with(|i| ([f32::from(i as u8) * 6.0, 0.0, 0.0], 1.0));
        let mut cache = PickCache::new();
        cache.sync(&state);
        let built = cache.last_object_count;

        state.instances[1].center = [0.0, 40.0, 0.0];
        state.revision += 1;
        cache.sync(&state);
        assert_eq!(
            cache.last_object_count, built,
            "no rebuild: the object list is untouched"
        );

        let camera = camera_for(&state);
        let viewport = viewport_for(&state, 1.0);
        let moved = Ray::new([0.0, 40.0, 20.0], [0.0, 0.0, -1.0]);
        let hit = cache
            .nearest(&camera, &viewport, &moved, &click_filter())
            .expect("found at its NEW position");
        assert_eq!(entity_of(&state, &hit).as_deref(), Some("entity:1"));

        // …and it is no longer at its old one.
        let old = Ray::new([6.0, 0.0, 20.0], [0.0, 0.0, -1.0]);
        assert!(cache
            .nearest(&camera, &viewport, &old, &click_filter())
            .is_none());
    }

    #[test]
    fn the_selection_highlight_is_a_projection_of_one_selection_model() {
        let mut state = scene_with(|i| ([f32::from(i as u8) * 6.0, 0.0, 0.0], 1.0));
        let mut selection = SelectionModel::new();
        selection.add(0, 0);
        selection.add(2, 0);
        let primary = apply_selection_highlight(&mut state, &selection);
        assert_eq!(primary, Some(2), "the most recently added is active");
        assert_eq!(state.instances[0].highlight, 1.0);
        assert_eq!(state.instances[1].highlight, 0.0);
        assert_eq!(state.instances[2].highlight, 1.0);
        assert_eq!(
            selected_ids(&state, &selection),
            vec!["entity:0", "entity:2"]
        );

        // Clearing the model clears every highlight — there is no second copy to go stale.
        selection.clear();
        assert_eq!(apply_selection_highlight(&mut state, &selection), None);
        assert!(state.instances.iter().all(|i| i.highlight == 0.0));
        assert_eq!(state.selected, None);
    }

    #[test]
    fn a_zero_scale_instance_cannot_poison_the_pick_scene() {
        let instance = Instance {
            center: [0.0; 3],
            scale: 0.0,
            color: [1.0; 3],
            highlight: 0.0,
            rotation: [f32::NAN; 4],
            material: [0.0; 4],
        };
        let m = instance_matrix(&instance);
        assert!(m.iter().all(|c| c.iter().all(|v| v.is_finite())));
        assert!(
            metrocalk_spatial::transform::mat_inverse(m).is_some(),
            "the matrix stays invertible, so the object is simply un-hittable rather than NaN"
        );
    }
}
