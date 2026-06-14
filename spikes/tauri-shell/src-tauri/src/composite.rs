//! Sub-gate 1b — native wgpu triangle rendered to the Tauri window surface, *under* the
//! transparent React webview. The empirical compositing test: on Windows WebView2, does the
//! triangle show through the transparent viewport region (PASS) or does the webview occlude /
//! flicker against the native layer (FAIL → the Graphite problem → CEF pivot)?
//!
//! wgpu 29.0.3 API mirrors the project's render spike (`spikes/wasm`) so it tracks the real engine.

use tauri::{WebviewWindow, Manager};

/// Spawn the render loop on its own thread, targeting the given Tauri window's surface.
pub fn start(window: WebviewWindow) {
    std::thread::spawn(move || pollster::block_on(render_loop(window)));
}

async fn render_loop(window: WebviewWindow) {
    let size = window.inner_size().expect("inner_size");
    let (w, h) = (size.width.max(1), size.height.max(1));

    let instance = wgpu::Instance::default();
    // Surface straight from the Tauri window (it implements raw-window-handle 0.6).
    let surface = match instance.create_surface(window.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[1b] create_surface FAILED: {e} — compositing not even attemptable this way");
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
        "[1b] adapter='{}' backend={:?} format={:?} alpha_modes={:?} size={w}x{h}",
        info.name, info.backend, format, caps.alpha_modes
    );

    let config = wgpu::SurfaceConfiguration {
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

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("triangle"),
        source: wgpu::ShaderSource::Wgsl(include_str!("triangle.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("layout"),
        bind_group_layouts: &[],
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

    eprintln!("[1b] render loop started — clear=dark-blue, triangle=cyan. Watch for it through the webview.");
    let mut frame_no: u64 = 0;
    loop {
        // re-read size so a resize is roughly tracked (cheap; full event wiring is out of scope)
        if let Ok(s) = window.inner_size() {
            if (s.width.max(1), s.height.max(1)) != (config.width, config.height) {
                let mut c = config.clone();
                c.width = s.width.max(1);
                c.height = s.height.max(1);
                surface.configure(&device, &c);
            }
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
            rp.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
        frame.present();
        frame_no += 1;
        if frame_no == 1 {
            eprintln!("[1b] first native frame presented");
        }
        std::thread::sleep(std::time::Duration::from_millis(16)); // ~60 fps
    }
}

/// Helper so `main` can fetch + hand off the window without importing Manager itself.
pub fn start_from_app(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        start(win);
    } else {
        eprintln!("[1b] no 'main' webview window to composite over");
    }
}
