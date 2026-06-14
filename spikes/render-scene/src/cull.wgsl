// M2.2 GPU frustum culling: one compute thread per instance; survivors are compacted into
// visible[] and the count is atomically accumulated into `counter` (later copied into the indirect
// draw-args). Separate module from shader.wgsl so `visible` can be read_write here (compute) and
// read-only there (vertex) — WebGPU forbids read_write storage in the vertex stage.

struct Camera {
    view_proj: mat4x4<f32>,
    planes: array<vec4<f32>, 6>,   // world-space frustum planes (normalized, pointing inward)
};
@group(0) @binding(0) var<uniform> cam: Camera;

struct Instance {
    center: vec3<f32>,
    radius: f32,
    color: vec3<f32>,
    scale: f32,
};

@group(1) @binding(0) var<storage, read>       instances: array<Instance>;
@group(1) @binding(1) var<storage, read_write> visible: array<u32>;
@group(1) @binding(2) var<storage, read_write> counter: atomic<u32>;

@compute @workgroup_size(64)
fn cull(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&instances)) {
        return;
    }
    let inst = instances[i];
    // Bounding-sphere vs 6 planes: outside if any signed distance < -radius.
    for (var p = 0u; p < 6u; p = p + 1u) {
        let pl = cam.planes[p];
        if (dot(pl.xyz, inst.center) + pl.w + inst.radius < 0.0) {
            return;
        }
    }
    let slot = atomicAdd(&counter, 1u);
    visible[slot] = i;
}
