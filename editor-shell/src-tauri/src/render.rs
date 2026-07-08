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

use glam::{Mat4, Vec3, Vec4};
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
        if self.instances.is_empty() {
            return;
        }
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for inst in &self.instances {
            // The per-instance radius floor is CM-SCALE (0.05), not the old 0.5: a centimetre-scale CAD
            // part (a mm-unit import at scale 0.001) was being inflated to a half-metre sphere, so
            // frame-all parked the camera metres away and the part rendered sub-pixel — "first-class CAD"
            // failed for small parts (the M15.9 screenshot assessment caught it).
            let r = inst.scale.max(0.05);
            for k in 0..3 {
                lo[k] = lo[k].min(inst.center[k] - r);
                hi[k] = hi[k].max(inst.center[k] + r);
            }
        }
        self.cam_target = [
            (lo[0] + hi[0]) * 0.5,
            (lo[1] + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        ];
        let radius = (0..3)
            .map(|k| (hi[k] - lo[k]) * 0.5)
            .fold(0.0_f32, f32::max)
            .max(0.02);
        // Distance floor 0.3 (3× the 0.1 near plane), not the old metre-scale 3.0: the computed fit
        // (radius × 2.4) already frames a unit prop at ~2.4; the metre floor pushed small CAD out of
        // view. Multi-object scenes have a larger radius, so the floor only affects tiny scenes.
        self.distance = (radius * 2.4).clamp(0.3, 400.0);
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
        // Center: look straight at the entity.
        self.cam_target = self.instances[i].center;
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
        let half_extent = self.instances[i].scale.max(0.02);
        self.distance = (half_extent * 4.0).clamp(0.15, 40.0);
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

/// Upload an RGBA8 texture → a sampled view. `srgb` picks `Rgba8UnormSrgb` (base-color — linearized on
/// sample, the BRDF works in linear space) vs `Rgba8Unorm` for **data** textures (metallic-roughness +
/// normal maps MUST stay linear — sampling a normal map as sRGB would corrupt the decoded vectors).
fn upload_tex(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    tex: &Texture,
    srgb: bool,
) -> wgpu::TextureView {
    let (w, h) = (tex.width.max(1), tex.height.max(1));
    let size = wgpu::Extent3d {
        width: w,
        height: h,
        depth_or_array_layers: 1,
    };
    let format = if srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh-tex"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &tex.rgba8,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        size,
    );
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
        srgb,
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
        false,
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
/// M11.4 (ADR-043) — the bloom post-processing shaders (separate module; see `post.wgsl`).
const POST: &str = include_str!("post.wgsl");
const CUBE_INDICES: [u16; 36] = [
    0, 2, 3, 0, 3, 1, 4, 5, 7, 4, 7, 6, 0, 4, 6, 0, 6, 2, 1, 3, 7, 1, 7, 5, 0, 1, 5, 0, 5, 4, 2, 6,
    7, 2, 7, 3,
];
const GRID_VERTS: u32 = (2 * (40 + 1) * 2) as u32;

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
}
// The WGSL `Camera` (3×mat4 + 2×vec4) is 224 bytes; keep this struct byte-identical or wgpu rejects the
// uniform at draw. A compile-time tripwire so a future field can't silently desync the layout.
const _: () = assert!(std::mem::size_of::<Camera>() == 224);

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
    // M11.4 (ADR-043) — MSAA anti-aliasing. Sample count chosen once from `MTK_MSAA` (`off`/`1`/`2`/`4`/`8`,
    // default 4), clamped to adapter support; 1 = off (the min-spec path: render straight to the swapchain,
    // identical to the pre-MSAA frame). The scene depth + every scene-pass pipeline are built at this count;
    // the depth-only shadow pass stays single-sample.
    let samples = msaa_sample_count(&adapter, format);
    eprintln!("[viewport] MSAA samples={samples}");
    let mut depth = make_depth(&device, w, h, samples);
    // The multisampled scene COLOR target (resolved to the swapchain at pass end); `None` when off.
    let mut msaa = make_msaa(&device, format, w, h, samples);

    // M11.4 (ADR-043) — bloom post-processing. When on, the scene renders/resolves into an offscreen target,
    // then bright-pass → separable Gaussian blur → composite (scene + bloom) → swapchain. `MTK_BLOOM=off`
    // skips it entirely (scene → swapchain, byte-identical to pre-bloom). Display-space bloom (the cheap,
    // overlay-safe path): it operates on the tonemapped scene, so it never touches the scene shaders.
    let bloom = bloom_enabled();
    eprintln!("[viewport] bloom={bloom}");
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
        bind_group_layouts: &[Some(&post_bgl1)],
        immediate_size: 0,
    });
    let post_layout2 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("post-layout-2"),
        bind_group_layouts: &[Some(&post_bgl2)],
        immediate_size: 0,
    });
    let bright_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout1,
        format,
        "fs_bright",
        "bloom-bright",
    );
    let blur_h_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout1,
        format,
        "fs_blur_h",
        "bloom-blur-h",
    );
    let blur_v_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout1,
        format,
        "fs_blur_v",
        "bloom-blur-v",
    );
    let composite_pipeline = make_post_pipeline(
        &device,
        &post_shader,
        &post_layout2,
        format,
        "fs_composite",
        "bloom-composite",
    );
    let mut bloom_t = make_bloom_targets(&device, format, w, h, &post_samp, &post_bgl1, &post_bgl2);

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
    // ── SSAO (screen-space ambient occlusion): a post pass that darkens creases / contact points so an
    // imported CAD assembly reads as solid, connected parts. When on, the scene renders to `scene_raw`, the
    // AO pass writes the darkened colour to `bloom_t.scene` (which then feeds bloom, or a blit to the
    // swapchain). `MTK_SSAO=off` restores the exact pre-SSAO path. Group 0 = camera (for depth→position),
    // group 1 = { sampler, scene colour, scene depth (multisampled) }. ─────────────────────────────────────
    //
    // The AO pass reads the scene depth through a MULTISAMPLED depth binding (`ssao.wgsl`
    // `texture_depth_multisampled_2d`), so it requires MSAA > 1. With MSAA at 1 (`MTK_MSAA=off`, or an
    // adapter that can't MSAA this surface format — the min-spec fallback) the depth texture is
    // single-sampled, and binding it would be a wgpu validation error that kills the render loop — a dead
    // black viewport at launch. Honest degrade instead: SSAO off, viewport alive.
    let ssao_requested = ssao_enabled();
    let ssao = ssao_requested && samples > 1;
    if ssao_requested && !ssao {
        eprintln!("[viewport] ssao requested but MSAA=1 (single-sampled depth) — ssao disabled");
    }
    eprintln!("[viewport] ssao={ssao}");
    let ssao_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ssao"),
        source: wgpu::ShaderSource::Wgsl(include_str!("ssao.wgsl").into()),
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
                    multisampled: true,
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
        format,
        "fs_ssao",
        "ssao",
    );
    let ssao_blit_pipeline = make_post_pipeline(
        &device,
        &ssao_shader,
        &ssao_layout,
        format,
        "fs_blit",
        "ssao-blit",
    );
    // The size-dependent SSAO resources — ONLY when the pass can run (multisampled depth exists): the
    // offscreen the scene renders to (`scene_raw`), the AO input bind group, and the blit input (SSAO on +
    // bloom off: the AO'd colour in `bloom_t.scene` → swapchain). Creating these against a single-sampled
    // depth would be the validation panic this gate exists to prevent.
    let mut ssao_t: Option<(wgpu::TextureView, wgpu::BindGroup, wgpu::BindGroup)> =
        ssao.then(|| {
            let scene_raw = make_post_tex(&device, format, w, h);
            let bg = make_ssao_bg(&device, &ssao_input_bgl, &post_samp, &scene_raw, &depth);
            let blit = make_ssao_bg(&device, &ssao_input_bgl, &post_samp, &bloom_t.scene, &depth);
            (scene_raw, bg, blit)
        });
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
        format: wgpu::TextureFormat::Depth32Float,
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
        color: [0.30, 0.31, 0.34],
        metallic: 0.0,
        roughness: 0.95,
        uv: [0.0, 0.0], // untextured (binds the white dummy)
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
        ],
    });
    let albedo_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("albedo-samp"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::Repeat,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
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
        &albedo_sampler,
    );

    let depth_state = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
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
        format,
        &depth_state,
        "vs_cube",
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
    let grid_pipeline = make_pipeline(
        &device,
        &shader,
        &grid_layout,
        format,
        &depth_state,
        "vs_grid",
        wgpu::PrimitiveTopology::LineList,
        None,
        samples,
        "grid",
    );
    // Tracking lines: same layout as the cubes (cam + a storage buffer of points), LineList topology,
    // reading `vs_line`. A separate buffer holds the line endpoints (filled from the bindings). They
    // draw with an always-pass, no-write depth state so a binding reads as an overlay the user can
    // actually see — never buried inside or behind the dense cube field (the centres they connect).
    let line_depth_state = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: Some(false),
        depth_compare: Some(wgpu::CompareFunction::Always),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let line_pipeline = make_pipeline(
        &device,
        &shader,
        &cube_layout,
        format,
        &line_depth_state,
        "vs_line",
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
        format,
        &line_depth_state,
        "vs_overlay",
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
            targets: &[Some(format.into())],
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
        format: wgpu::TextureFormat::Depth32Float,
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
            targets: &[Some(format.into())],
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
        format: wgpu::TextureFormat::Depth32Float,
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
                depth = make_depth(&device, w, h, samples);
                msaa = make_msaa(&device, format, w, h, samples);
                bloom_t =
                    make_bloom_targets(&device, format, w, h, &post_samp, &post_bgl1, &post_bgl2);
                // SSAO targets follow the window + the recreated depth/scene views (only when the pass runs).
                ssao_t = ssao.then(|| {
                    let scene_raw = make_post_tex(&device, format, w, h);
                    let bg = make_ssao_bg(&device, &ssao_input_bgl, &post_samp, &scene_raw, &depth);
                    let blit =
                        make_ssao_bg(&device, &ssao_input_bgl, &post_samp, &bloom_t.scene, &depth);
                    (scene_raw, bg, blit)
                });
            }
        }

        // Snapshot the OS cursor BEFORE locking `shared` (perf audit F11 / RC-4): on the render thread a
        // `window.cursor_position()` marshals to the main (tao) thread, so holding the hot render mutex
        // across it convoys tao behind the render frame. Read once up-front (like the resize `inner_size`
        // above); the orbit-drag and gizmo-drag branches below consume the snapshot inside the lock.
        let cursor_pos = window.cursor_position().ok();

        // read shared state; re-upload instances on revision change (picking is NOT serviced here —
        // it's done synchronously in the viewport_pick command, decoupled from the frame cadence)
        let (cam, cam_eye, focus_active, gizmo_verts, light_vp, caster_idx, exposure) = {
            let mut st = shared.lock().unwrap();
            if st.distance == 0.0 {
                st.distance = 60.0;
                st.elevation = 0.4;
            }
            if st.exposure == 0.0 {
                st.exposure = 1.0; // M11.4 — default exposure (0 = uninitialised)
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
                // Upload one `MeshGpu` (the full mesh OR a LOD) → a `GpuMesh` with per-submesh texture views.
                // M11.2: base-color is sRGB; metallic-roughness + normal are LINEAR.
                let upload_mesh = |m: &MeshGpu| -> Option<GpuMesh> {
                    if m.vertices.is_empty() || m.indices.is_empty() {
                        return None;
                    }
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
                                |t| upload_tex(&device, &queue, t, true),
                            ),
                            mr_view: sm.metallic_roughness_texture.as_ref().map_or_else(
                                || dummy_mr_view.clone(),
                                |t| upload_tex(&device, &queue, t, false),
                            ),
                            normal_view: sm.normal_texture.as_ref().map_or_else(
                                || dummy_normal_view.clone(),
                                |t| upload_tex(&device, &queue, t, false),
                            ),
                        })
                        .collect();
                    Some(GpuMesh {
                        vbuf,
                        ibuf,
                        n_idx: m.indices.len() as u32,
                        submeshes,
                    })
                };
                for m in &st.meshes {
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
            let mut cam = camera_matrix(
                st.orbit,
                st.elevation,
                st.distance,
                aspect,
                st.cam_target.into(),
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
                    shadow_view_proj(shadow_dir, &st.instances),
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
            // Focus dim flag (read under the same lock as the camera, so it can't lag the frame).
            (
                cam,
                cam_eye,
                if st.focused.is_some() { 1.0f32 } else { 0.0 },
                gizmo_verts,
                light_vp,
                caster_idx,
                st.exposure,
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
                shadow: [caster_idx, exposure, 0.0, 0.0], // .y = M11.4 post exposure (display_encode)
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
                    format,
                    samples,
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
            // M11.4 — with MSAA on, the scene draws into the multisampled target and RESOLVES to the
            // swapchain at pass end; off, it draws straight to the swapchain (resolve_target None).
            // M11.4 — the scene's final color destination: the bloom offscreen when bloom is on, else the
            // swapchain. With MSAA it's the resolve target; without, the direct color attachment.
            // When SSAO is on, the scene renders to `scene_raw` (the AO pass then writes `bloom_t.scene`);
            // else the pre-SSAO destination (bloom offscreen, or the swapchain directly).
            let scene_dest = match &ssao_t {
                Some((scene_raw, _, _)) => scene_raw,
                None if bloom => &bloom_t.scene,
                None => &view,
            };
            let (scene_color, scene_resolve) = match msaa.as_ref() {
                Some(m) => (m, Some(scene_dest)),
                None => (scene_dest, None),
            };
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: scene_color,
                    resolve_target: scene_resolve,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04,
                            g: 0.05,
                            b: 0.08,
                            a: 1.0,
                        }),
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
            // M11.3 inc.2 — skybox first: a fullscreen triangle sampling the env (group 3). Depth-write off,
            // so the grid + meshes below draw in front of it. Gives the viewport an HDR backdrop and the
            // environment the metals reflect.
            rp.set_pipeline(&sky_pipeline);
            rp.set_bind_group(1, &empty_bg, &[]);
            rp.set_bind_group(2, &empty_bg, &[]);
            rp.set_bind_group(3, &ibl.bind_group, &[]);
            rp.draw(0..3, 0..1);
            rp.set_pipeline(&grid_pipeline);
            rp.draw(0..GRID_VERTS, 0..1);
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
                let level = lod_level(cam_eye, mesh_centroid.get(slot).copied(), n_lods);
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
        // SSAO: darken creases / contact points. Reads the offscreen scene (`scene_raw`) + the scene depth,
        // reconstructs positions via the camera uniform, and writes the AO-multiplied colour to
        // `bloom_t.scene` — which then feeds the bloom chain (below) or the blit (else-branch).
        if let Some((_, ssao_bg, _)) = &ssao_t {
            let mut ap = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &bloom_t.scene,
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
            ap.set_pipeline(&ssao_pipeline);
            ap.set_bind_group(0, &cam_bg, &[]);
            ap.set_bind_group(1, ssao_bg, &[]);
            ap.draw(0..3, 0..1);
        }
        // M11.4 (ADR-043) — bloom post chain: bright-pass → separable Gaussian (H then V) → composite, each
        // a fullscreen triangle. The scene pass wrote `bloom_t.scene`; composite writes the swapchain.
        if bloom {
            let mut post = |target: &wgpu::TextureView,
                            pipeline: &wgpu::RenderPipeline,
                            bg: &wgpu::BindGroup| {
                let mut p = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("bloom"),
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
                p.set_bind_group(0, bg, &[]);
                p.draw(0..3, 0..1);
            };
            post(&bloom_t.a, &bright_pipeline, &bloom_t.bg_bright); // scene → bloom_a (extract highlights)
            post(&bloom_t.b, &blur_h_pipeline, &bloom_t.bg_blur_h); // a → b (blur horizontal)
            post(&bloom_t.a, &blur_v_pipeline, &bloom_t.bg_blur_v); // b → a (blur vertical)
            post(&view, &composite_pipeline, &bloom_t.bg_composite); // scene + bloom_a → swapchain
        } else if let Some((_, _, ssao_blit_bg)) = &ssao_t {
            // SSAO on + bloom off: the AO'd colour is in `bloom_t.scene`; blit it to the swapchain.
            let mut bp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ssao-blit"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            bp.set_pipeline(&ssao_blit_pipeline);
            bp.set_bind_group(0, &cam_bg, &[]);
            bp.set_bind_group(1, ssao_blit_bg, &[]);
            bp.draw(0..3, 0..1);
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
    let offset = Vec3::new(
        orbit.cos() * distance * elevation.cos(),
        distance * elevation.sin(),
        orbit.sin() * distance * elevation.cos(),
    );
    let eye = target + offset;
    let proj = Mat4::perspective_rh(55f32.to_radians(), aspect, 0.1, distance * 8.0 + 100.0);
    proj * Mat4::look_at_rh(eye, target, Vec3::Y)
}

/// M11.3 inc.3 — the shadow-casting light's ortho view-proj, fitted to the scene's instance bounds so the
/// fixed-resolution shadow map lands its detail on the actual objects (not the whole ±40 grid). `None`
/// shadow_dir ⇒ identity: `fs_mesh`'s reprojection then falls outside the unit cube, reading as fully lit
/// (the depth pass is also skipped). wgpu NDC z ∈ [0,1] (`orthographic_rh`, matching `perspective_rh`).
fn shadow_view_proj(shadow_dir: Option<[f32; 3]>, instances: &[Instance]) -> Mat4 {
    let Some(dir) = shadow_dir else {
        return Mat4::IDENTITY;
    };
    let dir = Vec3::from(dir).normalize_or_zero();
    if dir.length_squared() < 1e-6 {
        return Mat4::IDENTITY;
    }
    // Bound the instances (centre + radius); cap the radius so a sprawling scene doesn't make shadows too
    // coarse, and floor it so a single object still gets a sane frustum. Empty scene ⇒ a small origin box.
    let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
    for inst in instances {
        let r = inst.scale.max(0.5);
        for k in 0..3 {
            lo[k] = lo[k].min(inst.center[k] - r);
            hi[k] = hi[k].max(inst.center[k] + r);
        }
    }
    let (center, radius) = if lo[0].is_finite() {
        let lo_y = lo[1].min(-0.1); // pull the box down to the ground plane (y≈0) so it receives shadows
        let c = Vec3::new(
            (lo[0] + hi[0]) * 0.5,
            (lo_y + hi[1]) * 0.5,
            (lo[2] + hi[2]) * 0.5,
        );
        let ext = Vec3::new(hi[0] - lo[0], hi[1] - lo_y, hi[2] - lo[2]) * 0.5;
        (c, ext.length().clamp(2.0, 30.0))
    } else {
        (Vec3::ZERO, 8.0)
    };
    let up = if dir.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let eye = center - dir * (radius * 2.0);
    let view = Mat4::look_at_rh(eye, center, up);
    let proj = Mat4::orthographic_rh(-radius, radius, -radius, radius, 0.05, radius * 4.0);
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
) -> ([f32; 3], [f32; 3]) {
    let inv = camera_matrix(orbit, elevation, distance, aspect, target.into()).inverse();
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
) -> Option<(f32, f32)> {
    let clip = camera_matrix(orbit, elevation, distance, aspect, target.into())
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
            format: wgpu::TextureFormat::Depth32Float,
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
            entry_point: Some("fs_main"),
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

/// M11.4 (ADR-043) — MSAA sample count from `MTK_MSAA` (`off`/`1`/`2`/`4`/`8`, default 4), clamped to the
/// highest count ≤ requested that the adapter supports for `format` (so a downlevel GPU still gets some AA,
/// or falls back to 1 = off). `1` means no multisample target + no resolve — the pre-MSAA path.
fn msaa_sample_count(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> u32 {
    let requested = match std::env::var("MTK_MSAA").ok().as_deref() {
        Some("off" | "1") => 1,
        Some("2") => 2,
        Some("8") => 8,
        _ => 4,
    };
    if requested <= 1 {
        return 1;
    }
    let flags = adapter.get_texture_format_features(format).flags;
    for c in [requested, 4, 2] {
        if c <= requested && flags.sample_count_supported(c) {
            return c;
        }
    }
    1
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
/// texture alive (wgpu resources are ref-counted), so the texture handle isn't returned.
fn make_post_tex(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("post-target"),
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
#[allow(clippy::too_many_arguments)]
fn render_thumbnail(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    samples: u32,
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
    let size = size.clamp(32, 256);
    // Frame a COPY of the entity at the origin — a consistent "portrait" regardless of its world position.
    let scale = instance.scale.max(0.1);
    let framed = Instance {
        center: [0.0, 0.0, 0.0],
        scale,
        color: instance.color,
        selected: 0.0,
        rotation: instance.rotation,
        material: instance.material,
    };
    let dist = (scale * 3.2).clamp(2.0, 200.0);
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
            shadow: [-1.0, 1.0, 0.0, 0.0], // caster -1 = none; exposure 1.0
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

    // Offscreen targets: render at the scene `samples` count (so the scene pipelines match), resolving into a
    // single-sample COPY_SRC target we read back. With MSAA off, render straight into the resolve target.
    let resolve_tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("thumb-resolve"),
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
    let resolve_view = resolve_tex.create_view(&wgpu::TextureViewDescriptor::default());
    let msaa_view = (samples > 1).then(|| make_post_tex_msaa(device, format, size, size, samples));
    let depth = make_depth(device, size, size, samples);
    let (color_view, resolve_target) = match &msaa_view {
        Some(m) => (m, Some(&resolve_view)),
        None => (&resolve_view, None),
    };

    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("thumb-enc"),
    });
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("thumb"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color_view,
                resolve_target,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: 0.04,
                        g: 0.05,
                        b: 0.08,
                        a: 1.0,
                    }),
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
            texture: &resolve_tex,
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
/// the given fragment entry, writing `format`.
fn make_post_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
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
            targets: &[Some(format.into())],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

/// M11.4 — the bloom render targets + their bind groups. `scene` is the full-res offscreen the scene pass
/// renders/resolves into; `a`/`b` are half-res ping-pong buffers. Recreated on resize (texture views change).
struct BloomTargets {
    scene: wgpu::TextureView,
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    bg_bright: wgpu::BindGroup,    // reads scene → bloom_a
    bg_blur_h: wgpu::BindGroup,    // reads a → b
    bg_blur_v: wgpu::BindGroup,    // reads b → a
    bg_composite: wgpu::BindGroup, // reads scene + a → swapchain
}

fn make_bloom_targets(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
    samp: &wgpu::Sampler,
    bgl1: &wgpu::BindGroupLayout,
    bgl2: &wgpu::BindGroupLayout,
) -> BloomTargets {
    let scene = make_post_tex(device, format, w, h);
    let a = make_post_tex(device, format, w / 2, h / 2);
    let b = make_post_tex(device, format, w / 2, h / 2);
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
    let bg_composite = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("bloom-composite"),
        layout: bgl2,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(samp),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&scene),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&a),
            },
        ],
    });
    BloomTargets {
        bg_bright: bg1("bloom-bright", &scene),
        bg_blur_h: bg1("bloom-blur-h", &a),
        bg_blur_v: bg1("bloom-blur-v", &b),
        bg_composite,
        scene,
        a,
        b,
    }
}

/// M11.1 (ADR-040) — choose a LOD level from the camera distance to an asset's instance centroid: nearer =
/// finer. `0` = the full mesh; `1..=n_lods` = progressively coarser (the normalized meshes are ~1 unit, so
/// the thresholds are world distances). Clamped to the LODs that actually exist; `0` if there are none or no
/// centroid.
fn lod_level(cam_eye: [f32; 3], centroid: Option<[f32; 3]>, n_lods: usize) -> usize {
    if n_lods == 0 {
        return 0;
    }
    let Some(c) = centroid else {
        return 0;
    };
    let d2 = (0..3).map(|k| (cam_eye[k] - c[k]).powi(2)).sum::<f32>();
    let level = if d2 < 16.0 * 16.0 {
        0
    } else if d2 < 34.0 * 34.0 {
        1
    } else {
        2
    };
    level.min(n_lods)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lod_level_picks_coarser_with_distance_and_clamps() {
        let c = Some([0.0, 0.0, 0.0]);
        assert_eq!(lod_level([0.0, 0.0, 5.0], c, 2), 0, "near → full");
        assert_eq!(lod_level([0.0, 0.0, 20.0], c, 2), 1, "mid → LOD-1");
        assert_eq!(lod_level([0.0, 0.0, 50.0], c, 2), 2, "far → LOD-2");
        assert_eq!(
            lod_level([0.0, 0.0, 50.0], c, 1),
            1,
            "clamped to the available LODs"
        );
        assert_eq!(lod_level([0.0, 0.0, 50.0], c, 0), 0, "no LODs → full");
        assert_eq!(
            lod_level([0.0, 0.0, 50.0], None, 2),
            0,
            "no centroid → full"
        );
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
