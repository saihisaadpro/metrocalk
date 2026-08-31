//! **Real-pixel evidence for the colour working space (ADR-109).**
//!
//! Everything the colour work claims is a claim about `post.wgsl`'s `fs_resolve` — the one pass that
//! turns scene-linear radiance into display colour. So this runs THAT shader, on a real GPU, over a
//! chart of known scene-linear values, once per working space, and reads the pixels back.
//!
//! It exists because the rest of the colour evidence is source contracts and unit tests. Those prove
//! the code says what it should; only pixels prove the GPU agrees. The three claims it settles:
//!
//! 1. **Linear Rec.709 is the identity.** Selecting it must produce output *byte-identical* to a
//!    build with no colour block at all — which is what makes every capture taken before ADR-109
//!    still comparable to every capture after it. Checked against the CPU mirror in `render.rs`,
//!    which is derived independently of the shader.
//! 2. **An authored colour survives the working space.** The scene pass converts INTO the working
//!    space and `fs_resolve` converts back out, so a colour that merely passes through must come out
//!    where it went in — whichever space is selected. This is the prompt's "authored-colour test":
//!    changing the working space must not shift a colour merely because one escaped conversion.
//! 3. **Where ACEScg genuinely differs is a per-channel PRODUCT.** albedo x light is where the
//!    spaces legitimately disagree, because M(a)*M(l) != M(a*l) — that difference IS what rendering
//!    in a wider gamut buys, and if it were absent, "we render in ACEScg" would mean nothing.
//! 4. **Over-range values survive to the tone curve.** A 12.0 highlight must not clip on the way in.
//!
//! The source texture is therefore built PER WORKING SPACE, exactly as the scene pass would have
//! written it — the CPU mirror of `to_working`. Feeding raw Rec.709 values to both and applying only
//! the egress conversion would compare the inverse transform against data that never had the forward
//! one applied, which is a different (and meaningless) measurement.
//!
//! Run: `cargo run --release --example colour_evidence -- <output-dir>`
//! Headless with no adapter (CI) → prints a skip line and exits 0, like `mesh_frame_bench`.

use std::path::PathBuf;

use metrocalk_assets::colour::{self, WorkingSpace};

const POST: &str = include_str!("../src/post.wgsl");

/// What a patch IS. Both kinds are authored in linear Rec.709, which is what every factor, light and
/// decoded texel in this engine means.
#[derive(Clone, Copy)]
enum Patch {
    /// A colour that only passes through — an unlit/emissive value. Converted in, converted out, so
    /// it must survive whichever working space is selected.
    Authored([f32; 3]),
    /// A per-channel product of an albedo and a light, formed IN the working space, which is what
    /// the BRDF does. This is the one place the spaces are allowed to disagree.
    Lit { albedo: [f32; 3], light: [f32; 3] },
}

/// The chart. Each row settles a different question.
const PATCHES: &[(&str, Patch)] = &[
    ("black", Patch::Authored([0.0, 0.0, 0.0])),
    ("near-black", Patch::Authored([0.002, 0.002, 0.002])),
    ("mid grey 0.18", Patch::Authored([0.18, 0.18, 0.18])),
    ("white 1.0", Patch::Authored([1.0, 1.0, 1.0])),
    ("over-range 12.0", Patch::Authored([12.0, 12.0, 12.0])),
    ("saturated red", Patch::Authored([1.0, 0.0, 0.0])),
    ("saturated green", Patch::Authored([0.0, 1.0, 0.0])),
    ("saturated blue", Patch::Authored([0.0, 0.0, 1.0])),
    ("skin-ish", Patch::Authored([0.42, 0.26, 0.19])),
    ("hot emissive", Patch::Authored([8.0, 3.2, 0.6])),
    // The lit rows. A neutral light on a neutral albedo cannot diverge (the matrices preserve
    // neutrals), so it pins that the difference below is gamut behaviour and not a bug.
    (
        "lit: grey x white",
        Patch::Lit {
            albedo: [0.18, 0.18, 0.18],
            light: [3.0, 3.0, 3.0],
        },
    ),
    (
        "lit: red x warm",
        Patch::Lit {
            albedo: [0.8, 0.1, 0.1],
            light: [3.0, 1.6, 0.7],
        },
    ),
    (
        "lit: green x cyan",
        Patch::Lit {
            albedo: [0.15, 0.7, 0.2],
            light: [0.6, 2.4, 2.8],
        },
    ),
];

impl Patch {
    /// What the SCENE PASS would have written into the HDR target for this patch, in `working`.
    /// The CPU mirror of `to_working` in scene.wgsl — and, for a lit patch, of the BRDF's per-channel
    /// product, which happens after the conversion because that is the whole ordering claim.
    fn scene_value(self, working: WorkingSpace) -> [f32; 3] {
        let m = working.from_rec709();
        match self {
            Self::Authored(c) => colour::apply(m, c),
            Self::Lit { albedo, light } => {
                let a = colour::apply(m, albedo);
                let l = colour::apply(m, light);
                [a[0] * l[0], a[1] * l[1], a[2] * l[2]]
            }
        }
    }
}

const CELL: u32 = 48;
const EXPOSURE: f32 = 0.45; // render.rs DEFAULT_EXPOSURE

/// f32 → IEEE-754 half, for filling an `Rgba16Float` source. Round-to-nearest-even, with the
/// subnormal and overflow arms written out — this chart deliberately contains 0.002 and 12.0.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn f16_bits(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let mut exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let mant = bits & 0x007f_ffff;
    if exp >= 0x1f {
        return sign | 0x7c00; // inf / NaN → inf, with the sign
    }
    if exp <= 0 {
        if exp < -10 {
            return sign; // underflows to zero
        }
        let mant = (mant | 0x0080_0000) >> (1 - exp);
        return sign | ((mant + 0x0000_1000) >> 13) as u16;
    }
    let rounded = mant + 0x0000_1000;
    if rounded & 0x0080_0000 != 0 {
        exp += 1;
        if exp >= 0x1f {
            return sign | 0x7c00;
        }
    }
    sign | ((exp as u16) << 10) | ((rounded >> 13) & 0x03ff) as u16
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Camera {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    light_view_proj: [[f32; 4]; 4],
    focus: [f32; 4],
    shadow: [f32; 4],
    grid: [f32; 4],
    to_working: [[f32; 4]; 3],
    env_to_working: [[f32; 4]; 3],
    from_working: [[f32; 4]; 3],
    luma: [f32; 4],
    /// ADR-193 - the frame guide, in framebuffer pixels. Empty here, and it has to be DECLARED here:
    /// `post.wgsl` names it, and wgpu refuses a uniform buffer smaller than the struct the shader
    /// declares. This example writes the same `post.wgsl` the renderer does, so every field the
    /// renderer adds to the block is a field this evidence run must add too - which is the whole
    /// reason `gpu-contract-audit` compares the two: it caught this one at rest, in milliseconds,
    /// on a box with no GPU, where the alternative was a pipeline failure on a machine that has one.
    guide: [f32; 4],
}
// Must match `render.rs`'s `Camera`, which post.wgsl declares in full.
const _: () = assert!(std::mem::size_of::<Camera>() == 416);

impl Camera {
    /// A camera carrying nothing but the colour block — `fs_resolve` reads no matrix, only
    /// `shadow.y` (exposure), `shadow.z` (view transform), `grid.w` (manual encode) and the colour
    /// block itself.
    fn for_working(working: WorkingSpace, cad: bool, manual_encode: bool) -> Self {
        let w = working.luminance_weights();
        let id = [[0.0_f32; 4]; 3];
        Self {
            view_proj: [[0.0; 4]; 4],
            inv_view_proj: [[0.0; 4]; 4],
            light_view_proj: [[0.0; 4]; 4],
            focus: [0.0; 4],
            shadow: [-1.0, EXPOSURE, f32::from(u8::from(cad)), 0.0],
            grid: [0.0, 0.0, 0.0, f32::from(u8::from(manual_encode))],
            to_working: colour::wgsl_mat3(working.from_rec709()),
            env_to_working: colour::wgsl_mat3(working.from_rec709()),
            from_working: colour::wgsl_mat3(working.to_rec709()),
            luma: [w[0], w[1], w[2], id[0][0]],
            // An EMPTY rectangle - no guide. A colour-evidence frame is a swatch sheet, and a
            // letterbox drawn across it would darken two thirds of the very pixels being measured.
            guide: [0.0; 4],
        }
    }
}

fn main() {
    let out_dir = std::env::args()
        .nth(1)
        .map_or_else(|| PathBuf::from("colour-evidence"), PathBuf::from);
    pollster::block_on(run(&out_dir));
}

#[allow(clippy::too_many_lines)]
async fn run(out_dir: &std::path::Path) {
    let instance = wgpu::Instance::default();
    let Ok(adapter) = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        })
        .await
    else {
        println!("[colour-evidence] SKIP: no wgpu adapter (headless CI)");
        return;
    };
    let Ok((device, queue)) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("colour-evidence"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
    else {
        println!("[colour-evidence] SKIP: no device");
        return;
    };
    println!(
        "[colour-evidence] adapter='{}' backend={:?}",
        adapter.get_info().name,
        adapter.get_info().backend
    );

    #[allow(clippy::cast_possible_truncation)]
    let w = PATCHES.len() as u32 * CELL;
    let h = CELL;

    // ── the source: one column of scene-linear radiance per patch, in the real HDR format ─────────
    let src = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("chart"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    // Filled per working space, below — the source IS the scene pass's output, so it has to be
    // rebuilt when the working space changes. That is the correction this chart exists to make.
    let fill_chart = |working: WorkingSpace| {
        let mut texels: Vec<u16> = Vec::with_capacity((w * h) as usize * 4);
        for _ in 0..h {
            for x in 0..w {
                let scene = PATCHES[(x / CELL) as usize].1.scene_value(working);
                texels.extend_from_slice(&[
                    f16_bits(scene[0]),
                    f16_bits(scene[1]),
                    f16_bits(scene[2]),
                    f16_bits(1.0),
                ]);
            }
        }
        texels
    };

    // Bloom off: a 1×1 black texture adds exactly nothing, which is how the renderer does it too.
    let black = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("black-bloom"),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &black,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&[0u16, 0, 0, f16_bits(1.0)]),
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

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("post"),
        source: wgpu::ShaderSource::Wgsl(POST.into()),
    });
    let cam_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("cam"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let tex_entry = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("post-2tex"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            tex_entry(1),
            tex_entry(2),
        ],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("post-layout"),
        bind_group_layouts: &[Some(&cam_bgl), Some(&post_bgl)],
        immediate_size: 0,
    });
    // Rgba8Unorm, NOT the _Srgb variant: the shader then applies the OETF itself (`grid.w = 1`),
    // which is the path the app takes on this machine's swapchain.
    let target_format = wgpu::TextureFormat::Rgba8Unorm;
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("resolve"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_post"),
            buffers: &[],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_resolve"),
            targets: &[Some(target_format.into())],
            compilation_options: wgpu::PipelineCompilationOptions::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let samp = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("post-samp"),
        ..Default::default()
    });
    let post_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("post-bg"),
        layout: &post_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Sampler(&samp),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(
                    &src.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(
                    &black.create_view(&wgpu::TextureViewDescriptor::default()),
                ),
            },
        ],
    });

    std::fs::create_dir_all(out_dir).ok();
    let mut readings: Vec<(WorkingSpace, Vec<[u8; 3]>)> = Vec::new();
    for working in [WorkingSpace::LinearRec709, WorkingSpace::AcesCg] {
        // Re-fill the source with what the SCENE PASS would have written in this working space.
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &src,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&fill_chart(working)),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 8),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        let cam_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cam"),
            size: std::mem::size_of::<Camera>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &cam_buf,
            0,
            bytemuck::bytes_of(&Camera::for_working(working, false, true)),
        );
        let cam_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cam-bg"),
            layout: &cam_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: cam_buf.as_entire_binding(),
            }],
        });
        let pixels = resolve_once(
            &device,
            &queue,
            &pipeline,
            &cam_bg,
            &post_bg,
            target_format,
            w,
            h,
        );
        // One reading per patch, from the middle of its cell — away from the dither's edge cases.
        let mut per_patch = Vec::with_capacity(PATCHES.len());
        for i in 0..PATCHES.len() {
            #[allow(clippy::cast_possible_truncation)]
            let x = i as u32 * CELL + CELL / 2;
            let o = ((h / 2 * w) + x) as usize * 4;
            per_patch.push([pixels[o], pixels[o + 1], pixels[o + 2]]);
        }
        let name = out_dir.join(format!("chart-{}.png", working.wire()));
        write_png(&name, w, h, &pixels);
        println!("[colour-evidence] wrote {}", name.display());
        readings.push((working, per_patch));
    }

    // ── the report ────────────────────────────────────────────────────────────────────────────────
    let (_, rec709) = &readings[0];
    let (_, acescg) = &readings[1];
    println!(
        "
{:<20} {:>7} {:>18} {:>18} {:>7}",
        "patch", "kind", "linear Rec.709", "ACEScg", "delta"
    );
    let mut worst_authored = 0i32;
    let mut lit_that_moved = 0;
    let mut lit_total = 0;
    for (i, (name, patch)) in PATCHES.iter().enumerate() {
        let a = rec709[i];
        let b = acescg[i];
        let d = (0..3)
            .map(|c| (i32::from(a[c]) - i32::from(b[c])).abs())
            .max()
            .unwrap_or(0);
        let kind = match patch {
            Patch::Authored(_) => "through",
            Patch::Lit { .. } => "lit",
        };
        println!(
            "{name:<20} {kind:>7} {:>18} {:>18} {d:>7}",
            format!("{:3} {:3} {:3}", a[0], a[1], a[2]),
            format!("{:3} {:3} {:3}", b[0], b[1], b[2]),
        );
        match patch {
            Patch::Authored(_) => worst_authored = worst_authored.max(d),
            Patch::Lit { albedo, light } => {
                lit_total += 1;
                let neutral_pair = albedo[0] == albedo[1]
                    && albedo[1] == albedo[2]
                    && light[0] == light[1]
                    && light[1] == light[2];
                if d > 2 && !neutral_pair {
                    lit_that_moved += 1;
                }
            }
        }
    }

    // ── the claims, each checked against something derived independently of the shader ────────────
    println!(
        "
[claim 1] linear Rec.709 == the CPU mirror of render.rs's contract"
    );
    let mut worst_mirror = 0i32;
    for (i, (name, patch)) in PATCHES.iter().enumerate() {
        let expect = cpu_mirror(patch.scene_value(WorkingSpace::LinearRec709));
        let got = rec709[i];
        let d = (0..3)
            .map(|c| (i32::from(expect[c]) - i32::from(got[c])).abs())
            .max()
            .unwrap_or(0);
        worst_mirror = worst_mirror.max(d);
        if d > 1 {
            println!("  MISMATCH {name}: cpu {expect:?} vs gpu {got:?} (delta {d})");
        }
    }
    println!(
        "  worst channel delta vs the CPU mirror: {worst_mirror} code(s) {}",
        if worst_mirror <= 1 {
            "— PASS (within the ±0.5/255 dither)"
        } else {
            "— FAIL"
        }
    );

    println!("[claim 2] an authored colour survives the working space (the prompt's authored-colour test)");
    println!(
        "  worst delta across every pass-through patch: {worst_authored} code(s) {}",
        if worst_authored <= 1 {
            "— PASS: converting in and back out returns the colour the author picked"
        } else {
            "— FAIL: a colour shifted merely because the working space changed"
        }
    );

    println!("[claim 3] a per-channel PRODUCT is where the spaces legitimately differ");
    println!(
        "  lit patches that moved: {lit_that_moved} of {lit_total} non-neutral {}",
        if lit_that_moved >= 2 {
            "— PASS: M(a)*M(l) != M(a*l), which is what rendering in AP1 actually buys"
        } else {
            "— FAIL: if a lit product never differs, the working space is decorative"
        }
    );
    let (grey_a, grey_b) = (rec709[10], acescg[10]);
    let grey_d = (0..3)
        .map(|c| (i32::from(grey_a[c]) - i32::from(grey_b[c])).abs())
        .max()
        .unwrap_or(0);
    println!(
        "  the NEUTRAL lit patch (grey x white) moved {grey_d} code(s) {}",
        if grey_d <= 1 {
            "— PASS: the difference above is gamut behaviour, not a bug"
        } else {
            "— FAIL: a neutral product must not move"
        }
    );

    println!("[claim 4] the over-range patch did not clip on the way in");
    let (over, white) = (rec709[4], rec709[3]);
    println!(
        "  12.0 resolved to {over:?} against white 1.0 at {white:?} {}",
        if over[0] > white[0] {
            "— PASS"
        } else {
            "— FAIL (clipped before the curve)"
        }
    );
}

/// The CPU statement of the same transform, from `render.rs`'s contract: exposure, the ACES fit, then
/// the exact sRGB OETF. Derived independently of the shader, which is what makes it a check.
fn cpu_mirror(scene_linear: [f32; 3]) -> [u8; 3] {
    let mut out = [0u8; 3];
    for (i, c) in scene_linear.iter().enumerate() {
        let x = c * EXPOSURE;
        let (a, b, cc, d, e) = (2.51_f32, 0.03_f32, 2.43_f32, 0.59_f32, 0.14_f32);
        let mapped = ((x * (a * x + b)) / (x * (cc * x + d) + e)).clamp(0.0, 1.0);
        let encoded = colour::linear_to_srgb(mapped);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        {
            out[i] = (encoded * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn resolve_once(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pipeline: &wgpu::RenderPipeline,
    cam_bg: &wgpu::BindGroup,
    post_bg: &wgpu::BindGroup,
    format: wgpu::TextureFormat,
    w: u32,
    h: u32,
) -> Vec<u8> {
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("resolve-target"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&wgpu::TextureViewDescriptor::default());
    let padded = (w * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: u64::from(padded * h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
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
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, cam_bg, &[]);
        rp.set_bind_group(1, post_bg, &[]);
        rp.draw(0..3, 0..1);
    }
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([enc.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::wait_indefinitely());
    let data = slice.get_mapped_range();
    let mut out = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        let src = (y * padded) as usize;
        let dst = (y * w * 4) as usize;
        out[dst..dst + (w * 4) as usize].copy_from_slice(&data[src..src + (w * 4) as usize]);
    }
    drop(data);
    readback.unmap();
    out
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(rgba)
        .expect("png data");
}
