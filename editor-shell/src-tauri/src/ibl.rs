//! M11.3 inc.2 (ADR-042) — image-based lighting (IBL) + the skybox source.
//!
//! An HDR environment gives metals something to REFLECT — closing the M11.2 "dark metal" (a metal has no
//! diffuse, so with nothing to reflect it renders near-black) — and also backs the viewport as a skybox.
//! Either half of it can come from a user-supplied panorama or from the built-in procedural studio.
//!
//! The environment is split into the two halves the rendering equation actually asks for:
//!
//! * **Specular** — an equirect mip chain where level `i` is the environment convolved with the GGX lobe
//!   for roughness `i / last`, by importance sampling ([`prefilter_level`]). This is the half of the
//!   split-sum approximation that lives in a texture; the other half is the precomputed BRDF LUT.
//! * **Diffuse** — nine cosine-convolved spherical-harmonic coefficients
//!   ([`sh_irradiance_coefficients`]), which reproduce a cosine lobe to about 1%.
//!
//! Both replaced a single box-filtered mip chain that was asked to serve as both. A box filter is not the
//! GGX lobe at any roughness and not a cosine lobe either, so every glossy surface reflected a blur that
//! matched no material and every diffuse surface received ambient light with almost no direction to it.
//!
//! All prep is CPU-side (no GPU prefilter passes). The env is `rgba16float` because the device runs
//! without `FLOAT32_FILTERABLE` (f32 textures aren't filterable there), so the panorama is baked to halves
//! and uploaded per mip.

use glam::Vec3;
use wgpu::util::DeviceExt as _;

const ENV_W: usize = 1024;
const ENV_H: usize = 512;
const LUT_N: usize = 128;
const LUT_SAMPLES: u32 = 256;

/// GPU-side IBL resources: bound as group 3 on the mesh pipeline and sampled by the skybox. The shader
/// reads the env's mip count via `textureNumLevels`, so no max-mip needs to cross the boundary.
pub struct Ibl {
    pub bind_group: wgpu::BindGroup,
}

/// An environment panorama supplied at RUNTIME.
///
/// The whole IBL chain — equirect upload, box-filtered mip chain, diffuse irradiance from the top mip,
/// roughness-indexed specular — already existed and already called the HDR decoder. It was reachable
/// only through the `MTK_ENV_HDR` environment variable, read once at process start: a complete
/// capability that no user could invoke, which is the same failure the STEP writer had. This type is
/// what lets a command hand the renderer a new sky while the app is running.
#[derive(Clone, Debug)]
pub struct EnvSource {
    /// Equirectangular width in texels.
    pub width: usize,
    /// Equirectangular height in texels.
    pub height: usize,
    /// Linear radiance per texel, row-major. HDR: values exceed 1.0.
    pub pixels: Vec<[f32; 3]>,
    /// What to call it in the UI — usually the file stem.
    pub label: String,
}

impl EnvSource {
    /// Reject a panorama that is not usable as an equirect before it reaches the GPU.
    ///
    /// # Errors
    /// A sentence naming what is wrong, suitable for showing the user directly.
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0 || self.height == 0 {
            return Err("that image has no pixels".into());
        }
        if self.pixels.len() != self.width * self.height {
            return Err(format!(
                "that image is {}x{} but carries {} pixels — the file is damaged",
                self.width,
                self.height,
                self.pixels.len()
            ));
        }
        // An equirectangular panorama is 2:1 by definition. Anything else will map to the sphere
        // wrongly, so it is reported rather than stretched into place silently.
        let ratio = self.width as f64 / self.height as f64;
        if (ratio - 2.0).abs() > 0.02 {
            return Err(format!(
                "an environment map has to be equirectangular (twice as wide as it is tall); this one \
                 is {}x{}, a ratio of {ratio:.2}",
                self.width, self.height
            ));
        }
        if self.pixels.iter().any(|p| !p.iter().all(|c| c.is_finite())) {
            return Err(
                "that image contains non-finite radiance values — the file is damaged".into(),
            );
        }
        Ok(())
    }
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

/// The procedural environment radiance (HDR), dispatched by `MTK_ENV` (default `studio`; `sky` = the outdoor
/// look). `studio` is a product/CAD-viewer environment; `sky` is the original outdoor daylight.
fn sky_radiance(d: Vec3, studio: bool) -> Vec3 {
    if studio {
        studio_radiance(d)
    } else {
        outdoor_radiance(d)
    }
}

/// A rectangular soft-box as the environment sees it: a panel of constant HDR radiance with a feathered
/// edge, placed by its centre direction and oriented by an up vector.
///
/// Rectangular rather than a cosine lobe on purpose. A `pow(dot(d, dir), n)` lobe is a round blob, and a
/// round blob is the thing that makes CG metal look like CG. Real studio lighting shapes its sources, and
/// the straight-edged streak a rectangular soft-box leaves along a machined curve is most of what makes a
/// render read as photographed hardware.
#[derive(Clone, Copy)]
struct SoftBox {
    /// Centre direction of the panel (unit).
    dir: Vec3,
    /// Which way is "up" across the panel — this is what makes it rectangular rather than square-agnostic.
    up: Vec3,
    /// Half angular width / height in radians.
    half_u: f32,
    half_v: f32,
    /// Angular width of the feathered edge, in radians.
    feather: f32,
    /// Radiance inside the panel. HDR by construction: these are the values a reflective surface reflects.
    radiance: Vec3,
}

/// Smoothstep, so a panel edge is feathered rather than a hard cut that would alias in the prefilter.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge1 <= edge0 {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl SoftBox {
    /// The radiance this panel contributes along `d`.
    fn radiance_towards(self, d: Vec3) -> Vec3 {
        let cos = d.dot(self.dir);
        if cos <= 1.0e-3 {
            return Vec3::ZERO;
        }
        let projected = self.up - self.dir * self.dir.dot(self.up);
        if projected.length_squared() < 1.0e-6 {
            return Vec3::ZERO;
        }
        let v_axis = projected.normalize();
        let u_axis = self.dir.cross(v_axis);
        // Angular offsets from the panel centre, along the panel's own axes.
        let au = d.dot(u_axis).atan2(cos).abs();
        let av = d.dot(v_axis).atan2(cos).abs();
        let inside = (1.0 - smoothstep(self.half_u, self.half_u + self.feather, au))
            * (1.0 - smoothstep(self.half_v, self.half_v + self.feather, av));
        self.radiance * inside
    }
}

/// The soft-boxes of the neutral studio: a shaped key, a broad fill, a rim, and two ceiling luminaires.
///
/// The angular sizes are small and the radiances large on purpose. What makes a metal read as metal is the
/// RATIO between the brightest thing it can reflect and the room around it; spreading the same energy over
/// a big dim panel produces exactly the flat, grey, plastic look this replaced. See
/// [`STUDIO_MEAN_RADIANCE`] for how the total is held steady while the contrast goes up.
fn studio_panels() -> [SoftBox; 5] {
    [
        // Key: the dominant shaping light, high on the front left.
        SoftBox {
            dir: Vec3::new(-0.45, 0.72, 0.52).normalize(),
            up: Vec3::Y,
            half_u: 0.085,
            half_v: 0.055,
            feather: 0.030,
            radiance: Vec3::splat(170.0),
        },
        // Fill: broad and dim, opposite side. Opens the shadows without flattening the form.
        SoftBox {
            dir: Vec3::new(0.62, 0.42, -0.30).normalize(),
            up: Vec3::Y,
            half_u: 0.200,
            half_v: 0.150,
            feather: 0.090,
            radiance: Vec3::splat(9.0),
        },
        // Rim: a tall narrow strip behind the subject. This is what separates machinery from its
        // background instead of letting a dark part sink into a dark room.
        SoftBox {
            dir: Vec3::new(0.18, 0.36, -0.92).normalize(),
            up: Vec3::Y,
            half_u: 0.045,
            half_v: 0.200,
            feather: 0.030,
            radiance: Vec3::splat(150.0),
        },
        // Two long ceiling luminaires. A linear source draws a straight streak down a cylinder, which is
        // the single most recognisable "this is a real workshop" cue on turned and extruded parts.
        SoftBox {
            dir: Vec3::new(-0.10, 0.97, 0.20).normalize(),
            up: Vec3::Z,
            half_u: 0.500,
            half_v: 0.020,
            feather: 0.018,
            radiance: Vec3::splat(45.0),
        },
        SoftBox {
            dir: Vec3::new(0.28, 0.94, -0.20).normalize(),
            up: Vec3::Z,
            half_u: 0.460,
            half_v: 0.018,
            feather: 0.018,
            radiance: Vec3::splat(34.0),
        },
    ]
}

/// The solid-angle-weighted mean radiance the studio is normalised to.
///
/// This is the measured mean of the ORIGINAL studio environment. Holding it fixed is what lets the
/// dynamic range be raised by more than an order of magnitude without re-calibrating every scene's
/// exposure: rough and diffuse surfaces, which integrate the whole environment, receive the same total
/// energy as before, while polished surfaces — which reflect a narrow cone — finally have something
/// bright to find.
const STUDIO_MEAN_RADIANCE: f32 = 0.5111;

/// **Neutral studio** radiance (the `MTK_ENV=studio` default) — a product/CAD-viewer environment (the look
/// KeyShot / Fusion / Onshape default to), so imported machined parts read as studio-lit metal, not
/// outdoor-tinted plastic. A dim neutral room (bright ceiling → grey walls → dark floor) carrying five
/// SHAPED, genuinely HDR sources. Neutral by construction — a polished part reflects grey, not blue sky.
///
/// Un-normalised; [`procedural_level0`] scales the baked panorama to [`STUDIO_MEAN_RADIANCE`].
fn studio_radiance(d: Vec3) -> Vec3 {
    let t = d.y.clamp(-1.0, 1.0);
    // The room is a backdrop, not the light source — but it still has to BE a room. Normalising to a
    // fixed mean means every unit of radiance the panels take is a unit the room gives up, and pushing
    // this gradient too far down crushes every downward-facing surface in the scene to black. Since the
    // panels dominate the mean anyway, keeping a genuine floor bounce costs almost none of the dynamic
    // range that makes metal read as metal.
    let ceiling = Vec3::new(0.60, 0.61, 0.63);
    let walls = Vec3::new(0.34, 0.35, 0.36);
    let floor = Vec3::new(0.13, 0.13, 0.14);
    let mut lit = if t >= 0.0 {
        walls.lerp(ceiling, t.powf(0.65))
    } else {
        walls.lerp(floor, (-t).powf(0.55))
    };
    for panel in studio_panels() {
        lit += panel.radiance_towards(d);
    }
    lit
}

/// **Outdoor daylight** radiance (`MTK_ENV=sky`) — a cool zenith → warm horizon gradient over a dim ground,
/// plus a warm sun in the upper-right (HDR, so polished metals get a sharp glint) that doubles as the scene's
/// implied key direction. The original environment before the studio default.
fn outdoor_radiance(d: Vec3) -> Vec3 {
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

/// The procedural sky as a level-0 equirect (`ENV_W × ENV_H` linear-RGB texels). `MTK_ENV` (read once here)
/// picks `studio` (default) vs the outdoor `sky`.
fn procedural_level0() -> (usize, usize, Vec<[f32; 3]>) {
    let studio = std::env::var("MTK_ENV").map_or(true, |v| !v.eq_ignore_ascii_case("sky"));
    let mut level0 = vec![[0.0f32; 3]; ENV_W * ENV_H];
    for y in 0..ENV_H {
        for x in 0..ENV_W {
            let c = sky_radiance(texel_dir(x, y), studio);
            level0[y * ENV_W + x] = [c.x, c.y, c.z];
        }
    }
    // The studio is authored for CONTRAST and then normalised for BRIGHTNESS, so the two can be tuned
    // independently. The outdoor sky is left alone: its sun/sky ratio is already its calibration.
    if studio {
        scale_to_mean_radiance(ENV_W, ENV_H, &mut level0, STUDIO_MEAN_RADIANCE);
    }
    (ENV_W, ENV_H, level0)
}

/// The solid-angle-weighted mean luminance of an equirectangular panorama.
///
/// Weighted by `sin(theta)`, because equirect rows near the poles cover far less of the sphere than rows
/// near the equator. An unweighted mean would count a handful of pole texels as heavily as the horizon and
/// mis-calibrate every environment that is not uniform.
fn mean_radiance(width: usize, height: usize, pixels: &[[f32; 3]]) -> f32 {
    let mut weighted = 0.0f64;
    let mut total_weight = 0.0f64;
    for y in 0..height {
        let theta = (y as f32 + 0.5) / height as f32 * std::f32::consts::PI;
        let weight = f64::from(theta.sin());
        let mut row = 0.0f64;
        for x in 0..width {
            let p = pixels[y * width + x];
            row += f64::from(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
        }
        weighted += row * weight;
        total_weight += weight * width as f64;
    }
    if total_weight <= 0.0 {
        return 0.0;
    }
    (weighted / total_weight) as f32
}

/// Scale a panorama so its mean radiance is exactly `target`, leaving its dynamic range untouched.
fn scale_to_mean_radiance(width: usize, height: usize, pixels: &mut [[f32; 3]], target: f32) {
    let mean = mean_radiance(width, height, pixels);
    if !mean.is_finite() || mean <= 1.0e-6 {
        return;
    }
    let scale = target / mean;
    for texel in pixels.iter_mut() {
        texel[0] *= scale;
        texel[1] *= scale;
        texel[2] *= scale;
    }
}

/// Level 0 for the env: a real `.hdr` panorama if `MTK_ENV_HDR` points to a readable Radiance file
/// (M11.3 inc.2 — the image-crate HDR path; the bytes could equally come from a store handle), else the
/// procedural sky. A load failure logs and falls back, so a bad path never breaks the viewport.
fn env_level0() -> (usize, usize, Vec<[f32; 3]>) {
    let Ok(path) = std::env::var("MTK_ENV_HDR") else {
        return procedural_level0();
    };
    match std::fs::read(&path)
        .map_err(|e| e.to_string())
        .and_then(|b| {
            metrocalk_assets::env_import::load_hdr_equirect(&b).map_err(|e| e.to_string())
        }) {
        Ok(env) => {
            eprintln!(
                "[ibl] loaded HDR env '{path}' ({}x{})",
                env.width, env.height
            );
            (env.width as usize, env.height as usize, env.pixels)
        }
        Err(e) => {
            eprintln!("[ibl] HDR env '{path}' failed ({e}); using procedural sky");
            procedural_level0()
        }
    }
}

/// Direction of an equirect texel at an arbitrary panorama size.
fn texel_dir_at(x: usize, y: usize, width: usize, height: usize) -> Vec3 {
    let u = (x as f32 + 0.5) / width as f32;
    let v = (y as f32 + 0.5) / height as f32;
    let phi = (u - 0.5) * std::f32::consts::TAU;
    let theta = v * std::f32::consts::PI;
    Vec3::new(
        theta.sin() * phi.cos(),
        theta.cos(),
        theta.sin() * phi.sin(),
    )
}

/// One equirect level as f32 radiance.
type Level = (usize, usize, Vec<[f32; 3]>);

/// Box-filtered pyramid of the source, used ONLY as the sampling pyramid for prefiltering.
///
/// Importance sampling takes a few hundred samples of a panorama that may hold a source hundreds of times
/// brighter than the room around it. Reading those from the sharp level 0 makes "did this sample happen to
/// land on the key light" a coin toss, and the prefiltered result fills with fireflies. Reading from a
/// level whose texels cover roughly the sample's own solid angle is the standard remedy.
fn box_pyramid(level0: Level) -> Vec<Level> {
    let mut levels = vec![level0];
    loop {
        let (pw, ph) = {
            let last = levels.last().expect("at least one level");
            (last.0, last.1)
        };
        if pw == 1 && ph == 1 {
            break;
        }
        let nw = (pw / 2).max(1);
        let nh = (ph / 2).max(1);
        let mut next = vec![[0.0f32; 3]; nw * nh];
        {
            let prev = &levels.last().expect("at least one level").2;
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
        levels.push((nw, nh, next));
    }
    levels
}

/// Bilinear equirect fetch from one pyramid level. Wraps in longitude, clamps in latitude.
fn sample_equirect(level: &Level, dir: Vec3) -> Vec3 {
    let (w, h, texels) = level;
    let d = dir.normalize_or_zero();
    let u = d.z.atan2(d.x) / std::f32::consts::TAU + 0.5;
    let v = d.y.clamp(-1.0, 1.0).acos() / std::f32::consts::PI;
    let fx = u * *w as f32 - 0.5;
    let fy = v * *h as f32 - 0.5;
    let x0 = fx.floor();
    let y0 = fy.floor();
    let tx = fx - x0;
    let ty = fy - y0;
    let width = *w as i64;
    let height = *h as i64;
    let wrap_x = |x: i64| -> usize { x.rem_euclid(width) as usize };
    let clamp_y = |y: i64| -> usize { y.clamp(0, height - 1) as usize };
    let (x0i, x1i) = (wrap_x(x0 as i64), wrap_x(x0 as i64 + 1));
    let (y0i, y1i) = (clamp_y(y0 as i64), clamp_y(y0 as i64 + 1));
    let fetch = |x: usize, y: usize| -> Vec3 {
        let p = texels[y * *w + x];
        Vec3::new(p[0], p[1], p[2])
    };
    let top = fetch(x0i, y0i).lerp(fetch(x1i, y0i), tx);
    let bottom = fetch(x0i, y1i).lerp(fetch(x1i, y1i), tx);
    top.lerp(bottom, ty)
}

/// GGX normal distribution (CPU side), for the importance-sampling pdf.
fn distribution_ggx(n_dot_h: f32, rough: f32) -> f32 {
    let a = rough * rough;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    a2 / (std::f32::consts::PI * d * d).max(1.0e-7)
}

/// The split-sum prefilter for one roughness level (Karis, *Real Shading in Unreal Engine 4*, 2013).
///
/// For each output texel the normal, the view and the reflection vector are all taken to be that texel's
/// own direction — the standard split-sum simplification, and what makes a single 2D environment usable
/// from every view direction. Half-vectors are drawn from the GGX distribution, weighted by `n dot l`, and
/// NORMALISED by the accumulated weight, so a constant environment prefilters to itself and the chain
/// conserves energy.
fn prefilter_level(
    pyramid: &[Level],
    width: usize,
    height: usize,
    roughness: f32,
    samples: u32,
) -> Vec<[f32; 3]> {
    let (src_w, src_h, _) = pyramid[0];
    // Mean solid angle of one level-0 texel, for choosing which pyramid level a sample reads from.
    let sa_texel = 4.0 * std::f32::consts::PI / (src_w * src_h) as f32;
    let top_level = pyramid.len() - 1;

    let mut out = vec![[0.0f32; 3]; width * height];
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let rows_per_chunk = height.div_ceil(threads.max(1)).max(1);
    std::thread::scope(|scope| {
        for (chunk_index, chunk) in out.chunks_mut(rows_per_chunk * width).enumerate() {
            scope.spawn(move || {
                let y_start = chunk_index * rows_per_chunk;
                for (row, texels) in chunk.chunks_mut(width).enumerate() {
                    let y = y_start + row;
                    for (x, texel) in texels.iter_mut().enumerate() {
                        let n = texel_dir_at(x, y, width, height);
                        let v = n;
                        // A tangent frame around n; the choice of up only rotates the sample pattern.
                        let up = if n.y.abs() < 0.999 { Vec3::Y } else { Vec3::X };
                        let tangent = up.cross(n).normalize_or_zero();
                        let bitangent = n.cross(tangent);

                        let mut sum = Vec3::ZERO;
                        let mut weight = 0.0f32;
                        for i in 0..samples {
                            let xi = (i as f32 / samples as f32, radical_inverse_vdc(i));
                            let ht = importance_sample_ggx(xi, roughness);
                            let h =
                                (tangent * ht.x + bitangent * ht.y + n * ht.z).normalize_or_zero();
                            let l = h * (2.0 * v.dot(h)) - v;
                            let n_dot_l = n.dot(l);
                            if n_dot_l <= 0.0 {
                                continue;
                            }
                            let n_dot_h = n.dot(h).max(0.0);
                            let v_dot_h = v.dot(h).max(1.0e-4);
                            // The solid angle this sample stands for, against one source texel.
                            let pdf = distribution_ggx(n_dot_h, roughness) * n_dot_h
                                / (4.0 * v_dot_h)
                                + 1.0e-4;
                            let sa_sample = 1.0 / (samples as f32 * pdf);
                            let mip = if roughness <= 0.0 {
                                0.0
                            } else {
                                0.5 * (sa_sample / sa_texel).max(1.0e-8).log2()
                            };
                            let level = (mip.round().max(0.0) as usize).min(top_level);
                            sum += sample_equirect(&pyramid[level], l) * n_dot_l;
                            weight += n_dot_l;
                        }
                        let c = if weight > 0.0 {
                            sum / weight
                        } else {
                            sample_equirect(&pyramid[0], n)
                        };
                        *texel = [c.x, c.y, c.z];
                    }
                }
            });
        }
    });
    out
}

/// The equirect env + its GGX-PREFILTERED mip chain, each level as packed `rgba16f` texels (row-major).
///
/// Level 0 is the sharp environment (the skybox, and what a mirror reflects). Level `i` is the environment
/// convolved with the GGX lobe for roughness `i / last`, which is exactly what the split-sum approximation
/// in the shader assumes it is sampling.
///
/// This replaced a plain 2x2 box chain. A box filter blurs in TEXEL space — anisotropically on an equirect,
/// worst at the poles — and its kernel is not the GGX lobe at any roughness, so every glossy surface in the
/// scene reflected a blur that matched no material. Rough metal came out dull and shapeless: the most
/// visible way a physically based renderer can look wrong while each individual term in it is right.
fn build_env_mips_from(level0: Level) -> Vec<(usize, usize, Vec<u16>)> {
    let pyramid = box_pyramid(level0);
    let last = pyramid.len() - 1;
    let mut levels: Vec<Level> = Vec::with_capacity(pyramid.len());
    for (index, (w, h, _)) in pyramid.iter().enumerate() {
        if index == 0 || last == 0 {
            levels.push(pyramid[index].clone());
            continue;
        }
        let roughness = index as f32 / last as f32;
        // Sample count tracks the LOBE, not the texture. A near-mirror level has a very tight GGX lobe
        // that converges in a few dozen samples, and it is also the largest texture in the chain — so
        // sampling it hardest is exactly backwards. Scaling with roughness cuts the dominant level's cost
        // several-fold at no visible quality difference. The budget term then bounds very large
        // user-supplied panoramas without starving the small, cheap levels.
        let texels = w * h;
        let lobe_samples = (32.0 + roughness * 96.0) as usize;
        let budget_samples = (20_000_000usize / texels.max(1)).max(24);
        let samples = lobe_samples.min(budget_samples).clamp(24, 128) as u32;
        levels.push((
            *w,
            *h,
            prefilter_level(&pyramid, *w, *h, roughness, samples),
        ));
    }

    levels
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

// ── diffuse irradiance as spherical harmonics ─────────────────────────────────────────────────────────

/// Number of L2 spherical-harmonic coefficients.
const SH_COEFFS: usize = 9;

/// The nine real L2 SH basis functions evaluated along `d`.
fn sh_basis(d: Vec3) -> [f32; SH_COEFFS] {
    let (x, y, z) = (d.x, d.y, d.z);
    [
        0.282_095,
        0.488_603 * y,
        0.488_603 * z,
        0.488_603 * x,
        1.092_548 * x * y,
        1.092_548 * y * z,
        0.315_392 * (3.0 * z * z - 1.0),
        1.092_548 * x * z,
        0.546_274 * (x * x - y * y),
    ]
}

/// Project an equirectangular panorama onto cosine-convolved SH, ready for the shader to evaluate.
///
/// Ramamoorthi & Hanrahan, *An Efficient Representation for Irradiance Environment Maps* (SIGGRAPH 2001).
/// Diffuse shading integrates the environment against a COSINE lobe, and a cosine lobe is so smooth that
/// nine coefficients reproduce it to within about 1% — which is why this is the standard representation
/// rather than a texture fetch.
///
/// What it replaced: the shader read diffuse ambient from a high mip of the specular chain, i.e. the
/// environment convolved with a GGX lobe at roughness 0.8, from a 4x2 texture. That is the wrong kernel at
/// far too low a resolution, and it is why ambient light had almost no sense of direction — every surface
/// facing away from the key received nearly the same grey, so unlit sides of machinery went flat and
/// shapeless. Softening a bad kernel further is not the same as convolving with the right one.
///
/// The convolution and the `1/PI` of the Lambert BRDF are folded into the coefficients, so what the shader
/// evaluates is directly the average incident radiance for a normal — the same quantity the old texture
/// fetch stood in for, and therefore a drop-in for it.
fn sh_irradiance_coefficients(
    width: usize,
    height: usize,
    pixels: &[[f32; 3]],
) -> [[f32; 4]; SH_COEFFS] {
    // Cosine-lobe convolution factors per band (Ramamoorthi's A-hat).
    const A_HAT: [f32; 3] = [
        std::f32::consts::PI,
        2.094_395, // 2*PI/3
        std::f32::consts::FRAC_PI_4,
    ];
    const BAND_OF: [usize; SH_COEFFS] = [0, 1, 1, 1, 2, 2, 2, 2, 2];

    let mut acc = [[0.0f64; 3]; SH_COEFFS];
    // Solid angle of one texel: (2*PI/W) * (PI/H) * sin(theta).
    let d_phi_d_theta = f64::from(std::f32::consts::TAU / width as f32)
        * f64::from(std::f32::consts::PI / height as f32);
    for y in 0..height {
        let theta = (y as f32 + 0.5) / height as f32 * std::f32::consts::PI;
        let solid_angle = d_phi_d_theta * f64::from(theta.sin());
        for x in 0..width {
            let basis = sh_basis(texel_dir_at(x, y, width, height));
            let p = pixels[y * width + x];
            for i in 0..SH_COEFFS {
                let w = f64::from(basis[i]) * solid_angle;
                acc[i][0] += f64::from(p[0]) * w;
                acc[i][1] += f64::from(p[1]) * w;
                acc[i][2] += f64::from(p[2]) * w;
            }
        }
    }

    let mut out = [[0.0f32; 4]; SH_COEFFS];
    for i in 0..SH_COEFFS {
        let scale = f64::from(A_HAT[BAND_OF[i]] / std::f32::consts::PI);
        for c in 0..3 {
            let v = acc[i][c] * scale;
            out[i][c] = if v.is_finite() { v as f32 } else { 0.0 };
        }
        out[i][3] = 0.0;
    }
    out
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

/// group 3 layout: env texture + its sampler, BRDF LUT + its sampler, and (M11.3 inc.3) the shadow map +
/// its comparison sampler — all FRAGMENT-visible. The shadow lives here, not its own group, because the
/// device caps bind groups at 4 (web-portable).
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
        entries: &[
            tex(0),
            samp(1),
            tex(2),
            samp(3),
            // M11.3 inc.3 — shadow map (depth) + comparison sampler.
            wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 5,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                count: None,
            },
            // Cosine-convolved SH irradiance: nine RGB coefficients, the diffuse half of the environment.
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Bake the procedural env + BRDF LUT and build the group-3 bind group (which also carries the M11.3 inc.3
/// shadow map + comparison sampler, supplied by the caller — they share group 3 to stay within the device's
/// 4-bind-group cap).
pub fn create(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    shadow_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
) -> Ibl {
    create_with(device, queue, layout, shadow_view, shadow_sampler, None)
}

/// Build the IBL resources from an explicit panorama, or from the startup default when `env` is
/// `None`. Rebuilding is a whole-texture replacement: the mip chain is box-filtered on the CPU, so
/// there is no GPU prefilter pass to invalidate, and the new bind group simply supersedes the old.
pub fn create_with(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    layout: &wgpu::BindGroupLayout,
    shadow_view: &wgpu::TextureView,
    shadow_sampler: &wgpu::Sampler,
    env: Option<&EnvSource>,
) -> Ibl {
    let level0 = match env {
        Some(e) => (e.width, e.height, e.pixels.clone()),
        // No runtime environment supplied yet, so this is the startup default -- and the startup
        // default has always honoured `MTK_ENV_HDR`. Adding the runtime `EnvSource` had quietly
        // narrowed this arm to the procedural sky, which left the documented environment variable
        // doing nothing at all (and `env_level0` dead). A capability that stops working without
        // saying so is the failure this codebase keeps naming; it stays wired.
        None => env_level0(),
    };
    let sh = sh_irradiance_coefficients(level0.0, level0.1, &level0.2);
    let mips = build_env_mips_from(level0);
    let mip_count = mips.len() as u32;
    let (base_w, base_h, _) = &mips[0];
    let env = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("env-equirect"),
        size: wgpu::Extent3d {
            width: *base_w as u32,
            height: *base_h as u32,
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

    let sh_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("ibl-sh-irradiance"),
        contents: bytemuck::cast_slice(&sh),
        usage: wgpu::BufferUsages::UNIFORM,
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
            // M11.3 inc.3 — the shadow map + comparison sampler (owned by render.rs; shared in group 3).
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::TextureView(shadow_view),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::Sampler(shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: sh_buffer.as_entire_binding(),
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
        // The procedural chain by name: this asserts `ENV_W`/`ENV_H`, so it must not be at the mercy
        // of whatever `MTK_ENV_HDR` happens to point at on the machine running the tests.
        let mips = build_env_mips_from(procedural_level0());
        assert_eq!(mips[0].0, ENV_W);
        assert_eq!(mips[0].1, ENV_H);
        let last = mips.last().unwrap();
        assert_eq!((last.0, last.1), (1, 1), "mip chain bottoms out at 1x1");
        // 1024 → 1 is 11 levels (inclusive).
        assert_eq!(mips.len(), 11);
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

    /// Decode one packed rgba16f texel's red channel back to f32.
    fn decode_red(packed: &[u16], index: usize) -> f32 {
        let bits = packed[index * 4];
        let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
        let exp = ((bits >> 10) & 0x1f) as i32;
        let mant = (bits & 0x3ff) as f32;
        if exp == 0 {
            sign * (mant / 1024.0) * 2f32.powi(-14)
        } else {
            sign * (1.0 + mant / 1024.0) * 2f32.powi(exp - 15)
        }
    }

    #[test]
    fn the_studio_has_the_dynamic_range_a_reflective_surface_needs() {
        // The defect this pins: the original studio peaked at 7.3x its own mean. A polished part had
        // nothing bright to reflect, so every metal in an imported assembly read as flat grey plastic no
        // matter how correct the BRDF was. Studio lighting is a RATIO, and the ratio was the bug.
        let mut peak = 0.0f32;
        let mut pixels = Vec::with_capacity(ENV_W * ENV_H);
        for y in 0..ENV_H {
            for x in 0..ENV_W {
                let c = studio_radiance(texel_dir(x, y));
                pixels.push([c.x, c.y, c.z]);
            }
        }
        scale_to_mean_radiance(ENV_W, ENV_H, &mut pixels, STUDIO_MEAN_RADIANCE);
        for p in &pixels {
            peak = peak.max(0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]);
        }
        let mean = mean_radiance(ENV_W, ENV_H, &pixels);
        assert!(
            (mean - STUDIO_MEAN_RADIANCE).abs() < 1.0e-3,
            "the studio must keep its calibrated mean so exposure does not move, got {mean}"
        );
        assert!(
            peak / mean > 80.0,
            "a studio needs a source far brighter than its room; peak/mean was {:.1}:1",
            peak / mean
        );
    }

    #[test]
    fn prefiltering_spreads_a_bright_source_further_at_every_rougher_level() {
        // A single hot texel in an otherwise black panorama. Each rougher level must spread its energy
        // wider — that IS the GGX convolution — while none of them may invent or lose energy.
        let (w, h) = (64usize, 32usize);
        let mut pixels = vec![[0.0f32; 3]; w * h];
        pixels[(h / 2) * w + w / 2] = [400.0, 400.0, 400.0];
        let mips = build_env_mips_from((w, h, pixels));

        // "Spread" measured as the fraction of texels carrying a meaningful share of the level's peak.
        let spread = |level: &(usize, usize, Vec<u16>)| -> f32 {
            let count = level.0 * level.1;
            let mut peak = 0.0f32;
            for i in 0..count {
                peak = peak.max(decode_red(&level.2, i));
            }
            let lit = (0..count)
                .filter(|i| decode_red(&level.2, *i) > peak * 0.1)
                .count();
            lit as f32 / count as f32
        };
        let sharp = spread(&mips[1]);
        let rough = spread(&mips[3]);
        assert!(
            rough > sharp,
            "roughness must widen the reflected lobe: level1 {sharp:.3} vs level3 {rough:.3}"
        );

        // Prefiltering redistributes energy; it must never manufacture any. (The exact-conservation case
        // is a CONSTANT environment, covered by `the_mip_chain_preserves_average_radiance` — a prefiltered
        // top mip is NOT the sphere average, because its GGX lobe is centred on that texel's own
        // direction rather than covering the whole sphere.)
        for (index, level) in mips.iter().enumerate() {
            for i in 0..(level.0 * level.1) {
                let value = decode_red(&level.2, i);
                assert!(
                    value.is_finite() && value >= 0.0,
                    "level {index} texel {i} is not a valid radiance: {value}"
                );
                assert!(
                    value <= 400.0 * 1.01,
                    "level {index} texel {i} exceeds the source peak: {value}"
                );
            }
        }
    }

    /// Evaluate the shader's SH sum on the CPU, so the test exercises the same series the GPU will.
    fn eval_sh(coeff: &[[f32; 4]; SH_COEFFS], n: Vec3) -> Vec3 {
        let basis = sh_basis(n);
        let mut out = Vec3::ZERO;
        for i in 0..SH_COEFFS {
            out += Vec3::new(coeff[i][0], coeff[i][1], coeff[i][2]) * basis[i];
        }
        out.max(Vec3::ZERO)
    }

    #[test]
    fn constant_light_from_every_direction_reproduces_itself_exactly() {
        // The white-furnace case: a uniform environment of radiance L must give every normal exactly L.
        // Any other answer means the projection, the cosine convolution or the 1/PI is wrong, and the
        // whole scene would be uniformly mis-lit in a way that is easy to mistake for an exposure choice.
        let (w, h) = (64usize, 32usize);
        let pixels = vec![[0.7f32, 0.7, 0.7]; w * h];
        let coeff = sh_irradiance_coefficients(w, h, &pixels);
        for dir in [
            Vec3::Y,
            Vec3::NEG_Y,
            Vec3::X,
            Vec3::NEG_Z,
            Vec3::new(0.3, 0.6, -0.7).normalize(),
        ] {
            let got = eval_sh(&coeff, dir);
            assert!(
                (got.x - 0.7).abs() < 0.01 && (got.y - 0.7).abs() < 0.01,
                "uniform radiance 0.7 must come back as 0.7 for {dir:?}, got {got:?}"
            );
        }
    }

    #[test]
    fn irradiance_is_directional_and_peaks_towards_the_light() {
        // The defect this pins: diffuse ambient used to come from a 4x2 GGX mip, so a surface facing the
        // key and a surface facing away received nearly the same grey and machinery lost its form. A
        // correct cosine convolution must be strongly directional.
        let (w, h) = (128usize, 64usize);
        let mut pixels = vec![[0.0f32; 3]; w * h];
        // A bright patch overhead.
        for y in 0..(h / 8) {
            for x in 0..w {
                pixels[y * w + x] = [50.0, 50.0, 50.0];
            }
        }
        let coeff = sh_irradiance_coefficients(w, h, &pixels);
        let up = eval_sh(&coeff, Vec3::Y).y;
        let down = eval_sh(&coeff, Vec3::NEG_Y).y;
        assert!(
            up > down * 4.0,
            "a normal facing an overhead source must receive far more than one facing away: {up} vs {down}"
        );
        assert!(down >= 0.0, "irradiance must never go negative, got {down}");
    }

    #[test]
    fn sun_is_hdr_and_ground_is_dim() {
        // The OUTDOOR env (`MTK_ENV=sky`, studio=false) — the sun/ground contrast this asserts is a
        // property of the sky look, not the neutral studio.
        let sun_dir = Vec3::new(0.35, 0.55, 0.40).normalize();
        assert!(sky_radiance(sun_dir, false).x > 5.0, "the sun is HDR (≫1)");
        assert!(
            sky_radiance(Vec3::NEG_Y, false).length() < 0.4,
            "the ground is dim"
        );
    }
    fn env(w: usize, h: usize) -> EnvSource {
        EnvSource {
            width: w,
            height: h,
            pixels: vec![[1.0, 1.0, 1.0]; w * h],
            label: "test".into(),
        }
    }

    #[test]
    fn a_two_to_one_panorama_is_accepted() {
        assert!(env(64, 32).validate().is_ok());
        assert!(env(4096, 2048).validate().is_ok());
    }

    #[test]
    fn a_non_equirectangular_image_is_refused_with_its_actual_shape() {
        // Stretching a square photo onto the sphere silently is the wrong answer: it produces a
        // plausible-looking sky whose lighting directions are all wrong.
        let err = env(32, 32).validate().expect_err("square is refused");
        assert!(err.contains("equirectangular"), "{err}");
        assert!(err.contains("32x32"), "must state the actual shape: {err}");
    }

    #[test]
    fn an_empty_or_mismatched_image_is_refused() {
        assert!(env(0, 0).validate().is_err());
        let mut broken = env(64, 32);
        broken.pixels.truncate(10);
        let err = broken.validate().expect_err("refused");
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn non_finite_radiance_never_reaches_the_gpu() {
        // A NaN in an env map propagates through the mip chain into every reflection in the scene.
        let mut bad = env(64, 32);
        bad.pixels[100] = [f32::NAN, 0.0, 0.0];
        assert!(bad.validate().is_err());
        let mut inf = env(64, 32);
        inf.pixels[7] = [0.0, f32::INFINITY, 0.0];
        assert!(inf.validate().is_err());
    }

    #[test]
    fn a_supplied_panorama_builds_a_complete_mip_chain_down_to_one_texel() {
        // The diffuse irradiance is read from the TOP mip, so a chain that stops early would leave
        // ambient lighting sampling a still-detailed image — subtly wrong in a way nobody would trace.
        let mips = build_env_mips_from((64, 32, vec![[0.5, 0.25, 0.125]; 64 * 32]));
        assert_eq!(mips[0].0, 64);
        assert_eq!(mips[0].1, 32);
        let (lw, lh, _) = mips.last().expect("at least one level");
        assert_eq!((*lw, *lh), (1, 1), "the chain must reach 1x1");
        // 64x32 → 32x16 → 16x8 → 8x4 → 4x2 → 2x1 → 1x1
        assert_eq!(mips.len(), 7, "unexpected chain length");
    }

    #[test]
    fn the_mip_chain_preserves_average_radiance() {
        // A box filter should conserve energy: the 1x1 top mip is the panorama's mean. If it did not,
        // an imported sky would light the scene at the wrong intensity.
        let mips = build_env_mips_from((64, 32, vec![[4.0, 2.0, 1.0]; 64 * 32]));
        let (_, _, top) = mips.last().expect("top mip");
        // rgba16f: 4 halves per texel. Decode the red channel back to f32.
        let bits = top[0];
        let exp = ((bits >> 10) & 0x1f) as i32;
        let mant = (bits & 0x3ff) as f32;
        let red = (1.0 + mant / 1024.0) * 2f32.powi(exp - 15);
        assert!(
            (red - 4.0).abs() < 0.2,
            "a uniform 4.0 panorama should average 4.0, got {red}"
        );
    }
}
