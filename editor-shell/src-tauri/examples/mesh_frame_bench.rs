//! Offscreen frame-budget bench + render evidence for the M4 mesh path. Renders a representative
//! real-mesh scene — 5000 placeholder cubes (the M2.2 baseline) + the two imported demo meshes
//! instanced across many entities — to an offscreen texture using the *same* pipelines + WGSL as the
//! live viewport (`src/scene.wgsl`), measures CPU-submit + GPU frame time in release, reads the pixels
//! back to prove the meshes are actually drawn (not just the cubes), and writes the evidence PNG.
//!
//! This is the automatable companion to the live `[viewport]` instrumentation: no window / WebView2,
//! so it runs from the agent and reproduces a real GPU number on the documented machine.
//! Run: `cargo run -p metrocalk-editor-shell-app --release --example mesh_frame_bench`.
//! Headless with no adapter (CI) → prints a skip line and exits 0.

use metrocalk_assets::{GltfImporter, MeshGpu, MeshSource, MeshVertex};

const W: u32 = 1024;
const H: u32 = 768;
const N_CUBES: usize = 5000;
const MESH_PER_ASSET: usize = 100; // 100 health-bars + 100 props = 200 mesh instances
const FRAMES: usize = 600;
const WARMUP: usize = 120;
const CLEAR: [f64; 3] = [0.04, 0.05, 0.08];

const HEALTHBAR_GLB: &[u8] = include_bytes!("../../assets/healthbar.glb");
const PROP_GLB: &[u8] = include_bytes!("../../assets/prop.glb");

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Instance {
    center: [f32; 3],
    scale: f32,
    color: [f32; 3],
    selected: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
}

const CUBE_INDICES: [u16; 36] = [
    0, 2, 3, 0, 3, 1, 4, 5, 7, 4, 7, 6, 0, 4, 6, 0, 6, 2, 1, 3, 7, 1, 7, 5, 0, 1, 5, 0, 5, 4, 2, 6,
    7, 2, 7, 3,
];

fn main() {
    pollster::block_on(run());
}

#[allow(clippy::too_many_lines)]
async fn run() {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
    else {
        println!(
            "[mesh-bench] SKIP: no wgpu adapter available (headless host) — run on a GPU machine"
        );
        return;
    };
    let info = adapter.get_info();
    println!(
        "[mesh-bench] adapter='{}' backend={:?}",
        info.name, info.backend
    );

    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("mesh-bench"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .expect("device");

    let format = wgpu::TextureFormat::Rgba8Unorm;
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("color"),
        size: wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
    let depth = device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: W,
                height: H,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default());

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("scene"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../src/scene.wgsl").into()),
    });

    // ── scene ────────────────────────────────────────────────────────────────
    let cubes = build_cubes(N_CUBES);
    let importer = GltfImporter::new();
    let assets = [
        MeshGpu::from_asset(&importer.import(HEALTHBAR_GLB).unwrap()),
        MeshGpu::from_asset(&importer.import(PROP_GLB).unwrap()),
    ];
    let mesh_groups: Vec<Vec<Instance>> = (0..assets.len())
        .map(|a| build_mesh_instances(a, &assets[a]))
        .collect();

    // ── gpu resources ──────────────────────────────────────────────────────────
    let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("camera"),
        size: std::mem::size_of::<Camera>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let cam_bgl = bgl(&device, wgpu::BufferBindingType::Uniform);
    let inst_bgl = bgl(
        &device,
        wgpu::BufferBindingType::Storage { read_only: true },
    );
    let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &cam_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: camera_buf.as_entire_binding(),
        }],
    });

    let cube_buf = storage(&device, bytemuck::cast_slice(&cubes));
    let cube_bg = inst_bg(&device, &inst_bgl, &cube_buf);
    let cube_idx = init_buf(
        &device,
        bytemuck::cast_slice(&CUBE_INDICES),
        wgpu::BufferUsages::INDEX,
    );

    let mesh_v: Vec<wgpu::Buffer> = assets
        .iter()
        .map(|m| {
            init_buf(
                &device,
                bytemuck::cast_slice(&m.vertices),
                wgpu::BufferUsages::VERTEX,
            )
        })
        .collect();
    let mesh_i: Vec<wgpu::Buffer> = assets
        .iter()
        .map(|m| {
            init_buf(
                &device,
                bytemuck::cast_slice(&m.indices),
                wgpu::BufferUsages::INDEX,
            )
        })
        .collect();
    let mesh_inst_buf: Vec<wgpu::Buffer> = mesh_groups
        .iter()
        .map(|g| storage(&device, bytemuck::cast_slice(g)))
        .collect();
    let mesh_inst_bg: Vec<wgpu::BindGroup> = mesh_inst_buf
        .iter()
        .map(|b| inst_bg(&device, &inst_bgl, b))
        .collect();

    let depth_state = wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: Some(true),
        depth_compare: Some(wgpu::CompareFunction::Less),
        stencil: wgpu::StencilState::default(),
        bias: wgpu::DepthBiasState::default(),
    };
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&cam_bgl), Some(&inst_bgl)],
        immediate_size: 0,
    });
    let grid_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&cam_bgl)],
        immediate_size: 0,
    });
    let cube_pl = pipeline(
        &device,
        &shader,
        &layout,
        format,
        &depth_state,
        "vs_cube",
        wgpu::PrimitiveTopology::TriangleList,
        Some(wgpu::Face::Back),
        &[],
    );
    let grid_pl = pipeline(
        &device,
        &shader,
        &grid_layout,
        format,
        &depth_state,
        "vs_grid",
        wgpu::PrimitiveTopology::LineList,
        None,
        &[],
    );
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
        ],
    };
    let mesh_pl = pipeline(
        &device,
        &shader,
        &layout,
        format,
        &depth_state,
        "vs_mesh",
        wgpu::PrimitiveTopology::TriangleList,
        None,
        std::slice::from_ref(&mesh_vbl),
    );

    let cam = Camera {
        view_proj: camera_matrix(0.7, 0.42, 30.0, W as f32 / H as f32).to_cols_array_2d(),
    };
    queue.write_buffer(&camera_buf, 0, bytemuck::bytes_of(&cam));

    let grid_verts: u32 = 2 * (40 + 1) * 2;
    let n_mesh: u32 = mesh_groups.iter().map(|g| g.len() as u32).sum();

    // ── timing loop ──────────────────────────────────────────────────────────
    let mut submit_ms = Vec::with_capacity(FRAMES);
    let mut total_ms = Vec::with_capacity(FRAMES);
    for frame in 0..(WARMUP + FRAMES) {
        let t0 = std::time::Instant::now();
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: CLEAR[0],
                            g: CLEAR[1],
                            b: CLEAR[2],
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
            rp.set_pipeline(&grid_pl);
            rp.draw(0..grid_verts, 0..1);
            rp.set_pipeline(&cube_pl);
            rp.set_bind_group(1, &cube_bg, &[]);
            rp.set_index_buffer(cube_idx.slice(..), wgpu::IndexFormat::Uint16);
            rp.draw_indexed(0..CUBE_INDICES.len() as u32, 0, 0..cubes.len() as u32);
            rp.set_pipeline(&mesh_pl);
            for a in 0..assets.len() {
                if mesh_groups[a].is_empty() {
                    continue;
                }
                rp.set_bind_group(1, &mesh_inst_bg[a], &[]);
                rp.set_vertex_buffer(0, mesh_v[a].slice(..));
                rp.set_index_buffer(mesh_i[a].slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(
                    0..assets[a].indices.len() as u32,
                    0,
                    0..mesh_groups[a].len() as u32,
                );
            }
        }
        queue.submit([enc.finish()]);
        let t_submit = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let t_total = t0.elapsed().as_secs_f64() * 1000.0;
        if frame >= WARMUP {
            submit_ms.push(t_submit);
            total_ms.push(t_total);
        }
    }
    let (sp50, sp99) = pct(&mut submit_ms);
    let (tp50, tp99) = pct(&mut total_ms);
    println!(
        "[mesh-bench] {N_CUBES} cubes + {n_mesh} mesh instances (2 assets, {} verts) @ {W}x{H}, {FRAMES} frames",
        assets.iter().map(MeshGpu::vertex_count).sum::<usize>()
    );
    println!("[mesh-bench] CPU-submit p50={sp50:.3}ms p99={sp99:.3}ms | CPU+GPU(wait) p50={tp50:.3}ms p99={tp99:.3}ms (budget 16ms)");
    assert!(
        tp99 < 16.0,
        "frame budget must hold on a real-mesh scene: p99={tp99:.3}ms"
    );

    // ── evidence: a CLEAN mesh showcase (grid + the imported meshes only, close camera) ──────────
    // The timing above is the representative real-mesh scene; this frame makes the meshes legible —
    // the point being that a described/placed object renders as its mesh (a framed health-bar, a
    // faceted octahedron prop), not a placeholder cube.
    let show_cam = Camera {
        view_proj: camera_matrix(1.45, 0.2, 22.0, W as f32 / H as f32).to_cols_array_2d(),
    };
    queue.write_buffer(&camera_buf, 0, bytemuck::bytes_of(&show_cam));
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("showcase"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &color_view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: CLEAR[0],
                        g: CLEAR[1],
                        b: CLEAR[2],
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
        rp.set_pipeline(&grid_pl);
        rp.draw(0..grid_verts, 0..1);
        rp.set_pipeline(&mesh_pl);
        for a in 0..assets.len() {
            rp.set_bind_group(1, &mesh_inst_bg[a], &[]);
            rp.set_vertex_buffer(0, mesh_v[a].slice(..));
            rp.set_index_buffer(mesh_i[a].slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(
                0..assets[a].indices.len() as u32,
                0,
                0..mesh_groups[a].len() as u32,
            );
        }
    }
    queue.submit([enc.finish()]);
    let _ = device.poll(wgpu::PollType::wait_indefinitely());

    let rgba = readback(&device, &queue, &color);
    let (bg, mesh_red, mesh_teal) = classify(&rgba);
    let covered = (W * H) as usize - bg;
    println!(
        "[mesh-bench] showcase pixels: {covered} non-background of {} ({:.1}%); healthbar-red={mesh_red} prop-teal={mesh_teal}",
        W * H,
        100.0 * covered as f64 / (W * H) as f64
    );
    assert!(
        mesh_red > 200,
        "the imported health-bar mesh is visibly drawn (red pixels)"
    );
    assert!(
        mesh_teal > 200,
        "the imported prop mesh is visibly drawn (teal pixels)"
    );

    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("evidence")
        .join("m4-mesh-scene.png");
    write_png(&out, &rgba);
    println!("[mesh-bench] wrote evidence {}", out.display());
}

/// 5000 cubes spread deterministically in a volume (same shape as the live seed), bright hash colors.
fn build_cubes(n: usize) -> Vec<Instance> {
    let mut s: u64 = 0x4D45_5452_4F43_4131;
    let mut rng = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f32 / (1u64 << 53) as f32
    };
    let extent = 18.0f32;
    (0..n)
        .map(|i| {
            let p = [
                (rng() * 2.0 - 1.0) * extent,
                (rng() * 2.0 - 1.0) * extent,
                (rng() * 2.0 - 1.0) * extent,
            ];
            let mut h: u32 = 2_166_136_261;
            for b in i.to_le_bytes() {
                h = (h ^ u32::from(b)).wrapping_mul(16_777_619);
            }
            Instance {
                center: p,
                scale: 0.45,
                color: [
                    0.4 + (h & 0xff) as f32 / 425.0,
                    0.4 + ((h >> 8) & 0xff) as f32 / 425.0,
                    0.4 + ((h >> 16) & 0xff) as f32 / 425.0,
                ],
                selected: 0.0,
            }
        })
        .collect()
}

/// Mesh entities in a visible cluster near the origin (so the evidence frame shows them), normalized
/// to the same on-screen scale the shell uses (0.9 / max_extent).
fn build_mesh_instances(asset_idx: usize, gpu: &MeshGpu) -> Vec<Instance> {
    let ext = gpu
        .vertices
        .iter()
        .flat_map(|v| v.position)
        .fold(0.0f32, |m, c| m.max(c.abs()))
        .max(0.01)
        * 2.0;
    let scale = 0.9 / ext;
    let cols = 10;
    (0..MESH_PER_ASSET)
        .map(|k| {
            let col = (k % cols) as f32;
            let row = (k / cols) as f32;
            // health-bars on one side, props on the other, in a tidy grid the readback can see.
            let x = if asset_idx == 0 { -6.0 } else { 6.0 } + (col - 5.0) * 1.2;
            Instance {
                center: [x, (row - 5.0) * 1.2, 0.0],
                scale,
                color: [1.0, 1.0, 1.0],
                selected: 0.0,
            }
        })
        .collect()
}

fn camera_matrix(orbit: f32, elevation: f32, distance: f32, aspect: f32) -> glam::Mat4 {
    let eye = glam::Vec3::new(
        orbit.cos() * distance * elevation.cos(),
        distance * elevation.sin(),
        orbit.sin() * distance * elevation.cos(),
    );
    let proj = glam::Mat4::perspective_rh(55f32.to_radians(), aspect, 0.1, distance * 8.0 + 100.0);
    proj * glam::Mat4::look_at_rh(eye, glam::Vec3::ZERO, glam::Vec3::Y)
}

fn pct(v: &mut [f64]) -> (f64, f64) {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (v[v.len() / 2], v[v.len() * 99 / 100])
}

/// (background, healthbar-red, prop-teal) pixel counts. Cubes are bright in all channels (≥~0.4);
/// the health-bar fill is strong red (low green), the prop is teal (low red, high blue) — so these
/// thresholds isolate mesh pixels from both background and cubes.
fn classify(rgba: &[u8]) -> (usize, usize, usize) {
    let (mut bg, mut red, mut teal) = (0, 0, 0);
    let near = |c: u8, target: f64| (f64::from(c) - target * 255.0).abs() < 12.0;
    for px in rgba.chunks_exact(4) {
        let (r, g, b) = (px[0], px[1], px[2]);
        if near(r, CLEAR[0]) && near(g, CLEAR[1]) && near(b, CLEAR[2]) {
            bg += 1;
        } else if r > 150 && g < 90 && b < 90 {
            red += 1;
        } else if r < 110 && g > 110 && b > 110 {
            teal += 1;
        }
    }
    (bg, red, teal)
}

fn readback(device: &wgpu::Device, queue: &wgpu::Queue, tex: &wgpu::Texture) -> Vec<u8> {
    let bpr = W * 4; // 1024*4 = 4096, already 256-aligned
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(bpr * H),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: tex,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(H),
            },
        },
        wgpu::Extent3d {
            width: W,
            height: H,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    buf.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.expect("map"));
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = buf.slice(..).get_mapped_range().to_vec();
    buf.unmap();
    data
}

fn write_png(path: &std::path::Path, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

// ── small wgpu helpers ─────────────────────────────────────────────────────────
fn bgl(device: &wgpu::Device, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}
fn inst_bg(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buf.as_entire_binding(),
        }],
    })
}
fn storage(device: &wgpu::Device, data: &[u8]) -> wgpu::Buffer {
    init_buf(device, data, wgpu::BufferUsages::STORAGE)
}
fn init_buf(device: &wgpu::Device, data: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: data,
        usage,
    })
}
#[allow(clippy::too_many_arguments)]
fn pipeline(
    device: &wgpu::Device,
    shader: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    format: wgpu::TextureFormat,
    depth: &wgpu::DepthStencilState,
    vs: &str,
    topology: wgpu::PrimitiveTopology,
    cull: Option<wgpu::Face>,
    buffers: &[wgpu::VertexBufferLayout],
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(vs),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module: shader,
            entry_point: Some(vs),
            buffers,
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: shader,
            entry_point: Some("fs_main"),
            targets: &[Some(format.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
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
