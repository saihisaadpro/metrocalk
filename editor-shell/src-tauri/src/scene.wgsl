// Editor viewport shader: instanced entity cubes (from the storage buffer the app fills from /core
// Transforms) + a ground grid. Selected entity highlights. Matches render.rs's Instance/Camera.

// `inv_view_proj` (M11.3 inc.2) lets the skybox turn a screen pixel into a world ray; `light_view_proj`
// (M11.3 inc.3) is the shadow-casting light's ortho view-proj, for both the depth pass and fs_mesh's shadow
// lookup. The cube/grid/line shaders ignore the trailing fields. Field order matches render.rs's `Camera`
// (view_proj · inv_view_proj · light_view_proj · focus) — 208 bytes, std140-clean.
struct Camera {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    focus: vec4<f32>,
    // M11.3 inc.3 — `shadow.x` = index of the shadow-casting directional light (-1 = none); the single
    // shadow map applies to ONLY that light, so other directionals (no map) stay unshadowed.
    shadow: vec4<f32>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

// M11.3 inc.3 (ADR-042) — the directional shadow map: a depth texture rendered from the shadow-casting
// light's POV, sampled with a comparison sampler (hardware PCF). Filled by the depth-only shadow pass before
// the main pass each frame; a render projection, never doc state. Shares GROUP 3 with the IBL env/LUT
// (bindings 4/5) because the device caps bind groups at 4 (web-portable) — and the shadow pass doesn't bind
// group 3, so there's no render-target-vs-sampled conflict.
@group(3) @binding(4) var shadow_map: texture_depth_2d;
@group(3) @binding(5) var shadow_samp: sampler_comparison;

// Fraction of the directional light reaching `world_pos` (1 = fully lit, 0 = fully shadowed). Projects into
// the light's clip space, does a 3×3 PCF compare with a slope-scaled depth bias. Anything outside the
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
    let bias = clamp(0.0016 * tan(acos(clamp(n_dot_l, 0.0, 1.0))), 0.0006, 0.004);
    let ref_depth = ndc.z - bias;
    let dim = vec2<f32>(textureDimensions(shadow_map));
    let texel = 1.0 / dim;
    var sum = 0.0;
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let o = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSampleCompareLevel(shadow_map, shadow_samp, uv + o, ref_depth);
        }
    }
    return sum / 9.0;
}

// Focus mode (M3.3): when `cam.focus_active > 0.5`, every entity that isn't the focused/selected one
// is grayed toward the background so it reads as faded/transparent (depth-correct, no alpha blend).
// `is_focused` ⇒ the lit one; everything else dims. Returns the de-emphasized colour.
const DIM_TARGET = vec3<f32>(0.06, 0.07, 0.10); // the viewport clear colour — fade toward "gone"
fn apply_focus_dim(col: vec3<f32>, is_focused: bool) -> vec3<f32> {
    if (cam.focus.x > 0.5 && !is_focused) {
        return mix(col, DIM_TARGET, 0.86);
    }
    return col;
}

// `rotation` is a unit quaternion (x,y,z,w); identity = (0,0,0,1). Applied per-instance so a tumbling
// physics body / a rotated authored Transform / a posed part actually *looks* rotated (M9.1+ — the shared
// renderer-rotation path). Matches render.rs's Instance (48 bytes, std430-clean).
// `material` (M11.2) = per-entity PBR override [metallic, roughness, has_override, _]; when has_override>0.5
// the mesh path uses it (+ `color` as the override base color) instead of the asset's baked vertex material.
struct Instance { center: vec3<f32>, scale: f32, color: vec3<f32>, selected: f32, rotation: vec4<f32>, material: vec4<f32> };
@group(1) @binding(0) var<storage, read> instances: array<Instance>;
// M11.2 follow-up — the per-mesh base-color (albedo) texture rides the already-per-mesh instance group
// (group 1), staying within the 4-bind-group cap. An untextured mesh binds a 1×1 WHITE dummy, so sampling
// is always valid (white × the baked factor = the factor → an untextured mesh looks exactly as before).
// Only `fs_mesh` references these; the cube/grid/line pipelines' group-1 layout omits them (unused = fine).
@group(1) @binding(1) var base_color_tex: texture_2d<f32>;
@group(1) @binding(2) var base_color_samp: sampler;

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
    if (inst.selected > 0.5) {
        col = mix(col, vec3<f32>(1.0, 0.85, 0.2), 0.7); // selection highlight
    }
    out.color = apply_focus_dim(col, inst.selected > 0.5);
    return out;
}

const GRID_N: u32 = 40u;
const GRID_HALF: f32 = 40.0;

@vertex
fn vs_grid(@builtin(vertex_index) vi: u32) -> VsOut {
    let line = vi / 2u;
    let endp = vi % 2u;
    let lines_per_dir = GRID_N + 1u;
    let span = GRID_HALF * 2.0;
    var world: vec3<f32>;
    if (line < lines_per_dir) {
        let f = f32(line) / f32(GRID_N);
        let x = -GRID_HALF + f * span;
        let z = select(-GRID_HALF, GRID_HALF, endp == 1u);
        world = vec3<f32>(x, 0.0, z);
    } else {
        let f = f32(line - lines_per_dir) / f32(GRID_N);
        let z = -GRID_HALF + f * span;
        let x = select(-GRID_HALF, GRID_HALF, endp == 1u);
        world = vec3<f32>(x, 0.0, z);
    }
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    out.color = vec3<f32>(0.18, 0.20, 0.26);
    return out;
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
};

struct MeshVsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) base_color: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) mr: vec2<f32>,       // metallic, roughness
    @location(4) selected: f32,
    @location(5) uv: vec2<f32>,
};

@vertex
fn vs_mesh(v: MeshIn, @builtin(instance_index) ii: u32) -> MeshVsOut {
    let inst = instances[ii];
    let world = inst.center + quat_rotate(inst.rotation, v.position * inst.scale);
    var out: MeshVsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    out.world_pos = world;
    out.world_normal = quat_rotate(inst.rotation, normalize(v.normal));
    out.selected = inst.selected;
    // Per-entity material override (M11.2): a "make it metal/rusty/gold" intent recolors ONLY this entity;
    // absent → the asset's baked vertex material.
    let has_override = inst.material.z > 0.5;
    out.base_color = select(v.color, inst.color, has_override);
    out.mr = select(vec2<f32>(v.metallic, v.roughness), inst.material.xy, has_override);
    out.uv = v.uv;
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

// M11.3 inc.2 — the lit/IBL paths work in LINEAR HDR (the sun + bright reflections exceed 1.0), but the
// swapchain is a NON-sRGB (linear-store) format, so the shader must both compress the highlights and
// encode to sRGB itself. ACES filmic (Narkowicz fit) + a 2.2 gamma — applied to the mesh + sky outputs.
fn tonemap_aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}
fn to_srgb(x: vec3<f32>) -> vec3<f32> {
    return pow(x, vec3<f32>(1.0 / 2.2));
}
fn display_encode(hdr: vec3<f32>) -> vec3<f32> {
    return to_srgb(tonemap_aces(hdr));
}

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

// One light's Cook-Torrance contribution. `l` = unit direction TO the light; `radiance` = colour·intensity
// (already attenuated). Energy-conserving Lambert diffuse + GGX/Smith/Fresnel specular; metals have no diffuse.
fn light_contrib(
    n: vec3<f32>, v: vec3<f32>, base: vec3<f32>, metallic: f32, roughness: f32, f0: vec3<f32>,
    l: vec3<f32>, radiance: vec3<f32>,
) -> vec3<f32> {
    let h = normalize(v + l);
    let n_dot_l = max(dot(n, l), 0.0);
    let n_dot_v = max(dot(n, v), 1e-4);
    let n_dot_h = max(dot(n, h), 0.0);
    let v_dot_h = max(dot(v, h), 0.0);
    let f = fresnel_schlick(v_dot_h, f0);
    let ndf = distribution_ggx(n_dot_h, roughness);
    let g = geometry_smith(n_dot_v, n_dot_l, roughness);
    let specular = (ndf * g * f) / max(4.0 * n_dot_v * n_dot_l, 1e-4);
    let kd = (vec3<f32>(1.0) - f) * (1.0 - metallic);
    let diffuse = kd * base / PI;
    return (diffuse + specular) * radiance * n_dot_l;
}

@fragment
fn fs_mesh(in: MeshVsOut) -> @location(0) vec4<f32> {
    // M11.2 follow-up — albedo = the base-color TEXTURE × the baked/override factor. An untextured mesh
    // binds a 1×1 white dummy, so this is the factor unchanged (identical to the prior flat shading).
    let base = textureSample(base_color_tex, base_color_samp, in.uv).rgb * in.base_color;
    let metallic = clamp(in.mr.x, 0.0, 1.0);
    let roughness = clamp(in.mr.y, 0.04, 1.0); // floor avoids a singular mirror highlight

    let n = normalize(in.world_normal);
    let cam_eye = cam.focus.yzw; // packed in the Camera uniform's spare slot
    let v = normalize(cam_eye - in.world_pos);
    // F0: dielectric 0.04, lerped toward the base color as the surface becomes metallic.
    let f0 = mix(vec3<f32>(0.04), base, metallic);

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
        let radiance = lt.color_intensity.xyz * lt.color_intensity.w * atten * shadow;
        lo = lo + light_contrib(n, v, base, metallic, roughness, f0, l, radiance);
    }

    // M11.3 inc.2 — image-based ambient (replaces the flat fill): diffuse from a blurred env mip (a cheap
    // irradiance) + specular from the roughness-matched env mip × the split-sum BRDF LUT. THIS is what
    // gives a metal something to reflect, so chrome/gold are no longer near-black (the M11.2 dark-metal).
    let n_dot_v_amb = max(dot(n, v), 1e-4);
    let max_mip = f32(textureNumLevels(env) - 1);
    let f_amb = fresnel_schlick_roughness(n_dot_v_amb, f0, roughness);
    let kd_amb = (vec3<f32>(1.0) - f_amb) * (1.0 - metallic);
    let irradiance = textureSampleLevel(env, env_samp, dir_to_equirect(n), max(max_mip - 2.0, 0.0)).rgb;
    let diffuse_ibl = kd_amb * irradiance * base;
    let refl = reflect(-v, n);
    let prefiltered = textureSampleLevel(env, env_samp, dir_to_equirect(refl), roughness * max_mip).rgb;
    let brdf = textureSampleLevel(brdf_lut, lut_samp, vec2<f32>(n_dot_v_amb, roughness), 0.0).rg;
    let specular_ibl = prefiltered * (f_amb * brdf.x + brdf.y);
    // Linear HDR → tonemapped, sRGB-encoded display colour (the swapchain is linear-store).
    var col = display_encode(diffuse_ibl + specular_ibl + lo);

    // Editor overlays applied AFTER shading, in display space: selection highlight + focus-dim.
    if (in.selected > 0.5) {
        col = mix(col, vec3<f32>(1.0, 0.85, 0.2), 0.55);
    }
    col = apply_focus_dim(col, in.selected > 0.5);
    return vec4<f32>(col, 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
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
    let hdr = textureSampleLevel(env, env_samp, dir_to_equirect(dir), 0.0).rgb;
    return vec4<f32>(display_encode(hdr), 1.0);
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
