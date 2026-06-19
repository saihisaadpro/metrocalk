// Editor viewport shader: instanced entity cubes (from the storage buffer the app fills from /core
// Transforms) + a ground grid. Selected entity highlights. Matches render.rs's Instance/Camera.

struct Camera { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> cam: Camera;

struct Instance { center: vec3<f32>, scale: f32, color: vec3<f32>, selected: f32 };
@group(1) @binding(0) var<storage, read> instances: array<Instance>;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec3<f32> };

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
    let world = inst.center + local * inst.scale;
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    let shade = 0.55 + 0.45 * clamp(dot(normalize(local), normalize(vec3<f32>(0.4, 0.8, 0.3))), 0.0, 1.0);
    var col = inst.color * shade;
    if (inst.selected > 0.5) {
        col = mix(col, vec3<f32>(1.0, 0.85, 0.2), 0.7); // selection highlight
    }
    out.color = col;
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
    out.color = vec3<f32>(0.60, 1.0, 0.93);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
