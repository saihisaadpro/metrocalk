//! The native wgpu viewport — M2.2's instanced render path on the Tauri window surface (ADR-008
//! single-window: this surface is OS-composited *under* the transparent WebView2). Renders the live
//! `/core` scene: one instanced cube per entity (from its `Transform`) + a ground grid, depth-tested,
//! with an orbiting camera. Instancing is the M2.2 technique that holds the frame budget; the GPU
//! frustum-cull→indirect refinement is also proven in `spikes/render-scene` and ports in on top.
//!
//! The render loop owns no scene truth — it reads a shared [`SceneState`] the app updates from the
//! authoritative core (deltas). Hot interaction stays in Rust (invariant 4): camera orbit/zoom update
//! natively in the loop (zero per-frame IPC), and picking is a pure projection ([`pick_nearest`]) run
//! synchronously inside the `viewport_pick` command — neither crosses the JS boundary per frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use glam::{Mat4, Quat, Vec3, Vec4};
use metrocalk_assets::{MeshGpu, MeshVertex, Texture};
use metrocalk_editor_shell::reveal::intent_order;
use metrocalk_gizmo::{Gizmo, TransformGizmo};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// M9.4 — the magnetic-snap radius (world units): during a gizmo drag the dragged instance snaps onto the
/// nearest meaningful target within this range (the live "magnetic intent snapping").
pub const SNAP_RADIUS: f32 = 1.5;

/// Total UI→core IPC calls (every `#[tauri::command]` bumps this). The render loop reports it next to
/// the frame count so a sustained drag can be shown to cross the JS boundary **zero times per frame**
/// (invariant 4) — orbit/zoom update natively in the loop; only the start/end of a gesture are IPC.
pub static IPC_CALLS: AtomicU64 = AtomicU64::new(0);

/// One renderable entity instance. 64 bytes, std430-clean (matches the WGSL `Instance`). `rotation` is a
/// unit quaternion (xyzw; identity `[0,0,0,1]`) applied per-instance by the shader — so a tumbling physics
/// body / a rotated authored Transform / a posed part actually *looks* rotated (the shared renderer-
/// rotation path). The line/overlay/gizmo passes reuse `Instance` purely as a point carrier and ignore it.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub center: [f32; 3],
    pub scale: f32,
    pub color: [f32; 3],
    pub selected: f32,
    pub rotation: [f32; 4],
    /// M11.2 per-entity PBR material override `[metallic, roughness, has_override, _pad]`. When
    /// `has_override > 0.5` the mesh shader uses these (with `color` as the override base color) instead of
    /// the asset's baked vertex material — this is how a "make it metal/rusty/gold" intent recolors ONE
    /// entity without touching the shared geometry. `[0; 4]` (the default) = use the baked material.
    pub material: [f32; 4],
}

/// One light for the fragment shader's multi-light loop (M11.3, ADR-042). 48 bytes, std430-clean (matches
/// the WGSL `Light`). `kind` packs in `pos.w` (0=directional, 1=point, 2=spot); for point/spot `pos.xyz` is
/// the world position, for directional `dir.xyz` is the direction. `color` is linear RGB, `range` the
/// point/spot falloff radius. Built each rebuild from the scene's authored `Light` entities (a render
/// projection — never Loro; the light ENTITY is the undoable doc state, this is its per-frame upload).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightGpu {
    /// `xyz` = world position (point/spot); `w` = kind (0 dir, 1 point, 2 spot).
    pub pos_kind: [f32; 4],
    /// `xyz` = linear RGB colour; `w` = intensity.
    pub color_intensity: [f32; 4],
    /// `xyz` = direction (directional/spot); `w` = range (point/spot falloff, 0 = infinite).
    pub dir_range: [f32; 4],
}

/// The identity quaternion (no rotation) — the default for `Instance::rotation`.
pub const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Authored-space bounds shared by camera framing, focus, LOD, and thumbnail portraits. Imported CAD is
/// intentionally not normalized (millimetre vertices commonly render with a 0.001 instance scale), so any
/// path that guesses size from `Instance::scale` alone will either clip it or park the camera far away.
#[derive(Clone, Copy, Debug)]
struct LocalBounds {
    center: Vec3,
    half_size: Vec3,
}

impl LocalBounds {
    const UNIT_CUBE: Self = Self {
        center: Vec3::ZERO,
        half_size: Vec3::ONE,
    };

    fn max_extent(self) -> f32 {
        (self.half_size * 2.0).max_element()
    }
}

impl SceneState {
    /// Half-height of the mesh in `slot`, in the mesh's own local units.
    ///
    /// Exists so a projection can SEAT an actor on the ground instead of guessing its height. Every
    /// imported asset is normalised by `AssetAffine::unit_from_bounds` to a max extent of 1.0 and
    /// recentred on its bounding-box centre, so an asset's half-height is 0.5 only along its LONGEST
    /// axis and less along the others. A caller that assumes 0.5 sinks tall assets into the floor and
    /// floats wide ones above it — and a floating caster throws a shadow that is visibly detached from
    /// it, which is exactly how this was found.
    ///
    /// Falls back to 0.5, the half-extent of the unit cube the placeholder path draws.
    #[must_use]
    pub fn mesh_half_height(&self, slot: i32) -> f32 {
        usize::try_from(slot)
            .ok()
            .and_then(|slot| self.meshes.get(slot))
            .and_then(local_mesh_bounds)
            .map_or(0.5, |bounds| bounds.half_size[1])
    }
}

/// The presentation ground's albedo.
///
/// Darkened from 0.62-ish grey: the ground measured BRIGHTER than the lit characters standing on it, so
/// silhouettes had nothing to read against and the whole frame collapsed into one mid-grey band. A
/// viewport floor's job is to be the value everything else is legible against, not to compete.
pub const GROUND_ALBEDO: [f32; 3] = [0.30, 0.31, 0.34];

fn local_mesh_bounds(mesh: &MeshGpu) -> Option<LocalBounds> {
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;
    for vertex in &mesh.vertices {
        let position = Vec3::from(vertex.position);
        if position.is_finite() {
            lo = lo.min(position);
            hi = hi.max(position);
            found = true;
        }
    }
    found.then(|| LocalBounds {
        center: (lo + hi) * 0.5,
        half_size: ((hi - lo) * 0.5).max(Vec3::splat(0.000_5)),
    })
}

fn instance_rotation(instance: &Instance) -> Quat {
    let rotation = Quat::from_array(instance.rotation);
    if rotation.is_finite() && rotation.length_squared() > 1.0e-8 {
        rotation.normalize()
    } else {
        Quat::IDENTITY
    }
}

/// Exact world AABB for an authored local AABB under this renderer's uniform scale + quaternion transform.
fn instance_world_bounds(instance: &Instance, local: LocalBounds) -> (Vec3, Vec3) {
    let rotation = instance_rotation(instance);
    let translation = Vec3::from(instance.center);
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                let corner = local.center + local.half_size * Vec3::new(x, y, z);
                let world = translation + rotation * (corner * instance.scale);
                lo = lo.min(world);
                hi = hi.max(world);
            }
        }
    }
    (lo, hi)
}

/// Rendered world bounds using each instance's authored mesh bounds (or the placeholder unit cube).
/// Keeping this in one place prevents camera framing and shadow fitting from quietly disagreeing on CAD
/// assets whose vertices are authored in millimetres and displayed through a small instance scale.
/// The viewport's vertical field of view. Named once so the projection and the framing that fits
/// against it can never disagree — they were two unrelated literals before.
pub const CAMERA_FOV_DEG: f32 = 55.0;

/// How the viewport projects the scene.
///
/// The canonical views existed before this did, and every one of them was a 55-degree perspective — a
/// "top" view rendered with vanishing points, which is precisely why it read as useless. An axis view
/// is only an axis view if parallel lines stay parallel: that is what lets you compare two distances on
/// screen, see that two parts share an edge, and read a plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Projection {
    #[default]
    Perspective,
    /// Parallel projection. Its height is derived from the SAME orbit distance the perspective camera
    /// uses, so zoom, framing and the orientation cube keep working across a mode switch instead of
    /// needing a second scale to be kept in sync.
    Orthographic,
}

/// Viewport exposure. Placed so the lit scene lands near the tone curve's mid-grey rather than a stop
/// and a half above it. Named once because a second, independent default in the thumbnail path had
/// already drifted from the viewport's.
pub const DEFAULT_EXPOSURE: f32 = 0.45;

/// The aspect assumed when framing happens before a real surface size is known. 16:9 rather than 1:1 so
/// the pre-surface guess errs toward the shape of a real editor viewport.
pub const DEFAULT_ASPECT: f32 = 16.0 / 9.0;

/// The fraction of the tighter viewport half-extent the subject should span. 0.72 sits inside the
/// 55-75% band a professional viewport targets, leaving a margin that reads as deliberate framing
/// rather than as a crop.
pub const FRAME_OCCUPANCY: f32 = 0.72;

/// Distance at which the scene's bounding box fills `FRAME_OCCUPANCY` of the frame, fitted per axis
/// against the lens actually in use.
///
/// A **projected-AABB** fit, not a bounding-sphere one, and that distinction is the whole point. The
/// sphere around a 12 m x 1 m lane is 12 m tall, so a sphere fit reserves 12 m of VERTICAL frame for
/// content that is one metre tall and pushes the camera back by an order of magnitude. That is exactly
/// what the baseline screenshot shows: a wide, flat scene stranded in empty grey. Projecting the box
/// onto the camera's own right/up axes and fitting each against its own half-angle spends the frame on
/// the extent that is really there.
///
/// The previous implementation was `max_half_edge * 2.4`, which read neither the field of view nor the
/// aspect ratio nor the view direction - three independent ways for the framing to be wrong.
#[must_use]
pub fn fit_distance(half_extent: Vec3, aspect: f32, orbit: f32, elevation: f32) -> f32 {
    let aspect = if aspect.is_finite() && aspect > 0.01 {
        aspect
    } else {
        DEFAULT_ASPECT
    };
    let half_v = (CAMERA_FOV_DEG.to_radians() * 0.5).clamp(0.01, 1.5);
    let half_h = (half_v.tan() * aspect).atan().max(0.01);

    // The camera basis this distance will be used with, so the fit matches the view it frames.
    let forward = -Vec3::new(
        orbit.cos() * elevation.cos(),
        elevation.sin(),
        orbit.sin() * elevation.cos(),
    )
    .normalize_or_zero();
    let world_up = if forward.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let right = forward.cross(world_up).normalize_or_zero();
    let up = right.cross(forward).normalize_or_zero();

    // An axis-aligned box's extent along an arbitrary unit axis is the dot of its half-extents with
    // that axis's absolute components - no corner enumeration needed.
    let h = half_extent.abs();
    let ext_r = h.dot(right.abs()).max(1e-4);
    let ext_u = h.dot(up.abs()).max(1e-4);
    let ext_f = h.dot(forward.abs());

    // Each axis needs its own distance; the frame must satisfy the more demanding one. `ext_f` is
    // added because the box has depth: fitting its centre would push its near face through the lens.
    let need_v = ext_u / half_v.tan();
    let need_h = ext_r / half_h.tan();
    (need_v.max(need_h) / FRAME_OCCUPANCY + ext_f).clamp(0.3, 4000.0)
}

fn scene_world_bounds(
    instances: &[Instance],
    mesh_slots: &[i32],
    meshes: &[MeshGpu],
) -> Option<(Vec3, Vec3)> {
    if instances.is_empty() {
        return None;
    }
    let mesh_bounds: Vec<Option<LocalBounds>> = meshes.iter().map(local_mesh_bounds).collect();
    let mut lo = Vec3::splat(f32::INFINITY);
    let mut hi = Vec3::splat(f32::NEG_INFINITY);
    for (index, instance) in instances.iter().enumerate() {
        let bounds = mesh_slots
            .get(index)
            .and_then(|slot| usize::try_from(*slot).ok())
            .and_then(|slot| mesh_bounds.get(slot).copied().flatten())
            .unwrap_or(LocalBounds::UNIT_CUBE);
        let (instance_lo, instance_hi) = instance_world_bounds(instance, bounds);
        lo = lo.min(instance_lo);
        hi = hi.max(instance_hi);
    }
    Some((lo, hi))
}

/// M11.4 (ADR-043) — the active scene camera's look-through view parameters. A render PROJECTION (never
/// Loro/undo): when `SceneState.cam_override` is `Some`, the frame renders from this scene camera instead
/// of the editor fly-cam. Set by `look_through_camera` from the authored `Camera` entity.
#[derive(Clone, Copy)]
pub struct CamView {
    pub pos: [f32; 3],
    pub fov_deg: f32,
    pub near: f32,
    pub far: f32,
}

/// Scene state shared between the app (writer, from core deltas + input) and the render loop (reader).
#[derive(Default)]
pub struct SceneState {
    /// Perspective, or a true parallel projection for the axis views.
    pub projection: Projection,
    /// The live surface aspect (width/height), published by the render loop each frame.
    ///
    /// Without this, `frame_all` framed against a compile-time constant and produced the SAME camera
    /// distance at 900x900 as at 1600x850 — the resize captures in the viewport stress audit were
    /// byte-identical in camera state, which is how this was found. A framing routine that cannot see
    /// the shape of the frame is guessing.
    pub surface_aspect: f32,
    pub instances: Vec<Instance>,
    /// Entity id (Loro key) parallel to `instances` — maps a picked index back to an entity.
    pub ids: Vec<String>,
    /// Tracking-line endpoints: each consecutive pair is one `LineList` segment drawn between two
    /// bound entities (binding-by-intent). Reuses [`Instance`] purely as a point carrier — only
    /// `center` is read (by `vs_line`); the other fields are ignored. Rebuilt with `instances`, so a
    /// `revision` bump re-uploads both. Empty when nothing is bound (the line pass is skipped).
    pub line_points: Vec<Instance>,
    /// M11.4 (ADR-043) — wireframe ICON glyphs for light/camera marker entities (a warm burst for a light,
    /// a cyan frustum for a camera). Same `LineList` carrier as `line_points` but drawn by the per-segment
    /// **overlay** pass (so each glyph is colour-coded), always-pass depth so the icon reads as an overlay.
    /// Built in `rebuild` (markers are NOT rendered as solid cubes); empty ⇒ the marker pass is skipped.
    pub marker_glyphs: Vec<Instance>,
    /// M8.4 contact-debugger overlay endpoints — same `LineList` carrier as [`Self::line_points`], drawn by
    /// the same always-pass line pass (each consecutive pair is one segment). **Empty by default** (the
    /// debugger is off → the overlay pass is skipped → zero per-frame cost). Updated independently from
    /// `instances`, on its own [`Self::overlay_revision`].
    pub overlay_lines: Vec<Instance>,
    /// Pipe Forge's transient editable route. This is render-only tool state: clicks update it, Cancel/Bake
    /// clear it, and it never dirties Loro or undo history. The render loop turns it into bright line/cross
    /// geometry in the existing always-visible gizmo pass.
    pub pipe_points: Vec<[f32; 3]>,
    /// Every stable graph edge in world space. Unlike `pipe_points`, this includes secondary branches.
    pub pipe_edges: Vec<[[f32; 3]; 2]>,
    /// Every stable graph handle in world space, including branch-only nodes.
    pub pipe_handles: Vec<[f32; 3]>,
    /// Bump when `overlay_lines` changes so the loop re-uploads them (decoupled from `revision`).
    pub overlay_revision: u64,
    /// M11.3 (ADR-042) — the scene's lights, built each rebuild from the authored `Light` entities (a
    /// render projection: the light ENTITIES are the undoable Loro doc state, this is their per-frame GPU
    /// upload). Never empty when uploaded — `rebuild` falls back to a single default key light so an
    /// unlit scene isn't black (the prior hard-coded directional, now a real entry in the list).
    pub lights: Vec<LightGpu>,
    /// Bump when `lights` changes so the loop re-uploads them (decoupled from `revision`).
    pub lights_revision: u64,
    /// Per-instance mesh-asset slot, parallel to `instances`: `-1` ⇒ render the M2.2 placeholder cube
    /// (the honest fallback for an entity with no mesh handle); `>= 0` ⇒ an index into [`Self::meshes`]
    /// (render that imported mesh instead). The render loop partitions `instances` by this into the
    /// cube pass + per-asset instanced mesh draws. The entity stays in `instances`/`ids` regardless, so
    /// picking (centre-based) is uniform across cubes and meshes.
    pub mesh_slots: Vec<i32>,
    /// The distinct imported meshes, slot-indexed (referenced by [`Self::mesh_slots`]). Packed,
    /// `wasm32`-portable geometry from the asset store; uploaded once per `meshes_revision`.
    pub meshes: Vec<MeshGpu>,
    /// Bump when `meshes` changes so the loop re-uploads the per-asset vertex/index buffers (rare —
    /// the asset set is loaded once at startup).
    pub meshes_revision: u64,
    /// Slot indices the MOB match projection draws with, resolved once from the real imported catalog.
    /// Published here because the asset runtime lives on the engine thread while the match projection is
    /// render-side; -1 means the catalog had no such asset and the match draws nothing rather than a stand-in.
    pub moba_hero_slot: i32,
    pub moba_minion_slot: i32,
    pub moba_structure_slot: i32,
    /// Currently-selected instance index (drives the highlight).
    pub selected: Option<usize>,
    /// Bump when `instances` changes so the loop re-uploads the buffer.
    pub revision: u64,
    /// Orbit/zoom driven by drag input (stays in Rust — invariant 4).
    pub orbit: f32,
    pub elevation: f32,
    pub distance: f32,
    /// Right-drag orbit: while true, the render loop polls the cursor and orbits — zero per-frame IPC
    /// (only the gesture's start/end are commands). Set by `drag_start`/`drag_end`.
    pub dragging: bool,
    /// Last polled cursor (physical screen px) during a drag, for the per-frame delta.
    pub drag_last: Option<(f64, f64)>,
    /// Pending wheel-zoom to fold into `distance` (one command per wheel tick, not per frame).
    pub zoom_delta: f32,
    /// The camera look-at target (orbit centre). Default origin; `focus_entity` sets it to an entity's
    /// position so the camera frames it. Orbit/zoom stay relative to this target.
    pub cam_target: [f32; 3],
    /// M11.4 — post-processing exposure (a linear multiplier applied before the ACES tonemap in
    /// `display_encode`). Render-only state (a projection, never Loro/undo — like the camera pose), set by
    /// `set_exposure` (0-IPC). 0 is treated as "uninitialised" → defaults to 1.0.
    pub exposure: f32,
    /// Viewport presentation profile: `0` = cinematic/game, `1` = CAD inspection. This is render-only
    /// state, like exposure and camera pose. The HDR/PBR core stays shared; tone mapping, selection
    /// treatment and technical edge emphasis are the intentional presentation differences.
    pub render_profile: f32,
    /// M11.4 — look-through: when `Some`, the frame renders from this scene camera (the editor fly-cam is
    /// bypassed). Render-only (a projection, never Loro — ADR-021); set by `look_through_camera`.
    pub cam_override: Option<CamView>,
    /// M3.3 Focus mode: the focused instance index (`Some` ⇒ focus active). Drives the shader dim
    /// (`focus_active` uniform) so every *other* entity grays out, and is the camera-frame target.
    /// Cleared by `unfocus` ("everything comes back to normal"). The focused entity is also the
    /// selected one, so the shader keeps it lit via the existing per-instance `selected` flag.
    pub focused: Option<usize>,
    /// The orbit `distance` saved when focus mode was *entered* (`get nearby` zooms in; unfocus
    /// restores this). `None` ⇒ not focused / nothing to restore. Saved once on enter so focusing a
    /// second entity without un-focusing first doesn't lose the original framing.
    pub pre_focus_distance: Option<f32>,
    /// M9.1 transform gizmo — its mode (W/E/R) + in-flight drag live here so the render loop can run the
    /// per-frame drag natively (0 per-frame IPC, like the orbit). The drawn geometry is regenerated each
    /// frame at the selected entity (constant pixel size); the gizmo shows whenever `selected` is `Some`.
    pub gizmo: TransformGizmo,
    /// A gizmo drag is active (the render loop polls the cursor + moves the dragged instance).
    pub gizmo_dragging: bool,
    /// The instance index being dragged (frozen at drag start).
    pub gizmo_sel: Option<usize>,
    /// Ctrl-hold snapping for the active drag.
    pub gizmo_snap: bool,
    /// The dragged instance's display scale at drag start (so a scale-drag multiplies from it).
    pub gizmo_start_scale: f32,
    /// A test-injected normalized cursor: when `Some`, the render loop drives the drag from it instead of
    /// the OS cursor (so an E2E can drive the SAME render-loop drag path deterministically). `None` ⇒ the
    /// live OS cursor drives (the production path).
    pub gizmo_test_cursor: Option<(f32, f32)>,
    /// M9.4 — per-instance snap **affinity** (parallel to `instances`): a pivot (a parent) is a stronger
    /// spatial intent than a bare origin, so it wins the affinity tiebreak in the shared ADR-011 ranker
    /// ([`nearest_snap`]). Built on rebuild from the engine's hierarchy.
    pub snap_affinity: Vec<u32>,
    /// M9.4 — magnetic snapping disabled (default `false` ⇒ snapping ON). The render-loop drag pulls the
    /// dragged instance onto the nearest snap target when enabled; toggled by the `set_snap` command.
    pub snap_disabled: bool,
    /// M9.4 — the current snap **ghost** (the nearest target's world position during a drag), drawn as an
    /// overlay marker + read by `snap_ghost` (the HUD/E2E). `None` ⇒ no candidate in range / not dragging.
    pub snap_ghost: Option<[f32; 3]>,
    /// M11.3 inc.3 — index (into `lights`) of the scene's shadow-casting directional light (the first
    /// authored directional with `castShadows`, else the default key light). `None` ⇒ nothing casts → the
    /// shadow pass is skipped, `light_view_proj` stays identity, and `fs_mesh` shadows nothing. The INDEX
    /// (not just the direction) so the shader applies the single shadow map to ONLY its caster, not every
    /// directional light. Rebuilt with `lights` (a render projection).
    pub shadow_caster: Option<usize>,
    /// M14.2 (ADR-058) — pending live-thumbnail render requests `(entity id, size px)`, pushed by the
    /// `thumbnail` command and drained by the render thread, which renders each entity to a small offscreen
    /// target on **its own encoder before the swapchain frame** (off the per-frame orbit path — invariant 4;
    /// a discrete, dirty-only, budget-limited surface, NEVER per-frame). A presentation artifact: thumbnails
    /// never enter the op-stream/Loro doc (zero determinism impact, like the M11.3 lights projection).
    pub thumb_requests: Vec<(String, u32)>,
    /// Serviced thumbnail results `(entity id → PNG bytes, or None when the entity has no renderable
    /// instance)`. The `thumbnail` command polls this for its id, then removes the entry. Capped so a
    /// timed-out request can't grow it unbounded.
    pub thumb_results: Vec<(String, Option<Vec<u8>>)>,
}

pub type Shared = Arc<Mutex<SceneState>>;

impl SceneState {
    /// M10.7 — **frame the whole scene**: center the orbit target on the scene's bounds and set a distance
    /// that fits them in view. A pure camera op (invariant 4 — render-state only, not undoable). No-op on an
    /// empty scene. Exits focus dim (framing-all looks at everything).
    pub fn frame_all(&mut self) {
        // The aspect the user is actually looking through, not a constant. Falls back only before the
        // first frame has published one.
        let aspect = if self.surface_aspect > 0.01 {
            self.surface_aspect
        } else {
            DEFAULT_ASPECT
        };
        self.frame_all_with_aspect(aspect);
    }

    /// Frame the whole scene through a lens of the given aspect ratio.
    ///
    /// The previous implementation was `distance = max_half_edge * 2.4`, and every part of that was
    /// wrong in a way that only shows up as "the asset looks small":
    ///
    /// * **It never read the field of view.** 2.4 is only a fit for one particular FOV; the projection
    ///   uses a hard 55 degrees, and nothing tied the two together. Changing the lens silently changed
    ///   how much of the frame the subject filled.
    /// * **It never read the aspect ratio.** It fitted the scene's longest axis against the VERTICAL
    ///   extent regardless of orientation, so a wide, flat scene — a lane, an assembly, a character
    ///   turntable — was pushed back by the full aspect ratio and left empty bands above and below.
    /// * **It used the longest half-EDGE, not the bounding-sphere radius**, which under-covers a
    ///   diagonal by up to sqrt(3) and makes the fit orientation-dependent.
    ///
    /// This computes the true tangency distance for a bounding sphere against whichever of the two
    /// half-angles is the tighter, then divides by a target occupancy so the subject fills a chosen
    /// fraction of the shorter viewport dimension with deliberate padding rather than an accident.
    pub fn frame_all_with_aspect(&mut self, aspect: f32) {
        if self.instances.is_empty() {
            return;
        }
        let Some((lo, hi)) = scene_world_bounds(&self.instances, &self.mesh_slots, &self.meshes)
        else {
            return;
        };
        self.cam_target = ((lo + hi) * 0.5).to_array();
        self.distance = fit_distance((hi - lo) * 0.5, aspect, self.orbit, self.elevation);
        self.clear_focus();
        self.revision = self.revision.wrapping_add(1);
    }

    /// M10.7 — snap the camera to a **canonical view** (top/front/side/persp), keeping the orbit target +
    /// distance. A pure camera op (invariant 4). `orbit` = azimuth, `elevation` = pitch (see `camera_eye`).
    pub fn set_view_preset(&mut self, preset: &str) {
        use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};
        let (orbit, elevation) = match preset {
            "top" => (-FRAC_PI_2, 1.4), // near-straight-down (clamped below the look_at degeneracy)
            "front" => (FRAC_PI_2, 0.0), // eye on +Z, looking horizontally
            "side" => (0.0, 0.0),       // eye on +X
            _ => (FRAC_PI_4, 0.5),      // perspective 3/4 (the default-ish view)
        };
        // An AXIS view is orthographic. Under perspective, a "top" view still has vanishing points, so
        // two objects the same size read as different sizes and nothing can be compared by eye — which
        // is the whole reason to be in a top view. The 3/4 view stays perspective, because depth cues
        // are what make it readable as a space rather than a diagram.
        self.projection = match preset {
            "top" | "front" | "side" => Projection::Orthographic,
            _ => Projection::Perspective,
        };
        self.orbit = orbit;
        self.elevation = elevation;
        self.revision = self.revision.wrapping_add(1);
    }

    /// M10.7 — the camera state `[orbit, elevation, distance, target_x, target_y, target_z]` for the
    /// orientation cube + the E2E (the viewport is a native wgpu surface WebDriver can't read pixels from).
    #[must_use]
    pub fn camera_state(&self) -> [f32; 6] {
        [
            self.orbit,
            self.elevation,
            self.distance,
            self.cam_target[0],
            self.cam_target[1],
            self.cam_target[2],
        ]
    }

    /// Enter Focus mode on instance `i` (M3.3) — the pure state transition the `focus_entity` command
    /// applies: select it, center the camera on it, zoom in to frame it by size ("get nearby"), and
    /// raise the focus flag so the shader dims the rest. Saves the pre-focus `distance` once, so the
    /// first [`Self::clear_focus`] restores the original framing even after focusing several entities
    /// in a row. Bumps `revision` so the new `selected` flag re-uploads. No-op if `i` is out of range.
    pub fn focus_on(&mut self, i: usize) {
        if i >= self.instances.len() {
            return;
        }
        // The focused entity is also the selected one (the shader keeps the selected instance lit while
        // focus dims the rest) — clear any prior highlight first.
        if let Some(p) = self.selected {
            if p < self.instances.len() {
                self.instances[p].selected = 0.0;
            }
        }
        self.selected = Some(i);
        self.instances[i].selected = 1.0;
        // Center and size come from the real authored geometry. This is essential for offset meshes and for
        // CAD whose vertices are in millimetres while its instance scale is 0.001.
        let local_bounds = self
            .mesh_slots
            .get(i)
            .and_then(|slot| usize::try_from(*slot).ok())
            .and_then(|slot| self.meshes.get(slot))
            .and_then(local_mesh_bounds)
            .unwrap_or(LocalBounds::UNIT_CUBE);
        let (world_lo, world_hi) = instance_world_bounds(&self.instances[i], local_bounds);
        self.cam_target = ((world_lo + world_hi) * 0.5).to_array();
        // Get nearby: save the framing once, then zoom to ~4× the entity's half-extent, clamped to the
        // orbit range so a huge or tiny entity still lands at a sensible, in-bounds distance.
        if self.pre_focus_distance.is_none() {
            self.pre_focus_distance = Some(if self.distance == 0.0 {
                60.0
            } else {
                self.distance
            });
        }
        // CM-SCALE floors (not the old 0.5 m / 6 m): focusing a centimetre-scale CAD part must get the
        // camera NEAR it (the old 6 m floor parked a 2 cm part sub-pixel — the same M15.9 defect family
        // as frame-all's metre floors).
        let half_extent = ((world_hi - world_lo) * 0.5).max_element().max(0.02);
        self.distance = (half_extent * 4.0).clamp(0.15, 400.0);
        self.focused = Some(i);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Exit Focus mode ("everything comes back to normal"): clear the focus flag (the shader un-dims
    /// every entity) and restore the orbit `distance` saved when focus was entered. Idempotent — a
    /// no-op (no `revision` bump) when nothing is focused, so a stray Escape never disturbs the scene.
    /// Selection is intentionally left as-is (only the dim + zoom revert).
    pub fn clear_focus(&mut self) {
        if self.focused.is_none() {
            return;
        }
        self.focused = None;
        if let Some(d) = self.pre_focus_distance.take() {
            self.distance = d;
        }
        self.revision = self.revision.wrapping_add(1);
    }
}

/// One uploaded mesh asset's GPU geometry (per-asset vertex + index buffers — the non-bindless path:
/// one bound vertex/index buffer per asset, drawn instanced across the entities that use it). M11.2
/// follow-up: partitioned into per-primitive [`GpuSubMesh`]es, each with its own uploaded textures, so a
/// multi-material mesh draws every part's texture (one sub-draw + bind group per submesh).
struct GpuMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    /// Whole-mesh index count — the depth-only shadow pass draws the full mesh in one call (no textures).
    n_idx: u32,
    local_bounds: LocalBounds,
    submeshes: Vec<GpuSubMesh>,
}

/// One submesh's draw range + its uploaded texture views (dummies where its material ships none). The
/// per-submesh main-pass bind group (instances + these three textures) is rebuilt in the revision block.
struct GpuSubMesh {
    index_offset: u32,
    index_count: u32,
    base_view: wgpu::TextureView,
    mr_view: wgpu::TextureView,
    normal_view: wgpu::TextureView,
    ao_view: wgpu::TextureView,
}

/// A growable storage buffer of [`Instance`]s + its bind group — the per-asset instance list for one
/// mesh slot (the transforms of every entity rendering as that mesh). Grows by powers of two.
struct InstanceBuf {
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
    cap: u64,
    n: u32,
}

impl InstanceBuf {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, cap: u64) -> Self {
        let buf = new_instance_storage(device, cap);
        let bg = make_inst_bg(device, layout, &buf);
        Self { buf, bg, cap, n: 0 }
    }

    /// Upload `data`, growing (and rebinding) the buffer if needed. Sets `n` to the count drawn.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        data: &[Instance],
    ) {
        let needed = data.len() as u64;
        if needed > self.cap {
            self.cap = needed.next_power_of_two();
            self.buf = new_instance_storage(device, self.cap);
            self.bg = make_inst_bg(device, layout, &self.buf);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(data));
        }
        self.n = data.len() as u32;
    }
}

fn new_instance_storage(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: cap * std::mem::size_of::<Instance>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

// M11.2 follow-up — base-color texture sampling on the mesh pipeline. Non-bindless (ADR-003): one texture
// per mesh, bound on the already-per-mesh instance group (group 1) so the bind-group count stays at the
// WebGPU 4-group cap. An untextured mesh binds a 1×1 white dummy → `fs_mesh` always samples (white × the
// baked factor = the factor), so it looks exactly as before.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextureSemantic {
    Color,
    Data,
    Normal,
}

fn srgb_to_linear(byte: u8) -> f32 {
    let value = f32::from(byte) / 255.0;
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0).round().clamp(0.0, 255.0) as u8
}

// ── COLOUR-SPACE CONTRACT (M15.11 — the linear-HDR intermediate) ──────────────────────────────────────
//
// The renderer has exactly THREE colour spaces, and every value belongs to precisely one of them:
//
//   1. AUTHORED sRGB      — what a human typed. UI-picked colours, hard-coded helper/gizmo/grid/marker
//                           constants, the viewport clear colour. Non-linear, display-referred, in [0,1].
//   2. SCENE LINEAR (HDR) — what lives in the `Rgba16Float` scene target. Radiance, unbounded above 1.
//                           Lighting, IBL, MSAA resolve, SSAO and bloom ALL operate here.
//   3. DISPLAY            — what the swapchain receives. Produced exactly ONCE, by `fs_resolve` in
//                           post.wgsl: exposure → tone curve → transfer function.
//
// Lit surfaces produce (2) directly from the BRDF — their albedo factors and light colours are already
// linear (glTF convention), so they are NEVER passed through a conversion here.
//
// UNLIT content (cubes, grid, lines, gizmos, markers, overlays, selection tint, clear colour) is authored
// in (1) and must reach (2). A plain `srgb_to_linear` is NOT sufficient: the value would then be exposed
// and tone-mapped like radiance, so a `#9fe` tracking line would land on screen 25–40% darker than the
// colour the author picked, and would drift again whenever the user changed exposure or switched the
// presentation profile. `unlit_srgb_to_scene_linear` therefore inverts the *whole* display transform —
// tone curve and exposure included — so an authored colour renders back as itself. That is still exactly
// one conversion, at one boundary, in one direction.
//
// It takes the exposure to invert AGAINST, and there are two answers:
//
//   * CHROME  — grid, lines, markers, the gizmo, the selection tint: the LIVE exposure, so they hold their
//               authored colour wherever the user puts the slider. A gizmo handle that dims when you stop
//               the scene down is a handle you cannot grab.
//   * CONTENT — the placeholder cubes and the clear colour: the REFERENCE exposure (`DEFAULT_EXPOSURE`),
//               so they brighten and blow out with the frame. An exposure sweep of the captured frames is
//               what forced this: unlit cubes held at their default brightness while the lit scene around
//               them blew out, and they read as pasted onto the image. Anchoring at the default keeps
//               them EXACTLY the authored colour at the default exposure.
//
// The inverse is capped, per profile, by `UNLIT_DISPLAY_CAP_*`. The cap is applied to the TARGET and
// scales the colour uniformly, so hue is exact and only the overall level gives.
//
// The WGSL mirror of everything below lives in scene.wgsl (`unlit_srgb_at_exposure` and its two callers)
// and post.wgsl (the forward direction). The tests in this file are the executable statement of the
// contract: `unlit_authored_colour_round_trips_through_the_display_transform`,
// `capping_a_saturated_unlit_colour_preserves_its_hue`, and
// `chrome_holds_its_colour_across_the_exposure_range_and_content_does_not`.

/// The scene's linear-HDR intermediate format. Every pipeline that draws into the scene colour attachment
/// declares THIS, never the swapchain format; see [`RenderFormats`].
pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// The depth format, for the scene pass, the shadow pass and every pipeline's depth-stencil state. Named
/// once because it must agree everywhere — including with the MSAA sample count, which is where a stray
/// literal previously hid the fact that the depth texture had its own opinion about sample counts.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Ceiling on the EXPOSED value an unlit authored colour may inverse-map to. In exposed units so the
/// clamp behaves identically at every exposure setting. See the colour-space contract above.
const UNLIT_EXPOSED_CEILING: f32 = 2.0;

/// The brightest display-linear value an unlit authored colour is allowed to target, per presentation
/// profile. The cap is applied to the TARGET before inverting, and scales the colour UNIFORMLY so the hue
/// is exact and only the overall level gives.
///
/// The two profiles are capped for different reasons, and both are properties of their tone curve:
///
/// * **Cinematic (ACES)** — the fit is per channel and monotone, so the only limit is that it approaches
///   1.0 asymptotically: display white has no finite scene value. The cap is simply the curve evaluated
///   at [`UNLIT_EXPOSED_CEILING`], so authored white renders at ~0.96 sRGB.
/// * **CAD (PBR Neutral)** — above `start_compression` the curve DESATURATES: it mixes toward the peak by
///   `g < 1`, which LIFTS the darkest channel. Past a certain peak a saturated bright colour is no longer
///   in the operator's range at all, and no inverse can recover it — at a cap of 0.96 the inverse returned
///   a negative channel and the amber light marker's blue came back at 0.39 instead of 0.20. The cap is
///   therefore set where that shadow lift is still negligible (below one 8-bit code), which is just inside
///   the compression region; the cost is that authored white renders at ~0.91 sRGB in this profile.
///
/// `the_unlit_display_caps_stay_in_each_curves_faithful_range` pins both.
const UNLIT_DISPLAY_CAP_ACES: f32 = 0.914_855;
const UNLIT_DISPLAY_CAP_PBR_NEUTRAL: f32 = 0.78;

/// The cap for a presentation profile. One accessor so the conversion and its tests cannot disagree.
#[must_use]
fn unlit_display_cap(cad: bool) -> f32 {
    if cad {
        UNLIT_DISPLAY_CAP_PBR_NEUTRAL
    } else {
        UNLIT_DISPLAY_CAP_ACES
    }
}

/// The two formats the renderer works in, carried together so a scene pipeline cannot be built against the
/// swapchain format by accident: `hdr` is the scene intermediate, `display` is the surface. Passing one
/// bare `format` for both was the original defect — MSAA then resolved gamma-encoded samples, and every
/// scene shader had to encode for display itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RenderFormats {
    /// The linear-HDR scene intermediate (always [`HDR_FORMAT`]).
    pub hdr: wgpu::TextureFormat,
    /// The swapchain / surface format — written ONLY by the final resolve pass.
    pub display: wgpu::TextureFormat,
}

impl RenderFormats {
    #[must_use]
    pub fn new(display: wgpu::TextureFormat) -> Self {
        Self {
            hdr: HDR_FORMAT,
            display,
        }
    }

    /// `1.0` when `fs_resolve` must apply the sRGB OETF itself (a linear-store swapchain), `0.0` when the
    /// surface format performs the conversion in hardware. Applying both would double-encode the frame.
    #[must_use]
    pub fn manual_display_encode(self) -> f32 {
        f32::from(u8::from(!self.display.is_srgb()))
    }
}

/// sRGB electro-optical transfer function (authored → linear). The exact piecewise curve, not a 2.2
/// approximation: the approximation visibly shifts near-black gradients, which is where banding shows.
#[must_use]
pub fn srgb_to_linear_f32(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

// ── the forward display transform: the CPU mirror of post.wgsl's `fs_resolve` ─────────────────────────
// In production the GPU does this, once per frame, in the final resolve — nothing on the CPU needs it.
// It exists so the round-trip that the whole unlit colour contract rests on can be PROVEN rather than
// asserted: `unlit_round_trips` composes `unlit_srgb_to_scene_linear` with this and checks it lands back
// on the authored colour. Keep it in step with post.wgsl.
#[cfg(test)]
/// The inverse OETF (linear → authored/display encoding).
#[must_use]
pub fn linear_to_srgb_f32(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
/// The cinematic tone curve (Narkowicz ACES fit), applied per channel. Mirrors `tonemap_aces` in post.wgsl.
#[must_use]
fn tonemap_aces_channel(x: f32) -> f32 {
    let (a, b, c, d, e) = (2.51f32, 0.03f32, 2.43f32, 0.59f32, 0.14f32);
    ((x * (a * x + b)) / (x * (c * x + d) + e)).clamp(0.0, 1.0)
}

/// Exact inverse of [`tonemap_aces_channel`]. The fit is a ratio of quadratics, so inverting it is solving
/// `(yc-a)x² + (yd-b)x + ye = 0`; for every `y` the curve can actually produce, `yc-a < 0` and the root
/// below is the non-negative one.
#[must_use]
fn inverse_tonemap_aces_channel(y: f32) -> f32 {
    let (a, b, c, d, e) = (2.51f32, 0.03f32, 2.43f32, 0.59f32, 0.14f32);
    let y = y.clamp(0.0, 0.999);
    let denom = a - y * c; // > 0 for every reachable y
    if denom <= 1e-6 {
        return UNLIT_EXPOSED_CEILING;
    }
    let bb = y * d - b;
    let cc = y * e;
    let disc = (bb * bb + 4.0 * denom * cc).max(0.0);
    ((bb + disc.sqrt()) / (2.0 * denom)).max(0.0)
}

/// The CAD tone curve (Khronos PBR Neutral). Mirrors `tonemap_pbr_neutral` in post.wgsl.
const PBR_NEUTRAL_START: f32 = 0.8 - 0.04;
const PBR_NEUTRAL_DESAT: f32 = 0.15;

#[cfg(test)]
#[must_use]
fn tonemap_pbr_neutral(input: [f32; 3]) -> [f32; 3] {
    let x = input[0].min(input[1]).min(input[2]);
    let offset = if x < 0.08 { x - 6.25 * x * x } else { 0.04 };
    let color = [input[0] - offset, input[1] - offset, input[2] - offset];
    let peak = color[0].max(color[1]).max(color[2]);
    if peak < PBR_NEUTRAL_START {
        return color;
    }
    let new_peak = 1.0
        - (1.0 - PBR_NEUTRAL_START) * (1.0 - PBR_NEUTRAL_START)
            / (peak + 1.0 - 2.0 * PBR_NEUTRAL_START);
    let g = 1.0 / (PBR_NEUTRAL_DESAT * (peak - new_peak) + 1.0);
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let compressed = color[i] * (new_peak / peak);
        out[i] = new_peak + g * (compressed - new_peak);
    }
    out
}

/// Exact inverse of [`tonemap_pbr_neutral`]. Every step is invertible: the compression maps `peak` to
/// `new_peak` monotonically, and the black-lift `offset` is recovered from the minimum channel.
#[must_use]
fn inverse_tonemap_pbr_neutral(out: [f32; 3]) -> [f32; 3] {
    let out_peak = out[0].max(out[1]).max(out[2]).clamp(0.0, 0.999);
    let mut color = out;
    if out_peak >= PBR_NEUTRAL_START {
        // `max(out)` IS `new_peak` (the compressed colour and the desaturation target share that peak).
        let new_peak = out_peak;
        let peak =
            2.0 * PBR_NEUTRAL_START - 1.0 + (1.0 - PBR_NEUTRAL_START).powi(2) / (1.0 - new_peak);
        let g = 1.0 / (PBR_NEUTRAL_DESAT * (peak - new_peak) + 1.0);
        for i in 0..3 {
            let compressed = new_peak + (out[i] - new_peak) / g;
            color[i] = compressed * (peak / new_peak);
        }
    }
    // Decompression can undershoot below zero for a target the curve cannot actually produce (see
    // UNLIT_DISPLAY_CAP_PBR_NEUTRAL — the caps keep unlit colour out of that region, but a caller passing
    // an arbitrary value should get the nearest reachable colour, not a negative one).
    for c in &mut color {
        *c = c.max(0.0);
    }
    // Undo the black lift. `offset` is 0.04 unless the original minimum channel was below 0.08, and the
    // pre-offset minimum satisfies `min(color) = x - 6.25x²` there, so `x = 0.4·sqrt(min(color))`.
    let min_c = color[0].min(color[1]).min(color[2]).max(0.0);
    let offset = if min_c < 0.04 {
        0.4 * min_c.sqrt() - min_c
    } else {
        0.04
    };
    [color[0] + offset, color[1] + offset, color[2] + offset]
}

/// Turn an AUTHORED sRGB colour into the SCENE-LINEAR value that the single final resolve will render back
/// as that same colour. `cad` selects the presentation profile's tone curve (`cam.shadow.z > 0.5`).
///
/// This is the one and only conversion applied to unlit authored colour, and it is applied on the way IN
/// to the HDR target — never on the way out. Pass the LIVE exposure for chrome (it then holds its colour
/// at any exposure) or [`DEFAULT_EXPOSURE`] for content (it then tracks the exposure slider). See the
/// colour-space contract above.
#[must_use]
pub fn unlit_srgb_to_scene_linear(rgb: [f32; 3], exposure: f32, cad: bool) -> [f32; 3] {
    let mut display_linear = [
        srgb_to_linear_f32(rgb[0].clamp(0.0, 1.0)),
        srgb_to_linear_f32(rgb[1].clamp(0.0, 1.0)),
        srgb_to_linear_f32(rgb[2].clamp(0.0, 1.0)),
    ];
    // Cap the TARGET uniformly (hue-preserving) at what this profile can faithfully reproduce.
    let cap = unlit_display_cap(cad);
    let peak = display_linear[0]
        .max(display_linear[1])
        .max(display_linear[2]);
    if peak > cap {
        let scale = cap / peak;
        for c in &mut display_linear {
            *c *= scale;
        }
    }
    let exposed = if cad {
        inverse_tonemap_pbr_neutral(display_linear)
    } else {
        [
            inverse_tonemap_aces_channel(display_linear[0]),
            inverse_tonemap_aces_channel(display_linear[1]),
            inverse_tonemap_aces_channel(display_linear[2]),
        ]
    };
    let e = if exposure > 1e-4 { exposure } else { 1.0 };
    let mut out = [0.0f32; 3];
    for i in 0..3 {
        // The cap already bounds the peak; this only guards against negatives and rounding.
        out[i] = exposed[i].clamp(0.0, UNLIT_EXPOSED_CEILING) / e;
    }
    out
}

#[cfg(test)]
/// The forward display transform. Mirrors `fs_resolve` in post.wgsl (minus bloom and dither).
#[must_use]
fn scene_linear_to_display(rgb: [f32; 3], exposure: f32, cad: bool) -> [f32; 3] {
    let exposed = [
        (rgb[0] * exposure).max(0.0),
        (rgb[1] * exposure).max(0.0),
        (rgb[2] * exposure).max(0.0),
    ];
    let mapped = if cad {
        tonemap_pbr_neutral(exposed)
    } else {
        [
            tonemap_aces_channel(exposed[0]),
            tonemap_aces_channel(exposed[1]),
            tonemap_aces_channel(exposed[2]),
        ]
    };
    [
        linear_to_srgb_f32(mapped[0].clamp(0.0, 1.0)),
        linear_to_srgb_f32(mapped[1].clamp(0.0, 1.0)),
        linear_to_srgb_f32(mapped[2].clamp(0.0, 1.0)),
    ]
}

/// The viewport background, AUTHORED in sRGB. Only visible where the sky is not drawn (`MTK_SKY=off`, and
/// the thumbnail RTT); it still goes through the same conversion as every other authored colour so that
/// enabling bloom, SSAO or MSAA can never change it.
pub const CLEAR_COLOR_SRGB: [f32; 3] = [0.04, 0.05, 0.08];

/// The clear colour as the scene-linear value the final resolve renders back as [`CLEAR_COLOR_SRGB`].
///
/// Anchored at [`DEFAULT_EXPOSURE`], not the live one: the background is CONTENT (it is what the scene sits
/// in), so it has to brighten with the frame when the exposure comes up. Chrome — the grid, the gizmo, the
/// markers — is anchored at the live exposure instead, so it stays legible. Both go through the same
/// conversion; only the anchor differs. See `scene_srgb_to_scene_linear` in scene.wgsl.
#[must_use]
fn clear_color(cad: bool) -> wgpu::Color {
    let c = unlit_srgb_to_scene_linear(CLEAR_COLOR_SRGB, DEFAULT_EXPOSURE, cad);
    wgpu::Color {
        r: f64::from(c[0]),
        g: f64::from(c[1]),
        b: f64::from(c[2]),
        a: 1.0,
    }
}

/// Deterministically derive a complete mip chain. Base colour is averaged in linear light, packed
/// material data is averaged channel-wise, and tangent-space normals are decoded/averaged/renormalized.
/// This avoids the distant shimmer and dark halos caused by treating every PBR texture as raw colour.
fn texture_mips(tex: &Texture, semantic: TextureSemantic) -> Vec<(u32, u32, Vec<u8>)> {
    let mut width = tex.width.max(1);
    let mut height = tex.height.max(1);
    let expected = width as usize * height as usize * 4;
    let mut pixels = if tex.rgba8.len() == expected {
        tex.rgba8.clone()
    } else {
        vec![255; expected]
    };
    let mut levels = vec![(width, height, pixels.clone())];
    while width > 1 || height > 1 {
        let next_width = width.div_ceil(2);
        let next_height = height.div_ceil(2);
        let mut next = vec![0u8; next_width as usize * next_height as usize * 4];
        for y in 0..next_height {
            for x in 0..next_width {
                let mut samples = [[0u8; 4]; 4];
                let mut sample_count = 0usize;
                for oy in 0..2 {
                    for ox in 0..2 {
                        let sx = x * 2 + ox;
                        let sy = y * 2 + oy;
                        if sx < width && sy < height {
                            let source = (sy as usize * width as usize + sx as usize) * 4;
                            samples[sample_count].copy_from_slice(&pixels[source..source + 4]);
                            sample_count += 1;
                        }
                    }
                }
                let target = (y as usize * next_width as usize + x as usize) * 4;
                let inv = 1.0 / sample_count as f32;
                match semantic {
                    TextureSemantic::Color => {
                        for channel in 0..3 {
                            let linear = samples[..sample_count]
                                .iter()
                                .map(|sample| srgb_to_linear(sample[channel]))
                                .sum::<f32>()
                                * inv;
                            next[target + channel] = linear_to_srgb(linear);
                        }
                    }
                    TextureSemantic::Data => {
                        for channel in 0..3 {
                            next[target + channel] = (samples[..sample_count]
                                .iter()
                                .map(|sample| f32::from(sample[channel]))
                                .sum::<f32>()
                                * inv)
                                .round() as u8;
                        }
                    }
                    TextureSemantic::Normal => {
                        let mut normal = [0.0_f32; 3];
                        for sample in &samples[..sample_count] {
                            for channel in 0..3 {
                                normal[channel] +=
                                    (f32::from(sample[channel]) / 255.0 * 2.0 - 1.0) * inv;
                            }
                        }
                        let length = normal.iter().map(|value| value * value).sum::<f32>().sqrt();
                        let normal = if length > 1.0e-6 {
                            normal.map(|value| value / length)
                        } else {
                            [0.0, 0.0, 1.0]
                        };
                        for channel in 0..3 {
                            next[target + channel] = ((normal[channel] * 0.5 + 0.5) * 255.0)
                                .round()
                                .clamp(0.0, 255.0)
                                as u8;
                        }
                    }
                }
                next[target + 3] = (samples[..sample_count]
                    .iter()
                    .map(|sample| f32::from(sample[3]))
                    .sum::<f32>()
                    * inv)
                    .round() as u8;
            }
        }
        width = next_width;
        height = next_height;
        pixels = next;
        levels.push((width, height, pixels.clone()));
    }
    levels
}

/// Upload an RGBA8 texture → a sampled view with a full semantic-aware mip chain. Base colour uses an
/// sRGB format (linearized on sample); metallic-roughness and normal maps stay linear.
fn upload_tex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &Texture,
    semantic: TextureSemantic,
) -> wgpu::TextureView {
    let (w, h) = (tex.width.max(1), tex.height.max(1));
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let format = if semantic == TextureSemantic::Color {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let mips = texture_mips(tex, semantic);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh-tex"),
        size,
        mip_level_count: mips.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (mip_width, mip_height, rgba8)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba8,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(mip_width * 4),
                rows_per_image: Some(*mip_height),
            },
            wgpu::Extent3d {
                width: *mip_width,
                height: *mip_height,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A 1×1 white texture view — the dummy bound for a mesh with no base-color/MR texture (white × the baked
/// factor = the factor). Created once at setup, cloned per untextured mesh.
fn white_dummy(device: &wgpu::Device, queue: &wgpu::Queue, srgb: bool) -> wgpu::TextureView {
    upload_tex(
        device,
        queue,
        &Texture {
            width: 1,
            height: 1,
            rgba8: vec![255, 255, 255, 255],
        },
        if srgb {
            TextureSemantic::Color
        } else {
            TextureSemantic::Data
        },
    )
}

/// A 1×1 flat-normal dummy ([128,128,255] linear → +Z) — bound for a mesh with no normal map, so the
/// tangent-space perturbation is a no-op (the geometric normal is used).
fn flat_normal_dummy(device: &wgpu::Device, queue: &wgpu::Queue) -> wgpu::TextureView {
    upload_tex(
        device,
        queue,
        &Texture {
            width: 1,
            height: 1,
            rgba8: vec![128, 128, 255, 255],
        },
        TextureSemantic::Normal,
    )
}

/// The mesh main-pass group 1: instances (vertex) + base-color/metallic-roughness/normal textures + a
/// shared sampler (fragment). One per mesh, rebuilt when the instance buffer is (re)allocated.
#[allow(clippy::too_many_arguments)]
fn make_mesh_main_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    inst_buf: &wgpu::Buffer,
    base: &wgpu::TextureView,
    mr: &wgpu::TextureView,
    normal: &wgpu::TextureView,
    ao: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh-main-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: inst_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(base),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(mr),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(normal),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(ao),
            },
        ],
    })
}

/// M11.3 — the scene's lights as a growable FRAGMENT-visible storage buffer (the shader reads the count via
/// `arrayLength`, so the upload is always ≥1 element — `rebuild` guarantees a default key light). Mirrors
/// [`InstanceBuf`] for the lights bind group (group 2 on the mesh pipeline).
struct LightBuf {
    buf: wgpu::Buffer,
    bg: wgpu::BindGroup,
    cap: u64,
}

impl LightBuf {
    fn new(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, cap: u64) -> Self {
        let buf = new_light_storage(device, cap);
        let bg = make_inst_bg(device, layout, &buf);
        Self { buf, bg, cap }
    }
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        data: &[LightGpu],
    ) {
        let needed = data.len().max(1) as u64;
        if needed > self.cap {
            self.cap = needed.next_power_of_two();
            self.buf = new_light_storage(device, self.cap);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(data));
        }
        // Bind ONLY the populated range, not the whole (next-power-of-two over-allocated) buffer: the shader
        // loops `arrayLength(&lights)`, which is binding_size / stride — binding the full buffer would make
        // it iterate trailing zero lights (a zero directional is `normalize(vec3(0))` = NaN). Rebound here
        // (on each light revision, not per frame) so the count always matches.
        let stride = std::mem::size_of::<LightGpu>() as u64;
        self.bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lights-bg"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &self.buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(needed * stride),
                }),
            }],
        });
    }
}

fn new_light_storage(device: &wgpu::Device, cap: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lights"),
        size: cap.max(1) * std::mem::size_of::<LightGpu>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

const SHADER: &str = include_str!("scene.wgsl");
const SSAO_SRC: &str = include_str!("ssao.wgsl");

/// The marked block in `ssao.wgsl` that declares the depth binding and its texel reader.
const SSAO_DEPTH_BLOCK_START: &str = "// >>> DEPTH BINDING BLOCK <<<";
const SSAO_DEPTH_BLOCK_END: &str = "// >>> END DEPTH BINDING BLOCK <<<";

/// The single-sample depth binding substituted into `ssao.wgsl` when MSAA is off. WGSL types
/// `texture_depth_multisampled_2d` and `texture_depth_2d` differently and one module cannot hold both, so
/// the MSAA-off variant is produced by swapping the marked block rather than by disabling SSAO (which is
/// what used to happen, leaving the "SSAO on, MSAA off" configuration unreachable).
const SSAO_SINGLE_SAMPLE_BLOCK: &str = "\
@group(1) @binding(2) var depth_tex: texture_depth_2d;
fn depth_texel(coord: vec2<i32>) -> f32 {
    return textureLoad(depth_tex, coord, 0);
}
";

/// Produce the single-sample variant of the SSAO shader. Returns the source unchanged if the markers are
/// missing, which the unit tests assert cannot happen.
fn single_sample_ssao_source(src: &str) -> String {
    let (Some(start), Some(end)) = (
        src.find(SSAO_DEPTH_BLOCK_START),
        src.find(SSAO_DEPTH_BLOCK_END),
    ) else {
        return src.to_string();
    };
    let end = end + SSAO_DEPTH_BLOCK_END.len();
    let mut out = String::with_capacity(src.len());
    out.push_str(&src[..start]);
    out.push_str(SSAO_SINGLE_SAMPLE_BLOCK);
    out.push_str(&src[end..]);
    out
}
/// M11.4 (ADR-043) — the bloom post-processing shaders (separate module; see `post.wgsl`).
const POST: &str = include_str!("post.wgsl");
const CUBE_INDICES: [u16; 36] = [
    0, 2, 3, 0, 3, 1, 4, 5, 7, 4, 7, 6, 0, 4, 6, 0, 6, 2, 1, 3, 7, 1, 7, 5, 0, 1, 5, 0, 5, 4, 2, 6,
    7, 2, 7, 3,
];
const GRID_VERTS: u32 = 6;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
    /// M11.3 inc.2 — inverse view-proj, so the skybox can turn a screen pixel back into a world ray to
    /// sample the equirect env. Unused by the cube/grid/line shaders (they ignore the trailing field).
    inv_view_proj: [[f32; 4]; 4],
    /// M11.3 inc.3 — the shadow-casting light's ortho view-proj: the depth pass projects geometry by it,
    /// and `fs_mesh` reprojects each fragment through it to look up the shadow map. Identity when nothing
    /// casts (the lookup then falls outside the unit cube → unshadowed).
    light_view_proj: [[f32; 4]; 4],
    /// Focus-mode flag, packed in `focus[0]`: `1.0` while an entity is focused, `0.0` otherwise. The
    /// shaders dim every instance whose `selected < 0.5` when this is set, so only the focused (=
    /// selected) entity stays lit — "gray out the rest." A `vec4` (not a bare `f32` + pad) so the WGSL
    /// uniform layout matches byte-for-byte: a `vec3` tail would round the std140 struct to 96 bytes
    /// while this struct is 80, and wgpu would reject the undersized buffer at draw. `[1..4]` unused.
    focus: [f32; 4],
    /// M11.3 inc.3 — `shadow[0]` is the index (into the lights buffer) of the shadow-casting directional
    /// light, or `-1.0` when nothing casts. `fs_mesh` applies the single shadow map to ONLY that light, so
    /// other directional lights (which have no map) stay unshadowed. `[1..4]` unused (pad to a vec4).
    shadow: [f32; 4],
    /// Adaptive grid metadata: target X/Z in `.xy`, camera distance in `.z`. This keeps line density stable
    /// in world space while sizing the grid plane to the current view.
    grid: [f32; 4],
}
// The WGSL `Camera` (3×mat4 + 3×vec4) is 240 bytes; keep this struct byte-identical or wgpu rejects the
// uniform at draw. A compile-time tripwire so a future field can't silently desync the layout.
const _: () = assert!(std::mem::size_of::<Camera>() == 240);

/// M11.3 inc.3 — shadow-map quality profile, chosen once at startup from `MTK_SHADOW_QUALITY`
/// (`off`|`low`|`medium`|`high`, default medium). Drives the shadow-map resolution; `Low` is the
/// entry-level gate and **`Off` is the true min-spec profile** — it skips the depth pass *and* the
/// per-fragment PCF (the scene renders fully lit, the cheapest path). Higher = sharper shadows at more
/// depth-pass + sampling cost.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ShadowQuality {
    Off,
    Low,
    Medium,
    High,
}

impl ShadowQuality {
    fn from_env() -> Self {
        match std::env::var("MTK_SHADOW_QUALITY").ok().as_deref() {
            Some("off") => Self::Off,
            Some("low") => Self::Low,
            Some("high") => Self::High,
            _ => Self::Medium,
        }
    }
    fn shadow_size(self) -> u32 {
        match self {
            // Off still allocates a tiny valid depth map (the bind group + comparison sampler need one),
            // but nothing draws into it and nothing samples it — a negligible per-frame clear.
            Self::Off => 256,
            Self::Low => 1024,
            Self::Medium => 2048,
            Self::High => 4096,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
    fn shader_level(self) -> f32 {
        match self {
            Self::Off => 0.0,
            Self::Low => 1.0,
            Self::Medium => 2.0,
            Self::High => 3.0,
        }
    }
}

/// Window handle wrapper so wgpu can make a surface from the Tauri window on a render thread.
struct WinHandle {
    window: tauri::WebviewWindow,
}
impl HasWindowHandle for WinHandle {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.window.window_handle()
    }
}
impl HasDisplayHandle for WinHandle {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        self.window.display_handle()
    }
}

/// Spawn the render loop targeting `window`'s surface, reading/writing `shared`.
pub fn start(window: tauri::WebviewWindow, shared: Shared) {
    std::thread::spawn(move || pollster::block_on(render_loop(window, shared)));
}

async fn render_loop(window: tauri::WebviewWindow, shared: Shared) {
    let size = window.inner_size().expect("inner_size");
    let (mut w, mut h) = (size.width.max(1), size.height.max(1));

    let instance = wgpu::Instance::default();
    let target = Arc::new(WinHandle {
        window: window.clone(),
    });
    let surface = match instance.create_surface(target) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[viewport] create_surface FAILED: {e}");
            return;
        }
    };
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .expect("no adapter");
    eprintln!(
        "[viewport] adapter='{}' backend={:?}",
        adapter.get_info().name,
        adapter.get_info().backend
    );
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("viewport"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .expect("device");

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(caps.formats[0]);
    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: w,
        height: h,
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);
    // M15.11 — the two render formats, carried together from here down. `hdr` is the linear-HDR scene
    // intermediate that every scene pipeline draws into; `display` is the swapchain, written by exactly one
    // pass. Before this split there was a single `format` doing both jobs, which is why MSAA resolved
    // gamma-encoded samples and each scene shader had to tone-map for itself.
    let formats = RenderFormats::new(format);
    if let Err(missing) = hdr_support_gap(&adapter, formats.hdr) {
        // No silent degrade to the old mixed-space pipeline: that path IS the defect. Rgba16Float
        // render-attachment + filtering + blending is mandatory in WebGPU core, so this is a tripwire.
        eprintln!(
            "[viewport] FATAL: adapter '{}' cannot host the linear-HDR scene target {:?} — missing {missing}. \
             There is no gamma-space fallback; update the graphics driver or select a conformant adapter.",
            adapter.get_info().name,
            formats.hdr
        );
        return;
    }
    // M11.4 (ADR-043) — MSAA anti-aliasing. Sample count chosen once from `MTK_MSAA` (`off`/`1`/`2`/`4`/`8`,
    // default 4), clamped to adapter support; 1 = off. The scene depth + every scene-pass pipeline are built
    // at this count; the depth-only shadow pass stays single-sample. The support query now asks about the
    // HDR format — the format the multisampled target actually is — not about the swapchain.
    let samples = msaa_sample_count(&adapter, formats);
    // M11.4 (ADR-043) — bloom. When on, the resolved HDR scene is bright-passed → separable Gaussian →
    // added back inside the final resolve. `MTK_BLOOM=off` skips the chain and binds a 1×1 black texture,
    // so the SAME final resolve still runs: bloom changes what is composited, never where the frame ends.
    let bloom = bloom_enabled();
    eprintln!(
        "[viewport] hdr={:?} display={:?} manual_srgb_encode={} MSAA samples={samples} bloom={bloom}",
        formats.hdr,
        formats.display,
        formats.manual_display_encode() > 0.5
    );

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene"),
        source: wgpu::ShaderSource::Wgsl(SHADER.into()),
    });
    let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("camera"),
        size: std::mem::size_of::<Camera>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let cam_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cam-bgl"),
        // VERTEX_FRAGMENT: the PBR fs_mesh (M11.2) reads the camera eye (packed in `cam.focus.yzw`) for the
        // view direction, so the camera uniform must be visible to the fragment stage too.
        entries: &[bgl_entry(
            0,
            wgpu::ShaderStages::VERTEX_FRAGMENT,
            wgpu::BufferBindingType::Uniform,
        )],
    });
    let inst_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("inst-bgl"),
        entries: &[bgl_entry(
            0,
            wgpu::ShaderStages::VERTEX,
            wgpu::BufferBindingType::Storage { read_only: true },
        )],
    });
    // M11.3 — the scene's lights, a FRAGMENT-visible read-only storage buffer (fs_mesh loops over them).
    let lights_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("lights-bgl"),
        entries: &[bgl_entry(
            0,
            wgpu::ShaderStages::FRAGMENT,
            wgpu::BufferBindingType::Storage { read_only: true },
        )],
    });
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cam-bg"),
        layout: &cam_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buf.as_entire_binding(),
        }],
    });
    // ── the post chain: bloom (HDR) + SSAO (HDR) + THE final resolve (display) ────────────────────────────
    // Built after the camera layout because every post pass now binds the Camera uniform at group 0: the
    // final resolve needs exposure and the presentation profile, and bloom extraction is exposure-aware.
    let post_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("post"),
        source: wgpu::ShaderSource::Wgsl(POST.into()),
    });
    let post_samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("post-sampler"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        ..Default::default()
    });
    let tex_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let samp_entry = wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    let post_bgl1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("post-bgl-1tex"),
        entries: &[samp_entry, tex_entry(1)],
    });
    let post_bgl2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("post-bgl-2tex"),
        entries: &[samp_entry, tex_entry(1), tex_entry(2)],
    });
    let post_layout1 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("post-layout-1"),
        bind_group_layouts: &[Some(&cam_bgl), Some(&post_bgl1)],
        immediate_size: 0,
    });
    let post_layout2 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("post-layout-2"),
        bind_group_layouts: &[Some(&cam_bgl), Some(&post_bgl2)],
        immediate_size: 0,
    });
    // Bloom lives entirely in HDR: extract → blur → blur, all Rgba16Float. Nothing here writes display.
    let bright_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout1,
        formats.hdr,
        "fs_bright",
        "bloom-bright(hdr)",
    );
    let blur_h_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout1,
        formats.hdr,
        "fs_blur_h",
        "bloom-blur-h(hdr)",
    );
    let blur_v_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout1,
        formats.hdr,
        "fs_blur_v",
        "bloom-blur-v(hdr)",
    );
    // THE final resolve — the ONLY pipeline in the renderer whose colour target is the display format, and
    // the only place exposure, tone mapping and the transfer function are applied. Every route ends here.
    let resolve_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout2,
        formats.display,
        "fs_resolve",
        "final-resolve(display)",
    );
    // The stand-in bound as "bloom" when bloom is off: 1×1, black, HDR. It makes the bloom-off route use
    // the identical pipeline, bind-group layout and shader path as the bloom-on route — the additive term
    // is simply zero — instead of forking into a second terminal pass that could drift.
    let black_bloom = black_hdr_dummy(&device, &queue, formats.hdr);

    // ── SSAO (screen-space ambient occlusion): darkens creases / contact points so an imported CAD
    // assembly reads as solid, connected parts. It now runs INSIDE the HDR half: it reads the resolved
    // linear scene and writes linear, so occlusion attenuates light and still passes through the single
    // final resolve. `MTK_SSAO=off` skips the pass; the route still ends at `fs_resolve`.
    //
    // The scene depth follows the MSAA sample count, and WGSL types a multisampled depth binding
    // differently from a single-sampled one. Rather than disabling SSAO whenever MSAA is off (the previous
    // behaviour — an honest degrade, but it left one required configuration uncovered), the shader carries
    // a substitutable depth-binding block and we build BOTH variants' resources from it.
    let ssao = ssao_enabled();
    eprintln!("[viewport] ssao={ssao} (depth binding: {}-sample)", samples);
    let ssao_source = if samples > 1 {
        SSAO_SRC.to_string()
    } else {
        single_sample_ssao_source(SSAO_SRC)
    };
    let ssao_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ssao"),
        source: wgpu::ShaderSource::Wgsl(ssao_source.into()),
    });
    let ssao_input_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ssao-input-bgl"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    // Must track the depth texture's actual sample count, or the bind group is invalid.
                    multisampled: samples > 1,
                },
                count: None,
            },
        ],
    });
    let ssao_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("ssao-layout"),
        bind_group_layouts: &[Some(&cam_bgl), Some(&ssao_input_bgl)],
        immediate_size: 0,
    });
    let ssao_pipeline = make_post_pipeline(
        &device,
        &ssao_shader,
        &ssao_layout,
        formats.hdr,
        "fs_ssao",
        "ssao(hdr)",
    );

    // Every size-dependent target in one place, built by one constructor, so a resize cannot recreate some
    // of them against stale dimensions or a stale sample count.
    let target_spec = TargetSpec {
        formats,
        samples,
        ssao,
        bloom,
    };
    let mut targets = Targets::create(
        &device,
        &target_spec,
        w,
        h,
        &post_samp,
        &post_bgl1,
        &post_bgl2,
        &ssao_input_bgl,
        &black_bloom,
    );

    // Cube instances (the M2.2 placeholder/fallback path + the perf baseline) — the subset of entities
    // with NO mesh asset. Grows with the scene.
    let mut cube = InstanceBuf::new(&device, &inst_bgl, 1024);
    // M11.3 — the scene's lights (group 2 on the mesh pipeline). Starts with room for a handful; grows.
    let mut lights_buf = LightBuf::new(&device, &lights_bgl, 8);
    // M11.3 inc.3 — the directional shadow map: a depth texture rendered from the caster's POV each frame,
    // sampled by fs_mesh with a COMPARISON sampler (hardware PCF). Fixed size per quality profile (it's the
    // LIGHT's view — independent of the window, never resized). Created BEFORE the IBL group because it
    // rides group 3 (bindings 4/5) — the device caps bind groups at 4, so the shadow can't have its own.
    let shadow_quality = ShadowQuality::from_env();
    let shadow_size = shadow_quality.shadow_size();
    let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("shadow-map"),
        size: wgpu::Extent3d {
            width: shadow_size,
            height: shadow_size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let shadow_view = shadow_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("shadow-cmp"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear, // bilinear PCF on the depth compares
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        compare: Some(wgpu::CompareFunction::LessEqual),
        ..Default::default()
    });
    // M11.3 inc.2/3 — image-based lighting + the shadow map share group 3: a procedural HDR sky + split-sum
    // BRDF LUT (bindings 0-3) + the shadow map/sampler (4/5). The shadow pass does NOT bind group 3, so the
    // map can be a render target there and sampled here without conflict.
    let ibl_bgl = crate::ibl::bind_group_layout(&device);
    let ibl = crate::ibl::create(&device, &queue, &ibl_bgl, &shadow_view, &shadow_sampler);

    let index_buf = create_init_buffer(
        &device,
        "cube-idx",
        bytemuck::cast_slice(&CUBE_INDICES),
        wgpu::BufferUsages::INDEX,
    );

    // M11.3 inc.3 — a matte ground plane (a large quad just below y=0) so the scene's shadows have a
    // surface to land on: the grid is only lines and cubes use the flat `fs_main`, so without a receiver
    // shadows would be invisible. Drawn through the mesh pipeline (fs_mesh ⇒ IBL + shadow), and NOT through
    // the shadow pass (it's a receiver, not a caster — including it would just self-shadow-acne the plane).
    let ground_vert = |x: f32, z: f32| MeshVertex {
        position: [x, 0.0, z],
        normal: [0.0, 1.0, 0.0],
        color: GROUND_ALBEDO,
        metallic: 0.0,
        roughness: 0.95,
        uv: [0.0, 0.0], // untextured (binds the white dummy)
        tangent: [0.0, 0.0, 0.0, 1.0],
    };
    let ground_verts = [
        ground_vert(-1.0, -1.0),
        ground_vert(1.0, -1.0),
        ground_vert(1.0, 1.0),
        ground_vert(-1.0, 1.0),
    ];
    let ground_vbuf = create_init_buffer(
        &device,
        "ground-vbuf",
        bytemuck::cast_slice(&ground_verts),
        wgpu::BufferUsages::VERTEX,
    );
    const GROUND_IDX: [u32; 6] = [0, 1, 2, 0, 2, 3];
    let ground_ibuf = create_init_buffer(
        &device,
        "ground-ibuf",
        bytemuck::cast_slice(&GROUND_IDX),
        wgpu::BufferUsages::INDEX,
    );
    // M11.2 follow-up — the mesh main pass's group 1: instances + a base-color texture + sampler. Distinct
    // from `inst_bgl` (cubes/ground/lines keep that), so adding a texture doesn't ripple to those pipelines.
    let mesh_inst_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("mesh-inst-bgl"),
        entries: &[
            bgl_entry(
                0,
                wgpu::ShaderStages::VERTEX,
                wgpu::BufferBindingType::Storage { read_only: true },
            ),
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3, // M11.2 follow-up — metallic-roughness texture (linear)
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 4, // M11.2 follow-up — tangent-space normal map (linear)
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
        ],
    });
    let albedo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("albedo-samp"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let dummy_view = white_dummy(&device, &queue, true); // base-color (sRGB)
    let dummy_mr_view = white_dummy(&device, &queue, false); // metallic-roughness (linear; b=g=1 → no change)
    let dummy_normal_view = flat_normal_dummy(&device, &queue); // flat +Z normal (linear)

    let mut ground_inst = InstanceBuf::new(&device, &inst_bgl, 1);
    ground_inst.upload(
        &device,
        &queue,
        &inst_bgl,
        &[Instance {
            center: [0.0, -0.02, 0.0], // a hair below the grid so the grid lines read on top
            scale: 60.0,
            color: [0.30, 0.31, 0.34],
            selected: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0; 4], // no override → use the baked matte vertex material
        }],
    );
    // The ground draws with the MESH pipeline → its group 1 must be a `mesh_inst_bgl` bind group too. It's
    // untextured (the white dummy), and its single cap-1 instance buffer never grows → built once here.
    let ground_main_bg = make_mesh_main_bg(
        &device,
        &mesh_inst_bgl,
        &ground_inst.buf,
        &dummy_view,
        &dummy_mr_view,
        &dummy_normal_view,
        &dummy_mr_view,
        &albedo_sampler,
    );

    let depth_state = wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let cube_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("cube-layout"),
        bind_group_layouts: &[Some(&cam_bgl), Some(&inst_bgl)],
        immediate_size: 0,
    });
    // M11.3 — the mesh pipeline adds group 2 (lights) for the multi-light PBR fragment shader and group 3
    // (IBL env + BRDF LUT for image-based ambient/specular [inc.2], plus the shadow map + comparison sampler
    // for directional shadows [inc.3] — all in group 3 to stay within the 4-bind-group cap).
    let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("mesh-layout"),
        bind_group_layouts: &[
            Some(&cam_bgl),
            Some(&mesh_inst_bgl), // M11.2 — group 1 = instances + base-color texture + sampler
            Some(&lights_bgl),
            Some(&ibl_bgl),
        ],
        immediate_size: 0,
    });
    // M11.3 inc.3 — the depth-only shadow pass needs only the camera (group 0, for light_view_proj) +
    // the instances (group 1). No lights/IBL/shadow groups.
    let shadow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("shadow-layout"),
        bind_group_layouts: &[Some(&cam_bgl), Some(&inst_bgl)],
        immediate_size: 0,
    });
    // M11.3 inc.2 — the skybox uses only groups 0 (camera) + 3 (env), but wgpu requires every layout slot
    // bound at draw, so 1/2 are explicit EMPTY layouts (clearer than binding unrelated buffers there).
    let empty_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("empty-bgl"),
        entries: &[],
    });
    let empty_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("empty-bg"),
        layout: &empty_bgl,
        entries: &[],
    });
    let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("sky-layout"),
        bind_group_layouts: &[
            Some(&cam_bgl),
            Some(&empty_bgl),
            Some(&empty_bgl),
            Some(&ibl_bgl),
        ],
        immediate_size: 0,
    });
    let cube_pipeline = make_pipeline(
        &device,
        &shader,
        &cube_layout,
        formats.hdr,
        &depth_state,
        "vs_cube",
        "fs_cube",
        wgpu::PrimitiveTopology::TriangleList,
        Some(wgpu::Face::Back),
        samples,
        "cube",
    );
    let grid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grid-layout"),
        bind_group_layouts: &[Some(&cam_bgl)],
        immediate_size: 0,
    });
    let mut grid_depth_state = depth_state.clone();
    grid_depth_state.depth_write_enabled = Some(false);
    let grid_pipeline = make_grid_pipeline(
        &device,
        &shader,
        &grid_layout,
        formats.hdr,
        &grid_depth_state,
        samples,
    );
    // Tracking lines: same layout as the cubes (cam + a storage buffer of points), LineList topology,
    // reading `vs_line`. A separate buffer holds the line endpoints (filled from the bindings). They
    // draw with an always-pass, no-write depth state so a binding reads as an overlay the user can
    // actually see — never buried inside or behind the dense cube field (the centres they connect).
    let line_depth_state = wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let line_pipeline = make_pipeline(
        &device,
        &shader,
        &cube_layout,
        formats.hdr,
        &line_depth_state,
        "vs_line",
        "fs_main",
        wgpu::PrimitiveTopology::LineList,
        None,
        samples,
        "line",
    );
    let mut lines = InstanceBuf::new(&device, &inst_bgl, 256);
    // M8.4 contact-debugger overlay: its own pipeline (`vs_overlay` reads each segment's per-instance
    // colour) + buffer, sharing the always-pass line depth state so contacts/normals/swept-volume read as
    // an overlay over the scene. Off by default (the buffer stays empty → the draw is skipped).
    let overlay_pipeline = make_pipeline(
        &device,
        &shader,
        &cube_layout,
        formats.hdr,
        &line_depth_state,
        "vs_overlay",
        "fs_main",
        wgpu::PrimitiveTopology::LineList,
        None,
        samples,
        "overlay",
    );
    let mut overlay = InstanceBuf::new(&device, &inst_bgl, 256);
    // M11.4 — light/camera ICON glyphs (wireframe), drawn via the overlay pipeline. Rebuilt with the scene
    // (uploaded on `revision`, like `lines`); empty ⇒ skipped.
    let mut markers = InstanceBuf::new(&device, &inst_bgl, 256);
    let mut cur_overlay_rev = u64::MAX;
    let mut cur_lights_rev = u64::MAX;
    // M9.1 transform gizmo: its own buffer, drawn with the SAME `overlay_pipeline` (vs_overlay reads the
    // per-segment colour) + always-pass depth, so the X/Y/Z handles read as an overlay over the scene.
    // Regenerated + uploaded each frame at the selected entity (constant pixel size); empty ⇒ pass skipped.
    let mut gizmo_buf = InstanceBuf::new(&device, &inst_bgl, 256);

    // Imported-mesh path (invariant 4: built/uploaded on the render thread; the hot path never crosses
    // JS). A real vertex buffer (pos/normal/baked-color) + the same cam(0)+instance-storage(1) bind
    // groups as the cube path — non-bindless (ADR-003, web-required): one vertex/index buffer bound per
    // asset, drawn instanced across the entities using it. cull=None tolerates arbitrary import winding.
    let mesh_vbl = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<MeshVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 24,
                shader_location: 2,
            },
            // M11.2 (ADR-041): baked metallic-roughness PBR factors (the Cook-Torrance inputs in fs_main).
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 36,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32,
                offset: 40,
                shader_location: 4,
            },
            // M11.2 follow-up — the UV for base-color texture sampling in fs_mesh.
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 44,
                shader_location: 5,
            },
            // Production normal maps use the same explicit MikkTSpace basis that the baker/exporter use.
            // A zero tangent on a legacy asset selects the derivative fallback in the fragment shader.
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x4,
                offset: 52,
                shader_location: 6,
            },
        ],
    };
    let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("mesh"),
        layout: Some(&mesh_layout), // M11.3: cam(0) + instances(1) + lights(2)
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_mesh"),
            buffers: std::slice::from_ref(&mesh_vbl), // borrowed — also reused by the shadow mesh pipeline
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_mesh"), // M11.2: per-fragment metallic-roughness PBR (Cook-Torrance)
            // Linear HDR, like every scene pipeline. `fs_mesh` writes radiance; nothing is encoded here.
            targets: &[Some(formats.hdr.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(depth_state.clone()),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });
    // M11.3 inc.2 — skybox: a fullscreen triangle (no vertex buffer) drawn FIRST at the far plane. Depth
    // write OFF + LessEqual so it fills the background but every mesh/grid line draws in front of it.
    let sky_depth = wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::LessEqual),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sky"),
        layout: Some(&sky_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_sky"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_sky"),
            targets: &[Some(formats.hdr.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(sky_depth),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });
    // M11.3 inc.3 — depth-only shadow pipelines: render cube + mesh geometry from the light's POV into the
    // shadow map. No fragment stage (depth only). A constant + slope depth bias on the *pass* side (here
    // via DepthBiasState) plus the shader-side bias together fight acne; cull_mode None so thin/flat
    // geometry still occludes. Same `depth_state` format (Depth32Float, Less, write on).
    let shadow_depth_state = wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState {
            constant: 2,
            slope_scale: 2.0,
            clamp: 0.0,
        },
    };
    let make_shadow_pipeline = |label: &str, entry: &str, buffers: &[wgpu::VertexBufferLayout]| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(label),
            layout: Some(&shadow_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some(entry),
                buffers,
                compilation_options: wgpu::PipelineCompilationOptions::default(),
            },
            fragment: None,
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: Some(shadow_depth_state.clone()),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        })
    };
    let shadow_cube_pipeline = make_shadow_pipeline("shadow-cube", "vs_cube_shadow", &[]);
    let shadow_mesh_pipeline = make_shadow_pipeline(
        "shadow-mesh",
        "vs_mesh_shadow",
        std::slice::from_ref(&mesh_vbl),
    );
    // Per-asset GPU geometry (slot-indexed, uploaded once per meshes_revision) + per-asset instance
    // lists (rebuilt per scene revision). `cube_scratch`/`mesh_scratch` are reused partition buffers.
    let mut gpu_meshes: Vec<Option<GpuMesh>> = Vec::new();
    let mut mesh_inst: Vec<InstanceBuf> = Vec::new();
    // M11.2 follow-up — the main-pass group-1 bind groups, **per submesh** (instances + that submesh's three
    // textures): `mesh_main_bg[slot][submesh]`. Rebuilt when an instance buffer grows (a revision). The
    // per-submesh texture views live in `gpu_meshes[slot].submeshes` (uploaded once per meshes_revision).
    let mut mesh_main_bg: Vec<Vec<wgpu::BindGroup>> = Vec::new();
    // M11.1 (ADR-040) — LOD: coarser uploaded copies per asset (`gpu_lods[slot][level]`, level 0 = LOD-1)
    // + their per-submesh bind groups (`lod_main_bg[slot][level][submesh]`), selected per slot by the camera
    // distance to that asset's instances (`mesh_centroid[slot]`). LOD-0 (full) stays `gpu_meshes`/`mesh_main_bg`.
    let mut gpu_lods: Vec<Vec<GpuMesh>> = Vec::new();
    let mut lod_main_bg: Vec<Vec<Vec<wgpu::BindGroup>>> = Vec::new();
    let mut mesh_centroid: Vec<[f32; 3]> = Vec::new();
    // Raw authored extent per mesh + displayed extent per instance group. Distance-only thresholds assume
    // every asset is normalized to one metre and prematurely collapse long procedural runs; using world
    // extent keeps LOD selection tied to projected visual size for both imports and authored geometry.
    let mut mesh_extent: Vec<f32> = Vec::new();
    let mut mesh_world_extent: Vec<f32> = Vec::new();
    let sky = sky_enabled();
    eprintln!("[viewport] sky={sky}");
    let lod_on = !matches!(std::env::var("MTK_LOD").ok().as_deref(), Some("off" | "0"));
    let mut cur_mesh_rev = u64::MAX;
    let mut cube_scratch: Vec<Instance> = Vec::new();
    let mut mesh_scratch: Vec<Vec<Instance>> = Vec::new();

    eprintln!("[viewport] render loop started");
    let mut cur_rev = u64::MAX;
    // frame-budget instrumentation (CPU submit time = encode+submit; the integrated viewport's cost)
    let mut acc_ms = 0.0f64;
    let mut acc_n = 0u32;
    let mut last_report = std::time::Instant::now();
    let mut cpu_samples: Vec<f64> = Vec::new();
    let mut last_ipc = IPC_CALLS.load(Ordering::Relaxed);
    loop {
        let frame_t0 = std::time::Instant::now();
        // resize tracking
        if let Ok(s) = window.inner_size() {
            if (s.width.max(1), s.height.max(1)) != (w, h) {
                w = s.width.max(1);
                h = s.height.max(1);
                config.width = w;
                config.height = h;
                surface.configure(&device, &config);
                // ONE call rebuilds depth, the MSAA target, the HDR scene, the SSAO input and the bloom
                // chain together, at the same size and sample count, with the bind groups that reference
                // them. Recreating these individually is how a resize used to leave a bind group pointing
                // at a view of the previous size.
                targets = Targets::create(
                    &device,
                    &target_spec,
                    w,
                    h,
                    &post_samp,
                    &post_bgl1,
                    &post_bgl2,
                    &ssao_input_bgl,
                    &black_bloom,
                );
            }
        }

        // Snapshot the OS cursor BEFORE locking `shared` (perf audit F11 / RC-4): on the render thread a
        // `window.cursor_position()` marshals to the main (tao) thread, so holding the hot render mutex
        // across it convoys tao behind the render frame. Read once up-front (like the resize `inner_size`
        // above); the orbit-drag and gizmo-drag branches below consume the snapshot inside the lock.
        let cursor_pos = window.cursor_position().ok();

        // read shared state; re-upload instances on revision change (picking is NOT serviced here —
        // it's done synchronously in the viewport_pick command, decoupled from the frame cadence)
        let (
            cam,
            cam_eye,
            focus_active,
            gizmo_verts,
            light_vp,
            caster_idx,
            exposure,
            render_profile,
            grid_meta,
        ) = {
            // The real surface aspect, so the very first framing fits the lens the user is looking
            // through rather than an assumed one.
            let aspect_hint = config.width as f32 / config.height.max(1) as f32;
            let mut st = shared.lock().unwrap();
            // Publish it before anything reads it, so a `frame_all` arriving from the UI this frame
            // frames against the surface as it is now rather than as it was when the window opened.
            st.surface_aspect = aspect_hint;
            if st.distance == 0.0 {
                // The camera has never been placed. Frame the scene rather than falling back to a
                // hard-coded distance of 60: that constant is the reason every asset, at every scale,
                // opened as a speck in an empty viewport. `frame_all` is a no-op on an empty scene, so
                // the literal survives only as the genuinely-nothing-to-look-at case.
                st.elevation = 0.4;
                st.frame_all_with_aspect(aspect_hint);
                if st.distance == 0.0 {
                    st.distance = 60.0;
                }
            }
            if st.exposure == 0.0 {
                // 0.45, not 1.0. At unity the lit scene sits about 1.6 stops above the value the ACES
                // curve is fitted around, so every pixel landed in the top half of the range with no
                // black point and no white point, and the curve's shoulder flattened the material
                // contrast that separates a rough surface from a smooth one. This is the single
                // largest pixel change in the frame. (0 = uninitialised.)
                st.exposure = DEFAULT_EXPOSURE;
            }
            // Camera input — entirely native (invariant 4): fold in any wheel zoom, and while a
            // right-drag is active, poll the OS cursor and orbit by its per-frame delta. No `invoke`
            // here; the JS side only sent drag_start/drag_end (2 calls per gesture), never per frame.
            if st.zoom_delta != 0.0 {
                // Per-unit step = 15% of the current distance, capped at the legacy 1 m/unit: wheel feel
                // is unchanged for metre-scale scenes (≥ ~7 m), while a cm-scale CAD scene zooms
                // proportionally. Floor just past the 0.1 near plane — the old 5 m floor made any zoom
                // leap a centimetre-scale mechanism out to 5 m (sub-pixel parts; the M15.9 screenshot
                // assessment caught it).
                let step = (st.distance * 0.15).min(1.0);
                st.distance = (st.distance + st.zoom_delta * step).clamp(0.12, 400.0);
                st.zoom_delta = 0.0;
            }
            if st.dragging {
                if let Some(p) = cursor_pos {
                    if let Some((lx, ly)) = st.drag_last {
                        st.orbit += (p.x - lx) as f32 * 0.01;
                        st.elevation = (st.elevation + (p.y - ly) as f32 * 0.01).clamp(-1.45, 1.45);
                    }
                    st.drag_last = Some((p.x, p.y));
                }
            } else {
                st.drag_last = None;
            }
            // M9.1 gizmo drag — parallel to the orbit, also fully native (0 per-frame IPC): poll the cursor
            // (the OS cursor, or a test-injected one) + run the gizmo's drag_update, moving the dragged
            // instance live. Only gizmo_pick_drag/gizmo_drag_end cross JS (2 per gesture), never per frame.
            if st.gizmo_dragging {
                let cursor = st.gizmo_test_cursor.or_else(|| {
                    cursor_pos.map(|p| (p.x as f32 / w.max(1) as f32, p.y as f32 / h.max(1) as f32))
                });
                if let Some(cur) = cursor {
                    let aspect = w as f32 / h.max(1) as f32;
                    let (ro, rd) = cursor_ray(
                        cur,
                        st.orbit,
                        st.elevation,
                        st.distance,
                        aspect,
                        st.cam_target,
                        st.projection,
                    );
                    let snap = st.gizmo_snap;
                    let mut world_new = st.gizmo.drag_update(
                        metrocalk_gizmo::Ray {
                            origin: ro,
                            dir: rd,
                        },
                        snap,
                    );
                    if let Some(sel) = st.gizmo_sel {
                        if sel < st.instances.len() {
                            // M9.4 magnetic intent snapping (0 per-frame IPC, all native): find the nearest
                            // meaningful target (the SHARED ADR-011 ranker) to the dragged position; show its
                            // ghost, and — unless snapping is disabled — pull the drag onto it. The drag_end
                            // command re-applies the same snap so the committed pose matches the ghost.
                            let ghost = nearest_snap(
                                &st.instances,
                                &st.snap_affinity,
                                sel,
                                world_new.translation,
                                SNAP_RADIUS,
                            )
                            .map(|i| st.instances[i].center);
                            st.snap_ghost = ghost;
                            if !st.snap_disabled {
                                if let Some(g) = ghost {
                                    world_new.translation = g;
                                }
                            }
                            // Apply the full TRS live: translate→center, rotate→rotation (so a tumble/pose
                            // is VISIBLE via the shader), scale→display scale (multiplied from the start
                            // scale). Re-upload only on actual change (a frozen drag costs nothing, and the
                            // per-frame work is all native — 0 per-frame IPC).
                            let new_scale = st.gizmo_start_scale * world_new.scale[0];
                            let inst = &mut st.instances[sel];
                            if inst.center != world_new.translation
                                || inst.rotation != world_new.rotation
                                || inst.scale != new_scale
                            {
                                inst.center = world_new.translation;
                                inst.rotation = world_new.rotation;
                                inst.scale = new_scale;
                                st.revision = st.revision.wrapping_add(1);
                            }
                        }
                    }
                }
            } else if st.snap_ghost.is_some() {
                st.snap_ghost = None; // clear the snap ghost when not dragging
            }
            // Upload per-asset mesh GEOMETRY once when the asset set changes (rare — loaded at startup).
            if st.meshes_revision != cur_mesh_rev {
                cur_mesh_rev = st.meshes_revision;
                gpu_meshes.clear();
                gpu_lods.clear();
                mesh_extent.clear();
                // Upload one `MeshGpu` (the full mesh OR a LOD) → a `GpuMesh` with per-submesh texture views.
                // M11.2: base-color is sRGB; metallic-roughness + normal are LINEAR.
                let upload_mesh = |m: &MeshGpu| -> Option<GpuMesh> {
                    if m.vertices.is_empty() || m.indices.is_empty() {
                        return None;
                    }
                    let local_bounds = local_mesh_bounds(m)?;
                    let vbuf = create_init_buffer(
                        &device,
                        "mesh-v",
                        bytemuck::cast_slice(&m.vertices),
                        wgpu::BufferUsages::VERTEX,
                    );
                    let ibuf = create_init_buffer(
                        &device,
                        "mesh-i",
                        bytemuck::cast_slice(&m.indices),
                        wgpu::BufferUsages::INDEX,
                    );
                    let submeshes = m
                        .submeshes
                        .iter()
                        .map(|sm| GpuSubMesh {
                            index_offset: sm.index_offset,
                            index_count: sm.index_count,
                            base_view: sm.base_color_texture.as_ref().map_or_else(
                                || dummy_view.clone(),
                                |t| upload_tex(&device, &queue, t, TextureSemantic::Color),
                            ),
                            mr_view: sm.metallic_roughness_texture.as_ref().map_or_else(
                                || dummy_mr_view.clone(),
                                |t| upload_tex(&device, &queue, t, TextureSemantic::Data),
                            ),
                            normal_view: sm.normal_texture.as_ref().map_or_else(
                                || dummy_normal_view.clone(),
                                |t| upload_tex(&device, &queue, t, TextureSemantic::Normal),
                            ),
                            ao_view: sm.occlusion_texture.as_ref().map_or_else(
                                || dummy_mr_view.clone(),
                                |t| upload_tex(&device, &queue, t, TextureSemantic::Data),
                            ),
                        })
                        .collect();
                    Some(GpuMesh {
                        vbuf,
                        ibuf,
                        n_idx: m.indices.len() as u32,
                        local_bounds,
                        submeshes,
                    })
                };
                for m in &st.meshes {
                    mesh_extent.push(local_mesh_bounds(m).map_or(0.001, LocalBounds::max_extent));
                    // M11.1 — also build coarser LODs for distance selection (skipped when `MTK_LOD=off`).
                    let lods: Vec<GpuMesh> = if lod_on {
                        m.lods(2).iter().filter_map(&upload_mesh).collect()
                    } else {
                        Vec::new()
                    };
                    gpu_lods.push(lods);
                    gpu_meshes.push(upload_mesh(m));
                }
                while mesh_inst.len() < gpu_meshes.len() {
                    mesh_inst.push(InstanceBuf::new(&device, &inst_bgl, 64));
                }
                while mesh_scratch.len() < gpu_meshes.len() {
                    mesh_scratch.push(Vec::new());
                }
            }
            if st.revision != cur_rev {
                cur_rev = st.revision;
                // Partition entities by mesh slot: cubes (no/unknown mesh) vs each asset's instances.
                // The entity stays in `instances`/`ids` for picking; only the *render* routing splits.
                cube_scratch.clear();
                for g in &mut mesh_scratch {
                    g.clear();
                }
                for (i, inst) in st.instances.iter().enumerate() {
                    let slot = st.mesh_slots.get(i).copied().unwrap_or(-1);
                    match usize::try_from(slot).ok() {
                        Some(s) if s < gpu_meshes.len() && gpu_meshes[s].is_some() => {
                            mesh_scratch[s].push(*inst);
                        }
                        _ => cube_scratch.push(*inst),
                    }
                }
                cube.upload(&device, &queue, &inst_bgl, &cube_scratch);
                for (slot, group) in mesh_scratch.iter().enumerate() {
                    mesh_inst[slot].upload(&device, &queue, &inst_bgl, group);
                }
                // M11.1 — each slot's instance centroid (the camera-distance basis for LOD selection).
                mesh_centroid.clear();
                for group in &mesh_scratch {
                    mesh_centroid.push(if group.is_empty() {
                        [0.0, 0.0, 0.0]
                    } else {
                        let n = group.len() as f32;
                        let mut s = [0.0f32; 3];
                        for inst in group {
                            for (sk, &ck) in s.iter_mut().zip(&inst.center) {
                                *sk += ck;
                            }
                        }
                        [s[0] / n, s[1] / n, s[2] / n]
                    });
                }
                mesh_world_extent.clear();
                for (slot, group) in mesh_scratch.iter().enumerate() {
                    let largest_instance = group
                        .iter()
                        .map(|instance| instance.scale.abs())
                        .fold(0.0_f32, f32::max);
                    mesh_world_extent.push(
                        mesh_extent.get(slot).copied().unwrap_or(1.0) * largest_instance.max(0.001),
                    );
                }
                // M11.2 follow-up — rebuild each mesh's main-pass group-1 bind groups, **one per submesh**
                // (an instance upload may have grown → a new buffer), pairing the current instance buffer with
                // that submesh's own textures. Few meshes/submeshes, only on scene-edit revisions (never per
                // frame). `mesh_inst.len() ≥ gpu_meshes.len()` (the meshes_revision block grows it first).
                mesh_main_bg.clear();
                for slot in 0..gpu_meshes.len() {
                    let groups = gpu_meshes[slot].as_ref().map_or_else(Vec::new, |mesh| {
                        mesh.submeshes
                            .iter()
                            .map(|sm| {
                                make_mesh_main_bg(
                                    &device,
                                    &mesh_inst_bgl,
                                    &mesh_inst[slot].buf,
                                    &sm.base_view,
                                    &sm.mr_view,
                                    &sm.normal_view,
                                    &sm.ao_view,
                                    &albedo_sampler,
                                )
                            })
                            .collect()
                    });
                    mesh_main_bg.push(groups);
                }
                // M11.1 — per-LOD bind groups: same (current) instance buffer + that LOD's submesh textures.
                lod_main_bg.clear();
                for (slot, lods) in gpu_lods.iter().enumerate() {
                    let per_lod: Vec<Vec<wgpu::BindGroup>> = lods
                        .iter()
                        .map(|lod| {
                            lod.submeshes
                                .iter()
                                .map(|sm| {
                                    make_mesh_main_bg(
                                        &device,
                                        &mesh_inst_bgl,
                                        &mesh_inst[slot].buf,
                                        &sm.base_view,
                                        &sm.mr_view,
                                        &sm.normal_view,
                                        &sm.ao_view,
                                        &albedo_sampler,
                                    )
                                })
                                .collect()
                        })
                        .collect();
                    lod_main_bg.push(per_lod);
                }
                // tracking-line endpoints (rebuilt in lock-step with instances)
                lines.upload(&device, &queue, &inst_bgl, &st.line_points);
                // M11.4 — light/camera icon glyphs (rebuilt with the scene).
                markers.upload(&device, &queue, &inst_bgl, &st.marker_glyphs);
            }
            // M8.4 contact-debugger overlay — uploaded on its OWN revision (the debugger updates
            // independently of scene edits; while off, the buffer is empty so there's nothing to upload).
            if st.overlay_revision != cur_overlay_rev {
                cur_overlay_rev = st.overlay_revision;
                overlay.upload(&device, &queue, &inst_bgl, &st.overlay_lines);
            }
            // M11.3 — upload the scene's lights on their own revision (decoupled from entity edits).
            if st.lights_revision != cur_lights_rev {
                cur_lights_rev = st.lights_revision;
                lights_buf.upload(&device, &queue, &lights_bgl, &st.lights);
            }
            let aspect = w as f32 / h.max(1) as f32;
            let mut cam = camera_matrix_with(
                st.orbit,
                st.elevation,
                st.distance,
                aspect,
                st.cam_target.into(),
                st.projection,
            );
            // The camera eye (world) — the PBR view direction in fs_mesh (M11.2). Carried in the Camera
            // uniform's spare `focus.yzw` (focus.x stays the focus-dim flag).
            let mut cam_eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
            // M11.4 — LOOK THROUGH the active scene camera: replace the editor view-proj with the camera's
            // (its position + fov, looking at the orbit target). A pure render projection (never Loro).
            if let Some(ov) = st.cam_override {
                let eye = Vec3::from(ov.pos);
                let proj = Mat4::perspective_rh(ov.fov_deg.to_radians(), aspect, ov.near, ov.far);
                cam = proj * Mat4::look_at_rh(eye, st.cam_target.into(), Vec3::Y);
                cam_eye = ov.pos;
            }
            // M11.3 inc.3 — the shadow-casting light's ortho view-proj, fitted to the live instance bounds.
            // The caster's shine direction comes from its entry in the lights buffer; `caster_idx` (as f32,
            // -1 = none) goes to the shader so the map shadows ONLY that light. The `Off` quality profile
            // (min-spec) forces no caster + identity VP: the depth pass draws nothing and `fs_mesh` skips the
            // per-fragment PCF (the scene renders fully lit — the cheapest path).
            let (light_vp, caster_idx) = if shadow_quality == ShadowQuality::Off {
                (Mat4::IDENTITY, -1.0)
            } else {
                let shadow_dir = st
                    .shadow_caster
                    .and_then(|i| st.lights.get(i))
                    .map(|l| [l.dir_range[0], l.dir_range[1], l.dir_range[2]]);
                (
                    shadow_view_proj(
                        shadow_dir,
                        &st.instances,
                        &st.mesh_slots,
                        &st.meshes,
                        st.cam_target.into(),
                        st.distance,
                        shadow_quality.shadow_size(),
                    ),
                    st.shadow_caster.map_or(-1.0, |i| i as f32),
                )
            };
            // M9.1: regenerate the gizmo geometry at the selected entity each frame — constant pixel size,
            // and it follows the entity through a drag. Empty when nothing is selected → the pass is
            // skipped (zero cost). World-space basis (the cube/mesh shaders don't show rotation).
            let mut gizmo_verts: Vec<Instance> = match st.selected {
                Some(sel) if sel < st.instances.len() => {
                    let origin = st.instances[sel].center;
                    let eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
                    let scale = metrocalk_gizmo::pixel_scale(eye, origin, 55f32.to_radians(), 0.14);
                    st.gizmo
                        .geometry(origin, [0.0, 0.0, 0.0, 1.0], scale)
                        .into_iter()
                        .map(|gv| Instance {
                            center: gv.pos,
                            scale: 0.0,
                            color: gv.color,
                            selected: 0.0,
                            rotation: IDENTITY_QUAT,
                            material: [0.0; 4],
                        })
                        .collect()
                }
                _ => Vec::new(),
            };
            // M9.4: the snap **ghost** — a small cyan 3-axis cross at the nearest target during a drag
            // (constant pixel size), drawn through the same overlay pass. Empty unless snapping is live.
            if let Some(g) = st.snap_ghost {
                let eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
                let s = metrocalk_gizmo::pixel_scale(eye, g, 55f32.to_radians(), 0.05);
                const GHOST: [f32; 3] = [0.2, 0.9, 0.9];
                for ax in [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]] {
                    let mark = |o: f32| Instance {
                        center: [g[0] + ax[0] * o, g[1] + ax[1] * o, g[2] + ax[2] * o],
                        scale: 0.0,
                        color: GHOST,
                        selected: 0.0,
                        rotation: IDENTITY_QUAT,
                        material: [0.0; 4],
                    };
                    gizmo_verts.push(mark(-1.0));
                    gizmo_verts.push(mark(1.0));
                }
            }
            // Pipe Forge live route: connected amber segments plus a cyan 3-axis cross at each authored
            // point. It shares the tiny gizmo line buffer (no mesh re-upload, no JS per frame) and is always
            // depth-visible, matching the direct-manipulation preview contract.
            if !st.pipe_handles.is_empty() {
                gizmo_verts.extend(pipe_graph_preview_vertices(
                    &st.pipe_edges,
                    &st.pipe_handles,
                    0.09,
                ));
            }
            // Focus dim flag (read under the same lock as the camera, so it can't lag the frame).
            (
                cam,
                cam_eye,
                if st.focused.is_some() { 1.0f32 } else { 0.0 },
                gizmo_verts,
                light_vp,
                caster_idx,
                st.exposure,
                st.render_profile,
                // `.w` tells the final resolve whether IT must apply the sRGB OETF (linear-store
                // swapchain) or the surface format does it in hardware. Applying both double-encodes.
                [
                    st.cam_target[0],
                    st.cam_target[2],
                    st.distance,
                    formats.manual_display_encode(),
                ],
            )
        };
        queue.write_buffer(
            &camera_buf,
            0,
            bytemuck::bytes_of(&Camera {
                view_proj: cam.to_cols_array_2d(),
                inv_view_proj: cam.inverse().to_cols_array_2d(),
                light_view_proj: light_vp.to_cols_array_2d(),
                focus: [focus_active, cam_eye[0], cam_eye[1], cam_eye[2]],
                // .y = exposure; .z = cinematic(0)/CAD(1); .w = shadow quality level.
                shadow: [
                    caster_idx,
                    exposure,
                    render_profile,
                    shadow_quality.shader_level(),
                ],
                grid: grid_meta,
            }),
        );
        // M9.1: upload the gizmo handle geometry (tiny — regenerated each frame at the selection).
        gizmo_buf.upload(&device, &queue, &inst_bgl, &gizmo_verts);

        // M14.2 (ADR-058) — service up to a FEW pending thumbnails per frame on our OWN encoder + readback,
        // BEFORE acquiring the swapchain (so it never contends the per-frame orbit path — invariant 4). The
        // small per-frame CAP bounds the per-frame readback stall (each readback briefly blocks the render
        // thread) so a burst can't blow the frame budget — while draining the queue fast enough that a
        // thumbnail request returns promptly (off the hot path). The JS side is dirty-only + budget-limited,
        // so the queue is EMPTY during an orbit (0 thumbnail IPC). A request whose entity vanished is dropped.
        const THUMB_PER_FRAME: usize = 4;
        let thumb_jobs: Vec<(String, u32, Instance, i32)> = {
            let mut st = shared.lock().unwrap();
            let n = st.thumb_requests.len().min(THUMB_PER_FRAME);
            let mut jobs = Vec::with_capacity(n);
            for _ in 0..n {
                let (id, size) = st.thumb_requests.remove(0);
                if let Some(i) = st.ids.iter().position(|x| x == &id) {
                    let inst = st.instances[i];
                    let slot = st.mesh_slots.get(i).copied().unwrap_or(-1);
                    jobs.push((id, size, inst, slot));
                }
            }
            jobs
        };
        if !thumb_jobs.is_empty() {
            let mut results: Vec<(String, Option<Vec<u8>>)> = Vec::with_capacity(thumb_jobs.len());
            for (id, size, inst, slot) in thumb_jobs {
                let mesh = usize::try_from(slot)
                    .ok()
                    .and_then(|s| gpu_meshes.get(s))
                    .and_then(|m| m.as_ref());
                let png = render_thumbnail(
                    &device,
                    &queue,
                    &ThumbnailPass {
                        formats,
                        samples,
                        render_profile,
                        resolve_pipeline: &resolve_pipeline,
                        post_samp: &post_samp,
                        post_bgl2: &post_bgl2,
                        black_bloom: &black_bloom,
                    },
                    &cam_bgl,
                    &inst_bgl,
                    &mesh_inst_bgl,
                    &albedo_sampler,
                    &cube_pipeline,
                    &mesh_pipeline,
                    &lights_buf.bg,
                    &ibl.bind_group,
                    &index_buf,
                    &inst,
                    mesh,
                    size,
                );
                results.push((id, png));
            }
            let mut st = shared.lock().unwrap();
            st.thumb_results.extend(results);
            // Cap so timed-out requests can't grow the result list unbounded.
            if st.thumb_results.len() > 64 {
                let excess = st.thumb_results.len() - 64;
                st.thumb_results.drain(0..excess);
            }
        }

        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&device, &config);
                continue;
            }
            _ => {
                std::thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        // M11.3 inc.3 — shadow depth pass FIRST (same encoder ⇒ it finishes before the scene pass samples
        // the map; wgpu inserts the texture barrier). ALWAYS clears the map to 1.0 (far = lit) so a scene
        // with no caster — or a directional whose castShadows is off — samples "lit", not stale depth; only
        // the geometry DRAWS are gated on having a caster (identity light_vp ⇒ skip the draws).
        {
            let mut sp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if light_vp != Mat4::IDENTITY {
                sp.set_bind_group(0, &cam_bg, &[]);
                if cube.n > 0 {
                    sp.set_pipeline(&shadow_cube_pipeline);
                    sp.set_bind_group(1, &cube.bg, &[]);
                    sp.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint16);
                    sp.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..cube.n);
                }
                sp.set_pipeline(&shadow_mesh_pipeline);
                for (slot, mesh) in gpu_meshes.iter().enumerate() {
                    let (Some(mesh), Some(inst)) = (mesh.as_ref(), mesh_inst.get(slot)) else {
                        continue;
                    };
                    if inst.n == 0 {
                        continue;
                    }
                    sp.set_bind_group(1, &inst.bg, &[]);
                    sp.set_vertex_buffer(0, mesh.vbuf.slice(..));
                    sp.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    sp.draw_indexed(0..mesh.n_idx, 0, 0..inst.n);
                }
            }
        }
        {
            // The scene pass, entirely in LINEAR HDR. With MSAA on it draws into the multisampled
            // Rgba16Float target and resolves — in linear light — into a single-sampled Rgba16Float; with
            // MSAA off it draws straight into that same texture. The destination is `scene_raw` when SSAO
            // will run (the AO pass then produces `hdr_scene`), otherwise `hdr_scene` itself. The
            // swapchain is NOT reachable from here under any configuration.
            let scene_dest = targets.scene_raw.as_ref().unwrap_or(&targets.hdr_scene);
            let (scene_color, scene_resolve) = match targets.msaa.as_ref() {
                Some(m) => (m, Some(scene_dest)),
                None => (scene_dest, None),
            };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene(hdr-linear)"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_color,
                    resolve_target: scene_resolve,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // The authored background, converted ONCE into scene-linear by the same function
                        // the shaders use — so the backdrop cannot shift just because bloom, SSAO or MSAA
                        // was switched on.
                        load: wgpu::LoadOp::Clear(clear_color(render_profile > 0.5)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &cam_bg, &[]);
            // M11.3 inc.2 — skybox first: a fullscreen triangle sampling the env (group 3). Depth-write off,
            // so the grid + meshes below draw in front of it. Gives the viewport an HDR backdrop and the
            // environment the metals reflect.
            // `MTK_SKY=off` leaves the background as the cleared colour instead — the configuration that
            // makes the clear colour observable, and the one the colour-space captures need.
            if sky {
                rp.set_pipeline(&sky_pipeline);
                rp.set_bind_group(1, &empty_bg, &[]);
                rp.set_bind_group(2, &empty_bg, &[]);
                rp.set_bind_group(3, &ibl.bind_group, &[]);
                rp.draw(0..3, 0..1);
            }
            // The grid is drawn AFTER the ground quad, further down this pass. It writes no depth, so
            // drawing it here — before the opaque ground — meant the ground painted over it completely
            // and the viewport had no visible grid at all.
            // Cube pass: the placeholder/fallback (entities with no mesh asset) + the M2.2 perf baseline.
            if cube.n > 0 {
                rp.set_pipeline(&cube_pipeline);
                rp.set_bind_group(1, &cube.bg, &[]);
                rp.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint16);
                rp.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..cube.n);
            }
            // Mesh pass: each imported asset drawn once, instanced across the entities using it
            // (non-bindless — one vertex/index buffer bound per asset).
            rp.set_pipeline(&mesh_pipeline);
            rp.set_bind_group(2, &lights_buf.bg, &[]); // M11.3 — the scene lights, shared across mesh draws
            rp.set_bind_group(3, &ibl.bind_group, &[]); // M11.3 inc.2/3 — env + BRDF LUT + shadow map/sampler
            for (slot, mesh) in gpu_meshes.iter().enumerate() {
                let (Some(mesh), Some(inst), Some(bgs)) =
                    (mesh.as_ref(), mesh_inst.get(slot), mesh_main_bg.get(slot))
                else {
                    continue;
                };
                if inst.n == 0 {
                    continue;
                }
                // M11.1 — pick the LOD by the camera distance to this asset's instance centroid; level 0 =
                // the full mesh, higher = coarser. Falls back to the full mesh if the LOD/bg isn't present.
                let n_lods = gpu_lods.get(slot).map_or(0, |l| l.len());
                let level = lod_level(
                    cam_eye,
                    mesh_centroid.get(slot).copied(),
                    n_lods,
                    mesh_world_extent.get(slot).copied().unwrap_or(1.0),
                );
                let (geo, geo_bgs) = if level == 0 {
                    (mesh, bgs)
                } else {
                    match (
                        gpu_lods.get(slot).and_then(|l| l.get(level - 1)),
                        lod_main_bg.get(slot).and_then(|l| l.get(level - 1)),
                    ) {
                        (Some(g), Some(b)) => (g, b),
                        _ => (mesh, bgs),
                    }
                };
                rp.set_vertex_buffer(0, geo.vbuf.slice(..));
                rp.set_index_buffer(geo.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                // M11.2 follow-up — one sub-draw per submesh, each with its own group-1 (instances + that
                // submesh's textures), so a multi-material mesh shows every part's texture.
                for (sm, main_bg) in geo.submeshes.iter().zip(geo_bgs) {
                    rp.set_bind_group(1, main_bg, &[]);
                    let end = sm.index_offset + sm.index_count;
                    rp.draw_indexed(sm.index_offset..end, 0, 0..inst.n);
                }
            }
            // M11.3 inc.3 — the ground plane (matte; receives IBL + the scene's shadows). Same mesh
            // pipeline, so groups 0/2/3 stay bound; only its instance (group 1, the untextured dummy) +
            // geometry change.
            rp.set_bind_group(1, &ground_main_bg, &[]);
            rp.set_vertex_buffer(0, ground_vbuf.slice(..));
            rp.set_index_buffer(ground_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..GROUND_IDX.len() as u32, 0, 0..1);
            // The grid, now that the surface it describes has been laid down.
            rp.set_pipeline(&grid_pipeline);
            rp.draw(0..GRID_VERTS, 0..1);
            // Tracking lines (binding-by-intent overlay) last, with the always-pass depth state.
            if lines.n > 0 {
                rp.set_pipeline(&line_pipeline);
                rp.set_bind_group(1, &lines.bg, &[]);
                rp.draw(0..lines.n, 0..1);
            }
            // M8.4 contact-debugger overlay, drawn over everything (per-segment colour, always-pass depth).
            // Skipped entirely when the debugger is off (`overlay.n == 0`) — zero per-frame cost.
            if overlay.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &overlay.bg, &[]);
                rp.draw(0..overlay.n, 0..1);
            }
            // M11.4 — light/camera ICON glyphs (wireframe, per-segment colour, always-pass depth) so a
            // light/camera reads as an icon, not a solid placeholder cube. Empty ⇒ skipped.
            if markers.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &markers.bg, &[]);
                rp.draw(0..markers.n, 0..1);
            }
            // M9.1 transform gizmo, drawn LAST (over everything), per-segment X/Y/Z colour, always-pass
            // depth. Skipped when nothing is selected (`gizmo_buf.n == 0`) — zero per-frame cost.
            if gizmo_buf.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &gizmo_buf.bg, &[]);
                rp.draw(0..gizmo_buf.n, 0..1);
            }
        }
        // Every remaining pass is a fullscreen triangle over the HDR scene. They share one helper so the
        // group-0 (camera) binding, the clear and the draw cannot diverge between routes.
        let mut fullscreen = |label: &str,
                              target: &wgpu::TextureView,
                              pipeline: &wgpu::RenderPipeline,
                              bg: &wgpu::BindGroup| {
            let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            p.set_pipeline(pipeline);
            p.set_bind_group(0, &cam_bg, &[]); // exposure + presentation profile + encode selector
            p.set_bind_group(1, bg, &[]);
            p.draw(0..3, 0..1);
        };
        // The route is DATA, not control flow: `post_route` appends the final resolve by construction, so
        // no combination of SSAO/bloom can produce a frame that misses it — and the test module asserts
        // exactly that for all four combinations, which an `if`/`else` chain could not be held to.
        let (route, route_len) = post_route(targets.ssao_bg.is_some(), targets.bloom.is_some());
        for &pass in &route[..route_len] {
            let resolved = match pass {
                // SSAO (HDR → HDR): reads `scene_raw` + the scene depth, reconstructs positions from the
                // camera uniform, and writes occlusion-attenuated LINEAR radiance into `hdr_scene`.
                PostPass::Ssao => targets
                    .ssao_bg
                    .as_ref()
                    .map(|bg| (pass.label(), &targets.hdr_scene, &ssao_pipeline, bg)),
                // Bloom (HDR → HDR): bright-pass → separable Gaussian (H then V). It does NOT composite;
                // the final resolve adds it, so bloom cannot be applied after tone mapping.
                PostPass::BloomBright => targets
                    .bloom
                    .as_ref()
                    .map(|b| (pass.label(), &b.a, &bright_pipeline, &b.bg_bright)),
                PostPass::BloomBlurH => targets
                    .bloom
                    .as_ref()
                    .map(|b| (pass.label(), &b.b, &blur_h_pipeline, &b.bg_blur_h)),
                PostPass::BloomBlurV => targets
                    .bloom
                    .as_ref()
                    .map(|b| (pass.label(), &b.a, &blur_v_pipeline, &b.bg_blur_v)),
                // THE final resolve — the one pass that writes the swapchain, and the one place exposure,
                // tone mapping and the display transfer function are applied.
                PostPass::Resolve => {
                    Some((pass.label(), &view, &resolve_pipeline, &targets.resolve_bg))
                }
            };
            let Some((label, target, pipeline, bg)) = resolved else {
                continue;
            };
            fullscreen(label, target, pipeline, bg);
        }
        queue.submit([enc.finish()]);
        frame.present();

        let cpu_ms = frame_t0.elapsed().as_secs_f64() * 1000.0;
        acc_ms += cpu_ms;
        acc_n += 1;
        cpu_samples.push(cpu_ms);
        if last_report.elapsed().as_secs_f64() >= 2.0 {
            cpu_samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let p50 = cpu_samples[cpu_samples.len() / 2];
            let p99 = cpu_samples[cpu_samples.len() * 99 / 100];
            let ipc_now = IPC_CALLS.load(Ordering::Relaxed);
            let ipc_window = ipc_now - last_ipc;
            last_ipc = ipc_now;
            let ipc_per_frame = ipc_window as f64 / f64::from(acc_n.max(1));
            let n_mesh: u32 = mesh_inst.iter().map(|m| m.n).sum();
            eprintln!(
                "[viewport] cubes={} meshes={n_mesh} shadow={}@{shadow_size} frames={acc_n} cpu-submit p50={p50:.3}ms p99={p99:.3}ms avg={:.3}ms | ipc={ipc_window} ({ipc_per_frame:.3}/frame)",
                cube.n,
                shadow_quality.label(),
                acc_ms / f64::from(acc_n.max(1))
            );
            acc_ms = 0.0;
            acc_n = 0;
            cpu_samples.clear();
            last_report = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
}

#[must_use]
pub fn camera_matrix(orbit: f32, elevation: f32, distance: f32, aspect: f32, target: Vec3) -> Mat4 {
    camera_matrix_with(
        orbit,
        elevation,
        distance,
        aspect,
        target,
        Projection::Perspective,
    )
}

/// The view-projection for a given projection mode.
///
/// The orthographic height is `2 * distance * tan(fov/2)` — the exact height the perspective frustum
/// spans at the orbit distance. That identity is deliberate: it means switching modes does not move the
/// subject, `frame_all` needs no separate orthographic path, and zoom keeps its meaning. An independent
/// "ortho scale" would be a second representation of the same quantity, free to disagree with the first.
#[must_use]
pub fn camera_matrix_with(
    orbit: f32,
    elevation: f32,
    distance: f32,
    aspect: f32,
    target: Vec3,
    projection: Projection,
) -> Mat4 {
    let offset = Vec3::new(
        orbit.cos() * distance * elevation.cos(),
        distance * elevation.sin(),
        orbit.sin() * distance * elevation.cos(),
    );
    let eye = target + offset;
    let far = distance * 8.0 + 100.0;
    let proj = match projection {
        Projection::Perspective => {
            Mat4::perspective_rh(CAMERA_FOV_DEG.to_radians(), aspect, 0.1, far)
        }
        Projection::Orthographic => {
            let half_h = distance * (CAMERA_FOV_DEG.to_radians() * 0.5).tan();
            let half_w = half_h * aspect;
            // The near plane goes BEHIND the eye. A parallel projection has no apex to clip against, and
            // starting at the eye would slice away everything between the camera and its orbit target -
            // which in a top view is most of the scene.
            Mat4::orthographic_rh(-half_w, half_w, -half_h, half_h, -far, far)
        }
    };
    proj * Mat4::look_at_rh(eye, target, Vec3::Y)
}

/// M11.3 inc.3 — the shadow-casting light's ortho view-proj, fitted to the scene's instance bounds so the
/// fixed-resolution shadow map lands its detail on the actual objects (not the whole ±40 grid). `None`
/// shadow_dir ⇒ identity: `fs_mesh`'s reprojection then falls outside the unit cube, reading as fully lit
/// (the depth pass is also skipped). wgpu NDC z ∈ [0,1] (`orthographic_rh`, matching `perspective_rh`).
fn shadow_view_proj(
    shadow_dir: Option<[f32; 3]>,
    instances: &[Instance],
    mesh_slots: &[i32],
    meshes: &[MeshGpu],
    camera_target: Vec3,
    camera_distance: f32,
    shadow_size: u32,
) -> Mat4 {
    let Some(dir) = shadow_dir else {
        return Mat4::IDENTITY;
    };
    let dir = Vec3::from(dir).normalize_or_zero();
    if dir.length_squared() < 1e-6 {
        return Mat4::IDENTITY;
    }
    // Use authored mesh extents, not just instance scale. The latter made a 1,000 mm CAD body displayed at
    // scale 0.001 look like a millimetre-wide caster to the shadow camera. Fit one camera-centred cascade:
    // frame-all covers the assembly, while focus mode spends the same texels on the inspected part.
    let Some((mut lo, hi)) = scene_world_bounds(instances, mesh_slots, meshes) else {
        return Mat4::IDENTITY;
    };
    lo.y = lo.y.min(0.0); // include the ground receiver without imposing a metre-scale minimum.
    let scene_center = (lo + hi) * 0.5;
    let scene_radius = ((hi - lo) * 0.5).length().max(0.02);
    let radius = (camera_distance * 0.9)
        .clamp((scene_radius * 0.02).max(0.02), scene_radius)
        .max(0.02);
    let mut center = if camera_target.distance(scene_center) + radius < scene_radius * 1.15 {
        camera_target
    } else {
        scene_center
    };
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    // Snap the light-space centre to a shadow texel. Without this, sub-texel camera motion makes the whole
    // shadow field crawl even though the geometry and light are stationary.
    let right = dir.cross(up).normalize();
    let light_up = right.cross(dir).normalize();
    let texel_world = (2.0 * radius) / shadow_size.max(1) as f32;
    let snap = |value: f32| (value / texel_world).round() * texel_world;
    center += right * (snap(center.dot(right)) - center.dot(right));
    center += light_up * (snap(center.dot(light_up)) - center.dot(light_up));

    let eye = center - dir * (radius * 2.5);
    let view = Mat4::look_at_rh(eye, center, up);
    let proj = Mat4::orthographic_rh(
        -radius,
        radius,
        -radius,
        radius,
        (radius * 0.005).max(0.0001),
        radius * 6.0,
    );
    proj * view
}

/// Pick the instance nearest the click in screen space — a pure function over the instance list +
/// camera, so the `viewport_pick` command runs it synchronously (no render-loop round-trip, no
/// frame-cadence race — the bug a hidden/throttled window exposed). `cursor` is a normalized [0,1]
/// window fraction (DPI/offset-free). No tolerance, so a click always selects the closest cube
/// (immune to the ray-vs-sphere gap problem AND to clicking a big cube's face far from its centre).
/// `best` prefers cubes in front (ndc.z ∈ [0,1], wgpu depth); `best_nc` is the fallback so a depth/`w`
/// sign convention can never make picking return `None`. `None` only when there are no instances.
#[must_use]
pub fn pick_nearest(instances: &[Instance], cursor: (f32, f32), view_proj: &Mat4) -> Option<usize> {
    let (nx, ny) = cursor;
    let click_x = nx * 2.0 - 1.0;
    let click_y = 1.0 - ny * 2.0;
    let mut best: Option<(usize, f32)> = None; // in-front nearest
    let mut best_nc: Option<(usize, f32)> = None; // nearest ignoring the depth cull
    for (i, inst) in instances.iter().enumerate() {
        let clip = *view_proj * Vec3::from(inst.center).extend(1.0);
        if clip.w.abs() < 1e-6 {
            continue;
        }
        let ndc = clip.truncate() / clip.w;
        if ndc.x.is_nan() || ndc.y.is_nan() {
            continue;
        }
        let d2 = (ndc.x - click_x).powi(2) + (ndc.y - click_y).powi(2);
        if best_nc.is_none_or(|(_, bd)| d2 < bd) {
            best_nc = Some((i, d2));
        }
        if (0.0..=1.0).contains(&ndc.z) && best.is_none_or(|(_, bd)| d2 < bd) {
            best = Some((i, d2));
        }
    }
    best.or(best_nc).map(|(i, _)| i)
}

/// M9.4 — the index of the nearest snap target to `from` (excluding `sel`) within `radius`, ranked by the
/// **shared ADR-011 `intent_order`** (proximity primary, then affinity — the *same* ranker the bind reveal
/// and the snap-graph use, NOT a parallel heuristic; the adversarial guard). `None` if nothing is in range.
/// Runs on the render thread during a drag (0 per-frame IPC). The recency tiebreak is omitted on this hot
/// path — distance and affinity dominate, and exact ties are negligible for continuous float positions.
#[must_use]
pub fn nearest_snap(
    instances: &[Instance],
    affinity: &[u32],
    sel: usize,
    from: [f32; 3],
    radius: f32,
) -> Option<usize> {
    let mut best: Option<(usize, (f32, u32, u64, u64))> = None;
    for (i, inst) in instances.iter().enumerate() {
        if i == sel {
            continue;
        }
        let (dx, dy, dz) = (
            from[0] - inst.center[0],
            from[1] - inst.center[1],
            from[2] - inst.center[2],
        );
        let dist = (dx * dx + dy * dy + dz * dz).sqrt();
        if dist > radius {
            continue;
        }
        let key = (dist, affinity.get(i).copied().unwrap_or(0), 0u64, i as u64);
        if best.is_none_or(|(_, bk)| intent_order(key, bk) == std::cmp::Ordering::Less) {
            best = Some((i, key));
        }
    }
    best.map(|(i, _)| i)
}

/// The camera eye (world position) for the orbit camera — the cursor ray origin + the pixel-scale
/// reference. Returns a plain array so it feeds the gizmo's boundary types directly.
#[must_use]
pub fn camera_eye(orbit: f32, elevation: f32, distance: f32, target: [f32; 3]) -> [f32; 3] {
    let offset = Vec3::new(
        orbit.cos() * distance * elevation.cos(),
        distance * elevation.sin(),
        orbit.sin() * distance * elevation.cos(),
    );
    (Vec3::from(target) + offset).to_array()
}

/// Unproject a normalized `[0,1]` cursor into a world-space ray `(origin, direction)` under the orbit
/// camera — the gizmo's pick + drag input. Origin is the near-plane hit; direction is normalized. Plain
/// arrays in/out (glam stays internal). wgpu NDC depth is `[0,1]`, so the near plane is `z=0`.
#[must_use]
pub fn cursor_ray(
    cursor: (f32, f32),
    orbit: f32,
    elevation: f32,
    distance: f32,
    aspect: f32,
    target: [f32; 3],
    projection: Projection,
) -> ([f32; 3], [f32; 3]) {
    let inv = camera_matrix_with(
        orbit,
        elevation,
        distance,
        aspect,
        target.into(),
        projection,
    )
    .inverse();
    let ndc_x = cursor.0 * 2.0 - 1.0;
    let ndc_y = 1.0 - cursor.1 * 2.0;
    let near = inv * Vec4::new(ndc_x, ndc_y, 0.0, 1.0);
    let far = inv * Vec4::new(ndc_x, ndc_y, 1.0, 1.0);
    let np = near.truncate() / near.w;
    let fp = far.truncate() / far.w;
    (np.to_array(), (fp - np).normalize_or_zero().to_array())
}

/// Project a world point to a normalized `[0,1]` cursor (the inverse of [`cursor_ray`]) — lets a test
/// drive a deterministic gizmo drag by supplying a world TARGET (projected to a cursor the render loop
/// then drags toward). `None` if the point is behind the camera.
#[must_use]
pub fn project_to_screen(
    world: [f32; 3],
    orbit: f32,
    elevation: f32,
    distance: f32,
    aspect: f32,
    target: [f32; 3],
    projection: Projection,
) -> Option<(f32, f32)> {
    let clip = camera_matrix_with(
        orbit,
        elevation,
        distance,
        aspect,
        target.into(),
        projection,
    ) * Vec3::from(world).extend(1.0);
    if clip.w <= 1e-6 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some(((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5))
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32, samples: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            // TEXTURE_BINDING so the SSAO post pass can sample the scene depth (MSAA → textureLoad sample 0).
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// SSAO on unless `MTK_SSAO=off` — the screen-space ambient-occlusion post pass (crease/contact darkening).
fn ssao_enabled() -> bool {
    std::env::var("MTK_SSAO").map_or(true, |v| !v.eq_ignore_ascii_case("off"))
}

/// The procedural sky backdrop, on unless `MTK_SKY=off`. With it off the scene shows the cleared
/// background instead — the only configuration in which the clear colour is visible on screen, and
/// therefore the one that can demonstrate it lands in the same colour space as everything else.
fn sky_enabled() -> bool {
    !matches!(
        std::env::var("MTK_SKY").ok().as_deref(),
        Some("off" | "0" | "false")
    )
}

/// What the adapter is missing for the linear-HDR scene target, or `Ok(())` if it can host it. Checked
/// before anything is allocated so the failure is a clear message rather than a wgpu validation abort.
fn hdr_support_gap(adapter: &wgpu::Adapter, hdr: wgpu::TextureFormat) -> Result<(), String> {
    let features = adapter.get_texture_format_features(hdr);
    let needed = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
    let mut gaps: Vec<&str> = Vec::new();
    if !features.allowed_usages.contains(needed) {
        gaps.push("RENDER_ATTACHMENT|TEXTURE_BINDING usage");
    }
    if !features
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::FILTERABLE)
    {
        // The post chain samples the HDR scene with a linear sampler (bloom blur, the final resolve).
        gaps.push("linear filtering");
    }
    if !features
        .flags
        .contains(wgpu::TextureFormatFeatureFlags::BLENDABLE)
    {
        // The grid is alpha-blended into the HDR target.
        gaps.push("blending");
    }
    if gaps.is_empty() {
        Ok(())
    } else {
        Err(gaps.join(", "))
    }
}

/// A 1×1 black HDR texture, bound as the "bloom" input when bloom is off so the final resolve runs the
/// same pipeline, layout and shader path in every configuration. Its additive contribution is exactly 0.
fn black_hdr_dummy(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    hdr: wgpu::TextureFormat,
) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bloom-off-black(hdr)"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: hdr,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Four zeroed f16 channels.
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &[0u8; 8],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(8),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// One fullscreen pass of the post chain, in execution order.
///
/// Every variant except [`PostPass::Resolve`] reads and writes [`RenderFormats::hdr`]; `Resolve` is the
/// only one that touches [`RenderFormats::display`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostPass {
    Ssao,
    BloomBright,
    BloomBlurH,
    BloomBlurV,
    Resolve,
}

impl PostPass {
    /// The debug label, so a GPU capture reads as the render graph rather than as five anonymous passes.
    fn label(self) -> &'static str {
        match self {
            Self::Ssao => "ssao(hdr)",
            Self::BloomBright => "bloom-bright(hdr)",
            Self::BloomBlurH => "bloom-blur-h(hdr)",
            Self::BloomBlurV => "bloom-blur-v(hdr)",
            Self::Resolve => "final-resolve(display)",
        }
    }
}

/// The ordered post passes for a configuration.
///
/// The point of this function is the LAST write: the final resolve is appended unconditionally, so there
/// is no arrangement of SSAO and bloom that yields a route ending anywhere else. Before the HDR migration
/// the three terminal paths (direct-to-swapchain, bloom composite, SSAO blit) were separate `if` branches,
/// and two of them silently skipped work the third did.
///
/// Fixed-capacity rather than a `Vec`, because this runs once per frame on the render thread and the route
/// has a compile-time maximum length; there is no reason for it to touch the allocator.
fn post_route(ssao: bool, bloom: bool) -> ([PostPass; 5], usize) {
    let mut route = [PostPass::Resolve; 5];
    let mut n = 0;
    let mut push = |pass| {
        route[n] = pass;
        n += 1;
    };
    if ssao {
        push(PostPass::Ssao);
    }
    if bloom {
        push(PostPass::BloomBright);
        push(PostPass::BloomBlurH);
        push(PostPass::BloomBlurV);
    }
    push(PostPass::Resolve);
    (route, n)
}

/// The configuration every size-dependent target is built from. Fixed for the life of the render loop, so
/// a resize can only change the dimensions — never the formats or the sample count.
#[derive(Clone, Copy)]
struct TargetSpec {
    formats: RenderFormats,
    samples: u32,
    ssao: bool,
    bloom: bool,
}

/// Every window-sized GPU target, plus the bind groups that reference them, built together.
///
/// They are one struct because they are one lifetime: the depth texture's sample count must match the
/// MSAA colour target, the SSAO bind group holds views of both the scene and the depth, and the final
/// resolve's bind group holds views of the scene and the bloom result. Recreating any of them alone on a
/// resize leaves a bind group pointing at a texture of the previous size.
struct Targets {
    /// Scene depth, at the scene sample count; also sampled by SSAO.
    depth: wgpu::TextureView,
    /// The multisampled HDR colour target; `None` when MSAA is off.
    msaa: Option<wgpu::TextureView>,
    /// The composed linear-HDR scene the final resolve reads. When SSAO runs it is the AO pass's output.
    hdr_scene: wgpu::TextureView,
    /// The scene pass's destination when SSAO will run (SSAO reads it and writes `hdr_scene`).
    scene_raw: Option<wgpu::TextureView>,
    /// SSAO's group-1 input: sampler + `scene_raw` + depth.
    ssao_bg: Option<wgpu::BindGroup>,
    /// The half-res bloom ping-pong chain; `None` when bloom is off.
    bloom: Option<BloomTargets>,
    /// The final resolve's group-1 input: sampler + `hdr_scene` + (bloom result | 1×1 black).
    resolve_bg: wgpu::BindGroup,
}

impl Targets {
    #[allow(clippy::too_many_arguments)] // each argument is a distinct GPU resource this must reference
    fn create(
        device: &wgpu::Device,
        spec: &TargetSpec,
        w: u32,
        h: u32,
        samp: &wgpu::Sampler,
        bgl1: &wgpu::BindGroupLayout,
        bgl2: &wgpu::BindGroupLayout,
        ssao_bgl: &wgpu::BindGroupLayout,
        black_bloom: &wgpu::TextureView,
    ) -> Self {
        let hdr = spec.formats.hdr;
        let depth = make_depth(device, w, h, spec.samples);
        let msaa = make_msaa(device, hdr, w, h, spec.samples);
        let hdr_scene = make_post_tex(device, hdr, w, h, "hdr-scene");
        let scene_raw = spec
            .ssao
            .then(|| make_post_tex(device, hdr, w, h, "hdr-scene-pre-ao"));
        let ssao_bg = scene_raw
            .as_ref()
            .map(|raw| make_ssao_bg(device, ssao_bgl, samp, raw, &depth));
        let bloom = spec
            .bloom
            .then(|| make_bloom_targets(device, hdr, w, h, samp, bgl1, &hdr_scene));
        let bloom_src = bloom.as_ref().map_or(black_bloom, |b| &b.a);
        let resolve_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("final-resolve-input"),
            layout: bgl2,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&hdr_scene),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(bloom_src),
                },
            ],
        });
        Self {
            depth,
            msaa,
            hdr_scene,
            scene_raw,
            ssao_bg,
            bloom,
            resolve_bg,
        }
    }
}

/// The SSAO input bind group (group 1): a filtering sampler + the offscreen scene colour + the scene depth
/// (multisampled — read via `textureLoad`). Rebuilt on resize when the views change.
fn make_ssao_bg(
    device: &wgpu::Device,
    bgl: &wgpu::BindGroupLayout,
    samp: &wgpu::Sampler,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ssao-input"),
        layout: bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(samp),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(color),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(depth),
            },
        ],
    })
}

fn make_inst_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("inst-bg"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    })
}

fn create_init_buffer(
    device: &wgpu::Device,
    label: &str,
    data: &[u8],
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some(label),
        contents: data,
        usage,
    })
}

fn bgl_entry(
    binding: u32,
    vis: wgpu::ShaderStages,
    ty: wgpu::BufferBindingType,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

// A thin builder over the wgpu pipeline descriptor — each parameter is one descriptor field, so the
// arity is inherent (not a sign it should be split).
#[allow(clippy::too_many_arguments)]
fn make_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    depth: &wgpu::DepthStencilState,
    vs: &str,
    // The unlit fragment entry. Explicit rather than hard-coded, because the two unlit outputs differ in
    // which exposure their authored colour is anchored to: `fs_main` is chrome (constant on screen at any
    // exposure), `fs_cube` is content (responds to exposure like the rest of the frame).
    fs: &str,
    topology: wgpu::PrimitiveTopology,
    cull: Option<wgpu::Face>,
    samples: u32,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            targets: &[Some(format.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            cull_mode: cull,
            ..Default::default()
        },
        depth_stencil: Some(depth.clone()),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

/// The grid is an antialiased transparent plane, not opaque hardware lines, so it needs its own fragment
/// entry point and alpha blend state. Keeping this descriptor separate prevents overlay/cube pipelines from
/// accidentally inheriting transparency or losing depth writes.
fn make_grid_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    depth: &wgpu::DepthStencilState,
    samples: u32,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("grid"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_grid"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_grid"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                // PREMULTIPLIED, not straight alpha: `fs_grid` emits `colour × coverage` so the blend is a
                // plain `src + (1-a)·dst` in LINEAR light. Straight alpha would still work for colour, but
                // premultiplied keeps the destination alpha meaningful and composes correctly when the
                // grid overlaps itself at grazing angles.
                blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None,
            ..Default::default()
        },
        depth_stencil: Some(depth.clone()),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    })
}

/// Sample counts this DEVICE may actually use.
///
/// `adapter.get_texture_format_features()` describes the **adapter**, and includes counts that require
/// `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`. The device here is created with `Features::empty()` (the
/// web-portable posture — see the `request_device` call), so only the counts WebGPU guarantees are legal,
/// and asking for one of the others is a validation error at texture creation, not a graceful failure.
///
/// That is exactly what `MTK_MSAA=2` and `MTK_MSAA=8` did: this GPU advertises `[1, 2, 4, 8]`, the old
/// check believed it, and the render thread panicked creating the depth texture — a dead black viewport.
const PORTABLE_SAMPLE_COUNTS: [u32; 2] = [1, 4];

/// Pick the highest usable count ≤ `requested`. Split out from the adapter query so the choice itself is
/// testable: `supported` answers whether EVERY format attached to the scene pass can be created at `c`.
fn choose_sample_count(requested: u32, supported: impl Fn(u32) -> bool) -> u32 {
    if requested <= 1 {
        return 1;
    }
    for c in [requested, 8, 4, 2] {
        if c <= requested && PORTABLE_SAMPLE_COUNTS.contains(&c) && supported(c) {
            return c;
        }
    }
    1
}

/// M11.4 (ADR-043) — MSAA sample count from `MTK_MSAA` (`off`/`1`/`2`/`4`/`8`, default 4), clamped to what
/// the device can actually do. `1` means no multisample target + no resolve — the pre-MSAA path.
///
/// The count is checked against the HDR colour format AND the depth format, because the scene pass creates
/// both at this count and a count either one cannot honour is not a usable count. Checking only the colour
/// format — which is what this did, and against the *swapchain* format at that — is the same
/// one-format-stands-for-all mistake the `RenderFormats` split exists to prevent.
fn msaa_sample_count(adapter: &wgpu::Adapter, formats: RenderFormats) -> u32 {
    let requested = match std::env::var("MTK_MSAA").ok().as_deref() {
        Some("off" | "1") => 1,
        Some("2") => 2,
        Some("8") => 8,
        _ => 4,
    };
    let colour = adapter.get_texture_format_features(formats.hdr).flags;
    let depth = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
    let chosen = choose_sample_count(requested, |c| {
        colour.sample_count_supported(c) && depth.sample_count_supported(c)
    });
    if chosen != requested {
        // Never a silent downgrade: the user asked for a specific AA quality and did not get it.
        eprintln!(
            "[viewport] MSAA {requested}x is not usable on this device for {:?}+{DEPTH_FORMAT:?} \
             (portable counts are {PORTABLE_SAMPLE_COUNTS:?}) — using {chosen}x",
            formats.hdr
        );
    }
    chosen
}

/// M11.4 — the multisampled scene COLOR target (resolved to the swapchain at the scene pass's end).
/// `None` when `samples <= 1` (MSAA off → the scene pass renders straight to the swapchain view).
fn make_msaa(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    samples: u32,
) -> Option<wgpu::TextureView> {
    if samples <= 1 {
        return None;
    }
    Some(
        device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("msaa-color"),
                size: wgpu::Extent3d {
                    width: w.max(1),
                    height: h.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&wgpu::TextureViewDescriptor::default()),
    )
}

/// M11.4 (ADR-043) — whether bloom post-processing is on. `MTK_BLOOM` = `off`/`0`/`false` disables it
/// (the min-spec path: the scene renders straight to the swapchain, byte-identical to the pre-bloom frame).
fn bloom_enabled() -> bool {
    !matches!(
        std::env::var("MTK_BLOOM").ok().as_deref(),
        Some("off" | "0" | "false")
    )
}

/// A sampleable post-pass color target (RENDER_ATTACHMENT + TEXTURE_BINDING). The returned view keeps its
/// texture alive (wgpu resources are ref-counted), so the texture handle isn't returned. `label` is
/// per-target rather than generic so a GPU capture names each stage of the HDR chain.
fn make_post_tex(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    label: &str,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// M14.2 (ADR-058) — the flagship: render ONE entity to a small offscreen target (its **real** mesh +
/// material + transform, framed at the origin and lit by the scene's lights/IBL — exactly how it renders on
/// the stage, not a type icon), read it back, and PNG-encode it → the live side-panel thumbnail. A
/// **discrete off-frame RTT** (its own encoder + readback): called by the render thread before the swapchain
/// frame, so it never touches the per-frame orbit path (invariant 4). A presentation artifact — never in the
/// op-stream/doc (zero determinism impact). Renders at the scene `samples` count (MSAA → resolve) so it
/// reuses the existing scene pipelines verbatim. Returns the PNG bytes, or `None` if the readback fails.
fn thumbnail_framing(instance: &Instance, local_bounds: LocalBounds) -> (Instance, f32) {
    let rotation = instance_rotation(instance);
    // Move the authored bounds centre—not merely the instance origin—to the thumbnail origin. Offset CAD
    // bodies and exported meshes with a distant modelling origin otherwise render cropped or entirely blank.
    let offset = rotation * (local_bounds.center * instance.scale);
    let framed = Instance {
        center: (-offset).to_array(),
        scale: instance.scale,
        color: instance.color,
        selected: 0.0,
        rotation: rotation.to_array(),
        material: instance.material,
    };
    // A bounding sphere is orientation-independent, so a portrait remains fully contained for every asset
    // rotation. The 18% margin keeps antialiased silhouettes away from the PNG edge at every DPR.
    let radius = local_bounds.half_size.length() * instance.scale.abs();
    let distance = (radius * 1.18 / (55f32.to_radians() * 0.5).tan()).clamp(0.25, 2_000.0);
    (framed, distance)
}

#[allow(clippy::too_many_arguments)]
fn render_thumbnail(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    thumb: &ThumbnailPass<'_>,
    cam_bgl: &wgpu::BindGroupLayout,
    inst_bgl: &wgpu::BindGroupLayout,
    mesh_inst_bgl: &wgpu::BindGroupLayout,
    albedo_sampler: &wgpu::Sampler,
    cube_pipeline: &wgpu::RenderPipeline,
    mesh_pipeline: &wgpu::RenderPipeline,
    lights_bg: &wgpu::BindGroup,
    ibl_bg: &wgpu::BindGroup,
    cube_index_buf: &wgpu::Buffer,
    instance: &Instance,
    mesh: Option<&GpuMesh>,
    size: u32,
) -> Option<Vec<u8>> {
    let (formats, samples) = (thumb.formats, thumb.samples);
    let format = formats.display;
    let size = size.clamp(32, 256);
    // Frame a COPY of the entity from its real authored bounds. Do not floor the instance scale: production
    // CAD commonly uses millimetre vertices with a 0.001 scale, and replacing it with 0.1 magnifies it 100×.
    let (framed, dist) = thumbnail_framing(
        instance,
        mesh.map_or(LocalBounds::UNIT_CUBE, |geometry| geometry.local_bounds),
    );
    let cam = camera_matrix(std::f32::consts::FRAC_PI_4, 0.5, dist, 1.0, Vec3::ZERO);
    let eye = camera_eye(std::f32::consts::FRAC_PI_4, 0.5, dist, [0.0; 3]);
    let cam_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("thumb-cam"),
        size: std::mem::size_of::<Camera>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(
        &cam_buf,
        0,
        bytemuck::bytes_of(&Camera {
            view_proj: cam.to_cols_array_2d(),
            inv_view_proj: cam.inverse().to_cols_array_2d(),
            // No shadow in the thumb: identity light VP ⇒ the lookup falls outside the unit cube ⇒ unshadowed.
            light_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            focus: [0.0, eye[0], eye[1], eye[2]],
            // Exposure AND the presentation profile are shared with the viewport rather than re-stated:
            // both had already drifted (the profile was pinned to cinematic here), so an asset-browser
            // thumbnail was graded differently from the same asset on the stage.
            shadow: [-1.0, DEFAULT_EXPOSURE, thumb.render_profile, 0.0], // caster -1 = none
            // `.w` — the thumbnail resolves into the display format too, so it needs the same OETF answer.
            grid: [0.0, 0.0, dist, formats.manual_display_encode()],
        }),
    );
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("thumb-cam-bg"),
        layout: cam_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: cam_buf.as_entire_binding(),
        }],
    });

    // The single-entity instance buffer (group 1 of the cube path; the storage buffer of the mesh path).
    let mut inst_buf = InstanceBuf::new(device, inst_bgl, 1);
    inst_buf.upload(device, queue, inst_bgl, &[framed]);

    // Offscreen targets: the thumbnail runs the SAME pipeline as the stage. It renders into a linear-HDR
    // target at the scene `samples` count (so the scene pipelines match and the MSAA resolve happens in
    // linear light), then goes through the SAME final resolve to a display-format COPY_SRC texture we read
    // back. Rendering it straight into the display format would have been a second, divergent grade.
    let hdr_tex = make_post_tex(device, formats.hdr, size, size, "thumb-hdr");
    let display_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumb-display"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let display_view = display_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let msaa_view =
        (samples > 1).then(|| make_post_tex_msaa(device, formats.hdr, size, size, samples));
    let depth = make_depth(device, size, size, samples);
    let (color_view, resolve_target) = match &msaa_view {
        Some(m) => (m, Some(&hdr_tex)),
        None => (&hdr_tex, None),
    };
    let resolve_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("thumb-resolve-input"),
        layout: thumb.post_bgl2,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(thumb.post_samp),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&hdr_tex),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(thumb.black_bloom),
            },
        ],
    });

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("thumb-enc"),
    });
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("thumb-scene(hdr-linear)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target,
                depth_slice: None,
                ops: wgpu::Operations {
                    // The same authored background as the stage, through the same conversion.
                    load: wgpu::LoadOp::Clear(clear_color(thumb.render_profile > 0.5)),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_bind_group(0, &cam_bg, &[]);
        if let Some(mesh) = mesh {
            // The REAL mesh: lights(2) + IBL(3) like the scene, a per-submesh group 1 (this 1-instance buffer
            // + the submesh's textures), drawn for instance 0.
            rp.set_pipeline(mesh_pipeline);
            rp.set_bind_group(2, lights_bg, &[]);
            rp.set_bind_group(3, ibl_bg, &[]);
            rp.set_vertex_buffer(0, mesh.vbuf.slice(..));
            rp.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for sm in &mesh.submeshes {
                let bg = make_mesh_main_bg(
                    device,
                    mesh_inst_bgl,
                    &inst_buf.buf,
                    &sm.base_view,
                    &sm.mr_view,
                    &sm.normal_view,
                    &sm.ao_view,
                    albedo_sampler,
                );
                rp.set_bind_group(1, &bg, &[]);
                let end = sm.index_offset + sm.index_count;
                rp.draw_indexed(sm.index_offset..end, 0, 0..1);
            }
        } else {
            // The cube fallback (a primitive / no-mesh entity) — its real transform + material colour.
            rp.set_pipeline(cube_pipeline);
            rp.set_bind_group(1, &inst_buf.bg, &[]);
            rp.set_index_buffer(cube_index_buf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..1);
        }
    }
    {
        // THE final resolve, the same pipeline the swapchain frame uses: exposure → tone curve → OETF.
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("thumb-final-resolve(display)"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &display_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rp.set_pipeline(thumb.resolve_pipeline);
        rp.set_bind_group(0, &cam_bg, &[]);
        rp.set_bind_group(1, &resolve_bg, &[]);
        rp.draw(0..3, 0..1);
    }

    // Readback: copy the resolved color into a CPU-mappable buffer (256-byte row alignment).
    let unpadded = size * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("thumb-readback"),
        size: u64::from(padded * size),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &display_tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(size),
            },
        },
        wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);

    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r.is_ok());
    });
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    if rx.recv().ok() != Some(true) {
        return None;
    }
    let data = slice.get_mapped_range();

    // De-pad rows + reorder to RGBA8 (the swapchain format is BGRA on the Windows/Vulkan path). Then PNG.
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut rgba = Vec::with_capacity((unpadded * size) as usize);
    for row in 0..size {
        let start = (row * padded) as usize;
        let line = &data[start..start + unpadded as usize];
        if bgra {
            for px in line.chunks_exact(4) {
                rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        } else {
            rgba.extend_from_slice(line);
        }
    }
    drop(data);
    buf.unmap();

    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut pe = png::Encoder::new(&mut png_bytes, size, size);
        pe.set_color(png::ColorType::Rgba);
        pe.set_depth(png::BitDepth::Eight);
        let mut w = pe.write_header().ok()?;
        w.write_image_data(&rgba).ok()?;
    }
    Some(png_bytes)
}

/// The non-size-dependent resources the thumbnail RTT borrows from the render loop. Grouped so the
/// already-long parameter list does not grow another five entries — and so the thumbnail provably uses the
/// SAME final resolve pipeline as the swapchain frame rather than a lookalike of its own.
struct ThumbnailPass<'a> {
    formats: RenderFormats,
    samples: u32,
    /// The live presentation profile (cinematic 0 / CAD 1), so a thumbnail is graded like the stage.
    render_profile: f32,
    resolve_pipeline: &'a wgpu::RenderPipeline,
    post_samp: &'a wgpu::Sampler,
    post_bgl2: &'a wgpu::BindGroupLayout,
    black_bloom: &'a wgpu::TextureView,
}

/// A multisampled offscreen color target (for the thumbnail RTT to resolve from). Mirrors [`make_msaa`] but
/// always allocates (the caller only invokes it when `samples > 1`).
fn make_post_tex_msaa(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    samples: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("thumb-msaa"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// A fullscreen post-processing pipeline (no vertex buffer, no depth, single-sample) running `vs_post` +
/// the given fragment entry, writing `target`.
///
/// `target` is deliberately explicit and per-call: the bloom and SSAO passes write
/// [`RenderFormats::hdr`], and exactly ONE call — the final resolve — writes [`RenderFormats::display`].
/// If you are adding a pass here, it almost certainly wants `formats.hdr`.
fn make_post_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    target: wgpu::TextureFormat,
    fs: &str,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_post"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some(fs),
            targets: &[Some(target.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// M11.4 — the bloom ping-pong chain, now entirely in linear HDR. `a`/`b` are half-res `Rgba16Float`
/// buffers; the composed scene lives in [`Targets::hdr_scene`] and the ADD happens in the final resolve,
/// so bloom is structurally incapable of being applied after tone mapping. Recreated on resize.
struct BloomTargets {
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    bg_bright: wgpu::BindGroup, // reads hdr_scene → a
    bg_blur_h: wgpu::BindGroup, // reads a → b
    bg_blur_v: wgpu::BindGroup, // reads b → a
}

fn make_bloom_targets(
    device: &wgpu::Device,
    hdr: wgpu::TextureFormat,
    w: u32,
    h: u32,
    samp: &wgpu::Sampler,
    bgl1: &wgpu::BindGroupLayout,
    hdr_scene: &wgpu::TextureView,
) -> BloomTargets {
    // Half-res, at full HDR precision: dropping to 8-bit here would clip exactly the values bloom exists
    // to carry, and would reintroduce banding in the glow.
    let a = make_post_tex(device, hdr, (w / 2).max(1), (h / 2).max(1), "bloom-a(hdr)");
    let b = make_post_tex(device, hdr, (w / 2).max(1), (h / 2).max(1), "bloom-b(hdr)");
    let bg1 = |label: &str, tex: &wgpu::TextureView| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: bgl1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Sampler(samp),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(tex),
                },
            ],
        })
    };
    BloomTargets {
        bg_bright: bg1("bloom-bright-input", hdr_scene),
        bg_blur_h: bg1("bloom-blur-h-input", &a),
        bg_blur_v: bg1("bloom-blur-v-input", &b),
        a,
        b,
    }
}

/// M11.1 (ADR-040) — choose a LOD level from camera distance relative to the asset's displayed world
/// extent: nearer/larger on screen = finer. `0` is the full mesh; `1..=n_lods` are progressively coarser.
/// Relative thresholds work for both normalized imports and real-scale procedural geometry. Clamped to the
/// LODs that actually exist; `0` if there are none or no centroid.
fn lod_level(
    cam_eye: [f32; 3],
    centroid: Option<[f32; 3]>,
    n_lods: usize,
    world_extent: f32,
) -> usize {
    if n_lods == 0 {
        return 0;
    }
    let Some(c) = centroid else {
        return 0;
    };
    let d2 = (0..3).map(|k| (cam_eye[k] - c[k]).powi(2)).sum::<f32>();
    let extent = world_extent.max(0.001);
    let near = 16.0 * extent;
    let mid = 34.0 * extent;
    let level = if d2 < near * near {
        0
    } else if d2 < mid * mid {
        1
    } else {
        2
    };
    level.min(n_lods)
}

/// Build the Pipe Forge route overlay as line-list endpoint pairs. Kept pure for interaction regression
/// tests: N points produce N-1 route segments and three small axis marks at every control point.
fn pipe_graph_preview_vertices(
    edges: &[[[f32; 3]; 2]],
    handles: &[[f32; 3]],
    marker: f32,
) -> Vec<Instance> {
    const ROUTE: [f32; 3] = [1.0, 0.55, 0.12];
    const POINT: [f32; 3] = [0.20, 0.92, 0.95];
    let vertex = |center: [f32; 3], color: [f32; 3]| Instance {
        center,
        scale: 0.0,
        color,
        selected: 0.0,
        rotation: IDENTITY_QUAT,
        material: [0.0; 4],
    };
    let mut out = Vec::with_capacity(edges.len() * 2 + handles.len() * 6);
    for &[from, to] in edges {
        out.push(vertex(from, ROUTE));
        out.push(vertex(to, ROUTE));
    }
    for &p in handles {
        for axis in [[marker, 0.0, 0.0], [0.0, marker, 0.0], [0.0, 0.0, marker]] {
            out.push(vertex(
                [p[0] - axis[0], p[1] - axis[1], p[2] - axis[2]],
                POINT,
            ));
            out.push(vertex(
                [p[0] + axis[0], p[1] + axis[1], p[2] + axis[2]],
                POINT,
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {

    // ── the HDR pipeline: colour space, formats, routing, shader contracts ────────────────────
    // These exist because the previous pipeline's defects were all invisible to a compiler and to a
    // screenshot taken on one machine: a scene shader that tone-mapped for itself still produced a
    // plausible picture, and MSAA resolving gamma-encoded samples only shows on a high-contrast edge.

    /// The four routing configurations, named the way the route matrix is reported.
    const ROUTES: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

    /// The route for a configuration, as an owned slice (the renderer's own call site iterates in place).
    fn route_of(ssao: bool, bloom: bool) -> Vec<PostPass> {
        let (route, n) = post_route(ssao, bloom);
        route[..n].to_vec()
    }

    /// WGSL with `//` comments removed. The shader-contract tests below assert on what the shader DOES,
    /// and the comments explaining what it deliberately no longer does would otherwise trip them.
    fn wgsl_code(src: &str) -> String {
        src.lines()
            .map(|line| line.split_once("//").map_or(line, |(code, _)| code))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// This file's source WITHOUT this test module — the source-contract tests scan the renderer, and
    /// would otherwise match the very literals they are searching for inside their own assertions.
    fn renderer_source() -> &'static str {
        let src: &'static str = include_str!("render.rs");
        // Truncate at THIS module, not at the first `#[cfg(test)]` — several colour helpers above are
        // test-only too, and cutting at the first one hid the pipeline factories from the scan entirely.
        src.find("mod tests {").map_or(src, |at| &src[..at])
    }

    #[test]
    fn every_route_ends_at_the_final_resolve() {
        // The whole point of the migration: SSAO on or off, bloom on or off, the frame always terminates
        // at the one pass that applies exposure, tone mapping and the display transfer function. The old
        // renderer had THREE terminal paths and they did not agree.
        for (ssao, bloom) in ROUTES {
            let route = route_of(ssao, bloom);
            assert_eq!(
                route.last(),
                Some(&PostPass::Resolve),
                "ssao={ssao} bloom={bloom} must end at the final resolve, got {route:?}"
            );
            assert_eq!(
                route.iter().filter(|p| **p == PostPass::Resolve).count(),
                1,
                "ssao={ssao} bloom={bloom} must tone-map exactly ONCE, got {route:?}"
            );
        }
    }

    #[test]
    fn bloom_is_composed_before_tone_mapping_never_after() {
        // Ordering, not just presence: every bloom pass must precede the resolve, or the glow would be
        // added to display-encoded pixels — the classic "bloom applied after tone mapping" artefact.
        for (ssao, bloom) in ROUTES {
            let route = route_of(ssao, bloom);
            let resolve = route.iter().position(|p| *p == PostPass::Resolve).unwrap();
            for (i, pass) in route.iter().enumerate() {
                assert!(
                    *pass == PostPass::Resolve || i < resolve,
                    "ssao={ssao} bloom={bloom}: {pass:?} runs after the resolve"
                );
            }
            assert_eq!(
                route.contains(&PostPass::BloomBright),
                bloom,
                "the bloom chain must appear exactly when bloom is on"
            );
            assert_eq!(
                route.contains(&PostPass::Ssao),
                ssao,
                "the SSAO pass must appear exactly when SSAO is on"
            );
        }
    }

    #[test]
    fn the_scene_intermediate_is_a_float_hdr_format_and_the_display_is_separate() {
        let formats = RenderFormats::new(wgpu::TextureFormat::Bgra8Unorm);
        assert_eq!(formats.hdr, wgpu::TextureFormat::Rgba16Float);
        assert_ne!(
            formats.hdr, formats.display,
            "the scene must not render into the swapchain format"
        );
        // 16-bit float: values above 1.0 survive, which is what makes bloom extraction and exposure real
        // rather than a rescale of already-clipped pixels.
        assert!(formats.hdr.components() == 4);
    }

    #[test]
    fn the_sample_count_needs_every_attached_format_to_agree() {
        // The defect this closes: the count was checked against ONE format (and the swapchain's, at that).
        // This GPU's adapter advertises [1,2,4,8] for both colour and depth, but the device is created
        // with `Features::empty()`, so 2 and 8 need TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES and asking
        // for them panicked the render thread inside `make_depth` — a black viewport at launch.
        let everything = |_c: u32| true;
        assert_eq!(choose_sample_count(4, everything), 4);
        assert_eq!(
            choose_sample_count(8, everything),
            4,
            "8x is not portable — degrade to the highest count that is"
        );
        assert_eq!(
            choose_sample_count(2, everything),
            1,
            "2x is not portable either, and 4x is above what was asked for"
        );
        assert_eq!(choose_sample_count(1, everything), 1);
        assert_eq!(choose_sample_count(0, everything), 1);

        // A format that cannot do 4x drags the whole scene pass down to 1, because the colour target, the
        // depth target and every pipeline's multisample state are all created at this one count.
        let depth_cannot_multisample = |c: u32| c == 1;
        assert_eq!(choose_sample_count(4, depth_cannot_multisample), 1);
        assert_eq!(choose_sample_count(8, depth_cannot_multisample), 1);

        // Whatever is chosen must be a count WebGPU guarantees, for any request.
        for requested in [0u32, 1, 2, 3, 4, 8, 16, 64] {
            let chosen = choose_sample_count(requested, everything);
            assert!(
                PORTABLE_SAMPLE_COUNTS.contains(&chosen),
                "requested {requested}x chose {chosen}x, which is not portable"
            );
            assert!(
                chosen <= requested.max(1),
                "{chosen}x exceeds the {requested}x asked for"
            );
        }
    }

    #[test]
    fn the_display_encode_follows_the_surface_format() {
        // Applying the OETF to a swapchain that already converts in hardware double-encodes the frame
        // (a washed-out, milky image); omitting it on a linear-store swapchain gives a near-black one.
        assert_eq!(
            RenderFormats::new(wgpu::TextureFormat::Bgra8Unorm).manual_display_encode(),
            1.0,
            "a linear-store swapchain needs the shader to encode"
        );
        assert_eq!(
            RenderFormats::new(wgpu::TextureFormat::Bgra8UnormSrgb).manual_display_encode(),
            0.0,
            "an sRGB swapchain converts in hardware"
        );
    }

    #[test]
    fn srgb_conversion_round_trips_and_is_not_a_gamma_approximation() {
        for step in 0..=255u8 {
            let v = f32::from(step) / 255.0;
            let back = linear_to_srgb_f32(srgb_to_linear_f32(v));
            assert!(
                (back - v).abs() < 1e-4,
                "sRGB round-trip broken at {v}: got {back}"
            );
        }
        // The exact curve, not pow(2.2): they differ most in the toe, which is where banding lives.
        let toe = srgb_to_linear_f32(0.02);
        assert!(
            (toe - 0.02 / 12.92).abs() < 1e-6,
            "the linear segment of the OETF must be exact, got {toe}"
        );
    }

    #[test]
    fn unlit_authored_colour_round_trips_through_the_display_transform() {
        // The contract for every helper, gizmo, marker, grid line and clear colour: what the author typed
        // is what lands on screen, at ANY exposure and in EITHER presentation profile. A plain
        // srgb_to_linear would leave these 25–40% dark and would drift with the exposure slider.
        let authored: [[f32; 3]; 9] = [
            [0.10, 0.12, 0.17], // grid
            [0.60, 1.00, 0.93], // tracking line
            [0.90, 0.25, 0.25], // gizmo X
            [0.30, 0.85, 0.30], // gizmo Y
            [0.30, 0.50, 0.95], // gizmo Z
            [1.00, 0.82, 0.20], // light marker
            [0.35, 0.80, 1.00], // camera marker
            [0.04, 0.05, 0.08], // clear colour
            [1.00, 0.82, 0.16], // selection tint
        ];
        for cad in [false, true] {
            for exposure in [0.2f32, DEFAULT_EXPOSURE, 1.0, 2.5] {
                for c in authored {
                    let scene = unlit_srgb_to_scene_linear(c, exposure, cad);
                    let back = scene_linear_to_display(scene, exposure, cad);
                    let peak = c
                        .iter()
                        .copied()
                        .fold(0.0f32, |m, v| m.max(srgb_to_linear_f32(v)));
                    if peak <= unlit_display_cap(cad) {
                        // Below the profile's cap, an authored colour round-trips EXACTLY.
                        for i in 0..3 {
                            assert!(
                                (back[i] - c[i]).abs() < 0.02,
                                "cad={cad} exposure={exposure} channel {i} of {c:?} rendered {back:?}"
                            );
                        }
                        continue;
                    }
                    // Above the cap the guarantee is not equality but a single UNIFORM LINEAR SCALE: the
                    // hue is exact and only the overall level gives. That is the deliberate trade for a
                    // tone curve that only approaches display white, and for a bloom extractor that
                    // cannot blow out on a helper line.
                    let ratio = |i: usize| srgb_to_linear_f32(back[i]) / srgb_to_linear_f32(c[i]);
                    let k = ratio(0);
                    // The scale is `cap / peak`, and `peak <= 1`, so it can never fall below the cap.
                    assert!(
                        k >= unlit_display_cap(cad) - 0.005,
                        "cad={cad} exposure={exposure}: {c:?} lost {:.1}% of its level ({back:?}) — \
                         more than the profile's cap of {} accounts for",
                        (1.0 - k) * 100.0,
                        unlit_display_cap(cad)
                    );
                    for i in 1..3 {
                        assert!(
                            (ratio(i) - k).abs() < 0.01,
                            "cad={cad} exposure={exposure}: {c:?} was not scaled UNIFORMLY \
                             (channel 0 ×{k}, channel {i} ×{}) — the hue shifted: {back:?}",
                            ratio(i)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_unlit_ceiling_keeps_bright_helpers_out_of_bloom_blow_out() {
        // Authored white would otherwise need an unbounded scene value (the tone curve only approaches
        // 1.0), and feeding that to the bloom extractor wraps every white helper line in a blown halo.
        for exposure in [0.2f32, DEFAULT_EXPOSURE, 1.0] {
            for cad in [false, true] {
                let white = unlit_srgb_to_scene_linear([1.0, 1.0, 1.0], exposure, cad);
                for ch in white {
                    let exposed = ch * exposure;
                    assert!(
                        exposed <= UNLIT_EXPOSED_CEILING + 1e-4,
                        "unlit white reached exposed {exposed} (ceiling {UNLIT_EXPOSED_CEILING})"
                    );
                }
                // …and it still reads as white on screen: exactly the profile's cap, encoded. Cinematic
                // gives 0.962, CAD 0.896 (its cap is lower, which is what keeps saturated helper colours
                // exactly reproducible under a curve that desaturates as it compresses).
                let shown = scene_linear_to_display(white, exposure, cad);
                let expected = linear_to_srgb_f32(unlit_display_cap(cad));
                assert!(
                    expected > 0.89,
                    "cad={cad}: the cap renders authored white at {expected} — too grey to read"
                );
                for ch in shown {
                    assert!(
                        (ch - expected).abs() < 0.01,
                        "unlit white rendered as {shown:?}, expected {expected}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_unlit_display_caps_stay_in_each_curves_faithful_range() {
        // Cinematic: the cap IS the curve at the ceiling, so white costs exactly the ceiling's worth.
        assert!(
            (tonemap_aces_channel(UNLIT_EXPOSED_CEILING) - UNLIT_DISPLAY_CAP_ACES).abs() < 1e-4,
            "UNLIT_DISPLAY_CAP_ACES is stale: the curve gives {}",
            tonemap_aces_channel(UNLIT_EXPOSED_CEILING)
        );
        // CAD: PBR Neutral desaturates above its compression knee, which LIFTS the darkest channel and
        // eventually puts saturated bright colours outside the operator's range entirely (at a cap of
        // 0.96 the amber light marker's blue came back at 0.39 instead of 0.20). The cap must sit where
        // that lift is still below one 8-bit code — i.e. invisible.
        let cap = UNLIT_DISPLAY_CAP_PBR_NEUTRAL;
        let lift = if cap < PBR_NEUTRAL_START {
            0.0 // no compression at all below the knee
        } else {
            let peak =
                2.0 * PBR_NEUTRAL_START - 1.0 + (1.0 - PBR_NEUTRAL_START).powi(2) / (1.0 - cap);
            let g = 1.0 / (PBR_NEUTRAL_DESAT * (peak - cap) + 1.0);
            cap * (1.0 - g) // the darkest display-linear value still reachable alongside `cap`
        };
        assert!(
            linear_to_srgb_f32(lift) < 1.0 / 255.0,
            "the CAD cap ({cap}) lifts black to {} sRGB — a saturated helper colour would wash out",
            linear_to_srgb_f32(lift)
        );
        // …and it must still be bright enough to read as white.
        assert!(
            linear_to_srgb_f32(UNLIT_DISPLAY_CAP_PBR_NEUTRAL) > 0.89,
            "the CAD cap renders authored white at {} — too dim for a helper line",
            linear_to_srgb_f32(UNLIT_DISPLAY_CAP_PBR_NEUTRAL)
        );
        // The WGSL mirror must carry the same numbers, or unlit colour drifts between CPU and GPU.
        let code = wgsl_code(SHADER);
        assert!(code.contains("const UNLIT_EXPOSED_CEILING: f32 = 2.0;"));
        assert!(code.contains("const UNLIT_DISPLAY_CAP_ACES: f32 = 0.914855;"));
        assert!(code.contains("const UNLIT_DISPLAY_CAP_PBR_NEUTRAL: f32 = 0.78;"));
    }

    #[test]
    fn capping_a_saturated_unlit_colour_preserves_its_hue() {
        // The bug this closes: clamping the inverse per channel turned the cyan tracking line and the
        // amber light marker grey, because one saturated channel dragged its neighbours to the ceiling.
        for cad in [false, true] {
            for c in [[0.60, 1.00, 0.93], [1.00, 0.82, 0.20], [0.35, 0.80, 1.00]] {
                let shown = scene_linear_to_display(
                    unlit_srgb_to_scene_linear(c, DEFAULT_EXPOSURE, cad),
                    DEFAULT_EXPOSURE,
                    cad,
                );
                // Channel ORDER is the hue signature: it must survive the cap intact.
                let order = |v: [f32; 3]| (v[0] < v[1], v[1] < v[2], v[0] < v[2]);
                assert_eq!(
                    order(shown),
                    order(c),
                    "cad={cad}: {c:?} lost its hue, rendered {shown:?}"
                );
            }
        }
    }

    #[test]
    fn the_clear_colour_is_the_authored_background_and_tracks_exposure() {
        // The invariant the brief cares about is that enabling bloom, SSAO or MSAA can never shift the
        // background — none of them touch it. What the background MUST do is behave like content: render
        // as authored at the default exposure, and brighten with the frame as the exposure comes up.
        for cad in [false, true] {
            let c = clear_color(cad);
            assert!(c.a == 1.0, "the viewport background is opaque");
            #[allow(clippy::cast_possible_truncation)]
            let scene = [c.r as f32, c.g as f32, c.b as f32];

            let at_default = scene_linear_to_display(scene, DEFAULT_EXPOSURE, cad);
            for i in 0..3 {
                assert!(
                    (at_default[i] - CLEAR_COLOR_SRGB[i]).abs() < 0.02,
                    "cad={cad}: background rendered {at_default:?}, authored {CLEAR_COLOR_SRGB:?}"
                );
            }
            let mut previous = -1.0f32;
            for exposure in [0.15f32, DEFAULT_EXPOSURE, 1.0, 4.0] {
                let level = scene_linear_to_display(scene, exposure, cad)[2];
                assert!(
                    level > previous,
                    "cad={cad}: the background must track exposure, but {exposure} gave {level} \
                     after {previous}"
                );
                previous = level;
            }
        }
    }

    #[test]
    fn chrome_holds_its_colour_across_the_exposure_range_and_content_does_not() {
        // The distinction the captures forced. At exposure 4.0 the lit scene blew out while the unlit
        // placeholder cubes sat at their default-exposure brightness, so they read as pasted onto the
        // image. Chrome SHOULD hold — a gizmo handle that dims when you stop the scene down is one you
        // cannot grab — but scene content must not.
        let colour = [0.30, 0.85, 0.30]; // the same authored value, read either way
        for cad in [false, true] {
            let mut chrome = Vec::new();
            let mut content = Vec::new();
            for exposure in [0.15f32, DEFAULT_EXPOSURE, 1.0, 4.0] {
                // Chrome converts against the LIVE exposure, so it lands on the same pixel every time.
                let c = unlit_srgb_to_scene_linear(colour, exposure, cad);
                chrome.push(scene_linear_to_display(c, exposure, cad)[1]);
                // Content converts against the REFERENCE exposure, so the slider moves it.
                let s = unlit_srgb_to_scene_linear(colour, DEFAULT_EXPOSURE, cad);
                content.push(scene_linear_to_display(s, exposure, cad)[1]);
            }
            for level in &chrome {
                assert!(
                    (level - chrome[0]).abs() < 0.01,
                    "cad={cad}: chrome must not move with exposure, got {chrome:?}"
                );
            }
            assert!(
                content[3] - content[0] > 0.2,
                "cad={cad}: content must track exposure, got {content:?}"
            );
            // …and at the default exposure the two agree exactly. That is what anchoring buys: the
            // migration changes nothing about how the viewport looks when it opens.
            assert!(
                (content[1] - chrome[1]).abs() < 0.001,
                "cad={cad}: at the default exposure chrome and content must render identically"
            );
        }
        // The WGSL mirror must anchor content at the same reference, or CPU and GPU disagree.
        let code = wgsl_code(SHADER);
        assert!(
            code.contains("const REFERENCE_EXPOSURE: f32 = 0.45;"),
            "scene.wgsl's REFERENCE_EXPOSURE must equal DEFAULT_EXPOSURE ({DEFAULT_EXPOSURE})"
        );
        assert!((DEFAULT_EXPOSURE - 0.45).abs() < 1e-6);
        assert!(
            code.contains("fn fs_cube"),
            "the cube path needs its own unlit output, anchored differently from chrome"
        );
    }

    #[test]
    fn the_tone_curves_are_monotone_bounded_and_stable_at_the_extremes() {
        // Numerical stability of the operator the whole frame passes through: no NaN, no inversion, no
        // value escaping [0,1] — including at 0 and at values far above the display range.
        for cad in [false, true] {
            let mut previous = -1.0f32;
            for step in 0i16..2000 {
                let x = f32::from(step) * 0.05;
                let mapped = scene_linear_to_display([x, x, x], 1.0, cad)[0];
                assert!(mapped.is_finite(), "cad={cad} x={x} produced {mapped}");
                assert!(
                    (0.0..=1.0).contains(&mapped),
                    "cad={cad} x={x} escaped the display range: {mapped}"
                );
                assert!(
                    mapped >= previous - 1e-5,
                    "cad={cad} the curve inverted at x={x}: {previous} → {mapped}"
                );
                previous = mapped;
            }
            assert_eq!(
                scene_linear_to_display([0.0; 3], 1.0, cad)[0],
                0.0,
                "black must stay black"
            );
        }
    }

    #[test]
    fn the_tone_curve_inverses_are_exact() {
        // The unlit round-trip rests on these; a sloppy inverse would show as a hue shift on the gizmo.
        for step in 1i16..999 {
            let y = f32::from(step) / 1000.0;
            let back = tonemap_aces_channel(inverse_tonemap_aces_channel(y));
            assert!(
                (back - y).abs() < 1e-3,
                "ACES inverse broken at {y}: got {back}"
            );
        }
        for step in 1i16..99 {
            let v = f32::from(step) / 100.0;
            let colour = [v, v * 0.6, v * 0.3];
            let back = tonemap_pbr_neutral(inverse_tonemap_pbr_neutral(colour));
            for i in 0..3 {
                assert!(
                    (back[i] - colour[i]).abs() < 2e-3,
                    "PBR-Neutral inverse broken at {colour:?}: got {back:?}"
                );
            }
        }
    }

    // ── shader-source contracts ───────────────────────────────────────────────────────────────
    // Cheap, exact, and they fail the moment someone reintroduces the thing the migration removed.

    #[test]
    fn no_scene_shader_encodes_for_display() {
        let code = wgsl_code(SHADER);
        for banned in ["display_encode", "fn to_srgb", "to_srgb(", "fn tonemap_"] {
            assert!(
                !code.contains(banned),
                "scene.wgsl must write LINEAR HDR, but still contains `{banned}` — tone mapping and \
                 display encoding belong in post.wgsl's fs_resolve, which runs once per frame"
            );
        }
        // The conversion that IS allowed there: authored sRGB coming IN.
        assert!(SHADER.contains("fn unlit_srgb_to_scene_linear"));
    }

    #[test]
    fn the_final_resolve_is_the_only_display_encode() {
        assert!(
            POST.contains("fn fs_resolve"),
            "the final resolve must exist"
        );
        assert!(
            POST.contains("fn to_srgb"),
            "the display transfer function lives in post.wgsl"
        );
        // The old second terminal path is gone from the SSAO module.
        assert!(
            !wgsl_code(SSAO_SRC).contains("fs_blit"),
            "the SSAO blit was a second path to the swapchain; every route now ends at fs_resolve"
        );
        assert!(
            !wgsl_code(POST).contains("fs_composite"),
            "bloom no longer composites to the swapchain — the final resolve adds it"
        );
    }

    #[test]
    fn the_bloom_threshold_was_retuned_for_linear_hdr() {
        // 0.78 was tuned against TONEMAPPED values in [0,1]. Carried into linear HDR unchanged it would
        // have caught most of a normally-lit scene, so the whole image would glow.
        assert!(
            !POST.contains("THRESHOLD: f32 = 0.78"),
            "the display-space bloom threshold survived the migration to linear HDR"
        );
        assert!(
            POST.contains("BLOOM_THRESHOLD"),
            "bloom extraction must state its threshold explicitly"
        );
        for expected in ["BLOOM_KNEE", "BLOOM_CLAMP", "fn luminance"] {
            assert!(
                POST.contains(expected),
                "HDR bloom needs {expected} (soft knee, firefly suppression, luminance extraction)"
            );
        }
    }

    #[test]
    fn the_scene_shader_writes_linear_and_the_grid_writes_premultiplied() {
        assert!(
            SHADER.contains("unlit_srgb_to_scene_linear(GRID_COLOR) * alpha"),
            "the grid must emit premultiplied linear colour to match PREMULTIPLIED_ALPHA_BLENDING"
        );
    }

    #[test]
    fn ssao_has_a_single_sample_variant_that_differs_only_in_the_depth_binding() {
        // Without this, "SSAO on + MSAA off" was unreachable: the multisampled depth binding is a hard
        // validation error against a single-sampled texture, so SSAO silently turned itself off.
        let single = single_sample_ssao_source(SSAO_SRC);
        assert_ne!(single, SSAO_SRC, "the depth-binding block must be swapped");
        assert!(SSAO_SRC.contains("texture_depth_multisampled_2d"));
        assert!(single.contains("var depth_tex: texture_depth_2d;"));
        assert!(!single.contains("texture_depth_multisampled_2d"));
        assert!(
            !single.contains("textureNumSamples"),
            "textureNumSamples does not exist for a non-multisampled texture"
        );
        // Everything outside the block is untouched — same AO maths, same bindings, same entry point.
        assert!(single.contains("fn fs_ssao"));
        assert!(single.contains("fn depth_texel(coord: vec2<i32>) -> f32"));
        assert!(single.contains("@group(1) @binding(1) var color_tex: texture_2d<f32>;"));
    }

    #[test]
    fn every_scene_pipeline_is_built_against_the_hdr_format() {
        // A pipeline whose declared fragment target does not match its attachment is a wgpu validation
        // error, so this catches the mistake at `cargo test` rather than as a black viewport at launch.
        let src = renderer_source();
        for factory in ["make_pipeline(", "make_grid_pipeline("] {
            let mut from = 0usize;
            let mut seen = 0usize;
            while let Some(at) = src[from..].find(factory) {
                let start = from + at;
                // Skip the definitions themselves (`fn make_pipeline(`).
                if src[..start].trim_end().ends_with("fn") {
                    from = start + factory.len();
                    continue;
                }
                let window = &src[start..(start + 400).min(src.len())];
                let hdr = window.find("formats.hdr");
                let display = window.find("formats.display");
                assert!(
                    hdr.is_some() && (display.is_none() || display > hdr),
                    "a `{factory}` call site does not pass formats.hdr — a scene pipeline must never be \
                     built against the swapchain format:\n{window}"
                );
                seen += 1;
                from = start + factory.len();
            }
            assert!(seen > 0, "expected to find `{factory}` call sites");
        }
    }

    #[test]
    fn only_the_final_resolve_writes_the_display_format() {
        let src = renderer_source();
        let mut display_targets = 0usize;
        let mut from = 0usize;
        while let Some(at) = src[from..].find("make_post_pipeline(") {
            let start = from + at;
            if src[..start].trim_end().ends_with("fn") {
                from = start + 1;
                continue;
            }
            let window = &src[start..(start + 400).min(src.len())];
            if window.contains("formats.display") {
                display_targets += 1;
                assert!(
                    window.contains("fs_resolve"),
                    "only the final resolve may target the display format:\n{window}"
                );
            }
            from = start + 1;
        }
        assert_eq!(
            display_targets, 1,
            "there must be exactly ONE display-format pipeline in the renderer"
        );
    }

    // ── viewport framing ──────────────────────────────────────────────────────────────────────
    // These exist because `frame_all` had NO test coverage at all, which is why a framing constant
    // that ignored both the field of view and the aspect ratio survived unnoticed.

    #[test]
    fn the_axis_views_are_orthographic_and_the_three_quarter_view_is_not() {
        // The defect this closes: every canonical view was a 55-degree perspective, so a "top" view
        // still had vanishing points and nothing in it could be compared by eye.
        let mut st = SceneState::default();
        for preset in ["top", "front", "side"] {
            st.set_view_preset(preset);
            assert_eq!(
                st.projection,
                Projection::Orthographic,
                "{preset} must be parallel"
            );
        }
        st.set_view_preset("persp");
        assert_eq!(st.projection, Projection::Perspective);
    }

    #[test]
    fn a_parallel_projection_keeps_equal_objects_equal_at_different_depths() {
        // The property that MAKES it an axis view: two identical objects at different distances from the
        // camera must measure the same on screen. Under perspective the far one shrinks; that is exactly
        // what stopped a top view being usable for comparing anything.
        let (orbit, elevation, distance, aspect) = (0.0, 0.0, 20.0, 16.0 / 9.0);
        let screen_size = |projection, depth: f32| {
            let m = camera_matrix_with(orbit, elevation, distance, aspect, Vec3::ZERO, projection);
            // At orbit 0 the camera sits on +X looking down -X, so DEPTH is the x axis. Probing z
            // would move the points sideways and measure nothing.
            let at = |y: f32| {
                let c = m * Vec3::new(depth, y, 0.0).extend(1.0);
                c.y / c.w
            };
            (at(1.0) - at(-1.0)).abs()
        };
        let (near_o, far_o) = (
            screen_size(Projection::Orthographic, -6.0),
            screen_size(Projection::Orthographic, 6.0),
        );
        assert!(
            (near_o - far_o).abs() < 1e-5,
            "parallel projection must not foreshorten: {near_o} vs {far_o}"
        );
        let (near_p, far_p) = (
            screen_size(Projection::Perspective, -6.0),
            screen_size(Projection::Perspective, 6.0),
        );
        assert!(
            (near_p - far_p).abs() > 1e-3,
            "the 3/4 view must KEEP its depth cues: {near_p} vs {far_p}"
        );
    }

    #[test]
    fn switching_projection_does_not_move_the_subject() {
        // The orthographic height is derived from the SAME orbit distance, so a mode switch is a change
        // of projection and not a jump. An independent "ortho scale" would be free to disagree.
        let (orbit, elevation, distance, aspect) = (0.0, 0.0, 20.0, 16.0 / 9.0);
        let height_at_target = |projection| {
            let m = camera_matrix_with(orbit, elevation, distance, aspect, Vec3::ZERO, projection);
            let at = |y: f32| {
                let c = m * Vec3::new(0.0, y, 0.0).extend(1.0);
                c.y / c.w
            };
            (at(1.0) - at(-1.0)).abs()
        };
        let p = height_at_target(Projection::Perspective);
        let o = height_at_target(Projection::Orthographic);
        assert!(
            (p - o).abs() < 1e-4,
            "at the orbit target the two projections must agree: {p} vs {o}"
        );
    }

    #[test]
    fn frame_all_uses_the_live_surface_aspect_not_a_constant() {
        // The regression the stress audit caught: resizing the window from 1600x850 to 900x900 left the
        // camera distance byte-identical, because `frame_all` framed against a compile-time constant.
        let mut wide = SceneState {
            surface_aspect: 1600.0 / 850.0,
            ..SceneState::default()
        };
        let mut tall = SceneState {
            surface_aspect: 900.0 / 900.0,
            ..SceneState::default()
        };
        // A subject WIDER than it is tall, viewed front-on so its width maps to the screen's horizontal
        // axis. A cube would prove nothing: for any aspect >= 1 the vertical axis limits, so a cube is
        // correctly framed at the same distance in a square window and a wide one — as an earlier
        // version of this test discovered the hard way.
        let unit = |x: f32| Instance {
            center: [x, 0.0, 0.0],
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
            selected: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0, 0.5, 0.0, 0.0],
        };
        for st in [&mut wide, &mut tall] {
            st.instances = vec![unit(-6.0), unit(6.0)];
            st.mesh_slots = vec![-1, -1];
            st.orbit = std::f32::consts::FRAC_PI_2; // front-on: world X becomes screen right
            st.elevation = 0.0;
            st.frame_all();
        }
        assert!(
            (wide.distance - tall.distance).abs() > 1e-3,
            "a square window and a wide one must not produce the same framing: {} vs {}",
            wide.distance,
            tall.distance
        );
        // The squarer window sees LESS horizontally, so it needs more distance for the same subject.
        assert!(tall.distance > wide.distance);
    }

    #[test]
    fn framing_fits_the_tighter_viewport_axis_not_always_the_vertical_one() {
        // A wide, flat scene — a lane, a turntable, an assembly. The old fit pushed the camera back by
        // the full aspect ratio because it measured the longest axis against the VERTICAL extent, and
        // left empty bands above and below. A wider window must never require MORE distance.
        // Viewed from above, so the 12 m spans the screen's horizontal axis and the 1 m the vertical.
        // A sphere fit reserves 12 m of VERTICAL frame for 1 m of content; the projected fit spends the
        // frame on the axis the content actually occupies.
        let half = Vec3::new(6.0, 0.5, 0.5);
        let (orbit, elevation) = (-std::f32::consts::FRAC_PI_2, 1.4);
        let projected = fit_distance(half, 16.0 / 9.0, orbit, elevation);
        let as_a_cube = fit_distance(Vec3::splat(half.length()), 16.0 / 9.0, orbit, elevation);
        assert!(
            projected < as_a_cube * 0.6,
            "a flat scene must not be framed as though it were a cube: {projected} vs {as_a_cube}"
        );
        // A wider lens sees more horizontally, so a horizontally-limited scene needs LESS distance.
        let narrow = fit_distance(half, 1.0, orbit, elevation);
        assert!(
            projected < narrow,
            "{projected} should be closer than {narrow}"
        );
    }

    #[test]
    fn framing_is_orientation_independent() {
        // The same box, turned. A fit built on the longest half-EDGE changes answer as the scene
        // rotates, which reads in the viewport as the subject breathing while you orbit.
        // A cube is the case where orientation genuinely must not change the answer.
        let a = fit_distance(Vec3::splat(2.0), 1.6, 0.3, 0.5);
        let b = fit_distance(
            Vec3::splat(2.0),
            1.6,
            0.3 + std::f32::consts::FRAC_PI_2,
            0.5,
        );
        assert!((a - b).abs() < 1e-3, "{a} != {b}");
    }

    #[test]
    fn framing_scales_linearly_with_the_subject() {
        // Tiny and enormous assets must both be framed, not clamped into a default. Ten times the
        // object is ten times the distance, so the subject occupies the same fraction of frame.
        // Both above the 0.3 distance floor, so this measures the fit and not the clamp.
        let small = fit_distance(Vec3::splat(0.5), 1.6, 0.3, 0.5);
        let large = fit_distance(Vec3::splat(5.0), 1.6, 0.3, 0.5);
        assert!(
            (large / small - 10.0).abs() < 0.01,
            "framing must be scale-invariant: {small} -> {large}"
        );
    }

    #[test]
    fn the_subject_actually_occupies_the_intended_fraction_of_the_frame() {
        // The claim the whole change rests on, checked rather than asserted: at the returned distance
        // the subject's angular radius is FRAME_OCCUPANCY of the tighter half-angle.
        // A cube viewed straight on: its screen half-height over the frame half-height.
        let half = Vec3::splat(1.0);
        let aspect = 16.0 / 9.0;
        let d = fit_distance(half, aspect, 0.0, 0.0);
        let half_v = (CAMERA_FOV_DEG.to_radians() * 0.5).clamp(0.01, 1.5);
        // Discount the near-face depth the fit added, then compare screen extent to frame extent.
        let occupancy = half.y / ((d - half.z) * half_v.tan());
        assert!(
            (occupancy - FRAME_OCCUPANCY).abs() < 0.02,
            "subject fills {occupancy} of the frame, wanted {FRAME_OCCUPANCY}"
        );
        // ...and that lands inside the professional 55-75% band the brief asks for.
        assert!((0.55..=0.75).contains(&occupancy));
    }

    #[test]
    fn framing_a_degenerate_or_empty_scene_cannot_produce_a_nonsense_camera() {
        // A single point, a zero extent, and a garbage aspect must all still yield a usable distance
        // rather than zero, NaN or infinity — any of which strands the camera with nothing on screen.
        for aspect in [0.0, -1.0, f32::NAN, f32::INFINITY, 1.6] {
            let d = fit_distance(Vec3::ZERO, aspect, 0.7, 0.4);
            assert!(d.is_finite() && d >= 0.3, "aspect {aspect} gave {d}");
        }
    }

    use super::*;

    #[test]
    fn cad_bounds_apply_authored_coordinates_and_instance_scale_once() {
        let instance = Instance {
            center: [1.0, 2.0, 3.0],
            scale: 0.001,
            color: [1.0; 3],
            selected: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0; 4],
        };
        let bounds = LocalBounds {
            center: Vec3::new(1_000.0, 0.0, 0.0),
            half_size: Vec3::new(500.0, 250.0, 125.0),
        };
        let (lo, hi) = instance_world_bounds(&instance, bounds);
        let center = (lo + hi) * 0.5;
        let size = hi - lo;
        assert!((center - Vec3::new(2.0, 2.0, 3.0)).length() < 1.0e-5);
        assert!((size - Vec3::new(1.0, 0.5, 0.25)).length() < 1.0e-5);
    }

    #[test]
    fn thumbnail_framing_preserves_millimetre_cad_scale_and_recenters_offset_mesh() {
        let instance = Instance {
            center: [75.0, -20.0, 9.0], // world placement must not leak into a portrait
            scale: 0.001,
            color: [0.7, 0.8, 0.9],
            selected: 1.0,
            rotation: IDENTITY_QUAT,
            material: [0.0; 4],
        };
        let bounds = LocalBounds {
            center: Vec3::new(1_000.0, 0.0, 0.0),
            half_size: Vec3::new(500.0, 250.0, 125.0),
        };
        let (framed, distance) = thumbnail_framing(&instance, bounds);
        assert_eq!(
            framed.scale, 0.001,
            "never replace a CAD scale with the old 0.1 floor"
        );
        assert!((Vec3::from(framed.center) - Vec3::new(-1.0, 0.0, 0.0)).length() < 1.0e-6);
        assert_eq!(
            framed.selected, 0.0,
            "portrait selection must not tint the source material"
        );
        assert!(
            distance > 1.0 && distance < 2.0,
            "real displayed bounds drive a tight portrait"
        );
    }

    #[test]
    fn lod_level_picks_coarser_with_distance_and_clamps() {
        let c = Some([0.0, 0.0, 0.0]);
        assert_eq!(lod_level([0.0, 0.0, 5.0], c, 2, 1.0), 0, "near → full");
        assert_eq!(lod_level([0.0, 0.0, 20.0], c, 2, 1.0), 1, "mid → LOD-1");
        assert_eq!(lod_level([0.0, 0.0, 50.0], c, 2, 1.0), 2, "far → LOD-2");
        assert_eq!(
            lod_level([0.0, 0.0, 50.0], c, 1, 1.0),
            1,
            "clamped to the available LODs"
        );
        assert_eq!(lod_level([0.0, 0.0, 50.0], c, 0, 1.0), 0, "no LODs → full");
        assert_eq!(
            lod_level([0.0, 0.0, 50.0], None, 2, 1.0),
            0,
            "no centroid → full"
        );
        assert_eq!(
            lod_level([0.0, 0.0, 50.0], c, 2, 10.0),
            0,
            "a ten-metre authored asset stays full-detail at the same camera distance"
        );
    }

    #[test]
    fn pbr_texture_mips_are_complete_and_semantic_aware() {
        let color_tex = Texture {
            width: 2,
            height: 1,
            rgba8: vec![0, 0, 0, 255, 255, 255, 255, 255],
        };
        let color = texture_mips(&color_tex, TextureSemantic::Color);
        let raw = texture_mips(&color_tex, TextureSemantic::Data);
        assert_eq!(
            color.iter().map(|(w, h, _)| (*w, *h)).collect::<Vec<_>>(),
            vec![(2, 1), (1, 1)]
        );
        assert!(
            color[1].2[0] > raw[1].2[0] + 50,
            "linear-light colour averaging avoids the dark 128 grey produced by raw byte averaging"
        );

        let odd = Texture {
            width: 3,
            height: 5,
            rgba8: vec![128; 3 * 5 * 4],
        };
        assert_eq!(
            texture_mips(&odd, TextureSemantic::Data)
                .iter()
                .map(|(w, h, _)| (*w, *h))
                .collect::<Vec<_>>(),
            vec![(3, 5), (2, 3), (1, 2), (1, 1)],
            "odd dimensions reduce with ceil-half until a complete 1x1 chain"
        );

        let normals = Texture {
            width: 2,
            height: 1,
            rgba8: vec![255, 128, 128, 255, 128, 255, 128, 255],
        };
        let pixel = &texture_mips(&normals, TextureSemantic::Normal)[1].2;
        let decoded = [0, 1, 2].map(|channel| f32::from(pixel[channel]) / 255.0 * 2.0 - 1.0);
        let length = decoded
            .iter()
            .map(|value| value * value)
            .sum::<f32>()
            .sqrt();
        assert!(
            (length - 1.0).abs() < 0.02,
            "normal-map mips stay normalized"
        );
    }

    #[test]
    fn pipe_preview_contains_route_segments_and_control_point_crosses() {
        let handles = [[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0]];
        let edges = [
            [handles[0], handles[1]],
            [handles[1], handles[2]],
            [handles[1], handles[3]],
        ];
        let v = pipe_graph_preview_vertices(&edges, &handles, 0.1);
        assert_eq!(v.len(), 3 * 2 + 4 * 6);
        assert_eq!(v[0].center, [0.0; 3]);
        assert_eq!(v[1].center, [1.0, 0.0, 0.0]);
        assert_eq!(v[4].center, [1.0, 0.0, 0.0]);
        assert_eq!(v[5].center, [2.0, 0.0, 0.0]);
        assert!(v.iter().all(|p| p.scale == 0.0));
    }

    /// A bare scene of `n` unit-scale cubes on a line — enough to exercise the focus state transition
    /// (no GPU; `focus_on`/`clear_focus` touch only plain fields).
    fn scene(n: usize) -> SceneState {
        let mut st = SceneState {
            distance: 60.0,
            ..Default::default()
        };
        for i in 0..n {
            st.instances.push(Instance {
                center: [i as f32 * 2.0, 1.0, 0.0],
                scale: 1.0,
                color: [0.5, 0.5, 0.5],
                selected: 0.0,
                rotation: IDENTITY_QUAT,
                material: [0.0; 4],
            });
            st.ids.push(format!("e{i}"));
        }
        st
    }

    #[test]
    fn focus_centers_zooms_selects_and_flags() {
        let mut st = scene(4);
        let rev0 = st.revision;
        st.focus_on(2);
        // Center: the orbit target is the focused entity's position.
        assert_eq!(st.cam_target, [4.0, 1.0, 0.0]);
        // Get nearby: zoomed in from 60 → half_extent(max(1.0, 0.02))*4 clamped to [0.15, 40] = 4.
        // (The floor is cm-scale, 0.15 m — the old 6 m floor parked a cm-scale CAD part sub-pixel; M15.9.)
        assert_eq!(st.distance, 4.0);
        assert!(st.distance < 60.0, "focus must zoom IN (get nearby)");
        // Selected + focused are the same entity; the shader keeps it lit while dimming the rest.
        assert_eq!(st.selected, Some(2));
        assert_eq!(st.focused, Some(2));
        assert_eq!(st.instances[2].selected, 1.0);
        // The framing was saved for restore, and the revision bumped so the new flags upload.
        assert_eq!(st.pre_focus_distance, Some(60.0));
        assert_ne!(st.revision, rev0);
    }

    #[test]
    fn unfocus_restores_everything_to_normal() {
        let mut st = scene(4);
        st.focus_on(1);
        assert!(st.focused.is_some() && st.distance < 60.0);
        st.clear_focus();
        // "Everything comes back to normal": dim flag cleared + the saved distance restored.
        assert_eq!(st.focused, None);
        assert_eq!(st.distance, 60.0);
        assert_eq!(st.pre_focus_distance, None);
        // Selection is intentionally retained (only the dim + zoom revert).
        assert_eq!(st.selected, Some(1));
    }

    #[test]
    fn refocusing_keeps_the_original_framing_then_restores_it() {
        let mut st = scene(4);
        st.focus_on(0); // saves 60.0
        st.focus_on(3); // must NOT overwrite the saved framing with the zoomed-in 6.0
        assert_eq!(st.pre_focus_distance, Some(60.0));
        assert_eq!(st.cam_target, [6.0, 1.0, 0.0]); // re-centered on the new entity
        st.clear_focus();
        assert_eq!(st.distance, 60.0); // back to the true original, not the intermediate focus distance
    }

    #[test]
    fn clear_focus_is_a_noop_when_not_focused() {
        let mut st = scene(2);
        let rev0 = st.revision;
        st.clear_focus(); // a stray Escape with nothing focused
        assert_eq!(st.focused, None);
        assert_eq!(st.revision, rev0, "no revision bump when nothing changed");
    }

    #[test]
    fn focus_on_out_of_range_is_ignored() {
        let mut st = scene(2);
        st.focus_on(9);
        assert_eq!(st.focused, None);
        assert_eq!(st.selected, None);
    }
}
