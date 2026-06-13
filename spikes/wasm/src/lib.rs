//! M0 spike ③: one wgpu codebase, two targets (native window + browser canvas).
//! Spinning triangle + a frame-time overlay (window title on native, a DOM div on web).
//! Also dumps adapter info / limits / bindless-relevant features once at startup.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::window::{Window, WindowId};

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

const SHADER: &str = r#"
struct Uniforms { angle: f32, aspect: f32 };
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec3<f32> };

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VsOut {
    // equilateral triangle
    var base = array<vec2<f32>, 3>(
        vec2<f32>( 0.0,  0.6),
        vec2<f32>(-0.52, -0.3),
        vec2<f32>( 0.52, -0.3),
    );
    var cols = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.15, 0.30),
        vec3<f32>(0.15, 0.85, 0.40),
        vec3<f32>(0.20, 0.45, 1.0),
    );
    let c = cos(u.angle); let s = sin(u.angle);
    var p = base[vi];
    p = vec2<f32>(p.x * c - p.y * s, p.x * s + p.y * c);
    p.x = p.x / u.aspect; // keep shape square regardless of canvas aspect
    var out: VsOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.color = cols[vi];
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

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

struct Gfx {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    window: Arc<Window>,
    // timing
    start_ms: f64,
    last_frame_ms: f64,
    ttff_logged: bool,
    ttff_ms: f64,
    frame_count: u64,
    fps_accum_ms: f64,
    fps_frames: u32,
    // native bench mode: when SPIKE_FRAMES is set, collect dts then exit & report.
    bench_cap: u64,
    frame_dts: Vec<f64>,
}

impl Gfx {
    async fn new(window: Arc<Window>) -> Gfx {
        let start_ms = now_ms();
        let mut size = window.inner_size();
        size.width = size.width.max(1);
        size.height = size.height.max(1);
        // winit's web backend often leaves the canvas backing at 1x1 in headless / pre-layout;
        // force a real surface size and matching canvas backing so we render more than one texel.
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            if size.width <= 1 || size.height <= 1 {
                size = winit::dpi::PhysicalSize::new(512, 512);
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

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits()),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: wgpu::MemoryHints::Performance,
                trace: wgpu::Trace::Off,
            })
            .await
            .expect("request device");

        dump_limits(&device);

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("triangle"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("uniforms"),
            size: 16, // angle:f32, aspect:f32 + pad to 16
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buf.as_entire_binding(),
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("layout"),
            bind_group_layouts: &[Some(&bgl)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        Gfx {
            surface,
            device,
            queue,
            config,
            pipeline,
            uniform_buf,
            bind_group,
            window,
            start_ms,
            last_frame_ms: start_ms,
            ttff_logged: false,
            ttff_ms: 0.0,
            frame_count: 0,
            fps_accum_ms: 0.0,
            fps_frames: 0,
            bench_cap: bench_cap(),
            frame_dts: Vec::new(),
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        // Ignore 1x1 (winit web emits a spurious tiny resize pre-layout that blanks the surface).
        if w > 1 && h > 1 {
            self.config.width = w;
            self.config.height = h;
            self.surface.configure(&self.device, &self.config);
        }
    }

    fn render(&mut self) {
        let t = now_ms();
        let dt = t - self.last_frame_ms;
        self.last_frame_ms = t;

        let angle = ((t - self.start_ms) / 1000.0) as f32; // ~1 rad/s spin
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        self.queue.write_buffer(&self.uniform_buf, 0, bytemuck_pair(angle, aspect).as_slice());

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            _ => return, // Timeout / Occluded / Validation: skip this frame
        };
        let view = frame.texture.create_view(&wgpu::TextureViewDescriptor::default());
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("rp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.08, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&self.pipeline);
            rp.set_bind_group(0, &self.bind_group, &[]);
            rp.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
        frame.present();

        self.frame_count += 1;
        if !self.ttff_logged {
            self.ttff_logged = true;
            self.ttff_ms = t - self.start_ms;
            log_line(&format!("TTFF (gfx-new → first present): {:.1} ms", self.ttff_ms));
            set_overlay(&format!("first frame {:.0} ms", self.ttff_ms));
        }
        // bench: skip 60 warmup frames, then collect dt
        if self.bench_cap > 0 && self.frame_count > 60 {
            self.frame_dts.push(dt);
        }

        // overlay update ~4x/sec
        self.fps_accum_ms += dt;
        self.fps_frames += 1;
        if self.fps_accum_ms >= 250.0 {
            let avg = self.fps_accum_ms / self.fps_frames as f64;
            let msg = format!("{avg:.2} ms/frame  ({:.0} fps)  frame #{}", 1000.0 / avg, self.frame_count);
            set_overlay(&msg);
            #[cfg(not(target_arch = "wasm32"))]
            self.window.set_title(&format!("Metrocalk wasm spike — {msg}"));
            self.fps_accum_ms = 0.0;
            self.fps_frames = 0;
        }
    }
}

impl Gfx {
    fn bench_done(&self) -> bool {
        self.bench_cap > 0 && self.frame_count >= self.bench_cap
    }
    fn report_bench(&mut self) {
        let mut d = std::mem::take(&mut self.frame_dts);
        if d.is_empty() {
            return;
        }
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = d.len();
        let med = d[n / 2];
        let p99 = d[((n as f64 * 0.99).ceil() as usize).min(n) - 1];
        let max = d[n - 1];
        log_line(&format!(
            "BENCH native: frames={} TTFF={:.1}ms steady median={:.3}ms p99={:.3}ms max={:.3}ms (~{:.0} fps median)",
            self.frame_count, self.ttff_ms, med, p99, max, 1000.0 / med
        ));
    }
}

/// Native bench frame cap from SPIKE_FRAMES env (0 = run forever / visual mode).
fn bench_cap() -> u64 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("SPIKE_FRAMES").ok().and_then(|s| s.parse().ok()).unwrap_or(0)
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// angle + aspect packed into 16 bytes (std140 min uniform size).
fn bytemuck_pair(a: f32, b: f32) -> Vec<u8> {
    let mut v = Vec::with_capacity(16);
    v.extend_from_slice(&a.to_le_bytes());
    v.extend_from_slice(&b.to_le_bytes());
    v.extend_from_slice(&[0u8; 8]);
    v
}

#[derive(Default)]
struct App {
    gfx: Rc<RefCell<Option<Gfx>>>,
    window: Option<Arc<Window>>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        #[allow(unused_mut)]
        let mut attrs = Window::default_attributes().with_title("Metrocalk wasm spike");
        #[cfg(target_arch = "wasm32")]
        {
            use wasm_bindgen::JsCast;
            use winit::platform::web::WindowAttributesExtWebSys;
            // Use the explicitly-sized #app canvas from index.html so the surface isn't 1x1.
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
        #[cfg(target_arch = "wasm32")]
        {
            wasm_bindgen_futures::spawn_local(async move {
                let g = Gfx::new(window.clone()).await;
                *gfx.borrow_mut() = Some(g);
                window.request_redraw();
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            *gfx.borrow_mut() = Some(pollster::block_on(Gfx::new(window.clone())));
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
                            g.report_bench();
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
                    w.request_redraw(); // continuous animation
                }
            }
            _ => {}
        }
    }
}

fn run_event_loop() {
    let event_loop = EventLoop::new().expect("event loop");
    event_loop.set_control_flow(winit::event_loop::ControlFlow::Poll);
    let mut app = App::default();
    #[cfg(not(target_arch = "wasm32"))]
    event_loop.run_app(&mut app).expect("run");
    #[cfg(target_arch = "wasm32")]
    {
        use winit::platform::web::EventLoopExtWebSys;
        event_loop.spawn_app(app);
    }
}

/// Native entry.
#[cfg(not(target_arch = "wasm32"))]
pub fn run() {
    env_logger::init();
    run_event_loop();
}

/// Wasm entry — called by the wasm-bindgen glue on page load.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub fn wasm_main() {
    console_error_panic_hook::set_once();
    let coi = cross_origin_isolated();
    log_line("metrocalk-wasm-spike booting");
    log_line(&format!("crossOriginIsolated = {coi}"));
    log_line(&format!("navigator.gpu present = {}", gpu_present()));
    // Persistent evidence banner (never overwritten by the frame-time overlay), so a single
    // screenshot proves both the render AND cross-origin isolation.
    set_banner(&format!("crossOriginIsolated = {coi}  ·  navigator.gpu = {}", gpu_present()));
    run_event_loop();
}

// ---------- logging / overlay / COI helpers ----------

fn log_line(s: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::console::log_1(&s.into());
        // Also accumulate into globalThis.__spikelog so a headless driver can read the full log
        // regardless of when it attached.
        let g = js_sys::global();
        let key = JsValue::from_str("__spikelog");
        let prev = js_sys::Reflect::get(&g, &key).ok().and_then(|v| v.as_string()).unwrap_or_default();
        let _ = js_sys::Reflect::set(&g, &key, &JsValue::from_str(&format!("{prev}{s}\n")));
    }
    #[cfg(not(target_arch = "wasm32"))]
    println!("{s}");
}

#[cfg(target_arch = "wasm32")]
fn cross_origin_isolated() -> bool {
    // window.crossOriginIsolated
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
    let doc = web_sys::window().unwrap().document().unwrap();
    let el = match doc.get_element_by_id("overlay") {
        Some(e) => e,
        None => {
            let e = doc.create_element("div").unwrap();
            e.set_id("overlay");
            let html: web_sys::HtmlElement = e.clone().dyn_into().unwrap();
            html.style().set_property("position", "fixed").ok();
            html.style().set_property("top", "8px").ok();
            html.style().set_property("left", "8px").ok();
            html.style().set_property("font", "13px monospace").ok();
            html.style().set_property("color", "#0f0").ok();
            html.style().set_property("background", "rgba(0,0,0,0.6)").ok();
            html.style().set_property("padding", "4px 8px").ok();
            html.style().set_property("z-index", "10").ok();
            doc.body().unwrap().append_child(&e).ok();
            e
        }
    };
    el.set_text_content(Some(text));
}

#[cfg(not(target_arch = "wasm32"))]
fn set_overlay(_text: &str) {}

// ---------- diagnostics: adapter + limits + bindless flags ----------

fn dump_adapter(adapter: &wgpu::Adapter) {
    let info = adapter.get_info();
    log_line(&format!(
        "ADAPTER name='{}' backend={:?} type={:?} driver='{}'",
        info.name, info.backend, info.device_type, info.driver
    ));
    let f = adapter.features();
    // "bindless"-relevant features (non-uniform indexing / binding arrays).
    let bindless = [
        ("TEXTURE_BINDING_ARRAY", wgpu::Features::TEXTURE_BINDING_ARRAY),
        ("BUFFER_BINDING_ARRAY", wgpu::Features::BUFFER_BINDING_ARRAY),
        ("STORAGE_RESOURCE_BINDING_ARRAY", wgpu::Features::STORAGE_RESOURCE_BINDING_ARRAY),
        (
            "SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING",
            wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING,
        ),
        ("PARTIALLY_BOUND_BINDING_ARRAY", wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY),
    ];
    for (name, feat) in bindless {
        log_line(&format!("  bindless feature {name}: {}", f.contains(feat)));
    }
    // ADAPTER limits = true hardware capability (what the backend reports), independent of the
    // portable floor we constrain the device to below.
    let l = adapter.limits();
    log_line(&format!(
        "ADAPTER LIMITS max_texture_2d={} max_buffer_size={} max_bind_groups={} max_storage_buffers_per_stage={} max_storage_textures_per_stage={} max_uniform_buffer_binding_size={} max_compute_invocations_per_wg={} max_bindings_per_bind_group={}",
        l.max_texture_dimension_2d,
        l.max_buffer_size,
        l.max_bind_groups,
        l.max_storage_buffers_per_shader_stage,
        l.max_storage_textures_per_shader_stage,
        l.max_uniform_buffer_binding_size,
        l.max_compute_invocations_per_workgroup,
        l.max_bindings_per_bind_group,
    ));
}

fn dump_limits(device: &wgpu::Device) {
    let l = device.limits();
    // Device limits = the portable floor we REQUESTED (downlevel_webgl2_defaults) — what code that
    // must also run on the web is actually allowed to use.
    log_line(&format!(
        "DEVICE LIMITS (requested webgl2 downlevel floor) max_texture_2d={} max_buffer_size={} max_storage_buffers_per_stage={} max_compute_invocations_per_wg={}",
        l.max_texture_dimension_2d,
        l.max_buffer_size,
        l.max_storage_buffers_per_shader_stage,
        l.max_compute_invocations_per_workgroup,
    ));
}
