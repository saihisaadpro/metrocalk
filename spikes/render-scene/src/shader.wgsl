// M2.2 render module: instanced cubes + per-entity gizmos + static grid. The cull compute lives in
// cull.wgsl (separate module — see the read_write/vertex-stage note there). Pipelines select by
// entry-point name; the cubes/gizmos read the compacted visible[] list by instance_index, so
// first_instance stays 0 (no `indirect-first-instance` needed).

struct Camera {
    view_proj: mat4x4<f32>,
    planes: array<vec4<f32>, 6>,
};
@group(0) @binding(0) var<uniform> cam: Camera;

struct Instance {
    center: vec3<f32>,
    radius: f32,
    color: vec3<f32>,
    scale: f32,
};
@group(1) @binding(0) var<storage, read> instances: array<Instance>;
@group(1) @binding(1) var<storage, read> visible: array<u32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) color: vec3<f32>,
};

// 8 cube corners, indexed by the dereferenced index value (corner ID 0..7).
fn corner(id: u32) -> vec3<f32> {
    return vec3<f32>(
        select(-1.0, 1.0, (id & 1u) != 0u),
        select(-1.0, 1.0, (id & 2u) != 0u),
        select(-1.0, 1.0, (id & 4u) != 0u),
    );
}

// ---- instanced cubes ------------------------------------------------------------------------
@vertex
fn vs_cube(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[visible[ii]];
    let local = corner(vi);
    let world = inst.center + local * inst.scale;
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    let shade = 0.55 + 0.45 * clamp(dot(normalize(local), normalize(vec3<f32>(0.4, 0.8, 0.3))), 0.0, 1.0);
    out.color = inst.color * shade;
    return out;
}

// ---- instanced gizmos (3-axis cross, LineList, 6 verts) -------------------------------------
@vertex
fn vs_gizmo(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[visible[ii]];
    let axis = vi / 2u;
    let tip = f32(vi % 2u);
    var dir = vec3<f32>(0.0, 0.0, 0.0);
    if (axis == 0u) { dir.x = 1.0; } else if (axis == 1u) { dir.y = 1.0; } else { dir.z = 1.0; }
    let world = inst.center + dir * (inst.scale * 1.9) * tip;
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    out.color = dir;
    return out;
}

// ---- static grid (render bundle, LineList) --------------------------------------------------
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
    out.color = vec3<f32>(0.20, 0.22, 0.28);
    return out;
}

@fragment
fn fs_solid(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
