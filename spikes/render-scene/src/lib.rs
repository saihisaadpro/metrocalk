//! M2.2 render gate: the M1.4 stress scene (5k / 20k entities) rendered through wgpu, native AND in
//! the browser, to measure the *real* frame cost (M0 only proved a triangle ≈ 0 render work).
//!
//! Approach (per the task's verified research findings):
//! - **Instancing** — entity cubes and per-entity gizmos are each ONE instanced draw; the grid is a
//!   render bundle. Draw-call count is therefore constant (3) regardless of entity count.
//! - **GPU frustum culling → indirect draws** — a compute pass tests each instance's bounding
//!   sphere against the 6 frustum planes and compacts the survivors into a `visible[]` index list,
//!   `atomicAdd`-ing the count into a counter that is copied into the draw-args buffers. We then
//!   issue ONE `draw_indexed_indirect` (cubes) + ONE `draw_indirect` (gizmos). This is designed
//!   **without** multi-draw-indirect and **without** `indirect-first-instance` (both browser-absent):
//!   `first_instance` stays 0 and the vertex shader indexes `visible[]` by `instance_index`.
//! - **Separate GPU vs CPU timing** — `TIMESTAMP_QUERY` gives GPU frame time; wall-clock around
//!   encode+submit gives CPU submit. Reported independently (p50/p95/p99/max).

mod rng;
mod scene;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

use glam::{Mat4, Vec3, Vec4};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const WG_SIZE: u32 = 64;
/// Draw calls per frame: grid bundle + cube indirect + gizmo indirect. Constant by design.
const DRAW_CALLS: u32 = 3;

const SHADER: &str = include_str!("shader.wgsl");
const CULL_SHADER: &str = include_str!("cull.wgsl");

// ----------------------------------------------------------------------------------------------
// GPU-side data layouts (must match shader.wgsl).
// ----------------------------------------------------------------------------------------------

/// Camera uniform: view-projection + 6 world-space frustum planes (Gribb–Hartmann).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    view_proj: [[f32; 4]; 4],
    planes: [[f32; 4]; 6],
}

/// `DrawIndexedIndirectArgs` (wgpu/WebGPU layout): index_count, instance_count, first_index,
/// base_vertex, first_instance.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawIndexedIndirect {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

/// `DrawIndirectArgs`: vertex_count, instance_count, first_vertex, first_instance.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawIndirect {
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

/// A unit cube as 36 corner-IDs (0..7) with CCW-outward winding. Each value is dereferenced by the
/// indexed draw into `@builtin(vertex_index)`, which the shader uses to look up the 8-corner array —
/// so the cube geometry is fully procedural (no vertex buffer). Corner ID bit layout:
/// x=bit0, y=bit1, z=bit2, each 0→-1 / 1→+1.
const CUBE_INDICES: [u16; 36] = [
    0, 2, 3, 0, 3, 1, // -z
    4, 5, 7, 4, 7, 6, // +z
    0, 4, 6, 0, 6, 2, // -x
    1, 3, 7, 1, 7, 5, // +x
    0, 1, 5, 0, 5, 4, // -y
    2, 6, 7, 2, 7, 3, // +y
];

/// Cross-platform millisecond clock.
fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window().unwrap().performance().unwrap().now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs_f64() * 1000.0
    }
}

// ----------------------------------------------------------------------------------------------
// GPU timer: TIMESTAMP_QUERY readback, double-buffered & non-blocking. Each timing frame also
// copies the visible-count atomic into the same readback so we record GPU ms + visible count
// together with a ~2-frame lag — no per-frame stall (a stall would poison the CPU-submit metric).
// ----------------------------------------------------------------------------------------------

struct TimerSlot {
    resolve: wgpu::Buffer,  // QUERY_RESOLVE | COPY_SRC  (2 × u64 timestamps)
    readback: wgpu::Buffer, // MAP_READ | COPY_DST       (16B timestamps + 4B count + pad)
    ready: Arc<Mutex<Option<(f64, u32)>>>,
    inflight: bool,
}

struct GpuTimer {
    qset: wgpu::QuerySet,
    period_ns: f32,
    slots: [TimerSlot; 2],
}

impl GpuTimer {
    fn new(device: &wgpu::Device, period_ns: f32) -> Self {
        let qset = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("timestamps"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let mk = || TimerSlot {
            resolve: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ts-resolve"),
                size: 16,
                usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            }),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ts-readback"),
                size: 24,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            ready: Arc::new(Mutex::new(None)),
            inflight: false,
        };
        GpuTimer { qset, period_ns, slots: [mk(), mk()] }
    }
}

// ----------------------------------------------------------------------------------------------
// Bench accumulator.
// ----------------------------------------------------------------------------------------------

struct Bench {
    /// Seconds of steady-state to collect after warmup (0 = visual mode, never reports/exits).
    secs: f64,
    warmup_frames: u64,
    cpu_ms: Vec<f64>,
    gpu_ms: Vec<f64>,
    vis_min: u32,
    vis_max: u32,
    started_ms: f64,
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let idx = ((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1;
    sorted[idx]
}

// ----------------------------------------------------------------------------------------------
// Renderer.
// ----------------------------------------------------------------------------------------------

struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,

    camera_buf: wgpu::Buffer,
    camera_bg: wgpu::BindGroup,

    cull_pipeline: wgpu::ComputePipeline,
    cull_bg: wgpu::BindGroup,
    cube_pipeline: wgpu::RenderPipeline,
    gizmo_pipeline: wgpu::RenderPipeline,
    render_bg: wgpu::BindGroup,
    grid_bundle: wgpu::RenderBundle,

    index_buf: wgpu::Buffer,
    counter: wgpu::Buffer,
    cube_args: wgpu::Buffer,
    gizmo_args: wgpu::Buffer,

    n: u32,
    extent: f32,
    biggest_storage_bytes: u64,
    timer: Option<GpuTimer>,
    frame_parity: usize,

    window: Arc<Window>,
    start_ms: f64,
    last_frame_ms: f64,
    frame_count: u64,
    ttff_ms: f64,
    ttff_logged: bool,
    fps_accum_ms: f64,
    fps_frames: u32,
    last_gpu_ms: f64,
    last_vis: u32,
    bench: Bench,
    reported: bool,
}

impl Gfx {
    async fn new(window: Arc<Window>, n: u32, bench_secs: f64) -> Gfx {
        let start_ms = now_ms();
        let mut size = window.inner_size();
        size.width = size.width.max(1);
        size.height = size.height.max(1);
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            if size.width <= 1 || size.height <= 1 {
                size = winit::dpi::PhysicalSize::new(960, 600);
            }
            if let Some(canvas) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("app"))
                .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
            {
                canvas.set_width(size.width);
                canvas.set_height(size.height);
            }
        }

        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).expect("create surface");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no compatible adapter");

        dump_adapter(&adapter);

        // Request TIMESTAMP_QUERY when available (Chrome/Dawn + native expose it; some browsers
        // don't — then GPU frame time is reported "n/a" and we fall back to CPU timing only).
        let want = wgpu::Features::TIMESTAMP_QUERY;
        let features = adapter.features() & want;
        let has_ts = features.contains(wgpu::Features::TIMESTAMP_QUERY);
        log_line(&format!("TIMESTAMP_QUERY available = {has_ts}"));

        // downlevel_defaults (NOT webgl2 defaults): we need compute + storage buffers, which WebGL2
        // can't do. downlevel_defaults caps the largest storage buffer at 128 MB — exactly the
        // browser ceiling the task flags — so building against it proves we stay under it.
        let limits = wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits());
        log_line(&format!(
            "DEVICE LIMITS (downlevel_defaults floor) max_storage_buffer_binding_size={} max_storage_buffers_per_stage={} max_compute_invocations_per_wg={}",
            limits.max_storage_buffer_binding_size,
            limits.max_storage_buffers_per_shader_stage,
            limits.max_compute_invocations_per_workgroup,
        ));

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_features: features,
                required_limits: limits,
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("request device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            // AutoVsync = FIFO present (the DXGI frame-pacing fix lives in this present path on the
            // pinned wgpu 29; recorded in CONSTRAINTS). GPU/CPU time are measured independent of it.
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth = make_depth(&device, size.width, size.height);

        // ---- scene → instance storage buffer ----
        let scene = scene::build_scene(n as usize);
        let extent = scene.extent;
        let instances_bytes = std::mem::size_of_val(&scene.instances[..]) as u64;
        let instance_buf = create_init_buffer(
            &device,
            "instances",
            bytemuck::cast_slice(&scene.instances),
            wgpu::BufferUsages::STORAGE,
        );
        let visible_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("visible"),
            size: u64::from(n) * 4,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let biggest_storage_bytes = instances_bytes.max(u64::from(n) * 4);
        log_line(&format!(
            "STORAGE buffers: instances={instances_bytes}B visible={}B  biggest={:.2} MB (ceiling 128 MB)",
            u64::from(n) * 4,
            biggest_storage_bytes as f64 / (1024.0 * 1024.0),
        ));

        let index_buf = create_init_buffer(
            &device,
            "cube-indices",
            bytemuck::cast_slice(&CUBE_INDICES),
            wgpu::BufferUsages::INDEX,
        );

        // counter (atomic visible count); cleared to 0 each frame, copied into both arg buffers.
        let counter = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("counter"),
            size: 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let cube_args = create_init_buffer(
            &device,
            "cube-args",
            bytemuck::bytes_of(&DrawIndexedIndirect {
                index_count: CUBE_INDICES.len() as u32,
                instance_count: 0,
                first_index: 0,
                base_vertex: 0,
                first_instance: 0,
            }),
            wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );
        let gizmo_args = create_init_buffer(
            &device,
            "gizmo-args",
            bytemuck::bytes_of(&DrawIndirect {
                vertex_count: 6, // 3 axes × 2 verts (LineList)
                instance_count: 0,
                first_vertex: 0,
                first_instance: 0,
            }),
            wgpu::BufferUsages::INDIRECT | wgpu::BufferUsages::COPY_DST,
        );

        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("scene-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let cull_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cull-shader"),
            source: wgpu::ShaderSource::Wgsl(CULL_SHADER.into()),
        });

        // ---- bind group layouts ----
        let camera_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera-bgl"),
            entries: &[bgl_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::COMPUTE,
                wgpu::BufferBindingType::Uniform,
            )],
        });
        let cull_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cull-bgl"),
            entries: &[
                bgl_entry(0, wgpu::ShaderStages::COMPUTE, wgpu::BufferBindingType::Storage { read_only: true }),
                bgl_entry(1, wgpu::ShaderStages::COMPUTE, wgpu::BufferBindingType::Storage { read_only: false }),
                bgl_entry(2, wgpu::ShaderStages::COMPUTE, wgpu::BufferBindingType::Storage { read_only: false }),
            ],
        });
        let render_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("render-bgl"),
            entries: &[
                bgl_entry(0, wgpu::ShaderStages::VERTEX, wgpu::BufferBindingType::Storage { read_only: true }),
                bgl_entry(1, wgpu::ShaderStages::VERTEX, wgpu::BufferBindingType::Storage { read_only: true }),
            ],
        });

        let camera_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera-bg"),
            layout: &camera_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() }],
        });
        let cull_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cull-bg"),
            layout: &cull_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: instance_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: visible_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: counter.as_entire_binding() },
            ],
        });
        let render_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("render-bg"),
            layout: &render_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: instance_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: visible_buf.as_entire_binding() },
            ],
        });

        // ---- pipelines ----
        let cull_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cull-layout"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&cull_bgl)],
            immediate_size: 0,
        });
        let cull_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("cull"),
            layout: Some(&cull_layout),
            module: &cull_shader,
            entry_point: Some("cull"),
            compilation_options: Default::default(),
            cache: None,
        });

        let render_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("render-layout"),
            bind_group_layouts: &[Some(&camera_bgl), Some(&render_bgl)],
            immediate_size: 0,
        });
        let grid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("grid-layout"),
            bind_group_layouts: &[Some(&camera_bgl)],
            immediate_size: 0,
        });

        let depth_state = wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Less),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        };
        let cube_pipeline = make_pipeline(
            &device, &shader, &render_layout, format, &depth_state,
            "vs_cube", "fs_solid", wgpu::PrimitiveTopology::TriangleList, Some(wgpu::Face::Back), "cube",
        );
        let gizmo_pipeline = make_pipeline(
            &device, &shader, &render_layout, format, &depth_state,
            "vs_gizmo", "fs_solid", wgpu::PrimitiveTopology::LineList, None, "gizmo",
        );
        let grid_pipeline = make_pipeline(
            &device, &shader, &grid_layout, format, &depth_state,
            "vs_grid", "fs_solid", wgpu::PrimitiveTopology::LineList, None, "grid",
        );

        // ---- grid render bundle (stable content, recorded once) ----
        let grid_bundle = {
            let mut enc = device.create_render_bundle_encoder(&wgpu::RenderBundleEncoderDescriptor {
                label: Some("grid-bundle"),
                color_formats: &[Some(format)],
                depth_stencil: Some(wgpu::RenderBundleDepthStencil {
                    format: DEPTH_FORMAT,
                    depth_read_only: false,
                    stencil_read_only: true,
                }),
                sample_count: 1,
                multiview: None,
            });
            enc.set_pipeline(&grid_pipeline);
            enc.set_bind_group(0, &camera_bg, &[]);
            enc.draw(0..GRID_VERTS, 0..1);
            enc.finish(&wgpu::RenderBundleDescriptor { label: Some("grid") })
        };

        let timer = has_ts.then(|| GpuTimer::new(&device, queue.get_timestamp_period()));

        Gfx {
            surface,
            device,
            queue,
            config,
            depth,
            camera_buf,
            camera_bg,
            cull_pipeline,
            cull_bg,
            cube_pipeline,
            gizmo_pipeline,
            render_bg,
            grid_bundle,
            index_buf,
            counter,
            cube_args,
            gizmo_args,
            n,
            extent,
            biggest_storage_bytes,
            timer,
            frame_parity: 0,
            window,
            start_ms,
            last_frame_ms: start_ms,
            frame_count: 0,
            ttff_ms: 0.0,
            ttff_logged: false,
            fps_accum_ms: 0.0,
            fps_frames: 0,
            last_gpu_ms: f64::NAN,
            last_vis: 0,
            bench: Bench {
                secs: bench_secs,
                warmup_frames: 90,
                cpu_ms: Vec::new(),
                gpu_ms: Vec::new(),
                vis_min: u32::MAX,
                vis_max: 0,
                started_ms: 0.0,
            },
            reported: false,
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w > 1 && h > 1 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
            self.depth = make_depth(&self.device, w, h);
        }
    }

    /// Camera path: orbits the origin while the distance breathes between a close vantage (heavy
    /// culling) and a pulled-back vantage where the whole cloud is in-frustum (the worst-case
    /// "everything visible" frame the task asks to capture).
    fn camera(&self, t: f64) -> CameraUniform {
        let orbit = (t * 0.25) as f32;
        let breathe = 0.5 + 0.5 * (t * 0.18).sin() as f32; // 0..1
        let dist = self.extent * (1.15 + 1.55 * breathe); // 1.15×..2.70× extent
        let eye = Vec3::new(orbit.cos() * dist, dist * 0.4, orbit.sin() * dist);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let proj = Mat4::perspective_rh(55f32.to_radians(), aspect, 0.1, self.extent * 12.0);
        let view = Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y);
        let vp = proj * view;

        // Gribb–Hartmann frustum planes from the row vectors of view_proj (wgpu clip z ∈ [0,1]).
        let r0 = Vec4::new(vp.x_axis.x, vp.y_axis.x, vp.z_axis.x, vp.w_axis.x);
        let r1 = Vec4::new(vp.x_axis.y, vp.y_axis.y, vp.z_axis.y, vp.w_axis.y);
        let r2 = Vec4::new(vp.x_axis.z, vp.y_axis.z, vp.z_axis.z, vp.w_axis.z);
        let r3 = Vec4::new(vp.x_axis.w, vp.y_axis.w, vp.z_axis.w, vp.w_axis.w);
        let raw = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
        let mut planes = [[0.0f32; 4]; 6];
        for (i, p) in raw.iter().enumerate() {
            let len = p.truncate().length().max(1e-6);
            let np = *p / len; // normalize so plane·point + w is a world-unit signed distance
            planes[i] = [np.x, np.y, np.z, np.w];
        }
        CameraUniform { view_proj: vp.to_cols_array_2d(), planes }
    }

    fn render(&mut self) {
        let t = now_ms();
        let dt = t - self.last_frame_ms;
        self.last_frame_ms = t;
        let elapsed = (t - self.start_ms) / 1000.0;

        // Upload camera (also recomputes frustum planes for the cull pass).
        let cam = self.camera(elapsed);
        self.queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&cam));
        // Reset the visible counter; the cull pass atomics into it, then we copy it into the args.
        self.queue.write_buffer(&self.counter, 0, &0u32.to_le_bytes());

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return,
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());

        // Time only when the slot we'd write into is free (its previous map was consumed) — avoids
        // ever copying into a still-mapped readback buffer.
        let s = self.frame_parity;
        let do_timing = self
            .timer
            .as_ref()
            .is_some_and(|tm| !tm.slots[s].inflight);

        // Portable WebGPU timing: timestamps written at pass boundaries (compute-begin → render-end
        // spans the whole frame's GPU work). This needs only TIMESTAMP_QUERY — encoder-level
        // write_timestamp would require the native-only TIMESTAMP_QUERY_INSIDE_ENCODERS feature,
        // which the browser doesn't expose.
        let qset = self.timer.as_ref().map(|t| &t.qset);
        let cull_ts = do_timing.then(|| wgpu::ComputePassTimestampWrites {
            query_set: qset.unwrap(),
            beginning_of_pass_write_index: Some(0),
            end_of_pass_write_index: None,
        });
        let render_ts = do_timing.then(|| wgpu::RenderPassTimestampWrites {
            query_set: qset.unwrap(),
            beginning_of_pass_write_index: None,
            end_of_pass_write_index: Some(1),
        });

        let cpu_t0 = now_ms();
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });

        // ---- cull (compute) ----
        {
            let mut cp = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("cull"),
                timestamp_writes: cull_ts,
            });
            cp.set_pipeline(&self.cull_pipeline);
            cp.set_bind_group(0, &self.camera_bg, &[]);
            cp.set_bind_group(1, &self.cull_bg, &[]);
            cp.dispatch_workgroups(self.n.div_ceil(WG_SIZE), 1, 1);
        }
        // Publish the visible count into both indirect-arg buffers (instance_count is at offset 4).
        enc.copy_buffer_to_buffer(&self.counter, 0, &self.cube_args, 4, 4);
        enc.copy_buffer_to_buffer(&self.counter, 0, &self.gizmo_args, 4, 4);

        // ---- draw (one indirect per mesh kind + the static grid bundle) ----
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.04, g: 0.04, b: 0.06, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: render_ts,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.execute_bundles(std::iter::once(&self.grid_bundle));

            rp.set_bind_group(0, &self.camera_bg, &[]);
            rp.set_bind_group(1, &self.render_bg, &[]);
            rp.set_pipeline(&self.cube_pipeline);
            rp.set_index_buffer(self.index_buf.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed_indirect(&self.cube_args, 0);

            rp.set_pipeline(&self.gizmo_pipeline);
            rp.draw_indirect(&self.gizmo_args, 0);
        }

        if do_timing {
            let tm = self.timer.as_ref().unwrap();
            enc.resolve_query_set(&tm.qset, 0..2, &tm.slots[s].resolve, 0);
            enc.copy_buffer_to_buffer(&tm.slots[s].resolve, 0, &tm.slots[s].readback, 0, 16);
            enc.copy_buffer_to_buffer(&self.counter, 0, &tm.slots[s].readback, 16, 4);
        }

        self.queue.submit([enc.finish()]);
        let cpu_ms = now_ms() - cpu_t0;
        frame.present();

        // Kick the readback for this slot; consume any slot whose map has resolved.
        if do_timing {
            let period = self.timer.as_ref().unwrap().period_ns;
            let tm = self.timer.as_mut().unwrap();
            let ready = tm.slots[s].ready.clone();
            let rb = tm.slots[s].readback.clone();
            tm.slots[s].readback.slice(..).map_async(wgpu::MapMode::Read, move |res| {
                if res.is_ok() {
                    let data = rb.slice(..).get_mapped_range();
                    let t0 = u64::from_le_bytes(data[0..8].try_into().unwrap());
                    let t1 = u64::from_le_bytes(data[8..16].try_into().unwrap());
                    let count = u32::from_le_bytes(data[16..20].try_into().unwrap());
                    let ms = (t1.wrapping_sub(t0)) as f64 * f64::from(period) / 1.0e6;
                    drop(data);
                    rb.unmap();
                    *ready.lock().unwrap() = Some((ms, count));
                }
            });
            tm.slots[s].inflight = true;
            self.frame_parity ^= 1;
        }
        #[cfg(not(target_arch = "wasm32"))]
        let _ = self.device.poll(wgpu::PollType::Poll);
        if let Some(tm) = self.timer.as_mut() {
            for slot in &mut tm.slots {
                if slot.inflight {
                    let v = slot.ready.lock().unwrap().take();
                    if let Some((ms, count)) = v {
                        self.last_gpu_ms = ms;
                        self.last_vis = count;
                        slot.inflight = false;
                    }
                }
            }
        }

        self.frame_count += 1;
        if !self.ttff_logged {
            self.ttff_logged = true;
            self.ttff_ms = t - self.start_ms;
            log_line(&format!("TTFF (gfx-new → first present): {:.1} ms", self.ttff_ms));
        }

        // ---- bench accumulation ----
        if self.bench.secs > 0.0 && self.frame_count > self.bench.warmup_frames {
            if self.bench.started_ms == 0.0 {
                self.bench.started_ms = t;
            }
            self.bench.cpu_ms.push(cpu_ms);
            if self.last_gpu_ms.is_finite() {
                self.bench.gpu_ms.push(self.last_gpu_ms);
            }
            if self.last_vis > 0 {
                self.bench.vis_min = self.bench.vis_min.min(self.last_vis);
                self.bench.vis_max = self.bench.vis_max.max(self.last_vis);
            }
        }

        // ---- overlay (~4×/s) ----
        self.fps_accum_ms += dt;
        self.fps_frames += 1;
        if self.fps_accum_ms >= 250.0 {
            let avg = self.fps_accum_ms / f64::from(self.fps_frames);
            let gpu = if self.last_gpu_ms.is_finite() {
                format!("{:.2}", self.last_gpu_ms)
            } else {
                "n/a".to_string()
            };
            let msg = format!(
                "{}k ents · {:.2} ms/frame ({:.0} fps) · GPU {gpu} ms · CPU {cpu_ms:.2} ms · vis {} · {DRAW_CALLS} draws",
                self.n / 1000,
                avg,
                1000.0 / avg,
                self.last_vis,
            );
            set_overlay(&msg);
            #[cfg(not(target_arch = "wasm32"))]
            self.window.set_title(&format!("Metrocalk render spike — {msg}"));
            self.fps_accum_ms = 0.0;
            self.fps_frames = 0;
        }
    }

    fn bench_done(&self) -> bool {
        self.bench.secs > 0.0
            && self.bench.started_ms > 0.0
            && (now_ms() - self.bench.started_ms) / 1000.0 >= self.bench.secs
    }

    fn report(&mut self) {
        if self.reported {
            return;
        }
        self.reported = true;
        let mut cpu = self.bench.cpu_ms.clone();
        let mut gpu = self.bench.gpu_ms.clone();
        cpu.sort_by(|a, b| a.partial_cmp(b).unwrap());
        gpu.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let gpu_line = if gpu.is_empty() {
            "GPU n/a (no TIMESTAMP_QUERY)".to_string()
        } else {
            format!(
                "GPU ms p50={:.3} p95={:.3} p99={:.3} max={:.3} (n={})",
                pct(&gpu, 0.50), pct(&gpu, 0.95), pct(&gpu, 0.99), pct(&gpu, 1.0), gpu.len()
            )
        };
        let report = format!(
            "BENCH RESULT n={} ({}k) frames={} | CPU-submit ms p50={:.3} p95={:.3} p99={:.3} max={:.3} (n={}) | {gpu_line} | visible min={} max={} of {} ({:.0}% max) | draw_calls={DRAW_CALLS} (constant) | biggest_storage={:.2}MB",
            self.n, self.n / 1000, self.frame_count,
            pct(&cpu, 0.50), pct(&cpu, 0.95), pct(&cpu, 0.99), pct(&cpu, 1.0), cpu.len(),
            self.bench.vis_min, self.bench.vis_max, self.n,
            100.0 * f64::from(self.bench.vis_max) / f64::from(self.n.max(1)),
            self.biggest_storage_bytes as f64 / (1024.0 * 1024.0),
        );
        log_line(&report);
        set_overlay(&report);
        #[cfg(target_arch = "wasm32")]
        {
            let g = js_sys::global();
            let _ = js_sys::Reflect::set(&g, &JsValue::from_str("__benchresult"), &JsValue::from_str(&report));
        }
    }
}

// ----------------------------------------------------------------------------------------------
// wgpu construction helpers.
// ----------------------------------------------------------------------------------------------

const GRID_VERTS: u32 = (2 * (GRID_N + 1) * 2) as u32; // lines in 2 directions × 2 verts
const GRID_N: usize = 40;

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn create_init_buffer(device: &wgpu::Device, label: &str, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents: data, usage })
}

fn bgl_entry(binding: u32, vis: wgpu::ShaderStages, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    depth: &wgpu::DepthStencilState,
    vs: &str,
    fs: &str,
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
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}

// ----------------------------------------------------------------------------------------------
// winit app shell (mirrors the M0 wasm spike).
// ----------------------------------------------------------------------------------------------

#[derive(Default)]
struct App {
    gfx: Rc<RefCell<Option<Gfx>>>,
    window: Option<Arc<Window>>,
    n: u32,
    bench_secs: f64,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes().with_title("Metrocalk render spike");
        #[cfg(not(target_arch = "wasm32"))]
        {
            attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(1280, 800));
        }
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;
            let canvas = web_sys::window()
                .unwrap()
                .document()
                .unwrap()
                .get_element_by_id("app")
                .expect("index.html must contain <canvas id=app>")
                .dyn_into::<web_sys::HtmlCanvasElement>()
                .unwrap();
            attrs = attrs.with_canvas(Some(canvas));
        }
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));
        self.window = Some(window.clone());

        let gfx = self.gfx.clone();
        let n = self.n;
        let secs = self.bench_secs;
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let g = Gfx::new(window.clone(), n, secs).await;
                *gfx.borrow_mut() = Some(g);
                window.request_redraw();
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            *gfx.borrow_mut() = Some(pollster::block_on(Gfx::new(window.clone(), n, secs)));
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(s) => {
                if let Some(g) = self.gfx.borrow_mut().as_mut() {
                    g.resize(s.width, s.height);
                }
            }
            WindowEvent::RedrawRequested => {
                let done = {
                    let mut b = self.gfx.borrow_mut();
                    if let Some(g) = b.as_mut() {
                        g.render();
                        if g.bench_done() {
                            g.report();
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };
                if done {
                    event_loop.exit();
                } else if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            _ => {}
        }
    }
}

fn run_app(n: u32, bench_secs: f64) {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut app = App { n, bench_secs, ..Default::default() };
    #[cfg(not(target_arch = "wasm32"))]
    event_loop.run_app(&mut app).expect("run");
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(app);
    }
}

/// Native entry. `SCENE_N` (default 5000) picks the preset; `SPIKE_SECS` (default 0 = visual mode)
/// runs a timed bench then prints the result table and exits.
#[cfg(not(target_arch = "wasm32"))]
pub fn run() {
    env_logger::init();
    let n = std::env::var("SCENE_N").ok().and_then(|s| s.parse().ok()).unwrap_or(5000);
    let secs = std::env::var("SPIKE_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(0.0);
    log_line(&format!("render-spike native: n={n} bench_secs={secs}"));
    run_app(n, secs);
}

/// Wasm entry — `?n=` picks the entity count, `?secs=` runs a timed bench (default visual).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn wasm_main() {
    console_error_panic_hook::set_once();
    let (n, secs) = query_params();
    log_line("metrocalk-render-spike booting");
    log_line(&format!("crossOriginIsolated = {}", cross_origin_isolated()));
    log_line(&format!("navigator.gpu present = {}  n={n} secs={secs}", gpu_present()));
    set_banner(&format!(
        "render spike · n={n} · crossOriginIsolated = {}  ·  navigator.gpu = {}",
        cross_origin_isolated(),
        gpu_present()
    ));
    run_app(n, secs);
}

// ----------------------------------------------------------------------------------------------
// logging / overlay / COI helpers (mirror the M0 wasm spike).
// ----------------------------------------------------------------------------------------------

fn log_line(s: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::console::log_1(&s.into());
        let g = js_sys::global();
        let key = JsValue::from_str("__spikelog");
        let prev = js_sys::Reflect::get(&g, &key).ok().and_then(|v| v.as_string()).unwrap_or_default();
        let _ = js_sys::Reflect::set(&g, &key, &JsValue::from_str(&format!("{prev}{s}\n")));
    }
    #[cfg(not(target_arch = "wasm32"))]
    println!("{s}");
}

#[cfg(target_arch = "wasm32")]
fn query_params() -> (u32, f64) {
    let search = web_sys::window().unwrap().location().search().unwrap_or_default();
    let params = web_sys::UrlSearchParams::new_with_str(&search).unwrap();
    let n = params.get("n").and_then(|s| s.parse().ok()).unwrap_or(5000);
    let secs = params.get("secs").and_then(|s| s.parse().ok()).unwrap_or(0.0);
    (n, secs)
}

#[cfg(target_arch = "wasm32")]
fn cross_origin_isolated() -> bool {
    js_sys::Reflect::get(&web_sys::window().unwrap(), &JsValue::from_str("crossOriginIsolated"))
        .ok()
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn gpu_present() -> bool {
    js_sys::Reflect::get(&web_sys::window().unwrap().navigator(), &JsValue::from_str("gpu"))
        .map(|v| !v.is_undefined() && !v.is_null())
        .unwrap_or(false)
}

#[cfg(target_arch = "wasm32")]
fn set_banner(text: &str) {
    use wasm_bindgen::JsCast;
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.get_element_by_id("coi").unwrap_or_else(|| {
        let e = doc.create_element("div").unwrap();
        e.set_id("coi");
        let html: web_sys::HtmlElement = e.clone().dyn_into().unwrap();
        for (k, v) in [
            ("position", "fixed"),
            ("bottom", "8px"),
            ("left", "8px"),
            ("font", "13px monospace"),
            ("color", "#9cf"),
            ("background", "rgba(0,0,0,0.6)"),
            ("padding", "4px 8px"),
            ("z-index", "10"),
        ] {
            html.style().set_property(k, v).ok();
        }
        doc.body().unwrap().append_child(&e).ok();
        e
    });
    el.set_text_content(Some(text));
}

#[cfg(target_arch = "wasm32")]
fn set_overlay(text: &str) {
    use wasm_bindgen::JsCast;
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = doc.get_element_by_id("overlay").unwrap_or_else(|| {
        let e = doc.create_element("div").unwrap();
        e.set_id("overlay");
        let html: web_sys::HtmlElement = e.clone().dyn_into().unwrap();
        for (k, v) in [
            ("position", "fixed"),
            ("top", "8px"),
            ("left", "8px"),
            ("font", "13px monospace"),
            ("color", "#0f0"),
            ("background", "rgba(0,0,0,0.6)"),
            ("padding", "4px 8px"),
            ("z-index", "10"),
            ("max-width", "96vw"),
        ] {
            html.style().set_property(k, v).ok();
        }
        doc.body().unwrap().append_child(&e).ok();
        e
    });
    el.set_text_content(Some(text));
}

#[cfg(not(target_arch = "wasm32"))]
fn set_overlay(_text: &str) {}

// ----------------------------------------------------------------------------------------------
// diagnostics.
// ----------------------------------------------------------------------------------------------

fn dump_adapter(adapter: &wgpu::Adapter) {
    let info = adapter.get_info();
    log_line(&format!(
        "ADAPTER name='{}' backend={:?} type={:?} driver='{}'",
        info.name, info.backend, info.device_type, info.driver
    ));
}
