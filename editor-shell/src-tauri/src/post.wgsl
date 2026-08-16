// M15.11 — the HDR post chain and THE authoritative final resolve.
//
// Everything upstream of this file is SCENE LINEAR radiance in an Rgba16Float target. `fs_resolve` is the
// one and only place a frame becomes display colour: exposure, tone curve and transfer function all happen
// here, exactly once, for every routing combination. If you find yourself adding a tone map or a gamma
// encode anywhere else, that is the bug this module exists to prevent.
//
// Per-frame chain (bloom optional, SSAO optional — both stay inside the HDR half):
//   scene pass                      → hdr_scene  (Rgba16Float, MSAA-resolved in LINEAR space)
//   fs_ssao(hdr_scene, depth)       → hdr_scene  (linear AO multiply; see ssao.wgsl)
//   fs_bright(hdr_scene)            → bloom_a    (half-res: energy the tone curve will compress)
//   fs_blur_h(bloom_a)              → bloom_b    (separable Gaussian, horizontal)
//   fs_blur_v(bloom_b)              → bloom_a    (separable Gaussian, vertical)
//   fs_resolve(hdr_scene, bloom_a)  → swapchain  (exposure → bloom add → tone curve → OETF → dither)
// With bloom off, `bloom_a` is a 1×1 black texture and the SAME `fs_resolve` runs: one pass, one code
// path, no branch that can drift.
//
// Group 0 is the scene Camera uniform (exposure in `shadow.y`, cinematic/CAD profile in `shadow.z`, the
// display-encode selector in `grid.w`); group 1 is the sampler + source texture(s). Bright/blur bind a
// 1-texture layout, resolve a 2-texture one, so each entry point only references bindings it has.

struct Camera {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    focus: vec4<f32>,
    // `.y` = exposure, `.z` = cinematic(0)/CAD(1) presentation profile.
    shadow: vec4<f32>,
    // `.w` = 1.0 when this pass must apply the sRGB OETF itself (a linear-store swapchain), 0.0 when the
    // surface format converts in hardware. Applying both would double-encode the frame.
    grid: vec4<f32>,
    // The working space (mirrors render.rs's `ColourUniform`). The two ingress matrices are unused in
    // this file — nothing enters the pipeline here — but they must be declared to reach `from_working`,
    // which this file is the ONLY user of: it is the seam where the renderer's working space ends and
    // the view transform's own space begins. `luma.xyz` is what "bright" means in the working space.
    to_working: mat3x3<f32>,
    env_to_working: mat3x3<f32>,
    from_working: mat3x3<f32>,
    luma: vec4<f32>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

@group(1) @binding(0) var samp: sampler;
@group(1) @binding(1) var src: texture_2d<f32>;       // scene HDR (bright/resolve) or previous blur (blur)
@group(1) @binding(2) var bloom_tex: texture_2d<f32>; // resolve only — the blurred bloom (black when off)

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A single fullscreen triangle (3 verts, no vertex buffer). UV.y matches the texture (0 = top).
@vertex
fn vs_post(@builtin(vertex_index) vid: u32) -> VsOut {
    var out: VsOut;
    let x = f32((vid << 1u) & 2u); // 0, 2, 0
    let y = f32(vid & 2u);         // 0, 0, 2
    out.uv = vec2<f32>(x, y);
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}

// Luminance IN THE WORKING SPACE — the basis for bloom extraction. A per-channel max would let a
// saturated but dim colour glow as hard as a genuinely bright one; luminance is what "bright" means.
//
// The weights come from the uniform rather than being the Rec.709 constants they used to be. That was
// the working-space bug in miniature: the Y row of AP1 is (0.2722, 0.6741, 0.0537), so shading in
// ACEScg while metering with Rec.709's (0.2126, 0.7152, 0.0722) would under-read reds by a quarter and
// over-read greens — bloom would pick the wrong pixels, and it would read as a threshold that needs
// tuning rather than as a colour bug.
fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, cam.luma.xyz);
}

// ── bloom ─────────────────────────────────────────────────────────────────────────────────────────────
// The old threshold of 0.78 was tuned against TONEMAPPED values living in [0,1]; in linear HDR it would
// have caught most of the lit scene. The threshold is now stated in EXPOSED linear units and sits at the
// point where the tone curve starts genuinely compressing (ACES maps 1.0 → 0.80), so bloom extracts the
// energy the display cannot represent — which is what bloom is FOR. Because extraction is exposure-aware,
// raising exposure blooms more, exactly as a real camera does.
const BLOOM_THRESHOLD: f32 = 1.0;
const BLOOM_KNEE: f32 = 0.5;   // soft knee half-width; avoids a hard halo edge at the threshold
const BLOOM_CLAMP: f32 = 12.0; // firefly suppression: one blown specular texel must not pump the whole kernel
const BLOOM_INTENSITY: f32 = 0.35;

// Bright pass: keep the energy above the threshold, preserving hue. Output is in EXPOSED units, and
// `fs_resolve` adds it after exposure — so the two never disagree about what scale bloom lives on.
@fragment
fn fs_bright(in: VsOut) -> @location(0) vec4<f32> {
    let hdr = max(textureSample(src, samp, in.uv).rgb, vec3<f32>(0.0));
    let exposed = min(hdr * cam.shadow.y, vec3<f32>(BLOOM_CLAMP));
    let l = luminance(exposed);
    // Quadratic knee (Karis): a smooth ramp through the threshold instead of a step, which is what keeps
    // the halo boundary from crawling as the camera moves.
    var soft = clamp(l - BLOOM_THRESHOLD + BLOOM_KNEE, 0.0, 2.0 * BLOOM_KNEE);
    soft = soft * soft / (4.0 * BLOOM_KNEE + 1e-4);
    let contribution = clamp(max(l - BLOOM_THRESHOLD, soft) / max(l, 1e-4), 0.0, 1.0);
    return vec4<f32>(exposed * contribution, 1.0);
}

// A 9-tap separable Gaussian; texel size derived from the source dims (resolution-independent).
fn blur(in: VsOut, dir: vec2<f32>) -> vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    let off = dir * texel;
    let w0 = 0.227027;
    let w1 = 0.1945946;
    let w2 = 0.1216216;
    let w3 = 0.054054;
    let w4 = 0.016216;
    var acc = textureSample(src, samp, in.uv).rgb * w0;
    acc = acc + textureSample(src, samp, in.uv + off * 1.0).rgb * w1;
    acc = acc + textureSample(src, samp, in.uv - off * 1.0).rgb * w1;
    acc = acc + textureSample(src, samp, in.uv + off * 2.0).rgb * w2;
    acc = acc + textureSample(src, samp, in.uv - off * 2.0).rgb * w2;
    acc = acc + textureSample(src, samp, in.uv + off * 3.0).rgb * w3;
    acc = acc + textureSample(src, samp, in.uv - off * 3.0).rgb * w3;
    acc = acc + textureSample(src, samp, in.uv + off * 4.0).rgb * w4;
    acc = acc + textureSample(src, samp, in.uv - off * 4.0).rgb * w4;
    return vec4<f32>(acc, 1.0);
}

@fragment
fn fs_blur_h(in: VsOut) -> @location(0) vec4<f32> {
    return blur(in, vec2<f32>(1.0, 0.0));
}

@fragment
fn fs_blur_v(in: VsOut) -> @location(0) vec4<f32> {
    return blur(in, vec2<f32>(0.0, 1.0));
}

// ── the final resolve ─────────────────────────────────────────────────────────────────────────────────
// Cinematic mode uses the Narkowicz ACES fit; CAD mode uses the Khronos PBR Neutral reference curve, which
// preserves material hue/saturation under neutral lighting. These moved here from scene.wgsl — they used
// to run per scene fragment, which meant MSAA averaged already-compressed values.
fn tonemap_aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn tonemap_pbr_neutral(input_color: vec3<f32>) -> vec3<f32> {
    let start_compression = 0.8 - 0.04;
    let desaturation = 0.15;
    let x = min(input_color.r, min(input_color.g, input_color.b));
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    let color = input_color - vec3<f32>(offset);
    let peak = max(color.r, max(color.g, color.b));
    if (peak < start_compression) {
        return color;
    }
    let new_peak = 1.0 - (1.0 - start_compression) * (1.0 - start_compression)
        / (peak + 1.0 - 2.0 * start_compression);
    let compressed = color * (new_peak / peak);
    let g = 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(vec3<f32>(new_peak), compressed, g);
}

// The exact sRGB OETF rather than a 2.2 approximation, so dark gradients and brand colours stay stable.
fn to_srgb(x: vec3<f32>) -> vec3<f32> {
    let safe = clamp(x, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = safe * 12.92;
    let high = 1.055 * pow(safe, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(high, low, safe <= vec3<f32>(0.0031308));
}

// Interleaved-gradient noise, ±0.5/255 in display space. The tone curve's shoulder maps a wide band of HDR
// values onto adjacent 8-bit codes, so a clear sky or a dark gradient banded visibly; a sub-LSB dither
// turns those contours back into noise the eye integrates away. Applied AFTER encoding, which is the only
// place the quantisation step is uniform.
fn dither(pos: vec2<f32>) -> f32 {
    let m = fract(52.9829189 * fract(dot(pos, vec2<f32>(0.06711056, 0.00583715))));
    return (m - 0.5) / 255.0;
}

// THE final resolve. Every routing combination — SSAO on/off × bloom on/off, MSAA on/off, perspective or
// orthographic — terminates here and nowhere else.
@fragment
fn fs_resolve(in: VsOut) -> @location(0) vec4<f32> {
    let hdr = max(textureSample(src, samp, in.uv).rgb, vec3<f32>(0.0));
    // 1. Exposure, on the linear scene.
    let exposed = hdr * cam.shadow.y;
    // 2. Bloom, already in exposed units (a 1×1 black texture when bloom is off ⇒ adds exactly nothing).
    let bloom = max(textureSample(bloom_tex, samp, in.uv).rgb, vec3<f32>(0.0));
    let composed = exposed + bloom * BLOOM_INTENSITY;
    // 3. THE SEAM: out of the renderer's working space, into the space the view transform is defined on.
    //
    // Both curves below are specified on linear Rec.709/sRGB primaries — the Khronos PBR Neutral
    // reference implementation is, and the Narkowicz fit was published against the same. Feeding them
    // AP1 channels would apply a per-channel curve to channels it does not describe, which shows up as
    // a hue shift in saturated highlights: the fault is invisible on a grey ramp and obvious on a red
    // light, which is why it survives casual review. Identity when the working space IS Rec.709.
    let view_input = cam.from_working * composed;
    // 4. The view transform. Exactly one runs, and this is the only place either is called.
    var mapped: vec3<f32>;
    if (cam.shadow.z > 0.5) {
        mapped = tonemap_pbr_neutral(view_input);
    } else {
        mapped = tonemap_aces(view_input);
    }
    // 5. The display transfer function — but only when the swapchain is not doing it for us.
    var display = mapped;
    if (cam.grid.w > 0.5) {
        display = to_srgb(mapped);
    }
    // 6. Break up 8-bit quantisation contours.
    return vec4<f32>(display + vec3<f32>(dither(in.pos.xy)), 1.0);
}
