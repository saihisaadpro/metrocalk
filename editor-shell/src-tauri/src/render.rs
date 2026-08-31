//! The native wgpu viewport — M2.2's instanced render path on the Tauri window surface (ADR-008
//! single-window: this surface is OS-composited *under* the transparent WebView2). Renders the live
//! `/core` scene: one instanced cube per entity (from its `Transform`) + a ground grid, depth-tested,
//! with an orbiting camera. Instancing is the M2.2 technique that holds the frame budget; the GPU
//! frustum-cull→indirect refinement is also proven in `spikes/render-scene` and ports in on top.
//!
//! The render loop owns no scene truth — it reads a shared [`SceneState`] the app updates from the
//! authoritative core (deltas). Hot interaction stays in Rust (invariant 4): camera orbit/zoom update
//! natively in the loop (zero per-frame IPC), and picking is a real raycast (see `scene_pick`) run
//! synchronously inside the `viewport_pick` command — neither crosses the JS boundary per frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::diag_log;
use glam::{Mat4, Quat, Vec3, Vec4};
use metrocalk_assets::colour::TextureRole as Role;
use metrocalk_assets::{MeshGpu, MeshVertex, Texture};
use metrocalk_editor_shell::reveal::intent_order;
use metrocalk_gizmo::{Gizmo, TransformGizmo};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// M9.4 — the magnetic-snap radius (world units): during a gizmo drag the dragged instance snaps onto the
/// nearest meaningful target within this range (the live "magnetic intent snapping").
/// Terrain instance slots the shared storage buffer holds. Fixed, never reallocated: a chunk's bind group
/// references this buffer, so growing it would invalidate every chunk's binding at once. 4 096 chunks is far
/// beyond what any budget lets be resident, and the buffer costs 256 kB.
pub const TERRAIN_INSTANCE_CAPACITY: u64 = 4096;

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
    /// What the viewport is saying about this instance right now — a small integer CODE, not a
    /// boolean: bit 0 ([`HIGHLIGHT_SELECTED`]) is the committed selection, bit 1
    /// ([`HIGHLIGHT_HOVERED`]) is what the cursor is over. They are independent facts and an object
    /// is routinely both, which is exactly why one flag could not carry them: a hover that wrote
    /// `1.0` would report a selection the selection model does not have, and clearing it would
    /// silently deselect.
    ///
    /// A code rather than a second field because this is an `array<Instance>` STRIDE — a 15 711-part
    /// import re-uploads 1 MB of it whenever the highlight changes, and growing the struct to carry
    /// one more bit would make that 1.25 MB on every hover.
    ///
    /// The particle pass reuses this lane as OPACITY, the same way it reuses `color` as HDR radiance
    /// and `scale` as a radius — `Instance` is a carrier there, not an entity.
    pub highlight: f32,
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

/// [`Instance::highlight`] bit 0 — this instance is in the committed selection.
pub const HIGHLIGHT_SELECTED: u32 = 1;
/// [`Instance::highlight`] bit 1 — the cursor is over this instance, or over an assembly it belongs
/// to. Transient: a render projection of a hover, never document state, and it survives a rebuild no
/// longer than the hover does.
pub const HIGHLIGHT_HOVERED: u32 = 2;

/// Read one bit out of an [`Instance::highlight`] code.
#[must_use]
pub fn highlight_has(code: f32, bit: u32) -> bool {
    (highlight_code(code) & bit) != 0
}

/// The code as an integer. Written from these constants and read back through `+ 0.5` rounding, so
/// the float never has to be compared for equality.
#[must_use]
pub fn highlight_code(code: f32) -> u32 {
    if code <= 0.0 {
        return 0;
    }
    (code + 0.5) as u32
}

/// The code with `bit` set or cleared, leaving every other bit alone. This is the whole reason the
/// field is a code: the selection writes bit 0 and a hover writes bit 1, and neither may erase the
/// other's answer.
#[must_use]
pub fn highlight_with(code: f32, bit: u32, on: bool) -> f32 {
    let next = if on {
        highlight_code(code) | bit
    } else {
        highlight_code(code) & !bit
    };
    next as f32
}

/// The identity quaternion (no rotation) — the default for `Instance::rotation`.
pub const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Authored-space bounds shared by camera framing, focus, LOD, and thumbnail portraits. Imported CAD is
/// intentionally not normalized (millimetre vertices commonly render with a 0.001 instance scale), so any
/// path that guesses size from `Instance::scale` alone will either clip it or park the camera far away.
#[derive(Clone, Copy, Debug)]
pub struct LocalBounds {
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
            .and_then(|slot| self.local_bounds_for_slot(slot))
            .map_or(0.5, |bounds| bounds.half_size[1])
    }

    /// One mesh slot's local bounds, from the memo when it is current and by walking the vertices when
    /// it is not.
    ///
    /// [`local_mesh_bounds`] reads EVERY VERTEX of the mesh. That is fine once; it is not fine per
    /// instance, and [`Self::rendered_instance_bounds`] -- the seam the cinematic camera samples its
    /// subject through -- called it exactly that way. Framing the imported factory meant ~15,711
    /// instances each re-walking their shared mesh's vertices, twice a tick at 60 Hz: hundreds of
    /// millions of vertex reads per frame for an answer that only changes when the meshes do. The engine
    /// thread stopped draining its command queue and the whole editor stopped answering.
    ///
    /// `scene_world_bounds` had already learned this and builds the same table per call; this makes it
    /// the state's, so every framing path shares one. The fallback is deliberate: a stale memo is SLOW,
    /// never wrong.
    #[must_use]
    fn local_bounds_for_slot(&self, slot: usize) -> Option<LocalBounds> {
        if self.mesh_bounds_are_current() {
            return self.mesh_bounds.get(slot).copied().flatten();
        }
        self.meshes.get(slot).and_then(local_mesh_bounds)
    }

    fn mesh_bounds_are_current(&self) -> bool {
        self.mesh_bounds_revision == self.meshes_revision
            && self.mesh_bounds.len() == self.meshes.len()
    }

    /// The instance index for an entity key, through the memo when it is current and by scanning when it
    /// is not. A stale memo is slow, never wrong.
    #[must_use]
    pub fn index_of(&self, key: &str) -> Option<usize> {
        if self.index_of_id_revision == self.ids_revision
            && self.index_of_id.len() == self.ids.len()
        {
            return self.index_of_id.get(key).copied();
        }
        self.ids.iter().position(|candidate| candidate == key)
    }

    /// Rebuild the key -> index memo if the instance set has moved on.
    pub fn sync_index_of_id(&mut self) {
        if self.index_of_id_revision == self.ids_revision
            && self.index_of_id.len() == self.ids.len()
        {
            return;
        }
        self.index_of_id = self
            .ids
            .iter()
            .enumerate()
            .map(|(index, key)| (key.clone(), index))
            .collect();
        self.index_of_id_revision = self.ids_revision;
    }

    /// Rebuild the per-slot bounds memo if the mesh table has moved on. Cheap and idempotent when it
    /// has not, so callers may run it every frame.
    pub fn sync_mesh_bounds(&mut self) {
        if self.mesh_bounds_are_current() {
            return;
        }
        self.mesh_bounds = self.meshes.iter().map(local_mesh_bounds).collect();
        self.mesh_bounds_revision = self.meshes_revision;
    }

    /// The exact world-space AABB of the rendered instance at `index`.
    ///
    /// This is the public projection seam for features that must frame what the viewport actually draws
    /// (cinematics, selection summaries, and future camera tools). It deliberately resolves the instance's
    /// real mesh slot, local vertex bounds, uniform display scale, and quaternion rotation through the same
    /// helper used by frame-all/focus. A missing mesh keeps the renderer's established unit-cube fallback;
    /// an invalid index or a transform that cannot produce finite geometry returns `None` rather than
    /// poisoning a camera with infinities.
    #[must_use]
    pub fn rendered_instance_bounds(&self, index: usize) -> Option<([f32; 3], [f32; 3])> {
        let instance = self.instances.get(index)?;
        let local_bounds = self
            .mesh_slots
            .get(index)
            .and_then(|slot| usize::try_from(*slot).ok())
            .and_then(|slot| self.local_bounds_for_slot(slot))
            .unwrap_or(LocalBounds::UNIT_CUBE);
        let (lo, hi) = instance_world_bounds(instance, local_bounds);
        (lo.is_finite() && hi.is_finite() && lo.cmple(hi).all())
            .then(|| (lo.to_array(), hi.to_array()))
    }

    /// Build the cinematic occlusion broad phase if the instance set has moved on. Cheap and idempotent
    /// when it has not, so the cutscene loop may call it whenever it plans a shot.
    ///
    /// Nothing else in the engine triggers this: the structure exists for one caller, and a scene that
    /// is never filmed never builds it.
    /// Size the presentation room to whatever is currently in the scene.
    ///
    /// Memoised against the revisions that can change the answer, and called from both the draw pass
    /// and the camera planner so those two can never be looking at different rooms. Cheap enough to
    /// call every frame; the memo exists because the bounds walk behind it is not.
    pub fn sync_stage(&mut self) {
        let key = (self.ids_revision, self.meshes_revision);
        if self.hall_revision == Some(key) {
            return;
        }
        self.hall_revision = Some(key);
        self.hall = match self.presentation_set {
            PresentationSet::Studio => None,
            PresentationSet::FactoryHall => {
                scene_world_bounds(&self.instances, &self.mesh_slots, &self.meshes).and_then(
                    |(lo, hi)| {
                        crate::hall::Hall::around(lo.to_array(), hi.to_array(), GROUND_PLANE_Y)
                    },
                )
            }
        };
    }

    /// Forget the sized room, so the next [`Self::sync_stage`] rebuilds it.
    ///
    /// Needed because the memo is keyed on what is IN the scene, and switching the set changes the
    /// answer without changing the scene — the one case the key cannot see.
    pub fn invalidate_stage(&mut self) {
        self.hall_revision = None;
    }

    /// Where a camera may stand, for the shot solver.
    ///
    /// The margin is applied here, once, rather than by the solver: the room knows how thick its own
    /// walls are and how much clearance a lens needs off them, and a solver with a second opinion about
    /// that would be a second place the number is decided.
    #[must_use]
    pub fn camera_stage(&self) -> metrocalk_animation::shot::Stage {
        metrocalk_animation::shot::Stage {
            room: self.hall.map(crate::hall::Hall::camera_room),
        }
    }

    pub fn sync_occlusion(&mut self) {
        // The planner asks about the room in the same breath as it asks about obstruction, and a
        // vantage judged against a room that has not been sized yet reports the void the scene used to
        // be. Sized here, so no caller has to remember to.
        self.sync_stage();
        if self.occlusion_revision == Some(self.ids_revision) {
            return;
        }
        self.sync_mesh_bounds();
        let bounds: Vec<metrocalk_spatial::Aabb> = (0..self.instances.len())
            .map(|index| {
                self.rendered_instance_bounds(index).map_or(
                    metrocalk_spatial::Aabb::EMPTY,
                    |(lo, hi)| {
                        metrocalk_spatial::Aabb::new(
                            [f64::from(lo[0]), f64::from(lo[1]), f64::from(lo[2])],
                            [f64::from(hi[0]), f64::from(hi[1]), f64::from(hi[2])],
                        )
                    },
                )
            })
            .collect();
        self.occlusion = metrocalk_spatial::SceneBvh::build(&bounds);
        self.occlusion_revision = Some(self.ids_revision);
    }

    /// What the world has to say about one candidate camera placement — the answer
    /// `metrocalk_animation::shot::plan_shot` negotiates against.
    ///
    /// `subject` is the set of instance indices the shot is ABOUT; they are excluded from every
    /// obstruction test, because a camera close enough to fill the frame with its subject is by
    /// definition close to it, and the pure solver already guarantees it stays outside it.
    ///
    /// Tested against world bounding boxes rather than triangles. That is a deliberate
    /// over-approximation: it is one BVH walk instead of a mesh raycast per candidate, it can only ever
    /// report a view as *more* blocked than it is, and the planner's acceptance threshold is set below
    /// 1.0 precisely so a handrail's generous box does not veto a shot a viewer would call clear.
    #[must_use]
    pub fn vantage(
        &self,
        eye: [f32; 3],
        look_at: [f32; 3],
        subject_center: [f32; 3],
        subject_radius: f32,
        subject: &std::collections::HashSet<u32>,
    ) -> metrocalk_animation::shot::Vantage {
        use metrocalk_animation::shot::Vantage;
        if self.occlusion.is_empty() {
            return Vantage::OPEN;
        }
        let eye64 = [f64::from(eye[0]), f64::from(eye[1]), f64::from(eye[2])];
        let centre64 = [
            f64::from(subject_center[0]),
            f64::from(subject_center[1]),
            f64::from(subject_center[2]),
        ];
        let radius = f64::from(subject_radius.abs().max(1.0e-3));
        let range = {
            let d = [
                centre64[0] - eye64[0],
                centre64[1] - eye64[1],
                centre64[2] - eye64[2],
            ];
            (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
        };
        if !range.is_finite() || range <= 1.0e-6 {
            return Vantage::OPEN;
        }
        let mut scratch = Vec::new();

        // ── Is the camera buried? ──────────────────────────────────────────────────────────────
        // A box the size of the camera's own near clip, so "inside" means what the viewer sees: the
        // solid-colour frame you get when a wall is closer than the near plane.
        let probe = (range * 0.01).clamp(0.02, 0.35);
        let cell = metrocalk_spatial::Aabb::new(
            [eye64[0] - probe, eye64[1] - probe, eye64[2] - probe],
            [eye64[0] + probe, eye64[1] + probe, eye64[2] + probe],
        );
        let mut inside_keys = Vec::new();
        self.occlusion
            .query_bounds(&cell, &mut scratch, &mut inside_keys);
        let eye_inside = inside_keys.iter().any(|key| !subject.contains(key));

        // ── Can it see the subject? ────────────────────────────────────────────────────────────
        // Rays at the centre and at six points on the subject's bounding sphere, so a subject half
        // behind a column reads as half blocked rather than as a binary yes.
        let (right, up) = frame_basis(eye64, centre64);
        let spread = radius * 0.6;
        let mut targets = Vec::with_capacity(7);
        targets.push(centre64);
        for (rx, uy) in [
            (1.0_f64, 0.0_f64),
            (-1.0, 0.0),
            (0.0, 1.0),
            (0.0, -1.0),
            (0.7, 0.7),
            (-0.7, -0.7),
        ] {
            targets.push([
                centre64[0] + (right[0] * rx + up[0] * uy) * spread,
                centre64[1] + (right[1] * rx + up[1] * uy) * spread,
                centre64[2] + (right[2] * rx + up[2] * uy) * spread,
            ]);
        }
        let mut reached = 0usize;
        for target in &targets {
            let d = [
                target[0] - eye64[0],
                target[1] - eye64[1],
                target[2] - eye64[2],
            ];
            let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            if !length.is_finite() || length <= 1.0e-6 {
                reached += 1;
                continue;
            }
            // Stop short of the target: the subject's own neighbours count, the subject's own skin
            // does not, and a ray that runs all the way in would be blocked by whatever it lands on.
            let t_max = length * 0.98;
            let ray = metrocalk_spatial::ray::Ray::new(eye64, d);
            if !self
                .occlusion
                .ray_hits(&ray, t_max, &mut scratch, |key| !subject.contains(&key))
            {
                reached += 1;
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let clear = reached as f32 / targets.len() as f32;

        // ── What fills the frame, in front of the subject and behind it ────────────────────────
        // Nine directions spread across the frame answer both remaining questions at once.
        let (look_right, look_up) = frame_basis(
            eye64,
            [
                f64::from(look_at[0]),
                f64::from(look_at[1]),
                f64::from(look_at[2]),
            ],
        );
        let forward = [
            (centre64[0] - eye64[0]) / range,
            (centre64[1] - eye64[1]) / range,
            (centre64[2] - eye64[2]) / range,
        ];
        // Anything not the subject, this much nearer than the subject, is foreground rather than context.
        let crowding_reach = range * 0.35;
        // Backing is measured from the SUBJECT outwards, never from the lens. Measured from the lens, a
        // wall twenty centimetres in front of the camera counts as a rich backdrop, and the frames that
        // are nothing but that wall score as the best-composed in the film -- which is exactly what the
        // second film's numbers said.
        let backdrop = range * 5.0;
        // The presentation ground is a quad, not an instance, so it is absent from the BVH; a downward
        // shot would otherwise report a void it is in fact aimed straight at.
        let ground_y = f64::from(GROUND_PLANE_Y);
        let mut backed = 0usize;
        let mut crowded_probes = 0usize;
        let mut probes = 0usize;
        for rx in [-0.55_f64, 0.0, 0.55] {
            for uy in [-0.4_f64, 0.0, 0.4] {
                probes += 1;
                let dir = [
                    forward[0] + look_right[0] * rx + look_up[0] * uy,
                    forward[1] + look_right[1] * rx + look_up[1] * uy,
                    forward[2] + look_right[2] * rx + look_up[2] * uy,
                ];
                let ray = metrocalk_spatial::ray::Ray::new(eye64, dir);
                if self
                    .occlusion
                    .ray_hits(&ray, crowding_reach, &mut scratch, |key| {
                        !subject.contains(&key)
                    })
                {
                    crowded_probes += 1;
                }
                // Start the backdrop ray at the subject's own plane, so only what lies BEYOND the
                // subject can back it. The subject is deliberately not excluded here: a shot of a whole
                // assembly is legitimately backed by its own far side.
                let beyond = [
                    eye64[0] + ray.direction[0] * range,
                    eye64[1] + ray.direction[1] * range,
                    eye64[2] + ray.direction[2] * range,
                ];
                let behind = metrocalk_spatial::ray::Ray::new(beyond, ray.direction);
                if self
                    .occlusion
                    .ray_hits(&behind, backdrop, &mut scratch, |_| true)
                {
                    backed += 1;
                    continue;
                }
                // The presentation ROOM, for the same reason as the ground below it: the hall's slab,
                // walls, columns and roof are drawn directly rather than published as instances, so
                // they are absent from the BVH. Without this the planner keeps reporting the void the
                // scene used to be — and the delivered films measured `backing` at 1/9 on every wide
                // shot of the assembly for exactly that reason, while the frame the viewer would have
                // seen was a wall.
                if let Some(hall) = self.hall {
                    if hall.shell_within(beyond, behind.direction, backdrop) {
                        backed += 1;
                        continue;
                    }
                }
                // Falling toward the floor within the same reach counts as content.
                let dy = behind.direction[1];
                if dy < -1.0e-6 {
                    let t = (ground_y - beyond[1]) / dy;
                    if t > 0.0 && t <= backdrop {
                        backed += 1;
                    }
                }
            }
        }
        #[allow(clippy::cast_precision_loss)]
        let (backing, crowded) = if probes == 0 {
            (0.0, 0.0)
        } else {
            (
                backed as f32 / probes as f32,
                crowded_probes as f32 / probes as f32,
            )
        };

        Vantage {
            eye_inside,
            clear,
            backing,
            crowded,
        }
    }
}

/// Where the presentation ground sits. Slightly under the origin so a model resting exactly on `y = 0`
/// does not z-fight with the floor it is standing on. Named because the cinematic camera's backdrop test
/// has to know the floor is there: the ground is a quad the renderer draws directly, not an instance, so
/// it is invisible to any broad phase built over the instance list.
pub const GROUND_PLANE_Y: f32 = -0.02;

/// One shot's negotiated camera placement — what was directed, and what was actually filmed.
///
/// A plain record rather than a serialisable type: this module has no serde dependency and does not need
/// one, and the command that reports it already builds JSON by hand.
#[derive(Clone, Debug, PartialEq)]
pub struct CinematicPlacement {
    /// The shot's stable id.
    pub shot: String,
    /// The entity the shot frames.
    pub subject: String,
    /// The framing as authored.
    pub directed_size: &'static str,
    /// The framing as filmed. Different from `directed_size` means the close placement was buried and
    /// the shot had to step back to find air.
    pub filmed_size: &'static str,
    /// Degrees the camera was swung around the subject to find a clear, backed view. Zero means none.
    pub yaw_offset_deg: f32,
    /// How many placements were rejected before this one. Zero means the direction was filmed as written.
    pub rejected: u8,
    /// WHAT THE ORACLE SAW at the placement it settled on, mid-shot.
    ///
    /// Recorded because the first film measured with a sound legibility metric disagreed with it flatly:
    /// fifteen frames were a picture of nothing, and every shot that produced them reported
    /// `rejected: 0` -- the negotiation had looked at those placements and called them acceptable. A
    /// record that says only "filmed as directed" cannot distinguish an oracle that was never consulted
    /// from one that was consulted and was wrong, and those need opposite fixes.
    pub vantage: metrocalk_animation::shot::Vantage,
}

/// A right/up pair for the plane facing `target` from `eye`, used to spread sample rays across a frame.
///
/// World up is the reference, with a sideways fallback for the degenerate case of looking straight down —
/// a bird's-eye shot is exactly the one that would otherwise produce a zero-length cross product and
/// silently collapse every sample ray onto the same direction.
fn frame_basis(eye: [f64; 3], target: [f64; 3]) -> ([f64; 3], [f64; 3]) {
    let cross = |a: [f64; 3], b: [f64; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let norm = |v: [f64; 3]| {
        let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if len.is_finite() && len > 1.0e-9 {
            [v[0] / len, v[1] / len, v[2] / len]
        } else {
            [0.0, 0.0, 0.0]
        }
    };
    let forward = norm([target[0] - eye[0], target[1] - eye[1], target[2] - eye[2]]);
    let mut right = norm(cross(forward, [0.0, 1.0, 0.0]));
    if right == [0.0, 0.0, 0.0] {
        right = norm(cross(forward, [0.0, 0.0, 1.0]));
    }
    if right == [0.0, 0.0, 0.0] {
        return ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
    }
    (right, norm(cross(right, forward)))
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

/// The renderer's single visibility policy for viewport presentation chrome.
///
/// Scene geometry and the ground/shadow receiver remain visible in both modes. Every editor-only helper
/// pass is named here so cinematic playback cannot accidentally inherit a newly scattered one-off check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewportVisibility {
    Editor,
    Cinematic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ViewportLayer {
    GroundShadowReceiver,
    Grid,
    TrackingLines,
    MarkerGlyphs,
    PhysicsDebugOverlay,
    TerrainToolOverlay,
    GizmoAndSnapChrome,
}

impl ViewportVisibility {
    const fn from_cinematic(cinematic: bool) -> Self {
        if cinematic {
            Self::Cinematic
        } else {
            Self::Editor
        }
    }

    const fn allows(self, layer: ViewportLayer) -> bool {
        match layer {
            ViewportLayer::GroundShadowReceiver => true,
            ViewportLayer::Grid
            | ViewportLayer::TrackingLines
            | ViewportLayer::MarkerGlyphs
            | ViewportLayer::PhysicsDebugOverlay
            | ViewportLayer::TerrainToolOverlay
            | ViewportLayer::GizmoAndSnapChrome => matches!(self, Self::Editor),
        }
    }
}

/// Viewport exposure. Placed so the lit scene lands near the tone curve's mid-grey rather than a stop
/// and a half above it. Named once because a second, independent default in the thumbnail path had
/// already drifted from the viewport's.
pub const DEFAULT_EXPOSURE: f32 = 0.45;

/// The aspect assumed when framing happens before a real surface size is known. 16:9 rather than 1:1 so
/// the pre-surface guess errs toward the shape of a real editor viewport.
pub const DEFAULT_ASPECT: f32 = 16.0 / 9.0;

/// The whole surface: the composition frame of a viewport with nothing drawn over it. What a
/// thumbnail, a bench or an offscreen render composes for, because those really do own every pixel.
pub const FULL_FRAME: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A composition rectangle that can be trusted, in surface fractions `[x, y, width, height]` measured
/// from the top-left.
///
/// A missing, degenerate or out-of-range rectangle collapses to the whole surface, which is exactly the
/// behaviour before any rectangle was reported. A bad rectangle must never be able to shear the
/// projection or fling the camera somewhere the subject is not.
#[must_use]
pub fn sane_frame(frame: [f32; 4]) -> [f32; 4] {
    let [x, y, w, h] = frame;
    let ok = frame.iter().all(|v| v.is_finite())
        && w > 0.02
        && h > 0.02
        && w <= 1.001
        && h <= 1.001
        && x >= -0.001
        && y >= -0.001
        && x + w <= 1.001
        && y + h <= 1.001;
    if ok {
        [x.max(0.0), y.max(0.0), w.min(1.0), h.min(1.0)]
    } else {
        FULL_FRAME
    }
}

/// The aspect ratio of the composed rectangle ITSELF - the shape of the picture, not of the window it
/// is cut out of.
#[must_use]
pub fn frame_aspect(surface_aspect: f32, frame: [f32; 4]) -> f32 {
    let [_, _, w, h] = sane_frame(frame);
    let surface = if surface_aspect.is_finite() && surface_aspect > 0.01 {
        surface_aspect
    } else {
        DEFAULT_ASPECT
    };
    (surface * w / h).clamp(0.05, 20.0)
}

/// Inset `rect` until its own aspect ratio is `want`, centred - the bars of a delivery frame.
///
/// Pillarbox when the rectangle is wider than the frame it delivers, letterbox when it is taller. This
/// is the ONE place the bars are decided: the engine composes for the result and hands the same
/// rectangle to the UI to draw the mask from, so the picture and the bars around it cannot disagree.
#[must_use]
pub fn inset_to_aspect(rect: [f32; 4], surface_aspect: f32, want: f32) -> [f32; 4] {
    let [x, y, w, h] = sane_frame(rect);
    if !want.is_finite() {
        return [x, y, w, h];
    }
    let want = want.clamp(0.05, 20.0);
    let have = frame_aspect(surface_aspect, [x, y, w, h]);
    if (have - want).abs() <= 1.0e-4 {
        [x, y, w, h]
    } else if have > want {
        let inner = w * want / have;
        [(w - inner).mul_add(0.5, x), y, inner, h]
    } else {
        let inner = h * have / want;
        [x, (h - inner).mul_add(0.5, y), w, inner]
    }
}

/// The shape and placement of the rectangle a picture is composed for.
///
/// Two numbers that only mean anything together: a sub-rectangle without the surface it sits on has no
/// aspect ratio, and a surface aspect without the sub-rectangle is the shape of a window rather than of
/// a picture. They travel as one value so no caller can pass half of the answer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewFrame {
    surface_aspect: f32,
    rect: [f32; 4],
}

impl ViewFrame {
    /// A frame composed for `rect` of a surface of `surface_aspect`. The rectangle is sanitised here,
    /// once, so an unusable `ViewFrame` cannot exist.
    #[must_use]
    pub fn new(surface_aspect: f32, rect: [f32; 4]) -> Self {
        Self {
            surface_aspect,
            rect: sane_frame(rect),
        }
    }

    /// A frame that owns its whole surface.
    #[must_use]
    pub fn whole(surface_aspect: f32) -> Self {
        Self::new(surface_aspect, FULL_FRAME)
    }

    /// The composed rectangle, in surface fractions.
    #[must_use]
    pub fn rect(self) -> [f32; 4] {
        self.rect
    }

    /// The composed rectangle's own aspect ratio.
    #[must_use]
    pub fn aspect(self) -> f32 {
        frame_aspect(self.surface_aspect, self.rect)
    }
}

/// A composed frame as a pixel rectangle `[x, y, width, height]` on a `w` x `h` surface, clamped so it
/// always names at least one pixel inside the surface. What a scissor needs.
#[must_use]
pub fn frame_pixels(frame: [f32; 4], w: u32, h: u32) -> [u32; 4] {
    let [fx, fy, fw, fh] = sane_frame(frame);
    let (sw, sh) = (w.max(1), h.max(1));
    let px = |v: f32, span: u32| (v * span as f32).round().clamp(0.0, span as f32) as u32;
    let x = px(fx, sw).min(sw - 1);
    let y = px(fy, sh).min(sh - 1);
    [
        x,
        y,
        px(fw, sw).clamp(1, sw - x),
        px(fh, sh).clamp(1, sh - y),
    ]
}

/// Expand a frame's own symmetric half-extents into the SURFACE extents that place them inside it.
/// Returns `(left, right, bottom, top)` in the plane the half-extents were measured at.
fn surface_extents(frame: [f32; 4], half_w: f32, half_h: f32) -> (f32, f32, f32, f32) {
    let [x, y, w, h] = sane_frame(frame);
    let span_x = 2.0 * half_w / w;
    let span_y = 2.0 * half_h / h;
    // NDC y points up, so it is the frame's distance from the BOTTOM that offsets the vertical extents.
    let left = (-x).mul_add(span_x, -half_w);
    let bottom = -(1.0 - y - h).mul_add(span_y, half_h);
    (left, left + span_x, bottom, bottom + span_y)
}

/// A right-handed off-centre perspective projection with wgpu's `[0, 1]` depth - the same matrix
/// `Mat4::perspective_rh` builds when `l = -r` and `b = -t`, with the symmetry dropped.
fn perspective_off_centre_rh(l: f32, r: f32, b: f32, t: f32, near: f32, far: f32) -> Mat4 {
    let rl = (r - l).max(1.0e-6);
    let tb = (t - b).max(1.0e-6);
    let depth = far / (near - far);
    Mat4::from_cols(
        Vec4::new(2.0 * near / rl, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * near / tb, 0.0, 0.0),
        Vec4::new((r + l) / rl, (t + b) / tb, depth, -1.0),
        Vec4::new(0.0, 0.0, near * depth, 0.0),
    )
}

/// The projection that draws, INSIDE `frame`, exactly the picture a camera owning a surface of that
/// frame's shape would draw.
///
/// WHY THE PROJECTION AND NOT THE CAMERA TARGET. The wgpu surface is the whole window; the editor is
/// composited over it and the 3D shows through a transparent hole. The first answer to that was to
/// slide the ORBIT TARGET sideways at framing time by the hole's offset, which put the subject in the
/// hole for exactly one instant. That offset is proportional to `distance` and expressed in the
/// camera's own right/up basis, so zooming, orbiting, opening a dock and resizing the window each left
/// a target that had been correct for a frame which no longer existed - and nothing re-derived it,
/// because framing is a user action and none of those are. Shearing the frustum instead is not a
/// correction applied once: it IS the frame, every tick, for nothing.
#[must_use]
pub fn framed_projection(
    projection: Projection,
    fov_deg: f32,
    frame: ViewFrame,
    distance: f32,
    near: f32,
    far: f32,
) -> Mat4 {
    let half_v = (fov_deg.to_radians() * 0.5).clamp(0.01, 1.5).tan();
    let aspect = frame.aspect();
    match projection {
        Projection::Perspective => {
            let half_h = near * half_v;
            let (l, r, b, t) = surface_extents(frame.rect(), half_h * aspect, half_h);
            perspective_off_centre_rh(l, r, b, t, near, far)
        }
        Projection::Orthographic => {
            let half_h = distance * half_v;
            let (l, r, b, t) = surface_extents(frame.rect(), half_h * aspect, half_h);
            // The near plane goes BEHIND the eye. A parallel projection has no apex to clip against, and
            // starting at the eye would slice away everything between the camera and its orbit target -
            // which in a top view is most of the scene.
            Mat4::orthographic_rh(l, r, b, t, -far, far)
        }
    }
}

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
/// The camera's viewing direction for an orbit/elevation pair (unit, pointing at the target).
fn camera_forward(orbit: f32, elevation: f32) -> Vec3 {
    -Vec3::new(
        orbit.cos() * elevation.cos(),
        elevation.sin(),
        orbit.sin() * elevation.cos(),
    )
    .normalize_or_zero()
}

/// The camera's `(right, up)` basis for an orbit/elevation pair. Shared by the fit and by the framing
/// offset, so a subject is measured and then placed against the same axes.
fn camera_right_up(orbit: f32, elevation: f32) -> (Vec3, Vec3) {
    let forward = camera_forward(orbit, elevation);
    // Looking near-straight down, world +Y is degenerate as an up reference.
    let world_up = if forward.y.abs() > 0.99 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    let right = forward.cross(world_up).normalize_or_zero();
    (right, right.cross(forward).normalize_or_zero())
}

/// `aspect` is the shape of the FRAME the picture is composed for, not of the window it is cut out of.
///
/// This used to take a second pair of `visible` fractions and divide its tangents by them, because the
/// projection spanned the whole window and the frame only saw a share of it. That knob is gone:
/// [`framed_projection`] gives the frame its own frustum, so it really does span what it is fitted
/// against, and a second compensation here would apply the same correction twice.
#[must_use]
pub fn fit_distance_in_viewport(half_extent: Vec3, aspect: f32, orbit: f32, elevation: f32) -> f32 {
    let aspect = if aspect.is_finite() && aspect > 0.01 {
        aspect
    } else {
        DEFAULT_ASPECT
    };
    let half_v = (CAMERA_FOV_DEG.to_radians() * 0.5).clamp(0.01, 1.5);
    let half_h = (half_v.tan() * aspect).atan().max(0.01);

    // The camera basis this distance will be used with, so the fit matches the view it frames.
    let (right, up) = camera_right_up(orbit, elevation);
    let forward = camera_forward(orbit, elevation);

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

/// Where the ground receiver must sit, and how big, to stand under `bounds` without ending inside frame.
///
/// The receiver used to be a hard-coded scale-60 quad uploaded once at startup and never touched again.
/// Every imported model was therefore judged against a fixed 120-unit plane: anything larger hung off the
/// edge into empty space — a weld line whose far end floats over nothing — and anything smaller sat on a
/// slab that dominated the shot. Neither is a lighting problem, and no amount of shading fixes either.
///
/// Returned as `(centre, scale)` for the unit quad, sized generously past the model so that in ordinary
/// framing the edge falls outside the frame rather than drawing a visible horizon across it.
fn ground_placement(bounds: Option<(Vec3, Vec3)>) -> ([f32; 3], f32) {
    const MIN_SCALE: f32 = 60.0;
    // Enough that the edge falls outside ordinary framing, not so much that the model is left adrift on a
    // grey plain. At 3.0 the plane came out six times the model's width and the horizon owned the shot.
    const MARGIN: f32 = 1.6;
    // A ceiling on the quad's extent. Vertices are placed at +/- scale, so a degenerate bound — a stray
    // instance at 1e30, a scene caught mid-import with nothing loaded yet — would otherwise put
    // non-representable positions into the vertex stream and take the device down with it. Any scene that
    // legitimately needs more than this is already far outside the depth precision the viewport has.
    const MAX_SCALE: f32 = 100_000.0;
    let fallback = ([0.0, GROUND_PLANE_Y, 0.0], MIN_SCALE);
    let Some((lo, hi)) = bounds else {
        return fallback;
    };
    if !lo.is_finite() || !hi.is_finite() {
        return fallback;
    }
    let centre = (lo + hi) * 0.5;
    // Only the horizontal footprint matters: a tall model does not need a wider floor.
    let half_extent = ((hi.x - lo.x) * 0.5).max((hi.z - lo.z) * 0.5);
    if !half_extent.is_finite() {
        return fallback;
    }
    let scale = (half_extent * MARGIN).clamp(MIN_SCALE, MAX_SCALE);
    ([centre.x, GROUND_PLANE_Y, centre.z], scale)
}

/// M11.4 (ADR-043) — the active scene camera's look-through view parameters. A render PROJECTION (never
/// Loro/undo): when `SceneState.cam_override` is `Some`, the frame renders from this scene camera instead
/// of the editor fly-cam. Set by `look_through_camera` from the authored `Camera` entity.
#[derive(Clone, Copy)]
pub struct CamView {
    pub pos: [f32; 3],
    /// Where the camera AIMS. Until this existed the override reused the editor's orbit target, so a
    /// scene camera could stand anywhere but could only ever look at the same point — which makes a
    /// cutscene impossible by construction. `None` keeps the old behaviour (aim at the orbit target).
    pub look_at: Option<[f32; 3]>,
    pub fov_deg: f32,
    pub near: f32,
    pub far: f32,
}

/// The camera-centred slice covered by the directional shadow map.
///
/// Editor orbiting retains the established whole-scene safety fallback, while a cinematic camera owns
/// its framing outright: a hero shot aimed at a subject near the edge of a large CAD assembly must not
/// spend the shadow map back at the assembly centre. Keeping this decision explicit also prevents the
/// image camera and its shadows from acquiring two independent interpretations of `cam_override`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ShadowFraming {
    target: Vec3,
    distance: f32,
    lock_to_camera: bool,
}

impl ShadowFraming {
    const fn editor(target: Vec3, distance: f32) -> Self {
        Self {
            target,
            distance,
            lock_to_camera: false,
        }
    }

    fn cinematic(eye: Vec3, target: Vec3) -> Self {
        Self {
            target,
            distance: eye.distance(target).max(0.02),
            lock_to_camera: true,
        }
    }
}

/// Resolve the authored camera's aim exactly once for both the visible frame and its shadow framing.
fn resolved_camera_aim(override_view: CamView, editor_target: Vec3) -> (Vec3, Vec3, Vec3) {
    let eye = Vec3::from(override_view.pos);
    let mut target = Vec3::from(override_view.look_at.unwrap_or(editor_target.to_array()));
    // Two degenerate aims produce a NaN view matrix and a black frame. Both are reachable from ordinary
    // authoring (a camera dropped exactly on its subject; a top-down shot), so nudge rather than trust it.
    if (target - eye).length_squared() < 1.0e-8 {
        target = eye + Vec3::NEG_Z;
    }
    let dir = (target - eye).normalize_or_zero();
    let up = if dir.dot(Vec3::Y).abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    (eye, target, up)
}

fn active_shadow_framing(
    editor_target: Vec3,
    editor_distance: f32,
    cam_override: Option<CamView>,
) -> ShadowFraming {
    cam_override.map_or_else(
        || ShadowFraming::editor(editor_target, editor_distance),
        |override_view| {
            let (eye, target, _) = resolved_camera_aim(override_view, editor_target);
            ShadowFraming::cinematic(eye, target)
        },
    )
}

/// A drawn-but-geometry-free entity the viewport must still be able to select: a light, a camera, a
/// CAD assembly container, a terrain recipe.
///
/// These are exactly the entities that are visible in the outliner and visible in the viewport but
/// were absent from every pick candidate list, which is the difference between "you can select it"
/// and "clicking it selects something else".
#[derive(Clone, Debug)]
pub struct MarkerEntity {
    /// The entity's Loro key.
    pub id: String,
    /// Its world position — the centre of the grab proxy.
    pub position: [f32; 3],
    /// What it is, so the picker can rank it and the UI can name it.
    pub kind: metrocalk_spatial::HitKind,
}

/// A live-thumbnail request, carrying the identity that makes a late answer DETECTABLE.
///
/// The previous shape was `(id, size)`, and that is the whole bug: with nothing but an entity id, the
/// only question a consumer could ask was "is there a picture of this entity?" — never "is this the
/// picture I asked for?". Draining stale results before re-requesting narrowed the window; it could not
/// close it, because a request already handed to the render thread finishes AFTER the drain and lands
/// in the same id-keyed slot the next caller reads. `req` closes it: a result satisfies exactly one
/// request, by construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbRequest {
    /// The entity to picture.
    pub id: String,
    /// Pixel size (clamped by the renderer).
    pub size: u32,
    /// Unique and monotonic. The answer must quote it back.
    pub req: u64,
    /// The presentation state ([`metrocalk_assets::colour::PresentationState::hash`]) at request time —
    /// working space, view transform and exposure. Everything that can visibly change the image without
    /// changing the scene.
    pub state: u64,
}

/// A serviced thumbnail, stamped with the request it answers and the state it was actually rendered in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbResult {
    /// The entity pictured (kept for diagnostics and for dropping a superseded entity's leftovers).
    pub id: String,
    /// The request this answers. A consumer accepts ONLY its own.
    pub req: u64,
    /// The presentation state the render actually used — read at render time, not copied from the
    /// request. If the user moved the exposure slider in between, these differ, and the consumer is
    /// told the truth (an explicit miss) rather than handed a picture of a state nobody asked about.
    pub state: u64,
    /// The PNG, or `None` when the entity has no renderable instance — an answer, not a silence.
    pub png: Option<Vec<u8>>,
}

/// The four honest outcomes of asking for a thumbnail. Four, not "an `Option<Vec<u8>>`": "no image
/// yet", "this entity has no picture", and "your picture was rendered in a state that has since moved"
/// are different answers, and collapsing them is what let a stale image pass for a fresh one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThumbTake {
    /// Not serviced yet. Keep polling.
    Pending,
    /// The picture, rendered in the state the caller asked about.
    Ready(Vec<u8>),
    /// The entity has no renderable instance — a light, a camera, a folder row. An answer, immediately.
    NoImage,
    /// It was rendered, but the presentation state changed underneath it. Reported rather than
    /// returned: an image of a state nobody asked about is a wrong answer wearing a right one.
    StateMoved,
}

/// A request to keep the picture this viewport is about to present, as a file rather than a frame.
///
/// There is exactly one render path in this shell, and a still has to come out of THAT one — a second
/// offscreen renderer would be a second grade, the divergence [`render_thumbnail`]'s own comment names
/// and avoids. So a capture is a read of the swapchain texture between the submit and the present: the
/// same instances, the same lights, the same post route, the same final resolve, the same bars.
///
/// **`min_epoch` is the whole correctness argument.** A capture is nearly always asked for by something
/// that has just moved the camera — a render job stepping a cutscene a frame at a time — and the frame
/// already in flight when the request lands was drawn with the pose BEFORE it. Servicing that frame
/// would write shot 1's picture into shot 2's file, silently, and every file after it would be off by
/// one. [`SceneState::pose_epoch`] counts camera publications; a request records the epoch current when
/// it was made, and only a frame drawn at that epoch or later may answer it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRequest {
    /// Unique and monotonic, from the same counter the thumbnails use. The answer quotes it back.
    pub req: u64,
    /// The lowest [`SceneState::pose_epoch`] a frame may have been drawn at and still answer this.
    pub min_epoch: u64,
}

/// A serviced frame capture: the PNG, or the sentence saying why there is none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameResult {
    /// The request this answers.
    pub req: u64,
    /// The image, in the delivery frame's own pixels.
    pub png: Option<Vec<u8>>,
    /// The written size. Reported even on failure as `0 x 0`, so a caller never has to infer it.
    pub width: u32,
    pub height: u32,
    /// Why there is no image. `None` when there is one — never both.
    pub reason: Option<String>,
}

/// The three honest outcomes of asking for a frame, for the same reason [`ThumbTake`] has four: "not
/// yet" and "never, because …" are different answers and an `Option` collapses them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameTake {
    /// No frame has been drawn at the requested pose yet. Keep polling.
    Pending,
    /// The picture, and the pixel size it was written at.
    Ready {
        png: Vec<u8>,
        width: u32,
        height: u32,
    },
    /// It cannot be produced, and this is why. A sentence, not a silence.
    Failed(String),
}

/// Which room a scene is presented in.
///
/// A presentation choice with the same standing as the view transform: it changes what a camera can
/// see and nothing about what the project *is*. The default is deliberately the old behaviour, so no
/// existing scene, baseline capture or viewport test changes because this type exists — the room is
/// something a presentation asks for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PresentationSet {
    /// A ground plane under the model and open air around it. Right for inspecting a part; it is what
    /// every earlier film of the factory was shot in, and why those films measured mostly-empty frames.
    #[default]
    Studio,
    /// An industrial hall around the model — slab, walkways, clad walls, a column grid and a roof.
    FactoryHall,
}

impl PresentationSet {
    /// The wire name, for the command and the sidecar. Round-trips through [`Self::parse`].
    #[must_use]
    pub fn wire(self) -> &'static str {
        match self {
            Self::Studio => "studio",
            Self::FactoryHall => "factoryHall",
        }
    }

    /// Read a wire name, accepting the spellings a caller is likely to type.
    ///
    /// `None` for anything else rather than a silent fall back to `Studio`: a command that quietly
    /// ignores the word it was given would report success for a set it did not apply, and the operator
    /// would be looking at the wrong room believing it was the right one.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_', ' '], "")
            .as_str()
        {
            "studio" | "none" | "open" => Some(Self::Studio),
            "factoryhall" | "hall" | "factory" | "industrial" => Some(Self::FactoryHall),
            _ => None,
        }
    }

    /// The author-facing name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Studio => "Studio (open ground)",
            Self::FactoryHall => "Factory hall",
        }
    }
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
    /// The sub-rectangle of the surface the viewer can actually SEE, as window fractions
    /// `[x, y, width, height]`, reported by the shell whenever the layout changes.
    ///
    /// The wgpu surface is the whole window; the editor UI is drawn over it and shows the 3D through a
    /// transparent hole. Framing had no idea that hole existed, so `frame_all` and `focus_entity` fitted
    /// and CENTRED their subject on the window -- and with a dock open on each side, the window centre
    /// is not even inside the visible region. In the production factory captures the entire imported
    /// assembly sat against the left edge of the viewport with a third of the framed area hidden behind
    /// panels: not a subtle miscomposition, a subject the user cannot see properly.
    ///
    /// A degenerate value (including the `[0,0,0,0]` this derives by default) means "the whole surface",
    /// so the behaviour is unchanged until the shell reports a real rectangle.
    pub visible_rect: [f32; 4],
    /// The aspect ratio the ACTIVE cutscene is delivered in, or `None` when nothing is composing for a
    /// frame of its own.
    ///
    /// A cutscene is authored for a delivery frame - 16:9, scope, vertical - and the author's stage is
    /// whatever shape the docks have left it. Held here rather than on the cutscene document because
    /// this is render state: it lives and dies with `cam_override`, is never undoable, and is written
    /// by the same tick that poses the camera.
    pub delivery_aspect: Option<f32>,
    /// Per-slot local mesh bounds, memoised against `meshes_revision` -- see
    /// [`Self::local_bounds_for_slot`] for why walking them per instance was ruinous.
    /// DERIVED: write it only through [`Self::sync_mesh_bounds`], which keeps it and the revision
    /// below in step. (`pub` only because every other field is, so the struct-update syntax the tests
    /// and examples build state with keeps working.)
    pub mesh_bounds: Vec<Option<LocalBounds>>,
    /// The `meshes_revision` [`Self::mesh_bounds`] was built for. A mismatch is not a bug: it makes the
    /// readers fall back to walking the vertices, which is slow and correct.
    pub mesh_bounds_revision: u64,
    /// Bumped whenever the instance/id SET is replaced -- i.e. by `rebuild`, and not by a pose update.
    ///
    /// `revision` moves on every published pose, which is every animation tick; this moves only when the
    /// scene's membership changes. Anything derived from WHICH entities are drawn (rather than where they
    /// are) can be memoised against it.
    pub ids_revision: u64,
    /// Instance indices under a cinematic subject, memoised against [`Self::ids_revision`].
    ///
    /// Resolving a subject means descending its hierarchy and testing every published id against the
    /// result. On the imported factory that is ~17,000 `children_of` queries plus 17,793 key parses --
    /// per solve, twice per tick (a shot and the shot it blends from), sixty times a second, for a set
    /// that cannot change without a rebuild. It was the dominant cost of a 2.5-3 SECOND tick.
    pub cinema_subtree: std::collections::HashMap<String, Vec<usize>>,
    /// The `ids_revision` [`Self::cinema_subtree`] was built for.
    pub cinema_subtree_revision: u64,
    /// A broad phase over every published instance's world bounds, for the cinematic camera's occlusion
    /// queries — the structure that lets a shot ask "is anything between me and my subject?".
    ///
    /// Built lazily and ONLY when a cutscene asks for one, so a scene nobody is filming never pays for
    /// it. Deliberately **not refitted while a mechanism animates**: a moving part's box goes stale by at
    /// most its own stroke, which makes the camera very slightly more conservative about a place it was
    /// already unwilling to stand, and refitting 17,793 boxes per animation frame to buy that back would
    /// cost more than the whole query it serves.
    pub occlusion: metrocalk_spatial::SceneBvh,
    /// The [`Self::ids_revision`] [`Self::occlusion`] was built for; `None` until something asks.
    pub occlusion_revision: Option<u64>,
    /// Every camera placement this cutscene run has negotiated, in the order the shots were filmed.
    ///
    /// Recorded because "the film was directed well" and "the film quietly re-aimed half of it" look
    /// identical from outside, and the difference is the whole point of the negotiation. Cleared when a
    /// cutscene takes the camera, so it always describes the run being watched.
    pub cinematic_placements: Vec<CinematicPlacement>,
    /// Which room the scene is presented in. A viewing choice, exactly like the tone curve: it changes
    /// no entity, no component and no count, and it is not written into the document.
    pub presentation_set: PresentationSet,
    /// The room [`Self::presentation_set`] currently resolves to, sized to the live scene.
    ///
    /// Derived, memoised, and read by three separate consumers that must not disagree: the draw pass
    /// (what is on screen), the camera planner's backdrop test (what a shot can be backed by) and the
    /// shot solver's confinement (where a camera may stand). Three copies of "how big is the hall"
    /// would be three chances for the picture, the verdict and the placement to describe different
    /// rooms — which is the shape of every defect the last four passes of this work found.
    pub hall: Option<crate::hall::Hall>,
    /// The `(ids_revision, meshes_revision)` [`Self::hall`] was sized for.
    pub hall_revision: Option<(u64, u64)>,
    /// Entity key -> instance index, memoised against [`Self::ids_revision`].
    ///
    /// Publishing an animation pose used to scan all of `ids` looking for the handful of keys it had
    /// posed. That is 17,793 short-string hashes per tick on the imported factory to find 24 of them --
    /// measured at 21 ms of a 52 ms animation apply, against a 16.7 ms frame budget.
    pub index_of_id: std::collections::HashMap<String, usize>,
    /// The `ids_revision` [`Self::index_of_id`] was built for.
    pub index_of_id_revision: u64,
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
    /// The marker entities those glyphs belong to — id, world position and kind.
    ///
    /// The glyphs above are anonymous line segments: they say where to *draw* an icon and nothing
    /// about which entity it is. So a light or a camera was drawn but could not be clicked, and
    /// because the old picker always returned its nearest projected instance, clicking one silently
    /// selected an unrelated mesh. This list is what lets the picker give a light a pixel-sized grab
    /// proxy and answer with the light.
    pub marker_entities: Vec<MarkerEntity>,
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
    /// VFX particles that EMIT light — fire, sparks, magic. Reuses [`Instance`] as a particle carrier
    /// (`center` = world position, `scale` = world radius, `color` = LINEAR HDR colour, `selected` =
    /// opacity), drawn as camera-facing quads with ADDITIVE blending inside the HDR scene pass, which is
    /// why an over-1.0 colour blooms without any effect-specific plumbing. A Play-only projection: the
    /// document is never written, and Stop clears it. Empty ⇒ the pass is skipped, zero per-frame cost.
    pub fx_additive: Vec<Instance>,
    /// VFX particles that OCCLUDE light — smoke, dust, steam. Same carrier, alpha-blended, drawn after
    /// the additive layer so glow reads through haze rather than the other way round.
    pub fx_soft: Vec<Instance>,
    /// Bump when either particle list changes so the loop re-uploads them.
    pub fx_revision: u64,
    /// A panorama waiting to become the scene's environment, set by the import command and consumed
    /// by the render loop. `None` = the startup default (procedural sky, or `MTK_ENV_HDR`).
    pub pending_env: Option<crate::ibl::EnvSource>,
    /// Bump to make the loop rebuild the IBL from [`Self::pending_env`].
    pub env_revision: u64,
    /// What the current environment is called, for the UI to report.
    pub env_label: String,
    /// How many MOMENT-fired one-shot bursts are alive right now. Published purely so a test can tell
    /// "the burst was never created" from "the burst was created and drew nothing" — two failures that
    /// look identical from a particle count alone.
    pub fx_bursts: usize,
    /// HIGH-WATER MARKS since Play started: the most particles drawn in any one frame, and the total
    /// number of one-shot bursts that have ever come into existence this run.
    ///
    /// A sampled count cannot honestly answer "did the pick-up pop fire?": the burst lives 0.86 s and
    /// each poll costs two IPC round trips, so a test either catches it or does not, at random. A
    /// high-water mark answers the same question with one read at any later time, which is the
    /// difference between a measurement and a coin flip.
    pub fx_peak_total: usize,
    /// Monotonic count of one-shot bursts created this run.
    pub fx_bursts_fired: u64,
    /// Brightest particle seen in ANY frame this run. Sampled radiance has the same problem as a
    /// sampled count: a 0.6 s spark burst is over before a poll arrives, so the honest answer to "does
    /// this card emit?" is a high-water mark, not whatever happened to be on screen when asked.
    pub fx_peak_radiance: f32,
    /// True while a cutscene owns the camera. The viewport is an AUTHORING surface — a selection
    /// outline, a transform gizmo and binding lines are all correct there and all wrong the moment the
    /// frame becomes a shot. Found by looking at a capture of a working hero shot and seeing a yellow
    /// authoring rectangle and three RGB axis lines drawn across it. Set by the Play tick, cleared on
    /// Stop; it suppresses editor chrome INSIDE the viewport only — the surrounding UI is untouched,
    /// because the user still needs Stop.
    pub cinematic: bool,
    /// Diagnostic-only identity of the subject currently owned by cinematic playback. This is render
    /// evidence for runtime coverage checks; it is neither authored scene data nor persisted project state.
    pub cinematic_subject_id: Option<String>,
    /// Diagnostic-only zero-based shot index currently presented by cinematic playback. Like
    /// [`Self::cinematic_subject_id`], this exists solely to make playback evidence measurable.
    pub cinematic_shot_index: Option<usize>,
    /// Diagnostic-only high-water set (in first-visit order) of subjects presented during the current
    /// playback run. It is cleared/maintained by playback and never enters Loro, undo, or persistence.
    pub cinematic_visited_subjects: Vec<String>,
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
    /// What the cursor is over, as the loro keys of the SUBJECTS being pointed at — a leaf part, or
    /// an assembly whose whole subtree lights up. Kept as keys rather than instance indices for the
    /// same reason the cinema preview keeps its suppressed outline by key: indices are stable only
    /// between rebuilds, and a rebuild mid-hover would otherwise light an unrelated object.
    ///
    /// The RESOLVED half lives in `Instance::highlight`'s [`HIGHLIGHT_HOVERED`] bit. Empty is the
    /// normal state — a hover is a transient render projection, never document state, and nothing
    /// persists it.
    pub hovered: Vec<String>,
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
    /// The space the renderer SHADES in. Render-only state, like exposure and the profile.
    ///
    /// Not a scene fact: two people can open the same project in different working spaces and neither
    /// of them has changed the geometry, the materials or the lights. It is persisted with the
    /// project's PRESENTATION record rather than its document, and changing it dirties nothing.
    pub working_space: metrocalk_assets::colour::WorkingSpace,
    /// What the loaded environment map's values MEAN. Render-only state, defaulting to the documented
    /// assumption (linear Rec.709) rather than to a guess dressed as a fact.
    ///
    /// This is the per-asset override that is genuinely wireable here, and it is wireable because the
    /// environment is the one texture whose colour space the file honestly does not record: Radiance
    /// `.hdr` has no required primaries header and EXR's chromaticities attribute is optional. A studio
    /// that authors HDRIs in ACES can now say so, and the reflections, the ambient and the sky backdrop
    /// all move together, because all three read this one matrix.
    ///
    /// No IBL rebuild is needed when it changes: the conversion happens where the map is SAMPLED, and a
    /// primaries conversion commutes with the convolution that produced the mips.
    pub env_source_space: metrocalk_assets::colour::ColourSpace,
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
    /// The orbit `cam_target` saved alongside [`Self::pre_focus_distance`], and restored with it.
    ///
    /// Focusing does two things — it moves the camera IN, and it points it AT the part. Only the first
    /// was ever put back, so leaving focus returned the camera to its old distance while it went on
    /// staring at the part: "everything comes back to normal" left the viewer looking at a speck from
    /// across the factory, which reads as the camera having jumped somewhere arbitrary. Saved and taken
    /// in lock-step with the distance, so the "saved once on enter" rule covers both.
    pub pre_focus_target: Option<[f32; 3]>,
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
    /// authored directional with `castShadows`, or the synthesized key when no directional is authored).
    /// `None` means an authored directional explicitly disabled shadows: the pass is skipped,
    /// `light_view_proj` stays identity, and `fs_mesh` shadows nothing. Point/spot flags cannot select the
    /// directional-only map. Rebuilt with `lights` (a render projection).
    pub shadow_caster: Option<usize>,
    /// M14.2 (ADR-058) — pending live-thumbnail render requests `(entity id, size px)`, pushed by the
    /// `thumbnail` command and drained by the render thread, which renders each entity to a small offscreen
    /// target on **its own encoder before the swapchain frame** (off the per-frame orbit path — invariant 4;
    /// a discrete, dirty-only, budget-limited surface, NEVER per-frame). A presentation artifact: thumbnails
    /// never enter the op-stream/Loro doc (zero determinism impact, like the M11.3 lights projection).
    pub thumb_requests: Vec<ThumbRequest>,
    /// Serviced thumbnail results. The `thumbnail` command polls for the result carrying ITS OWN
    /// request id, then removes that entry — never merely the first one bearing the same entity id.
    /// Capped so a timed-out request can't grow it unbounded.
    pub thumb_results: Vec<ThumbResult>,
    /// The next request id to hand out. Monotonic for the life of the process; the counter lives here
    /// rather than in a global atomic because every producer and consumer already holds this lock, so
    /// there is exactly one place the ordering can be reasoned about. Shared with the frame captures
    /// below — one id space, so a result can never be mistaken for the other kind's.
    pub next_thumb_req: u64,
    /// ADR-175 — pending frame captures: "write down the picture you are about to present". Drained by
    /// the render thread AFTER its submit and BEFORE its present, so what lands in the file is the frame
    /// the viewer would have seen, produced by the one render path rather than a second one.
    pub frame_requests: Vec<FrameRequest>,
    /// Serviced captures. Polled by the engine thread, which owns the file writing — the render thread
    /// never touches the disk, so a slow or failing write cannot stall a frame.
    pub frame_results: Vec<FrameResult>,
    /// How many times the camera has been PUBLISHED — bumped by every writer of [`Self::cam_override`]
    /// that wants a capture to wait for its pose. See [`FrameRequest::min_epoch`]: without it a render
    /// job writes each shot's picture into the next shot's file.
    pub pose_epoch: u64,
    /// Whether this adapter's surface can be read back at all (`COPY_SRC` in its capabilities), decided
    /// once at start-up and published here so a refusal can name the real reason instead of the request
    /// timing out. `false` until the render thread has configured a surface.
    pub frame_capture_supported: bool,
    /// ADR-177 — the largest either side of a render target may be on THIS machine's graphics device
    /// (`max_texture_dimension_2d`), published once the device exists. `0` before then.
    ///
    /// Here rather than assumed at the ceiling constant because the alternative to refusing a size is
    /// the render loop asking a driver for a texture it will not make, and that is not an error
    /// anybody reads — it is the viewport going away in the middle of a render.
    pub max_render_dimension: u32,
    /// ADR-177 — the size the picture is DRAWN at, when it is not the window's.
    ///
    /// `None` — every frame the author is looking at — means the swapchain is the picture and a
    /// capture is a crop of it, which is why a render used to be exactly as tall as whatever the docks
    /// had left of the window. `Some((w, h))` means a render job asked for an output size: the frame is
    /// drawn into offscreen targets of exactly that shape, the file is the whole of them, and the
    /// window shows the same picture fitted into its viewport hole.
    ///
    /// One field, set by the job and cleared when it closes, because the alternative — a size passed
    /// down with each capture request — would let two frames of one sequence be different sizes.
    pub render_size: Option<(u32, u32)>,
    /// The composed rectangle ON SCREEN, in pixels, published by the render loop every frame.
    ///
    /// What a render is written at when nobody chose a size, and the number the dialog states so that
    /// "as on screen" is an offer with a size on it rather than a shrug. `[0, 0]` until the first
    /// frame — a size guessed before the surface exists is a claim the files may not honour.
    pub composed_pixels: [u32; 2],
    /// M19 (ADR-104) — terrain chunks the runtime wants uploaded, drained by the render thread a few per
    /// frame. Geometry and textures are separate `Option`s: a LOD switch re-sends only the vertices, because
    /// the chunk's half-megabyte splat texture has not changed.
    pub terrain_uploads: Vec<TerrainUpload>,
    /// Terrain slots whose GPU resources may be released (the chunk was evicted or the terrain replaced).
    pub terrain_drops: Vec<u32>,
    /// The terrain chunks to draw this frame, after culling and LOD selection. Rebuilt every update; the
    /// render loop reads it under the same lock it reads `instances` under, so terrain costs no extra IPC
    /// and no extra synchronization (invariant 4).
    pub terrain_draws: Vec<TerrainDraw>,
    /// The live terrain: its worker pool, residency ledger and streaming plan. Lives here so the render
    /// loop can advance it with the camera it already has, without a second lock or a per-frame command.
    pub terrain: crate::terrain::TerrainRuntime,
    /// Render slots for each scatter prototype's representations: `[mesh, lod1, lod2, lod3, impostor]`,
    /// `-1` where nothing is bound. Published by the engine thread (which owns the handle→slot map) so the
    /// render side can resolve a scattered instance without a lookup across the boundary.
    pub terrain_proto_slots: Vec<[i32; 5]>,
    /// This frame's scattered instances as `(mesh slot, instance)`. Appended to the ordinary per-asset
    /// instance lists, so vegetation draws through exactly the same instanced path as every other mesh —
    /// GPU instancing, one draw per prototype per LOD, no bespoke foliage pipeline.
    pub terrain_scatter: Vec<(i32, Instance)>,
    /// Which terrain tool the pointer is driving. Render-only state, like the gizmo mode.
    pub terrain_tool: TerrainTool,
    /// The active brush. Set once when the author changes it, never per frame.
    pub terrain_brush: TerrainBrush,
    /// A sculpt gesture is in flight: the render loop polls the cursor, ray-casts the terrain and extends
    /// the stroke natively. Only the gesture's start and end cross the JS boundary (invariant 4).
    pub terrain_painting: bool,
    /// Where the cursor last hit the terrain, for the brush ring overlay. `None` when it missed.
    pub terrain_cursor_hit: Option<[f32; 3]>,
    /// A terrain problem the author must see — a recipe that would not compile, a worker that died. Shown in
    /// the panel; `None` when the terrain is healthy. Reported rather than printed to a console nobody reads.
    pub terrain_problem: Option<String>,
    /// The world-space path the in-flight gesture has traced, in order.
    pub terrain_stroke_path: Vec<[f32; 2]>,
    /// Route control points clicked so far, awaiting a commit.
    pub terrain_route_points: Vec<[f32; 3]>,
    /// A one-shot request from the UI to place a route point at the cursor; the render loop consumes it,
    /// ray-casts, and appends. Keeps the click a single command rather than a screen-to-world round trip.
    pub terrain_place_point: bool,
    /// A test-injected normalized cursor. When `Some`, the render loop aims the terrain tools from it
    /// instead of the OS cursor, so an end-to-end test can drive the SAME native brush path deterministically
    /// — the identical trick `gizmo_test_cursor` plays for the transform gizmo. `None` ⇒ the live cursor
    /// drives, which is the production path.
    pub terrain_test_cursor: Option<(f32, f32)>,
}

/// Which terrain tool the pointer drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerrainTool {
    /// The pointer selects and orbits as usual.
    #[default]
    None,
    /// Left-drag sculpts.
    Sculpt,
    /// Left-click drops a route control point.
    Route,
}

/// The active brush, mirrored render-side so a gesture needs no round trip to know its own settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerrainBrush {
    /// 0 raise, 1 smooth, 2 flatten, 3 noise — the same order the panel lists them in.
    pub kind: u8,
    /// Radius in metres.
    pub radius_m: f32,
    /// Metres per dab, or blend weight for smooth/flatten.
    pub strength: f32,
    /// Falloff shape, 0 linear to 1 smooth.
    pub hardness: f32,
    /// Flatten target height, noise wavelength, or smooth radius scale.
    pub target_m: f32,
}

impl Default for TerrainBrush {
    fn default() -> Self {
        Self {
            kind: 0,
            radius_m: 24.0,
            strength: 2.0,
            hardness: 0.75,
            target_m: 0.0,
        }
    }
}

/// One terrain chunk's GPU payload.
///
/// `albedo`/`normal` are `None` when only the geometry changed — which is the common case, because a LOD
/// switch happens far more often than a texture re-bake and copying half a megabyte of splat for it would be
/// pure waste.
pub struct TerrainUpload {
    /// Stable slot index the runtime assigned to this chunk.
    pub slot: u32,
    /// Packed vertices, already in renderer form (the worker thread does the packing).
    pub vertices: Vec<MeshVertex>,
    /// Triangle-list indices.
    pub indices: Vec<u32>,
    /// Baked splat albedo, when it changed.
    pub albedo: Option<metrocalk_assets::mesh::Texture>,
    /// Baked detail normal map, when it changed.
    pub normal: Option<metrocalk_assets::mesh::Texture>,
}

/// One terrain chunk to draw.
pub struct TerrainDraw {
    /// Which uploaded slot.
    pub slot: u32,
    /// Where it sits; terrain geometry is chunk-local in XZ, so this is the chunk centre.
    pub instance: Instance,
}

/// Build an [`crate::ibl::EnvSource`] without callers naming the `ibl` module — the environment
/// import command lives in `main` and should not have to know where the IBL implementation sits.
#[must_use]
pub fn ibl_env_source(
    width: usize,
    height: usize,
    pixels: Vec<[f32; 3]>,
    label: String,
) -> crate::ibl::EnvSource {
    crate::ibl::EnvSource {
        width,
        height,
        pixels,
        label,
    }
}

pub type Shared = Arc<Mutex<SceneState>>;

impl SceneState {
    /// How this project is being LOOKED at, as one value.
    ///
    /// Everything in here can change the picture without changing the project, which is precisely why
    /// it is (a) the thing an async render result has to carry, and (b) the thing that must survive a
    /// reopen without being written into the document. One accessor so the two uses cannot disagree
    /// about what "the presentation" is — a hash that omits a field a shader reads is a stale image
    /// that passes every check.
    #[must_use]
    pub fn presentation(&self) -> metrocalk_assets::colour::PresentationState {
        use metrocalk_assets::colour::{PresentationState, ViewTransform};
        PresentationState {
            working: self.working_space,
            view: if self.render_profile > 0.5 {
                ViewTransform::PbrNeutral
            } else {
                ViewTransform::AcesFit
            },
            // 0 means "never set" here, and the render loop resolves it the same way. Hashing the raw
            // 0 would make an untouched project's state hash differ from the state it renders in.
            exposure: if self.exposure > 1e-4 {
                self.exposure
            } else {
                DEFAULT_EXPOSURE
            },
        }
    }

    /// Adopt a presentation state wholesale — the reopen path. Returns nothing and dirties nothing:
    /// this is a viewing choice, and the document is not involved.
    pub fn set_presentation(&mut self, p: metrocalk_assets::colour::PresentationState) {
        use metrocalk_assets::colour::ViewTransform;
        self.working_space = p.working;
        self.render_profile = f32::from(u8::from(p.view == ViewTransform::PbrNeutral));
        self.exposure = p.exposure.clamp(0.05, 8.0);
    }

    /// Allocate the next thumbnail request id. Monotonic; wrapping is unreachable in practice (a
    /// thumbnail per microsecond for 500,000 years) and would be harmless anyway, since only equality
    /// against a live request is ever tested.
    pub fn next_request(&mut self) -> u64 {
        self.next_thumb_req = self.next_thumb_req.wrapping_add(1);
        self.next_thumb_req
    }

    /// Queue a thumbnail request and return its identity. One place, so the request id and the state
    /// hash cannot be captured at two different moments — which would reintroduce the race in miniature.
    pub fn request_thumbnail(&mut self, id: &str, size: u32) -> (u64, u64) {
        let req = self.next_request();
        let state = self.presentation().hash();
        // Collapse a duplicate PENDING request only. Results are left alone: one may belong to a caller
        // still waiting, and taking it would be the same theft from the other direction.
        self.thumb_requests.retain(|r| r.id != id);
        self.thumb_requests.push(ThumbRequest {
            id: id.to_string(),
            size,
            req,
            state,
        });
        (req, state)
    }

    /// Take the answer to request `req`, if it has arrived.
    ///
    /// THE invariant, in one function so it can be tested without a GPU: *the image returned for
    /// request N was rendered from the state requested by N, or the request fails.* A result belonging
    /// to another request is never consumed here — not even one picturing the same entity — so a slow
    /// render cannot satisfy the request that replaced it.
    pub fn take_thumbnail(&mut self, req: u64, want_state: u64) -> ThumbTake {
        let Some(pos) = self.thumb_results.iter().position(|r| r.req == req) else {
            return ThumbTake::Pending;
        };
        let got = self.thumb_results.remove(pos);
        if got.state != want_state {
            return ThumbTake::StateMoved;
        }
        match got.png {
            Some(png) => ThumbTake::Ready(png),
            None => ThumbTake::NoImage,
        }
    }

    /// Publish a new camera pose to the render loop, so that captures asked for from here on wait for a
    /// frame that was drawn with it.
    ///
    /// Every writer of [`Self::cam_override`] that a capture could follow goes through this rather than
    /// assigning the field: an epoch that is bumped *most* of the time is worse than none, because the
    /// one path that forgot is the one that silently writes the previous pose's picture.
    pub fn publish_camera(&mut self, view: Option<CamView>) {
        self.cam_override = view;
        self.pose_epoch = self.pose_epoch.wrapping_add(1);
    }

    /// Ask for the next frame drawn at the current pose, as a PNG. Returns the request id to poll with.
    pub fn request_frame(&mut self) -> u64 {
        let req = self.next_request();
        let min_epoch = self.pose_epoch;
        self.frame_requests.push(FrameRequest { req, min_epoch });
        req
    }

    /// Take the answer to capture `req`, if it has arrived. A result belongs to exactly one request.
    pub fn take_frame(&mut self, req: u64) -> FrameTake {
        let Some(pos) = self.frame_results.iter().position(|r| r.req == req) else {
            return FrameTake::Pending;
        };
        let got = self.frame_results.remove(pos);
        match (got.png, got.reason) {
            (Some(png), _) => FrameTake::Ready {
                png,
                width: got.width,
                height: got.height,
            },
            (None, Some(why)) => FrameTake::Failed(why),
            // Unreachable by construction (the render thread writes one or the other), and a sentence
            // rather than an `expect` because the two producers are 2,000 lines apart.
            (None, None) => FrameTake::Failed("the frame came back empty".into()),
        }
    }

    /// Abandon a capture: drop its pending request and any result already waiting for it. Called when a
    /// render job is cancelled, so a finished job cannot be answered by the one before it.
    pub fn forget_frame(&mut self, req: u64) {
        self.frame_requests.retain(|r| r.req != req);
        self.frame_results.retain(|r| r.req != req);
    }

    /// The visible rectangle actually in force, after sanitisation -- what a caller reporting one gets
    /// back, so "it was rejected" is observable rather than a silent revert to the whole window.
    #[must_use]
    pub fn adopted_visible_rect(&self) -> [f32; 4] {
        sane_frame(self.visible_rect)
    }

    /// The rectangle the picture is COMPOSED for: the visible viewport, inset to the delivery frame when
    /// a cutscene is composing for one.
    ///
    /// The single answer to "what shape is the frame". The projection is sheared to it, the fit that
    /// `frame_all` and `focus_on` solve is measured against it, picking rays are cast through it, and
    /// the bars the stage draws are the difference between it and [`Self::adopted_visible_rect`]. One
    /// answer, so a shot cannot be composed for one frame and shown inside another.
    #[must_use]
    pub fn composition_rect(&self) -> [f32; 4] {
        let rect = self.adopted_visible_rect();
        // A delivery frame only insets while something is ACTUALLY holding the camera. Gated on
        // `cam_override` rather than on every teardown path remembering to clear the aspect: there are
        // five of those, and a missed one would letterbox the author's own viewport with no cutscene
        // on screen and no control anywhere to turn it off.
        if self.cam_override.is_none() {
            return rect;
        }
        match self.delivery_aspect {
            Some(want) if want.is_finite() && want > 0.05 => {
                inset_to_aspect(rect, self.surface_aspect, want)
            }
            _ => rect,
        }
    }

    /// The composed rectangle together with the surface it sits on -- what every projection, ray and fit
    /// in this file is built from.
    #[must_use]
    pub fn view_frame(&self, surface_aspect: f32) -> ViewFrame {
        ViewFrame::new(surface_aspect, self.composition_rect())
    }

    /// ADR-177 — the rectangle THIS frame is composed for, given whether it is being drawn into the
    /// window or into targets of its own.
    ///
    /// The ONE place that difference is decided, extracted from the frame loop so a gate can hold it.
    /// Offscreen the target *is* the frame: a render at a chosen size was handed a rectangle of exactly
    /// the delivery shape with no docks over it, so the composition is all of it — a plain centred
    /// projection, no inset, and a file that is the whole texture. On the window it is the hole the
    /// docks have left, inset to the delivery frame, which is what it has always been.
    #[must_use]
    pub fn drawn_frame(&self, offscreen: bool, aspect: f32) -> ViewFrame {
        if offscreen {
            ViewFrame::new(aspect, [0.0, 0.0, 1.0, 1.0])
        } else {
            self.view_frame(aspect)
        }
    }

    /// ADR-177 — the bars around this frame, in pixels, or `None` when there are none.
    ///
    /// Bars exist because a stage is the wrong shape for the delivery. A target built AT the delivery
    /// shape cannot be, so an offscreen render has none — and a file with bars baked into it would be
    /// a file that cannot be re-framed by anything downstream.
    #[must_use]
    pub fn drawn_letterbox(
        &self,
        offscreen: bool,
        frame: &ViewFrame,
        w: u32,
        h: u32,
    ) -> Option<[u32; 4]> {
        if offscreen {
            return None;
        }
        self.delivery_aspect
            .map(|_| frame_pixels(frame.rect(), w, h))
    }

    /// The aspect ratio of the rectangle the picture is composed for -- the number a shot solver needs
    /// to decide how far back the camera stands.
    #[must_use]
    pub fn composition_aspect(&self) -> f32 {
        self.view_frame(self.known_surface_aspect()).aspect()
    }

    /// The live surface aspect if one has been published, and the pre-surface guess otherwise.
    #[must_use]
    pub fn known_surface_aspect(&self) -> f32 {
        if self.surface_aspect > 0.01 {
            self.surface_aspect
        } else {
            DEFAULT_ASPECT
        }
    }

    /// M10.7 — **frame the whole scene**: center the orbit target on the scene's bounds and set a distance
    /// that fits them in view. A pure camera op (invariant 4 — render-state only, not undoable). No-op on an
    /// empty scene. Exits focus dim (framing-all looks at everything).
    pub fn frame_all(&mut self) {
        // The aspect the user is actually looking through, not a constant. Falls back only before the
        // first frame has published one.
        self.frame_all_with_aspect(self.known_surface_aspect());
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
        let frame = ViewFrame::new(aspect, self.composition_rect());
        let distance =
            fit_distance_in_viewport((hi - lo) * 0.5, frame.aspect(), self.orbit, self.elevation);
        // `clear_focus` restores the pre-focus distance, so it has to run BEFORE the new framing is
        // written -- otherwise framing everything hands the camera back the distance it had while
        // focused on one part.
        self.clear_focus();
        self.distance = distance;
        // The subject IS the orbit target. Sliding it sideways to land in the visible hole was the old
        // answer, and it was correct for exactly one frame: see `framed_projection`.
        self.cam_target = ((lo + hi) * 0.5).to_array();
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
                self.instances[p].highlight =
                    highlight_with(self.instances[p].highlight, HIGHLIGHT_SELECTED, false);
            }
        }
        self.selected = Some(i);
        self.instances[i].highlight =
            highlight_with(self.instances[i].highlight, HIGHLIGHT_SELECTED, true);
        // Center and size come from the real authored geometry. This is essential for offset meshes and for
        // CAD whose vertices are in millimetres while its instance scale is 0.001.
        let local_bounds = self
            .mesh_slots
            .get(i)
            .and_then(|slot| usize::try_from(*slot).ok())
            .and_then(|slot| self.local_bounds_for_slot(slot))
            .unwrap_or(LocalBounds::UNIT_CUBE);
        let (world_lo, world_hi) = instance_world_bounds(&self.instances[i], local_bounds);
        // Get nearby: save the framing once, then zoom to ~4× the entity's half-extent, clamped to the
        // orbit range so a huge or tiny entity still lands at a sensible, in-bounds distance.
        if self.pre_focus_distance.is_none() {
            self.pre_focus_distance = Some(if self.distance == 0.0 {
                60.0
            } else {
                self.distance
            });
            self.pre_focus_target = Some(self.cam_target);
        }
        // CM-SCALE floors (not the old 0.5 m / 6 m): focusing a centimetre-scale CAD part must get the
        // camera NEAR it (the old 6 m floor parked a 2 cm part sub-pixel — the same M15.9 defect family
        // as frame-all's metre floors).
        let half_extent = ((world_hi - world_lo) * 0.5).max_element().max(0.02);
        // Pull back when the composed frame is TALLER than it is wide: four half-extents of stand-off
        // clears the vertical field of view with room to spare, and only the horizontal one can be
        // tighter. A wide frame needs nothing, which is why this used to be the visible fraction and is
        // now the frame's own shape - the docks no longer crop the picture, they change what it is.
        let frame = self.view_frame(self.known_surface_aspect());
        let narrow = frame.aspect().clamp(0.05, 1.0);
        self.distance = (half_extent * 4.0 / narrow).clamp(0.15, 400.0);
        self.cam_target = ((world_lo + world_hi) * 0.5).to_array();
        self.focused = Some(i);
        self.revision = self.revision.wrapping_add(1);
    }

    /// Exit Focus mode ("everything comes back to normal"): clear the focus flag (the shader un-dims
    /// every entity) and restore BOTH the orbit `distance` and the `cam_target` saved when focus was
    /// entered. Idempotent — a no-op (no `revision` bump) when nothing is focused, so a stray Escape
    /// never disturbs the scene. Selection is intentionally left as-is (only the dim + framing revert).
    pub fn clear_focus(&mut self) {
        if self.focused.is_none() {
            return;
        }
        self.focused = None;
        if let Some(d) = self.pre_focus_distance.take() {
            self.distance = d;
        }
        if let Some(target) = self.pre_focus_target.take() {
            self.cam_target = target;
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
    /// Write `data` into the slot's storage buffer, growing it if it no longer fits.
    ///
    /// **Returns whether the underlying buffer was REPLACED.** A bind group holds a reference to a
    /// specific buffer, so it survives any number of writes into that buffer and is invalidated only by
    /// a reallocation. Reporting it is what lets the caller stop rebuilding every bind group in the
    /// scene on every frame of an animation, where the instance COUNT never changes at all.
    fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        data: &[Instance],
    ) -> bool {
        let needed = data.len() as u64;
        let reallocated = needed > self.cap;
        if reallocated {
            self.cap = needed.next_power_of_two();
            self.buf = new_instance_storage(device, self.cap);
            self.bg = make_inst_bg(device, layout, &self.buf);
        }
        if !data.is_empty() {
            queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(data));
        }
        self.n = data.len() as u32;
        reallocated
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

impl TextureSemantic {
    /// The upload treatment for a texture in a given role, DERIVED from the colour policy rather than
    /// restated here.
    ///
    /// The sRGB-or-raw decision belongs to exactly one place — `metrocalk_assets::colour::infer_for_role`
    /// — because it is the decision that goes wrong silently: a roughness map handed an sRGB decode is
    /// darkened through the mid-tones, reads as "the material looks off", and is almost never traced
    /// back to colour management. Previously each call site chose `Color` or `Data` by hand and happened
    /// to be right; "happened to be right" is not a property that survives a refactor.
    ///
    /// `Normal` is a filtering distinction layered ON TOP of that, not a competing one: a normal map is
    /// raw data as far as the transfer function goes (which is what the policy says), and additionally
    /// must be renormalised when mipped rather than box-averaged like a scalar.
    fn for_role(role: metrocalk_assets::colour::TextureRole) -> Self {
        use metrocalk_assets::colour::{ColourSpace, TextureRole};
        if role == TextureRole::Normal {
            return Self::Normal;
        }
        // Exhaustive on purpose. `Color` means "hand this to the sRGB hardware path", so ONLY the sRGB
        // space may map to it — a catch-all `_ => Self::Color` would mean that the day the policy
        // returns a linear colour space for some role, that texture silently receives an sRGB decode it
        // was never meant to get. Which is the exact class of bug this whole module exists to prevent.
        match metrocalk_assets::colour::infer_for_role(role).space {
            ColourSpace::Srgb => Self::Color,
            // Already linear, and NOT to be decoded. `Data` is the right treatment for both halves of
            // that: `Rgba8Unorm` applies no transfer function, and box-averaging a mip is correct for
            // values that are linear in light — which is exactly what `Color` mipping goes out of its
            // way to achieve by decoding first.
            ColourSpace::LinearRec709
            | ColourSpace::AcesCg
            | ColourSpace::Aces2065_1
            | ColourSpace::LinearRec2020
            | ColourSpace::Data => Self::Data,
        }
    }
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
///
/// It is cleared into the HDR scene target, so it is a SCENE value and takes the same working-space
/// conversion every other scene value takes — the CPU-side mirror of `to_working` in scene.wgsl. Left in
/// Rec.709 it would be the one colour in the frame that did not move when the working space did, and it
/// is the colour behind everything.
#[must_use]
fn clear_color(cad: bool, working: metrocalk_assets::colour::WorkingSpace) -> wgpu::Color {
    let c = metrocalk_assets::colour::apply(
        working.from_rec709(),
        unlit_srgb_to_scene_linear(CLEAR_COLOR_SRGB, DEFAULT_EXPOSURE, cad),
    );
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
    let format = texture_format(semantic);
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
/// The GPU format for a texture with this semantic — **the actual sRGB decision**, as a pure function.
///
/// Pulled out of `upload_tex` for one reason: it could not be tested where it was. `upload_tex` needs a
/// live `wgpu::Device`, so no unit test reached it, and the test that looked like it covered this only
/// re-asserted facts about `TextureSemantic::for_role`. Flipping the comparison would have sent every
/// roughness, metallic and occlusion map through a hardware sRGB decode with the whole suite still
/// green. Now the decision is one expression with a test on it, and `upload_tex` calls it rather than
/// restating it — the same discipline `for_role` applies one level up.
fn texture_format(semantic: TextureSemantic) -> wgpu::TextureFormat {
    match semantic {
        TextureSemantic::Color => wgpu::TextureFormat::Rgba8UnormSrgb,
        // Raw. Both because no transfer function may be applied, and because a normal map's channels
        // encode a direction rather than light.
        TextureSemantic::Data | TextureSemantic::Normal => wgpu::TextureFormat::Rgba8Unorm,
    }
}

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

/// The renderer's working space, as the shaders consume it.
///
/// This is the whole of "the engine renders in ACEScg": two matrices and a set of luminance weights. It
/// is deliberately not a shader variant or a second pipeline — a second code path is what lets one of
/// the two working spaces rot. With [`WorkingSpace::LinearRec709`] both matrices are the identity and
/// the weights are Rec.709's, so the AP1 path and the Rec.709 path are the SAME instructions, and
/// selecting Rec.709 is provably a no-op (`selecting_rec709_is_the_identity_transform`).
///
/// Where they apply, precisely:
///
/// * `to_working` converts every authored/imported colour — base colour, emissive, light colour,
///   environment radiance, and the unlit chrome constants — from linear Rec.709 into the working space,
///   BEFORE it takes part in any colour computation. That ordering is the requirement: converting the
///   framebuffer afterwards is not rendering in ACEScg, it is rendering in Rec.709 and relabelling.
/// * `from_working` converts the finished frame back to the space the view transform is DEFINED on
///   (see `ViewTransform::input_space`) — once, in `fs_resolve`, immediately before the tone curve.
/// * `luma` is what "brightness" means here. Bloom extraction uses it; using Rec.709's weights on AP1
///   values would bloom the wrong pixels, and it is the kind of wrong that looks like a tuning problem.
///
/// Primaries conversion is a LINEAR operator, which is why it is correct to apply it at the point of
/// sampling rather than baking it into the textures: it commutes with every linear operation between
/// here and the tone curve — mip filtering, the MSAA resolve, the IBL convolution, the Gaussian blur.
/// Baking would cost a re-upload per asset per working-space change, and would clip an AP1 value that a
/// Rec.709-primaries 8-bit texture cannot hold.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ColourUniform {
    /// linear Rec.709 → working space, WGSL column layout.
    to_working: [[f32; 4]; 3],
    /// The ENVIRONMENT's declared source space → working space, composed into one matrix.
    ///
    /// Separate from `to_working` because the environment is the one asset whose colour space is a
    /// genuine unknown rather than a convention: Radiance `.hdr` has no required primaries field and
    /// EXR's chromaticities attribute is optional, so a facility working in ACES can hand this renderer
    /// a panorama that is NOT Rec.709 and nothing in the file will say so. Equal to `to_working` until
    /// someone declares otherwise. Placed here — before `from_working` — so the scene pass can reach it
    /// while still being unable to name the inverse conversion.
    env_to_working: [[f32; 4]; 3],
    /// working space → linear Rec.709, WGSL column layout.
    from_working: [[f32; 4]; 3],
    /// `.xyz` = the working space's luminance weights; `.w` = 1.0 when the working space is not
    /// Rec.709, which is what the double-transform validator keys on.
    luma: [f32; 4],
}

impl ColourUniform {
    fn new(
        working: metrocalk_assets::colour::WorkingSpace,
        env_source: metrocalk_assets::colour::ColourSpace,
    ) -> Self {
        use metrocalk_assets::colour::{source_to_working, wgsl_mat3, WorkingSpace};
        let w = working.luminance_weights();
        Self {
            to_working: wgsl_mat3(working.from_rec709()),
            // A non-linear declaration cannot be expressed as a matrix, and the command that sets this
            // refuses one — so falling back to the Rec.709 assumption here is unreachable rather than
            // lenient. It is written as a fallback anyway because "unreachable" and "panics the render
            // thread" are one refactor apart.
            env_to_working: wgsl_mat3(
                source_to_working(env_source, working).unwrap_or_else(|| working.from_rec709()),
            ),
            from_working: wgsl_mat3(working.to_rec709()),
            luma: [
                w[0],
                w[1],
                w[2],
                f32::from(u8::from(working != WorkingSpace::LinearRec709)),
            ],
        }
    }
}

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
    /// The working space. Appended LAST on purpose: `ssao.wgsl` and the unlit shaders declare only the
    /// leading fields, and WGSL allows a uniform struct smaller than its buffer — so a pass that has no
    /// opinion about colour does not have to restate the block, and cannot restate it wrongly.
    colour: ColourUniform,
}
// The WGSL `Camera` is 3×mat4 (192) + 3×vec4 (48) + the colour block — 3×mat3x3 (48 each: WGSL pads a
// mat3 column to 16 bytes) + 1×vec4 (16) = 160 — so 400 bytes. Keep this struct byte-identical or wgpu
// rejects the uniform at draw. A compile-time tripwire so a future field can't silently desync the
// layout; it caught exactly that when the colour block was added.
const _: () = assert!(std::mem::size_of::<Camera>() == 400);
// And the colour block on its own, because a mat3 that is padded wrongly does not fail loudly — it
// transposes or shears the primaries conversion, which looks like a colour bug somewhere else entirely.
const _: () = assert!(std::mem::size_of::<ColourUniform>() == 160);

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

/// The memoised scene bounds: the `(revision, instance count, mesh count)` the bounds were computed
/// for, and the bounds themselves (`None` when the scene is empty). Recomputing per frame walks every
/// instance, which at factory scale is the difference between a smooth viewport and a stutter.
type BoundsCache = Option<((u64, usize, usize), Option<(Vec3, Vec3)>)>;

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
            crate::diag::log(&format!(
                "FATAL: create_surface failed: {e} - there will be no viewport this session."
            ));
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
    crate::diag::log(&format!(
        "viewport: adapter='{}' backend={:?} driver='{}'",
        adapter.get_info().name,
        adapter.get_info().backend,
        adapter.get_info().driver_info
    ));
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
    // A lost device and an uncaptured validation/OOM error are the two ways this loop can stop
    // producing frames without any code of ours returning an error. Neither had a handler, and in a
    // windows-subsystem binary the default ones print to a stderr nobody owns -- so a GPU-side death
    // looked, from outside, exactly like a healthy app that had stopped drawing.
    device.set_device_lost_callback(|reason, message| {
        crate::diag::log(&format!(
            "FATAL: wgpu device LOST ({reason:?}): {message} - the viewport cannot present again without a full re-initialisation."
        ));
    });
    device.on_uncaptured_error(std::sync::Arc::new(|error| {
        crate::diag::log(&format!("wgpu uncaptured error: {error}"));
    }));

    let caps = surface.get_capabilities(&adapter);
    let format = caps
        .formats
        .iter()
        .copied()
        .find(|f| !f.is_srgb())
        .unwrap_or(caps.formats[0]);
    // ADR-175 — a still has to come out of the ONE render path, which means reading back the swapchain
    // texture itself. That needs `COPY_SRC` on the surface, which every desktop backend offers and none
    // is obliged to; asking for it where it is unavailable makes `configure` fail and the viewport never
    // appears, so the capability decides and the answer is published for the refusal to quote.
    let can_capture = caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
    if !can_capture {
        crate::diag::log(
            "viewport: this surface cannot be copied from - rendering a frame to a file is unavailable on this adapter",
        );
    }
    {
        let mut st = shared.lock().unwrap();
        st.frame_capture_supported = can_capture;
        st.max_render_dimension = device.limits().max_texture_dimension_2d;
    }
    let mut config = wgpu::SurfaceConfiguration {
        usage: if can_capture {
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC
        } else {
            wgpu::TextureUsages::RENDER_ATTACHMENT
        },
        format,
        width: w,
        height: h,
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode: caps.alpha_modes[0],
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);
    // Consecutive frames on which the surface vended nothing -- see the acquisition match below.
    let mut surface_stall_frames: u64 = 0;
    // M15.11 — the two render formats, carried together from here down. `hdr` is the linear-HDR scene
    // intermediate that every scene pipeline draws into; `display` is the swapchain, written by exactly one
    // pass. Before this split there was a single `format` doing both jobs, which is why MSAA resolved
    // gamma-encoded samples and each scene shader had to tone-map for itself.
    let formats = RenderFormats::new(format);
    if let Err(missing) = hdr_support_gap(&adapter, formats.hdr) {
        // No silent degrade to the old mixed-space pipeline: that path IS the defect. Rgba16Float
        // render-attachment + filtering + blending is mandatory in WebGPU core, so this is a tripwire.
        crate::diag::log(&format!(
            "FATAL: adapter '{}' cannot host the linear-HDR scene target {:?} — missing {missing}. \
             There is no gamma-space fallback; update the graphics driver or select a conformant adapter.",
             adapter.get_info().name,
             formats.hdr
        ));
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
    // ADR-177 — allocated only while a render at a chosen size is running. Its size is read under the
    // same lock as the camera generation (see the frame loop), so a capture can never be answered by a
    // frame drawn at a size the requester did not ask for.
    let mut offscreen: Option<Offscreen> = None;
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
    let mut ibl = crate::ibl::create(&device, &queue, &ibl_bgl, &shadow_view, &shadow_sampler);
    let mut cur_env_rev: u64 = 0;

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
    // Placeholder placement for the empty scene; `ground_placement` re-sizes it to whatever gets imported.
    let mut ground_placed: ([f32; 3], f32) = ([0.0, -0.02, 0.0], 60.0);
    // Memoized scene bounds — see the frame body for why this must not be recomputed per frame.
    let mut bounds_cache: BoundsCache = None;
    ground_inst.upload(
        &device,
        &queue,
        &inst_bgl,
        &[Instance {
            center: [0.0, -0.02, 0.0], // a hair below the grid so the grid lines read on top
            scale: 60.0,
            color: [0.30, 0.31, 0.34],
            highlight: 0.0,
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

    // The presentation hall (see `crate::hall`). Its vertices are built in WORLD space, so its single
    // instance is the identity — unlike the ground quad, which is a unit quad the instance places and
    // scales. Rebuilt only when the room's dimensions change, which is when the scene's bounds do.
    let mut hall_inst = InstanceBuf::new(&device, &inst_bgl, 1);
    hall_inst.upload(
        &device,
        &queue,
        &inst_bgl,
        &[Instance {
            center: [0.0; 3],
            scale: 1.0,
            color: [1.0; 3],
            highlight: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0; 4], // no override → the baked per-surface material is the material
        }],
    );
    let hall_main_bg = make_mesh_main_bg(
        &device,
        &mesh_inst_bgl,
        &hall_inst.buf,
        &dummy_view,
        &dummy_mr_view,
        &dummy_normal_view,
        &dummy_mr_view,
        &albedo_sampler,
    );
    let mut hall_built: Option<crate::hall::Hall> = None;
    let mut hall_buffers: Option<(wgpu::Buffer, wgpu::Buffer, u32)> = None;

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
    // VFX: particles TEST depth (so a flame is correctly hidden behind a wall) but never WRITE it.
    let fx_depth_state = wgpu::DepthStencilState {
        format: DEPTH_FORMAT,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let fx_add_pipeline = make_particle_pipeline(
        &device,
        &shader,
        &cube_layout,
        formats.hdr,
        &fx_depth_state,
        wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        },
        samples,
        "fx-additive",
    );
    let fx_soft_pipeline = make_particle_pipeline(
        &device,
        &shader,
        &cube_layout,
        formats.hdr,
        &fx_depth_state,
        wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        samples,
        "fx-soft",
    );
    let mut fx_add_buf = InstanceBuf::new(&device, &inst_bgl, 256);
    let mut fx_soft_buf = InstanceBuf::new(&device, &inst_bgl, 256);
    let mut cur_fx_rev = u64::MAX;
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
    /// The `meshes_revision` the per-submesh main-pass bind groups were built against. `u64::MAX - 1`
    /// rather than `u64::MAX` so it cannot be equal to `cur_mesh_rev`'s own initial value: the very first
    /// scene must build its bind groups even if the mesh table is somehow already current.
    const BIND_GROUPS_UNBUILT: u64 = u64::MAX - 1;
    let mut bind_groups_built_for = BIND_GROUPS_UNBUILT;
    let mut cube_scratch: Vec<Instance> = Vec::new();
    let mut mesh_scratch: Vec<Vec<Instance>> = Vec::new();
    // M19 (ADR-104) — terrain chunks. A separate slot table from the entity meshes above, because terrain
    // slots churn with the camera while the asset set does not: mixing them would mean re-uploading every
    // asset each time a chunk streamed in. Each slot holds its own geometry and its own baked splat
    // textures, and every slot shares ONE fixed-capacity instance storage buffer so a chunk's bind group
    // stays valid for its whole life (the buffer is never reallocated, only written).
    let mut terrain_geo: Vec<Option<(wgpu::Buffer, wgpu::Buffer, u32)>> = Vec::new();
    let mut terrain_tex: Vec<(wgpu::TextureView, wgpu::TextureView)> = Vec::new();
    let mut terrain_bg: Vec<Option<wgpu::BindGroup>> = Vec::new();
    let terrain_inst_buf = new_instance_storage(&device, TERRAIN_INSTANCE_CAPACITY);
    // The depth-only shadow pass binds instances alone (no textures), so terrain needs its own group-1 over
    // the same buffer. Built once: the terrain instance buffer has a fixed capacity and never reallocates,
    // so this bind group can never go stale the way a growing per-asset one can.
    let terrain_shadow_bg = make_inst_bg(&device, &inst_bgl, &terrain_inst_buf);
    let mut terrain_scratch: Vec<Instance> = Vec::new();
    // The terrain tools' own always-on-top line overlay: the brush ring, and the route being drawn. Its own
    // buffer rather than the contact debugger's, so a debugger toggle cannot erase the brush.
    let mut terrain_overlay = InstanceBuf::new(&device, &inst_bgl, 256);
    let mut terrain_overlay_pts: Vec<Instance> = Vec::new();
    // M19 — how many entries of each per-slot instance list belong to real entities. Scattered vegetation is
    // appended after that mark every frame and trimmed back to it before the next append, so the entity
    // partition is computed once per scene revision while the foliage follows the camera.
    let mut entity_inst_len: Vec<usize> = Vec::new();
    let mut scatter_slots_used: Vec<usize> = Vec::new();
    // Slots parallel to `terrain_scratch`: draw `k` uses slot `terrain_slots[k]` with instance range
    // `k..k+1`.
    let mut terrain_slots: Vec<u32> = Vec::new();

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
        // …and the client area's own position on the desktop, for the same reason. `cursor_position()`
        // is DESKTOP-relative; every surface-fraction consumer below needs it CLIENT-relative.
        let client_origin = window
            .inner_position()
            .ok()
            .map(|p| (f64::from(p.x), f64::from(p.y)));

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
            terrain_active,
            visibility,
            env_source_space,
            working_space,
            letterbox,
            capture_rect,
            preview_rect,
            want_out,
            drawn_epoch,
        ) = {
            // The real surface aspect, so the very first framing fits the lens the user is looking
            // through rather than an assumed one.
            let aspect_hint = config.width as f32 / config.height.max(1) as f32;
            let mut st = shared.lock().unwrap();
            // ADR-177 — THE SIZE THE PICTURE IS DRAWN AT, read HERE and not at the top of the loop.
            //
            // It has to be read in the same critical section as `pose_epoch`, for exactly the reason
            // `pose_epoch` exists: a render job sets the size, publishes its camera and asks for a
            // frame, and each of those takes this lock separately. Read the size earlier and this
            // sequence is possible — loop sees `None`, job sets the size AND publishes, loop reads the
            // new epoch, and the frame is drawn at the WINDOW's size while carrying an epoch that says
            // it may answer the request. The first file of the sequence is then a different shape from
            // the other 59. That is not a thought experiment: it is what the `.exe` determinism gate
            // caught on 2026-08-31, as `take.0000.png` differing between two otherwise identical runs.
            //
            // Read together, the two are consistent by construction: a frame either has the new size
            // and the new epoch, or the old size and an epoch too low to answer.
            let want_out = st.render_size;
            let (rw, rh) = want_out.unwrap_or((w, h));
            let offscreen_on = want_out.is_some();
            let visibility = ViewportVisibility::from_cinematic(st.cinematic);
            // Publish it before anything reads it, so a `frame_all` arriving from the UI this frame
            // frames against the surface as it is now rather than as it was when the window opened.
            st.surface_aspect = aspect_hint;
            // M19 (ADR-104) — advance the terrain runtime FIRST, so the uploads and the draw list the
            // blocks below consume are this frame's. Planning is gated inside the runtime on camera
            // movement, so a still frame costs a distance check and a draw-list rebuild.
            if st.terrain.is_active() {
                let eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
                let vp = camera_matrix_with(
                    st.orbit,
                    st.elevation,
                    st.distance,
                    st.view_frame(aspect_hint),
                    st.cam_target.into(),
                    st.projection,
                )
                .to_cols_array();
                let focus = st.terrain.focus_chunk(eye);
                // Moved out and back so the runtime can write into the very state it lives in; the move is
                // a handful of pointers.
                let mut rt = std::mem::take(&mut st.terrain);
                rt.update(&mut st, eye, vp, focus);
                st.terrain = rt;
            }
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
                let cursor = st
                    .gizmo_test_cursor
                    .or_else(|| surface_fraction(cursor_pos, client_origin, w, h));
                if let Some(cur) = cursor {
                    let aspect = w as f32 / h.max(1) as f32;
                    let (ro, rd) = cursor_ray(
                        cur,
                        st.orbit,
                        st.elevation,
                        st.distance,
                        st.view_frame(aspect),
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
                            // Each role asks the colour policy what it is, rather than the call site
                            // asserting it. Same treatment as before — now for a stated reason.
                            base_view: sm.base_color_texture.as_ref().map_or_else(
                                || dummy_view.clone(),
                                |t| {
                                    upload_tex(
                                        &device,
                                        &queue,
                                        t,
                                        TextureSemantic::for_role(Role::BaseColour),
                                    )
                                },
                            ),
                            mr_view: sm.metallic_roughness_texture.as_ref().map_or_else(
                                || dummy_mr_view.clone(),
                                |t| {
                                    upload_tex(
                                        &device,
                                        &queue,
                                        t,
                                        TextureSemantic::for_role(Role::MetallicRoughness),
                                    )
                                },
                            ),
                            normal_view: sm.normal_texture.as_ref().map_or_else(
                                || dummy_normal_view.clone(),
                                |t| {
                                    upload_tex(
                                        &device,
                                        &queue,
                                        t,
                                        TextureSemantic::for_role(Role::Normal),
                                    )
                                },
                            ),
                            ao_view: sm.occlusion_texture.as_ref().map_or_else(
                                || dummy_mr_view.clone(),
                                |t| {
                                    upload_tex(
                                        &device,
                                        &queue,
                                        t,
                                        TextureSemantic::for_role(Role::Occlusion),
                                    )
                                },
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
                entity_inst_len.clear();
                // Did any slot's storage buffer actually move? That — and only that — invalidates the
                // bind groups built against it.
                let mut buffers_moved = false;
                for (slot, group) in mesh_scratch.iter().enumerate() {
                    entity_inst_len.push(group.len());
                    buffers_moved |= mesh_inst[slot].upload(&device, &queue, &inst_bgl, group);
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
                // M11.2 follow-up — rebuild each mesh's main-pass group-1 bind groups, **one per submesh**,
                // pairing the current instance buffer with that submesh's own textures.
                // `mesh_inst.len() ≥ gpu_meshes.len()` (the meshes_revision block grows it first).
                //
                // GATED, because the comment that used to sit here — "only on scene-edit revisions, never
                // per frame" — stopped being true the moment anything in the scene animated. A published
                // pose bumps `revision`, so on the imported factory this rebuilt a bind group for every
                // submesh of every one of 15,711 parts SIXTY TIMES A SECOND, while holding the scene lock
                // the engine thread needs to publish the next pose. It was the measured reason playback
                // ran at a quarter of real time.
                //
                // A bind group references a BUFFER, not its contents: writing new instance data into the
                // same buffer leaves it perfectly valid. Only two things invalidate it — a storage buffer
                // that was reallocated because the instance count grew, and a mesh table that was replaced
                // underneath it (new textures, new submeshes). Both are scene edits, which is what the
                // original comment meant.
                let bind_groups_stale = buffers_moved
                    || mesh_main_bg.len() != gpu_meshes.len()
                    || lod_main_bg.len() != gpu_lods.len()
                    || bind_groups_built_for != cur_mesh_rev;
                if bind_groups_stale {
                    bind_groups_built_for = cur_mesh_rev;
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
                }
                // Tracking-line endpoints (rebuilt in lock-step with instances). Visibility is routed at
                // the draw pass so switching modes cannot invalidate this scene-owned upload.
                lines.upload(&device, &queue, &inst_bgl, &st.line_points);
                // M11.4 — light/camera icon glyphs (rebuilt with the scene).
                markers.upload(&device, &queue, &inst_bgl, &st.marker_glyphs);
            }
            // M19 (ADR-104) — the terrain tools. Aiming and stroke extension happen HERE, on the render
            // thread, from the OS cursor: a paint gesture is two commands (down and up) however long it
            // lasts, exactly like the orbit and the gizmo drag (invariant 4). Doing it from JS mouse-moves
            // would put an IPC round trip and a frame of latency between the cursor and the brush.
            //
            // The ray-cast runs whenever a terrain is live, NOT only when a tool is armed: the cursor hit is
            // also what "this" means to a described edit ("raise this mountain"), and an author refining a
            // world by sentence is not holding a brush. Gating it on the tool made every deictic fall back
            // to the view centre.
            if st.terrain.is_active() {
                let cursor = st
                    .terrain_test_cursor
                    .or_else(|| surface_fraction(cursor_pos, client_origin, w, h));
                let hit = cursor.and_then(|cur| {
                    let (ro, rd) = cursor_ray(
                        cur,
                        st.orbit,
                        st.elevation,
                        st.distance,
                        st.view_frame(aspect_hint),
                        st.cam_target,
                        st.projection,
                    );
                    st.terrain
                        .terrain()
                        .and_then(|t| t.raycast(ro, rd, st.distance * 8.0 + 4000.0))
                });
                st.terrain_cursor_hit = hit;

                // A one-shot route point, requested by a click in the viewport.
                if st.terrain_place_point {
                    st.terrain_place_point = false;
                    if let Some(p) = hit {
                        st.terrain_route_points.push(p);
                    }
                }

                if st.terrain_painting {
                    if let Some(p) = hit {
                        // Space the recorded path by a fraction of the brush so a slow drag and a fast one
                        // over the same ground record the same stroke.
                        let spacing = (st.terrain_brush.radius_m * 0.25).max(0.05);
                        let far_enough = st.terrain_stroke_path.last().is_none_or(|last| {
                            let dx = p[0] - last[0];
                            let dz = p[2] - last[1];
                            (dx * dx + dz * dz).sqrt() >= spacing
                        });
                        if far_enough {
                            st.terrain_stroke_path.push([p[0], p[2]]);
                            let strokes =
                                strokes_from_path(&st.terrain_stroke_path, st.terrain_brush);
                            let mut rt = std::mem::take(&mut st.terrain);
                            rt.set_preview_strokes(&strokes, &mut st);
                            st.terrain = rt;
                        }
                    }
                } else if !st.terrain_stroke_path.is_empty() {
                    // The gesture ended and the commit has landed; drop the preview.
                    st.terrain_stroke_path.clear();
                    let mut rt = std::mem::take(&mut st.terrain);
                    rt.clear_preview(&mut st);
                    st.terrain = rt;
                }
            } else if st.terrain_cursor_hit.is_some() {
                st.terrain_cursor_hit = None;
            }
            // Rebuild the tool overlay each frame: a ring under the cursor, and the route so far.
            terrain_overlay_pts.clear();
            if st.terrain_tool == TerrainTool::Sculpt {
                if let Some(c) = st.terrain_cursor_hit {
                    push_brush_ring(
                        &mut terrain_overlay_pts,
                        c,
                        st.terrain_brush.radius_m,
                        &st.terrain,
                    );
                }
            }
            if st.terrain_tool == TerrainTool::Route {
                push_route_preview(
                    &mut terrain_overlay_pts,
                    &st.terrain_route_points,
                    st.terrain_cursor_hit,
                );
            }
            terrain_overlay.upload(&device, &queue, &inst_bgl, &terrain_overlay_pts);
            // Read here, used by the ground-plane decision in the pass below.
            let terrain_active = st.terrain.is_active();

            // Released slots first (so a slot recycled in the same frame is not
            // dropped after it is re-filled), then the runtime's budgeted uploads, then this frame's draw
            // list. All three are plain vectors the runtime refilled under this same lock, so terrain adds
            // no synchronization and no IPC (invariant 4).
            for slot in st.terrain_drops.drain(..) {
                let i = slot as usize;
                if i < terrain_geo.len() {
                    terrain_geo[i] = None;
                    terrain_bg[i] = None;
                    // Reset the textures too, or a recycled slot inherits the previous chunk's splat map.
                    // A terrain chunk always uploads its own albedo so it never noticed; a WATER surface
                    // uploads none (its colour is in the vertex data and the dummy albedo is white), so it
                    // silently sampled whatever sand or rock the slot held before — which drew the sea as a
                    // checkerboard of bright chunk-sized patches on the live app.
                    terrain_tex[i] = (dummy_view.clone(), dummy_normal_view.clone());
                }
            }
            for up in st.terrain_uploads.drain(..) {
                let i = up.slot as usize;
                while terrain_geo.len() <= i {
                    terrain_geo.push(None);
                    terrain_bg.push(None);
                    terrain_tex.push((dummy_view.clone(), dummy_normal_view.clone()));
                }
                if up.vertices.is_empty() || up.indices.is_empty() {
                    terrain_geo[i] = None;
                    terrain_bg[i] = None;
                    continue;
                }
                let vbuf = create_init_buffer(
                    &device,
                    "terrain-v",
                    bytemuck::cast_slice(&up.vertices),
                    wgpu::BufferUsages::VERTEX,
                );
                let ibuf = create_init_buffer(
                    &device,
                    "terrain-i",
                    bytemuck::cast_slice(&up.indices),
                    wgpu::BufferUsages::INDEX,
                );
                let n_idx = up.indices.len() as u32;
                terrain_geo[i] = Some((vbuf, ibuf, n_idx));
                // A LOD switch sends no textures; the views already held stay bound, which is the whole
                // reason the upload splits geometry from textures.
                let mut retex = false;
                if let Some(a) = &up.albedo {
                    terrain_tex[i].0 = upload_tex(
                        &device,
                        &queue,
                        a,
                        TextureSemantic::for_role(Role::BaseColour),
                    );
                    retex = true;
                }
                if let Some(n) = &up.normal {
                    terrain_tex[i].1 =
                        upload_tex(&device, &queue, n, TextureSemantic::for_role(Role::Normal));
                    retex = true;
                }
                if retex || terrain_bg[i].is_none() {
                    terrain_bg[i] = Some(make_mesh_main_bg(
                        &device,
                        &mesh_inst_bgl,
                        &terrain_inst_buf,
                        &terrain_tex[i].0,
                        &dummy_mr_view,
                        &terrain_tex[i].1,
                        &dummy_mr_view,
                        &albedo_sampler,
                    ));
                }
            }
            // The draw list, packed into the shared instance buffer in draw order so chunk `k` is drawn with
            // instance range `k..k+1`.
            terrain_scratch.clear();
            terrain_slots.clear();
            for d in &st.terrain_draws {
                let i = d.slot as usize;
                if terrain_geo.get(i).and_then(Option::as_ref).is_none() {
                    continue;
                }
                if terrain_scratch.len() as u64 >= TERRAIN_INSTANCE_CAPACITY {
                    break;
                }
                terrain_slots.push(d.slot);
                terrain_scratch.push(d.instance);
            }
            if !terrain_scratch.is_empty() {
                queue.write_buffer(&terrain_inst_buf, 0, bytemuck::cast_slice(&terrain_scratch));
            }
            // M19 — scattered vegetation and props. Appended to the per-asset instance lists AFTER the
            // entity partition, so a tree draws through the same instanced call an imported prop does. Only
            // the slots that actually changed are re-uploaded, and a slot whose buffer had to GROW gets its
            // bind groups rebuilt — the group-1 binding names the buffer, so a silent reallocation here
            // would leave every draw for that asset reading a freed allocation.
            if !st.terrain_scatter.is_empty() || !scatter_slots_used.is_empty() {
                for slot in scatter_slots_used.drain(..) {
                    if let (Some(list), Some(base)) =
                        (mesh_scratch.get_mut(slot), entity_inst_len.get(slot))
                    {
                        list.truncate(*base);
                    }
                }
                for (slot, inst) in &st.terrain_scatter {
                    let Ok(s) = usize::try_from(*slot) else {
                        continue;
                    };
                    if s >= mesh_scratch.len() {
                        continue;
                    }
                    if mesh_scratch[s].len() == entity_inst_len.get(s).copied().unwrap_or(0) {
                        scatter_slots_used.push(s);
                    }
                    mesh_scratch[s].push(*inst);
                }
                for &slot in &scatter_slots_used {
                    let before = mesh_inst[slot].cap;
                    mesh_inst[slot].upload(&device, &queue, &inst_bgl, &mesh_scratch[slot]);
                    if mesh_inst[slot].cap != before {
                        let rebuild_for = |mesh: &GpuMesh| -> Vec<wgpu::BindGroup> {
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
                        };
                        if let (Some(Some(mesh)), Some(dst)) =
                            (gpu_meshes.get(slot), mesh_main_bg.get_mut(slot))
                        {
                            *dst = rebuild_for(mesh);
                        }
                        if let (Some(lods), Some(dst)) =
                            (gpu_lods.get(slot), lod_main_bg.get_mut(slot))
                        {
                            *dst = lods.iter().map(rebuild_for).collect();
                        }
                    }
                }
            }
            // M8.4 contact-debugger overlay — uploaded on its OWN revision (the debugger updates
            // independently of scene edits; while off, the buffer is empty so there's nothing to upload).
            if st.overlay_revision != cur_overlay_rev {
                cur_overlay_rev = st.overlay_revision;
                overlay.upload(&device, &queue, &inst_bgl, &st.overlay_lines);
            }
            // VFX particles change every tick while an effect runs, so they ride their own revision —
            // a scene edit must not re-upload them and a running effect must not re-upload the scene.
            if st.fx_revision != cur_fx_rev {
                cur_fx_rev = st.fx_revision;
                fx_add_buf.upload(&device, &queue, &inst_bgl, &st.fx_additive);
                fx_soft_buf.upload(&device, &queue, &inst_bgl, &st.fx_soft);
            }
            // A newly imported environment replaces the whole IBL: equirect texture, box-filtered mip
            // chain and bind group. Gated on its own revision so importing a sky costs nothing on any
            // other frame, and done HERE because this is the only place that owns the device+queue.
            if st.env_revision != cur_env_rev {
                cur_env_rev = st.env_revision;
                ibl = crate::ibl::create_with(
                    &device,
                    &queue,
                    &ibl_bgl,
                    &shadow_view,
                    &shadow_sampler,
                    st.pending_env.as_ref(),
                );
            }
            // M11.3 — upload the scene's lights on their own revision (decoupled from entity edits).
            if st.lights_revision != cur_lights_rev {
                cur_lights_rev = st.lights_revision;
                lights_buf.upload(&device, &queue, &lights_bgl, &st.lights);
            }
            let aspect = rw as f32 / rh.max(1) as f32;
            // THE COMPOSED RECTANGLE ON SCREEN, always — where the author sees the picture, and (when
            // nobody has chosen a size) what a render is written at. Published so the dialog can put a
            // size on "as on screen" instead of shrugging.
            let window_rect = frame_pixels(st.view_frame(aspect_hint).rect(), w, h);
            st.composed_pixels = [window_rect[2], window_rect[3]];
            // The rectangle the picture is composed for, decided ONCE per frame and used by the editor
            // camera, the cinematic override below and the bars drawn around it.
            //
            // ADR-177 — OFFSCREEN, THE TARGET *IS* THE FRAME. A render at a chosen size was handed a
            // rectangle of exactly the delivery shape with no docks over it, so the composition is all
            // of it: a plain centred projection, no inset, and a file that is the whole texture. On the
            // window it is the hole in the docks, inset to the delivery frame, as it has always been.
            let frame = st.drawn_frame(offscreen_on, aspect);
            // LETTERBOX. When a cutscene composes for a delivery frame, the final resolve is scissored
            // to that frame and the swapchain's own black clear becomes the bars.
            //
            // Drawn here rather than as an overlay in the editor's DOM for two reasons. The bars belong
            // to the PICTURE, not to the chrome: Play, the timeline's preview and anything else that
            // ever takes the camera all get them from the one rectangle the camera was composed for,
            // with no command, no poll and no second copy of the inset rule to drift. And they are the
            // only honest way to author a shot for a frame the author's stage is not the shape of.
            //
            // None offscreen: bars exist because a stage is the wrong shape for the delivery, and a
            // target built AT the delivery shape cannot be.
            let letterbox = st.drawn_letterbox(offscreen_on, &frame, w, h);
            // WHAT A CAPTURE WOULD KEEP: the composed frame, in pixels, whether or not there are bars.
            // Not `letterbox`, which is `None` when nothing is delivering — and the whole window is the
            // wrong answer there, because the window includes the strips behind the docks that the
            // camera was never composed for. This is the same rectangle the projection is sheared to,
            // which is what makes the file and the picture agree at every aspect.
            let capture_rect = frame_pixels(frame.rect(), rw, rh);
            // WHERE THE WINDOW SHOWS IT while a render runs at another size: the composed hole, with
            // the picture FITTED into it rather than cropped to it. So the author watches the file
            // being written instead of a frozen viewport, and the dialog's "the stage is showing each
            // frame as it is written" stays true at every output size.
            let preview_rect = offscreen_on.then_some(window_rect);
            let mut cam = camera_matrix_with(
                st.orbit,
                st.elevation,
                st.distance,
                frame,
                st.cam_target.into(),
                st.projection,
            );
            // The camera eye (world) — the PBR view direction in fs_mesh (M11.2). Carried in the Camera
            // uniform's spare `focus.yzw` (focus.x stays the focus-dim flag).
            let mut cam_eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
            let shadow_framing =
                active_shadow_framing(Vec3::from(st.cam_target), st.distance, st.cam_override);
            // M11.4 — LOOK THROUGH the active scene camera: replace the editor view-proj with the camera's
            // (its position + fov, looking at its authored target). A pure render projection (never Loro).
            if let Some(ov) = st.cam_override {
                let (eye, target, up) = resolved_camera_aim(ov, Vec3::from(st.cam_target));
                // Sheared to the same composed frame. A cutscene that solved its pose for a delivery
                // aspect and was then drawn through a window-shaped frustum would be a picture of a
                // shot the engine did not film.
                let distance = (Vec3::from(ov.pos) - target).length();
                let proj = framed_projection(
                    Projection::Perspective,
                    ov.fov_deg,
                    frame,
                    distance,
                    ov.near,
                    ov.far,
                );
                cam = proj * Mat4::look_at_rh(eye, target, up);
                cam_eye = ov.pos;
            }
            // The scene's world bounds, computed AT MOST once per frame and reused by everything that
            // needs them. Deriving them walks every vertex of every mesh, so on an imported assembly this
            // is one of the most expensive things the frame can do — and the answer only changes when the
            // scene does. Keyed on the revision plus the instance/mesh counts, so an edit invalidates it
            // but an idle camera orbit does not.
            let bounds_key = (st.revision, st.instances.len(), st.meshes.len());
            if bounds_cache.map(|(key, _)| key) != Some(bounds_key) {
                bounds_cache = Some((
                    bounds_key,
                    scene_world_bounds(&st.instances, &st.mesh_slots, &st.meshes),
                ));
            }
            let scene_bounds = bounds_cache.and_then(|(_, bounds)| bounds);

            // Size the presentation room to the same bounds, and rebuild its mesh only when those
            // bounds actually move it. `sync_stage` is the ONE place the room's dimensions are decided;
            // the draw pass reads them rather than deriving a second set of its own.
            st.sync_stage();
            if st.hall != hall_built {
                hall_built = st.hall;
                hall_buffers = st.hall.map(|hall| {
                    let mesh = hall.build();
                    diag_log!(
                        "presentation hall: {} x {} m, {:.1} m clear, {:.1} m bays, {} triangles",
                        hall.half_x * 2.0,
                        hall.half_z * 2.0,
                        hall.height,
                        hall.bay,
                        mesh.triangle_count()
                    );
                    (
                        create_init_buffer(
                            &device,
                            "hall-vbuf",
                            bytemuck::cast_slice(&mesh.vertices),
                            wgpu::BufferUsages::VERTEX,
                        ),
                        create_init_buffer(
                            &device,
                            "hall-ibuf",
                            bytemuck::cast_slice(&mesh.indices),
                            wgpu::BufferUsages::INDEX,
                        ),
                        mesh.indices.len() as u32,
                    )
                });
            }

            // Stand the ground receiver under whatever is actually in the scene. Written straight into the
            // existing single-instance buffer (never reallocated), so `ground_main_bg` stays valid.
            {
                let wanted = ground_placement(scene_bounds);
                // Re-upload only on a real change: this runs every frame and the value rarely moves.
                let moved = (wanted.1 - ground_placed.1).abs() > ground_placed.1 * 0.01
                    || (0..3).any(|i| (wanted.0[i] - ground_placed.0[i]).abs() > 0.01);
                if moved {
                    ground_placed = wanted;
                    queue.write_buffer(
                        &ground_inst.buf,
                        0,
                        bytemuck::bytes_of(&Instance {
                            center: wanted.0,
                            scale: wanted.1,
                            color: GROUND_ALBEDO,
                            highlight: 0.0,
                            rotation: IDENTITY_QUAT,
                            material: [0.0; 4],
                        }),
                    );
                }
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
                        scene_bounds,
                        shadow_framing,
                        shadow_quality.shadow_size(),
                    ),
                    st.shadow_caster.map_or(-1.0, |i| i as f32),
                )
            };
            // M9.1: regenerate the gizmo geometry at the selected entity each frame — constant pixel size,
            // and it follows the entity through a drag. Empty when nothing is selected → the pass is
            // skipped (zero cost).
            //
            // The basis is the SELECTED ENTITY'S OWN ROTATION, not identity. Passing identity here (and
            // at both drag-start call sites) made `GizmoSpace::Local` a no-op: the toolbar said
            // "Local", the handles drew along world axes, and dragging the "local X" arrow moved the
            // object along world X. A label that does not match the mathematics is worse than no
            // label, because the user calibrates on it.
            let mut gizmo_verts: Vec<Instance> = if visibility
                .allows(ViewportLayer::GizmoAndSnapChrome)
            {
                match st.selected {
                    Some(sel) if sel < st.instances.len() => {
                        let origin = st.instances[sel].center;
                        let basis = st.instances[sel].rotation;
                        let eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
                        let scale =
                            metrocalk_gizmo::pixel_scale(eye, origin, 55f32.to_radians(), 0.14);
                        st.gizmo
                            .geometry(origin, basis, scale)
                            .into_iter()
                            .map(|gv| Instance {
                                center: gv.pos,
                                scale: 0.0,
                                color: gv.color,
                                highlight: 0.0,
                                rotation: IDENTITY_QUAT,
                                material: [0.0; 4],
                            })
                            .collect()
                    }
                    _ => Vec::new(),
                }
            } else {
                Vec::new()
            };
            // M9.4: the snap **ghost** — a small cyan 3-axis cross at the nearest target during a drag
            // (constant pixel size), drawn through the same overlay pass. Empty unless snapping is live.
            if visibility.allows(ViewportLayer::GizmoAndSnapChrome) {
                if let Some(g) = st.snap_ghost {
                    let eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
                    let s = metrocalk_gizmo::pixel_scale(eye, g, 55f32.to_radians(), 0.05);
                    const GHOST: [f32; 3] = [0.2, 0.9, 0.9];
                    for ax in [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]] {
                        let mark = |o: f32| Instance {
                            center: [g[0] + ax[0] * o, g[1] + ax[1] * o, g[2] + ax[2] * o],
                            scale: 0.0,
                            color: GHOST,
                            highlight: 0.0,
                            rotation: IDENTITY_QUAT,
                            material: [0.0; 4],
                        };
                        gizmo_verts.push(mark(-1.0));
                        gizmo_verts.push(mark(1.0));
                    }
                }
            }
            // Pipe Forge live route: connected amber segments plus a cyan 3-axis cross at each authored
            // point. It shares the tiny gizmo line buffer (no mesh re-upload, no JS per frame) and is always
            // depth-visible, matching the direct-manipulation preview contract.
            if visibility.allows(ViewportLayer::GizmoAndSnapChrome) && !st.pipe_handles.is_empty() {
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
                terrain_active,
                visibility,
                st.env_source_space,
                // Read under the SAME lock as the camera and the exposure. A working space that lags
                // the frame by one tick is a frame whose textures and lights are in different spaces —
                // the mixed-state frame the atomicity requirement exists to forbid.
                st.working_space,
                letterbox,
                capture_rect,
                preview_rect,
                want_out,
                // The camera generation THIS frame is being drawn at, read under the same lock as the
                // camera itself — and as the size above. A capture asked for at a later epoch must not
                // be answered by it.
                st.pose_epoch,
            )
        };
        // The same answer the block above computed its rectangles from, in the scope the draw runs in.
        let offscreen_on = want_out.is_some();
        // The chain the size just read asks for. Built AFTER the lock, because allocating five GPU
        // textures is not something to do while the render mutex is held — and nothing between here
        // and the draw needs it.
        match want_out {
            Some(size) if offscreen.as_ref().map(|o| o.size) != Some(size) => {
                offscreen = Some(Offscreen::new(
                    &device,
                    format,
                    size,
                    Targets::create(
                        &device,
                        &target_spec,
                        size.0,
                        size.1,
                        &post_samp,
                        &post_bgl1,
                        &post_bgl2,
                        &ssao_input_bgl,
                        &black_bloom,
                    ),
                ));
                crate::diag::log(&format!(
                    "viewport: rendering offscreen at {}x{} (window is {w}x{h})",
                    size.0, size.1
                ));
            }
            Some(_) => {}
            // Dropped the moment the job clears the size: a 2160-line chain is tens of megabytes an
            // editor sitting idle has no use for.
            None => offscreen = None,
        }
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
                colour: ColourUniform::new(working_space, env_source_space),
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
        // The presentation state THIS frame's thumbnails are rendered in, read under the same lock that
        // hands out the jobs. Stamped onto every result so a consumer can tell "the picture I asked
        // for" from "a picture of a state that has since moved" — the two are indistinguishable when a
        // result carries only an entity id, which is what made a timed-out render able to satisfy a
        // later request.
        let (thumb_state, thumb_jobs): (u64, Vec<(ThumbRequest, Instance, i32)>) = {
            let mut st = shared.lock().unwrap();
            let thumb_state = st.presentation().hash();
            let n = st.thumb_requests.len().min(THUMB_PER_FRAME);
            let mut jobs = Vec::with_capacity(n);
            let mut unrenderable: Vec<ThumbResult> = Vec::new();
            for _ in 0..n {
                let req = st.thumb_requests.remove(0);
                let id = req.id.clone();
                if let Some(i) = st.ids.iter().position(|x| x == &id) {
                    let inst = st.instances[i];
                    let slot = st.mesh_slots.get(i).copied().unwrap_or(-1);
                    jobs.push((req, inst, slot));
                } else {
                    // ANSWER "no", rather than saying nothing.
                    //
                    // This used to drop the request silently. The caller then polled for the full
                    // 600 ms and returned `None`, so "this entity has no picture" and "the renderer is
                    // busy" were indistinguishable — and equally slow. That is the common case, not a
                    // corner: lights, cameras and the group/folder rows of an imported CAD assembly all
                    // have no render instance, so scrolling an outliner full of folders cost 600 ms of
                    // waiting per visible row for an answer that was known immediately.
                    unrenderable.push(ThumbResult {
                        id,
                        req: req.req,
                        state: thumb_state,
                        png: None,
                    });
                }
            }
            if !unrenderable.is_empty() {
                st.thumb_results.extend(unrenderable);
            }
            (thumb_state, jobs)
        };
        if !thumb_jobs.is_empty() {
            let mut results: Vec<ThumbResult> = Vec::with_capacity(thumb_jobs.len());
            for (req, inst, slot) in thumb_jobs {
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
                        working: working_space,
                        env_source: env_source_space,
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
                    req.size,
                );
                results.push(ThumbResult {
                    id: req.id,
                    req: req.req,
                    state: thumb_state,
                    png,
                });
            }
            let mut st = shared.lock().unwrap();
            st.thumb_results.extend(results);
            // Cap so timed-out requests can't grow the result list unbounded.
            if st.thumb_results.len() > 64 {
                let excess = st.thumb_results.len() - 64;
                st.thumb_results.drain(0..excess);
            }
        }

        // ADR-177 — A RENDER AT A CHOSEN SIZE DOES NOT NEED THE WINDOW. Nothing the file depends on is
        // drawn into the swapchain, so a surface that has stopped vending textures — minimised,
        // occluded, mid-resize — costs the author the live preview and nothing else. Without the
        // offscreen branch every one of those `continue`s was a frame the render never got, which is
        // exactly how a render used to stall behind a minimised window for thirty seconds and then
        // stop with a sentence about it.
        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&device, &config);
                if !offscreen_on {
                    continue;
                }
                None
            }
            // Every other state (`Timeout`, `Other`, ...) used to be an unlabelled 16 ms sleep, so a
            // surface that had permanently stopped vending textures presented as a frozen viewport
            // with no record anywhere. Log the *first* one and then every ~5 s, which names the
            // condition without turning a transient hitch into an unbounded log.
            other => {
                surface_stall_frames += 1;
                if surface_stall_frames == 1 || surface_stall_frames.is_multiple_of(300) {
                    crate::diag::log(&format!(
                        "viewport: surface returned {other:?} - no frame acquired                          ({surface_stall_frames} consecutive)"
                    ));
                }
                if !offscreen_on {
                    std::thread::sleep(std::time::Duration::from_millis(16));
                    continue;
                }
                None
            }
        };
        if surface_stall_frames > 0 && frame.is_some() {
            crate::diag::log(&format!(
                "viewport: surface recovered after {surface_stall_frames} stalled acquisitions"
            ));
            surface_stall_frames = 0;
        }
        let window_view = frame.as_ref().map(|f| {
            f.texture
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        // WHERE THIS FRAME IS DRAWN, and what everything below attaches to. One binding, so no pass can
        // be routed to one target while the final resolve writes another.
        // `offscreen` was just reconciled with the `want_out` the rectangles were computed from, so
        // these two cannot disagree about which size this frame is.
        debug_assert_eq!(offscreen.is_some(), offscreen_on);
        let (draw_targets, draw_view) = match offscreen.as_ref() {
            Some(o) => (&o.targets, &o.view),
            // `None` here is unreachable: the acquire above only yields no texture while an offscreen
            // render is running, and the arm above claims it when one is.
            None => (
                &targets,
                window_view
                    .as_ref()
                    .expect("the window path always acquires a frame"),
            ),
        };
        // A preview needs somewhere to draw it. Offscreen with no swapchain texture, there is nowhere.
        let preview_rect = preview_rect.filter(|_| window_view.is_some());
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
                // M19 — terrain casts too. Without this a mountain throws no shadow into the valley below
                // it, nothing on the ground is shadowed by the land it stands on, and a world made ONLY of
                // terrain has no shadows at all: the depth pass would render an empty map and every
                // receiver would sample "lit". Same instance buffer and draw order as the colour pass, so a
                // chunk's shadow can never disagree with the chunk.
                for (k, slot) in terrain_slots.iter().enumerate() {
                    let Some((vbuf, ibuf, n_idx)) =
                        terrain_geo.get(*slot as usize).and_then(Option::as_ref)
                    else {
                        continue;
                    };
                    sp.set_bind_group(1, &terrain_shadow_bg, &[]);
                    sp.set_vertex_buffer(0, vbuf.slice(..));
                    sp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    let k = k as u32;
                    sp.draw_indexed(0..*n_idx, 0, k..k + 1);
                }
            }
        }
        {
            // The scene pass, entirely in LINEAR HDR. With MSAA on it draws into the multisampled
            // Rgba16Float target and resolves — in linear light — into a single-sampled Rgba16Float; with
            // MSAA off it draws straight into that same texture. The destination is `scene_raw` when SSAO
            // will run (the AO pass then produces `hdr_scene`), otherwise `hdr_scene` itself. The
            // swapchain is NOT reachable from here under any configuration.
            let scene_dest = draw_targets
                .scene_raw
                .as_ref()
                .unwrap_or(&draw_targets.hdr_scene);
            let (scene_color, scene_resolve) = match draw_targets.msaa.as_ref() {
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
                        load: wgpu::LoadOp::Clear(clear_color(render_profile > 0.5, working_space)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &draw_targets.depth,
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
            // M19 (ADR-104) — terrain chunks, on the same pipeline with groups 0/2/3 still bound. Each
            // chunk is one draw: its own geometry, its own baked splat textures on group 1, and one
            // instance taken from the shared terrain instance buffer by draw index. Terrain therefore
            // receives the scene's real lighting, IBL and shadows for free, because it is not a special
            // case anywhere in this pass.
            for (k, slot) in terrain_slots.iter().enumerate() {
                let i = *slot as usize;
                let (Some((vbuf, ibuf, n_idx)), Some(bg)) = (
                    terrain_geo.get(i).and_then(Option::as_ref),
                    terrain_bg.get(i).and_then(Option::as_ref),
                ) else {
                    continue;
                };
                rp.set_bind_group(1, bg, &[]);
                rp.set_vertex_buffer(0, vbuf.slice(..));
                rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                let k = k as u32;
                rp.draw_indexed(0..*n_idx, 0, k..k + 1);
            }
            // M11.3 inc.3 — the ground plane (matte; receives IBL + the scene's shadows). Same mesh
            // pipeline, so groups 0/2/3 stay bound; only its instance (group 1, the untextured dummy) +
            // geometry change.
            //
            // M19: SKIPPED when a terrain is live. The placeholder ground is a flat quad at y = 0 standing in
            // for "there is no world yet", and a real terrain routinely dips below zero — a shoreline, a
            // lake bed, a canyon floor. Leaving it in draws an opaque grey lid over exactly those places. On
            // the live app it hid the entire Rolling Hills sea floor and made the terrain look like it had
            // never streamed in. The grid goes with it: it describes a surface that is no longer there.
            //
            // The presentation HALL replaces the quad rather than joining it. Its own slab runs well
            // past the wall line, so it is already the ground everywhere the ground was; drawing both
            // would put two coplanar surfaces a centimetre apart across four hundred metres, which is
            // z-fighting at exactly the grazing angles a floor is seen at.
            if !terrain_active && visibility.allows(ViewportLayer::GroundShadowReceiver) {
                if let Some((vbuf, ibuf, indices)) = hall_buffers.as_ref() {
                    rp.set_bind_group(1, &hall_main_bg, &[]);
                    rp.set_vertex_buffer(0, vbuf.slice(..));
                    rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..*indices, 0, 0..1);
                } else {
                    rp.set_bind_group(1, &ground_main_bg, &[]);
                    rp.set_vertex_buffer(0, ground_vbuf.slice(..));
                    rp.set_index_buffer(ground_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..GROUND_IDX.len() as u32, 0, 0..1);
                }
            }
            if !terrain_active && visibility.allows(ViewportLayer::Grid) {
                rp.set_pipeline(&grid_pipeline);
                rp.draw(0..GRID_VERTS, 0..1);
            }
            // Tracking lines (binding-by-intent overlay) last, with the always-pass depth state.
            if visibility.allows(ViewportLayer::TrackingLines) && lines.n > 0 {
                rp.set_pipeline(&line_pipeline);
                rp.set_bind_group(1, &lines.bg, &[]);
                rp.draw(0..lines.n, 0..1);
            }
            // ── VFX particles ────────────────────────────────────────────────────────────────────
            // Drawn INSIDE the HDR scene pass, after everything opaque and before the editor overlays:
            // inside, so bloom and the tone-map see them and an over-1.0 spark actually glows; after the
            // opaque geometry, so the depth test hides a flame that is genuinely behind a wall; before
            // the overlays, so a gizmo is never buried in smoke. Six vertices per particle, no vertex
            // buffer — `vs_particle` reads the shared instance storage buffer by `vertex_index / 6`.
            // SOFT first, then additive. Alpha blending is order-dependent and additive is not, so
            // whichever bucket goes last is the one that cannot be corrupted — and drawing smoke last
            // meant an aged puff BEHIND a flame (soft particles grow to 2.6x, and the Smoke card's own
            // blurb is "pair it with fire") multiplied the flame core down by (1-a). Glow now reads
            // through haze, which is what the previous comment claimed and the order contradicted.
            if fx_soft_buf.n > 0 {
                rp.set_pipeline(&fx_soft_pipeline);
                rp.set_bind_group(1, &fx_soft_buf.bg, &[]);
                rp.draw(0..fx_soft_buf.n * 6, 0..1);
            }
            if fx_add_buf.n > 0 {
                rp.set_pipeline(&fx_add_pipeline);
                rp.set_bind_group(1, &fx_add_buf.bg, &[]);
                rp.draw(0..fx_add_buf.n * 6, 0..1);
            }
            // M8.4 contact-debugger overlay, drawn over everything (per-segment colour, always-pass depth).
            // Skipped entirely when the debugger is off (`overlay.n == 0`) — zero per-frame cost.
            if visibility.allows(ViewportLayer::PhysicsDebugOverlay) && overlay.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &overlay.bg, &[]);
                rp.draw(0..overlay.n, 0..1);
            }
            // M11.4 — light/camera ICON glyphs (wireframe, per-segment colour, always-pass depth) so a
            // light/camera reads as an icon, not a solid placeholder cube. Empty ⇒ skipped.
            if visibility.allows(ViewportLayer::MarkerGlyphs) && markers.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &markers.bg, &[]);
                rp.draw(0..markers.n, 0..1);
            }
            // M19 — the terrain brush ring / route preview, on the same always-pass overlay pipeline as the
            // contact debugger. Empty unless a terrain tool is active, so it costs nothing otherwise.
            if visibility.allows(ViewportLayer::TerrainToolOverlay) && terrain_overlay.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &terrain_overlay.bg, &[]);
                rp.draw(0..terrain_overlay.n, 0..1);
            }
            // M9.1 transform gizmo, drawn LAST (over everything), per-segment X/Y/Z colour, always-pass
            // depth. Skipped when nothing is selected (`gizmo_buf.n == 0`) — zero per-frame cost.
            if visibility.allows(ViewportLayer::GizmoAndSnapChrome) && gizmo_buf.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &gizmo_buf.bg, &[]);
                rp.draw(0..gizmo_buf.n, 0..1);
            }
        }
        // Every remaining pass is a fullscreen triangle over the HDR scene. They share one helper so the
        // group-0 (camera) binding, the clear and the draw cannot diverge between routes.
        //
        // `into` is the rectangle of the target the triangle is mapped onto: `None` is the whole of it
        // (every pass but one), and a rectangle FITS the picture inside it — which is a different thing
        // from `scissor`, and the difference is the whole reason both exist. A scissor clips a
        // full-target draw, so it CROPS; the viewport transform places the same clip-space triangle
        // inside the rectangle, so it SCALES. Bars are the first; showing a 2582-wide render inside a
        // 908-wide hole is the second.
        let mut fullscreen = |label: &str,
                              target: &wgpu::TextureView,
                              pipeline: &wgpu::RenderPipeline,
                              bg: &wgpu::BindGroup,
                              scissor: Option<[u32; 4]>,
                              into: Option<[u32; 4]>| {
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
            // The clear above has already painted the whole attachment black; scissoring the draw is
            // what leaves that black showing as bars. Only ever set on the final resolve: an
            // intermediate HDR pass that skipped part of its target would feed the next pass garbage.
            if let Some([x, y, sw, sh]) = scissor {
                p.set_scissor_rect(x, y, sw, sh);
            }
            // A viewport does NOT clip — the oversized fullscreen triangle would still paint past the
            // rectangle — so it is always paired with the scissor that does.
            if let Some([x, y, sw, sh]) = into {
                p.set_scissor_rect(x, y, sw, sh);
                p.set_viewport(x as f32, y as f32, sw as f32, sh as f32, 0.0, 1.0);
            }
            p.set_pipeline(pipeline);
            p.set_bind_group(0, &cam_bg, &[]); // exposure + presentation profile + encode selector
            p.set_bind_group(1, bg, &[]);
            p.draw(0..3, 0..1);
        };
        // The route is DATA, not control flow: `post_route` appends the final resolve by construction, so
        // no combination of SSAO/bloom can produce a frame that misses it — and the test module asserts
        // exactly that for all four combinations, which an `if`/`else` chain could not be held to.
        let (route, route_len) =
            post_route(draw_targets.ssao_bg.is_some(), draw_targets.bloom.is_some());
        for &pass in &route[..route_len] {
            let resolved = match pass {
                // SSAO (HDR → HDR): reads `scene_raw` + the scene depth, reconstructs positions from the
                // camera uniform, and writes occlusion-attenuated LINEAR radiance into `hdr_scene`.
                PostPass::Ssao => draw_targets
                    .ssao_bg
                    .as_ref()
                    .map(|bg| (pass.label(), &draw_targets.hdr_scene, &ssao_pipeline, bg)),
                // Bloom (HDR → HDR): bright-pass → separable Gaussian (H then V). It does NOT composite;
                // the final resolve adds it, so bloom cannot be applied after tone mapping.
                PostPass::BloomBright => draw_targets
                    .bloom
                    .as_ref()
                    .map(|b| (pass.label(), &b.a, &bright_pipeline, &b.bg_bright)),
                PostPass::BloomBlurH => draw_targets
                    .bloom
                    .as_ref()
                    .map(|b| (pass.label(), &b.b, &blur_h_pipeline, &b.bg_blur_h)),
                PostPass::BloomBlurV => draw_targets
                    .bloom
                    .as_ref()
                    .map(|b| (pass.label(), &b.a, &blur_v_pipeline, &b.bg_blur_v)),
                // THE final resolve — the one pass that writes the swapchain, and the one place exposure,
                // tone mapping and the display transfer function are applied.
                PostPass::Resolve => Some((
                    pass.label(),
                    draw_view,
                    &resolve_pipeline,
                    &draw_targets.resolve_bg,
                )),
            };
            let Some((label, target, pipeline, bg)) = resolved else {
                continue;
            };
            let scissor = if matches!(pass, PostPass::Resolve) {
                letterbox
            } else {
                None
            };
            fullscreen(label, target, pipeline, bg, scissor, None);
        }
        // ADR-177 — THE FINAL RESOLVE, A SECOND TIME, INTO THE WINDOW.
        //
        // Not a blit of the file's pixels: the same tone map over the same HDR scene, sampled down into
        // the composed hole. Copying the resolved texture would have run the output transform twice and
        // shown the author a picture the file does not contain — the one bug a "preview" of a render is
        // actually capable of. Costs one fullscreen pass, and only while a render at another size runs.
        if let (Some(rect), Some(window)) = (preview_rect, window_view.as_ref()) {
            fullscreen(
                "final-resolve-preview",
                window,
                &resolve_pipeline,
                &draw_targets.resolve_bg,
                None,
                Some(rect),
            );
        }
        queue.submit([enc.finish()]);
        // ADR-175 — KEEP THIS FRAME, if anybody asked for it while it was being drawn.
        //
        // Between the submit and the present, because those are the only two lines with a swapchain
        // texture in hand that already holds the finished picture. Reading it back here — rather than
        // re-rendering the scene into an offscreen target — is what makes the file and the viewport the
        // same image by construction: same instance list, same lights, same post route, same final
        // resolve, same scissored bars. A second renderer would be a second grade, which is precisely
        // the drift `render_thumbnail`'s own comment records paying for.
        //
        // Cropped to the delivery rectangle when there is one, so the FILE is the frame the shot was
        // composed for and the bars are absent from it rather than baked into it. A capture is rare,
        // explicit, and stalls this thread while it maps — which is correct: an author who asked for a
        // still is not orbiting, and a render job wants the frames more than it wants the frame rate.
        {
            let due: Vec<FrameRequest> = {
                let mut st = shared.lock().unwrap();
                let (mine, later): (Vec<_>, Vec<_>) = std::mem::take(&mut st.frame_requests)
                    .into_iter()
                    .partition(|r| r.min_epoch <= drawn_epoch);
                st.frame_requests = later;
                mine
            };
            if !due.is_empty() {
                let rect = capture_rect;
                // ADR-177 — an offscreen render reads back ITS OWN colour target, which is created with
                // `COPY_SRC` unconditionally. Only the swapchain path depends on the adapter allowing
                // its surface to be copied, and only that path has to refuse.
                let captured = match offscreen.as_ref() {
                    Some(o) => capture_presented_frame(&device, &queue, &o.color, format, rect),
                    None if can_capture => {
                        let surface_texture = &frame
                            .as_ref()
                            .expect("the window path always acquires a frame")
                            .texture;
                        capture_presented_frame(&device, &queue, surface_texture, format, rect)
                    }
                    None => Err("this graphics adapter does not allow the viewport to be copied, so a frame cannot be written to a file".to_string()),
                };
                let mut st = shared.lock().unwrap();
                for request in due {
                    let result = match &captured {
                        Ok(png) => FrameResult {
                            req: request.req,
                            png: Some(png.clone()),
                            width: rect[2],
                            height: rect[3],
                            reason: None,
                        },
                        Err(why) => FrameResult {
                            req: request.req,
                            png: None,
                            width: 0,
                            height: 0,
                            reason: Some(why.clone()),
                        },
                    };
                    st.frame_results.push(result);
                }
                // A caller that timed out or was cancelled must not be able to grow this without bound.
                let excess = st.frame_results.len().saturating_sub(16);
                st.frame_results.drain(0..excess);
            }
        }
        if let Some(frame) = frame {
            frame.present();
        }

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
        ViewFrame::whole(aspect),
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
    frame: ViewFrame,
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
    // The near plane SCALES with the stand-off. A fixed one destroys depth precision at plant scale:
    // this is a standard `perspective_rh` with no reversed-Z, so the resolvable depth step grows as
    // z^2 / near, and at a 500 m stand-off the old fixed 0.1 m near plane resolved roughly 15 cm. Any
    // two surfaces closer together than that - a floor and the lines painted on it, two faces of an
    // imported assembly, a slab and its bay joints - became a per-pixel coin toss, which is exactly how
    // a 400 m apron came to render as blocky grey wedges.
    //
    // One per cent of the distance is the rule `metrocalk_animation::shot::cinematic_clip_planes`
    // already applies to the cutscene camera, so the editor viewport and the film now agree about depth
    // instead of disagreeing by two orders of magnitude.
    let near = (distance * 0.01).clamp(0.02, 50.0).min(far * 0.5);
    let proj = framed_projection(projection, CAMERA_FOV_DEG, frame, distance, near, far);
    proj * Mat4::look_at_rh(eye, target, Vec3::Y)
}

/// M11.3 inc.3 — the shadow-casting light's ortho view-proj, fitted to the scene's instance bounds so the
/// fixed-resolution shadow map lands its detail on the actual objects (not the whole ±40 grid). `None`
/// shadow_dir ⇒ identity: `fs_mesh`'s reprojection then falls outside the unit cube, reading as fully lit
/// (the depth pass is also skipped). wgpu NDC z ∈ [0,1] (`orthographic_rh`, matching `perspective_rh`).
fn shadow_view_proj(
    shadow_dir: Option<[f32; 3]>,
    bounds: Option<(Vec3, Vec3)>,
    framing: ShadowFraming,
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
    let Some((mut lo, hi)) = bounds else {
        return Mat4::IDENTITY;
    };
    lo.y = lo.y.min(0.0); // include the ground receiver without imposing a metre-scale minimum.
    let scene_center = (lo + hi) * 0.5;
    let scene_radius = ((hi - lo) * 0.5).length().max(0.02);
    let radius = (framing.distance * 0.9)
        .clamp((scene_radius * 0.02).max(0.02), scene_radius)
        .max(0.02);
    let mut center = if framing.lock_to_camera
        || framing.target.distance(scene_center) + radius < scene_radius * 1.15
    {
        framing.target
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

/// The OS cursor as a normalized `[0, 1]` fraction **of the render surface**.
///
/// The bug this exists to make impossible: `WebviewWindow::cursor_position()` reports the cursor in
/// **desktop** coordinates, and the live gizmo drag and terrain brush both divided it by the
/// **surface** size as though it were client-relative. The result is an error equal to the window's
/// own position on screen — so a maximised window on the primary monitor looked fine, and the same
/// drag on a window 600 px from the left edge moved the object to somewhere else entirely. It reads
/// as "the gizmo is inaccurate" rather than as a coordinate-space mistake, which is why it survived.
///
/// `client_origin` is the client area's top-left in the same desktop space (`inner_position()`);
/// `None` means the platform could not report it, in which case the raw position is the best
/// available answer and is used unchanged rather than dropping the input.
#[must_use]
pub fn surface_fraction(
    cursor: Option<tauri::PhysicalPosition<f64>>,
    client_origin: Option<(f64, f64)>,
    width: u32,
    height: u32,
) -> Option<(f32, f32)> {
    let p = cursor?;
    let (ox, oy) = client_origin.unwrap_or((0.0, 0.0));
    let w = f64::from(width.max(1));
    let h = f64::from(height.max(1));
    #[allow(clippy::cast_possible_truncation)]
    Some((((p.x - ox) / w) as f32, ((p.y - oy) / h) as f32))
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
    frame: ViewFrame,
    target: [f32; 3],
    projection: Projection,
) -> ([f32; 3], [f32; 3]) {
    let inv =
        camera_matrix_with(orbit, elevation, distance, frame, target.into(), projection).inverse();
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
    frame: ViewFrame,
    target: [f32; 3],
    projection: Projection,
) -> Option<(f32, f32)> {
    let clip = camera_matrix_with(orbit, elevation, distance, frame, target.into(), projection)
        * Vec3::from(world).extend(1.0);
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

/// ADR-177 — the picture's OWN targets, for a render at a size the window is not.
///
/// The whole chain at the chosen size — depth, MSAA, the HDR scene, the SSAO copy, the bloom
/// ping-pong — plus the colour texture the final resolve writes and the capture reads. Built by
/// [`Targets::create`], the same constructor the window's chain goes through, so there is no second
/// definition of what a frame is made of and no way for the file's route to drift from the viewport's.
///
/// It also removes the one failure a swapchain capture could not avoid: a minimised or occluded window
/// stops vending swapchain textures, and these are vended by nothing.
struct Offscreen {
    /// What it was built for. Rebuilt only when this changes.
    size: (u32, u32),
    /// The final resolve's destination — `COPY_SRC`, because the capture reads it straight back.
    color: wgpu::Texture,
    view: wgpu::TextureView,
    targets: Targets,
}

impl Offscreen {
    /// `targets` is built by the CALLER, with [`Targets::create`] and the same concrete layouts the
    /// window's chain is built from.
    ///
    /// Not forwarded through here, though that would read better: `gpu-contract-audit` resolves a bind
    /// group's layout by following the value back to where it was created, and a second function
    /// passing the same parameter along makes that chain unresolvable — three bind groups went from
    /// checked to "UNCHECKED, not clean" the moment this constructor took them. The audit is right to
    /// say so, and the fix is to not add the hop.
    fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        (w, h): (u32, u32),
        targets: Targets,
    ) -> Self {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("render-output"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // The SWAPCHAIN's format, so the one resolve pipeline writes both this and the window with
            // no second variant and no second output transform.
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = color.create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            size: (w, h),
            color,
            view,
            targets,
        }
    }
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

/// VFX particles need their own factory because [`make_pipeline`] hard-codes an opaque target, and the
/// two particle looks differ ONLY in blend state: `Additive` (src·1 + dst·1) for anything that emits
/// light, premultiplied-alpha for anything that occludes it. Both TEST depth and neither WRITES it —
/// a particle that wrote depth would punch a hole in the smoke behind it and, being unsorted, would do
/// so in whatever order the buffer happened to be in.
#[allow(
    clippy::too_many_arguments,
    reason = "a pipeline descriptor builder, matching make_pipeline next to it"
)]
fn make_particle_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    depth: &wgpu::DepthStencilState,
    blend: wgpu::BlendState,
    samples: u32,
    label: &str,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some("vs_particle"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_particle"),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(blend),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: None, // a billboard is single-sided by construction
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
        highlight: 0.0,
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
            // The SAME working space as the viewport, for the same reason the exposure and profile are
            // shared: an asset-browser thumbnail graded in a different colour space from the stage is a
            // picture of a scene that does not exist.
            colour: ColourUniform::new(thumb.working, thumb.env_source),
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
                    load: wgpu::LoadOp::Clear(clear_color(
                        thumb.render_profile > 0.5,
                        thumb.working,
                    )),
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

    // De-pad rows + reorder to RGBA8 (the swapchain format is BGRA on the Windows/Vulkan path). Then
    // PNG. Shared with the frame capture (ADR-175): two readbacks disagreeing about channel order would
    // put a red-and-blue-swapped still beside a correct thumbnail of the same scene.
    let rgba = depad_to_rgba(&data, format, padded, unpadded, size);
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

/// ADR-175 — read the picture just submitted for `texture` back off the GPU, cropped to `rect`
/// (`[x, y, width, height]` in surface pixels), and PNG-encode it.
///
/// Called between the frame's submit and its present, so `texture` is the swapchain image with the
/// finished frame already in it — nothing is re-rendered and nothing is graded a second time. Every
/// failure is a sentence rather than a `None`, because the caller's whole job is telling an author why
/// a file they asked for does not exist.
fn capture_presented_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    format: wgpu::TextureFormat,
    rect: [u32; 4],
) -> Result<Vec<u8>, String> {
    let [x, y, width, height] = rect;
    if width == 0 || height == 0 {
        return Err("the viewport has no visible area to capture".into());
    }
    // 256-byte row alignment, exactly as the thumbnail readback does it.
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame-readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("frame-capture"),
    });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
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
        return Err("the graphics driver did not hand the finished frame back".into());
    }
    let data = slice.get_mapped_range();
    let rgba = depad_to_rgba(&data, format, padded, unpadded, height);
    drop(data);
    buf.unmap();

    let mut png_bytes: Vec<u8> = Vec::new();
    {
        let mut pe = png::Encoder::new(&mut png_bytes, width, height);
        pe.set_color(png::ColorType::Rgba);
        pe.set_depth(png::BitDepth::Eight);
        let mut w = pe
            .write_header()
            .map_err(|e| format!("the PNG header could not be written: {e}"))?;
        w.write_image_data(&rgba)
            .map_err(|e| format!("the image data could not be encoded: {e}"))?;
    }
    Ok(png_bytes)
}

/// Drop the row padding a GPU readback carries and put the channels in the order PNG expects.
///
/// The swapchain is BGRA on the Windows/Vulkan path and RGBA elsewhere, and the two are the same bytes
/// in a different order — a capture that skipped this swaps every red and blue in the file, which looks
/// like a colour-management bug and is not one. Extracted from `render_thumbnail`'s copy so the two
/// readbacks cannot disagree about it, and so it is testable without a GPU.
fn depad_to_rgba(
    data: &[u8],
    format: wgpu::TextureFormat,
    padded: u32,
    unpadded: u32,
    rows: u32,
) -> Vec<u8> {
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut rgba = Vec::with_capacity((unpadded * rows) as usize);
    for row in 0..rows {
        let start = (row * padded) as usize;
        let line = &data[start..start + unpadded as usize];
        if bgra {
            for px in line.as_chunks::<4>().0 {
                rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        } else {
            rgba.extend_from_slice(line);
        }
    }
    rgba
}

/// The non-size-dependent resources the thumbnail RTT borrows from the render loop. Grouped so the
/// already-long parameter list does not grow another five entries — and so the thumbnail provably uses the
/// SAME final resolve pipeline as the swapchain frame rather than a lookalike of its own.
struct ThumbnailPass<'a> {
    formats: RenderFormats,
    samples: u32,
    /// The live presentation profile (cinematic 0 / CAD 1), so a thumbnail is graded like the stage.
    render_profile: f32,
    /// The live working space, for the same reason.
    working: metrocalk_assets::colour::WorkingSpace,
    /// And the environment's declared source space, so a thumbnail's reflections match the stage's.
    env_source: metrocalk_assets::colour::ColourSpace,
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

/// Turn a traced world-space path into the dabs a gesture records.
///
/// The same `stroke_run` the committed edit uses, so the live preview and the committed result are the same
/// geometry — a preview that approximates its own commit is worse than no preview.
fn strokes_from_path(path: &[[f32; 2]], brush: TerrainBrush) -> Vec<metrocalk_terrain::Stroke> {
    use metrocalk_terrain::recipe::StrokeKind;
    let kind = match brush.kind {
        1 => StrokeKind::Smooth,
        2 => StrokeKind::Flatten,
        3 => StrokeKind::Noise,
        _ => StrokeKind::Raise,
    };
    let b = metrocalk_terrain::sculpt::Brush {
        kind,
        radius_m: brush.radius_m,
        strength: brush.strength,
        hardness: brush.hardness,
        target_m: brush.target_m,
        spacing: 0.25,
    };
    if path.len() == 1 {
        return vec![b.dab(path[0][0], path[0][1])];
    }
    let mut out = Vec::new();
    for w in path.windows(2) {
        out.extend(metrocalk_terrain::sculpt::stroke_run(&b, w[0], w[1]));
    }
    out
}

/// A ring of line segments laid ON the terrain under the cursor, so the brush reads as painting a surface
/// rather than hovering over one. Sampling the height per segment is what makes it hug a slope.
fn push_brush_ring(
    out: &mut Vec<Instance>,
    centre: [f32; 3],
    radius_m: f32,
    rt: &crate::terrain::TerrainRuntime,
) {
    const SEGMENTS: usize = 48;
    const COLOR: [f32; 3] = [1.0, 0.72, 0.22];
    // A tabulated circle would need 48 entries; the ring is presentation-only and never feeds content, so
    // trigonometry is fine here (unlike anywhere in the field path).
    let lift = (radius_m * 0.02).max(0.05);
    let point = |i: usize| -> [f32; 3] {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        let x = centre[0] + a.cos() * radius_m;
        let z = centre[2] + a.sin() * radius_m;
        let y = rt.height_at(x, z).unwrap_or(centre[1]) + lift;
        [x, y, z]
    };
    for i in 0..SEGMENTS {
        for p in [point(i), point((i + 1) % SEGMENTS)] {
            out.push(Instance {
                center: p,
                scale: 0.0,
                color: COLOR,
                highlight: 0.0,
                rotation: IDENTITY_QUAT,
                material: [0.0; 4],
            });
        }
    }
}

/// The route being drawn: the committed points joined, a rubber band to the cursor, and a cross at each
/// point so a single point is still visible.
fn push_route_preview(out: &mut Vec<Instance>, points: &[[f32; 3]], cursor: Option<[f32; 3]>) {
    const LINE: [f32; 3] = [0.25, 0.85, 1.0];
    const MARK: [f32; 3] = [1.0, 0.95, 0.4];
    let vertex = |p: [f32; 3], c: [f32; 3]| Instance {
        center: p,
        scale: 0.0,
        color: c,
        highlight: 0.0,
        rotation: IDENTITY_QUAT,
        material: [0.0; 4],
    };
    for w in points.windows(2) {
        out.push(vertex(w[0], LINE));
        out.push(vertex(w[1], LINE));
    }
    if let (Some(last), Some(c)) = (points.last(), cursor) {
        out.push(vertex(*last, LINE));
        out.push(vertex(c, LINE));
    }
    for p in points.iter().chain(cursor.iter()) {
        let m = 1.2;
        for (dx, dz) in [(m, 0.0), (0.0, m)] {
            out.push(vertex([p[0] - dx, p[1] + 0.3, p[2] - dz], MARK));
            out.push(vertex([p[0] + dx, p[1] + 0.3, p[2] + dz], MARK));
        }
    }
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
        highlight: 0.0,
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
mod viewport_visibility_tests {
    use super::{ground_placement, ViewportLayer, ViewportVisibility};
    use glam::Vec3;

    #[test]
    fn the_ground_receiver_stands_under_whatever_was_imported() {
        // The defect this pins: the receiver was a hard-coded scale-60 quad uploaded once at startup. A
        // factory larger than 120 units hung off the edge of it into empty space, and a small part sat on
        // a slab that filled the shot. Both read as a broken render long before anyone looks at shading.
        let empty = ground_placement(None);
        assert_eq!(
            empty,
            ([0.0, -0.02, 0.0], 60.0),
            "an empty scene keeps the placeholder"
        );

        // A long weld line, well past the old fixed extent, offset from the origin.
        let lo = Vec3::new(-40.0, 0.0, 120.0);
        let hi = Vec3::new(360.0, 18.0, 200.0);
        let (centre, scale) = ground_placement(Some((lo, hi)));
        assert!(
            (centre[0] - 160.0).abs() < 0.001 && (centre[2] - 160.0).abs() < 0.001,
            "the receiver must be centred under the model, got {centre:?}"
        );
        assert_eq!(centre[1], -0.02, "it stays just below the grid plane");
        // The unit quad spans +/- scale, so the model has to fall comfortably inside it.
        assert!(
            centre[0] - scale < lo.x && centre[0] + scale > hi.x,
            "the model overhangs the receiver in X: centre {centre:?} scale {scale}"
        );
        assert!(
            centre[2] - scale < lo.z && centre[2] + scale > hi.z,
            "the model overhangs the receiver in Z: centre {centre:?} scale {scale}"
        );

        // Height must not drive the footprint: a tall mast does not need a wider floor.
        let tall = ground_placement(Some((
            Vec3::new(-1.0, 0.0, -1.0),
            Vec3::new(1.0, 400.0, 1.0),
        )));
        assert_eq!(
            tall.1, 60.0,
            "a tall thin model keeps the minimum footprint"
        );

        // Degenerate bounds must fall back rather than emit non-representable vertex positions. A scene
        // caught mid-import can transiently report these, and the quad's vertices sit at +/- scale.
        let huge = ground_placement(Some((Vec3::splat(-1.0e30), Vec3::splat(1.0e30))));
        assert!(
            huge.1 <= 100_000.0,
            "an absurd bound must be clamped, got {}",
            huge.1
        );
        assert!(huge.1.is_finite() && huge.0.iter().all(|c| c.is_finite()));

        let nan = ground_placement(Some((Vec3::splat(f32::NAN), Vec3::splat(1.0))));
        assert_eq!(
            nan,
            ([0.0, -0.02, 0.0], 60.0),
            "NaN bounds fall back to the placeholder"
        );

        let infinite = ground_placement(Some((
            Vec3::splat(f32::INFINITY),
            Vec3::splat(f32::NEG_INFINITY),
        )));
        assert_eq!(
            infinite,
            ([0.0, -0.02, 0.0], 60.0),
            "empty/infinite bounds fall back"
        );
    }

    #[test]
    fn cinematic_route_contains_no_editor_helper_passes_but_keeps_the_shadow_receiver() {
        let editor_helpers = [
            ViewportLayer::Grid,
            ViewportLayer::TrackingLines,
            ViewportLayer::MarkerGlyphs,
            ViewportLayer::PhysicsDebugOverlay,
            ViewportLayer::TerrainToolOverlay,
            ViewportLayer::GizmoAndSnapChrome,
        ];
        let cinematic = ViewportVisibility::from_cinematic(true);
        assert!(cinematic.allows(ViewportLayer::GroundShadowReceiver));
        assert!(
            editor_helpers.iter().all(|layer| !cinematic.allows(*layer)),
            "cinematic frames must contain no editor helper pass"
        );

        let editor = ViewportVisibility::from_cinematic(false);
        assert!(editor.allows(ViewportLayer::GroundShadowReceiver));
        assert!(editor_helpers.iter().all(|layer| editor.allows(*layer)));
    }
}

#[cfg(test)]
mod shadow_framing_tests {
    use super::{
        active_shadow_framing, scene_world_bounds, shadow_view_proj, CamView, Instance, SceneState,
        ShadowFraming, IDENTITY_QUAT,
    };
    use glam::Vec3;

    fn instance(center: [f32; 3], scale: f32) -> Instance {
        Instance {
            center,
            scale,
            color: [0.5; 3],
            highlight: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0; 4],
        }
    }

    fn scene() -> Vec<Instance> {
        vec![
            instance([-100.0, 10.0, 0.0], 10.0),
            instance([100.0, 10.0, 0.0], 10.0),
        ]
    }

    #[test]
    fn editor_shadow_framing_retains_the_existing_scene_centre_safety_fallback() {
        let instances = scene();
        let at_scene = active_shadow_framing(Vec3::new(0.0, 10.0, 0.0), 8.0, None);
        let far_editor_orbit = active_shadow_framing(Vec3::new(500.0, 10.0, 0.0), 8.0, None);

        let centred = shadow_view_proj(
            Some([0.35, -1.0, 0.2]),
            scene_world_bounds(&instances, &[-1, -1], &[]),
            at_scene,
            2048,
        );
        let protected = shadow_view_proj(
            Some([0.35, -1.0, 0.2]),
            scene_world_bounds(&instances, &[-1, -1], &[]),
            far_editor_orbit,
            2048,
        );

        assert_eq!(far_editor_orbit.target, Vec3::new(500.0, 10.0, 0.0));
        assert_eq!(far_editor_orbit.distance, 8.0);
        assert!(!far_editor_orbit.lock_to_camera);
        assert_eq!(
            protected, centred,
            "normal editor framing must keep the pre-existing whole-scene fallback"
        );
    }

    #[test]
    fn cinematic_shadow_framing_tracks_the_live_target_and_camera_distance() {
        let instances = scene();
        let target = Vec3::new(500.0, 10.0, 0.0);
        let camera = |pos| CamView {
            pos,
            look_at: Some(target.to_array()),
            fov_deg: 42.0,
            near: 0.05,
            far: 2_000.0,
        };
        let near_shot = active_shadow_framing(
            Vec3::new(-40.0, 0.0, 0.0),
            80.0,
            Some(camera([500.0, 16.0, 8.0])),
        );
        let wide_shot = active_shadow_framing(
            Vec3::new(-40.0, 0.0, 0.0),
            80.0,
            Some(camera([500.0, 28.0, 24.0])),
        );
        let hidden_editor_state = ShadowFraming::editor(target, near_shot.distance);

        assert_eq!(near_shot.target, target);
        assert!((near_shot.distance - 10.0).abs() < 1.0e-6);
        assert!(near_shot.lock_to_camera);
        assert!(wide_shot.distance > near_shot.distance);

        let near_vp = shadow_view_proj(
            Some([0.35, -1.0, 0.2]),
            scene_world_bounds(&instances, &[-1, -1], &[]),
            near_shot,
            2048,
        );
        let wide_vp = shadow_view_proj(
            Some([0.35, -1.0, 0.2]),
            scene_world_bounds(&instances, &[-1, -1], &[]),
            wide_shot,
            2048,
        );
        let editor_vp = shadow_view_proj(
            Some([0.35, -1.0, 0.2]),
            scene_world_bounds(&instances, &[-1, -1], &[]),
            hidden_editor_state,
            2048,
        );

        assert_ne!(
            near_vp, editor_vp,
            "a cinematic target must not fall back to the hidden editor/scene framing"
        );
        assert_ne!(
            near_vp, wide_vp,
            "moving the cinematic camera must resize its directional-shadow coverage"
        );
    }

    #[test]
    fn cinematic_coverage_diagnostics_are_empty_by_default() {
        let state = SceneState::default();
        assert_eq!(state.cinematic_subject_id, None);
        assert_eq!(state.cinematic_shot_index, None);
        assert!(state.cinematic_visited_subjects.is_empty());
    }
}

#[cfg(test)]
mod colour_policy_tests {
    use super::{Role, TextureSemantic};

    #[test]
    fn the_upload_treatment_comes_from_the_colour_policy_and_still_matches_what_the_gpu_needs() {
        // Light decodes; measurements do not. This is the assertion that stops a policy edit from
        // quietly handing a roughness map an sRGB decode — the failure nobody traces back.
        assert_eq!(
            TextureSemantic::for_role(Role::BaseColour),
            TextureSemantic::Color
        );
        assert_eq!(
            TextureSemantic::for_role(Role::Emissive),
            TextureSemantic::Color
        );

        assert_eq!(
            TextureSemantic::for_role(Role::MetallicRoughness),
            TextureSemantic::Data
        );
        assert_eq!(
            TextureSemantic::for_role(Role::Occlusion),
            TextureSemantic::Data
        );
        assert_eq!(TextureSemantic::for_role(Role::Mask), TextureSemantic::Data);

        // Raw as far as the transfer function goes, and renormalised when mipped.
        assert_eq!(
            TextureSemantic::for_role(Role::Normal),
            TextureSemantic::Normal
        );
    }

    #[test]
    fn only_the_srgb_space_reaches_the_srgb_hardware_path() {
        use metrocalk_assets::colour::{infer_for_role, ColourSpace};
        // The invariant, stated over the policy rather than over a hand-listed set of roles: a role is
        // uploaded as `Color` if and only if the policy says it is sRGB. If someone adds a role, or
        // changes what an existing one is tagged as, this still holds them to it.
        for role in [
            Role::BaseColour,
            Role::Emissive,
            Role::Normal,
            Role::MetallicRoughness,
            Role::Occlusion,
            Role::Mask,
            Role::Environment,
        ] {
            let is_srgb = infer_for_role(role).space == ColourSpace::Srgb;
            let decodes = TextureSemantic::for_role(role) == TextureSemantic::Color;
            assert_eq!(
                decodes, is_srgb,
                "{role:?} disagrees with the colour policy"
            );
        }
    }

    #[test]
    fn role_to_gpu_format_is_covered_end_to_end() {
        use super::texture_format;
        use metrocalk_assets::colour::{infer_for_role, ColourSpace};

        // The chain that actually matters: role -> policy -> semantic -> GPU FORMAT. The previous
        // version of this test stopped at the semantic and never touched the format, so flipping the
        // comparison inside `upload_tex` would have handed every data map a hardware sRGB decode with
        // this test still passing. It asserts the last link now.
        for role in [
            Role::BaseColour,
            Role::Emissive,
            Role::Normal,
            Role::MetallicRoughness,
            Role::Occlusion,
            Role::Mask,
            Role::Environment,
        ] {
            let format = texture_format(TextureSemantic::for_role(role));
            let expected = if infer_for_role(role).space == ColourSpace::Srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            };
            assert_eq!(format, expected, "{role:?} reached the wrong GPU format");
        }

        // And spelled out literally, so the test STATES the invariant rather than only deriving it: if
        // both sides of the loop above were wrong in the same way, these still catch it.
        assert_eq!(
            texture_format(TextureSemantic::Color),
            wgpu::TextureFormat::Rgba8UnormSrgb
        );
        assert_eq!(
            texture_format(TextureSemantic::Data),
            wgpu::TextureFormat::Rgba8Unorm
        );
        assert_eq!(
            texture_format(TextureSemantic::Normal),
            wgpu::TextureFormat::Rgba8Unorm
        );
    }
}

#[cfg(test)]
mod tests {
    /// Fit against the WHOLE surface -- what every framing assertion below is about, and what the
    /// renderer did before it learned that docks hide part of the window.
    fn fit_distance(half_extent: super::Vec3, aspect: f32, orbit: f32, elevation: f32) -> f32 {
        super::fit_distance_in_viewport(half_extent, aspect, orbit, elevation)
    }

    // ── cursor coordinate spaces ──────────────────────────────────────────────────────────────
    #[test]
    fn the_cursor_fraction_is_client_relative_not_desktop_relative() {
        use super::surface_fraction;
        let pos = |x: f64, y: f64| Some(tauri::PhysicalPosition::new(x, y));

        // A window whose client area starts 600 px from the desktop's left edge. The cursor at the
        // client area's exact centre must read as (0.5, 0.5) — the previous code divided the RAW
        // desktop position by the surface size and got (0.8125, 0.9629), so a gizmo drag applied a
        // movement computed for a completely different part of the scene.
        let f = surface_fraction(pos(1240.0, 620.0), Some((600.0, 350.0)), 1280, 540)
            .expect("a cursor position");
        assert!(
            (f.0 - 0.5).abs() < 1e-6 && (f.1 - 0.5).abs() < 1e-6,
            "got {f:?}"
        );

        // The old (wrong) arithmetic, kept explicit so the regression is legible rather than implied.
        let wrong = (1240.0_f32 / 1280.0, 620.0_f32 / 540.0);
        assert!(
            (wrong.0 - f.0).abs() > 0.3,
            "the desktop-relative reading really is far off ({wrong:?} vs {f:?})"
        );

        // Corners map to the corners.
        assert_eq!(
            surface_fraction(pos(600.0, 350.0), Some((600.0, 350.0)), 1280, 540),
            Some((0.0, 0.0))
        );
        assert_eq!(
            surface_fraction(pos(1880.0, 890.0), Some((600.0, 350.0)), 1280, 540),
            Some((1.0, 1.0))
        );
        // A window at the desktop origin is unaffected, which is exactly why this survived.
        assert_eq!(
            surface_fraction(pos(640.0, 270.0), Some((0.0, 0.0)), 1280, 540),
            Some((0.5, 0.5))
        );
        // No cursor ⇒ no answer; an unknown client origin falls back rather than dropping the input.
        assert_eq!(surface_fraction(None, Some((0.0, 0.0)), 1280, 540), None);
        assert_eq!(
            surface_fraction(pos(640.0, 270.0), None, 1280, 540),
            Some((0.5, 0.5))
        );
        // A zero-sized surface must not divide by zero.
        let degenerate = surface_fraction(pos(10.0, 10.0), None, 0, 0).expect("still answers");
        assert!(degenerate.0.is_finite() && degenerate.1.is_finite());
    }

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

    // ── the working space ─────────────────────────────────────────────────────────────────────────

    #[test]
    fn every_shader_compiles_and_validates() {
        // Until now nothing in CI could see a WGSL mistake: the shaders are `include_str!`, so
        // `cargo check` reads them as string literals and the first real parse happens at device
        // creation, on a machine with a GPU. Every colour conversion in this work is WGSL, so that gap
        // had to close with it. naga is the same compiler wgpu uses, at wgpu's own version.
        for (name, src) in [
            ("scene.wgsl", SHADER),
            ("post.wgsl", POST),
            ("ssao.wgsl", SSAO_SRC),
        ] {
            let module = naga::front::wgsl::parse_str(src)
                .unwrap_or_else(|e| panic!("{name} does not parse:\n{}", e.emit_to_string(src)));
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::default(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{name} does not validate: {e:?}"));
        }
    }

    #[test]
    fn the_uniform_layouts_the_shaders_declare_fit_the_buffer_the_renderer_writes() {
        // A shader may declare a PREFIX of the uniform — that is deliberate, and it is what stops the
        // scene pass from being able to name the inverse conversion. What it may never do is declare
        // MORE than the renderer writes, which wgpu rejects at draw with a message that names a byte
        // count and nothing else. Checking it here turns that into a sentence.
        let full = std::mem::size_of::<Camera>();
        for (name, src, expect) in [
            // 3 mat4 (192) + 3 vec4 (48) + the two ingress mat3s, column-padded (48 each).
            ("scene.wgsl", SHADER, 336),
            // ...the same, plus the egress mat3 (48) and the luma vec4 (16) — the whole block.
            ("post.wgsl", POST, 400),
            // No colour block at all.
            ("ssao.wgsl", SSAO_SRC, 240),
        ] {
            let module = naga::front::wgsl::parse_str(src).expect("parses");
            let cam = module
                .types
                .iter()
                .find(|(_, t)| t.name.as_deref() == Some("Camera"))
                .expect("every one of these declares a Camera");
            let naga::TypeInner::Struct { span, .. } = cam.1.inner else {
                panic!("{name}'s Camera is not a struct");
            };
            assert_eq!(
                span as usize, expect,
                "{name} declares a {span}-byte Camera; the renderer writes {full}"
            );
            assert!(
                span as usize <= full,
                "{name} declares MORE uniform than the renderer writes — wgpu rejects this at draw"
            );
        }
    }

    #[test]
    fn selecting_rec709_is_the_identity_transform() {
        // The load-bearing property of the whole design: adopting a working space changes NOTHING
        // until someone selects a different one. If this drifts, every "before" capture in the
        // repository stops being comparable to every "after" one.
        let c = ColourUniform::new(
            metrocalk_assets::colour::WorkingSpace::LinearRec709,
            metrocalk_assets::colour::ColourSpace::LinearRec709,
        );
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        assert_eq!(c.to_working, identity, "Rec.709 ingress must be untouched");
        assert_eq!(c.from_working, identity, "Rec.709 egress must be untouched");
        assert_eq!(
            [c.luma[0], c.luma[1], c.luma[2]],
            metrocalk_assets::colour::REC709_LUMINANCE
        );
        assert_eq!(c.luma[3], 0.0, "the 'non-Rec.709' flag must be off");
    }

    #[test]
    fn acescg_converts_ingress_and_returns_it_exactly() {
        use metrocalk_assets::colour::{apply, WorkingSpace};
        let w = WorkingSpace::AcesCg;
        let c = ColourUniform::new(w, metrocalk_assets::colour::ColourSpace::LinearRec709);
        assert_ne!(c.to_working[0][0], 1.0, "AP1 ingress must actually convert");
        assert_eq!(c.luma[3], 1.0, "the 'non-Rec.709' flag must be on");
        // The AP1 luminance weights, not Rec.709's. Metering AP1 values with Rec.709's coefficients is
        // the working-space bug that reads as a mis-tuned bloom threshold.
        assert_eq!(
            [c.luma[0], c.luma[1], c.luma[2]],
            metrocalk_assets::colour::AP1_LUMINANCE
        );
        // And the round trip is what makes an authored colour survive: in → shade → out returns the
        // value the author picked, so switching working space cannot shift a neutral.
        for probe in [[0.18, 0.18, 0.18], [0.8, 0.2, 0.05], [6.0, 3.0, 0.5]] {
            let there = apply(w.from_rec709(), probe);
            let back = apply(w.to_rec709(), there);
            for i in 0..3 {
                assert!(
                    (back[i] - probe[i]).abs() < 2e-3,
                    "channel {i}: {} → {} → {}",
                    probe[i],
                    there[i],
                    back[i]
                );
            }
        }
    }

    #[test]
    fn the_camera_uniform_carries_the_matrix_in_wgsl_column_order() {
        // A transposed primaries matrix is the worst kind of wrong: it still produces a plausible
        // picture. WGSL wants columns; the module writes rows so a reviewer can compare it against the
        // published table. This pins the one function that bridges them.
        use metrocalk_assets::colour::{wgsl_mat3, REC709_TO_AP1};
        let cols = wgsl_mat3(REC709_TO_AP1);
        for r in 0..3 {
            for c in 0..3 {
                assert!(
                    (cols[c][r] - REC709_TO_AP1[r][c]).abs() < 1e-9,
                    "column {c} row {r} is not the transpose"
                );
            }
        }
        for c in &cols {
            assert_eq!(c[3], 0.0, "each mat3 column pads to a vec4");
        }
    }

    #[test]
    fn every_scene_colour_ingress_is_converted_before_lighting() {
        // A SOURCE CONTRACT, because the alternative is a screenshot no one re-reads. Each of these is
        // a place where colour enters the shader; if any of them stops going through `to_working`, the
        // renderer is mixing two colour spaces in one BRDF and the picture is subtly wrong everywhere.
        let code = wgsl_code(SHADER);
        for (what, needle) in [
            (
                "base colour",
                "to_working(textureSample(base_color_tex, base_color_samp, in.uv).rgb * in.base_color)",
            ),
            ("light colour", "to_working(lt.color_intensity.xyz)"),
            ("the sky backdrop", "env_to_working(hdr)"),
        ] {
            assert!(
                code.contains(needle),
                "{what} no longer enters through the working-space conversion"
            );
        }
        // Count the two conversions separately — "env_to_working(" contains "to_working(", so a naive
        // count double-counts and would keep passing after a real ingress was dropped.
        let env_total = code.matches("env_to_working(").count();
        let env_calls = env_total - code.matches("fn env_to_working(").count();
        let plain_calls = code.matches("to_working(").count()
            - env_total
            - code.matches("fn to_working(").count();
        assert_eq!(
            env_calls, 3,
            "the environment enters in exactly three places — the sky backdrop, the diffuse irradiance \
             and the specular reflection — and all three must read the SAME declared source space"
        );
        assert_eq!(
            plain_calls, 3,
            "expected exactly three Rec.709 ingress conversions: base colour, light colour, and the \
             shared unlit authored path"
        );
        // The unlit path converts INSIDE the shared helper, so no caller can forget it.
        assert!(
            code.contains("return to_working(clamp(exposed,"),
            "the unlit authored path must convert inside `unlit_srgb_at_exposure`, not at its callers"
        );
    }

    #[test]
    fn the_scene_pass_never_leaves_the_working_space() {
        // The other half: nothing in the scene shader converts BACK. Egress is post.wgsl's job, once.
        let code = wgsl_code(SHADER);
        assert!(
            !code.contains("from_working"),
            "the scene pass must not even DECLARE the inverse — see the note on its Camera struct: a \
             field it cannot name is a rule it cannot break"
        );
        assert!(
            !code.contains("cam.luma"),
            "metering brightness is the post chain's job, in the post chain's space"
        );
        // The FORWARD curves must not appear. The INVERSE ones legitimately do — that is how an
        // authored chrome colour is placed in the scene so it renders back as itself — so the count has
        // to discount them rather than banning the substring, which is why this is subtraction and not
        // a `contains`.
        for curve in ["tonemap_aces(", "tonemap_pbr_neutral("] {
            let forward =
                code.matches(curve).count() - code.matches(&format!("inverse_{curve}")).count();
            assert_eq!(
                forward, 0,
                "the scene pass calls {curve} — the output transform has exactly one owner, and it is \
                 post.wgsl's fs_resolve"
            );
        }
        assert!(
            !code.contains("to_srgb("),
            "the scene pass must not encode for display: MSAA would then resolve gamma-encoded samples"
        );
    }

    #[test]
    fn the_output_transform_happens_exactly_once_and_after_the_working_space_conversion() {
        let code = wgsl_code(POST);
        // Exactly one conversion out of the working space, and it is bound to the view input.
        assert_eq!(
            code.matches("cam.from_working").count(),
            1,
            "the working space is left exactly once"
        );
        let seam = code
            .find("let view_input = cam.from_working * composed;")
            .expect("the seam must be the named line the comment describes");
        // Both curves are CALLED only after that line — the ordering claim, checked by position rather
        // than by trusting the comment above it.
        for curve in [
            "tonemap_pbr_neutral(view_input)",
            "tonemap_aces(view_input)",
        ] {
            let at = code.find(curve).expect("both curves run on the view input");
            assert!(
                at > seam,
                "{curve} must run AFTER the conversion out of the working space"
            );
        }
        // One display encode, guarded by the surface-format answer, and nothing else encodes.
        assert_eq!(
            code.matches("to_srgb(").count(),
            2,
            "one definition, one call"
        );
        // Bloom meters with the working space's own weights, not a hard-coded Rec.709 triple.
        assert!(
            code.contains("dot(c, cam.luma.xyz)"),
            "luminance must come from the working space"
        );
        assert!(
            !code.contains("0.2126"),
            "a hard-coded Rec.709 luminance triple is exactly the bug this replaced"
        );
    }

    #[test]
    fn the_presentation_default_matches_the_renderer() {
        // Two crates hold the same default (the assets crate cannot depend on the shell). A drift here
        // would mean a restored project renders at a different exposure than a fresh one.
        assert!(
            (metrocalk_assets::colour::PresentationState::default().exposure - DEFAULT_EXPOSURE)
                .abs()
                < 1e-9
        );
    }

    #[test]
    fn the_presentation_hash_moves_with_every_dimension_that_changes_the_picture() {
        let mut st = SceneState::default();
        let base = st.presentation().hash();
        st.working_space = metrocalk_assets::colour::WorkingSpace::AcesCg;
        let working = st.presentation().hash();
        assert_ne!(base, working, "the working space changes the picture");
        st.render_profile = 1.0;
        let view = st.presentation().hash();
        assert_ne!(working, view, "the view transform changes the picture");
        st.exposure = 1.7;
        assert_ne!(
            view,
            st.presentation().hash(),
            "exposure changes the picture"
        );
        // And it is stable: the same state hashes the same, or every thumbnail misses forever.
        let again = st.presentation().hash();
        st.exposure = 1.7;
        assert_eq!(again, st.presentation().hash());
    }

    // ── generation-safe thumbnails ────────────────────────────────────────────────────────────────

    /// Stand in for the render thread: answer request `req` with `png`, rendered in `state`.
    fn service(st: &mut SceneState, id: &str, req: u64, state: u64, png: Option<Vec<u8>>) {
        st.thumb_results.push(ThumbResult {
            id: id.into(),
            req,
            state,
            png,
        });
    }

    #[test]
    fn a_delayed_render_can_never_satisfy_the_request_that_replaced_it() {
        // THE named race, reproduced exactly: request N is in flight, its caller times out, N+1 is
        // made, and only THEN does N finish. Before request identity, N's image was sitting in the
        // id-keyed slot and N+1 took it.
        let mut st = SceneState::default();
        let (n, n_state) = st.request_thumbnail("cube", 64);
        // ...N's caller gives up (the 600 ms cap) and asks again.
        let (n1, n1_state) = st.request_thumbnail("cube", 64);
        assert_ne!(n, n1, "each request has its own identity");
        // Now the ORIGINAL render lands.
        service(&mut st, "cube", n, n_state, Some(vec![1, 2, 3]));
        assert_eq!(
            st.take_thumbnail(n1, n1_state),
            ThumbTake::Pending,
            "N+1 must not be satisfied by N's image"
        );
        // N+1's own render lands, and it gets that one.
        service(&mut st, "cube", n1, n1_state, Some(vec![9, 9, 9]));
        assert_eq!(
            st.take_thumbnail(n1, n1_state),
            ThumbTake::Ready(vec![9, 9, 9])
        );
    }

    #[test]
    fn a_timed_out_result_is_not_eligible_for_anyone() {
        let mut st = SceneState::default();
        let (n, n_state) = st.request_thumbnail("cube", 64);
        service(&mut st, "cube", n, n_state, Some(vec![1]));
        // Its caller already returned None. A LATER request for the same entity must not find it.
        let (n1, n1_state) = st.request_thumbnail("cube", 64);
        assert_eq!(st.take_thumbnail(n1, n1_state), ThumbTake::Pending);
        // The orphan ages out of the capped list; it never becomes anyone's answer.
        assert_eq!(st.thumb_results.len(), 1);
    }

    #[test]
    fn a_render_whose_state_moved_underneath_it_fails_explicitly() {
        // The A/B/A case. Ask under view A; the user switches to B before the render is serviced; the
        // picture that arrives is of B. Returning it would report B's colours as A's.
        let mut st = SceneState::default();
        let (req, want) = st.request_thumbnail("cube", 64);
        st.render_profile = 1.0; // the view transform changed while it rendered
        let rendered_in = st.presentation().hash();
        assert_ne!(want, rendered_in);
        service(&mut st, "cube", req, rendered_in, Some(vec![7]));
        assert_eq!(
            st.take_thumbnail(req, want),
            ThumbTake::StateMoved,
            "an image of another state must be reported, never returned"
        );
    }

    #[test]
    fn an_entity_with_no_picture_is_answered_immediately_and_distinguishably() {
        let mut st = SceneState::default();
        let (req, want) = st.request_thumbnail("a-light", 64);
        service(&mut st, "a-light", req, want, None);
        assert_eq!(
            st.take_thumbnail(req, want),
            ThumbTake::NoImage,
            "'no picture exists' must be distinguishable from 'not yet' and from 'wrong state'"
        );
    }

    #[test]
    fn a_duplicate_request_collapses_but_never_steals_a_pending_answer() {
        let mut st = SceneState::default();
        let (first, first_state) = st.request_thumbnail("cube", 64);
        st.request_thumbnail("cube", 64);
        assert_eq!(
            st.thumb_requests.len(),
            1,
            "a polling caller must not queue a backlog"
        );
        // The first caller is still waiting. Its answer arrives and remains ITS answer.
        service(&mut st, "cube", first, first_state, Some(vec![4]));
        assert_eq!(
            st.take_thumbnail(first, first_state),
            ThumbTake::Ready(vec![4])
        );
    }

    #[test]
    fn a_view_change_never_touches_the_document() {
        // Presentation is not scene truth. This is a structural claim: `set_presentation` writes only
        // render-state fields, and the scan below is what keeps it that way when someone adds a field.
        let src = renderer_source();
        let body = src
            .split("pub fn set_presentation(")
            .nth(1)
            .and_then(|s| s.split("\n    }").next())
            .expect("set_presentation exists");
        for banned in [
            "commit",
            "revision",
            "dirty",
            "undo",
            "log.append",
            "tx.send",
        ] {
            assert!(
                !body.contains(banned),
                "set_presentation must not touch `{banned}` — a viewing choice is not a document edit"
            );
        }
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
            let c = clear_color(cad, metrocalk_assets::colour::WorkingSpace::LinearRec709);
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
            let m = camera_matrix_with(
                orbit,
                elevation,
                distance,
                ViewFrame::whole(aspect),
                Vec3::ZERO,
                projection,
            );
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
            let m = camera_matrix_with(
                orbit,
                elevation,
                distance,
                ViewFrame::whole(aspect),
                Vec3::ZERO,
                projection,
            );
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
            highlight: 0.0,
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

    // ── the per-slot mesh-bounds memo ─────────────────────────────────────────────────────────────

    fn mesh_of(half: f32) -> metrocalk_assets::MeshGpu {
        let vertex = |x: f32, y: f32, z: f32| metrocalk_assets::MeshVertex {
            position: [x, y, z],
            normal: [0.0, 1.0, 0.0],
            color: [1.0, 1.0, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            uv: [0.0, 0.0],
            tangent: [0.0, 0.0, 0.0, 1.0],
        };
        metrocalk_assets::MeshGpu {
            vertices: vec![vertex(-half, -half, -half), vertex(half, half, half)],
            indices: vec![0, 1, 0],
            submeshes: Vec::new(),
        }
    }

    fn one_instance_scene(half: f32) -> SceneState {
        SceneState {
            meshes: vec![mesh_of(half)],
            meshes_revision: 1,
            mesh_slots: vec![0],
            instances: vec![Instance {
                center: [10.0, 0.0, -4.0],
                scale: 2.0,
                color: [1.0, 1.0, 1.0],
                highlight: 0.0,
                rotation: IDENTITY_QUAT,
                material: [0.0; 4],
            }],
            ids: vec!["1_1".into()],
            ..SceneState::default()
        }
    }

    // ── the cinematic camera's occlusion broad phase ──────────────────────────────────────────────
    //
    // The pure planner in `metrocalk-animation` is tested against a stubbed world; these are the other
    // half — that the ENGINE answers the three questions truthfully about real published instances.

    /// A row of unit boxes along +X, plus one subject box at the origin. `1_1` is the subject; the rest
    /// are the neighbours a close shot keeps ending up inside.
    fn crowded_scene(neighbours: usize) -> SceneState {
        let mut st = SceneState {
            meshes: vec![mesh_of(0.5)],
            meshes_revision: 1,
            ids_revision: 1,
            ..SceneState::default()
        };
        let box_at = |x: f32, z: f32| Instance {
            center: [x, 0.0, z],
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
            highlight: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0; 4],
        };
        st.instances.push(box_at(0.0, 0.0));
        st.ids.push("1_1".into());
        st.mesh_slots.push(0);
        for n in 0..neighbours {
            #[allow(clippy::cast_precision_loss)]
            let x = 3.0 + n as f32 * 2.0;
            st.instances.push(box_at(x, 0.0));
            st.ids.push(format!("1_{}", n + 2));
            st.mesh_slots.push(0);
        }
        st
    }

    fn subject_only() -> std::collections::HashSet<u32> {
        std::iter::once(0u32).collect()
    }

    #[test]
    fn an_empty_occlusion_structure_reports_an_open_world() {
        // Nothing built yet: the answer must be the one that changes no direction at all.
        let st = SceneState::default();
        let vantage = st.vantage(
            [0.0, 0.0, -5.0],
            [0.0; 3],
            [0.0; 3],
            1.0,
            &std::collections::HashSet::new(),
        );
        assert_eq!(vantage, metrocalk_animation::shot::Vantage::OPEN);
        assert!(vantage.acceptable());
    }

    /// The frame that came back solid dark red: the camera standing inside a machine housing.
    #[test]
    fn a_camera_standing_inside_a_neighbour_is_reported_as_buried() {
        let mut st = crowded_scene(3);
        st.sync_occlusion();
        // Dead centre of the first neighbour (a unit box at x = 3).
        let buried = st.vantage([3.0, 0.0, 0.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        assert!(buried.eye_inside, "{buried:?}");
        assert!(!buried.acceptable());
        // And well clear of everything, on the far side, it is not.
        let clear = st.vantage([0.0, 0.0, -8.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        assert!(!clear.eye_inside, "{clear:?}");
        assert!(clear.acceptable(), "{clear:?}");
    }

    /// The subject is never its own obstruction — a close shot is by definition close to it, and the
    /// pure solver already guarantees the camera stays outside it.
    #[test]
    fn the_subject_is_excluded_from_its_own_obstruction_test() {
        let mut st = crowded_scene(0);
        st.sync_occlusion();
        // Inside the subject's own box, but not exactly at its centre: an eye AT the centre is a
        // degenerate camera the query declines to answer about at all, which would make this pass for
        // the wrong reason.
        let eye = [0.2, 0.0, 0.0];
        assert!(
            !st.vantage(eye, [0.0; 3], [0.0; 3], 0.87, &subject_only())
                .eye_inside,
            "the subject must not veto its own shot"
        );
        // With nothing excluded, the very same point IS buried — so the exclusion is doing the work,
        // and the test is not passing because the query found nothing at all.
        assert!(
            st.vantage(
                eye,
                [0.0; 3],
                [0.0; 3],
                0.87,
                &std::collections::HashSet::new()
            )
            .eye_inside
        );
    }

    /// A neighbour standing between the camera and its subject blocks the view, and the fraction is a
    /// fraction — a wall reports far less clearance than open air.
    #[test]
    fn a_part_between_the_camera_and_its_subject_reduces_the_clear_fraction() {
        let mut st = crowded_scene(1);
        st.sync_occlusion();
        // Behind the neighbour at x = 3, looking back at the subject at the origin.
        let blocked = st.vantage([8.0, 0.0, 0.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        let open = st.vantage([0.0, 0.0, -8.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        assert!(
            blocked.clear < open.clear,
            "a part in the way must reduce clearance: blocked={blocked:?} open={open:?}"
        );
        assert!(
            blocked.clear < metrocalk_animation::shot::MIN_CLEAR_FRACTION,
            "a box squarely in the line of sight is not an acceptable view: {blocked:?}"
        );
        assert!((open.clear - 1.0).abs() < 1.0e-6, "{open:?}");
    }

    /// The other half of a good frame: something behind the subject to read it against. This is what
    /// separates "a machine in a factory" from "a part floating in a void".
    #[test]
    fn backing_measures_what_is_behind_the_subject_not_what_is_in_front() {
        let mut st = crowded_scene(6);
        st.sync_occlusion();
        // Looking along the row: the neighbours are all behind the subject.
        let into_the_row = st.vantage([-6.0, 0.4, 0.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        // Looking the other way, level, from just past the far end of the row: nothing behind it, and
        // the camera is high enough that the floor is not in the sample cone either.
        let out_of_the_building = st.vantage(
            [18.0, 40.0, 0.0],
            [0.0, 40.0, 0.0],
            [0.0, 40.0, 0.0],
            0.87,
            &subject_only(),
        );
        assert!(
            into_the_row.backing > out_of_the_building.backing,
            "shooting into the factory must back better than shooting out of it: \
             {into_the_row:?} vs {out_of_the_building:?}"
        );
    }

    /// The defect that made the second film's worst frames score as its best: a wall a hand's breadth in
    /// front of the lens counted as a rich backdrop. Backing has to be measured from the SUBJECT
    /// outwards, so a near surface contributes nothing to it and is reported as crowding instead.
    ///
    /// The fixture is a narrow aisle — panels close on both sides, a clear corridor down the middle to
    /// the subject. That is the frame `eye_inside` cannot catch and `clear` has no opinion about: the
    /// camera is outside everything, the subject is in plain view, and most of the picture is the side
    /// of the machine next door.
    #[test]
    fn a_surface_in_front_of_the_lens_is_crowding_and_never_backing() {
        let mut st = crowded_scene(0);
        for (index, x) in [-1.6_f32, 1.6].into_iter().enumerate() {
            st.instances.push(Instance {
                center: [x, 0.0, -6.0],
                scale: 2.0,
                color: [1.0; 3],
                highlight: 0.0,
                rotation: IDENTITY_QUAT,
                material: [0.0; 4],
            });
            st.ids.push(format!("1_9{index}"));
            st.mesh_slots.push(0);
        }
        st.ids_revision += 1;
        st.sync_occlusion();

        let aisle = st.vantage([0.0, 0.0, -8.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        assert!(
            !aisle.eye_inside,
            "the camera is outside everything: {aisle:?}"
        );
        assert!(
            (aisle.clear - 1.0).abs() < 1.0e-6,
            "the corridor to the subject is open: {aisle:?}"
        );
        assert!(
            aisle.crowded > metrocalk_animation::shot::MAX_CROWDED_FRACTION,
            "panels filling most of the frame must read as crowding: {aisle:?}"
        );
        assert!(
            !aisle.acceptable(),
            "and crowding alone must be enough to reject it: {aisle:?}"
        );
        assert!(
            aisle.backing < 0.5,
            "surfaces in FRONT of the subject must not count as what is behind it: {aisle:?}"
        );

        // Take the panels away and the same placement is fine — so the rejection is about them.
        let mut open = crowded_scene(0);
        open.sync_occlusion();
        let clear_view = open.vantage([0.0, 0.0, -8.0], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        assert!(
            clear_view.acceptable() && clear_view.crowded == 0.0,
            "{clear_view:?}"
        );
    }

    /// The presentation ground is a quad the renderer draws directly, not an instance, so it is absent
    /// from any structure built over the instance list. A bird's-eye shot is aimed straight at it.
    #[test]
    fn the_floor_counts_as_backing_even_though_it_is_not_an_instance() {
        let mut st = crowded_scene(0);
        st.sync_occlusion();
        let looking_down = st.vantage([0.0, 12.0, 0.1], [0.0; 3], [0.0; 3], 0.87, &subject_only());
        assert!(
            looking_down.backing > 0.5,
            "a shot aimed at the floor is not aimed at a void: {looking_down:?}"
        );
    }

    /// The structure is built once for an instance SET and reused. A pose change (which is every
    /// animation frame) must not trigger a rebuild, or the query would cost more than the film.
    #[test]
    fn the_occlusion_structure_is_built_once_per_instance_set() {
        let mut st = crowded_scene(4);
        assert_eq!(st.occlusion_revision, None);
        st.sync_occlusion();
        assert_eq!(st.occlusion_revision, Some(1));
        let nodes = st.occlusion.node_count();
        assert!(nodes > 0);

        // A published pose moves `revision`, never `ids_revision` — no rebuild.
        st.revision = st.revision.wrapping_add(1);
        st.instances[1].center[0] += 1.5;
        st.sync_occlusion();
        assert_eq!(st.occlusion_revision, Some(1), "a pose must not rebuild it");

        // Membership changing does.
        st.ids_revision = 2;
        st.sync_occlusion();
        assert_eq!(st.occlusion_revision, Some(2));
    }

    /// End to end, through the real planner: a close shot whose directed placement is inside a
    /// neighbour must come back re-aimed, and the re-aimed placement must actually be clear.
    #[test]
    fn the_planner_moves_a_close_shot_out_of_the_part_next_door() {
        use metrocalk_animation::shot::{
            plan_shot, solve_shot_adjusted, ShotAngle, ShotMove, ShotRecipe, ShotSize,
            SubjectSample,
        };
        let mut st = crowded_scene(1);
        // Put the neighbour exactly where a Profile close-up wants to stand, rather than guessing.
        let subject = SubjectSample {
            center: [0.0; 3],
            half_extent: [0.5; 3],
            forward: [0.0, 0.0, 1.0],
            stage: metrocalk_animation::shot::Stage::OPEN,
        };
        let shot = ShotRecipe {
            id: "s".into(),
            subject: "1_1".into(),
            size: ShotSize::Close,
            angle: ShotAngle::Profile,
            motion: ShotMove::Hold,
            amount: 0.35,
            seconds: 2.0,
        };
        let directed =
            metrocalk_animation::shot::solve_shot_eased(&shot, subject, 0.0, 16.0 / 9.0, 50.0);
        st.instances[1].center = directed.eye;
        st.instances[1].scale = 2.0;
        st.ids_revision += 1;
        st.sync_occlusion();

        let buried = st.vantage(
            directed.eye,
            directed.look_at,
            subject.center,
            subject.radius(),
            &subject_only(),
        );
        assert!(buried.eye_inside, "fixture is wrong: {buried:?}");

        let plan = plan_shot(&shot, subject, 16.0 / 9.0, 50.0, |pose, _| {
            st.vantage(
                pose.eye,
                pose.look_at,
                subject.center,
                subject.radius(),
                &subject_only(),
            )
        });
        assert!(!plan.is_authored(&shot), "{plan:?}");
        let rescued = solve_shot_adjusted(&shot, plan, subject, 0.0, 16.0 / 9.0, 50.0);
        let after = st.vantage(
            rescued.eye,
            rescued.look_at,
            subject.center,
            subject.radius(),
            &subject_only(),
        );
        assert!(
            after.acceptable(),
            "the planner returned a placement that is still no good: {after:?} from {plan:?}"
        );
    }

    #[test]
    fn the_mesh_bounds_memo_answers_exactly_what_walking_the_vertices_does() {
        let mut st = one_instance_scene(3.0);
        // Cold: the memo is empty, so this is the vertex walk.
        let cold = st.rendered_instance_bounds(0).expect("bounds");
        assert!(!st.mesh_bounds_are_current());
        st.sync_mesh_bounds();
        assert!(st.mesh_bounds_are_current());
        let warm = st.rendered_instance_bounds(0).expect("bounds");
        assert_eq!(cold, warm, "the memo must not change the answer");
        // A 3.0 half-extent mesh at instance scale 2.0, centred at (10, 0, -4).
        assert!(
            (warm.0[0] - 4.0).abs() < 1.0e-4 && (warm.1[0] - 16.0).abs() < 1.0e-4,
            "{warm:?}"
        );
    }

    #[test]
    fn replacing_the_meshes_invalidates_the_memo_rather_than_answering_from_it() {
        let mut st = one_instance_scene(3.0);
        st.sync_mesh_bounds();
        let before = st.rendered_instance_bounds(0).expect("bounds");

        // A new mesh table, announced the way every real call site announces one.
        st.meshes = vec![mesh_of(0.5)];
        st.meshes_revision = st.meshes_revision.wrapping_add(1);
        assert!(!st.mesh_bounds_are_current());
        // STALE, AND STILL RIGHT: the fallback walks the vertices rather than trusting the old table.
        let stale = st.rendered_instance_bounds(0).expect("bounds");
        assert_ne!(before, stale, "a smaller mesh must produce smaller bounds");
        st.sync_mesh_bounds();
        assert_eq!(stale, st.rendered_instance_bounds(0).expect("bounds"));
    }

    #[test]
    fn syncing_the_memo_is_idempotent_and_survives_a_mesh_table_that_shrinks() {
        let mut st = one_instance_scene(3.0);
        st.sync_mesh_bounds();
        let bounds = st.mesh_bounds.clone();
        st.sync_mesh_bounds();
        assert_eq!(bounds.len(), st.mesh_bounds.len());
        // A revision that moves without the length changing must still invalidate; a length that moves
        // without the revision changing must too. Neither may read past the end.
        st.meshes = Vec::new();
        assert!(
            !st.mesh_bounds_are_current(),
            "a shorter table is a stale memo"
        );
        // No mesh backs the slot now, so both paths fall to the unit cube: 10 - 1.0 * 2.0. The point is
        // that the stale read and the synced read agree, and that neither indexes past the end.
        let stale = st.rendered_instance_bounds(0).map(|(lo, _)| lo[0]);
        st.sync_mesh_bounds();
        assert!(st.mesh_bounds.is_empty());
        assert_eq!(stale, Some(8.0));
        assert_eq!(st.rendered_instance_bounds(0).map(|(lo, _)| lo[0]), stale);
    }

    // ── framing against the part of the window the viewer can actually see ────────────────────────

    /// Two docks open: the 3D shows through a hole that is neither the window's size nor its centre.
    /// Fractions taken from the production capture (1296 px window, viewport hole x=478..990).
    fn docked_scene() -> SceneState {
        let unit = |x: f32| Instance {
            center: [x, 0.0, 0.0],
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
            highlight: 0.0,
            rotation: IDENTITY_QUAT,
            material: [0.0, 0.5, 0.0, 0.0],
        };
        SceneState {
            surface_aspect: 1296.0 / 839.0,
            instances: vec![unit(-6.0), unit(6.0)],
            mesh_slots: vec![-1, -1],
            orbit: std::f32::consts::FRAC_PI_2,
            elevation: 0.0,
            ..SceneState::default()
        }
    }

    /// Where a world point lands on the SURFACE, in the same `[0, 1]` top-left fractions the visible
    /// rectangle is reported in.
    ///
    /// Through the shipped projection, deliberately. The previous version of this helper rebuilt a
    /// symmetric window frustum by hand so the assertion would be "about the framing" -- which was true
    /// while framing was the only thing that knew about the visible rectangle, and became a way to
    /// measure a camera the renderer no longer uses the moment the rectangle moved into the projection.
    fn project_surface(st: &SceneState, world: Vec3) -> (f32, f32) {
        project_to_screen(
            world.to_array(),
            st.orbit,
            st.elevation,
            st.distance,
            st.view_frame(st.known_surface_aspect()),
            st.cam_target,
            st.projection,
        )
        .expect("the subject is in front of the camera")
    }

    /// Whether a surface fraction lands on the MIDDLE of a rectangle.
    ///
    /// The middle and not merely inside: a subject that has drifted a third of the way to an edge is
    /// still inside the frame, and "inside" would go on passing while the composition rotted. It is
    /// also a claim that can be defeated — a projection that ignores the rectangle centres on the
    /// window, which lands inside a hole that contains the window's centre and nowhere near its
    /// middle.
    fn centred(rect: [f32; 4], point: (f32, f32)) -> bool {
        let [x, y, w, h] = rect;
        (point.0 - w.mul_add(0.5, x)).abs() < 1.0e-3 && (point.1 - h.mul_add(0.5, y)).abs() < 1.0e-3
    }

    #[test]
    fn an_unreported_viewport_frames_exactly_as_it_always_did() {
        let mut whole = docked_scene();
        let mut defaulted = docked_scene();
        // The whole surface, said explicitly, and the `[0,0,0,0]` a `Default` derive produces.
        whole.visible_rect = [0.0, 0.0, 1.0, 1.0];
        whole.frame_all();
        defaulted.frame_all();
        assert!((whole.distance - defaulted.distance).abs() < 1.0e-5);
        assert_eq!(whole.cam_target, defaulted.cam_target);
        assert_eq!(defaulted.adopted_visible_rect(), [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn a_nonsense_viewport_rectangle_falls_back_to_the_whole_surface() {
        let baseline = {
            let mut st = docked_scene();
            st.frame_all();
            (st.distance, st.cam_target)
        };
        for bad in [
            [0.0, 0.0, 0.0, 0.0],           // the Default derive
            [0.5, 0.0, 0.9, 1.0],           // runs off the right edge
            [-0.2, 0.0, 0.5, 1.0],          // starts left of the window
            [0.0, 0.0, f32::NAN, 1.0],      // not a number
            [0.0, 0.0, 0.001, 1.0],         // degenerate sliver
            [0.0, 0.0, 1.0, f32::INFINITY], // not finite
        ] {
            let mut st = docked_scene();
            st.visible_rect = bad;
            st.frame_all();
            assert!(
                (st.distance - baseline.0).abs() < 1.0e-5 && st.cam_target == baseline.1,
                "a rejected rectangle {bad:?} must frame exactly as no rectangle does"
            );
        }
    }

    #[test]
    fn framing_puts_the_scene_in_the_middle_of_the_visible_viewport_not_the_window() {
        let mut docked = docked_scene();
        // The hole from the production capture: 40% of the width, offset to the right of centre.
        docked.visible_rect = [0.369, 0.104, 0.395, 0.836];
        docked.frame_all();

        let (lo, hi) = scene_world_bounds(&docked.instances, &docked.mesh_slots, &docked.meshes)
            .expect("bounds");
        let centre = (lo + hi) * 0.5;
        let (sx, sy) = project_surface(&docked, centre);

        // The hole's own centre, which is where the subject belongs.
        let (want_x, want_y) = (0.395_f32.mul_add(0.5, 0.369), 0.836_f32.mul_add(0.5, 0.104));
        assert!(
            (sx - want_x).abs() < 1.0e-3 && (sy - want_y).abs() < 1.0e-3,
            "subject landed at ({sx}, {sy}), the visible viewport is centred on ({want_x}, {want_y})"
        );

        // And the framing this replaces: dead centre of the WINDOW, which in this layout is 66 px of
        // the captured 1296 px window left of where the viewer is looking, consistently toward the
        // left dock.
        let mut whole = docked_scene();
        whole.frame_all();
        let (was_x, _) = project_surface(&whole, centre);
        assert!(
            (was_x - 0.5).abs() < 1.0e-3,
            "a viewport that owns the window centres on it: {was_x}"
        );
        assert!(
            (was_x - want_x).abs() > 0.05,
            "and that is a real displacement, not a rounding difference: {was_x} vs {want_x}"
        );
    }

    // ── the composition holds while the picture is USED ──────────────────────────────────────────
    //
    // Framing is a user action; orbiting, zooming and opening a dock are not, and none of them
    // re-frames. While the visible rectangle was applied as an offset to the ORBIT TARGET, each of
    // these three left an offset that had been solved for a frame that no longer existed - so the
    // subject a user had just framed walked out of the picture without anybody touching the framing.

    /// The scene, framed in the production dock layout, then interfered with.
    fn framed_in_the_dock_layout() -> SceneState {
        let mut st = docked_scene();
        st.visible_rect = [0.369, 0.104, 0.395, 0.836];
        st.frame_all();
        st
    }

    fn scene_centre(st: &SceneState) -> Vec3 {
        let (lo, hi) =
            scene_world_bounds(&st.instances, &st.mesh_slots, &st.meshes).expect("bounds");
        (lo + hi) * 0.5
    }

    #[test]
    fn orbiting_a_framed_subject_leaves_it_in_the_visible_viewport() {
        let centre = scene_centre(&framed_in_the_dock_layout());
        for turn in [0.4_f32, 1.1, 2.3, -0.9] {
            let mut st = framed_in_the_dock_layout();
            st.orbit += turn;
            let point = project_surface(&st, centre);
            assert!(
                centred(st.composition_rect(), point),
                "orbiting by {turn} rad put the subject at {point:?}, off centre in {:?}",
                st.composition_rect()
            );
        }
    }

    #[test]
    fn zooming_a_framed_subject_leaves_it_in_the_visible_viewport() {
        let centre = scene_centre(&framed_in_the_dock_layout());
        for scale in [0.25_f32, 0.5, 2.0, 4.0] {
            let mut st = framed_in_the_dock_layout();
            st.distance *= scale;
            let point = project_surface(&st, centre);
            assert!(
                centred(st.composition_rect(), point),
                "zooming by {scale}x put the subject at {point:?}, off centre in {:?}",
                st.composition_rect()
            );
        }
    }

    #[test]
    fn opening_a_dock_under_a_framed_subject_leaves_it_in_the_visible_viewport() {
        // The measured layouts from the ADR-162 captures: the stage is 715 px tall with the bottom
        // dock closed and 270 px with it open, in the same 840 px window.
        let mut st = docked_scene();
        st.visible_rect = [0.369, 0.048, 0.395, 0.851];
        st.frame_all();
        let centre = scene_centre(&st);
        let framed_at = project_surface(&st, centre);
        assert!(centred(st.composition_rect(), framed_at));

        // The dock opens. Nothing else happens: no re-frame, no camera command.
        st.visible_rect = [0.369, 0.048, 0.395, 0.321];
        let point = project_surface(&st, centre);
        assert!(
            centred(st.composition_rect(), point),
            "opening the bottom dock put the subject at {point:?}, off centre in {:?}",
            st.composition_rect()
        );
    }

    // ── the delivery frame ───────────────────────────────────────────────────────────────────────

    #[test]
    fn a_full_frame_is_exactly_the_symmetric_projection_it_replaces() {
        // The shear has to be free when there is nothing to shear. Two aspect ratios and both
        // projections, against the glam constructors the render loop used before.
        for aspect in [16.0 / 9.0, 0.6_f32] {
            let frame = ViewFrame::whole(aspect);
            let got = framed_projection(
                Projection::Perspective,
                CAMERA_FOV_DEG,
                frame,
                20.0,
                0.2,
                400.0,
            );
            let want = Mat4::perspective_rh(CAMERA_FOV_DEG.to_radians(), aspect, 0.2, 400.0);
            for (a, b) in got.to_cols_array().iter().zip(want.to_cols_array()) {
                assert!(
                    (a - b).abs() < 1.0e-5,
                    "perspective at {aspect}: {got:?} vs {want:?}"
                );
            }
            let got = framed_projection(
                Projection::Orthographic,
                CAMERA_FOV_DEG,
                frame,
                20.0,
                0.2,
                400.0,
            );
            let half_h = 20.0 * (CAMERA_FOV_DEG.to_radians() * 0.5).tan();
            let want = Mat4::orthographic_rh(
                -half_h * aspect,
                half_h * aspect,
                -half_h,
                half_h,
                -400.0,
                400.0,
            );
            for (a, b) in got.to_cols_array().iter().zip(want.to_cols_array()) {
                assert!(
                    (a - b).abs() < 1.0e-4,
                    "orthographic at {aspect}: {got:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn a_sheared_frame_puts_its_own_centre_where_the_rectangle_is() {
        // The whole claim of the off-axis projection in four numbers: a point on the view axis lands at
        // the middle of the composed rectangle, wherever that rectangle is.
        for rect in [
            [0.0, 0.0, 1.0, 1.0],
            [0.369, 0.104, 0.395, 0.836],
            [0.6, 0.0, 0.4, 0.25],
            [0.0, 0.75, 0.5, 0.25],
        ] {
            let frame = ViewFrame::new(1296.0 / 839.0, rect);
            let vp = camera_matrix_with(
                0.7,
                0.3,
                25.0,
                frame,
                Vec3::new(3.0, 1.0, -2.0),
                Projection::Perspective,
            );
            let clip = vp * Vec3::new(3.0, 1.0, -2.0).extend(1.0);
            let ndc = clip.truncate() / clip.w;
            let (sx, sy) = ((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5);
            let [x, y, w, h] = rect;
            assert!(
                (sx - w.mul_add(0.5, x)).abs() < 1.0e-4 && (sy - h.mul_add(0.5, y)).abs() < 1.0e-4,
                "the orbit target landed at ({sx}, {sy}) for {rect:?}"
            );
        }
    }

    #[test]
    fn a_delivery_frame_insets_the_visible_rectangle_to_its_own_ratio() {
        // A 16:9-ish stage delivering scope letterboxes; delivering vertical pillarboxes. The check is
        // on the RESULT's aspect, not on the arithmetic, so it holds whatever the inset does.
        let surface = 1600.0 / 900.0;
        let stage = [0.2, 0.1, 0.6, 0.8]; // 1600*0.6 x 900*0.8 = 960 x 720, aspect 1.333
        for want in [2.39_f32, 16.0 / 9.0, 1.0, 9.0 / 16.0] {
            let inner = inset_to_aspect(stage, surface, want);
            assert!(
                (frame_aspect(surface, inner) - want).abs() < 1.0e-3,
                "delivering {want} gave {inner:?}, which is {}",
                frame_aspect(surface, inner)
            );
            // Inside the stage, and centred on it.
            assert!(
                inner[2] <= stage[2] + 1.0e-4 && inner[3] <= stage[3] + 1.0e-4,
                "{inner:?}"
            );
            assert!(
                ((inner[0] + inner[2] * 0.5) - (stage[0] + stage[2] * 0.5)).abs() < 1.0e-4
                    && ((inner[1] + inner[3] * 0.5) - (stage[1] + stage[3] * 0.5)).abs() < 1.0e-4,
                "the bars are not equal: {inner:?}"
            );
        }
        // The frame it already is changes nothing at all.
        assert_eq!(
            inset_to_aspect(stage, surface, frame_aspect(surface, stage)),
            stage
        );
    }

    #[test]
    fn the_delivery_frame_is_what_a_shot_is_composed_for() {
        let mut st = docked_scene();
        st.visible_rect = [0.369, 0.104, 0.395, 0.836];
        let stage = st.composition_aspect();
        assert!(
            (stage - 1296.0 / 839.0 * 0.395 / 0.836).abs() < 1.0e-3,
            "{stage}"
        );
        st.delivery_aspect = Some(2.39);
        assert_eq!(
            st.composition_rect(),
            st.adopted_visible_rect(),
            "a delivery frame with nothing holding the camera letterboxes nothing"
        );
        // A cutscene takes the viewport. This is the state `present_cinematic_moment` writes.
        st.cam_override = Some(CamView {
            pos: [0.0, 2.0, 8.0],
            look_at: Some([0.0, 1.0, 0.0]),
            fov_deg: 50.0,
            near: 0.1,
            far: 400.0,
        });
        assert!(
            (st.composition_aspect() - 2.39).abs() < 1.0e-3,
            "a cutscene delivering scope is composed for scope, not for the stage: {}",
            st.composition_aspect()
        );
        // And the bars are the difference between the two rectangles, top and bottom only.
        let [_, _, w, h] = st.composition_rect();
        assert!(
            (w - 0.395).abs() < 1.0e-4,
            "scope on a taller stage letterboxes, never pillarboxes"
        );
        assert!(h < 0.836, "and the bars are real: {h}");
        st.delivery_aspect = None;
        assert_eq!(st.composition_rect(), st.adopted_visible_rect());
    }

    // ── ADR-177: a render at a size the window is not ────────────────────────────────────────────

    /// The state a render job puts the viewport in: a scope cutscene holding the camera, on a stage
    /// with both docks open. Everything below is the difference between drawing THAT into the window
    /// and drawing it into a target of its own.
    fn cutscene_on_a_docked_stage() -> SceneState {
        let mut st = docked_scene();
        st.visible_rect = [0.369, 0.104, 0.395, 0.836];
        st.surface_aspect = 1296.0 / 839.0;
        st.delivery_aspect = Some(2.39);
        st.cam_override = Some(CamView {
            pos: [0.0, 2.0, 8.0],
            look_at: Some([0.0, 1.0, 0.0]),
            fov_deg: 50.0,
            near: 0.1,
            far: 400.0,
        });
        st
    }

    #[test]
    fn a_render_at_its_own_size_is_composed_for_the_whole_target_and_has_no_bars() {
        let st = cutscene_on_a_docked_stage();
        // The offscreen target was BUILT at the delivery shape, so the picture is all of it.
        let out = 2582.0 / 1080.0;
        let frame = st.drawn_frame(true, out);
        assert_eq!(frame.rect(), [0.0, 0.0, 1.0, 1.0]);
        assert!(
            (frame.aspect() - out).abs() < 1.0e-4,
            "and its shape is the target's: {}",
            frame.aspect()
        );
        // NO BARS. A file with them baked in is a file nothing downstream can re-frame — and there is
        // nothing for them to be the difference between, because the target is already the frame.
        assert_eq!(st.drawn_letterbox(true, &frame, 2582, 1080), None);
        // The whole texture is what gets written.
        assert_eq!(frame_pixels(frame.rect(), 2582, 1080), [0, 0, 2582, 1080]);
    }

    #[test]
    fn the_window_path_is_exactly_what_it_was_before_a_size_could_be_chosen() {
        // NEGATIVE CONTROL, and the one that matters most: the assertions above are only interesting
        // if the other branch still insets, still letterboxes and still crops.
        let st = cutscene_on_a_docked_stage();
        let stage = st.known_surface_aspect();
        let frame = st.drawn_frame(false, stage);
        assert_eq!(frame.rect(), st.composition_rect());
        assert_ne!(
            frame.rect(),
            [0.0, 0.0, 1.0, 1.0],
            "the docks are still there"
        );
        assert!(
            (frame.aspect() - 2.39).abs() < 1.0e-3,
            "and it is still composed for scope: {}",
            frame.aspect()
        );
        let bars = st
            .drawn_letterbox(false, &frame, 1296, 839)
            .expect("a scope cutscene on a 1.55 stage letterboxes");
        assert_eq!(bars, frame_pixels(frame.rect(), 1296, 839));
        assert!(bars[1] > 0 && bars[3] < 839, "and they are real: {bars:?}");
    }

    #[test]
    fn the_two_paths_compose_the_same_picture_at_different_sizes() {
        // THE PROPERTY THE WHOLE PASS RESTS ON. A 1080-line render out of a 400-pixel stage is only
        // worth anything if it is the SAME SHOT — the same lens, the same framing — at more pixels.
        // Both frames are 2.39:1, so both projections have the same field of view; only the sampling
        // rate differs.
        let st = cutscene_on_a_docked_stage();
        let window = st.drawn_frame(false, st.known_surface_aspect());
        let offscreen = st.drawn_frame(true, 2582.0 / 1080.0);
        assert!(
            (window.aspect() - offscreen.aspect()).abs() < 1.0e-3,
            "{} vs {}",
            window.aspect(),
            offscreen.aspect()
        );
        // …and the same subject lands in the same place in each. Projected through both frames from
        // ONE camera — stated here rather than read off the state, because a `SceneState` that has
        // never framed anything has `distance == 0`, and a camera standing inside its own target
        // projects to NaN rather than to a wrong answer.
        let centre = Vec3::new(3.0, 1.0, -2.0);
        for (label, frame) in [("window", window), ("offscreen", offscreen)] {
            let vp = camera_matrix_with(0.7, 0.3, 25.0, frame, centre, st.projection);
            let clip = vp * centre.extend(1.0);
            let ndc = clip.truncate() / clip.w;
            let (sx, sy) = ((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5);
            let [x, y, fw, fh] = frame.rect();
            assert!(
                (sx - fw.mul_add(0.5, x)).abs() < 2.0e-3
                    && (sy - fh.mul_add(0.5, y)).abs() < 2.0e-3,
                "{label}: the look-at landed at ({sx}, {sy}), not the centre of {:?}",
                frame.rect()
            );
        }
    }

    #[test]
    fn a_frames_pixel_rectangle_always_names_pixels_inside_the_surface() {
        assert_eq!(frame_pixels(FULL_FRAME, 1600, 900), [0, 0, 1600, 900]);
        assert_eq!(
            frame_pixels([0.25, 0.5, 0.5, 0.5], 1600, 900),
            [400, 450, 800, 450]
        );
        // A rectangle the sanitiser rejects, and one so thin the rounding could produce a zero width:
        // a scissor of zero width is a validation error, not a thin picture.
        assert_eq!(
            frame_pixels([0.0, 0.0, 0.0, 0.0], 1600, 900),
            [0, 0, 1600, 900]
        );
        let thin = frame_pixels([0.5, 0.5, 0.021, 0.021], 100, 100);
        assert!(thin[2] >= 1 && thin[3] >= 1, "{thin:?}");
        assert!(
            thin[0] + thin[2] <= 100 && thin[1] + thin[3] <= 100,
            "{thin:?}"
        );
    }

    #[test]
    fn a_narrower_viewport_needs_more_distance_to_fit_the_same_scene() {
        let fit = |rect: [f32; 4]| {
            let mut st = docked_scene();
            st.visible_rect = rect;
            st.frame_all();
            st.distance
        };
        let whole = fit([0.0, 0.0, 1.0, 1.0]);
        let half_width = fit([0.25, 0.0, 0.5, 1.0]);
        let narrow = fit([0.30, 0.10, 0.40, 0.84]);
        assert!(
            half_width > whole * 1.5,
            "halving the visible width must roughly double the distance: {whole} -> {half_width}"
        );
        assert!(narrow > half_width, "{half_width} -> {narrow}");
    }

    #[test]
    fn focusing_an_entity_also_frames_it_inside_the_visible_viewport() {
        let mut st = docked_scene();
        st.visible_rect = [0.369, 0.104, 0.395, 0.836];
        let whole_window_distance = {
            let mut wide = docked_scene();
            wide.focus_on(1);
            wide.distance
        };
        st.focus_on(1);
        assert!(
            st.distance > whole_window_distance,
            "a part framed for the window is cropped by the docks: {whole_window_distance} -> {}",
            st.distance
        );
        let (sx, _) = project_surface(&st, Vec3::new(6.0, 0.0, 0.0));
        let want_x = 0.395_f32.mul_add(0.5, 0.369);
        assert!(
            (sx - want_x).abs() < 1.0e-3,
            "the focused part landed at {sx}, the visible viewport is centred on {want_x}"
        );
    }

    #[test]
    fn framing_everything_while_focused_uses_the_new_fit_not_the_saved_distance() {
        // `clear_focus` restores the distance saved when focus was entered. Running it after the new
        // framing was written threw that framing away, so "frame all" while focused on one part gave
        // back the pre-focus distance and the scene stayed out of frame.
        let mut st = docked_scene();
        st.frame_all();
        let framed = st.distance;
        st.focus_on(1);
        assert!(st.distance < framed, "focusing gets nearer");
        st.frame_all();
        assert!(
            (st.distance - framed).abs() < 1.0e-4,
            "framing everything must recompute the fit, got {} not {framed}",
            st.distance
        );
        assert!(st.focused.is_none(), "and must leave focus mode");
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
            highlight: 0.0,
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
            highlight: 1.0,
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
            framed.highlight, 0.0,
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
                highlight: 0.0,
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
        assert_eq!(st.instances[2].highlight, 1.0);
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
        // "Everything comes back to normal": dim flag cleared + the saved framing restored.
        assert_eq!(st.focused, None);
        assert_eq!(st.distance, 60.0);
        assert_eq!(st.pre_focus_distance, None);
        // The AIM comes back too. Restoring only the distance left the camera at its old remove while
        // still pointed at the part -- so leaving focus read as the camera jumping somewhere arbitrary
        // rather than as going back to where the viewer was.
        assert_eq!(st.cam_target, [0.0, 0.0, 0.0]);
        assert_eq!(st.pre_focus_target, None);
        // Selection is intentionally retained (only the dim + framing revert).
        assert_eq!(st.selected, Some(1));
    }

    /// The camera has to come back to WHERE IT WAS, not to the origin: a viewer working across a large
    /// imported scene is rarely orbiting `[0, 0, 0]`, and a "restore" that lands there is a jump too.
    #[test]
    fn unfocus_returns_the_camera_to_the_view_the_author_left() {
        let mut st = scene(4);
        st.cam_target = [120.0, 3.5, -45.0];
        st.distance = 88.0;
        st.focus_on(2);
        assert_ne!(
            st.cam_target,
            [120.0, 3.5, -45.0],
            "focus must aim at the part"
        );
        st.clear_focus();
        assert_eq!(st.cam_target, [120.0, 3.5, -45.0]);
        assert_eq!(st.distance, 88.0);
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
                                       // Same rule for the aim: the view saved on the FIRST focus, not the second one's subject.
        assert_eq!(st.cam_target, [0.0, 0.0, 0.0]);
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
