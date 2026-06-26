//! M11.3 inc.2 (ADR-042) — image-based lighting (IBL) + the skybox source.
//!
//! A procedural HDR sky (equirectangular, with a full box-filtered mip chain) gives metals an environment
//! to REFLECT — closing the M11.2 "dark metal" (a metal has no diffuse, so with nothing to reflect it
//! renders near-black) — and also backs the viewport as a skybox. Specular IBL is the split-sum
//! approximation: sample the env mip whose blur matches `roughness`, then scale by a precomputed BRDF LUT.
//! Diffuse IBL reads the env's top (maximally blurred) mip as a cheap irradiance. All prep is CPU-side
//! (no GPU prefilter passes). The env is `rgba16float` because the device runs without `FLOAT32_FILTERABLE`
//! (f32 textures aren't filterable there), so a procedural sky is baked to halves and uploaded per mip.

use glam::Vec3;

const ENV_W: usize = 512;
const ENV_H: usize = 256;
const LUT_N: usize = 128;
const LUT_SAMPLES: u32 = 256;

/// GPU-side IBL resources: bound as group 3 on the mesh pipeline and sampled by the skybox. The shader
/// reads the env's mip count via `textureNumLevels`, so no max-mip needs to cross the boundary.
pub struct Ibl {
    pub bind_group: wgpu::BindGroup,
}

/// f32 → IEEE-754 binary16, returned as raw bits. Round-toward-zero (drop low mantissa) is plenty for an
/// env map; values that overflow half range (the sun) clamp to the largest finite half.
fn f16_bits(f: f32) -> u16 {
    let x = f.max(0.0).to_bits(); // env radiance is non-negative
    let exp = ((x >> 23) & 0xff) as i32 - 127 + 15;
    let mant = x & 0x7f_ffff;
    if exp <= 0 {
        0 // flush tiny/zero to +0
    } else if exp >= 0x1f {
        0x7bff // clamp to max finite half
    } else {
        ((exp as u16) << 10) | ((mant >> 13) as u16)
    }
}

/// World direction for equirect texel `(x, y)`. MUST stay the inverse of the shader's `dir_to_equirect`.
fn texel_dir(x: usize, y: usize) -> Vec3 {
    let u = (x as f32 + 0.5) / ENV_W as f32;
    let v = (y as f32 + 0.5) / ENV_H as f32;
    let phi = (u - 0.5) * std::f32::consts::TAU;
    let theta = v * std::f32::consts::PI;
    Vec3::new(
        theta.sin() * phi.cos(),
        theta.cos(),
        theta.sin() * phi.sin(),
    )
}

/// Procedural, physically-plausible sky radiance (HDR: the sun disk is ≫1, so polished metals get a sharp
/// specular glint). A cool zenith → warm horizon gradient over a dim ground, plus a warm sun in the
/// upper-right that doubles as the scene's implied key direction.
fn sky_radiance(d: Vec3) -> Vec3 {
    let t = d.y.clamp(-1.0, 1.0);
    let zenith = Vec3::new(0.18, 0.34, 0.62);
    let horizon = Vec3::new(0.70, 0.75, 0.82);
    let ground = Vec3::new(0.10, 0.09, 0.08);
    let base = if t >= 0.0 {
        horizon.lerp(zenith, t.powf(0.45))
    } else {
        horizon.lerp(ground, (-t).powf(0.5))
    };
    let sun_dir = Vec3::new(0.35, 0.55, 0.40).normalize();
    let s = d.dot(sun_dir).max(0.0);
    let sun = if s > 0.9992 { 22.0 } else { 0.0 } + 1.6 * s.powf(800.0);
    base + Vec3::new(1.0, 0.93, 0.78) * sun
}

/// The equirect sky + its box-mip chain, each level as packed `rgba16f` texels (row-major).
fn build_env_mips() -> Vec<(usize, usize, Vec<u16>)> {
    // Level 0 in f32 (kept for clean downsampling; converted to halves at upload).
    let mut level0 = vec![[0.0f32; 3]; ENV_W * ENV_H];
    for y in 0..ENV_H {
        for x in 0..ENV_W {
            let c = sky_radiance(texel_dir(x, y));
            level0[y * ENV_W + x] = [c.x, c.y, c.z];
        }
    }
    let mut f32_levels: Vec<(usize, usize, Vec<[f32; 3]>)> = vec![(ENV_W, ENV_H, level0)];
    loop {
        let (pw, ph) = {
            let last = f32_levels.last().unwrap();
            (last.0, last.1)
        };
        if pw == 1 && ph == 1 {
            break;
        }
        let nw = (pw / 2).max(1);
        let nh = (ph / 2).max(1);
        let mut next = vec![[0.0f32; 3]; nw * nh];
        {
            let prev = &f32_levels.last().unwrap().2;
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0.0f32; 3];
                    for dy in 0..2 {
                        for dx in 0..2 {
                            let sx = (x * 2 + dx).min(pw - 1);
                            let sy = (y * 2 + dy).min(ph - 1);
                            let p = prev[sy * pw + sx];
                            acc[0] += p[0];
                            acc[1] += p[1];
                            acc[2] += p[2];
                        }
                    }
                    next[y * nw + x] = [acc[0] / 4.0, acc[1] / 4.0, acc[2] / 4.0];
                }
            }
        }
        f32_levels.push((nw, nh, next));
    }
    // Pack each level to rgba16f (alpha = 1).
    f32_levels
        .into_iter()
        .map(|(w, h, texels)| {
            let mut packed = Vec::with_capacity(w * h * 4);
            for p in texels {
                packed.extend_from_slice(&[
                    f16_bits(p[0]),
                    f16_bits(p[1]),
                    f16_bits(p[2]),
                    f16_bits(1.0),
                ]);
            }
            (w, h, packed)
        })
        .collect()
}

// ── split-sum BRDF LUT (the environment-BRDF half of the split-sum approximation) ──────────────────

/// Van der Corput radical inverse (base 2) — the second Hammersley coordinate.
fn radical_inverse_vdc(mut bits: u32) -> f32 {
    bits = bits.rotate_left(16);
    bits = ((bits & 0x5555_5555) << 1) | ((bits & 0xAAAA_AAAA) >> 1);
    bits = ((bits & 0x3333_3333) << 2) | ((bits & 0xCCCC_CCCC) >> 2);
    bits = ((bits & 0x0F0F_0F0F) << 4) | ((bits & 0xF0F0_F0F0) >> 4);
    bits = ((bits & 0x00FF_00FF) << 8) | ((bits & 0xFF00_FF00) >> 8);
    (bits as f32) * 2.328_306_4e-10 // 1 / 2^32
}

/// GGX importance-sampled half-vector in tangent space (N = +Z).
fn importance_sample_ggx(xi: (f32, f32), rough: f32) -> Vec3 {
    let a = rough * rough;
    let phi = std::f32::consts::TAU * xi.0;
    let cos_t = ((1.0 - xi.1) / (1.0 + (a * a - 1.0) * xi.1)).sqrt();
    let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
    Vec3::new(phi.cos() * sin_t, phi.sin() * sin_t, cos_t)
}

/// Smith geometry for IBL (note the IBL `k = a²/2`, unlike direct lighting's `(a+1)²/8`).
fn geom_smith_ibl(n_dot_v: f32, n_dot_l: f32, rough: f32) -> f32 {
    let k = rough * rough / 2.0;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    gv * gl
}

/// Precompute the `(scale, bias)` BRDF response over (NdotV, roughness) → an `rg16f` LUT.
fn bake_brdf_lut() -> Vec<u16> {
    let mut out = Vec::with_capacity(LUT_N * LUT_N * 2);
    for j in 0..LUT_N {
        let rough = (j as f32 + 0.5) / LUT_N as f32;
        for i in 0..LUT_N {
            let n_dot_v = (i as f32 + 0.5) / LUT_N as f32;
            let v = Vec3::new((1.0 - n_dot_v * n_dot_v).max(0.0).sqrt(), 0.0, n_dot_v);
            let (mut a, mut b) = (0.0f32, 0.0f32);
            for s in 0..LUT_SAMPLES {
                let xi = (s as f32 / LUT_SAMPLES as f32, radical_inverse_vdc(s));
                let h = importance_sample_ggx(xi, rough);
                let l = 2.0 * v.dot(h) * h - v;
                let n_dot_l = l.z.max(0.0);
                let n_dot_h = h.z.max(0.0);
                let v_dot_h = v.dot(h).max(0.0);
                if n_dot_l > 0.0 {
                    let g = geom_smith_ibl(n_dot_v, n_dot_l, rough);
                    let g_vis = g * v_dot_h / (n_dot_h * n_dot_v).max(1e-6);
                    let fc = (1.0 - v_dot_h).powi(5);
                    a += (1.0 - fc) * g_vis;
                    b += fc * g_vis;
                }
            }
            out.push(f16_bits(a / LUT_SAMPLES as f32));
            out.push(f16_bits(b / LUT_SAMPLES as f32));
        }
    }
    out
}

// ── GPU resources ──────────────────────────────────────────────────────────────────────────────────

/// group 3 layout: env texture + its sampler, BRDF LUT + its sampler (all FRAGMENT-visible).
pub fn bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let samp = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("ibl-bgl"),
        entries: &[tex(0), samp(1), tex(2), samp(3)],
    })
}

/// Bake the procedural env + BRDF LUT and build the group-3 bind group.
pub fn create(device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout) -> Ibl {
    let mips = build_env_mips();
    let mip_count = mips.len() as u32;
    let env = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("env-equirect"),
        size: wgpu::Extent3d {
            width: ENV_W as u32,
            height: ENV_H as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: mip_count,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (w, h, data)) in mips.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &env,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(data),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((*w as u32) * 8), // rgba16 = 8 bytes/texel
                rows_per_image: Some(*h as u32),
            },
            wgpu::Extent3d {
                width: *w as u32,
                height: *h as u32,
                depth_or_array_layers: 1,
            },
        );
    }
    let env_view = env.create_view(&wgpu::TextureViewDescriptor::default());

    let lut_data = bake_brdf_lut();
    let lut = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("brdf-lut"),
        size: wgpu::Extent3d {
            width: LUT_N as u32,
            height: LUT_N as u32,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rg16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &lut,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        bytemuck::cast_slice(&lut_data),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(LUT_N as u32 * 4), // rg16 = 4 bytes/texel
            rows_per_image: Some(LUT_N as u32),
        },
        wgpu::Extent3d {
            width: LUT_N as u32,
            height: LUT_N as u32,
            depth_or_array_layers: 1,
        },
    );
    let lut_view = lut.create_view(&wgpu::TextureViewDescriptor::default());

    // env: wrap horizontally (azimuth seam), clamp vertically (poles); trilinear for the roughness mips.
    let env_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("env-sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        ..Default::default()
    });
    let lut_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("lut-sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });

    let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("ibl-bg"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&env_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&env_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&lut_view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&lut_sampler),
            },
        ],
    });

    Ibl { bind_group }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f16_round_trips_representative_values() {
        // 1.0 → 0x3C00, 0.0 → 0, 0.5 → 0x3800; large clamps to max finite half.
        assert_eq!(f16_bits(0.0), 0);
        assert_eq!(f16_bits(1.0), 0x3C00);
        assert_eq!(f16_bits(0.5), 0x3800);
        assert_eq!(f16_bits(1.0e9), 0x7bff); // the sun clamps, not inf/nan
    }

    #[test]
    fn env_mip_chain_descends_to_1x1() {
        let mips = build_env_mips();
        assert_eq!(mips[0].0, ENV_W);
        assert_eq!(mips[0].1, ENV_H);
        let last = mips.last().unwrap();
        assert_eq!((last.0, last.1), (1, 1), "mip chain bottoms out at 1x1");
        // 512 → 1 is 10 levels (inclusive).
        assert_eq!(mips.len(), 10);
        // Each level carries rgba16 (4 halves) per texel.
        for (w, h, data) in &mips {
            assert_eq!(data.len(), w * h * 4);
        }
    }

    #[test]
    fn brdf_lut_is_bounded_unit_response() {
        let lut = bake_brdf_lut();
        assert_eq!(lut.len(), LUT_N * LUT_N * 2);
        // The split-sum scale/bias are in [0,1]; as halves that's ≤ 0x3C00 (=1.0).
        assert!(
            lut.iter().all(|&h| h <= 0x3C00),
            "BRDF terms stay within [0,1]"
        );
    }

    #[test]
    fn sun_is_hdr_and_ground_is_dim() {
        let sun_dir = Vec3::new(0.35, 0.55, 0.40).normalize();
        assert!(sky_radiance(sun_dir).x > 5.0, "the sun is HDR (≫1)");
        assert!(
            sky_radiance(Vec3::NEG_Y).length() < 0.4,
            "the ground is dim"
        );
    }
}
