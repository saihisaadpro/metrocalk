// Screen-space ambient occlusion (SSAO) — a post pass that darkens creases / contact points where geometry
// is mutually occluded, so an imported CAD assembly reads as solid connected parts, not stacked floating
// boxes. Runs AFTER the in-shader ACES tonemap (the scene is already display-space), so this is a
// display-space AO multiply on the offscreen scene colour. Depth is the scene depth (MSAA → textureLoad
// sample 0); positions are reconstructed via the camera's inv_view_proj; the geometric normal is
// reconstructed from screen-space derivatives (no G-buffer normal needed).

struct Camera {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    focus: vec4<f32>,   // focus.yzw = world-space camera eye
    shadow: vec4<f32>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

@group(1) @binding(0) var samp: sampler;
@group(1) @binding(1) var color_tex: texture_2d<f32>;
@group(1) @binding(2) var depth_tex: texture_depth_multisampled_2d;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// The fullscreen triangle (matches post.wgsl's vs_post: UV in [0,1], origin top-left).
@vertex
fn vs_post(@builtin(vertex_index) vid: u32) -> VsOut {
    var o: VsOut;
    let x = f32((vid << 1u) & 2u);
    let y = f32(vid & 2u);
    o.uv = vec2<f32>(x, y);
    o.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return o;
}

// Reconstruct a world-space position from a screen UV + a non-linear depth sample.
fn world_pos(uv: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, depth, 1.0);
    let p = cam.inv_view_proj * ndc;
    return p.xyz / p.w;
}

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(12.9898, 78.233))) * 43758.5453);
}

const SAMPLES: i32 = 24;       // more samples → less speckle (no separate blur pass)
const RADIUS: f32 = 0.06;      // world units (metres) — the occlusion search radius
const BIAS: f32 = 0.005;       // depth bias to avoid self-occlusion
const STRENGTH: f32 = 0.72;    // 0 = no AO, 1 = full
const POWER: f32 = 1.4;        // contrast of the AO term

@fragment
fn fs_ssao(in: VsOut) -> @location(0) vec4<f32> {
    let dim = vec2<f32>(textureDimensions(color_tex));
    let icoord = vec2<i32>(in.uv * dim);
    let scene = textureSample(color_tex, samp, in.uv).rgb;
    let d = textureLoad(depth_tex, icoord, 0);
    if (d >= 1.0) {
        return vec4<f32>(scene, 1.0); // background (no geometry) → no AO
    }

    let eye = cam.focus.yzw;
    let p = world_pos(in.uv, d);
    // Geometric normal from screen-space derivatives of the reconstructed position. The cross-product sign is
    // ambiguous (depends on screen winding), so FORCE it to face the camera — otherwise the hemisphere points
    // into the surface and every sample self-occludes (the whole surface goes black).
    var n = normalize(cross(dpdx(p), dpdy(p)));
    if (dot(n, eye - p) < 0.0) {
        n = -n;
    }
    // A tangent basis around n (guard the degenerate up-aligned case).
    var t = cross(n, vec3<f32>(0.0, 1.0, 0.0));
    if (dot(t, t) < 1e-4) {
        t = cross(n, vec3<f32>(1.0, 0.0, 0.0));
    }
    t = normalize(t);
    let b = cross(n, t);

    let rot = hash(in.uv * dim) * 6.2831853;
    var occ = 0.0;
    for (var i = 0; i < SAMPLES; i = i + 1) {
        // A cosine-ish hemisphere spiral: golden-angle rotation, radius grows with sqrt(i).
        let fi = (f32(i) + 0.5) / f32(SAMPLES);
        let ang = rot + f32(i) * 2.3999632;
        let r = RADIUS * sqrt(fi);
        let dir = t * cos(ang) + b * sin(ang);
        // Lift the sample off the surface along the normal (grows toward the rim) so a flat surface doesn't
        // false-occlude itself — the main source of flat-area speckle.
        let sample_pos = p + dir * r + n * (RADIUS * (0.35 + 0.4 * fi));
        let clip = cam.view_proj * vec4<f32>(sample_pos, 1.0);
        if (clip.w <= 0.0) {
            continue;
        }
        let suv = (clip.xy / clip.w) * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5);
        if (suv.x < 0.0 || suv.x > 1.0 || suv.y < 0.0 || suv.y > 1.0) {
            continue;
        }
        let sd = textureLoad(depth_tex, vec2<i32>(suv * dim), 0);
        let surf = world_pos(suv, sd);
        let sample_dist = distance(eye, sample_pos);
        let surf_dist = distance(eye, surf);
        // The surface at suv occludes the sample point if it sits in front of it (closer to the eye).
        // Range-check so a far-away surface across a depth gap doesn't over-darken.
        let range = smoothstep(0.0, 1.0, RADIUS / max(abs(sample_dist - surf_dist), 1e-4));
        if (surf_dist < sample_dist - BIAS) {
            occ = occ + range;
        }
    }
    let ao = clamp(1.0 - occ / f32(SAMPLES), 0.0, 1.0);
    let factor = mix(1.0, pow(ao, POWER), STRENGTH);
    return vec4<f32>(scene * factor, 1.0);
}

// Passthrough blit (used when bloom is off): copy the AO'd scene colour to the swapchain.
@fragment
fn fs_blit(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(textureSample(color_tex, samp, in.uv).rgb, 1.0);
}
