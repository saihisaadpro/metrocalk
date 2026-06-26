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
use metrocalk_assets::{MeshGpu, MeshVertex};
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

/// One renderable entity instance. 48 bytes, std430-clean (matches the WGSL `Instance`). `rotation` is a
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

/// The identity quaternion (no rotation) — the default for `Instance::rotation`.
pub const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

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
    /// M8.4 contact-debugger overlay endpoints — same `LineList` carrier as [`Self::line_points`], drawn by
    /// the same always-pass line pass (each consecutive pair is one segment). **Empty by default** (the
    /// debugger is off → the overlay pass is skipped → zero per-frame cost). Updated independently from
    /// `instances`, on its own [`Self::overlay_revision`].
    pub overlay_lines: Vec<Instance>,
    /// Bump when `overlay_lines` changes so the loop re-uploads them (decoupled from `revision`).
    pub overlay_revision: u64,
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
            let r = inst.scale.max(0.5);
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
            .max(1.0);
        self.distance = (radius * 2.4).clamp(6.0, 400.0);
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
        let half_extent = self.instances[i].scale.max(0.5);
        self.distance = (half_extent * 4.0).clamp(6.0, 40.0);
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
/// one bound vertex/index buffer per asset, drawn instanced across the entities that use it).
struct GpuMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    n_idx: u32,
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

const SHADER: &str = include_str!("scene.wgsl");
const CUBE_INDICES: [u16; 36] = [
    0, 2, 3, 0, 3, 1, 4, 5, 7, 4, 7, 6, 0, 4, 6, 0, 6, 2, 1, 3, 7, 1, 7, 5, 0, 1, 5, 0, 5, 4, 2, 6,
    7, 2, 7, 3,
];
const GRID_VERTS: u32 = (2 * (40 + 1) * 2) as u32;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
    /// Focus-mode flag, packed in `focus[0]`: `1.0` while an entity is focused, `0.0` otherwise. The
    /// shaders dim every instance whose `selected < 0.5` when this is set, so only the focused (=
    /// selected) entity stays lit — "gray out the rest." A `vec4` (not a bare `f32` + pad) so the WGSL
    /// uniform layout matches byte-for-byte: a `vec3` tail would round the std140 struct to 96 bytes
    /// while this struct is 80, and wgpu would reject the undersized buffer at draw. `[1..4]` unused.
    focus: [f32; 4],
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
    let mut depth = make_depth(&device, w, h);

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
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cam-bg"),
        layout: &cam_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buf.as_entire_binding(),
        }],
    });
    // Cube instances (the M2.2 placeholder/fallback path + the perf baseline) — the subset of entities
    // with NO mesh asset. Grows with the scene.
    let mut cube = InstanceBuf::new(&device, &inst_bgl, 1024);

    let index_buf = create_init_buffer(
        &device,
        "cube-idx",
        bytemuck::cast_slice(&CUBE_INDICES),
        wgpu::BufferUsages::INDEX,
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
    let cube_pipeline = make_pipeline(
        &device,
        &shader,
        &cube_layout,
        format,
        &depth_state,
        "vs_cube",
        wgpu::PrimitiveTopology::TriangleList,
        Some(wgpu::Face::Back),
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
        "overlay",
    );
    let mut overlay = InstanceBuf::new(&device, &inst_bgl, 256);
    let mut cur_overlay_rev = u64::MAX;
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
        ],
    };
    let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("mesh"),
        layout: Some(&cube_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_mesh"),
            buffers: &[mesh_vbl],
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
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    // Per-asset GPU geometry (slot-indexed, uploaded once per meshes_revision) + per-asset instance
    // lists (rebuilt per scene revision). `cube_scratch`/`mesh_scratch` are reused partition buffers.
    let mut gpu_meshes: Vec<Option<GpuMesh>> = Vec::new();
    let mut mesh_inst: Vec<InstanceBuf> = Vec::new();
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
                depth = make_depth(&device, w, h);
            }
        }

        // read shared state; re-upload instances on revision change (picking is NOT serviced here —
        // it's done synchronously in the viewport_pick command, decoupled from the frame cadence)
        let (cam, cam_eye, focus_active, gizmo_verts) = {
            let mut st = shared.lock().unwrap();
            if st.distance == 0.0 {
                st.distance = 60.0;
                st.elevation = 0.4;
            }
            // Camera input — entirely native (invariant 4): fold in any wheel zoom, and while a
            // right-drag is active, poll the OS cursor and orbit by its per-frame delta. No `invoke`
            // here; the JS side only sent drag_start/drag_end (2 calls per gesture), never per frame.
            if st.zoom_delta != 0.0 {
                st.distance = (st.distance + st.zoom_delta).clamp(5.0, 400.0);
                st.zoom_delta = 0.0;
            }
            if st.dragging {
                if let Ok(p) = window.cursor_position() {
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
                    window
                        .cursor_position()
                        .ok()
                        .map(|p| (p.x as f32 / w.max(1) as f32, p.y as f32 / h.max(1) as f32))
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
                for m in &st.meshes {
                    if m.vertices.is_empty() || m.indices.is_empty() {
                        gpu_meshes.push(None);
                        continue;
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
                    gpu_meshes.push(Some(GpuMesh {
                        vbuf,
                        ibuf,
                        n_idx: m.indices.len() as u32,
                    }));
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
                // tracking-line endpoints (rebuilt in lock-step with instances)
                lines.upload(&device, &queue, &inst_bgl, &st.line_points);
            }
            // M8.4 contact-debugger overlay — uploaded on its OWN revision (the debugger updates
            // independently of scene edits; while off, the buffer is empty so there's nothing to upload).
            if st.overlay_revision != cur_overlay_rev {
                cur_overlay_rev = st.overlay_revision;
                overlay.upload(&device, &queue, &inst_bgl, &st.overlay_lines);
            }
            let aspect = w as f32 / h.max(1) as f32;
            let cam = camera_matrix(
                st.orbit,
                st.elevation,
                st.distance,
                aspect,
                st.cam_target.into(),
            );
            // The camera eye (world) — the PBR view direction in fs_mesh (M11.2). Carried in the Camera
            // uniform's spare `focus.yzw` (focus.x stays the focus-dim flag).
            let cam_eye = camera_eye(st.orbit, st.elevation, st.distance, st.cam_target);
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
            )
        };
        queue.write_buffer(
            &camera_buf,
            0,
            bytemuck::bytes_of(&Camera {
                view_proj: cam.to_cols_array_2d(),
                focus: [focus_active, cam_eye[0], cam_eye[1], cam_eye[2]],
            }),
        );
        // M9.1: upload the gizmo handle geometry (tiny — regenerated each frame at the selection).
        gizmo_buf.upload(&device, &queue, &inst_bgl, &gizmo_verts);

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
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
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
            for (slot, mesh) in gpu_meshes.iter().enumerate() {
                let (Some(mesh), Some(inst)) = (mesh.as_ref(), mesh_inst.get(slot)) else {
                    continue;
                };
                if inst.n == 0 {
                    continue;
                }
                rp.set_bind_group(1, &inst.bg, &[]);
                rp.set_vertex_buffer(0, mesh.vbuf.slice(..));
                rp.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..mesh.n_idx, 0, 0..inst.n);
            }
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
            // M9.1 transform gizmo, drawn LAST (over everything), per-segment X/Y/Z colour, always-pass
            // depth. Skipped when nothing is selected (`gizmo_buf.n == 0`) — zero per-frame cost.
            if gizmo_buf.n > 0 {
                rp.set_pipeline(&overlay_pipeline);
                rp.set_bind_group(1, &gizmo_buf.bg, &[]);
                rp.draw(0..gizmo_buf.n, 0..1);
            }
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
                "[viewport] cubes={} meshes={n_mesh} frames={acc_n} cpu-submit p50={p50:.3}ms p99={p99:.3}ms avg={:.3}ms | ipc={ipc_window} ({ipc_per_frame:.3}/frame)",
                cube.n,
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

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
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
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // Get nearby: zoomed in from 60 → scale(1.0)*4 clamped to [6,40] = 6.
        assert_eq!(st.distance, 6.0);
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
