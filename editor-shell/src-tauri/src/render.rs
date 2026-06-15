//! The native wgpu viewport — M2.2's instanced render path on the Tauri window surface (ADR-008
//! single-window: this surface is OS-composited *under* the transparent WebView2). Renders the live
//! `/core` scene: one instanced cube per entity (from its `Transform`) + a ground grid, depth-tested,
//! with an orbiting camera. Instancing is the M2.2 technique that holds the frame budget; the GPU
//! frustum-cull→indirect refinement is also proven in `spikes/render-scene` and ports in on top.
//!
//! The render loop owns no scene truth — it reads a shared [`SceneState`] the app updates from the
//! authoritative core (deltas), and writes the picked entity back. Hot interaction stays in Rust
//! (invariant 4): camera + picking never cross the JS boundary.

use std::sync::{Arc, Mutex};

use glam::{Mat4, Vec3};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

/// One renderable entity instance. 32 bytes, std430-clean (matches the WGSL `Instance`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub center: [f32; 3],
    pub scale: f32,
    pub color: [f32; 3],
    pub selected: f32,
}

/// Scene state shared between the app (writer, from core deltas + input) and the render loop (reader).
#[derive(Default)]
pub struct SceneState {
    pub instances: Vec<Instance>,
    /// Entity id (Loro key) parallel to `instances` — maps a picked index back to an entity.
    pub ids: Vec<String>,
    /// Currently-selected instance index (drives the highlight).
    pub selected: Option<usize>,
    /// Bump when `instances` changes so the loop re-uploads the buffer.
    pub revision: u64,
    /// Orbit/zoom driven by drag input (stays in Rust — invariant 4).
    pub orbit: f32,
    pub elevation: f32,
    pub distance: f32,
    /// Cursor in physical pixels for picking; `pick_request` toggles a pick.
    pub cursor: (f32, f32),
    pub pick_request: bool,
    /// Result of the last pick: instance index, or `usize::MAX` for none. Read by the app.
    pub picked: Option<usize>,
}

pub type Shared = Arc<Mutex<SceneState>>;

const SHADER: &str = include_str!("scene.wgsl");
const CUBE_INDICES: [u16; 36] = [
    0, 2, 3, 0, 3, 1, 4, 5, 7, 4, 7, 6, 0, 4, 6, 0, 6, 2, 1, 3, 7, 1, 7, 5, 0, 1, 5, 0, 5, 4, 2, 6, 7,
    2, 7, 3,
];
const GRID_VERTS: u32 = (2 * (40 + 1) * 2) as u32;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
}

/// Window handle wrapper so wgpu can make a surface from the Tauri window on a render thread.
struct WinHandle {
    window: tauri::WebviewWindow,
}
impl HasWindowHandle for WinHandle {
    fn window_handle(&self) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        self.window.window_handle()
    }
}
impl HasDisplayHandle for WinHandle {
    fn display_handle(&self) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
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
    let target = Arc::new(WinHandle { window: window.clone() });
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
    eprintln!("[viewport] adapter='{}' backend={:?}", adapter.get_info().name, adapter.get_info().backend);
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
    let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
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
    // Instance storage buffer — grown as the scene grows.
    let mut inst_cap: u64 = 1024;
    let mut instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("instances"),
        size: inst_cap * std::mem::size_of::<Instance>() as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    let cam_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cam-bgl"),
        entries: &[bgl_entry(0, wgpu::ShaderStages::VERTEX, wgpu::BufferBindingType::Uniform)],
    });
    let inst_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("inst-bgl"),
        entries: &[bgl_entry(0, wgpu::ShaderStages::VERTEX, wgpu::BufferBindingType::Storage { read_only: true })],
    });
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("cam-bg"),
        layout: &cam_bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() }],
    });
    let mut inst_bg = make_inst_bg(&device, &inst_bgl, &instance_buf);

    let index_buf = create_init_buffer(&device, "cube-idx", bytemuck::cast_slice(&CUBE_INDICES), wgpu::BufferUsages::INDEX);

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
    let cube_pipeline = make_pipeline(&device, &shader, &cube_layout, format, &depth_state, "vs_cube", wgpu::PrimitiveTopology::TriangleList, Some(wgpu::Face::Back), "cube");
    let grid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("grid-layout"),
        bind_group_layouts: &[Some(&cam_bgl)],
        immediate_size: 0,
    });
    let grid_pipeline = make_pipeline(&device, &shader, &grid_layout, format, &depth_state, "vs_grid", wgpu::PrimitiveTopology::LineList, None, "grid");

    eprintln!("[viewport] render loop started");
    let mut cur_rev = u64::MAX;
    let mut n_inst: u32 = 0;
    // frame-budget instrumentation (CPU submit time = encode+submit; the integrated viewport's cost)
    let mut acc_ms = 0.0f64;
    let mut acc_n = 0u32;
    let mut last_report = std::time::Instant::now();
    let mut cpu_samples: Vec<f64> = Vec::new();
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

        // read shared state; re-upload instances on revision change; service a pick request
        let (cam, do_pick) = {
            let mut st = shared.lock().unwrap();
            if st.distance == 0.0 {
                st.distance = 60.0;
                st.elevation = 0.4;
            }
            if st.revision != cur_rev {
                cur_rev = st.revision;
                n_inst = st.instances.len() as u32;
                let needed = st.instances.len() as u64;
                if needed > inst_cap {
                    inst_cap = needed.next_power_of_two();
                    instance_buf = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("instances"),
                        size: inst_cap * std::mem::size_of::<Instance>() as u64,
                        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    inst_bg = make_inst_bg(&device, &inst_bgl, &instance_buf);
                }
                if !st.instances.is_empty() {
                    queue.write_buffer(&instance_buf, 0, bytemuck::cast_slice(&st.instances));
                }
            }
            let aspect = w as f32 / h.max(1) as f32;
            let cam = camera_matrix(st.orbit, st.elevation, st.distance, aspect);
            let pick = st.pick_request;
            st.pick_request = false;
            (cam, pick)
        };
        queue.write_buffer(&camera_buf, 0, bytemuck::bytes_of(&Camera { view_proj: cam.to_cols_array_2d() }));

        if do_pick {
            // CPU ray-pick against instance spheres — stays in Rust (invariant 4). Result back to app.
            let hit = pick_nearest(&shared, &cam, (w, h));
            shared.lock().unwrap().picked = hit;
        }

        let frame = match surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                surface.configure(&device, &config);
                continue;
            }
            _ => {
                std::thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.04, g: 0.05, b: 0.08, a: 1.0 }), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_bind_group(0, &cam_bg, &[]);
            rp.set_pipeline(&grid_pipeline);
            rp.draw(0..GRID_VERTS, 0..1);
            if n_inst > 0 {
                rp.set_pipeline(&cube_pipeline);
                rp.set_bind_group(1, &inst_bg, &[]);
                rp.set_index_buffer(index_buf.slice(..), wgpu::IndexFormat::Uint16);
                rp.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..n_inst);
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
            eprintln!(
                "[viewport] n={n_inst} frames={acc_n} cpu-submit p50={p50:.3}ms p99={p99:.3}ms avg={:.3}ms",
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

fn camera_matrix(orbit: f32, elevation: f32, distance: f32, aspect: f32) -> Mat4 {
    let eye = Vec3::new(orbit.cos() * distance * elevation.cos(), distance * elevation.sin(), orbit.sin() * distance * elevation.cos());
    let proj = Mat4::perspective_rh(55f32.to_radians(), aspect, 0.1, distance * 8.0 + 100.0);
    proj * Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Y)
}

/// CPU ray-pick: unproject the cursor to a world ray, return the nearest instance whose bounding
/// sphere it hits. Pure Rust — the hot picking path never touches JS (invariant 4).
fn pick_nearest(shared: &Shared, view_proj: &Mat4, (w, h): (u32, u32)) -> Option<usize> {
    let st = shared.lock().unwrap();
    let (cx, cy) = st.cursor;
    let inv = view_proj.inverse();
    let ndc_x = (cx / w as f32) * 2.0 - 1.0;
    let ndc_y = 1.0 - (cy / h as f32) * 2.0;
    let near = inv.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inv.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
    let dir = (far - near).normalize_or_zero();
    let mut best: Option<(usize, f32)> = None;
    for (i, inst) in st.instances.iter().enumerate() {
        let c = Vec3::from(inst.center);
        let r = inst.scale * 1.8;
        let oc = near - c;
        let b = oc.dot(dir);
        let cc = oc.dot(oc) - r * r;
        let disc = b * b - cc;
        if disc >= 0.0 {
            let t = -b - disc.sqrt();
            if t > 0.0 && best.is_none_or(|(_, bt)| t < bt) {
                best = Some((i, t));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn make_inst_bg(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, buf: &wgpu::Buffer) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("inst-bg"),
        layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }],
    })
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
        vertex: wgpu::VertexState { module: shader, entry_point: Some(vs), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: shader, entry_point: Some("fs_main"), targets: &[Some(format.into())], compilation_options: Default::default() }),
        primitive: wgpu::PrimitiveState { topology, cull_mode: cull, ..Default::default() },
        depth_stencil: Some(depth.clone()),
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    })
}
