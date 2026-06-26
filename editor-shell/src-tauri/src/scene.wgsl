// Editor viewport shader: instanced entity cubes (from the storage buffer the app fills from /core
// Transforms) + a ground grid. Selected entity highlights. Matches render.rs's Instance/Camera.

struct Camera { view_proj: mat4x4<f32>, focus: vec4<f32> };
@group(0) @binding(0) var<uniform> cam: Camera;

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
};

struct MeshVsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) base_color: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) world_normal: vec3<f32>,
    @location(3) mr: vec2<f32>,       // metallic, roughness
    @location(4) selected: f32,
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
    return out;
}

const PI = 3.14159265359;
const AMBIENT = 0.35; // fill so an UNLIT matte face isn't ~3x dimmer than the prior Lambert (M11.2 review fix)

// M11.3 (ADR-042) — the scene's authored lights (group 2), looped per fragment. Matches render.rs's
// `LightGpu`. `pos_kind.w`: 0=directional, 1=point, 2=spot. Directional/spot SHINE along `dir_range.xyz`;
// point/spot sit at `pos_kind.xyz` with `dir_range.w` = range falloff. `color_intensity` = linear RGB·intensity.
struct Light {
    pos_kind: vec4<f32>,
    color_intensity: vec4<f32>,
    dir_range: vec4<f32>,
};
@group(2) @binding(0) var<storage, read> lights: array<Light>;

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
    let base = in.base_color;
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
        let radiance = lt.color_intensity.xyz * lt.color_intensity.w * atten;
        lo = lo + light_contrib(n, v, base, metallic, roughness, f0, l, radiance);
    }

    // Ambient fill (metals get less, having no diffuse) so unlit faces aren't near-black (no IBL yet, M11.3 inc.2).
    let ambient = base * (1.0 - metallic * 0.6) * AMBIENT;
    var col = ambient + lo;

    // Editor overlays applied AFTER shading: selection highlight + focus-dim.
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
