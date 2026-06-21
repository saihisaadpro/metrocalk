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
struct Instance { center: vec3<f32>, scale: f32, color: vec3<f32>, selected: f32, rotation: vec4<f32> };
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

// Imported meshes (M4 asset pipeline). A real vertex stream (position/normal/baked material color)
// drawn instanced — `instances[ii]` carries the entity's centre, render scale, and selection flag
// (same Instance storage layout as the cubes; the cube `color` field is ignored here, the mesh uses
// its own baked vertex color). Non-bindless: one vertex/index buffer bound per asset (ADR-003).
struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

@vertex
fn vs_mesh(v: MeshIn, @builtin(instance_index) ii: u32) -> VsOut {
    let inst = instances[ii];
    let world = inst.center + quat_rotate(inst.rotation, v.position * inst.scale);
    var out: VsOut;
    out.pos = cam.view_proj * vec4<f32>(world, 1.0);
    let nrm = quat_rotate(inst.rotation, normalize(v.normal));
    let shade = 0.55 + 0.45 * clamp(dot(nrm, normalize(vec3<f32>(0.4, 0.8, 0.3))), 0.0, 1.0);
    var col = v.color * shade;
    if (inst.selected > 0.5) {
        col = mix(col, vec3<f32>(1.0, 0.85, 0.2), 0.7); // selection highlight
    }
    out.color = apply_focus_dim(col, inst.selected > 0.5);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
