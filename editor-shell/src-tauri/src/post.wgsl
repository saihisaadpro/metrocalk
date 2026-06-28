// M11.4 (ADR-043) — bloom post-processing (display-space). A SEPARATE module from scene.wgsl so its own
// group(0) bindings (a sampler + source texture[s]) don't clash with the scene shader's Camera uniform.
//
// Per-frame pipeline when bloom is on:
//   scene pass            → scene_tex (full-res, the composited viewport)
//   fs_bright(scene_tex)  → bloom_a  (half-res: extract bright highlights)
//   fs_blur_h(bloom_a)    → bloom_b  (separable Gaussian, horizontal)
//   fs_blur_v(bloom_b)    → bloom_a  (separable Gaussian, vertical)
//   fs_composite(scene,bloom_a) → swapchain (scene + bloom)
// Bright/blur sample one texture (bindings 0,1); composite samples two (0,1,2). Each entry only references
// the bindings it needs, so the bright/blur pipelines use a 1-texture layout and composite a 2-texture one.

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

@group(0) @binding(0) var samp: sampler;
@group(0) @binding(1) var src: texture_2d<f32>;       // scene (bright/composite) or previous blur (blur)
@group(0) @binding(2) var bloom_tex: texture_2d<f32>; // composite only — the blurred bloom

// Display-space bloom runs AFTER the ACES tonemap, where values are compressed into ~[0,1], so the
// threshold is fairly high (only genuinely bright highlights glow) and the add is strong enough to read.
const THRESHOLD: f32 = 0.70; // luminance above which a pixel contributes to bloom
const INTENSITY: f32 = 1.10; // bloom add strength at composite

// Bright pass: keep only the energy ABOVE the threshold (so only bright highlights glow), preserving hue.
@fragment
fn fs_bright(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(src, samp, in.uv).rgb;
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let contrib = max(luma - THRESHOLD, 0.0) / max(luma, 1e-4); // fraction of c above the threshold
    return vec4<f32>(c * contrib, 1.0);
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

// Composite: the original scene plus the additive blurred bloom.
@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let scene = textureSample(src, samp, in.uv).rgb;
    let bloom = textureSample(bloom_tex, samp, in.uv).rgb;
    return vec4<f32>(scene + bloom * INTENSITY, 1.0);
}
