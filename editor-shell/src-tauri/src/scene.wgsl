// Editor viewport shader: instanced entity cubes (from the storage buffer the app fills from /core
// Transforms) + a ground grid. Selected entity highlights. Matches render.rs's Instance/Camera.

// `inv_view_proj` (M11.3 inc.2) lets the skybox turn a screen pixel into a world ray; `light_view_proj`
// (M11.3 inc.3) is the shadow-casting light's ortho view-proj, for both the depth pass and fs_mesh's shadow
// lookup. The cube/grid/line shaders ignore the trailing fields. Field order matches render.rs's `Camera`
// (view_proj · inv_view_proj · light_view_proj · focus · shadow · grid · the colour block) — **336 bytes**,
// std140-clean: 3 mat4 (192) + 3 vec4 (48) + two column-padded mat3 (48 each). This shader declares a
// PREFIX of the 400-byte uniform render.rs writes, which is deliberate — naming only the ingress half is
// what stops the scene pass from being able to apply the inverse conversion. Pinned by
// `the_uniform_layouts_the_shaders_declare_fit_the_buffer_the_renderer_writes` and by
// `tools/gpu-contract-audit`. (Said 240 until 2026-08-17 — stale since the colour block landed.)
struct Camera {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    focus: vec4<f32>,
    // M11.3 inc.3 — `shadow.x` = index of the shadow-casting directional light (-1 = none); the single
    // shadow map applies to ONLY that light, so other directionals (no map) stay unshadowed. `.y` is
    // exposure, `.z` is the cinematic(0)/CAD(1) presentation profile, `.w` is shadow quality 0..3.
    shadow: vec4<f32>,
    // Adaptive grid: target X/Z in `.xy`, camera distance in `.z`.
    grid: vec4<f32>,
    // THE WORKING SPACE, ingress half. `to_working` takes linear Rec.709 — which is what every authored
    // factor, light colour, imported material and decoded sRGB texel means — into the space this
    // renderer shades in. The identity when the working space is linear Rec.709, so both working spaces
    // run the SAME instructions and neither can rot.
    //
    // The buffer also carries `from_working` and the luminance weights after this field, and this
    // struct DELIBERATELY stops here: the scene pass has no business leaving the working space or
    // metering brightness, and a field it cannot name is a rule it cannot break. post.wgsl declares the
    // full block. (WGSL permits a uniform struct smaller than its buffer, which is also how ssao.wgsl
    // and the shadow passes get away with declaring even less.)
    to_working: mat3x3<f32>,
    // The ENVIRONMENT's declared source space → the working space, composed into one matrix. Equal to
    // `to_working` unless someone has declared the panorama to be in another gamut — which the file
    // itself usually cannot say, since Radiance .hdr has no required primaries header.
    env_to_working: mat3x3<f32>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

// Linear Rec.709 → the renderer's working space.
//
// EVERY colour that takes part in a colour computation passes through here FIRST: base colour, emissive,
// light radiance, environment radiance, and the authored chrome constants. That ordering is the whole
// claim — converting the finished framebuffer instead would be rendering in Rec.709 and relabelling the
// result, which is the thing "we support ACEScg" usually turns out to mean.
fn to_working(c: vec3<f32>) -> vec3<f32> {
    return cam.to_working * c;
}

// The environment's declared source space → the working space. Every sample of the env map goes through
// THIS one — the sky backdrop, the diffuse irradiance and the specular reflection — so a declaration
// cannot take effect in the reflection but not in the sky behind it.
fn env_to_working(c: vec3<f32>) -> vec3<f32> {
    return cam.env_to_working * c;
}

// M11.3 inc.3 (ADR-042) — the directional shadow map: a depth texture rendered from the shadow-casting
// light's POV, sampled with a comparison sampler (hardware PCF). Filled by the depth-only shadow pass before
// the main pass each frame; a render projection, never doc state. Shares GROUP 3 with the IBL env/LUT
// (bindings 4/5) because the device caps bind groups at 4 (web-portable) — and the shadow pass doesn't bind
// group 3, so there's no render-target-vs-sampled conflict.
@group(3) @binding(4) var shadow_map: texture_depth_2d;
@group(3) @binding(5) var shadow_samp: sampler_comparison;

// Cosine-convolved SH irradiance (ibl.rs), the diffuse half of the environment. Nine RGB coefficients
// reproduce a cosine lobe to about 1%, which is why this is a uniform and not a texture fetch.
struct Irradiance { coeff: array<vec4<f32>, 9> };
@group(3) @binding(6) var<uniform> sh: Irradiance;

/// Average incident radiance for a normal. The cosine convolution and the Lambert 1/PI are already folded
/// into the coefficients, so this is directly the quantity the diffuse term multiplies its albedo by.
fn sh_irradiance(n: vec3<f32>) -> vec3<f32> {
    let x = n.x;
    let y = n.y;
    let z = n.z;
    var r = sh.coeff[0].rgb * 0.282095;
    r = r + sh.coeff[1].rgb * (0.488603 * y);
    r = r + sh.coeff[2].rgb * (0.488603 * z);
    r = r + sh.coeff[3].rgb * (0.488603 * x);
    r = r + sh.coeff[4].rgb * (1.092548 * x * y);
    r = r + sh.coeff[5].rgb * (1.092548 * y * z);
    r = r + sh.coeff[6].rgb * (0.315392 * (3.0 * z * z - 1.0));
    r = r + sh.coeff[7].rgb * (1.092548 * x * z);
    r = r + sh.coeff[8].rgb * (0.546274 * (x * x - y * y));
    // A truncated SH series can ring slightly negative under a very bright, very small source.
    return max(r, vec3<f32>(0.0));
}

// Fraction of the directional light reaching `world_pos` (1 = fully lit, 0 = fully shadowed). Projects into
// the light's clip space, does quality-scaled tent PCF with receiver/slope bias. Anything outside the
// shadow frustum is treated as lit (the map only covers the scene's fitted bounds).
fn shadow_factor(world_pos: vec3<f32>, n_dot_l: f32) -> f32 {
    let lc = cam.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc = lc.xyz / lc.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5, 0.5); // clip → UV (flip Y)
    if (uv.x < 0.0 || uv.x > 1.0 || uv.y < 0.0 || uv.y > 1.0 || ndc.z > 1.0 || ndc.z < 0.0) {
        return 1.0; // outside the shadow frustum → unshadowed
    }
    // Slope-scaled bias: steeper grazing angles need more, to avoid shadow acne; clamped so flat faces
    // don't peter-pan (detach their contact shadow).
    let slope_bias = 0.0012 * tan(acos(clamp(n_dot_l, 0.0, 1.0)));
    // Receiver-plane depth gradient catches the cases where normal-only bias underestimates a large
    // projected slope. It materially reduces crawling acne without globally detaching contact shadows.
    let receiver_bias = max(abs(dpdx(ndc.z)), abs(dpdy(ndc.z))) * 1.5;
    let bias = clamp(max(slope_bias, receiver_bias), 0.00035, 0.0035);
    let ref_depth = ndc.z - bias;
    let dim = vec2<f32>(textureDimensions(shadow_map));
    let texel = 1.0 / dim;
    let quality = cam.shadow.w;
    if (quality < 1.5) {
        return textureSampleCompareLevel(shadow_map, shadow_samp, uv, ref_depth);
    }
    let radius = select(1, 2, quality > 2.5);
    var sum = 0.0;
    var weight_sum = 0.0;
    for (var dy = -2; dy <= 2; dy = dy + 1) {
        for (var dx = -2; dx <= 2; dx = dx + 1) {
            if (abs(dx) > radius || abs(dy) > radius) {
                continue;
            }
            let o = vec2<f32>(f32(dx), f32(dy)) * texel;
            // 3×3 [1 2 1] or 5×5 [1 2 3 2 1] separable tent. Hardware comparison filtering expands
            // this into a soft, stable kernel without the box-PCF stepping visible during orbit.
            let wx = f32(radius + 1 - abs(dx));
            let wy = f32(radius + 1 - abs(dy));
            let weight = wx * wy;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_samp, uv + o, ref_depth) * weight;
            weight_sum = weight_sum + weight;
        }
    }
    return sum / weight_sum;
}

// ── COLOUR-SPACE CONTRACT ─────────────────────────────────────────────────────────────────────────────
// EVERY fragment entry point in this file writes SCENE-LINEAR HDR into an Rgba16Float attachment.
// There is no exposure, no tone curve and no transfer function here: post.wgsl's `fs_resolve` is the ONE
// place the frame becomes display colour. Do not reintroduce a `display_encode` in this file — MSAA would
// then resolve gamma-encoded samples and bloom/SSAO would operate on compressed values, which is exactly
// the defect the HDR intermediate exists to remove. The Rust-side statement of this contract (and its
// tests) lives in render.rs; keep the two in step.
//
// Lit surfaces already produce linear radiance, so nothing is converted for them. UNLIT authored colour
// (cube tints, grid, lines, gizmos, markers, overlays, selection) enters through
// `unlit_srgb_to_scene_linear`, which inverts the whole display transform so an authored colour renders
// back as itself at any exposure. Mirror of render.rs's function of the same name.

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let s = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let low = s / 12.92;
    let high = pow((s + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, s <= vec3<f32>(0.04045));
}

// Ceiling on the exposed value an unlit colour may inverse-map to; keeps bright helpers out of the bloom
// extractor's blow-out range. See render.rs `UNLIT_EXPOSED_CEILING`.
const UNLIT_EXPOSED_CEILING: f32 = 2.0;
// The brightest display-linear value an unlit colour may target, per profile. Applied to the TARGET and
// scaling the colour UNIFORMLY, so the hue is exact and only the level gives. ACES is capped at the curve
// evaluated at the ceiling (authored white → 0.962 sRGB). PBR Neutral is capped much lower (→ 0.896 sRGB)
// because above its knee that curve DESATURATES, lifting the darkest channel: at a higher cap the inverse
// returned a negative channel and the amber light marker's blue came back at 0.39 instead of 0.20.
// Kept in step with render.rs by `the_unlit_display_caps_stay_in_each_curves_faithful_range`.
const UNLIT_DISPLAY_CAP_ACES: f32 = 0.914855;
const UNLIT_DISPLAY_CAP_PBR_NEUTRAL: f32 = 0.78;

// Inverse of the Narkowicz ACES fit, per channel: solve (yc-a)x² + (yd-b)x + ye = 0 for the non-negative root.
fn inverse_tonemap_aces(y_in: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    let y = clamp(y_in, vec3<f32>(0.0), vec3<f32>(0.999));
    let denom = vec3<f32>(a) - y * c;
    let bb = y * d - b;
    let cc = y * e;
    let disc = max(bb * bb + 4.0 * denom * cc, vec3<f32>(0.0));
    return max((bb + sqrt(disc)) / (2.0 * max(denom, vec3<f32>(1e-6))), vec3<f32>(0.0));
}

// Inverse of the Khronos PBR Neutral curve (the CAD profile). max(out) IS the compressed peak, which
// recovers the original peak in closed form; the black lift is recovered from the minimum channel.
fn inverse_tonemap_pbr_neutral(out_in: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desaturation = 0.15;
    let out_peak = clamp(max(out_in.r, max(out_in.g, out_in.b)), 0.0, 0.999);
    var color = out_in;
    if (out_peak >= start) {
        let new_peak = out_peak;
        let peak = 2.0 * start - 1.0 + (1.0 - start) * (1.0 - start) / (1.0 - new_peak);
        let g = 1.0 / (desaturation * (peak - new_peak) + 1.0);
        let compressed = vec3<f32>(new_peak) + (out_in - vec3<f32>(new_peak)) / g;
        // Decompression can undershoot below zero for a target the curve cannot produce; the caps keep
        // unlit colour out of that region, but clamp so an out-of-range value yields the nearest colour.
        color = max(compressed * (peak / new_peak), vec3<f32>(0.0));
    }
    let min_c = max(min(color.r, min(color.g, color.b)), 0.0);
    let offset = select(0.04, 0.4 * sqrt(min_c) - min_c, min_c < 0.04);
    return color + vec3<f32>(offset);
}

// AUTHORED sRGB → SCENE LINEAR, against a given exposure. The single conversion applied to unlit colour,
// on the way IN. Mirror of render.rs's `unlit_srgb_to_scene_linear`.
fn unlit_srgb_at_exposure(rgb: vec3<f32>, exposure: f32) -> vec3<f32> {
    let cad = cam.shadow.z > 0.5;
    var display_linear = srgb_to_linear(rgb);
    // Cap the TARGET uniformly, so the hue survives a channel that sits at display maximum.
    let cap = select(UNLIT_DISPLAY_CAP_ACES, UNLIT_DISPLAY_CAP_PBR_NEUTRAL, cad);
    let peak = max(display_linear.r, max(display_linear.g, display_linear.b));
    if (peak > cap) {
        display_linear = display_linear * (cap / peak);
    }
    var exposed: vec3<f32>;
    if (cad) {
        exposed = inverse_tonemap_pbr_neutral(display_linear);
    } else {
        exposed = inverse_tonemap_aces(display_linear);
    }
    // Guard exactly as render.rs does — fall back to 1.0 rather than dividing by a near-zero exposure,
    // or the two mirrors disagree at the bottom of the exposure range.
    let e = select(1.0, exposure, exposure > 1e-4);
    // The inverse display transform lands in linear Rec.709 (that is the space the tone curves are
    // defined on — see `ViewTransform::input_space`), so the last step is into the working space, like
    // every other colour. Doing it here rather than at the four call sites is why an authored colour
    // cannot escape the conversion.
    return to_working(clamp(exposed, vec3<f32>(0.0), vec3<f32>(UNLIT_EXPOSED_CEILING)) / e);
}

// ── two callers, and the difference between them matters ──────────────────────────────────────────────
//
// UI and helper overlays — grid, tracking lines, the contact debugger, light/camera markers, the transform
// gizmo, the selection tint, the focus-dim target — convert against the LIVE exposure, so they render as
// the authored colour no matter where the user puts the exposure slider. That is what UI has to do: a
// gizmo handle that dims when you stop down the scene is a gizmo you cannot grab.
fn unlit_srgb_to_scene_linear(rgb: vec3<f32>) -> vec3<f32> {
    return unlit_srgb_at_exposure(rgb, cam.shadow.y);
}

// Placeholder scene GEOMETRY (the fallback cubes for entities with no mesh asset) converts against the
// REFERENCE exposure instead. It is content, not chrome: it has to brighten and blow out with the rest of
// the frame when the exposure comes up, or it reads as pasted onto the image — which is exactly what an
// exposure sweep of the captured frames showed when this path used the live exposure. Anchoring at the
// default means the authored identity colour is still exactly right at the default exposure.
// Must equal render.rs's `DEFAULT_EXPOSURE`.
const REFERENCE_EXPOSURE: f32 = 0.45;
fn scene_srgb_to_scene_linear(rgb: vec3<f32>) -> vec3<f32> {
    return unlit_srgb_at_exposure(rgb, REFERENCE_EXPOSURE);
}

// Focus mode (M3.3): when `cam.focus_active > 0.5`, every entity that isn't the focused/selected one
// is grayed toward the background so it reads as faded/transparent (depth-correct, no alpha blend).
// `is_focused` ⇒ the lit one; everything else dims. Returns the de-emphasized colour.
//
// Two variants, because the two callers work in different spaces: the unlit vertex paths compose entirely
// in AUTHORED space and convert once at the fragment output, while `fs_mesh` is already SCENE LINEAR.
// Mixing toward the same authored target in each space keeps the faded look identical on both paths.
const DIM_TARGET = vec3<f32>(0.06, 0.07, 0.10); // the viewport clear colour — fade toward "gone"
const DIM_AMOUNT = 0.86;
// The selection accent, AUTHORED sRGB (matches the panel's selection yellow).
const SELECTION_TINT = vec3<f32>(1.0, 0.82, 0.16);
// The HOVER accent, AUTHORED sRGB. A different HUE, not a different strength: hover and selection are
// on screen at the same time constantly — you hover one part of an assembly you have already selected —
// and two brightnesses of one yellow is a reader guessing which is which. Cyan is the far side of the
// wheel from the selection yellow, so the two never read as degrees of each other, and it survives the
// grey a CAD import mostly is. Matches `color.accent.hover` in the editor's palette.
const HOVER_TINT = vec3<f32>(0.30, 0.80, 1.0);
// The grid line colour, AUTHORED sRGB. Written PREMULTIPLIED by coverage (see `fs_grid`).
const GRID_COLOR = vec3<f32>(0.10, 0.12, 0.17);
fn apply_focus_dim(col: vec3<f32>, is_focused: bool) -> vec3<f32> {
    if (cam.focus.x > 0.5 && !is_focused) {
        return mix(col, DIM_TARGET, DIM_AMOUNT);
    }
    return col;
}
fn apply_focus_dim_linear(col: vec3<f32>, is_focused: bool) -> vec3<f32> {
    if (cam.focus.x > 0.5 && !is_focused) {
        // The CONTENT anchor, not the chrome one: this fades a lit surface toward the BACKGROUND, and the
        // background is content — so a de-emphasised object and the backdrop it is fading into track
        // exposure together instead of drifting apart as the slider moves.
        return mix(col, scene_srgb_to_scene_linear(DIM_TARGET), DIM_AMOUNT);
    }
    return col;
}

// `rotation` is a unit quaternion (x,y,z,w); identity = (0,0,0,1). Applied per-instance so a tumbling
// physics body / a rotated authored Transform / a posed part actually *looks* rotated (M9.1+ — the shared
// renderer-rotation path). Matches render.rs's Instance — **64 bytes**, std430-clean (offsets
// 0/12/16/28/32/48, align 16). This is an `array<Instance>` STRIDE, so a host struct of any other size
// does not fail: element N is simply read from the wrong offset. Pinned by `tools/gpu-contract-audit`.
// (Said 48 until 2026-08-17 — stale since `material` landed.)
// `material` (M11.2) = per-entity PBR override [metallic, roughness, has_override, _]; when has_override>0.5
// the mesh path uses it (+ `color` as the override base color) instead of the asset's baked vertex material.
struct Instance { center: vec3<f32>, scale: f32, color: vec3<f32>, highlight: f32, rotation: vec4<f32>, material: vec4<f32> };
@group(1) @binding(0) var<storage, read> instances: array<Instance>;

// `highlight` is a small integer CODE, not a boolean — bit 0 = the committed selection, bit 1 = what
// the cursor is over. Two INDEPENDENT facts: an object is routinely both, and a hover that wrote 1.0
// would claim a selection the selection model does not have. Read back through `+ 0.5` rounding so
// the float is never compared for equality. Matches render.rs's `HIGHLIGHT_SELECTED`/`HIGHLIGHT_HOVERED`.
const HIGHLIGHT_SELECTED: u32 = 1u;
const HIGHLIGHT_HOVERED: u32 = 2u;
fn highlight_code(code: f32) -> u32 {
    return u32(max(code, 0.0) + 0.5);
}
fn is_selected(code: f32) -> bool {
    return (highlight_code(code) & HIGHLIGHT_SELECTED) != 0u;
}
fn is_hovered(code: f32) -> bool {
    return (highlight_code(code) & HIGHLIGHT_HOVERED) != 0u;
}
// M11.2 follow-up — the per-mesh base-color (albedo) texture rides the already-per-mesh instance group
// (group 1), staying within the 4-bind-group cap. An untextured mesh binds a 1×1 WHITE dummy, so sampling
// is always valid (white × the baked factor = the factor → an untextured mesh looks exactly as before).
// Only `fs_mesh` references these; the cube/grid/line pipelines' group-1 layout omits them (unused = fine).
@group(1) @binding(1) var base_color_tex: texture_2d<f32>;
@group(1) @binding(2) var base_color_samp: sampler;
// M11.2 follow-up — metallic-roughness (glTF: roughness=G, metalness=B) + tangent-space normal map, sharing
// the sampler. Untextured slots bind dummies (white → factor unchanged; flat-normal [128,128,255] → +Z).
@group(1) @binding(3) var mr_tex: texture_2d<f32>;
@group(1) @binding(4) var normal_tex: texture_2d<f32>;
@group(1) @binding(5) var ao_tex: texture_2d<f32>;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec3<f32> };

// Rotate vector `v` by unit quaternion `q` (x,y,z,w): v + 2·q.w·(qv×v) + 2·qv×(qv×v).
fn quat_rotate(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let t = 2.0 * cross(q.xyz, v);
    return v + q.w * t + cross(q.xyz, t);
}

fn corner(id: u32) -> vec3<f32> {
    return vec3<f32>(
        select(-1.0, 1.0, (id & 1u) != 0u),
        select(-1.0, 1.0, (id & 2u) != 0u),
        select(-1.0, 1.0, (id & 4u) != 0u),
    );
}

@vertex
fn vs_cube(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[ii];
    let local = corner(vi);
    let world = inst.center + quat_rotate(inst.rotation, local * inst.scale);
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    let nrm = quat_rotate(inst.rotation, normalize(local)); // rotate the face normal so lighting follows
    let shade = 0.55 + 0.45 * clamp(dot(nrm, normalize(vec3<f32>(0.4, 0.8, 0.3))), 0.0, 1.0);
    var col = inst.color * shade;
    if (is_selected(inst.highlight)) {
        col = mix(col, vec3<f32>(1.0, 0.85, 0.2), 0.7); // selection highlight
    }
    col = apply_focus_dim(col, is_selected(inst.highlight));
    // AFTER the focus dim, deliberately — see `fs_mesh`. Pointing at something is a question being
    // asked right now, and an answer that is itself faded out is not one.
    if (is_hovered(inst.highlight)) {
        col = mix(col, HOVER_TINT, 0.45);
    }
    out.color = col;
    return out;
}

// Derivative-antialiased adaptive grid. The prior 82 hardware lines were always two metres apart and all
// survived into the distance; at CAD frame-all distances they collapsed into alternating pixel rows (moiré).
// This is one transparent plane whose world-stable spacing follows powers of ten and whose `fwidth` coverage
// integrates sub-pixel lines instead of flickering. Its extent follows the camera target and distance.
struct GridOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world_xz: vec2<f32>,
    @location(1) local_xz: vec2<f32>,
};

@vertex
fn vs_grid(@builtin(vertex_index) vi: u32) -> GridOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let half_size = max(40.0, cam.grid.z * 2.2);
    let local = corners[vi] * half_size;
    let world_xz = cam.grid.xy + local;
    var out: GridOut;
    out.pos = cam.view_proj * vec4<f32>(world_xz.x, 0.0, world_xz.y, 1.0);
    out.world_xz = world_xz;
    out.local_xz = local / half_size;
    return out;
}

fn grid_line_coverage(world_xz: vec2<f32>, spacing: f32) -> f32 {
    let coordinate = world_xz / spacing;
    let width = max(fwidth(coordinate), vec2<f32>(0.00001));
    let distance_to_line = abs(fract(coordinate - 0.5) - 0.5) / width;
    return 1.0 - min(min(distance_to_line.x, distance_to_line.y), 1.0);
}

@fragment
fn fs_grid(in: GridOut) -> @location(0) vec4<f32> {
    let view_distance = max(cam.grid.z, 0.1);
    let decade = floor(log2(view_distance / 12.0) / log2(10.0));
    let spacing = clamp(pow(10.0, decade), 0.01, 100.0);
    let minor = grid_line_coverage(in.world_xz, spacing);
    let major = grid_line_coverage(in.world_xz, spacing * 10.0);
    let edge_fade = 1.0 - smoothstep(0.72, 1.0, length(in.local_xz));
    let alpha = max(minor * 0.20, major * 0.36) * edge_fade;
    if (alpha < 0.004) {
        discard;
    }
    // PREMULTIPLIED output into the linear-HDR target. The grid is the one alpha-blended scene element, and
    // it now blends in linear light rather than over gamma-encoded pixels — which is what stops the lines
    // going muddy where they cross a bright ground. Premultiplying (rather than straight ALPHA_BLENDING)
    // keeps the destination alpha meaningful and the maths identical for the colour channels.
    return vec4<f32>(unlit_srgb_to_scene_linear(GRID_COLOR) * alpha, alpha);
}

// Tracking lines (binding-by-intent edges, drawn between bound entity centres). Reuses the `instances`
// storage buffer purely as a 16-byte-aligned point carrier — the line pipeline binds a *different*
// buffer (the app's line-point list) to the same slot, and we only read `.center`. One LineList vertex
// per array element, so consecutive pairs form one segment. A fixed tracking colour (the panel's `#9fe`).
@vertex
fn vs_line(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(instances[vi].center, 1.0);
    // Tracking lines are "the rest of the elements" too — fade them in focus mode (never the focused one).
    out.color = apply_focus_dim(vec3<f32>(0.60, 1.0, 0.93), false);
    return out;
}

// M8.4 contact-debugger overlay lines — same LineList point carrier as `vs_line`, but each segment
// carries its OWN colour (contact crosses hot, normals amber, saturated-friction white, swept trajectory
// cool — so the overlay colour-codes load/jitter). NOT focus-dimmed: the debugger must stay fully legible.
// Off by default (empty buffer → the pass is skipped → zero per-frame cost).
@vertex
fn vs_overlay(@builtin(vertex_index) vi: u32) -> VsOut {
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(instances[vi].center, 1.0);
    out.color = instances[vi].color;
    return out;
}

// ── VFX particles ────────────────────────────────────────────────────────────────────────────────────
// One camera-facing quad per particle, six vertices, no vertex buffer: `vertex_index / 6` picks the
// particle out of the shared instance storage buffer and `% 6` picks the corner. The `Instance` slots are
// reused as a particle carrier exactly the way the line/overlay passes reuse them as a point carrier:
// `center` = world position, `scale` = world radius, `color` = LINEAR HDR colour (deliberately allowed
// above 1.0 — the excess is what the bloom pass picks up), `highlight` = opacity.
//
// The billboard basis is read straight out of the view-projection matrix. For M = P·V the first two ROWS
// of the 3×3 part are the camera's world right/up scaled by the projection terms, so normalising them
// gives the exact camera basis without a second uniform, a CPU-side sort, or a per-frame upload.
struct FxOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
};

@vertex
fn vs_particle(@builtin(vertex_index) vi: u32) -> FxOut {
    let pi = vi / 6u;
    let ci = vi % 6u;
    // 0,1,2 / 0,2,3 over the corners (-1,-1) (1,-1) (1,1) (-1,1).
    var order = array<u32, 6>(0u, 1u, 2u, 0u, 2u, 3u);
    let c = order[ci];
    let cx = select(-1.0, 1.0, c == 1u || c == 2u);
    let cy = select(-1.0, 1.0, c == 2u || c == 3u);

    let p = instances[pi];
    let vp = cam.view_proj;
    let right = normalize(vec3<f32>(vp[0][0], vp[1][0], vp[2][0]));
    let up = normalize(vec3<f32>(vp[0][1], vp[1][1], vp[2][1]));
    let world = p.center + right * (cx * p.scale) + up * (cy * p.scale);

    var out: FxOut;
    out.pos = vp * vec4<f32>(world, 1.0);
    out.color = p.color;
    out.uv = vec2<f32>(cx, cy);
    out.alpha = p.highlight;
    return out;
}

@fragment
fn fs_particle(in: FxOut) -> @location(0) vec4<f32> {
    // A round, soft-edged dot. A hard-edged square reads as a bug at any size, and the quadratic
    // falloff is what makes a cluster of quads look like one continuous flame instead of confetti.
    let d = length(in.uv);
    if (d > 1.0) { discard; }
    let falloff = 1.0 - d;
    let a = in.alpha * falloff * falloff;
    // PREMULTIPLIED: both blend modes below expect colour already scaled by coverage, so additive and
    // alpha-blended layers can share one fragment entry without one of them being subtly wrong.
    return vec4<f32>(in.color * a, a);
}

// Imported meshes (M4 asset pipeline) with metallic-roughness PBR (M11.2, ADR-041). The vertex stream
// carries position/normal/baked-base-color + the baked metallic+roughness factors; `vs_mesh` interpolates
// world position + normal + material across the triangle and `fs_mesh` evaluates a Cook-Torrance BRDF
// PER FRAGMENT over one directional light (the editor key light) + a small ambient. Non-bindless: one
// vertex/index buffer per asset (ADR-003). The cube `color` field of `instances[ii]` is unused here.
struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) metallic: f32,
    @location(4) roughness: f32,
    @location(5) uv: vec2<f32>,
    @location(6) tangent: vec4<f32>,
};

struct MeshVsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) base_color: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) mr: vec2<f32>,       // metallic, roughness
    @location(4) highlight: f32,
    @location(5) uv: vec2<f32>,
    @location(6) world_tangent: vec4<f32>,
};

@vertex
fn vs_mesh(v: MeshIn, @builtin(instance_index) ii: u32) -> MeshVsOut {
    let inst = instances[ii];
    let world = inst.center + quat_rotate(inst.rotation, v.position * inst.scale);
    var out: MeshVsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    out.world_pos = world;
    out.world_normal = quat_rotate(inst.rotation, normalize(v.normal));
    out.highlight = inst.highlight;
    // Per-entity material override (M11.2): a "make it metal/rusty/gold" intent recolors ONLY this entity;
    // absent → the asset's baked vertex material.
    let has_override = inst.material.z > 0.5;
    out.base_color = select(v.color, inst.color, has_override);
    out.mr = select(vec2<f32>(v.metallic, v.roughness), inst.material.xy, has_override);
    out.uv = v.uv;
    out.world_tangent = vec4<f32>(quat_rotate(inst.rotation, v.tangent.xyz), v.tangent.w);
    return out;
}

const PI = 3.14159265359;

// M11.3 (ADR-042) — the scene's authored lights (group 2), looped per fragment. Matches render.rs's
// `LightGpu`. `pos_kind.w`: 0=directional, 1=point, 2=spot. Directional/spot SHINE along `dir_range.xyz`;
// point/spot sit at `pos_kind.xyz` with `dir_range.w` = range falloff. `color_intensity` = linear RGB·intensity.
struct Light {
    pos_kind: vec4<f32>,
    color_intensity: vec4<f32>,
    dir_range: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> lights: array<Light>;

// M11.3 inc.2 (ADR-042) — image-based lighting (group 3). `env` is a procedural HDR sky (equirectangular,
// box-mip chain): mip 0 is the skybox, higher mips approximate roughness blur for specular IBL, and the top
// mip is a cheap diffuse irradiance. `brdf_lut` holds the split-sum (scale, bias) over (NdotV, roughness).
@group(3) @binding(0) var env: texture_2d<f32>;
@group(3) @binding(1) var env_samp: sampler;
@group(3) @binding(2) var brdf_lut: texture_2d<f32>;
@group(3) @binding(3) var lut_samp: sampler;

// Unit direction → equirect UV. MUST stay the inverse of ibl.rs's `texel_dir`.
fn dir_to_equirect(d: vec3<f32>) -> vec2<f32> {
    let u = atan2(d.z, d.x) / (2.0 * PI) + 0.5;
    let v = acos(clamp(d.y, -1.0, 1.0)) / PI;
    return vec2<f32>(u, v);
}

// Fresnel-Schlick with a roughness-aware ceiling (Sébastien Lagarde) — the ambient/IBL Fresnel, which
// (unlike the per-light term) has no single half-vector, so it uses NdotV and rolls off with roughness.
fn fresnel_schlick_roughness(cos_theta: f32, f0: vec3<f32>, roughness: f32) -> vec3<f32> {
    let inv_rough = vec3<f32>(1.0 - roughness);
    return f0 + (max(inv_rough, f0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

// The tone curves and the sRGB OETF used to live here, and `fs_mesh`/`fs_sky` called them per fragment.
// They now live in post.wgsl and run ONCE, in `fs_resolve`, against the resolved HDR scene. Everything in
// this file stays in scene-linear radiance.

// GGX/Trowbridge-Reitz normal distribution.
fn distribution_ggx(n_dot_h: f32, rough: f32) -> f32 {
    let a = rough * rough;
    let a2 = a * a;
    let d = n_dot_h * n_dot_h * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-7);
}

// Schlick-GGX geometry term (direct lighting k), Smith-combined for view + light.
fn geometry_smith(n_dot_v: f32, n_dot_l: f32, rough: f32) -> f32 {
    let r = rough + 1.0;
    let k = (r * r) / 8.0;
    let gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    let gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

// Fresnel-Schlick.
fn fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    return f0 + (vec3<f32>(1.0) - f0) * pow(clamp(1.0 - cos_theta, 0.0, 1.0), 5.0);
}

// Geometric/specular AA: turn screen-space normal variance into additional microfacet roughness. This
// suppresses the sub-pixel fireflies and crawling highlights that MSAA cannot touch, while retaining broad
// reflection shape. CAD uses a slightly tighter filter to preserve inspection detail.
fn specular_aa_roughness(perceptual_roughness: f32, n: vec3<f32>) -> f32 {
    let dn_dx = dpdx(n);
    let dn_dy = dpdy(n);
    let cad = clamp(cam.shadow.z, 0.0, 1.0);
    let variance_scale = mix(0.15, 0.10, cad);
    let threshold = mix(0.18, 0.12, cad);
    let variance = variance_scale * (dot(dn_dx, dn_dx) + dot(dn_dy, dn_dy));
    let kernel_roughness2 = min(2.0 * variance, threshold);
    return clamp(sqrt(perceptual_roughness * perceptual_roughness + kernel_roughness2), 0.045, 1.0);
}

fn specular_ambient_occlusion(n_dot_v: f32, ao: f32, roughness: f32) -> f32 {
    let exponent = exp2(-16.0 * roughness - 1.0);
    return clamp(pow(n_dot_v + ao, exponent) - 1.0 + ao, 0.0, 1.0);
}

// One light's Cook-Torrance contribution. `l` = unit direction TO the light; `radiance` = colour·intensity
// (already attenuated). Energy-conserving Lambert diffuse + GGX/Smith/Fresnel specular; metals have no diffuse.
fn light_contrib(
    n: vec3<f32>, v: vec3<f32>, base: vec3<f32>, metallic: f32, roughness: f32, f0: vec3<f32>,
    energy_compensation: vec3<f32>, l: vec3<f32>, radiance: vec3<f32>,
) -> vec3<f32> {
    let h = normalize(v + l);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let v_dot_h = max(dot(v, h), 0.0);
    let f = fresnel_schlick(v_dot_h, f0);
    let ndf = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);
    let specular = ((ndf * g * f) / max(4.0 * n_dot_v * n_dot_l, 1e-4)) * energy_compensation;
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = kd * base / PI;
    return (diffuse + specular) * radiance * n_dot_l;
}

// M11.2 follow-up — tangent-space normal mapping WITHOUT precomputed tangents (Schüler's cotangent frame):
// build the TBN from screen-space derivatives of world position + UV, then rotate the sampled normal into
// world space. Degenerate UV (untextured meshes have a constant UV → zero gradient) falls back to the
// geometric normal, avoiding a NaN from inverseSqrt(0).
fn perturb_normal(n: vec3<f32>, tangent: vec4<f32>, world_pos: vec3<f32>, uv: vec2<f32>, map: vec3<f32>) -> vec3<f32> {
    if (dot(tangent.xyz, tangent.xyz) > 0.25) {
        // Gram-Schmidt removes interpolation drift. The sign is the MikkTSpace handedness emitted by
        // the same compiler that produced the baked normal map.
        let t = normalize(tangent.xyz - n * dot(n, tangent.xyz));
        let sign = select(-1.0, 1.0, tangent.w >= 0.0);
        let b = cross(n, t) * sign;
        return normalize(mat3x3<f32>(t, b, n) * map);
    }
    let dp1 = dpdx(world_pos);
    let dp2 = dpdy(world_pos);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    let b = dp2perp * duv1.y + dp1perp * duv2.y;
    let m = max(dot(t, t), dot(b, b));
    if (m < 1e-12) {
        return n; // no UV gradient → the geometric normal (the flat-normal dummy also lands here harmlessly)
    }
    let invmax = inverseSqrt(m);
    let tbn = mat3x3<f32>(t * invmax, b * invmax, n);
    return normalize(tbn * map);
}

@fragment
fn fs_mesh(in: MeshVsOut) -> @location(0) vec4<f32> {
    // M11.2 follow-up — albedo = the base-color TEXTURE × the baked/override factor. An untextured mesh
    // binds a 1×1 white dummy, so this is the factor unchanged (identical to the prior flat shading).
    //
    // COLOUR INGRESS #1. The texel arrives already decoded to linear light — by the hardware sRGB unit
    // when the policy said this is an sRGB colour texture, or untouched when it said data — and the
    // factor is linear Rec.709 by the glTF specification ("COLOR_0 ... acts as an additional linear
    // multiplier to base color"). Both are Rec.709, so ONE conversion covers the product, and it
    // happens here: before F0, before the BRDF, before anything reads a channel.
    let base = to_working(textureSample(base_color_tex, base_color_samp, in.uv).rgb * in.base_color);
    // M11.2 follow-up — multiply the baked metallic/roughness factors by the MR map (glTF packing:
    // roughness=G, metalness=B); an untextured mesh binds a white dummy (×1 → unchanged).
    let mr_s = textureSample(mr_tex, base_color_samp, in.uv);
    let metallic = clamp(in.mr.x * mr_s.b, 0.0, 1.0);
    var roughness = clamp(in.mr.y * mr_s.g, 0.04, 1.0); // floor avoids a singular mirror highlight

    // M11.2 follow-up — perturb the geometric normal by the tangent-space normal map (flat dummy → no-op).
    let geo_n = normalize(in.world_normal);
    let nmap = textureSample(normal_tex, base_color_samp, in.uv).rgb * 2.0 - 1.0;
    let n = perturb_normal(geo_n, in.world_tangent, in.world_pos, in.uv, nmap);
    roughness = specular_aa_roughness(roughness, n);
    let cam_eye = cam.focus.yzw; // packed in the Camera uniform's spare slot
    let v = normalize(cam_eye - in.world_pos);
    // F0: dielectric 0.04, lerped toward the base color as the surface becomes metallic.
    let f0 = mix(vec3<f32>(0.04), base, metallic);
    let n_dot_v_amb = max(dot(n, v), 1e-4);
    let brdf = textureSampleLevel(brdf_lut, lut_samp, vec2<f32>(n_dot_v_amb, roughness), 0.0).rg;
    // The LUT's A+B response is the GGX directional albedo for F0=1. Reuse it to restore the energy lost
    // by a single-scattering microfacet BRDF (white-furnace consistency), for direct and image lighting.
    let directional_albedo = max(brdf.x + brdf.y, 0.08);
    let energy_compensation = min(
        vec3<f32>(4.0),
        vec3<f32>(1.0) + f0 * (1.0 / directional_albedo - 1.0),
    );

    // M11.3 — accumulate every authored light (directional/point/spot). The list is never empty (render.rs
    // falls back to a default key light), so an unlit scene still renders.
    var lo = vec3<f32>(0.0);
    let count = arrayLength(&lights);
    for (var i = 0u; i < count; i = i + 1u) {
        let lt = lights[i];
        let kind = lt.pos_kind.w;
        var l: vec3<f32>;
        var atten = 1.0;
        if (kind < 0.5) {
            l = normalize(-lt.dir_range.xyz); // directional: toward the light = -shine direction
        } else {
            let to_light = lt.pos_kind.xyz - in.world_pos;
            let dist = max(length(to_light), 1e-4);
            l = to_light / dist;
            atten = 1.0 / (dist * dist); // physical inverse-square
            let range = lt.dir_range.w;
            if (range > 0.0) {
                let win = clamp(1.0 - pow(dist / range, 4.0), 0.0, 1.0);
                atten = atten * win * win; // smooth range cutoff
            }
            if (kind > 1.5) { // spot cone: narrow by the angle to the shine axis
                let cd = dot(normalize(lt.dir_range.xyz), -l);
                atten = atten * clamp((cd - 0.8) / 0.12, 0.0, 1.0);
            }
        }
        // M11.3 inc.3 — the single shadow map was rendered for ONE caster (cam.shadow.x). Apply it to that
        // light only, so other directional lights (which have no map) aren't falsely shadowed. Point/spot
        // are never shadowed (single directional caster).
        var shadow = 1.0;
        if (kind < 0.5 && cam.shadow.x >= 0.0 && f32(i) == cam.shadow.x) {
            shadow = shadow_factor(in.world_pos, dot(n, l));
        }
        // COLOUR INGRESS #2 — the light's own colour. An authored light is linear Rec.709; the
        // intensity, attenuation and shadow terms are scalars and are space-agnostic.
        let radiance = to_working(lt.color_intensity.xyz) * lt.color_intensity.w * atten * shadow;
        lo = lo + light_contrib(
            n, v, base, metallic, roughness, f0, energy_compensation, l, radiance,
        );
    }

    // ── image-based ambient, with MULTIPLE-SCATTERING energy compensation ────────────────────────────
    //
    // Fdez-Aguera, "A Multiple-Scattering Microfacet Model for Real-Time Image Based Lighting"
    // (JCGT 8.1, 2019). A single-scattering split-sum drops the energy that bounces more than once
    // between microfacets, and drops more of it the rougher the surface gets. Two things follow, and the
    // renderer previously did neither: that lost energy has to come back (`fms_ems`), and whatever the
    // specular lobe did NOT take has to be handed to the diffuse lobe rather than assumed away. Without
    // it, rough metal — which is most of a factory — renders dull and grey no matter how bright and
    // well-shaped the environment lighting it is.
    let max_mip = f32(textureNumLevels(env) - 1);
    let refl = reflect(-v, n);
    // COLOUR INGRESS #3 — environment radiance, diffuse and specular.
    //
    // Converted at the point of SAMPLING rather than baked into the map before convolution, and that is
    // exact, not a shortcut: a primaries conversion is a linear operator, so it commutes with the
    // weighted sums that mip filtering and the irradiance/prefilter convolution are. Sampling-time
    // conversion therefore gives bit-comparable results to converting the source first, while costing
    // no re-convolution when the working space changes and no precision loss from re-encoding an HDR
    // panorama into another gamut.
    let irradiance = env_to_working(sh_irradiance(n));
    // `env` is GGX-PREFILTERED per level (see ibl.rs), so this fetch is the environment already convolved
    // with the lobe for this roughness — which is what the split-sum approximation assumes it samples.
    let prefiltered = env_to_working(
        textureSampleLevel(env, env_samp, dir_to_equirect(refl), roughness * max_mip).rgb,
    );

    // Single-scattering specular: the split-sum (scale, bias) against a roughness-aware Fresnel.
    let ks = fresnel_schlick_roughness(n_dot_v_amb, f0, roughness);
    let fss_ess = ks * brdf.x + brdf.y;
    // The fraction of energy single scattering failed to account for.
    let ems = max(1.0 - (brdf.x + brdf.y), 0.0);
    // Average Fresnel over the hemisphere, which sums the multiple-bounce geometric series.
    let f_avg = f0 + (vec3<f32>(1.0) - f0) / 21.0;
    let fms_ems = ems * fss_ess * f_avg
        / max(vec3<f32>(1.0) - f_avg * ems, vec3<f32>(1e-4));
    // What neither scattering path took is what the diffuse lobe may have. Metals have no diffuse albedo.
    let c_diff = base * (1.0 - metallic);
    let k_d = c_diff * max(vec3<f32>(1.0) - fss_ess - fms_ems, vec3<f32>(0.0));

    let horizon = clamp(1.0 + dot(refl, geo_n), 0.0, 1.0);
    // AO is visibility for indirect light. Missing maps bind a white dummy, preserving prior output.
    let ao = clamp(textureSample(ao_tex, base_color_samp, in.uv).r, 0.0, 1.0);
    let specular_ao = specular_ambient_occlusion(n_dot_v_amb, ao, roughness);
    // `energy_compensation` is NOT applied here: it is the direct-light multiple-scattering term, and
    // `fms_ems` is the image-lighting one. Applying both to the same lobe would count that energy twice.
    let specular_ibl = fss_ess * prefiltered * specular_ao * horizon * horizon;
    let diffuse_ibl = (fms_ems + k_d) * irradiance * ao;
    // SCENE LINEAR radiance, straight into the HDR attachment. No exposure, no tone curve, no OETF.
    var col = diffuse_ibl + specular_ibl + lo;

    // CAD inspection gets a restrained silhouette cue; it clarifies coincident curved bodies without
    // painting over face colors. A multiplicative darkening is space-agnostic, so it is unchanged.
    if (cam.shadow.z > 0.5) {
        col = col * (1.0 - 0.12 * pow(1.0 - n_dot_v_amb, 3.0));
    }
    // Selection is a subtle wash plus a strong rim, so selected materials remain inspectable. The tint is
    // an AUTHORED colour, converted once so it renders back as the same yellow the UI shows; the blend
    // itself now happens in linear light, which is what keeps the rim from banding against bright metal.
    if (is_selected(in.highlight)) {
        let rim = smoothstep(0.12, 0.72, pow(1.0 - n_dot_v_amb, 2.2));
        col = mix(col, unlit_srgb_to_scene_linear(SELECTION_TINT), 0.08 + 0.52 * rim);
    }
    col = apply_focus_dim_linear(col, is_selected(in.highlight));
    // HOVER IS LAST, and that ordering is the rule: it is applied after the selection wash so a part
    // you point at inside something already selected still answers, and after the focus dim so the
    // answer is not itself greyed out. The wash is heavier than the selection's (0.22 vs 0.08) because
    // it has to be legible at a glance on ONE part of a 15 711-part import, where the selection has a
    // gizmo and an outliner row saying the same thing.
    if (is_hovered(in.highlight)) {
        let rim = smoothstep(0.05, 0.65, pow(1.0 - n_dot_v_amb, 1.6));
        col = mix(col, unlit_srgb_to_scene_linear(HOVER_TINT), 0.22 + 0.50 * rim);
    }
    return vec4<f32>(col, 1.0);
}

// The unlit fragment outputs. Both paths compose their colour in AUTHORED sRGB space (a shade multiply, a
// selection mix, a focus dim) exactly as they always did, and this is the single point where the result
// crosses into scene-linear HDR — they differ only in which exposure that conversion is anchored to.

/// Tracking lines, the contact-debugger overlay, light/camera markers, the gizmo, the snap ghost and the
/// pipe preview: CHROME. Constant on screen at any exposure.
@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(unlit_srgb_to_scene_linear(in.color), 1.0);
}

/// The placeholder cubes: CONTENT. Responds to exposure like every other object in the frame.
@fragment
fn fs_cube(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(scene_srgb_to_scene_linear(in.color), 1.0);
}

// M11.3 inc.2 — skybox. One oversized triangle covers the screen; the fragment reconstructs the world-space
// view ray from the inverse view-proj and samples the env (mip 0). Drawn first, depth-write off, so the
// grid + meshes draw in front. The env backdrop is also exactly what the meshes' specular IBL reflects.
struct SkyOut { @builtin(position) pos: vec4<f32>, @location(0) ndc: vec2<f32> };

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    var corners = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0),
    );
    let xy = corners[vi];
    var out: SkyOut;
    out.pos = vec4<f32>(xy, 1.0, 1.0); // z = w ⇒ depth 1.0 (far plane)
    out.ndc = xy;
    return out;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    // Unproject two points along the pixel's ray (wgpu NDC z ∈ [0,1]); their difference is the view dir.
    let near = cam.inv_view_proj * vec4<f32>(in.ndc, 0.0, 1.0);
    let far = cam.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(far.xyz / far.w - near.xyz / near.w);
    // The env map is already linear HDR radiance — it goes into the HDR attachment untouched. This is the
    // backdrop the meshes' specular IBL reflects, so it MUST stay on the same scale as their lighting.
    let hdr = textureSampleLevel(env, env_samp, dir_to_equirect(dir), 0.0).rgb;
    // Converted on exactly the same terms as the specular reflection of it in fs_mesh — if the backdrop
    // and the reflection of the backdrop disagreed about the working space, a mirror would not match
    // the sky behind it.
    return vec4<f32>(env_to_working(hdr), 1.0);
}

// M11.3 inc.3 — depth-only shadow pass: render the same cube + mesh geometry from the light's POV into the
// shadow map. Identical world transform to vs_cube / vs_mesh, but projected by `light_view_proj`. No
// fragment stage (depth only). Group 0 = camera (for light_view_proj), group 1 = instances.
@vertex
fn vs_cube_shadow(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> @builtin(position) vec4<f32> {
    let inst = instances[ii];
    let world = inst.center + quat_rotate(inst.rotation, corner(vi) * inst.scale);
    return cam.light_view_proj * vec4<f32>(world, 1.0);
}

@vertex
fn vs_mesh_shadow(v: MeshIn, @builtin(instance_index) ii: u32) -> @builtin(position) vec4<f32> {
    let inst = instances[ii];
    let world = inst.center + quat_rotate(inst.rotation, v.position * inst.scale);
    return cam.light_view_proj * vec4<f32>(world, 1.0);
}
