//! Sub-gate 1b / M2.3 — native wgpu viewport rendered to the Tauri window surface, *under* the
//! transparent React webview. The empirical compositing test: on Windows WebView2, does the native
//! layer show through the transparent viewport region (PASS) or blackout/flicker (the Graphite
//! problem)? The triangle ROTATES so the battery can test flicker under continuous viewport motion.
//!
//! `SHELL_GPU=low` selects the integrated GPU (`PowerPreference::LowPower`) for the ≥2-GPU battery
//! (M2.1's blackout was driver-sensitive); default is the discrete GPU. wgpu 29.0.3 API.

use tauri::{Manager, WebviewWindow};

/// Spawn the render loop on its own thread, targeting the given Tauri window's surface.
pub fn start(window: WebviewWindow) {
    std::thread::spawn(move || pollster::block_on(render_loop(window)));
}

async fn render_loop(window: WebviewWindow) {
    let size = window.inner_size().expect("inner_size");
    let (w, h) = (size.width.max(1), size.height.max(1));

    let instance = wgpu::Instance::default();
    let surface = match instance.create_surface(window.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[1b] create_surface FAILED: {e} — compositing not even attemptable this way");
            return;
        }
    };
    let power = if std::env::var("SHELL_GPU").as_deref() == Ok("low") {
        wgpu::PowerPreference::LowPower
    } else {
        wgpu::PowerPreference::HighPerformance
    };
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: power,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        })
        .await
        .expect("no compatible adapter");
    let info = adapter.get_info();
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("1b-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .expect("request device");

    let caps = surface.get_capabilities(&adapter);
    let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
    let alpha_mode = caps.alpha_modes[0];
    eprintln!(
        "[1b] gpu_pref={power:?} adapter='{}' backend={:?} type={:?} format={:?} alpha_modes={:?} size={w}x{h}",
        info.name, info.backend, info.device_type, format, caps.alpha_modes
    );

    let mut config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        width: w,
        height: h,
        present_mode: wgpu::PresentMode::AutoVsync,
        alpha_mode,
        view_formats: vec![],
        desired_maximum_frame_latency: 2,
    };
    surface.configure(&device, &config);

    // angle + aspect uniform (drives the rotation).
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("u"),
        size: 16,
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
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("triangle"),
        source: wgpu::ShaderSource::Wgsl(include_str!("triangle.wgsl").into()),
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

    eprintln!("[1b] render loop started — clear=dark-blue, triangle=cyan (rotating). Watch through the webview.");
    let mut frame_no: u64 = 0;
    let mut last_report = std::time::Instant::now();
    let mut acc_ms = 0.0f64;
    let mut acc_n = 0u32;
    loop {
        let t0 = std::time::Instant::now();
        if let Ok(s) = window.inner_size() {
            if (s.width.max(1), s.height.max(1)) != (config.width, config.height) {
                config.width = s.width.max(1);
                config.height = s.height.max(1);
                surface.configure(&device, &config);
            }
        }
        let angle = frame_no as f32 * 0.03;
        let aspect = config.width as f32 / config.height.max(1) as f32;
        queue.write_buffer(&uniform_buf, 0, bytemuck_u(angle, aspect).as_slice());

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
                label: Some("rp"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.04, g: 0.10, b: 0.20, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            rp.set_pipeline(&pipeline);
            rp.set_bind_group(0, &bind_group, &[]);
            rp.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
        frame.present();
        frame_no += 1;
        if frame_no == 1 {
            eprintln!("[1b] first native frame presented");
        }
        // CPU submit time (the part we control; present is vsync-bound).
        acc_ms += t0.elapsed().as_secs_f64() * 1000.0;
        acc_n += 1;
        if last_report.elapsed().as_secs_f64() >= 5.0 {
            eprintln!(
                "[1b] frame {} cpu-submit avg {:.3} ms/frame over {} frames",
                frame_no,
                acc_ms / f64::from(acc_n.max(1)),
                acc_n
            );
            acc_ms = 0.0;
            acc_n = 0;
            last_report = std::time::Instant::now();
        }
        std::thread::sleep(std::time::Duration::from_millis(16)); // ~60 fps
    }
}

/// angle + aspect packed to 16 bytes (std140 min uniform).
fn bytemuck_u(angle: f32, aspect: f32) -> Vec<u8> {
    let mut v = Vec::with_capacity(16);
    v.extend_from_slice(&angle.to_le_bytes());
    v.extend_from_slice(&aspect.to_le_bytes());
    v.extend_from_slice(&[0u8; 8]);
    v
}

/// Helper so `main` can fetch + hand off the window without importing Manager itself.
pub fn start_from_app(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        start(win);
    } else {
        eprintln!("[1b] no 'main' webview window to composite over");
    }
}
